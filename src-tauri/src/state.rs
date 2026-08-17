//! 宿主的共享状态。
//!
//! `Clone` 是浅拷贝（内部全是 `Arc`），因为 Tauri 的退出钩子拿不到
//! `State<'_, T>` 的所有权，只能克隆一份出来。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use tauri::ipc::Channel;
use tokio::sync::Mutex;

use riot_protocol::event::AgentEvent;
use riot_protocol::id::{IdGenerator, NanoIdGenerator};
use riot_protocol::message::Message;
use riot_protocol::permission::{PermissionMode, PermissionResponse};

use crate::config::AppConfig;
use crate::fence::Fence;
use crate::session::Session;
use crate::{HostError, HostResult};

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

/// 注册表里的一个会话：会话本体 + 只属于登记层的事实。
///
/// seq 和创建时间不放进 `Session`：它们是列表的排序依据，由登记方分配，
/// 会话自己既不产生也不消费。
struct Registered {
    session: Arc<Session>,
    /// 创建顺序号，前端按它排序。
    seq: u64,
    /// 创建时刻（Unix 毫秒）。进 transcript 元数据和索引。
    created_at_ms: u64,
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
    /// transcript 存取。`Arc` 因为每个会话的持久化通道都持有一份。
    transcripts: Arc<riot_store::Transcripts>,
    /// session_id → 事件出口。同一会话重复订阅取 epoch 最大的那个 ——
    /// 一个会话在 UI 上只有一个视图。
    sinks: Mutex<HashMap<String, Sink>>,
    /// session_id → 登记的会话。
    ///
    /// [约束] 每个会话在创建时绑定自己的项目根，之后不变。没有全局
    /// "当前工作区"—— 那个概念上一版有过，后果是换目录后旧会话
    /// 继续往旧目录写文件。多项目并行下它根本没法定义清楚。
    sessions: Mutex<HashMap<String, Registered>>,
    config: Mutex<Option<AppConfig>>,
    seq: AtomicU64,
    /// 索引写盘互斥。快照和写文件必须在同一临界区：否则两次并发保存
    /// 可能让旧快照后落盘，一次变更就这么静默丢了。
    index_lock: Mutex<()>,
    /// MCP 连接枢纽。应用级、会话共享（ARCHITECTURE.md §2.4）——
    /// 每会话各起一份的话，三个会话配三个服务器就是九个常驻子进程。
    mcp: Arc<riot_mcp::McpHub>,
    /// 终端面板。应用级：终端跟着应用走不跟着会话走（见 term.rs），
    /// 而模型起的服务要出现在用户眼前那一个面板里，不是各自一份。
    terminals: crate::term::Terminals,
}

impl Inner {
    fn at(config_path: PathBuf) -> Self {
        let sessions_dir = config_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("sessions");
        Self {
            transcripts: Arc::new(riot_store::Transcripts::new(&sessions_dir)),
            sessions_dir,
            config_path,
            sinks: Mutex::default(),
            sessions: Mutex::default(),
            config: Mutex::default(),
            seq: AtomicU64::default(),
            index_lock: Mutex::default(),
            mcp: Arc::new(riot_mcp::McpHub::new()),
            terminals: crate::term::Terminals::default(),
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

    /// 从指定配置路径恢复。路径参数化是为了能测（同 `config::load_at` 的理由）。
    ///
    /// 同步执行且不碰任何锁 —— 它跑在 Tauri runtime 起来之前。会话表在
    /// 构造 `Inner` 时整体塞入，历史留给各会话惰性水合。
    ///
    /// `[约束]` root 目录已经不存在的会话**照样恢复**。目录没了不等于对话
    /// 没价值 —— 历史仍然可读，工具会在使用时报真实错误；跳过它的话，
    /// 下一次索引落盘会把它永久抹掉，用户连历史都找不回。
    pub(crate) fn restore_at(config_path: PathBuf) -> Self {
        let inner = Inner::at(config_path);
        let index = crate::persist::load(&inner.sessions_dir, &inner.transcripts);

        let mut map = HashMap::new();
        let mut next_seq = 0u64;
        for p in index.sessions {
            next_seq = next_seq.max(p.seq + 1);
            let id = riot_protocol::id::SessionId::from_raw(p.id.clone());
            let log = inner.transcripts.open(riot_store::TranscriptMeta {
                id,
                root: PathBuf::from(&p.root),
                created_at_ms: p.created_at_ms,
            });
            let session = Session::restored(
                &p,
                PathBuf::from(&p.root),
                Some(crate::session::SessionPersist {
                    store: Arc::clone(&inner.transcripts),
                    log,
                }),
            );
            session.attach_terminals(inner.terminals.clone());
            map.insert(
                p.id.clone(),
                Registered {
                    session: Arc::new(session),
                    seq: p.seq,
                    created_at_ms: p.created_at_ms,
                },
            );
        }
        if !map.is_empty() {
            tracing::info!(count = map.len(), "从磁盘恢复了 {} 个会话", map.len());
        }

        Self(Arc::new(Inner {
            sessions: Mutex::new(map),
            seq: AtomicU64::new(next_seq),
            ..inner
        }))
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
        // 轮子持有的是会话的出口句柄（[`crate::session::SessionSink`]），
        // 不是某一个 channel —— 用户切走再切回来会换一个新的，不换过去
        // 的话这一轮剩下的事件（包括结束）全发给了没人听的旧 channel，
        // 界面就永远停在"它正在做事"。
        if let Some(r) = self.0.sessions.lock().await.get(&session_id) {
            r.session.attach_sink(channel);
        }
        g.insert(session_id, Sink { epoch });
        true
    }

    pub async fn config(&self) -> AppConfig {
        let mut g = self.0.config.lock().await;
        g.get_or_insert_with(|| {
            let (c, backup) = crate::config::load_at(&self.0.config_path);
            if let Some(b) = backup {
                crate::config::note_recovered(b);
            }
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
        let fence = Fence::new(root)?;
        let root = fence.root().display().to_string();

        let id = NanoIdGenerator.session_id();
        let seq = self.0.seq.fetch_add(1, Ordering::Relaxed);
        // 豁免理由：宿主层，持久化记录的是真实时刻，黄金回放不经过这里。
        #[allow(clippy::disallowed_methods)]
        let created_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let default_mode = self.config().await.default_mode;
        let log = self.0.transcripts.open(riot_store::TranscriptMeta {
            id: id.clone(),
            root: fence.root().to_path_buf(),
            created_at_ms,
        });
        let s = Arc::new(Session::new(
            id.clone(),
            fence.root().to_path_buf(),
            Some(crate::session::SessionPersist {
                store: Arc::clone(&self.0.transcripts),
                log,
            }),
        ));
        s.attach_terminals(self.0.terminals.clone());
        if let Some(m) = default_mode {
            s.set_mode(m).await;
        }
        self.0.sessions.lock().await.insert(
            id.as_str().to_owned(),
            Registered {
                session: Arc::clone(&s),
                seq,
                created_at_ms,
            },
        );
        self.persist_index().await;

        Ok(SessionInfo {
            id: id.as_str().to_owned(),
            root,
            title: None,
            seq,
            sampling: crate::config::Sampling::default(),
            // 从会话读回而不是直接用 default_mode：上面那个 if 是
            // Option，两边各写一遍迟早对不上。
            mode: s.mode().await,
            thinking: riot_protocol::ThinkingPolicy::default(),
            python_venv: None,
            system_prompt: None,
            busy: false,
        })
    }

    /// 所有活着的会话。前端启动或刷新（HMR）后用它对齐状态。
    pub async fn list_sessions(&self) -> Vec<SessionInfo> {
        let sessions: Vec<_> = self
            .0
            .sessions
            .lock()
            .await
            .iter()
            .map(|(id, r)| (id.clone(), Arc::clone(&r.session), r.seq))
            .collect();

        let mut out = Vec::with_capacity(sessions.len());
        for (id, s, seq) in sessions {
            out.push(SessionInfo {
                id,
                root: s.cwd.display().to_string(),
                title: s.title().await,
                seq,
                sampling: s.sampling().await,
                mode: s.mode().await,
                thinking: s.thinking().await,
                python_venv: s.python_venv().await,
                system_prompt: s.system_prompt_extra().await,
                busy: s.is_running().await,
            });
        }
        out.sort_by_key(|i| i.seq);
        out
    }

    /// 会话的对话历史，外加"此刻是否有轮子在跑"。
    ///
    /// 两样一起回：前端切回会话时要同时重建对话流和忙碌状态，分两次问
    /// 会在中间留一个窗口 —— 那一瞬间界面显示空闲，而模型正在干活。
    pub async fn history(&self, session_id: &str) -> HostResult<HistoryOut> {
        let s = self.session(session_id).await?;
        Ok(HistoryOut {
            messages: s.history().await,
            archived: s.ui_archive().await,
            busy: s.is_running().await,
            compacting: s.is_compacting(),
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
        if let Some(r) = removed {
            // 中断放在移除之后：轮子还持有 Arc<Session> 会把收尾跑完，
            // 但新的 send_turn 已经找不到它了。
            r.session.interrupt().await;
            // 先关句柄再删文件 —— Windows 删不掉还开着的文件。收尾中的
            // 轮子之后的追加会被静默丢弃：会话都删了，丢弃是正确行为。
            r.session.close_log().await;
            if let Err(e) = self.0.transcripts.remove(&r.session.id).await {
                tracing::warn!(error = %e, "transcript 删除失败，磁盘上可能留下孤儿文件");
            }
            crate::changes::remove_baselines(&crate::changes::baselines_path(
                &self.0.sessions_dir,
                session_id,
            ));
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
                tracing::info!(count = v.len(), "清掉了 {} 个没人认领的浏览器 profile", v.len());
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "清理孤儿 profile 的任务没跑完"),
        }
    }

    /// 重命名会话。空标题表示清除手动名，回退到第一条消息。
    pub async fn rename_session(&self, session_id: &str, title: &str) -> HostResult<()> {
        let s = self.session(session_id).await?;
        let t = title.trim();
        // 80 字够侧边栏显示三行了，再长就是粘贴错了东西
        s.set_title((!t.is_empty()).then(|| t.chars().take(80).collect()))
            .await;
        self.persist_index().await;
        Ok(())
    }

    /// 把会话表的当前状态写进索引。每次会话结构或元数据变化后调用。
    ///
    /// 整体重写而不是增量：索引就几 KB，原子替换没有"哪行说了算"的合并
    /// 问题。失败只告警 —— 索引丢了能从 transcript 重建，不值得为它
    /// 打断用户的操作。
    async fn persist_index(&self) {
        let _g = self.0.index_lock.lock().await;

        // 先出锁再逐个读会话字段，和 list_sessions 同一个模式：
        // 拿着 sessions 表的锁去等各会话的字段锁，是给死锁留门。
        let snapshot: Vec<_> = self
            .0
            .sessions
            .lock()
            .await
            .iter()
            .map(|(id, r)| (id.clone(), Arc::clone(&r.session), r.seq, r.created_at_ms))
            .collect();

        let mut sessions = Vec::with_capacity(snapshot.len());
        for (id, s, seq, created_at_ms) in snapshot {
            sessions.push(crate::persist::PersistedSession {
                id,
                root: s.cwd.display().to_string(),
                seq,
                created_at_ms,
                custom_title: s.custom_title().await,
                auto_title: s.auto_title().await,
                mode: s.mode().await,
                sampling: s.sampling().await,
                python_venv: s.python_venv().await,
                system_prompt: s.system_prompt_extra().await,
                thinking: s.thinking().await,
            });
        }
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
            .filter(|(_, r)| r.session.cwd.display().to_string() == root)
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
        self.session(id)
            .await?
            .panel_browser()
            .ok_or_else(|| HostError::Browser(riot_protocol::browser::BrowserUnavailable(
                "这个构建没有内置浏览器。开发时先跑 scripts/build-browser.sh。".into(),
            )))
    }

    async fn session(&self, id: &str) -> HostResult<Arc<Session>> {
        self.0
            .sessions
            .lock()
            .await
            .get(id)
            .map(|r| Arc::clone(&r.session))
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
        images: Vec<crate::session::ImageInput>,
        refs: Vec<String>,
    ) -> HostResult<Option<String>> {
        let session = self.session(session_id).await?;
        self.require_sink(session_id).await?;
        let sink = session.sink();

        // 每轮解析"此刻"的激活配置 —— 对话中途切换模型下一轮就生效。
        // 会话的采样覆盖叠在 provider 默认之上，只盖用户动过的字段。
        let config = self.config().await;
        let mut model = config.resolve()?;
        model.sampling = session.sampling().await.or(model.sampling);

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

        // UserPromptSubmit hooks：能把这条消息拦在发送之前（block），
        // 或给它附加模型可见的上下文。拦截当场报给界面 —— 这正是"提交
        // 检查"该有的样子，而不是消息发出去之后模型再说做不了。
        let mut extra_context = Vec::new();
        {
            let engine = crate::hooks::HookEngine::load(&session.cwd, session_id);
            if engine.has_user_prompt_submit() {
                for o in engine.user_prompt_submit(text).await {
                    match o {
                        crate::hooks::Outcome::Block { reason } => {
                            return Err(HostError::Hook(format!(
                                "消息被 UserPromptSubmit hook 拦下：{reason}"
                            )));
                        }
                        crate::hooks::Outcome::Context { text } => extra_context.push(text),
                        _ => {}
                    }
                }
            }
        }

        let input = crate::session::TurnInput {
            text: text.to_owned(),
            images,
            refs,
            extra_context,
        };

        // 联网能力同理，每轮按当时的配置现装：用户中途填上 SearXNG 地址，
        // 下一轮就能搜，不用重启。
        // 图片能力同理:用户中途给 provider 勾上「支持图片」、或者配了视觉
        // 兼容模型，下一轮就生效。
        // 外部工具同理：MCP 服务器中途连上/掉线、SKILL.md 中途改了，
        // 下一轮的工具清单就是新的。
        let mut extra_tools = self.0.mcp.tools().await;
        let skills = crate::skills::discover(&session.cwd);
        for p in &skills.problems {
            tracing::warn!(path = %p.path.display(), reason = %p.reason, "有技能没能加载");
        }
        // 只把模型能调的那些给 Skill 工具。写了 disable-model-invocation 的
        // 只出现在 `/` 菜单里 —— 拿全量的话那个开关就是骗人的。
        let model_cards = skills.model_cards();
        if !model_cards.is_empty() {
            // 没有技能就不装 Skill 工具 —— 一个"可用技能：（无）"的工具
            // 描述是每轮都付的上下文税，还会引诱模型去调它。
            extra_tools.push(Arc::new(riot_tools::tools::skill::SkillTool::new(model_cards)));
        }
        let caps = crate::session::TurnCapabilities {
            web: Arc::new(crate::web::HostWeb::from_config(&config)),
            vision: Arc::new(crate::vision::HostVision::from_config(&config)),
            subagent_cheap: crate::subagent::CheapModel::from_config(&config),
            // 判危分类器同理每轮现装。装不出来（没配便宜档）就给占位实现 ——
            // Auto 模式于是退化成 Default，照常弹窗，不会静默放行。
            classifier: crate::classifier::HostClassifier::from_config(&config).map_or_else(
                || Arc::new(riot_protocol::permission::NoClassifier)
                    as Arc<dyn riot_protocol::permission::SafetyClassifier>,
                |c| Arc::new(c) as Arc<dyn riot_protocol::permission::SafetyClassifier>,
            ),
            extra_tools,
        };
        let limits = crate::session::TurnLimits {
            ask_timeout_secs: config.ask_timeout_secs,
            max_turns: config.max_turns,
            compact_threshold_tokens: config.compact_threshold_tokens,
            sandbox: config.sandbox,
        };

        // 第一句话定下自动标题，立刻落索引 —— 重启后侧边栏就靠它显示名字。
        // 放在 spawn 之前：轮子是异步跑的，等它结束才写的话，中途强杀就
        // 是一个"有历史但没标题"的会话。
        if session.note_first_prompt(text).await {
            self.persist_index().await;
        }

        // submit 而不是 run_turn：上一轮还在跑时插话会排队（内核在安全点
        // 注入），而不是报错"上一轮还在进行中" —— 模型干活时说话是常态。
        // 开轮的话轮子已经被 submit 丢进后台，这里立刻返回。
        Ok(session.submit(input, model, caps, sink, limits).await)
    }

    /// 手动压缩会话历史（`/compact`）。完成时发 Compacted 事件。
    pub async fn compact_session(&self, session_id: &str) -> HostResult<()> {
        let session = self.session(session_id).await?;
        self.require_sink(session_id).await?;
        let sink = session.sink();
        let config = self.config().await;
        let mut model = config.resolve()?;
        model.sampling = session.sampling().await.or(model.sampling);
        session
            .compact_now(model, sink)
            .await
            .map_err(HostError::Provider)
    }

    /// `@` 补全菜单的文件搜索。查询为空时给项目里的前几个文件。
    pub async fn search_files(&self, session_id: &str, query: &str) -> HostResult<Vec<String>> {
        let session = self.session(session_id).await?;
        Ok(crate::mentions::search_files(&session.cwd, query).await)
    }

    /// 展开一条自定义命令。None = 没这条命令或它是内置的。
    pub async fn slash_expand(
        &self,
        session_id: &str,
        name: &str,
        args: &str,
    ) -> HostResult<Option<String>> {
        let session = self.session(session_id).await?;
        Ok(crate::slash::expand(Some(&session.cwd), name, args))
    }

    /// 排队面板：当前排着的插话。
    pub async fn queue_list(
        &self,
        session_id: &str,
    ) -> HostResult<Vec<crate::session::QueuedSummary>> {
        Ok(self.session(session_id).await?.queue_snapshot())
    }

    /// 删掉一条排队插话。false = 条目已经不在（被注入或早被删了）。
    pub async fn queue_remove(&self, session_id: &str, entry_id: &str) -> HostResult<bool> {
        Ok(self.session(session_id).await?.queue_remove(entry_id))
    }

    /// 撤回一条排队插话，还给前端原始输入（放回输入框编辑）。
    pub async fn queue_take(
        &self,
        session_id: &str,
        entry_id: &str,
    ) -> HostResult<Option<crate::session::QueuedInputOut>> {
        Ok(self
            .session(session_id)
            .await?
            .queue_take(entry_id)
            .map(|i| crate::session::QueuedInputOut {
                text: i.text,
                images: i.images,
                refs: i.refs,
            }))
    }

    pub async fn interrupt(&self, session_id: &str) -> HostResult<bool> {
        Ok(self.session(session_id).await?.interrupt().await)
    }

    /// 本会话改了哪些文件、哪些行。给 review 视图。
    pub async fn changes(&self, session_id: &str) -> HostResult<Vec<crate::changes::FileChange>> {
        Ok(self.session(session_id).await?.changes().await)
    }

    pub async fn set_mode(&self, session_id: &str, mode: PermissionMode) -> HostResult<()> {
        self.session(session_id).await?.set_mode(mode).await;
        self.persist_index().await;
        Ok(())
    }

    pub async fn scope_hosts(&self, session_id: &str) -> HostResult<Vec<String>> {
        Ok(self.session(session_id).await?.scope_hosts().await)
    }

    pub async fn revoke_scope(&self, session_id: &str, host: &str) -> HostResult<()> {
        self.session(session_id).await?.revoke_scope(host).await;
        Ok(())
    }

    /// 设置会话级采样覆盖。空字段继承 provider；下一轮生效。
    pub async fn set_sampling(
        &self,
        session_id: &str,
        sampling: crate::config::Sampling,
    ) -> HostResult<()> {
        self.session(session_id).await?.set_sampling(sampling).await;
        self.persist_index().await;
        Ok(())
    }

    /// 设置会话级思考策略。下一轮生效。
    pub async fn set_thinking(
        &self,
        session_id: &str,
        thinking: riot_protocol::ThinkingPolicy,
    ) -> HostResult<()> {
        self.session(session_id).await?.set_thinking(thinking).await;
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
        let s = self.session(session_id).await?;
        Ok([".venv", "venv"]
            .iter()
            .map(|name| s.cwd.join(name))
            .filter(|dir| venv_python(dir).exists())
            .map(|dir| dir.display().to_string())
            .collect())
    }

    /// 设置会话的 Python 虚拟环境。空字符串清除；下一轮生效。
    ///
    /// 在这里验证目录：venv 路径写错的表现是"pip 装到了系统环境里"，
    /// 那种失败要等模型跑完命令才暴露，而且报错完全不指向路径。
    pub async fn set_python_venv(&self, session_id: &str, path: &str) -> HostResult<()> {
        let s = self.session(session_id).await?;
        let trimmed = path.trim();
        if trimmed.is_empty() {
            s.set_python_venv(None).await;
            self.persist_index().await;
            return Ok(());
        }
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
        s.set_python_venv(Some(trimmed.to_owned())).await;
        self.persist_index().await;
        Ok(())
    }

    /// 设置会话级追加提示词。空字符串清除；下一轮生效。
    pub async fn set_system_prompt(&self, session_id: &str, prompt: &str) -> HostResult<()> {
        let p = prompt.trim();
        self.session(session_id)
            .await?
            .set_system_prompt((!p.is_empty()).then(|| p.to_owned()))
            .await;
        self.persist_index().await;
        Ok(())
    }

    pub async fn respond_permission(
        &self,
        session_id: &str,
        ask_id: &str,
        response: PermissionResponse,
    ) -> HostResult<()> {
        let s = self.session(session_id).await?;
        if !s.pending_asks().resolve(ask_id, response).await {
            // 不当成错误。用户在超时之后才点按钮是正常的人类行为。
            tracing::debug!(ask_id, "回应了一个已经不存在的权限请求");
        }
        Ok(())
    }

    /// 让 MCP 连接对齐当前配置。启动时和每次保存设置后调用。
    ///
    /// reconcile 本身只是 diff + spawn 连接任务，不等握手完成 ——
    /// 连接进度通过 [`Self::mcp_statuses`] 查询。
    pub async fn reconcile_mcp(&self) {
        let config = self.config().await;
        let specs: Vec<riot_mcp::ServerSpec> = config
            .mcp_servers
            .iter()
            // 空命令 = 刚添加还没填完的中间状态，跳过而不是报错 ——
            // 校验放行它正是为了让"添加"按钮能落地（见 config::validate_mcp）。
            .filter(|s| s.enabled && !s.command.trim().is_empty())
            .map(|s| riot_mcp::ServerSpec {
                id: s.id.clone(),
                command: s.command.clone(),
                args: s.args.clone(),
                env: s.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            })
            .collect();
        self.0.mcp.reconcile(specs).await;
    }

    /// 终端面板的句柄。Tauri 那边 `manage` 的和会话用的必须是同一份 ——
    /// 各拿各的话，模型起的服务会跑在一个用户永远看不到的面板里。
    pub fn terminals(&self) -> crate::term::Terminals {
        self.0.terminals.clone()
    }

    /// MCP 服务器的连接状态，给设置页看。
    pub async fn mcp_statuses(&self) -> Vec<riot_mcp::ServerStatus> {
        self.0.mcp.statuses().await
    }

    /// 手动重连一个 MCP 服务器。
    pub async fn mcp_restart(&self, id: &str) -> HostResult<()> {
        if self.0.mcp.restart(id).await {
            Ok(())
        } else {
            Err(HostError::Provider(format!(
                "没有叫「{id}」的 MCP 服务器在运行。先在设置里启用它。"
            )))
        }
    }

    pub async fn shutdown(&self) {
        let sessions: Vec<Arc<Session>> = self
            .0
            .sessions
            .lock()
            .await
            .values()
            .map(|r| Arc::clone(&r.session))
            .collect();
        for s in &sessions {
            s.interrupt().await;
        }
        // 中断只是发信号，正在收尾的轮子可能还有消息在持久化通道里排队。
        // 等它们真正落盘 —— 这个钩子的意义就是"退出前别丢东西"。
        for s in &sessions {
            s.flush_log().await;
        }
        // MCP 服务器是常驻子进程，进程组随宿主一起收掉 ——
        // 留下的孤儿会一直活到关机。
        self.0.mcp.shutdown().await;
    }
}

/// venv 根目录里 python 可执行文件的位置。
///
/// 探测和校验共用这一个定义 —— 各写一遍的话，某天有人只改了一边，
/// 表现是"探测说有，填进去却报不像虚拟环境"。
fn venv_python(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join(if cfg!(windows) { "Scripts" } else { "bin" })
        .join(if cfg!(windows) { "python.exe" } else { "python" })
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
        // 已经不听的 channel 了 —— 而且**全程没有任何报错**：send_turn
        // 查得到出口、整轮照常跑完、历史照常落盘。
        let state = state().await;
        let id = state.create_session(&temp_ws("epoch")).await.expect("会话").id;

        let (new_ch, new_hits) = probe();
        let (old_ch, old_hits) = probe();

        // 新的先落地，旧的后落地 —— 正是会出问题的那个顺序
        assert!(state.attach_sink(id.clone(), 2, new_ch).await);
        assert!(
            !state.attach_sink(id.clone(), 1, old_ch).await,
            "epoch 更小的订阅必须被拒绝"
        );

        // 走会话自己的出口句柄 —— 跑着的轮子用的就是它。
        state
            .session(&id)
            .await
            .expect("会话还在")
            .sink()
            .send(AgentEvent::Done {
                reason: riot_protocol::event::TerminalReason::Completed,
            })
            .expect("发送");

        assert_eq!(new_hits.load(Ordering::SeqCst), 1, "事件该进最新那个订阅");
        assert_eq!(old_hits.load(Ordering::SeqCst), 0, "旧订阅不该收到任何东西");
    }

    #[tokio::test]
    async fn 跑轮中途换订阅_事件跟着走() {
        // 用户切走再切回来时前端会换一个 channel，而轮子是在开轮那一刻
        // 拿到出口的。抓着旧 channel 不放的话，切回来看到的是一个永远
        // 停在"它正在做事"的界面 —— 轮子在跑，事件却发给了没人听的那头，
        // 连结束都收不到。
        let state = state().await;
        let id = state.create_session(&temp_ws("resub")).await.expect("会话").id;

        let (first, first_hits) = probe();
        assert!(state.attach_sink(id.clone(), 1, first).await);

        // 轮子在这一刻拿到出口句柄（send_turn 里就是这么拿的）。
        let session = state.session(&id).await.expect("会话");
        let sink = session.sink();

        // 前端切走又切回：换上新 channel。
        let (second, second_hits) = probe();
        assert!(state.attach_sink(id.clone(), 2, second).await);

        sink.send(AgentEvent::Done {
            reason: riot_protocol::event::TerminalReason::Completed,
        })
        .expect("发送");

        assert_eq!(second_hits.load(Ordering::SeqCst), 1, "该进新订阅");
        assert_eq!(first_hits.load(Ordering::SeqCst), 0, "旧订阅不该再收到");
    }

    #[tokio::test]
    async fn 更新的订阅可以顶掉旧的() {
        // 反方向也要成立，否则切走再切回来就再也收不到事件了。
        let state = state().await;
        let id = state.create_session(&temp_ws("epoch2")).await.expect("会话").id;

        let (first, first_hits) = probe();
        let (second, second_hits) = probe();

        assert!(state.attach_sink(id.clone(), 1, first).await);
        assert!(state.attach_sink(id.clone(), 2, second).await);

        // 走会话自己的出口句柄 —— 跑着的轮子用的就是它。
        state
            .session(&id)
            .await
            .expect("会话还在")
            .sink()
            .send(AgentEvent::Done {
                reason: riot_protocol::event::TerminalReason::Completed,
            })
            .expect("发送");

        assert_eq!(second_hits.load(Ordering::SeqCst), 1);
        assert_eq!(first_hits.load(Ordering::SeqCst), 0);
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

        let sa = state.session(&a.id).await.expect("取回 a");
        let sb = state.session(&b.id).await.expect("取回 b");
        assert_eq!(sa.cwd.display().to_string(), a.root);
        assert_eq!(sb.cwd.display().to_string(), b.root);
    }

    #[tokio::test]
    async fn 列表按创建顺序返回() {
        let state = state().await;
        let ws = temp_ws("seq");
        let first = state.create_session(&ws).await.expect("1");
        let second = state.create_session(&ws).await.expect("2");

        let ids: Vec<_> = state.list_sessions().await.into_iter().map(|i| i.id).collect();
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
        assert!(
            state
                .create_session("/definitely/not/a/real/path/xyz")
                .await
                .is_err(),
            "坏目录应该在创建时报错，而不是第一次工具调用才炸"
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
        let info = state.create_session(&temp_ws("prof-del")).await.expect("会话");
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
        let info = state.create_session(&temp_ws("prof-gc")).await.expect("会话");
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

        state.rename_session(&info.id, "  改过的名字  ").await.expect("重命名");
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
            state.rename_session(&a.id, "改过的名字").await.expect("重命名");
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

        let ids: Vec<_> = state.list_sessions().await.into_iter().map(|i| i.id).collect();
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
        let ids: Vec<_> = state.list_sessions().await.into_iter().map(|i| i.id).collect();
        assert_eq!(ids, vec![kept], "删掉的不能复活");
    }

    #[tokio::test]
    async fn 重启后历史能水合() {
        use riot_protocol::id::{MessageId, SessionId};
        use riot_protocol::message::{MessageMeta, UserContent};

        let cfg = temp_cfg("hydrate");
        let ws = temp_ws("restart-hydrate");
        let sessions_dir = cfg.parent().expect("有父目录").join("sessions");

        let id = {
            let state = AppState::restore_at(cfg.clone());
            state.set_config(AppConfig::default()).await;
            state.create_session(&ws).await.expect("会话").id
        };

        // 模拟跑过一轮：直接往它的 transcript 追加（宿主的 log 从没写过，
        // 文件没被占用，外部句柄安全）。
        {
            let transcripts = riot_store::Transcripts::new(&sessions_dir);
            let log = transcripts.open(riot_store::TranscriptMeta {
                id: SessionId::from_raw(id.clone()),
                root: PathBuf::from(&ws),
                created_at_ms: 1,
            });
            log.append(&Message::User {
                id: MessageId::from_raw("m1"),
                content: vec![UserContent::Text { text: "重启前说的话".into() }],
                meta: MessageMeta::default(),
            });
            log.flush().await;
        }

        let state = AppState::restore_at(cfg);
        let history = state.history(&id).await.expect("拿历史");
        assert_eq!(history.messages.len(), 1, "重启前的对话必须回来");
        assert!(matches!(
            &history.messages[0],
            Message::User { content, .. }
                if matches!(&content[0], UserContent::Text { text } if text == "重启前说的话")
        ));

        // 水合的自愈：索引里没有自动标题时，从历史里找回第一句话。
        let listed = state.list_sessions().await;
        assert_eq!(
            listed[0].title.as_deref(),
            Some("重启前说的话"),
            "水合后标题要自愈"
        );
    }

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
                content: vec![UserContent::Text { text: "别丢了我".into() }],
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

        let history = state.history(&id).await.expect("历史");
        assert_eq!(history.messages.len(), 1, "对话一条都不能丢");
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

        let alive: Vec<_> = state.list_sessions().await.into_iter().map(|i| i.id).collect();
        assert_eq!(alive, vec![b1.id], "B 项目的会话必须原样活着");
    }
}
