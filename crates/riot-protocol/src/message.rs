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
    /// 图片结果。`data` 是**给模型的那份**：产出方（截图、读图）会先把
    /// 大图压到适合视觉模型的尺寸再放进来 —— 一张整页截图直接进上下文
    /// 能吃掉小半个窗口，而模型判断布局并不需要原始分辨率。
    Image {
        /// `data` 的类型（压缩产物通常是 image/jpeg），不一定等于原图类型。
        media_type: String,
        data: String,
        /// 原图的位置：截图落盘的文件，或被读的图片本身。界面优先按它
        /// 显示原图（清晰、可另存）；`None` 表示没落成盘，界面显示
        /// `data` 里的压缩图兜底。模型不用这个字段。
        path: Option<PathBuf>,
    },
    /// 图片 + 它的文字转述。主模型收不了图时（视觉兼容，见 vision 模块）
    /// 的产物：**模型只读 `text`**（转述代替图片，provider 不发图），
    /// 图片本体留给界面贴出来 —— 用户看得见图，而不是看见一段写给模型的
    /// 转述文字。
    DescribedImage {
        /// `data` 的类型（压缩产物通常是 image/jpeg），不一定等于原图类型。
        media_type: String,
        /// 压缩图。界面在 `path` 缺失时用它兜底显示。
        data: String,
        /// 原图位置，语义同 [`Image::path`](ToolResultContent::Image)。
        path: Option<PathBuf>,
        /// 给模型的转述，自带"当作亲眼所见"的使用指示。
        text: String,
    },
    /// 图 + 编号清单（Set-of-Marks）。和 [`Image`](Self::Image) 一样，图是
    /// 给**能看图**的模型的（压缩图放 `data`，原图落盘走 `path`）；但多带一段
    /// `text`（编号清单），两路一起发:模型看图上第 [n] 个框、照清单查 [n] 是
    /// 什么。给纯文本模型时产出方不该用这个变体（图对它没用），退回
    /// [`Text`](Self::Text)。
    MarkedImage {
        media_type: String,
        data: String,
        path: Option<PathBuf>,
        /// 编号清单，和图上的框一一对应；编号同 [`crate::browser::MarkedView`]。
        text: String,
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
    /// 用户在消息里用 `@路径` 点名的文件。
    ///
    /// 和 `RestoredFile` 分开是因为读者不同：那个是"你之前读过"，
    /// 这个是"用户现在让你看"。界面也靠它在用户气泡下列出引用了哪些
    /// 文件 —— 混进 SystemReminder 的话，切回会话就只剩一段光秃秃的
    /// 提醒文本，看不出用户当时附了什么。
    UserFile {
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
