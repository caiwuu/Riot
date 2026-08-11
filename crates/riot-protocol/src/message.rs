//! 会话消息。进 transcript、可持久化、可回放、可送回模型的东西。
//!
//! 与 [`crate::event::AgentEvent`] 的区别：Message 是数据平面，
//! AgentEvent 是包含数据平面在内的整个输出通道。Delta 和 Progress
//! 只是 AgentEvent，不是 Message —— 它们不进 transcript。

use crate::id::{MessageId, ToolUseId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    User {
        id: MessageId,
        content: Vec<UserContent>,
        #[serde(default, skip_serializing_if = "MessageMeta::is_default")]
        meta: MessageMeta,
    },
    Assistant {
        id: MessageId,
        content: Vec<AssistantContent>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(default, skip_serializing_if = "MessageMeta::is_default")]
        meta: MessageMeta,
    },
    /// 仅展示给用户，**不送回模型**。
    ///
    /// 违反这条会污染上下文，并且模型会开始把系统提示当成用户指令。
    /// 由 INV-7 断言保证。
    System {
        id: MessageId,
        level: SystemLevel,
        text: String,
    },
}

impl Message {
    pub fn id(&self) -> &MessageId {
        match self {
            Message::User { id, .. }
            | Message::Assistant { id, .. }
            | Message::System { id, .. } => id,
        }
    }

    /// 是否应当出现在发给模型的请求里。
    pub fn goes_to_model(&self) -> bool {
        !matches!(self, Message::System { .. })
    }

    pub fn tool_use_ids(&self) -> Vec<&ToolUseId> {
        match self {
            Message::Assistant { content, .. } => content
                .iter()
                .filter_map(|c| match c {
                    AssistantContent::ToolUse { id, .. } => Some(id),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn tool_result_ids(&self) -> Vec<&ToolUseId> {
        match self {
            Message::User { content, .. } => content
                .iter()
                .filter_map(|c| match c {
                    UserContent::ToolResult { tool_use_id, .. } => Some(tool_use_id),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserContent {
    Text {
        text: String,
    },
    ToolResult {
        tool_use_id: ToolUseId,
        content: ToolResultContent,
        is_error: bool,
    },
    /// 文件引用、图片、系统提醒。展开时机由上下文管理层决定。
    Attachment(Attachment),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantContent {
    Text {
        text: String,
    },
    Thinking {
        text: String,
        /// 签名与模型绑定。换模型前必须剥离，否则 API 400。
        /// 由 INV-9 断言保证。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    ToolUse {
        id: ToolUseId,
        name: String,
        input: serde_json::Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultContent {
    Text {
        text: String,
    },
    /// 结果过大已落盘，模型收到路径与预览。
    Spilled {
        path: PathBuf,
        preview: String,
        total_bytes: u64,
    },
    /// 历史清理后的占位符（microcompact 产物）。
    Cleared,
    Image {
        media_type: String,
        data: String,
    },
}

impl ToolResultContent {
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text { text: s.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Attachment {
    /// 项目记忆文件（AGENTS.md 等）。包在 system-reminder 里注入首条 user 消息。
    Memory {
        path: PathBuf,
        content: String,
    },
    /// 压缩后重注入的工作集文件。
    RestoredFile {
        path: PathBuf,
        content: String,
    },
    /// 环境快照：cwd、git 状态、平台。
    Environment {
        text: String,
    },
    /// 给模型的带外提示，不是用户说的话。
    SystemReminder {
        text: String,
    },
    Image {
        media_type: String,
        data: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SystemLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MessageMeta {
    /// 该消息由哪个 agent 产生。None = 主 agent。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<crate::id::AgentId>,
    /// 是否为系统合成（而非模型产出或用户输入）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub synthetic: bool,
    /// API 错误产生的消息。**这类消息上绝不能跑 stop hooks**，
    /// 否则会形成 error → hook 注入 → 重试 → error 的死循环。
    /// 由 INV-6 断言保证。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_api_error: bool,
    /// 产生这条消息的模型。thinking signature 与模型绑定，
    /// 降级换模型时要靠这个字段找出需要剥离签名的消息。
    /// 由 INV-9 断言保证。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_origin: Option<String>,
}

impl MessageMeta {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Token 用量。
///
/// 注意：流式 API 报的是**累计值不是增量**。`message_delta` 里的
/// input/cache 字段可能回 0，直接覆盖会抹掉 `message_start` 的真值。
/// 累加时用 [`Usage::merge`]，它对这些字段做了 `> 0` 守卫。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_creation_tokens: u32,
    pub cache_read_tokens: u32,
}

impl Usage {
    /// 用新报告的用量更新自身。
    ///
    /// output_tokens 直接取新值（它是单调增的累计值）；
    /// 其余字段只在新值 > 0 时才覆盖 —— 这是防止 `message_delta`
    /// 的 0 值抹掉 `message_start` 真值的关键守卫。
    pub fn merge(&mut self, incoming: &Usage) {
        if incoming.input_tokens > 0 {
            self.input_tokens = incoming.input_tokens;
        }
        if incoming.cache_creation_tokens > 0 {
            self.cache_creation_tokens = incoming.cache_creation_tokens;
        }
        if incoming.cache_read_tokens > 0 {
            self.cache_read_tokens = incoming.cache_read_tokens;
        }
        self.output_tokens = incoming.output_tokens;
    }

    pub fn total(&self) -> u32 {
        self.input_tokens + self.output_tokens + self.cache_creation_tokens + self.cache_read_tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn system_messages_never_go_to_model() {
        let m = Message::System {
            id: MessageId::from_raw("m1"),
            level: SystemLevel::Warning,
            text: "switched model".into(),
        };
        assert!(!m.goes_to_model());
    }

    #[test]
    fn usage_merge_guards_against_zero_overwrite() {
        let mut u = Usage {
            input_tokens: 5000,
            cache_read_tokens: 12000,
            output_tokens: 10,
            ..Default::default()
        };
        // message_delta 常见形态：只带 output，其余为 0
        u.merge(&Usage {
            output_tokens: 250,
            ..Default::default()
        });

        assert_eq!(u.input_tokens, 5000, "input 被 0 抹掉了");
        assert_eq!(u.cache_read_tokens, 12000, "cache_read 被 0 抹掉了");
        assert_eq!(u.output_tokens, 250);
    }

    #[test]
    fn tool_ids_are_extractable() {
        let m = Message::Assistant {
            id: MessageId::from_raw("m1"),
            content: vec![
                AssistantContent::Text { text: "ok".into() },
                AssistantContent::ToolUse {
                    id: ToolUseId::from_raw("t1"),
                    name: "Read".into(),
                    input: serde_json::json!({}),
                },
            ],
            usage: None,
            meta: MessageMeta::default(),
        };
        assert_eq!(m.tool_use_ids(), vec![&ToolUseId::from_raw("t1")]);
    }
}
