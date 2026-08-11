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
}

/// 一个会话的事件出口，带上它是"第几次订阅"。
///
/// 光存 `Channel` 不够 —— 见 [`AppState::attach_sink`]，宿主没法从两个
/// 并发到达的订阅里看出谁更新。
struct Sink {
    /// 前端给的单调递增序号。只用来比新旧，不解释具体数值。
    epoch: u64,
    channel: Channel<AgentEvent>,
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
    /// session_id → 事件出口。同一会话重复订阅取 epoch 最大的那个 ——
    /// 一个会话在 UI 上只有一个视图。
    sinks: Mutex<HashMap<String, Sink>>,
    /// session_id → (会话, 创建序号)。
    ///
    /// [约束] 每个会话在创建时绑定自己的项目根，之后不变。没有全局
    /// "当前工作区"—— 那个概念上一版有过，后果是换目录后旧会话
    /// 继续往旧目录写文件。多项目并行下它根本没法定义清楚。
    sessions: Mutex<HashMap<String, (Arc<Session>, u64)>>,
    config: Mutex<Option<AppConfig>>,
    seq: AtomicU64,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            config_path: crate::config::config_path(),
            sinks: Mutex::default(),
            sessions: Mutex::default(),
            config: Mutex::default(),
            seq: AtomicU64::default(),
        }
    }
}

#[derive(Clone, Default)]
pub struct AppState(Arc<Inner>);

impl AppState {
    /// 配置读写指向 `p` 而不是用户真实的配置文件。**只给测试用。**
    #[cfg(test)]
    fn with_config_path(p: PathBuf) -> Self {
        Self(Arc::new(Inner {
            config_path: p,
            ..Default::default()
        }))
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
        g.insert(session_id, Sink { epoch, channel });
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
        let default_mode = self.config().await.default_mode;
        let s = Arc::new(Session::new(id.clone(), fence.root().to_path_buf()));
        if let Some(m) = default_mode {
            s.set_mode(m).await;
        }
        self.0
            .sessions
            .lock()
            .await
            .insert(id.as_str().to_owned(), (Arc::clone(&s), seq));

        Ok(SessionInfo {
            id: id.as_str().to_owned(),
            root,
            title: None,
            seq,
            sampling: crate::config::Sampling::default(),
            // 从会话读回而不是直接用 default_mode：上面那个 if 是
            // Option，两边各写一遍迟早对不上。
            mode: s.mode().await,
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
            .map(|(id, (s, seq))| (id.clone(), Arc::clone(s), *seq))
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
            });
        }
        out.sort_by_key(|i| i.seq);
        out
    }

    pub async fn history(&self, session_id: &str) -> HostResult<Vec<Message>> {
        Ok(self.session(session_id).await?.history().await)
    }

    /// 删除会话：中断正在跑的轮子，摘掉事件出口。
    ///
    /// 幂等 —— 删一个不存在的会话是成功，不是错误。用户连点两次删除、
    /// 或者两个窗口先后删同一个，第二次都不该弹报错。
    pub async fn delete_session(&self, session_id: &str) {
        let removed = self.0.sessions.lock().await.remove(session_id);
        self.0.sinks.lock().await.remove(session_id);
        if let Some((s, _)) = removed {
            // 中断放在移除之后：轮子还持有 Arc<Session> 会把收尾跑完，
            // 但新的 send_turn 已经找不到它了。
            s.interrupt().await;
        }
    }

    /// 重命名会话。空标题表示清除手动名，回退到第一条消息。
    pub async fn rename_session(&self, session_id: &str, title: &str) -> HostResult<()> {
        let s = self.session(session_id).await?;
        let t = title.trim();
        // 80 字够侧边栏显示三行了，再长就是粘贴错了东西
        s.set_title((!t.is_empty()).then(|| t.chars().take(80).collect()))
            .await;
        Ok(())
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
            .filter(|(_, (s, _))| s.cwd.display().to_string() == root)
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
            .map(|(s, _)| Arc::clone(s))
            .ok_or(HostError::NoSession)
    }

    async fn sink(&self, id: &str) -> HostResult<Channel<AgentEvent>> {
        self.0
            .sinks
            .lock()
            .await
            .get(id)
            .map(|s| s.channel.clone())
            .ok_or(HostError::NoSink)
    }

    /// 发起一轮。
    ///
    /// `[约束]` 不等它跑完就返回。整轮可能要几分钟，而 Tauri 的 command
    /// 阻塞住会让前端的 `await invoke(...)` 一直挂着 —— 用户按不了停止键，
    /// 而停止键正是这种时候最需要的东西。
    pub async fn send_turn(&self, session_id: &str, text: &str) -> HostResult<String> {
        let session = self.session(session_id).await?;
        let sink = self.sink(session_id).await?;
        let text = text.to_owned();

        // 每轮解析"此刻"的激活配置 —— 对话中途切换模型下一轮就生效。
        // 会话的采样覆盖叠在 provider 默认之上，只盖用户动过的字段。
        let config = self.config().await;
        let mut model = config.resolve()?;
        model.sampling = session.sampling().await.or(model.sampling);

        // 联网能力同理，每轮按当时的配置现装：用户中途填上 SearXNG 地址，
        // 下一轮就能搜，不用重启。
        let web = Arc::new(crate::web::HostWeb::from_config(&config))
            as Arc<dyn riot_protocol::web::WebAccess>;
        let ask_timeout = config.ask_timeout_secs;

        tokio::spawn(async move {
            if let Err(e) = session
                .run_turn(text, model, web, sink.clone(), ask_timeout)
                .await
            {
                tracing::error!(error = %e, "本轮失败");
                // 把失败也送到 UI。静默失败的表现是"发了消息没反应"，
                // 那种情况用户只会以为程序坏了。
                let _ = sink.send(AgentEvent::Done {
                    reason: riot_protocol::event::TerminalReason::Error {
                        error: riot_protocol::event::AgentError::Internal { message: e },
                    },
                });
            }
        });

        Ok(session_id.to_owned())
    }

    pub async fn interrupt(&self, session_id: &str) -> HostResult<()> {
        self.session(session_id).await?.interrupt().await;
        Ok(())
    }

    pub async fn set_mode(&self, session_id: &str, mode: PermissionMode) -> HostResult<()> {
        self.session(session_id).await?.set_mode(mode).await;
        Ok(())
    }

    /// 设置会话级采样覆盖。空字段继承 provider；下一轮生效。
    pub async fn set_sampling(
        &self,
        session_id: &str,
        sampling: crate::config::Sampling,
    ) -> HostResult<()> {
        self.session(session_id).await?.set_sampling(sampling).await;
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

    pub async fn shutdown(&self) {
        for (_, (s, _)) in self.0.sessions.lock().await.iter() {
            s.interrupt().await;
        }
    }
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

        state
            .sink(&id)
            .await
            .expect("出口还在")
            .send(AgentEvent::Done {
                reason: riot_protocol::event::TerminalReason::Completed,
            })
            .expect("发送");

        assert_eq!(new_hits.load(Ordering::SeqCst), 1, "事件该进最新那个订阅");
        assert_eq!(old_hits.load(Ordering::SeqCst), 0, "旧订阅不该收到任何东西");
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

        state
            .sink(&id)
            .await
            .expect("出口还在")
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
