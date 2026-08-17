//! 内核进程的会话管理器:活会话注册表 + turn 驱动 + 事件出口。
//!
//! 阶段 B 里这是内核 bin 的核心 —— 宿主通过 JSON-RPC 让它建会话、跑轮次,
//! 会话事件经 [`RpcEventSink`] 变成 stdout 上的 `event.agent` 通知回流
//! (见 ARCHITECTURE.md §12)。
//!
//! 职责边界:会话运行时(活 [`Session`]、transcript 落盘、MCP 连接)在这里;
//! UI 元数据(侧边栏索引、标题、会话设置持久化)留宿主。宿主每轮把所需的
//! 配置(模型端点、联网/视觉、limits、mode、会话设置)打进 [`TurnConfig`]
//! 传进来,内核不读 config.json / auth.json。

// 装配层:用真实时钟/文件系统/进程,不参与黄金回放(确定性约束只针对
// riot-core 的主循环)。见 clippy.toml。
#![allow(clippy::disallowed_methods)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{Mutex, mpsc};

use riot_protocol::event::AgentEvent;
use riot_protocol::id::{IdGenerator, NanoIdGenerator, SessionId};
use riot_protocol::message::Message;
use riot_protocol::permission::{PermissionResponse, SafetyClassifier};
use riot_protocol::rpc::RpcNotification;
use riot_protocol::turn::{SandboxKind, TurnConfig, TurnInput as RpcTurnInput};

use crate::session::{
    EventSink, ImageInput, Session, SessionPersist, SinkClosed, TurnCapabilities, TurnInput,
    TurnLimits,
};

/// 出站通道:序列化好的一行 JSON 交给 serve 的 writer 任务写 stdout。
pub type Outbound = mpsc::UnboundedSender<String>;

/// 把会话事件发成 `event.agent` 通知到 stdout。
///
/// 每个会话在创建时 attach 一个,`session_id` 固定 —— 宿主据此把事件分发到
/// 对应的前端订阅。序列化失败 / 通道断开都回 `SinkClosed`,主循环据此中止
/// (没人听时继续跑只是白烧额度)。
struct RpcEventSink {
    session_id: SessionId,
    out: Outbound,
}

impl EventSink for RpcEventSink {
    fn send(&self, event: AgentEvent) -> Result<(), SinkClosed> {
        let note = RpcNotification::Agent {
            session_id: self.session_id.clone(),
            event,
        };
        let line = serde_json::to_string(&note).map_err(|_| SinkClosed)?;
        self.out.send(line).map_err(|_| SinkClosed)
    }
}

/// 活会话注册表。内核 bin 持有一个。
pub struct SessionManager {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    transcripts: Arc<riot_store::Transcripts>,
    mcp: Arc<riot_mcp::McpHub>,
    ids: Arc<NanoIdGenerator>,
    out: Outbound,
}

impl SessionManager {
    pub fn new(out: Outbound, sessions_dir: PathBuf) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            transcripts: Arc::new(riot_store::Transcripts::new(&sessions_dir)),
            mcp: Arc::new(riot_mcp::McpHub::new()),
            ids: Arc::new(NanoIdGenerator),
            out,
        }
    }

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    async fn get(&self, session_id: &str) -> Option<Arc<Session>> {
        self.sessions.lock().await.get(session_id).map(Arc::clone)
    }

    /// 建一个绑定 `cwd` 的活会话,attach 好事件出口。
    pub async fn create(&self, cwd: PathBuf) -> SessionId {
        let id = self.ids.session_id();
        let log = self.transcripts.open(riot_store::TranscriptMeta {
            id: id.clone(),
            root: cwd.clone(),
            created_at_ms: Self::now_ms(),
        });
        let session = Arc::new(Session::new(
            id.clone(),
            cwd,
            Some(SessionPersist {
                store: Arc::clone(&self.transcripts),
                log,
            }),
        ));
        session.attach_sink(Arc::new(RpcEventSink {
            session_id: id.clone(),
            out: self.out.clone(),
        }));
        self.sessions
            .lock()
            .await
            .insert(id.as_str().to_owned(), session);
        id
    }

    /// 恢复一个会话(建活会话 + 从 transcript 水合历史),返回历史给宿主显示。
    pub async fn resume(&self, session_id: &str, cwd: PathBuf) -> (Vec<Message>, Vec<Message>) {
        let id = SessionId::from_raw(session_id.to_owned());
        let log = self.transcripts.open(riot_store::TranscriptMeta {
            id: id.clone(),
            root: cwd.clone(),
            created_at_ms: Self::now_ms(),
        });
        let session = Arc::new(Session::new(
            id.clone(),
            cwd,
            Some(SessionPersist {
                store: Arc::clone(&self.transcripts),
                log,
            }),
        ));
        session.attach_sink(Arc::new(RpcEventSink {
            session_id: id.clone(),
            out: self.out.clone(),
        }));
        let history = session.history().await;
        let archived = session.ui_archive().await;
        self.sessions
            .lock()
            .await
            .insert(session_id.to_owned(), session);
        (history, archived)
    }

    pub async fn delete(&self, session_id: &str) {
        if let Some(s) = self.sessions.lock().await.remove(session_id) {
            s.interrupt().await;
            s.close_log().await;
            if let Err(e) = self.transcripts.remove(&s.id).await {
                tracing::warn!(error = %e, "transcript 删除失败");
            }
        }
    }

    pub async fn history(&self, session_id: &str) -> Option<(Vec<Message>, Vec<Message>)> {
        let s = self.get(session_id).await?;
        Some((s.history().await, s.ui_archive().await))
    }

    pub async fn interrupt(&self, session_id: &str) -> bool {
        match self.get(session_id).await {
            Some(s) => s.interrupt().await,
            None => false,
        }
    }

    /// 回应一个权限请求。request_id 属于哪个会话未知,逐个尝试 resolve ——
    /// 命中一个即返回(request_id 全局唯一,只可能落在一个会话的待答表里)。
    pub async fn respond_permission(&self, request_id: &str, response: PermissionResponse) {
        let sessions: Vec<Arc<Session>> = self
            .sessions
            .lock()
            .await
            .values()
            .map(Arc::clone)
            .collect();
        for s in sessions {
            if s.pending_asks().resolve(request_id, response.clone()).await {
                return;
            }
        }
    }

    /// 提交一轮:从 [`TurnConfig`] 现装能力,跑主循环,事件经出口回流。
    /// 返回 `Some(条目 id)` = 上一轮在跑、进了插话队列;`None` = 直接开轮。
    pub async fn submit(
        &self,
        session_id: &str,
        input: RpcTurnInput,
        config: TurnConfig,
    ) -> Result<Option<String>, String> {
        let session = self.get(session_id).await.ok_or("会话不存在")?;

        // 会话设置:宿主是权威,每轮现设。ExitPlanMode 在内核改的 mode 会经
        // ModeChanged 事件回流宿主,下一轮再传回来。
        session.set_mode(config.mode).await;
        session.set_python_venv(config.python_venv.clone()).await;
        session
            .set_system_prompt(config.system_prompt_extra.clone())
            .await;
        session.set_thinking(config.thinking).await;

        // 能力现装(和内嵌期 send_turn 同一套,只是从 setup 而非 AppConfig)。
        let web = Arc::new(crate::web::HostWeb::from_setup(&config.web));
        let vision = Arc::new(crate::vision::HostVision::from_setup(&config.vision));
        let cheap = crate::subagent::CheapModel::from_endpoint(config.cheap_model.as_ref());
        let classifier: Arc<dyn SafetyClassifier> =
            crate::classifier::HostClassifier::from_cheap(cheap.as_ref())
                .map(|c| Arc::new(c) as Arc<dyn SafetyClassifier>)
                .unwrap_or_else(|| Arc::new(riot_protocol::permission::NoClassifier));

        let mut extra_tools = self.mcp.tools().await;
        let skills = crate::skills::discover(&session.cwd);
        let model_cards = skills.model_cards();
        if !model_cards.is_empty() {
            extra_tools.push(Arc::new(riot_tools::tools::skill::SkillTool::new(
                model_cards,
            )));
        }

        let caps = TurnCapabilities {
            web,
            vision,
            subagent_cheap: cheap,
            classifier,
            extra_tools,
        };
        let limits = TurnLimits {
            ask_timeout_secs: config.limits.ask_timeout_secs,
            max_turns: config.limits.max_turns,
            compact_threshold_tokens: config.limits.compact_threshold_tokens,
            sandbox: sandbox_mode(config.limits.sandbox),
        };

        // UserPromptSubmit hooks:能拦下这条消息或给它附加上下文。
        let mut extra_context = Vec::new();
        let engine = crate::hooks::HookEngine::load(&session.cwd, session_id);
        if engine.has_user_prompt_submit() {
            for o in engine.user_prompt_submit(&input.text).await {
                match o {
                    crate::hooks::Outcome::Block { reason } => {
                        return Err(format!("消息被 UserPromptSubmit hook 拦下：{reason}"));
                    }
                    crate::hooks::Outcome::Context { text } => extra_context.push(text),
                    _ => {}
                }
            }
        }

        let session_input = TurnInput {
            text: input.text,
            images: input
                .images
                .into_iter()
                .map(|i| ImageInput {
                    media_type: i.media_type,
                    data: i.data,
                })
                .collect(),
            refs: input.refs,
            extra_context,
        };
        let sink = session.sink();
        Ok(session
            .submit(session_input, config.model, caps, sink, limits)
            .await)
    }

    /// 关闭:中断所有会话、flush、收 MCP。宿主 kernel.shutdown 调它。
    pub async fn shutdown(&self) {
        let sessions: Vec<Arc<Session>> = self
            .sessions
            .lock()
            .await
            .values()
            .map(Arc::clone)
            .collect();
        for s in &sessions {
            s.interrupt().await;
        }
        for s in &sessions {
            s.flush_log().await;
        }
        self.mcp.shutdown().await;
    }
}

fn sandbox_mode(kind: SandboxKind) -> crate::config::SandboxMode {
    match kind {
        SandboxKind::WorkspaceWrite => crate::config::SandboxMode::WorkspaceWrite,
        SandboxKind::WorkspaceWriteNoNet => crate::config::SandboxMode::WorkspaceWriteNoNet,
        SandboxKind::Off => crate::config::SandboxMode::Off,
    }
}
