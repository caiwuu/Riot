//! 宿主的共享状态。
//!
//! `Clone` 是浅拷贝（内部全是 `Arc`），因为 Tauri 的退出钩子拿不到
//! `State<'_, T>` 的所有权，只能克隆一份出来。
//!
//! # 拆进程后的职责划分(ARCHITECTURE.md §2.2)
//!
//! 宿主是**会话注册表与设置的权威**:id、项目根、标题、权限模式、采样等
//! 全在这里(内存 + index.json),这些操作纯本地、不依赖内核活着。
//! 内核那边的活会话只是**运行时投影**——首次要跑轮子/拿历史时按需水合
//! (session.resume,幂等),会话设置随每轮 TurnConfig 传过去。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use tauri::ipc::Channel;
use tokio::sync::Mutex;

use riot_protocol::event::AgentEvent;
use riot_protocol::id::{IdGenerator, NanoIdGenerator, SessionId};
use riot_protocol::message::Message;
use riot_protocol::permission::{PendingAsk, PermissionMode, PermissionResponse};
use riot_protocol::rpc::{RpcRequest, RpcResponse};

use crate::config::AppConfig;
use crate::fence::Fence;
use crate::kernel::{HostNotice, KernelClient};
use crate::{HostError, HostResult};

fn sid(id: &str) -> SessionId {
    SessionId::from_raw(id.to_owned())
}

/// 切回一个会话时前端要的东西。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryOut {
    pub messages: Vec<Message>,
    /// 压缩边界之前的消息。模型看不见，界面画在分割线上面。
    pub archived: Vec<Message>,
    /// 此刻有没有轮子在跑。决定界面显示停止键还是发送键。
    pub busy: bool,
    /// 此刻是否在压缩。不回的话切回来只剩三个点，"正在压缩上下文"丢了。
    pub compacting: bool,
    /// 还在等用户回答的权限询问。`permission_request` 事件只发一次，
    /// 切走再切回时弹窗靠这份快照重建 —— 否则只能等超时被拒。
    pub pending_asks: Vec<PendingAsk>,
    /// 正在流式生成的正文。流式增量不进历史，切回来的界面靠它接着显示，
    /// 否则从 0 重新攒、缺头直到消息完成。
    pub live_text: String,
    /// 正在流式生成的思考。症状同上：思考块的字数清零重数。
    pub live_thinking: String,
}

/// 侧边栏需要知道的会话信息。**不含历史内容** —— 列表要快，内容按需拉。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    /// 这个会话绑定的项目根。创建后不会变。
    pub root: String,
    /// 第一条用户消息的开头；还没说过话就是 None。
    pub title: Option<String>,
    /// 创建顺序号，前端按它排序。HashMap 不保序，这里必须自带。
    pub seq: u64,
    /// 会话级采样覆盖。前端刷新后靠它恢复参数面板的显示。
    pub sampling: crate::config::Sampling,
    /// 当前权限模式。
    ///
    /// 和 `sampling` 同理，但漏掉它的后果重得多：前端原先拿全局默认值
    /// 当显示值，于是用户切到「全部放行」后，composer 一重挂载就退回显示
    /// 「每次询问」，而宿主这边仍然是放行的 —— 用户以为每步都会问他，
    /// 实际全部静默放行。**权限状态的显示必须以宿主为准。**
    pub mode: riot_protocol::permission::PermissionMode,
    /// 会话级思考策略。和 `sampling` 同理，前端刷新后靠它恢复显示。
    pub thinking: riot_protocol::ThinkingPolicy,
    /// 会话级 Python 虚拟环境（venv 根目录）。None = 用宿主默认环境。
    pub python_venv: Option<String>,
    /// 会话级追加的系统提示词。None = 只用内置提示词。
    pub system_prompt: Option<String>,
    /// 此刻有没有轮子在跑。侧栏靠它给后台忙碌的会话画指示点 ——
    /// 没订阅事件的会话，前端只有这一条途径知道它在干活。
    pub busy: bool,
}

/// 一个会话的事件出口，带上它是"第几次订阅"。
///
/// 光存 `Channel` 不够 —— 见 [`AppState::attach_sink`]，宿主没法从两个
/// 并发到达的订阅里看出谁更新。
/// 一次订阅的登记。channel 本身交给会话保管（见
/// [`crate::session::SessionSink`]），这里只留判新旧用的序号。
struct Sink {
    /// 前端给的单调递增序号。只用来比新旧，不解释具体数值。
    epoch: u64,
}

/// 注册表里的一个会话:UI 元数据 + 会话设置。**宿主是这些字段的权威。**
///
/// 拆进程后这里没有 `Session` 本体 —— 运行时(历史、轮子、队列)在内核
/// 进程里,设置随每轮 TurnConfig 传过去,内核不自己存。
struct Meta {
    /// 这个会话绑定的项目根。创建后不变。
    root: PathBuf,
    /// 创建顺序号，前端按它排序。
    seq: u64,
    /// 创建时刻（Unix 毫秒）。进索引。
    created_at_ms: u64,
    /// 手动改的名字。有它就不再自动起名。
    custom_title: Option<String>,
    /// 第一句话起的名字。
    auto_title: Option<String>,
    sampling: crate::config::Sampling,
    mode: PermissionMode,
    python_venv: Option<String>,
    system_prompt: Option<String>,
    thinking: riot_protocol::ThinkingPolicy,
    /// 有没有轮子在跑(send_turn 置真,Done 事件清掉)。侧栏指示点用。
    busy: bool,
    /// 内核那边已经水合过这个会话。resume 是幂等的,这个标记只是省掉
    /// 每次操作前的一轮 RPC + 全量历史传输。
    hydrated: bool,
}

struct Inner {
    /// 配置文件的位置。
    ///
    /// `[约束]` 不要在 `AppState` 里直接调 [`crate::config::save`] ——
    /// 那个函数写的是真实路径，而 `AppState` 的方法是有单元测试的。
    /// 走这个字段，测试才能把它指到临时目录。
    ///
    /// 这条是踩出来的：`remove_project` 曾经直接调 `config::save`，
    /// 于是每跑一次 `cargo test`，开发机上的 `config.json` 就被测试里
    /// 那份空配置覆盖一次。
    config_path: PathBuf,
    /// 会话持久化目录（transcript JSONL + index.json）。从 `config_path`
    /// 推导 —— 同一条"测试能指到临时目录"的线。
    sessions_dir: PathBuf,
    /// transcript 只读入口。transcript 由**内核进程**写;宿主只在启动时
    /// 用它做索引重建(内核还没起,读安全),以及内核不在时删除兜底。
    transcripts: Arc<riot_store::Transcripts>,
    /// 内核 RPC 客户端。会话运行时全在它背后的内核进程里。
    kernel: KernelClient,
    /// session_id → 事件出口。同一会话重复订阅取 epoch 最大的那个 ——
    /// 一个会话在 UI 上只有一个视图。
    sinks: Mutex<HashMap<String, Sink>>,
    /// session_id → 登记的会话。
    ///
    /// [约束] 每个会话在创建时绑定自己的项目根，之后不变。没有全局
    /// "当前工作区"—— 那个概念上一版有过，后果是换目录后旧会话
    /// 继续往旧目录写文件。多项目并行下它根本没法定义清楚。
    sessions: Mutex<HashMap<String, Meta>>,
    config: Mutex<Option<AppConfig>>,
    seq: AtomicU64,
    /// 索引写盘互斥。快照和写文件必须在同一临界区：否则两次并发保存
    /// 可能让旧快照后落盘，一次变更就这么静默丢了。
    index_lock: Mutex<()>,
    /// 终端面板。应用级：终端跟着应用走不跟着会话走（见 term.rs）。
    /// M-B3 反向 RPC 后,内核的终端工具经宿主服务端操作同一个面板。
    terminals: crate::term::Terminals,
    /// 会话的面板浏览器(HostBrowser)。会话级、**宿主持有** —— 面板的
    /// screencast / 输入转发是宿主能力,浏览器进程完全归宿主。
    /// M-B3 反向 RPC 后,内核的浏览器工具经宿主服务端用同一个实例。
    browsers: Mutex<HashMap<String, Arc<crate::browser::access::HostBrowser>>>,
    /// 环境告警的去重表（env.snapshot 用）。见 `env_probe`。
    env_alerts: crate::env_probe::AlertSeen,
}

impl Inner {
    fn at(config_path: PathBuf) -> Self {
        let sessions_dir = config_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("sessions");
        let kernel_exe = crate::kernel::locate_kernel().unwrap_or_else(|e| {
            // 起不来的时候报错在 ensure_running(第一次真正用到内核时),
            // 这里只留日志 —— 纯本地操作(建会话、看列表)不该被它挡住。
            tracing::warn!("{e}");
            PathBuf::new()
        });
        Self {
            transcripts: Arc::new(riot_store::Transcripts::new(&sessions_dir)),
            kernel: KernelClient::new(kernel_exe, sessions_dir.clone()),
            sessions_dir,
            config_path,
            sinks: Mutex::default(),
            sessions: Mutex::default(),
            config: Mutex::default(),
            seq: AtomicU64::default(),
            index_lock: Mutex::default(),
            terminals: crate::term::Terminals::default(),
            browsers: Mutex::default(),
            env_alerts: crate::env_probe::AlertSeen::default(),
        }
    }
}

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

impl AppState {
    /// 启动入口：从磁盘恢复上次的会话表。
    pub fn restore() -> Self {
        Self::restore_at(crate::config::config_path())
    }

    /// 从指定配置路径恢复。路径参数化是为了能测（同 `config::load_at` 的
    /// 理由;pub 是给集成测试用 —— 它们在 tests/ 目录里是外部 crate）。
    ///
    /// 同步执行且不碰任何锁 —— 它跑在 Tauri runtime 起来之前。会话表在
    /// 构造 `Inner` 时整体塞入，历史留给各会话惰性水合。
    ///
    /// `[约束]` root 目录已经不存在的会话**照样恢复**。目录没了不等于对话
    /// 没价值 —— 历史仍然可读，工具会在使用时报真实错误；跳过它的话，
    /// 下一次索引落盘会把它永久抹掉，用户连历史都找不回。
    pub fn restore_at(config_path: PathBuf) -> Self {
        let inner = Inner::at(config_path);
        let index = crate::persist::load(&inner.sessions_dir, &inner.transcripts);

        let mut map = HashMap::new();
        let mut browsers_map = HashMap::new();
        let mut next_seq = 0u64;
        for p in index.sessions {
            next_seq = next_seq.max(p.seq + 1);
            if let Some(b) = make_browser(&inner.config_path, &p.id) {
                browsers_map.insert(p.id.clone(), b);
            }
            map.insert(
                p.id.clone(),
                Meta {
                    // 索引可能是修掉 verbatim 前缀之前写的，恢复时归一化，
                    // 让老会话回到真实项目的分组下。
                    root: crate::fence::strip_verbatim(PathBuf::from(&p.root)),
                    seq: p.seq,
                    created_at_ms: p.created_at_ms,
                    custom_title: p.custom_title,
                    auto_title: p.auto_title,
                    sampling: p.sampling,
                    mode: p.mode,
                    python_venv: p.python_venv,
                    system_prompt: p.system_prompt,
                    thinking: p.thinking,
                    busy: false,
                    hydrated: false,
                },
            );
        }
        if !map.is_empty() {
            tracing::info!(count = map.len(), "从磁盘恢复了 {} 个会话", map.len());
        }

        Self(Arc::new(Inner {
            sessions: Mutex::new(map),
            seq: AtomicU64::new(next_seq),
            browsers: Mutex::new(browsers_map),
            ..inner
        }))
    }

    /// 起一个任务消费内核事件里宿主也关心的那几件(busy / mode),并注入
    /// 反向请求(终端/浏览器)的处理端。
    /// Tauri setup 时调一次 —— `restore` 是同步的,不能在那里 spawn。
    ///
    /// `[约束]` 这里必须用 `tauri::async_runtime::spawn`,不能用裸
    /// `tokio::spawn`:setup 回调跑在 macOS 的 AppKit 主线程上,不在任何
    /// tokio runtime 的上下文里 —— 裸 spawn 会当场 panic
    /// (no reactor running),App 直接起不来。
    pub fn spawn_host_bridge(&self) {
        self.0
            .kernel
            .set_host_service(Arc::new(HostCalls(self.clone())));
        let Some(mut rx) = self.0.kernel.take_host_notices() else {
            return;
        };
        let state = self.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(n) = rx.recv().await {
                match n {
                    HostNotice::Done { session_id } => {
                        if let Some(m) = state.0.sessions.lock().await.get_mut(&session_id) {
                            m.busy = false;
                        }
                    }
                    HostNotice::ModeChanged { session_id, mode } => {
                        if let Some(m) = state.0.sessions.lock().await.get_mut(&session_id) {
                            m.mode = mode;
                        }
                        // 持久化:不记的话下一轮 TurnConfig 又把旧模式传回去。
                        state.persist_index().await;
                    }
                    HostNotice::PromptWithdrawn {
                        session_id,
                        session_empty,
                    } => {
                        if !session_empty {
                            continue;
                        }
                        // 自动标题是从这句话取的(send_turn 里定的),而它现在
                        // 回到输入框了。不撤的话侧边栏留着一句从没发出去的
                        // 话当名字,之后真正的第一句也改不动它 —— 标题只定
                        // 一次。手动改过名的不动:那是用户自己起的。
                        {
                            let mut g = state.0.sessions.lock().await;
                            match g.get_mut(&session_id) {
                                Some(m) if m.auto_title.is_some() => m.auto_title = None,
                                _ => continue,
                            }
                        }
                        state.persist_index().await;
                    }
                    HostNotice::KernelGone => {
                        // 重启后的内核是一张白纸:所有会话都要重新水合,
                        // 也没有任何轮子还在跑。
                        for m in state.0.sessions.lock().await.values_mut() {
                            m.hydrated = false;
                            m.busy = false;
                        }
                    }
                }
            }
        });
    }

    /// 发一个内核 RPC,错误统一转 [`HostError`]。
    async fn kernel_call(&self, req: RpcRequest) -> HostResult<RpcResponse> {
        self.0.kernel.call(req).await.map_err(HostError::from)
    }

    /// 确保内核那边有这个会话的活体(没有就从 transcript 水合)。
    /// 只对"要碰运行时"的操作调用;纯登记操作(改名、改设置)不需要。
    async fn ensure_hydrated(&self, session_id: &str) -> HostResult<()> {
        let (root, hydrated) = {
            let g = self.0.sessions.lock().await;
            let m = g.get(session_id).ok_or(HostError::NoSession)?;
            (m.root.clone(), m.hydrated)
        };
        if hydrated {
            return Ok(());
        }
        self.kernel_call(RpcRequest::SessionResume {
            session_id: sid(session_id),
            cwd: root,
        })
        .await?;
        if let Some(m) = self.0.sessions.lock().await.get_mut(session_id) {
            m.hydrated = true;
        }
        Ok(())
    }

    /// 配置和持久化都指向 `p` 所在目录而不是用户真实的配置目录。**只给测试用。**
    #[cfg(test)]
    fn with_config_path(p: PathBuf) -> Self {
        Self::restore_at(p)
    }

    /// 挂上事件出口。`epoch` 是前端的订阅序号，越大越新。
    ///
    /// `[约束]` **迟到的旧订阅必须丢弃，不能覆盖新的。**
    ///
    /// 前端一次挂载会连着发两次订阅（React StrictMode 在开发模式下把
    /// effect 跑两遍：挂载 → 卸载 → 再挂载），两个 `subscribe_session`
    /// 命令各自是一个 async 任务，谁先拿到锁**没有保证**。要是第一次
    /// 那个后落地，宿主就把出口指向了一个前端已经弃用的 channel。
    ///
    /// 那种状态没有任何报错:`send_turn` 查得到出口所以照常返回，整轮
    /// 也照常跑完、历史照常落盘，`Channel::send` 更不会失败（宿主无从
    /// 知道 JS 那头已经不听了）。用户看到的就是**发完消息永远转圈**，
    /// 切走再切回来却能看到完整回复。
    ///
    /// 返回 `false` 表示这次订阅因为过期被忽略了。
    pub async fn attach_sink(
        &self,
        session_id: String,
        epoch: u64,
        channel: Channel<AgentEvent>,
    ) -> bool {
        let mut g = self.0.sinks.lock().await;
        if let Some(cur) = g.get(&session_id)
            && cur.epoch > epoch
        {
            return false;
        }
        // `[约束]` 正在跑的轮子也要换到新 channel 上。
        //
        // 分发点在 KernelClient 的 sinks 表(事件从内核 stdout 流进来时
        // 现查),换表即换出口 —— 不换的话这一轮剩下的事件(包括结束)
        // 全发给没人听的旧 channel,界面就永远停在"它正在做事"。
        self.0.kernel.attach_sink(&session_id, channel).await;
        g.insert(session_id, Sink { epoch });
        true
    }

    pub async fn config(&self) -> AppConfig {
        let mut g = self.0.config.lock().await;
        g.get_or_insert_with(|| {
            let (mut c, backup) = crate::config::load_at(&self.0.config_path);
            if let Some(b) = backup {
                crate::config::note_recovered(b);
            }
            // 治历史脏数据：verbatim 前缀修掉之前，`\\?\D:\x` 被当成新项目
            // 写进过列表。归一化再去重，不然幽灵项目会一直躺在侧边栏。
            normalize_projects(&mut c.projects);
            c
        })
        .clone()
    }

    pub async fn set_config(&self, c: AppConfig) {
        *self.0.config.lock().await = Some(c);
    }

    /// 新建一个绑定 `root` 的会话。
    ///
    /// `root` 在这里过一遍围栏构造 —— 不存在、不是目录、或 canonicalize
    /// 失败都会在创建时报错，而不是等到第一次工具调用才炸。
    pub async fn create_session(&self, root: &str) -> HostResult<SessionInfo> {
        let fence = match Fence::new(root) {
            Ok(f) => f,
            Err(crate::fence::FenceError::Unresolvable { path, .. }) => {
                return Err(HostError::MissingProject(path.display().to_string()));
            }
            Err(e) => return Err(e.into()),
        };
        let root_str = fence.root().display().to_string();

        // 纯宿主操作:分配 id、登记、落索引。内核那边**不建** —— 第一次
        // send_turn / history 时按需水合(resume 对不存在的会话就是建)。
        // 这样建会话不依赖内核活着,内核崩溃重启也不丢注册表。
        let id = NanoIdGenerator.session_id();
        let seq = self.0.seq.fetch_add(1, Ordering::Relaxed);
        // 豁免理由：宿主层，持久化记录的是真实时刻，黄金回放不经过这里。
        #[allow(clippy::disallowed_methods)]
        let created_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let mode = self
            .config()
            .await
            .default_mode
            .unwrap_or(PermissionMode::Default);
        if let Some(b) = make_browser(&self.0.config_path, id.as_str()) {
            self.0
                .browsers
                .lock()
                .await
                .insert(id.as_str().to_owned(), b);
        }
        self.0.sessions.lock().await.insert(
            id.as_str().to_owned(),
            Meta {
                root: fence.root().to_path_buf(),
                seq,
                created_at_ms,
                custom_title: None,
                auto_title: None,
                sampling: crate::config::Sampling::default(),
                mode,
                python_venv: None,
                system_prompt: None,
                thinking: riot_protocol::ThinkingPolicy::default(),
                busy: false,
                hydrated: false,
            },
        );
        self.persist_index().await;

        Ok(SessionInfo {
            id: id.as_str().to_owned(),
            root: root_str,
            title: None,
            seq,
            sampling: crate::config::Sampling::default(),
            mode,
            thinking: riot_protocol::ThinkingPolicy::default(),
            python_venv: None,
            system_prompt: None,
            busy: false,
        })
    }

    /// 所有活着的会话。前端启动或刷新（HMR）后用它对齐状态。
    /// 纯登记表读取,不打内核。
    pub async fn list_sessions(&self) -> Vec<SessionInfo> {
        let mut out: Vec<SessionInfo> = self
            .0
            .sessions
            .lock()
            .await
            .iter()
            .map(|(id, m)| SessionInfo {
                id: id.clone(),
                root: m.root.display().to_string(),
                title: m.custom_title.clone().or_else(|| m.auto_title.clone()),
                seq: m.seq,
                sampling: m.sampling,
                mode: m.mode,
                thinking: m.thinking,
                python_venv: m.python_venv.clone(),
                system_prompt: m.system_prompt.clone(),
                busy: m.busy,
            })
            .collect();
        out.sort_by_key(|i| i.seq);
        out
    }

    /// 会话的对话历史，外加"此刻是否有轮子在跑"。
    ///
    /// 两样一起回：前端切回会话时要同时重建对话流和忙碌状态，分两次问
    /// 会在中间留一个窗口 —— 那一瞬间界面显示空闲，而模型正在干活。
    pub async fn history(&self, session_id: &str) -> HostResult<HistoryOut> {
        let root = {
            let g = self.0.sessions.lock().await;
            g.get(session_id).ok_or(HostError::NoSession)?.root.clone()
        };
        // resume 幂等:在内存直接回快照,不在就从 transcript 水合。
        let resp = self
            .kernel_call(RpcRequest::SessionResume {
                session_id: sid(session_id),
                cwd: root,
            })
            .await?;
        let RpcResponse::SessionResumed {
            messages,
            archived,
            busy,
            compacting,
            pending_asks,
            live_text,
            live_thinking,
        } = resp
        else {
            return Err(HostError::Kernel(crate::kernel::KernelError::Rpc(
                "session.resume 回了意外的应答".into(),
            )));
        };

        // 顺手做两件登记:标记已水合;索引里没有标题时从历史第一句自愈
        // (老版本索引、或索引损坏重建后会缺)。
        {
            let mut g = self.0.sessions.lock().await;
            if let Some(m) = g.get_mut(session_id) {
                m.hydrated = true;
                m.busy = busy;
                if m.custom_title.is_none() && m.auto_title.is_none() {
                    let first = messages.iter().find_map(|msg| match msg {
                        Message::User { content, .. } => content.iter().find_map(|c| match c {
                            riot_protocol::message::UserContent::Text { text } => {
                                crate::session::title_excerpt(text)
                            }
                            _ => None,
                        }),
                        _ => None,
                    });
                    if first.is_some() {
                        m.auto_title = first;
                        drop(g);
                        self.persist_index().await;
                    }
                }
            }
        }

        Ok(HistoryOut {
            messages,
            archived,
            busy,
            compacting,
            pending_asks,
            live_text,
            live_thinking,
        })
    }

    /// 删除会话：中断正在跑的轮子，摘掉事件出口，**删掉磁盘上的 transcript、
    /// 基线和浏览器 profile**。
    ///
    /// 幂等 —— 删一个不存在的会话是成功，不是错误。用户连点两次删除、
    /// 或者两个窗口先后删同一个，第二次都不该弹报错。
    pub async fn delete_session(&self, session_id: &str) {
        let removed = self.0.sessions.lock().await.remove(session_id);
        self.0.sinks.lock().await.remove(session_id);
        if removed.is_some() {
            self.0.kernel.detach_sink(session_id).await;
            // 内核侧删除:中断轮子、关 transcript 句柄、删文件。内核不在
            // (没起过/已崩)就自己删文件 —— 不在 = 没有打开的句柄,直接删安全。
            if self
                .kernel_call(RpcRequest::SessionDelete {
                    session_id: sid(session_id),
                })
                .await
                .is_err()
            {
                let id = SessionId::from_raw(session_id.to_owned());
                if let Err(e) = self.0.transcripts.remove(&id).await {
                    tracing::warn!(error = %e, "transcript 删除失败，磁盘上可能留下孤儿文件");
                }
            }
            crate::changes::remove_baselines(&crate::changes::baselines_path(
                &self.0.sessions_dir,
                session_id,
            ));
            // 先摘掉内存里的浏览器句柄:Drop 会关掉 Chromium 进程,而
            // remove_browser_profile 删的是它锁着的 profile 目录,必须在
            // 进程退出之后(见 remove_browser_profile 的约束)。
            self.0.browsers.lock().await.remove(session_id);
            self.remove_browser_profile(session_id).await;
            self.persist_index().await;
        }
    }

    /// 删掉一个会话的浏览器 profile 目录。
    ///
    /// `[约束]` 必须跟着会话一起删。profile 里除了 cookie 和 localStorage，
    /// 还有 Chromium 自己塞的一堆缓存 —— 一个用过的 profile 是几十上百 MB，
    /// 而目录名就是会话 id，会话没了就再也没有人会认领它。漏掉这一步的后果
    /// 是缓存目录随着用过的会话数无上限增长，而用户从界面上完全看不到它。
    ///
    /// `[约束]` 必须在 `interrupt()` 之后。浏览器进程握着 profile 里的
    /// SingletonLock 和一批 leveldb 文件，边写边删会留下删不干净的残骸。
    ///
    /// 删不掉只告警。会话在逻辑上已经删除了，为一个缓存目录把整个操作报成
    /// 失败说不通 —— 残留的那份下次启动由 [`Self::gc_browser_profiles`] 收。
    async fn remove_browser_profile(&self, session_id: &str) {
        let dir = crate::config::profiles_dir(&self.0.config_path).join(session_id);
        if !dir.is_dir() {
            return;
        }
        // spawn_blocking：删的是上百 MB、几千个文件，同步做会把执行器的
        // 一个线程按住几百毫秒。
        let path = dir.clone();
        let done = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&path)).await;
        match done {
            Ok(Ok(())) => tracing::info!(dir = %dir.display(), "已删除会话的浏览器 profile"),
            Ok(Err(e)) => {
                tracing::warn!(error = %e, dir = %dir.display(), "浏览器 profile 没删掉");
            }
            Err(e) => tracing::warn!(error = %e, "删 profile 的任务没跑完"),
        }
    }

    /// 清掉没有会话认领的浏览器 profile 目录。
    ///
    /// [`Self::delete_session`] 已经会顺手删自己那份，所以这里收的是另外两类：
    /// 旧版本留下的（那时删会话不删 profile），以及应用被强杀、删到一半的。
    ///
    /// `[约束]` 判定依据是**内存里的会话表**。它在 `restore_at` 之后就是索引
    /// 的全部内容，所以这个方法只能在恢复完成之后调。拿磁盘上的 transcript
    /// 文件当依据是不够的：一个刚建好、还没写过消息的会话在索引里但没有
    /// transcript，照那个判会把它正在用的 profile 删掉。
    pub async fn gc_browser_profiles(&self) {
        let root = crate::config::profiles_dir(&self.0.config_path);
        let live: std::collections::HashSet<String> =
            self.0.sessions.lock().await.keys().cloned().collect();

        let cleaned = tokio::task::spawn_blocking(move || {
            let mut cleaned = Vec::new();
            // 目录还不存在 = 一次浏览器都没用过，没什么可收的。
            let Ok(entries) = std::fs::read_dir(&root) else {
                return cleaned;
            };
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if live.contains(&name) || !e.path().is_dir() {
                    continue;
                }
                match std::fs::remove_dir_all(e.path()) {
                    Ok(()) => cleaned.push(name),
                    Err(err) => {
                        tracing::warn!(error = %err, dir = %name, "孤儿 profile 没删掉");
                    }
                }
            }
            cleaned
        })
        .await;

        match cleaned {
            Ok(v) if !v.is_empty() => {
                tracing::info!(
                    count = v.len(),
                    "清掉了 {} 个没人认领的浏览器 profile",
                    v.len()
                );
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "清理孤儿 profile 的任务没跑完"),
        }
    }

    /// 重命名会话。空标题表示清除手动名，回退到第一条消息。
    pub async fn rename_session(&self, session_id: &str, title: &str) -> HostResult<()> {
        let t = title.trim();
        let custom: Option<String> = (!t.is_empty()).then(|| t.chars().take(80).collect());
        {
            let mut g = self.0.sessions.lock().await;
            let m = g.get_mut(session_id).ok_or(HostError::NoSession)?;
            // 80 字够侧边栏显示三行了，再长就是粘贴错了东西
            m.custom_title = custom.clone();
        }
        self.persist_index().await;
        // 尽力同步给内核(抑制它那边的自动起名)。失败无妨:标题以宿主为准。
        if let Err(e) = self
            .kernel_call(RpcRequest::SessionSetTitle {
                session_id: sid(session_id),
                title: custom,
            })
            .await
        {
            tracing::debug!(error = %e, "标题没同步到内核(以宿主为准,无妨)");
        }
        Ok(())
    }

    /// 把会话表的当前状态写进索引。每次会话结构或元数据变化后调用。
    ///
    /// 整体重写而不是增量：索引就几 KB，原子替换没有"哪行说了算"的合并
    /// 问题。失败只告警 —— 索引丢了能从 transcript 重建，不值得为它
    /// 打断用户的操作。
    async fn persist_index(&self) {
        let _g = self.0.index_lock.lock().await;

        let mut sessions: Vec<_> = self
            .0
            .sessions
            .lock()
            .await
            .iter()
            .map(|(id, m)| crate::persist::PersistedSession {
                id: id.clone(),
                root: m.root.display().to_string(),
                seq: m.seq,
                created_at_ms: m.created_at_ms,
                custom_title: m.custom_title.clone(),
                auto_title: m.auto_title.clone(),
                mode: m.mode,
                sampling: m.sampling,
                python_venv: m.python_venv.clone(),
                system_prompt: m.system_prompt.clone(),
                thinking: m.thinking,
            })
            .collect();
        sessions.sort_by_key(|p| p.seq);

        let index = crate::persist::SessionIndex { sessions };
        if let Err(e) = crate::persist::save(&self.0.sessions_dir, &index) {
            tracing::warn!(error = %e, "会话索引没能写盘，重启后列表可能不完整");
        }
    }

    /// 把项目从列表移除，并关闭它下面所有会话。
    ///
    /// 只动列表和内存里的会话，**不碰磁盘上的目录** —— "移除"和"删除"
    /// 是两个词，混淆它们的应用早晚删掉别人的代码。
    pub async fn remove_project(&self, root: &str) -> Vec<String> {
        let mut config = self.config().await;
        config.projects.retain(|p| p != root);
        if let Err(e) = crate::config::save_at(&self.0.config_path, &config) {
            tracing::warn!(error = %e, "项目列表没能写进配置");
        }
        self.set_config(config).await;

        let doomed: Vec<String> = self
            .0
            .sessions
            .lock()
            .await
            .iter()
            .filter(|(_, m)| m.root.display().to_string() == root)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &doomed {
            self.delete_session(id).await;
        }
        doomed
    }

    /// 某个会话的面板浏览器。
    ///
    /// 没打包浏览器时报错而不是静默成功 —— 面板点开一片黑却没有任何提示，
    /// 是最难查的那种。
    pub async fn panel_browser(
        &self,
        id: &str,
    ) -> HostResult<Arc<crate::browser::access::HostBrowser>> {
        // 先确认会话存在(给出一致的 NoSession 错误),再取它的浏览器。
        self.require_session(id).await?;
        self.0
            .browsers
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| {
                HostError::Browser(riot_protocol::browser::BrowserUnavailable(
                    "这个构建没有内置浏览器。开发时先跑 scripts/build-browser.sh。".into(),
                ))
            })
    }

    /// 会话在登记表里吗。给出一致的 NoSession 错误。
    async fn require_session(&self, id: &str) -> HostResult<()> {
        self.0
            .sessions
            .lock()
            .await
            .contains_key(id)
            .then_some(())
            .ok_or(HostError::NoSession)
    }

    /// 环境快照组装（`env_probe`）要用的内部件。只开 crate 内。
    /// （终端句柄走已有的 [`Self::terminals`]。）
    pub(crate) async fn browser_of(
        &self,
        session_id: &str,
    ) -> Option<Arc<crate::browser::access::HostBrowser>> {
        self.0.browsers.lock().await.get(session_id).cloned()
    }

    pub(crate) fn env_alerts(&self) -> &crate::env_probe::AlertSeen {
        &self.0.env_alerts
    }

    /// 会话的项目根。
    async fn session_root(&self, id: &str) -> HostResult<PathBuf> {
        self.0
            .sessions
            .lock()
            .await
            .get(id)
            .map(|m| m.root.clone())
            .ok_or(HostError::NoSession)
    }

    /// 确认这个会话已经有前端在听。真正发事件走会话自己的出口句柄
    /// （见 [`Self::attach_sink`]）。
    async fn require_sink(&self, id: &str) -> HostResult<()> {
        self.0
            .sinks
            .lock()
            .await
            .contains_key(id)
            .then_some(())
            .ok_or(HostError::NoSink)
    }

    /// 发起一轮。
    ///
    /// `[约束]` 不等它跑完就返回。整轮可能要几分钟，而 Tauri 的 command
    /// 阻塞住会让前端的 `await invoke(...)` 一直挂着 —— 用户按不了停止键，
    /// 而停止键正是这种时候最需要的东西。
    /// 返回 `Some(条目 id)` 表示上一轮还在跑、这条消息进了插话队列
    /// （前端排队面板靠这个 id 跟踪它）；`None` 表示直接开轮了。
    pub async fn send_turn(
        &self,
        session_id: &str,
        text: &str,
        images: Vec<riot_protocol::ImageInput>,
        refs: Vec<String>,
    ) -> HostResult<Option<String>> {
        self.require_sink(session_id).await?;
        self.ensure_hydrated(session_id).await?;
        let sampling = {
            let g = self.0.sessions.lock().await;
            g.get(session_id).ok_or(HostError::NoSession)?.sampling
        };

        // 每轮解析"此刻"的激活配置 —— 对话中途切换模型下一轮就生效。
        // 会话的采样覆盖叠在 provider 默认之上，只盖用户动过的字段。
        let config = self.config().await;
        let mut model = config.resolve()?;
        model.sampling = sampling.or(model.sampling);

        // `[约束]` 图片处理不了要**在这里**拒绝，而不是让这一轮跑起来。
        //
        // 这条命令的失败会当场变成界面上的提示；而轮子跑起来之后的失败只能
        // 走事件流，用户看到的是"发出去了，然后模型说它看不见图" —— 那时候
        // 他已经等了几秒，而且不知道该去改什么。
        if !images.is_empty() && !config.active_takes_images() && config.vision_target().is_none() {
            return Err(HostError::Provider(
                "当前模型收不了图片。去设置里给这个服务方打开「图片」，\
                 或者配一个视觉兼容模型（设置 → 服务方 → 视觉兼容）。"
                    .to_owned(),
            ));
        }

        // 第一句话定下自动标题，立刻落索引 —— 重启后侧边栏就靠它显示名字。
        // 放在提交之前：轮子在内核异步跑，等它结束才写的话，中途强杀就
        // 是一个"有历史但没标题"的会话。
        {
            let mut g = self.0.sessions.lock().await;
            if let Some(m) = g.get_mut(session_id)
                && m.custom_title.is_none()
                && m.auto_title.is_none()
                && let Some(t) = crate::session::title_excerpt(text)
            {
                m.auto_title = Some(t);
                drop(g);
                self.persist_index().await;
            }
        }

        // 剩下的活全在内核:UserPromptSubmit hook(拦截会变成这条 RPC 的
        // 错误应答,照样当场报给界面)、图片转述、@ 展开、能力装配。
        // 宿主只负责把配置快照打包成 TurnConfig。
        let turn_config = self.build_turn_config(&config, model, session_id).await?;
        let resp = self
            .kernel_call(RpcRequest::TurnSubmit {
                session_id: sid(session_id),
                input: riot_protocol::TurnInput {
                    text: text.to_owned(),
                    images,
                    refs,
                },
                config: Box::new(turn_config),
            })
            .await?;
        let RpcResponse::TurnSubmitted { queued_id } = resp else {
            return Err(HostError::Kernel(crate::kernel::KernelError::Rpc(
                "turn.submit 回了意外的应答".into(),
            )));
        };
        // 无论直接开轮还是进了插话队列,此刻都有轮子在跑(排队的前提就是
        // 上一轮还在)。Done 事件会清掉它。
        if let Some(m) = self.0.sessions.lock().await.get_mut(session_id) {
            m.busy = true;
        }
        Ok(queued_id)
    }

    /// 丢掉指定助手消息及其后的一切，从它前面那条用户提示再跑一轮。
    pub async fn regenerate_turn(&self, session_id: &str, message_id: &str) -> HostResult<()> {
        self.require_sink(session_id).await?;
        self.ensure_hydrated(session_id).await?;
        let sampling = {
            let g = self.0.sessions.lock().await;
            g.get(session_id).ok_or(HostError::NoSession)?.sampling
        };
        let config = self.config().await;
        let mut model = config.resolve()?;
        model.sampling = sampling.or(model.sampling);
        let turn_config = self.build_turn_config(&config, model, session_id).await?;
        self.kernel_call(RpcRequest::TurnRegenerate {
            session_id: sid(session_id),
            message_id: message_id.to_owned(),
            config: Box::new(turn_config),
        })
        .await?;
        if let Some(m) = self.0.sessions.lock().await.get_mut(session_id) {
            m.busy = true;
        }
        Ok(())
    }

    /// 把"此刻"的配置快照打包成随轮传给内核的 [`riot_protocol::TurnConfig`]。
    ///
    /// 内核不读 config.json / auth.json —— 模型端点(含明文 key)、联网/视觉
    /// 目标、limits、会话设置,全部在宿主解析好传过去。每轮现打包,
    /// 用户中途改设置下一轮就生效(和内嵌时代的"每轮现装"同一条线)。
    async fn build_turn_config(
        &self,
        config: &AppConfig,
        model: crate::config::ResolvedModel,
        session_id: &str,
    ) -> HostResult<riot_protocol::TurnConfig> {
        let endpoint = model.to_endpoint()?;

        // 可选的辅助端点:解析失败一律降级成 None(它们都是省钱/增强的
        // 可选项,配坏了不该挡住主流程),内核侧各有兜底。
        let named = |target: Option<(&str, &str)>| -> Option<riot_protocol::ModelEndpoint> {
            let (pid, m) = target?;
            config
                .resolve_named(pid, m)
                .inspect_err(|e| tracing::warn!(error = %e, "辅助模型解析失败"))
                .ok()?
                .to_endpoint()
                .inspect_err(|e| tracing::warn!(error = %e, "辅助模型缺密钥"))
                .ok()
        };
        let cheap_model = named(config.subagent_target());
        let distill = named(config.web.distill_target());
        let describe = named(config.vision_target());

        let (mode, python_venv, system_prompt, thinking) = {
            let g = self.0.sessions.lock().await;
            let m = g.get(session_id).ok_or(HostError::NoSession)?;
            (
                m.mode,
                m.python_venv.clone(),
                m.system_prompt.clone(),
                m.thinking,
            )
        };

        Ok(riot_protocol::TurnConfig {
            model: endpoint,
            cheap_model,
            web: riot_protocol::WebSetup {
                fetch_enabled: config.web.fetch_enabled,
                search_enabled: config.web.search_ready(),
                // 内置地址不传给内核进程，由内核自己补。覆盖才写进 turn。
                searxng_url: crate::config::normalize_searxng_url(&config.web.searxng_url),
                distill,
            },
            vision: riot_protocol::VisionSetup {
                accepts_images: config.active_takes_images(),
                describe,
            },
            limits: riot_protocol::TurnLimits {
                ask_timeout_secs: config.ask_timeout_secs,
                max_turns: config.max_turns,
                // 这个模型填了窗口就按窗口推，没填才用设置页那个全局数。
                // 内核只认最终的阈值 —— 窗口是宿主这边的配置概念，换算完
                // 就没必要再往下传一层。
                compact_threshold_tokens: model.compact_threshold(config.compact_threshold_tokens),
                sandbox: sandbox_kind(config.sandbox),
                sandbox_allow_read: config.sandbox_allow_read.clone(),
            },
            mode,
            rules: Vec::new(),
            python_venv,
            system_prompt_extra: system_prompt,
            thinking,
        })
    }

    /// 上下文编辑：替换一条历史消息的文本段。空闲时才能做，内核会拒绝并发。
    pub async fn edit_message(
        &self,
        session_id: &str,
        message_id: &str,
        text: &str,
    ) -> HostResult<()> {
        self.ensure_hydrated(session_id).await?;
        self.kernel_call(RpcRequest::HistoryEdit {
            session_id: sid(session_id),
            message_id: message_id.to_owned(),
            text: text.to_owned(),
        })
        .await?;
        Ok(())
    }

    /// 上下文删除：抹掉一条历史消息的可见内容，空心则整条移除。
    pub async fn delete_message(&self, session_id: &str, message_id: &str) -> HostResult<()> {
        self.ensure_hydrated(session_id).await?;
        self.kernel_call(RpcRequest::HistoryDelete {
            session_id: sid(session_id),
            message_id: message_id.to_owned(),
        })
        .await?;
        Ok(())
    }

    /// 手动压缩会话历史（`/compact`）。完成时发 Compacted 事件。
    pub async fn compact_session(&self, session_id: &str) -> HostResult<()> {
        self.require_sink(session_id).await?;
        self.ensure_hydrated(session_id).await?;
        let sampling = {
            let g = self.0.sessions.lock().await;
            g.get(session_id).ok_or(HostError::NoSession)?.sampling
        };
        let config = self.config().await;
        let mut model = config.resolve()?;
        model.sampling = sampling.or(model.sampling);
        self.kernel_call(RpcRequest::SessionCompact {
            session_id: sid(session_id),
            model: Box::new(model.to_endpoint()?),
        })
        .await?;
        Ok(())
    }

    /// `@` 补全菜单的文件搜索。查询为空时给项目里的前几个文件。
    /// 纯 cwd 函数,库两边都链接 —— 宿主本地调,不走 RPC。
    pub async fn search_files(&self, session_id: &str, query: &str) -> HostResult<Vec<String>> {
        let root = self.session_root(session_id).await?;
        Ok(crate::mentions::search_files(&root, query).await)
    }

    /// 展开一条自定义命令。None = 没这条命令或它是内置的。
    pub async fn slash_expand(
        &self,
        session_id: &str,
        name: &str,
        args: &str,
    ) -> HostResult<Option<String>> {
        let root = self.session_root(session_id).await?;
        Ok(crate::slash::expand(Some(&root), name, args))
    }

    /// 排队面板：当前排着的插话。
    pub async fn queue_list(
        &self,
        session_id: &str,
    ) -> HostResult<Vec<riot_protocol::QueuedSummary>> {
        self.require_session(session_id).await?;
        match self
            .kernel_call(RpcRequest::QueueList {
                session_id: sid(session_id),
            })
            .await?
        {
            RpcResponse::QueueList { entries } => Ok(entries),
            _ => Err(HostError::Kernel(crate::kernel::KernelError::Rpc(
                "queue.list 回了意外的应答".into(),
            ))),
        }
    }

    /// 删掉一条排队插话。false = 条目已经不在（被注入或早被删了）。
    pub async fn queue_remove(&self, session_id: &str, entry_id: &str) -> HostResult<bool> {
        self.require_session(session_id).await?;
        match self
            .kernel_call(RpcRequest::QueueRemove {
                session_id: sid(session_id),
                entry_id: entry_id.to_owned(),
            })
            .await?
        {
            RpcResponse::Removed { removed } => Ok(removed),
            _ => Err(HostError::Kernel(crate::kernel::KernelError::Rpc(
                "queue.remove 回了意外的应答".into(),
            ))),
        }
    }

    /// 撤回一条排队插话，还给前端原始输入（放回输入框编辑）。
    pub async fn queue_take(
        &self,
        session_id: &str,
        entry_id: &str,
    ) -> HostResult<Option<riot_protocol::TurnInput>> {
        self.require_session(session_id).await?;
        match self
            .kernel_call(RpcRequest::QueueTake {
                session_id: sid(session_id),
                entry_id: entry_id.to_owned(),
            })
            .await?
        {
            RpcResponse::QueueTaken { input } => Ok(input),
            _ => Err(HostError::Kernel(crate::kernel::KernelError::Rpc(
                "queue.take 回了意外的应答".into(),
            ))),
        }
    }

    pub async fn interrupt(&self, session_id: &str) -> HostResult<bool> {
        self.require_session(session_id).await?;
        self.kernel_call(RpcRequest::TurnInterrupt {
            session_id: sid(session_id),
            interjection: false,
        })
        .await?;
        Ok(true)
    }

    /// 本会话改了哪些文件、哪些行。给 review 视图。
    pub async fn changes(&self, session_id: &str) -> HostResult<Vec<riot_protocol::FileChange>> {
        self.ensure_hydrated(session_id).await?;
        match self
            .kernel_call(RpcRequest::SessionChanges {
                session_id: sid(session_id),
            })
            .await?
        {
            RpcResponse::Changes { changes } => Ok(changes),
            _ => Err(HostError::Kernel(crate::kernel::KernelError::Rpc(
                "session.changes 回了意外的应答".into(),
            ))),
        }
    }

    /// 工作区相对所选基线的差异。Git 面板用。
    pub async fn git_changes(
        &self,
        session_id: &str,
        base: Option<&str>,
    ) -> HostResult<riot_protocol::GitChanges> {
        // 会话可能还没进内核内存（刚重启）——先水合，git 才知道以哪个目录为根。
        self.ensure_hydrated(session_id).await?;
        match self
            .kernel_call(RpcRequest::SessionGitChanges {
                session_id: sid(session_id),
                base: base.map(str::to_owned),
            })
            .await?
        {
            RpcResponse::GitChanges { git } => Ok(git),
            _ => Err(HostError::Kernel(crate::kernel::KernelError::Rpc(
                "session.git_changes 回了意外的应答".into(),
            ))),
        }
    }

    pub async fn set_mode(&self, session_id: &str, mode: PermissionMode) -> HostResult<()> {
        {
            let mut g = self.0.sessions.lock().await;
            g.get_mut(session_id).ok_or(HostError::NoSession)?.mode = mode;
        }
        self.persist_index().await;
        // 尽力同步给内核,让**正在跑的轮子**立刻按新模式办。失败无妨:
        // 下一轮 TurnConfig 会带上新模式。
        if let Err(e) = self
            .kernel_call(RpcRequest::ConfigSetMode {
                session_id: sid(session_id),
                mode,
            })
            .await
        {
            tracing::debug!(error = %e, "模式没同步到内核(下一轮 TurnConfig 会带)");
        }
        Ok(())
    }

    pub async fn scope_hosts(&self, session_id: &str) -> HostResult<Vec<String>> {
        self.require_session(session_id).await?;
        match self
            .kernel_call(RpcRequest::ScopeList {
                session_id: sid(session_id),
            })
            .await?
        {
            RpcResponse::ScopeHosts { hosts } => Ok(hosts),
            _ => Err(HostError::Kernel(crate::kernel::KernelError::Rpc(
                "scope.list 回了意外的应答".into(),
            ))),
        }
    }

    pub async fn revoke_scope(&self, session_id: &str, host: &str) -> HostResult<()> {
        self.require_session(session_id).await?;
        self.kernel_call(RpcRequest::ScopeRevoke {
            session_id: sid(session_id),
            host: host.to_owned(),
        })
        .await?;
        Ok(())
    }

    /// 设置会话级采样覆盖。空字段继承 provider；下一轮生效
    /// (采样随 TurnConfig 的 ModelEndpoint 传,宿主登记即权威)。
    pub async fn set_sampling(
        &self,
        session_id: &str,
        sampling: crate::config::Sampling,
    ) -> HostResult<()> {
        {
            let mut g = self.0.sessions.lock().await;
            g.get_mut(session_id).ok_or(HostError::NoSession)?.sampling = sampling;
        }
        self.persist_index().await;
        Ok(())
    }

    /// 设置会话级思考策略。下一轮生效。
    pub async fn set_thinking(
        &self,
        session_id: &str,
        thinking: riot_protocol::ThinkingPolicy,
    ) -> HostResult<()> {
        {
            let mut g = self.0.sessions.lock().await;
            g.get_mut(session_id).ok_or(HostError::NoSession)?.thinking = thinking;
        }
        self.persist_index().await;
        Ok(())
    }

    /// 探测会话根目录下的常见虚拟环境（`.venv` / `venv`）。
    ///
    /// 存在的意义：系统目录选择框默认**隐藏**点开头的目录，而 venv 最常见
    /// 的名字恰恰是 `.venv` —— 用户在选择框里根本看不到它。探测出来让前端
    /// 一键填入，多数情况下连选择框都不用开。校验标准与 [`Self::set_python_venv`]
    /// 一致（有没有 python 可执行文件）。
    pub async fn detect_venvs(&self, session_id: &str) -> HostResult<Vec<String>> {
        let root = self.session_root(session_id).await?;
        Ok([".venv", "venv"]
            .iter()
            .map(|name| root.join(name))
            .filter(|dir| venv_python(dir).exists())
            .map(|dir| dir.display().to_string())
            .collect())
    }

    /// 设置会话的 Python 虚拟环境。空字符串清除；下一轮生效。
    ///
    /// 在这里验证目录：venv 路径写错的表现是"pip 装到了系统环境里"，
    /// 那种失败要等模型跑完命令才暴露，而且报错完全不指向路径。
    pub async fn set_python_venv(&self, session_id: &str, path: &str) -> HostResult<()> {
        self.require_session(session_id).await?;
        let trimmed = path.trim();
        let value = if trimmed.is_empty() {
            None
        } else {
            let python = venv_python(std::path::Path::new(trimmed));
            // 宿主层验证用户亲手填的路径，直接查真实文件系统。
            if !python.exists() {
                return Err(HostError::Provider(format!(
                    "{} 不像一个虚拟环境：找不到 {}。\
                     应该填 venv 的根目录（python -m venv 创建出来的那个）。",
                    trimmed,
                    python.display(),
                )));
            }
            Some(trimmed.to_owned())
        };
        if let Some(m) = self.0.sessions.lock().await.get_mut(session_id) {
            m.python_venv = value;
        }
        self.persist_index().await;
        Ok(())
    }

    /// 设置会话级追加提示词。空字符串清除；下一轮生效。
    pub async fn set_system_prompt(&self, session_id: &str, prompt: &str) -> HostResult<()> {
        let p = prompt.trim();
        {
            let mut g = self.0.sessions.lock().await;
            g.get_mut(session_id)
                .ok_or(HostError::NoSession)?
                .system_prompt = (!p.is_empty()).then(|| p.to_owned());
        }
        self.persist_index().await;
        Ok(())
    }

    pub async fn respond_permission(
        &self,
        session_id: &str,
        ask_id: &str,
        response: PermissionResponse,
    ) -> HostResult<()> {
        self.require_session(session_id).await?;
        // 内核按 request_id 在各会话的待答表里找。回应一个已经不存在的
        // 请求不是错误 —— 用户在超时之后才点按钮是正常的人类行为。
        self.kernel_call(RpcRequest::PermissionRespond {
            request_id: riot_protocol::id::RequestId::from_raw(ask_id.to_owned()),
            response,
        })
        .await?;
        Ok(())
    }

    /// 让内核的 MCP 连接对齐当前配置。启动时和每次保存设置后调用。
    ///
    /// 宿主组清单(读设置是宿主的职责),内核只管照单连接 —— MCP 工具是
    /// trait object,必须在内核进程里执行。reconcile 不等握手完成,
    /// 连接进度通过 [`Self::mcp_statuses`] 查询。
    pub async fn reconcile_mcp(&self) {
        let config = self.config().await;
        let servers: Vec<riot_protocol::rpc::McpServerSpec> = config
            .mcp_servers
            .iter()
            // 空命令 = 刚添加还没填完的中间状态，跳过而不是报错 ——
            // 校验放行它正是为了让"添加"按钮能落地（见 config::validate_mcp）。
            .filter(|s| s.enabled && !s.command.trim().is_empty())
            .map(|s| riot_protocol::rpc::McpServerSpec {
                id: s.id.clone(),
                command: s.command.clone(),
                args: s.args.clone(),
                env: s.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            })
            .collect();
        if let Err(e) = self.kernel_call(RpcRequest::McpReconcile { servers }).await {
            tracing::warn!(error = %e, "MCP 清单没送到内核");
        }
    }

    /// 终端面板的句柄。Tauri 那边 `manage` 的和会话用的必须是同一份 ——
    /// 各拿各的话，模型起的服务会跑在一个用户永远看不到的面板里。
    pub fn terminals(&self) -> crate::term::Terminals {
        self.0.terminals.clone()
    }

    /// MCP 服务器的连接状态，给设置页看。
    pub async fn mcp_statuses(&self) -> Vec<riot_protocol::rpc::McpServerStatus> {
        match self.kernel_call(RpcRequest::McpStatus).await {
            Ok(RpcResponse::McpStatuses { servers }) => servers,
            Ok(_) => Vec::new(),
            Err(e) => {
                tracing::debug!(error = %e, "拿不到 MCP 状态(内核未运行?)");
                Vec::new()
            }
        }
    }

    /// 手动重连一个 MCP 服务器。
    pub async fn mcp_restart(&self, id: &str) -> HostResult<()> {
        self.kernel_call(RpcRequest::McpRestart { id: id.to_owned() })
            .await?;
        Ok(())
    }

    /// 关闭:走内核的四步关闭序列(shutdown RPC → EOF → 等退出 → 杀进程组)。
    /// 会话 flush、MCP 子进程清理都在内核那边做。
    pub async fn shutdown(&self) {
        self.0.kernel.shutdown().await;
    }
}

/// 宿主配置的沙箱档位 → 传输档位。
fn sandbox_kind(mode: crate::config::SandboxMode) -> riot_protocol::SandboxKind {
    match mode {
        crate::config::SandboxMode::WorkspaceWrite => riot_protocol::SandboxKind::WorkspaceWrite,
        crate::config::SandboxMode::WorkspaceWriteNoNet => {
            riot_protocol::SandboxKind::WorkspaceWriteNoNet
        }
        crate::config::SandboxMode::Off => riot_protocol::SandboxKind::Off,
    }
}

/// 内核反向请求(终端/浏览器)的宿主处理端。
///
/// 持有 `AppState` 的一份浅拷贝,于是 AppState → KernelClient → HostCalls
/// → AppState 形成一个引用环 —— 无害:三者都是进程级单例,活到进程结束,
/// 没有"该被回收却被环撑着"的对象。
struct HostCalls(AppState);

fn host_unavailable(message: impl Into<String>) -> riot_protocol::hostcall::HostResponse {
    riot_protocol::hostcall::HostResponse::Error {
        kind: riot_protocol::hostcall::HostCallErrorKind::Unavailable,
        message: message.into(),
    }
}

fn interact_resp(
    e: riot_protocol::browser::InteractError,
) -> riot_protocol::hostcall::HostResponse {
    use riot_protocol::browser::InteractError;
    use riot_protocol::hostcall::{HostCallErrorKind, HostResponse};
    match e {
        InteractError::Unavailable(u) => HostResponse::Error {
            kind: HostCallErrorKind::Unavailable,
            message: u.0,
        },
        InteractError::Target(m) => HostResponse::Error {
            kind: HostCallErrorKind::Target,
            message: m,
        },
    }
}

/// 把一次浏览器调用分发到会话的 [`HostBrowser`](crate::browser::access::HostBrowser)。
async fn browser_call(
    b: Arc<crate::browser::access::HostBrowser>,
    call: riot_protocol::hostcall::BrowserCall,
) -> riot_protocol::hostcall::HostResponse {
    use riot_protocol::browser::{BrowserAccess, InteractError};
    use riot_protocol::hostcall::{BrowserCall as C, HostResponse as R};

    fn text(r: Result<String, InteractError>) -> R {
        match r {
            Ok(text) => R::Text { text },
            Err(e) => interact_resp(e),
        }
    }

    match call {
        C::Navigate { url } => match b.navigate(&url).await {
            Ok(()) => R::Ok,
            Err(e) => host_unavailable(e.0),
        },
        C::Screenshot { deterministic } => match b.screenshot(deterministic).await {
            Ok(text) => R::Text { text },
            Err(e) => host_unavailable(e.0),
        },
        C::Snapshot => match b.snapshot().await {
            Ok(text) => R::Text { text },
            Err(e) => host_unavailable(e.0),
        },
        C::SnapshotMarked => match b.snapshot_marked().await {
            Ok(m) => R::Marked {
                listing: m.listing,
                screenshot: m.screenshot,
            },
            Err(e) => host_unavailable(e.0),
        },
        C::Console => match b.console().await {
            Ok(lines) => R::Lines { lines },
            Err(e) => host_unavailable(e.0),
        },
        C::CurrentUrl => R::Text {
            text: b.current_url().await,
        },
        C::Click { target } => text(b.click(target).await),
        C::TypeText {
            target,
            text: t,
            submit,
        } => text(b.type_text(target, &t, submit).await),
        C::PressKey { key } => text(b.press_key(&key).await),
        C::Scroll { delta_y } => text(b.scroll(delta_y).await),
        C::WaitFor { cond, timeout_ms } => text(b.wait_for(cond, timeout_ms).await),
        C::Act { action } => text(b.act(action).await),
        C::Browse { nav } => text(b.browse(nav).await),
        C::Evaluate { expr } => text(b.evaluate(&expr).await),
        C::SourceOf { target } => text(b.source_of(target).await),
        C::SnapshotTab { tab } => text(b.snapshot_tab(tab).await),
        C::Upload { target, paths } => text(b.upload(target, paths).await),
        C::Cookies => text(b.cookies().await),
        C::Network { query } => text(b.network(query).await),
        C::Replay {
            url,
            method,
            headers,
            body,
        } => text(b.replay(&url, &method, headers, body).await),
        C::Intercept { op } => text(b.intercept(op).await),
    }
}

#[async_trait::async_trait]
impl crate::kernel::HostCallHandler for HostCalls {
    async fn handle(
        &self,
        req: riot_protocol::hostcall::HostRequest,
    ) -> riot_protocol::hostcall::HostResponse {
        use riot_protocol::hostcall::{HostRequest as Req, HostResponse as R};
        use riot_protocol::terminal::TerminalAccess;

        // 终端面板是应用级的,但 spawn 的 cwd 是会话的项目根、所有权按会话
        // 判定 —— 每次现建一个轻量的 HostTerminal 包装是安全的:它无状态,
        // 所有权在 Terminals 注册表里(docs/ENV_DESIGN.md §6)。
        let terminal = |root: PathBuf, sid: &riot_protocol::id::SessionId| {
            crate::term_access::HostTerminal::new(
                self.0.0.terminals.clone(),
                root,
                sid.as_str().to_owned(),
            )
        };

        match req {
            Req::TerminalSpawn {
                session_id,
                command,
                title,
            } => {
                let root = match self.0.session_root(session_id.as_str()).await {
                    Ok(r) => r,
                    Err(_) => return host_unavailable("会话不存在,起不了终端"),
                };
                match terminal(root, &session_id).spawn(&command, &title).await {
                    Ok(id) => R::TerminalId { id },
                    Err(e) => host_unavailable(e.0),
                }
            }
            Req::TerminalRead {
                session_id,
                id,
                lines,
            } => {
                let root = match self.0.session_root(session_id.as_str()).await {
                    Ok(r) => r,
                    Err(_) => return host_unavailable("会话不存在"),
                };
                match terminal(root, &session_id).read(id, lines).await {
                    Ok(text) => R::Text { text },
                    Err(e) => host_unavailable(e.0),
                }
            }
            Req::TerminalKill { session_id, id } => {
                let root = match self.0.session_root(session_id.as_str()).await {
                    Ok(r) => r,
                    Err(_) => return host_unavailable("会话不存在"),
                };
                match terminal(root, &session_id).kill(id).await {
                    Ok(()) => R::Ok,
                    Err(e) => host_unavailable(e.0),
                }
            }
            Req::TerminalList { session_id } => {
                let root = match self.0.session_root(session_id.as_str()).await {
                    Ok(r) => r,
                    Err(_) => return host_unavailable("会话不存在"),
                };
                R::Terminals {
                    items: terminal(root, &session_id).list().await,
                }
            }
            Req::EnvSnapshot { session_id } => {
                if self.0.session_root(session_id.as_str()).await.is_err() {
                    return host_unavailable("会话不存在");
                }
                R::Env {
                    snapshot: crate::env_probe::assemble(&self.0, session_id.as_str()).await,
                }
            }
            Req::BrowserCall { session_id, call } => {
                let browser = self
                    .0
                    .0
                    .browsers
                    .lock()
                    .await
                    .get(session_id.as_str())
                    .cloned();
                match browser {
                    Some(b) => browser_call(b, call).await,
                    None => host_unavailable(
                        "这个构建没有内置浏览器。开发时先跑 scripts/build-browser.sh。",
                    ),
                }
            }
        }
    }
}

/// venv 根目录里 python 可执行文件的位置。
///
/// 探测和校验共用这一个定义 —— 各写一遍的话，某天有人只改了一边，
/// 表现是"探测说有，填进去却报不像虚拟环境"。
fn venv_python(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join(if cfg!(windows) { "Scripts" } else { "bin" })
        .join(if cfg!(windows) {
            "python.exe"
        } else {
            "python"
        })
}

/// 项目列表的路径归一化 + 去重（保序，先出现的赢）。
///
/// 专治一类历史脏数据:Windows 上 `Fence::new` 曾把 canonicalize 的
/// verbatim 结果（`\\?\D:\x`）原样写进项目列表，和用户手选的 `D:\x`
/// 变成两个项目。归一化后两串相同，去重收敛成一个。
fn normalize_projects(projects: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    let normalized: Vec<String> = projects
        .drain(..)
        .map(|p| {
            crate::fence::strip_verbatim(PathBuf::from(&p))
                .display()
                .to_string()
        })
        .filter(|p| seen.insert(p.clone()))
        .collect();
    *projects = normalized;
}

/// 给一个会话装配面板浏览器。没打包浏览器时返回 None(工具装 NoBrowser、
/// 面板报不可用)。profile 目录按会话 id 隔离:同一数据目录不能跑两个
/// Chromium 实例,共用的话第二个会话一用就报不可用。
///
/// 从 Session 移到宿主(阶段 B):浏览器进程和面板都是宿主能力,内核只经
/// `dyn BrowserAccess` 用同一个实例。
fn make_browser(
    config_path: &std::path::Path,
    id: &str,
) -> Option<Arc<crate::browser::access::HostBrowser>> {
    let app = crate::browser::access::locate_app()?;
    let profile = crate::config::profiles_dir(config_path).join(id);
    Some(crate::browser::access::HostBrowser::new(app, profile))
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn temp_ws(tag: &str) -> String {
        let p = std::env::temp_dir().join(format!("riot-ws-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("建临时工作区");
        p.to_str().expect("utf8").to_owned()
    }

    /// 配置指向临时目录、并预置一份干净配置的 `AppState`。
    ///
    /// `[约束]` 测试里**永远不要**用裸的 `AppState::default()`。它的配置
    /// 路径是开发机上真实的 `config.json`，而 `AppState` 有会**写盘**的
    /// 方法（`remove_project`）。用默认路径的话，跑一次 `cargo test` 就把
    /// 用户配置覆盖成测试里这份空配置 —— 真发生过，而且排查时完全想不到
    /// 是测试干的：现象是"应用一重启我配的东西就没了"。
    ///
    /// 读同理：不指路径的话，测试结果取决于你自己在界面里点过什么。
    async fn state() -> AppState {
        let dir = std::env::temp_dir().join(format!(
            "riot-state-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("建临时配置目录");
        let s = AppState::with_config_path(dir.join("config.json"));
        s.set_config(AppConfig::default()).await;
        s
    }

    /// 给临时目录编号，避免并行用例互相踩。
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    /// 造一个能认出自己收没收到事件的 channel。
    fn probe() -> (Channel<AgentEvent>, Arc<AtomicU64>) {
        let hits = Arc::new(AtomicU64::new(0));
        let h = Arc::clone(&hits);
        let ch = Channel::new(move |_| {
            h.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        (ch, hits)
    }

    #[tokio::test]
    async fn 迟到的旧订阅不能顶掉新订阅() {
        // 这是"发完消息永远转圈"的根因。前端一次挂载会连发两次订阅
        // （StrictMode 把 effect 跑两遍），两个命令在宿主侧是并发任务，
        // 落地顺序没有保证。旧的那次要是后落地，事件就全发给一个前端
        // 已经不听的 channel 了 —— 而且**全程没有任何报错**。
        //
        // 拆进程后分发点在 KernelClient 的 sinks 表(attach 即换表,跑着
        // 的轮子自动跟过来);这里验证宿主侧的 epoch 语义,"事件真的进
        // 最新 channel"由内核 stdio smoke + 分发循环覆盖。
        let state = state().await;
        let id = state
            .create_session(&temp_ws("epoch"))
            .await
            .expect("会话")
            .id;

        let (new_ch, _) = probe();
        let (old_ch, _) = probe();

        // 新的先落地，旧的后落地 —— 正是会出问题的那个顺序
        assert!(state.attach_sink(id.clone(), 2, new_ch).await);
        assert!(
            !state.attach_sink(id.clone(), 1, old_ch).await,
            "epoch 更小的订阅必须被拒绝"
        );

        // 反方向也要成立，否则切走再切回来就再也收不到事件了。
        let (newer, _) = probe();
        assert!(
            state.attach_sink(id.clone(), 3, newer).await,
            "更新的订阅要能顶掉旧的"
        );
    }

    #[tokio::test]
    async fn 会话各自绑定自己的项目根() {
        // 多项目并行是这套结构存在的理由：A 项目的会话写 A，B 项目的
        // 会话写 B，谁也别碰谁。上一版的全局工作区在换目录时把这搞混过。
        let state = state().await;
        let a = state.create_session(&temp_ws("a")).await.expect("会话 a");
        let b = state.create_session(&temp_ws("b")).await.expect("会话 b");

        assert_ne!(a.id, b.id);
        assert_ne!(a.root, b.root);

        let roots: HashMap<String, String> = state
            .list_sessions()
            .await
            .into_iter()
            .map(|i| (i.id, i.root))
            .collect();
        assert_eq!(roots[&a.id], a.root);
        assert_eq!(roots[&b.id], b.root);
    }

    #[tokio::test]
    async fn 列表按创建顺序返回() {
        let state = state().await;
        let ws = temp_ws("seq");
        let first = state.create_session(&ws).await.expect("1");
        let second = state.create_session(&ws).await.expect("2");

        let ids: Vec<_> = state
            .list_sessions()
            .await
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(ids, vec![first.id, second.id]);
    }

    #[tokio::test]
    async fn 会话信息里带着宿主的真实权限模式() {
        // 前端拿这个字段决定 composer 上显示哪一档。少了它，前端只能拿
        // 全局默认值顶替 —— 用户选了「全部放行」，界面却写着「每次询问」，
        // 而放行是真的在放行。显示得比实际更严是最坏的一种错。
        let state = state().await;
        let ws = temp_ws("mode");
        let s = state.create_session(&ws).await.expect("建会话");
        assert_eq!(s.mode, PermissionMode::Default);

        state
            .set_mode(&s.id, PermissionMode::BypassPermissions)
            .await
            .expect("切模式");

        let listed = state.list_sessions().await;
        let it = listed.iter().find(|i| i.id == s.id).expect("在列表里");
        assert_eq!(
            it.mode,
            PermissionMode::BypassPermissions,
            "列表必须报告宿主的真实模式"
        );
    }

    #[tokio::test]
    async fn 各会话的权限模式互不影响() {
        // 模式是会话级的。一个会话开了放行，不该把别的会话也带下水。
        let state = state().await;
        let a = state.create_session(&temp_ws("m-a")).await.expect("a");
        let b = state.create_session(&temp_ws("m-b")).await.expect("b");

        state
            .set_mode(&a.id, PermissionMode::BypassPermissions)
            .await
            .expect("切 a");

        let listed = state.list_sessions().await;
        let mode_of = |id: &str| {
            listed
                .iter()
                .find(|i| i.id == id)
                .map(|i| i.mode)
                .expect("在列表里")
        };
        assert_eq!(mode_of(&a.id), PermissionMode::BypassPermissions);
        assert_eq!(mode_of(&b.id), PermissionMode::Default);
    }

    #[tokio::test]
    async fn 不存在的目录建不出会话() {
        let state = state().await;
        let err = state
            .create_session("/definitely/not/a/real/path/xyz")
            .await
            .expect_err("坏目录应该在创建时报错，而不是第一次工具调用才炸");
        assert!(
            matches!(err, HostError::MissingProject(_)),
            "缺目录是可恢复错误，实际: {err:?}"
        );
    }

    #[tokio::test]
    async fn 未创建的会话拿不到历史() {
        let state = state().await;
        assert!(state.history("s_ghost").await.is_err());
    }

    #[tokio::test]
    async fn 删除会话后列表和历史都找不到它() {
        let state = state().await;
        let info = state.create_session(&temp_ws("del")).await.expect("会话");

        state.delete_session(&info.id).await;

        assert!(state.list_sessions().await.is_empty());
        assert!(state.history(&info.id).await.is_err());
    }

    #[tokio::test]
    async fn 删除不存在的会话是无操作不是错误() {
        // 用户连点两次删除，第二次不该弹报错
        let state = state().await;
        state.delete_session("s_ghost").await;
    }

    /// 摆一个"这个会话用过浏览器"的 profile 目录。
    ///
    /// 测试环境里没有打包好的浏览器，会话装的是 `NoBrowser`，不会真的建
    /// 目录 —— 而这几个用例要验的正是"目录该不该被删"。
    fn 摆个profile(state: &AppState, id: &str) -> PathBuf {
        let dir = crate::config::profiles_dir(&state.0.config_path).join(id);
        std::fs::create_dir_all(dir.join("Default")).expect("建 profile");
        std::fs::write(dir.join("Default").join("Cookies"), b"x").expect("写点东西进去");
        dir
    }

    /// 删会话必须连浏览器 profile 一起删。
    ///
    /// 盯着的是一个用户完全看不见的泄漏：一个用过的 profile 是几十上百 MB，
    /// 目录名就是会话 id，会话删了就再没人会认领它。漏掉这一步的现象是
    /// "应用数据目录不知不觉涨到好几个 G"，而界面上一个会话都没有。
    #[tokio::test]
    async fn 删除会话连浏览器profile一起删() {
        let state = state().await;
        let info = state
            .create_session(&temp_ws("prof-del"))
            .await
            .expect("会话");
        let dir = 摆个profile(&state, &info.id);

        state.delete_session(&info.id).await;

        assert!(!dir.exists(), "会话删了 profile 还留着，缓存会无上限增长");
    }

    /// 启动时的清理只收孤儿，不碰活着的会话。
    ///
    /// `[约束]` 判定必须按会话表，不能按磁盘上的 transcript 文件。这个用例
    /// 里的会话刚建好、一条消息都没写过，所以它**没有** transcript ——
    /// 照 transcript 判的话，它正在用的 profile 会被当孤儿删掉，用户的现象
    /// 是"新建会话里浏览器的登录态莫名其妙丢了"。
    #[tokio::test]
    async fn 清理孤儿profile不碰活着的会话() {
        let state = state().await;
        let info = state
            .create_session(&temp_ws("prof-gc"))
            .await
            .expect("会话");
        let live = 摆个profile(&state, &info.id);
        let orphan = 摆个profile(&state, "ses_早就没了");

        state.gc_browser_profiles().await;

        assert!(live.is_dir(), "活着的会话的 profile 不能动");
        assert!(!orphan.exists(), "没人认领的 profile 该收掉");
    }

    #[tokio::test]
    async fn 重命名与清除重命名() {
        let state = state().await;
        let info = state.create_session(&temp_ws("rn")).await.expect("会话");

        state
            .rename_session(&info.id, "  改过的名字  ")
            .await
            .expect("重命名");
        let listed = state.list_sessions().await;
        assert_eq!(listed[0].title.as_deref(), Some("改过的名字"));

        // 空白串 = 清除手动名。没有历史时回退到 None
        state.rename_session(&info.id, "   ").await.expect("清除");
        let listed = state.list_sessions().await;
        assert_eq!(listed[0].title, None);
    }

    /// 独立的临时配置路径（不建 AppState）。"重启"类测试要对同一个路径
    /// 构造两次 AppState，helper `state()` 每次换目录，模拟不了重启。
    fn temp_cfg(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "riot-restart-{tag}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("建临时配置目录");
        dir.join("config.json")
    }

    #[tokio::test]
    async fn 重启后会话和设置都在() {
        // 这是这个模块存在的理由：重启前的会话列表、标题、权限模式、
        // 会话设置，重启后必须原样回来。
        let cfg = temp_cfg("all");
        let ws = temp_ws("restart");

        let (id_a, id_b) = {
            let state = AppState::restore_at(cfg.clone());
            state.set_config(AppConfig::default()).await;
            let a = state.create_session(&ws).await.expect("a");
            let b = state.create_session(&ws).await.expect("b");
            state
                .rename_session(&a.id, "改过的名字")
                .await
                .expect("重命名");
            state
                .set_mode(&b.id, PermissionMode::BypassPermissions)
                .await
                .expect("切模式");
            state
                .set_system_prompt(&b.id, "测试要跑 pytest")
                .await
                .expect("提示词");
            (a.id, b.id)
        };

        // "重启"：对同一个路径重新构造
        let state = AppState::restore_at(cfg);
        let listed = state.list_sessions().await;
        assert_eq!(listed.len(), 2, "两个会话都要回来");
        assert_eq!(listed[0].id, id_a, "顺序要保住");
        assert_eq!(listed[1].id, id_b);
        assert_eq!(listed[0].title.as_deref(), Some("改过的名字"));
        assert_eq!(listed[0].root, listed[1].root);
        assert_eq!(
            listed[1].mode,
            PermissionMode::BypassPermissions,
            "权限模式要恢复 —— 显示得比实际更严是最坏的一种错"
        );
        assert_eq!(listed[1].system_prompt.as_deref(), Some("测试要跑 pytest"));
    }

    #[tokio::test]
    async fn 重启后新会话的序号接着排() {
        // seq 从 0 重新数的话，新会话会插到老会话前面，侧边栏顺序乱掉。
        let cfg = temp_cfg("seq");
        let ws = temp_ws("restart-seq");

        let old_id = {
            let state = AppState::restore_at(cfg.clone());
            state.set_config(AppConfig::default()).await;
            state.create_session(&ws).await.expect("老会话").id
        };

        let state = AppState::restore_at(cfg);
        state.set_config(AppConfig::default()).await;
        let new_id = state.create_session(&ws).await.expect("新会话").id;

        let ids: Vec<_> = state
            .list_sessions()
            .await
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(ids, vec![old_id, new_id], "新会话必须排在老会话后面");
    }

    #[tokio::test]
    async fn 删除的会话重启后不复活() {
        let cfg = temp_cfg("del");
        let ws = temp_ws("restart-del");
        let sessions_dir = cfg.parent().expect("有父目录").join("sessions");

        let kept = {
            let state = AppState::restore_at(cfg.clone());
            state.set_config(AppConfig::default()).await;
            let doomed = state.create_session(&ws).await.expect("要删的");
            let kept = state.create_session(&ws).await.expect("留下的");
            state.delete_session(&doomed.id).await;
            assert!(
                !sessions_dir.join(format!("{}.jsonl", doomed.id)).exists(),
                "删除会话必须连 transcript 一起删"
            );
            kept.id
        };

        let state = AppState::restore_at(cfg);
        let ids: Vec<_> = state
            .list_sessions()
            .await
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(ids, vec![kept], "删掉的不能复活");
    }

    // 「重启后历史能水合」的行为(resume 从 transcript 捞回历史 + 标题自愈)
    // 现在跨进程:水合在内核(riot-kernel 的 session 测试覆盖),标题自愈在
    // history() 收到应答之后 —— 端到端要真内核进程,单测框架里没有。

    #[tokio::test]
    async fn 索引损坏时从transcript重建_对话不丢() {
        use riot_protocol::id::{MessageId, SessionId};
        use riot_protocol::message::{MessageMeta, UserContent};

        let cfg = temp_cfg("rebuild");
        let ws = temp_ws("restart-rebuild");
        let sessions_dir = cfg.parent().expect("有父目录").join("sessions");

        let id = {
            let state = AppState::restore_at(cfg.clone());
            state.set_config(AppConfig::default()).await;
            let id = state.create_session(&ws).await.expect("会话").id;
            // `[约束]` 关掉这个 state 的写手再往下走。create_session 已经
            // 为这个 transcript 开了一条后台写通道（元数据首行是它写的），
            // 下面测试又开第二条 —— 同一个文件两个写手，谁先落盘没有保证。
            // 不关的话这个用例会随机失败在"标题没重建出来"上。
            state.shutdown().await;
            id
        };
        {
            let transcripts = riot_store::Transcripts::new(&sessions_dir);
            let log = transcripts.open(riot_store::TranscriptMeta {
                id: SessionId::from_raw(id.clone()),
                root: PathBuf::from(&ws),
                created_at_ms: 1,
            });
            log.append(&Message::User {
                id: MessageId::from_raw("m1"),
                content: vec![UserContent::Text {
                    text: "别丢了我".into(),
                }],
                meta: MessageMeta::default(),
            });
            log.flush().await;
        }

        // 索引写坏 —— transcript 是事实来源，会话必须从它捞回来
        std::fs::write(sessions_dir.join("index.json"), "{坏的").expect("写坏索引");

        let state = AppState::restore_at(cfg);
        let listed = state.list_sessions().await;
        assert_eq!(listed.len(), 1, "会话要从 transcript 重建回来");
        assert_eq!(listed[0].id, id);
        assert_eq!(listed[0].title.as_deref(), Some("别丢了我"));
        // 对话内容本身由内核水合(transcript 是事实来源),此处不验 ——
        // 那需要真内核进程,riot-kernel 的测试覆盖了水合。
    }

    #[tokio::test]
    async fn 移除项目连带关闭它的会话_不动别的项目() {
        let state = state().await;
        let ws_a = temp_ws("rm-a");
        let ws_b = temp_ws("rm-b");
        let a1 = state.create_session(&ws_a).await.expect("a1");
        let a2 = state.create_session(&ws_a).await.expect("a2");
        let b1 = state.create_session(&ws_b).await.expect("b1");

        let doomed = state.remove_project(&a1.root).await;

        assert_eq!(doomed.len(), 2, "A 项目的两个会话都该被关闭");
        assert!(doomed.contains(&a1.id) && doomed.contains(&a2.id));

        let alive: Vec<_> = state
            .list_sessions()
            .await
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(alive, vec![b1.id], "B 项目的会话必须原样活着");
    }
}
