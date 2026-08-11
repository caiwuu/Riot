//! 宿主 ↔ 内核的 JSON-RPC 协议。
//!
//! 传输是换行分隔的 JSON over stdio。阶段 A 内核以 library 形式内嵌，
//! 但所有调用仍然穿过这里定义的类型 —— 这样阶段 B 拆进程时
//! 只需要换一个 transport 实现。见 ARCHITECTURE.md §2.2

use crate::event::AgentEvent;
use crate::id::{RequestId, SessionId, TurnId};
use crate::message::{Message, UserContent};
use crate::permission::{PermissionMode, PermissionResponse};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 宿主 → 内核。有返回值。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum RpcRequest {
    #[serde(rename = "session.create")]
    SessionCreate { cwd: PathBuf, model: String },
    #[serde(rename = "session.resume")]
    SessionResume { session_id: SessionId },
    #[serde(rename = "session.list")]
    SessionList,
    #[serde(rename = "session.delete")]
    SessionDelete { session_id: SessionId },

    #[serde(rename = "turn.submit")]
    TurnSubmit {
        session_id: SessionId,
        content: Vec<UserContent>,
    },
    /// 中断当前轮。
    #[serde(rename = "turn.interrupt")]
    TurnInterrupt {
        session_id: SessionId,
        /// 用户插话时为 true —— UI 不显示"已中断"文案。
        interjection: bool,
    },
    /// 运行中排队消息。会在工具结果全部就位后 drain，
    /// 绝不能插在 tool_use 和 tool_result 之间（INV-2）。
    #[serde(rename = "turn.queue_message")]
    TurnQueueMessage {
        session_id: SessionId,
        content: Vec<UserContent>,
    },

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
    },
    SessionList {
        sessions: Vec<SessionSummary>,
    },
    TurnStarted {
        turn_id: TurnId,
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
