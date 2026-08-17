//! 宿主 ↔ 内核的 JSON-RPC 协议。
//!
//! 传输是换行分隔的 JSON over stdio。阶段 A 内核以 library 形式内嵌，
//! 但所有调用仍然穿过这里定义的类型 —— 这样阶段 B 拆进程时
//! 只需要换一个 transport 实现。见 ARCHITECTURE.md §2.2

use crate::changes::FileChange;
use crate::event::AgentEvent;
use crate::id::{RequestId, SessionId, TurnId};
use crate::message::Message;
use crate::permission::{PermissionMode, PermissionResponse};
use crate::turn::{ModelEndpoint, QueuedSummary, TurnConfig, TurnInput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 宿主 → 内核。有返回值。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum RpcRequest {
    #[serde(rename = "session.create")]
    SessionCreate { cwd: PathBuf, model: String },
    /// 恢复/查询一个会话:不在内存就从 transcript 水合,已在内存就直接回
    /// 快照。宿主切回会话时调,幂等。
    #[serde(rename = "session.resume")]
    SessionResume { session_id: SessionId, cwd: PathBuf },
    #[serde(rename = "session.list")]
    SessionList,
    #[serde(rename = "session.delete")]
    SessionDelete { session_id: SessionId },

    #[serde(rename = "turn.submit")]
    TurnSubmit {
        session_id: SessionId,
        /// 用户原始输入(text/images/refs)。图片转述、`@` 展开、hook 都在内核做。
        input: crate::turn::TurnInput,
        /// 本轮的完整配置:模型端点、联网/视觉、limits、mode、会话设置。
        /// Box 是因为它比其它变体大得多,不装箱会把整个 enum 撑大。
        config: Box<TurnConfig>,
    },
    /// 中断当前轮。
    #[serde(rename = "turn.interrupt")]
    TurnInterrupt {
        session_id: SessionId,
        /// 用户插话时为 true —— UI 不显示"已中断"文案。
        interjection: bool,
    },
    /// 排队面板:列出等待注入的插话。(跑轮中的新消息经 turn.submit 自动
    /// 入队,drain 时绝不插在 tool_use 和 tool_result 之间 —— INV-2。)
    #[serde(rename = "queue.list")]
    QueueList { session_id: SessionId },
    /// 删一条排队插话。
    #[serde(rename = "queue.remove")]
    QueueRemove {
        session_id: SessionId,
        entry_id: String,
    },
    /// 撤回一条排队插话,拿回原始输入放回输入框编辑。
    #[serde(rename = "queue.take")]
    QueueTake {
        session_id: SessionId,
        entry_id: String,
    },

    /// 手动压缩(/compact)。带模型端点 —— 压缩要调 LLM。
    #[serde(rename = "session.compact")]
    SessionCompact {
        session_id: SessionId,
        model: Box<ModelEndpoint>,
    },
    /// 本会话的净改动(变更面板)。
    #[serde(rename = "session.changes")]
    SessionChanges { session_id: SessionId },
    /// 改会话标题。自定义标题会抑制自动起名。
    #[serde(rename = "session.set_title")]
    SessionSetTitle {
        session_id: SessionId,
        title: Option<String>,
    },

    /// 本会话已授权的网络主机。
    #[serde(rename = "scope.list")]
    ScopeList { session_id: SessionId },
    /// 撤销一个已授权主机。
    #[serde(rename = "scope.revoke")]
    ScopeRevoke {
        session_id: SessionId,
        host: String,
    },

    /// 让 MCP 连接对齐给定的服务器清单(宿主从设置里组好传入,内核不读
    /// 配置文件)。启动时和每次保存设置后调用。
    #[serde(rename = "mcp.reconcile")]
    McpReconcile { servers: Vec<McpServerSpec> },
    /// MCP 连接状态(设置页显示)。
    #[serde(rename = "mcp.status")]
    McpStatus,
    /// 手动重连一个 MCP 服务器。
    #[serde(rename = "mcp.restart")]
    McpRestart { id: String },

    #[serde(rename = "permission.respond")]
    PermissionRespond {
        request_id: RequestId,
        response: PermissionResponse,
    },

    #[serde(rename = "config.set_mode")]
    ConfigSetMode {
        session_id: SessionId,
        mode: PermissionMode,
    },
    #[serde(rename = "tools.list")]
    ToolsList { session_id: SessionId },

    /// 健康检查。宿主定期调用，无应答则重启内核。
    #[serde(rename = "kernel.ping")]
    KernelPing,

    /// 优雅关闭:内核 flush 会话、杀掉自己 spawn 的子进程,然后退出。
    /// 宿主关闭序列的第一步(见 ARCHITECTURE.md §2.3)。
    #[serde(rename = "kernel.shutdown")]
    KernelShutdown,
}

/// 内核 → 宿主，对 [`RpcRequest`] 的应答。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "result", content = "data", rename_all = "snake_case")]
pub enum RpcResponse {
    SessionCreated {
        session_id: SessionId,
    },
    SessionResumed {
        messages: Vec<Message>,
        /// 压缩边界之前的消息。模型看不见,界面画在分割线上面。
        archived: Vec<Message>,
        /// 有没有轮子在跑。决定界面显示停止键还是发送键。
        busy: bool,
        compacting: bool,
    },
    SessionList {
        sessions: Vec<SessionSummary>,
    },
    TurnStarted {
        turn_id: TurnId,
    },
    /// turn.submit 的应答。`queued_id` = Some 表示上一轮在跑、这条消息进了
    /// 插话队列(条目 id,前端排队面板据此跟踪);None = 直接开轮了。
    TurnSubmitted {
        queued_id: Option<String>,
    },
    QueueList {
        entries: Vec<QueuedSummary>,
    },
    /// queue.take 的应答。None = 条目已不在(被注入或被删)。
    QueueTaken {
        input: Option<TurnInput>,
    },
    /// queue.remove 的应答:是否真的删到了。
    Removed {
        removed: bool,
    },
    Changes {
        changes: Vec<FileChange>,
    },
    ScopeHosts {
        hosts: Vec<String>,
    },
    McpStatuses {
        servers: Vec<McpServerStatus>,
    },
    ToolsList {
        tools: Vec<ToolInfo>,
    },
    Pong {
        version: String,
    },
    /// 无返回数据的成功。
    Ok,
    Error {
        error: RpcError,
    },
}

/// 内核 → 宿主，单向推送。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum RpcNotification {
    /// 会话事件。**这是唯一的会话事件载体。**
    #[serde(rename = "event.agent")]
    Agent {
        session_id: SessionId,
        event: AgentEvent,
    },
    /// 内核级错误。fatal 时宿主应重启内核。
    #[serde(rename = "event.kernel_error")]
    KernelError { message: String, fatal: bool },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionSummary {
    pub session_id: SessionId,
    pub title: Option<String>,
    pub cwd: PathBuf,
    pub updated_at_ms: u64,
    pub message_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolInfo {
    pub name: String,
    pub user_facing_name: String,
    pub enabled: bool,
}

/// MCP 服务器的启动描述。宿主从设置里组好(过滤掉未启用/没填完的),
/// 内核只管照单连接 —— 内核不读配置文件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct McpServerSpec {
    /// 稳定标识,进工具名(`mcp__<id>__…`),也是权限规则的一部分。
    pub id: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// MCP 连接状态快照,给设置页看。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatus {
    pub id: String,
    /// `connecting` / `connected` / `failed`
    pub state: String,
    /// connected 时是服务器自报的名字和版本;failed 时是错误原因。
    pub detail: String,
    /// 对外的完整工具名(`mcp__…`)。
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RpcError {
    pub code: RpcErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RpcErrorCode {
    SessionNotFound,
    InvalidParams,
    /// 该会话已有一轮在运行。
    TurnInProgress,
    Internal,
}

/// 带 id 的信封。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RpcEnvelope<T> {
    pub id: u64,
    #[serde(flatten)]
    pub payload: T,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_uses_dotted_method_names() {
        let req = RpcRequest::SessionCreate {
            cwd: PathBuf::from("/tmp"),
            model: "test".into(),
        };
        let v: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(v["method"], "session.create");
        assert_eq!(v["params"]["cwd"], "/tmp");
    }

    #[test]
    fn envelope_flattens_payload() {
        let env = RpcEnvelope {
            id: 7,
            payload: RpcRequest::KernelPing,
        };
        let v: serde_json::Value = serde_json::to_value(&env).unwrap();
        assert_eq!(v["id"], 7);
        assert_eq!(v["method"], "kernel.ping");
    }
}
