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

    /// 补上产生时刻（见 [`MessageMeta::created_at_ms`]）。
    ///
    /// 已经打过的不动：同一条消息可能被重新写回历史（定稿、修复、重放），
    /// 每次都盖一遍的话，界面上一条三小时前的提问会随着重启变成"刚刚"。
    ///
    /// System 消息没有 meta，跳过 —— 它在界面上是一条通知，不是对话气泡，
    /// 没有承载时间的位置。
    pub fn stamp(&mut self, now_ms: u64) {
        match self {
            Message::User { meta, .. } | Message::Assistant { meta, .. } => {
                meta.created_at_ms.get_or_insert(now_ms);
            }
            Message::System { .. } => {}
        }
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

    /// 上下文编辑：把消息的文本段替换成一段新文本。
    ///
    /// 只动文本：思考（签名与模型绑定）、工具调用/结果（配对不能断）、
    /// 附件（图和系统注入）一律原位保留。多个文本段合并成一段，落在原
    /// 第一个文本段的位置；本来没有文本段时，Assistant 插在第一个工具
    /// 调用前（模型输出的自然顺序），User 插在最前。
    ///
    /// 返回 `false` = System 消息，没有可编辑的文本，原样未动。
    ///
    /// `[约束]` 内核的内存操作和 store 的加载重放都走这一个函数 ——
    /// 两边各写一份的话，重启后的历史和编辑当刻的历史差一个字都算 bug。
    pub fn edit_text(&mut self, new_text: &str) -> bool {
        match self {
            Message::User { content, .. } => {
                let at = content
                    .iter()
                    .position(|c| matches!(c, UserContent::Text { .. }))
                    .unwrap_or(0);
                content.retain(|c| !matches!(c, UserContent::Text { .. }));
                content.insert(
                    at.min(content.len()),
                    UserContent::Text {
                        text: new_text.to_owned(),
                    },
                );
                true
            }
            Message::Assistant { content, .. } => {
                let at = content
                    .iter()
                    .position(|c| matches!(c, AssistantContent::Text { .. }))
                    .or_else(|| {
                        content
                            .iter()
                            .position(|c| matches!(c, AssistantContent::ToolUse { .. }))
                    })
                    .unwrap_or(content.len());
                content.retain(|c| !matches!(c, AssistantContent::Text { .. }));
                content.insert(
                    at.min(content.len()),
                    AssistantContent::Text {
                        text: new_text.to_owned(),
                    },
                );
                true
            }
            Message::System { .. } => false,
        }
    }

    /// 真正的用户提问：有正文、附图或 `@` 文件的用户消息。工具结果的
    /// 合成消息、纯系统注入都不算。
    ///
    /// 这是"一轮问答"的边界判定：上下文删除按轮成对删（提问连同它引出
    /// 的全部回应），轮的起点就是提问、终点是下一条提问。内核删除和
    /// 重新生成的截断（`cut_at_user_prompt`）都以它为准 —— 两边各写
    /// 一份的话，"一轮"的边界会在某天悄悄分叉。
    pub fn is_user_prompt(&self) -> bool {
        match self {
            Message::User { content, .. } => content.iter().any(|c| match c {
                UserContent::Text { text } => !text.trim().is_empty(),
                UserContent::Attachment(
                    Attachment::Image { .. }
                    | Attachment::DescribedImage { .. }
                    | Attachment::UserFile { .. },
                ) => true,
                _ => false,
            }),
            _ => false,
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
    /// 图 + 配套文字，两路一起发。和 [`Image`](Self::Image) 一样，图是给
    /// **能看图**的模型的（压缩图放 `data`，原图落盘走 `path`）；`text` 是
    /// 和图同属一个结果的文字：Set-of-Marks 截图的编号清单（模型看图上
    /// 第 [n] 个框、照清单查 [n] 是什么）、MCP 工具图文混合结果的文本部分。
    /// 给纯文本模型时产出方不该用这个变体（图对它没用）：Set-of-Marks 退回
    /// [`Text`](Self::Text)，MCP 走视觉兼容（[`DescribedImage`](Self::DescribedImage)）。
    MarkedImage {
        media_type: String,
        data: String,
        path: Option<PathBuf>,
        /// 配套文字。Set-of-Marks 场景是编号清单（编号同
        /// [`crate::browser::MarkedView`]），MCP 场景是结果的文本内容块。
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
    /// 用户附的图 + 它的文字转述。主模型收不了图时（视觉兼容）的产物，
    /// 语义同 [`ToolResultContent::DescribedImage`]：**模型只读 `text`**，
    /// 图片本体只给界面。
    ///
    /// 和 `SystemReminder` 分开是因为图得留下来。转述本身塞进 SystemReminder
    /// 就够模型用了，但那样图片本体在这一步就没了 —— 实时路径靠乐观回显
    /// 还看得见，切回会话之后用户自己发过的图就再也找不回来（真实发生过）。
    DescribedImage {
        /// `data` 的类型（客户端压缩产物通常是 image/jpeg）。
        media_type: String,
        data: String,
        /// 给模型的转述，自带"当作亲眼所见"的使用指示。
        text: String,
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
    /// 这条回答是被用户按停止**截断**的，不是模型自己说完的。
    ///
    /// 只给界面标注用。模型那边不需要额外说明 —— 它看到的就是一句
    /// 半截话后面紧跟着用户的下一条消息，而 meta 从来不进 wire 格式。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupted: bool,
    /// 这条消息产生的时刻（Unix 毫秒）。只给界面显示用。
    ///
    /// `None` = 这条消息早于本字段（老 transcript），界面那里不显示时间
    /// —— 编一个出来的话，几个月前的对话会全部标成"刚刚"。
    ///
    /// 由内核在消息进历史/落盘的同一处打上（见 [`Message::stamp`]）。
    /// 不在 provider 解码层打：那里拿不到注入的 `Clock`，要打就得让每个
    /// provider 都持有一份时钟，顺带把黄金回放的确定性也搭进去。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_ms: Option<u64>,
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

    /// 编辑只换文本，思考和工具调用原位不动。
    ///
    /// 思考签名与模型绑定、工具调用与结果配对 —— 编辑碰了它们，轻则
    /// 服务方 400，重则历史重放对不上。文本的位置也要保住：模型输出的
    /// 自然顺序是 thinking → text → tool_use，编辑不该把它重排。
    #[test]
    fn edit_text_only_touches_text() {
        let mut m = Message::Assistant {
            id: MessageId::from_raw("m1"),
            content: vec![
                AssistantContent::Thinking {
                    text: "想一想".into(),
                    signature: Some("sig".into()),
                },
                AssistantContent::Text {
                    text: "旧话".into(),
                },
                AssistantContent::ToolUse {
                    id: ToolUseId::from_raw("t1"),
                    name: "Read".into(),
                    input: serde_json::json!({}),
                },
            ],
            usage: None,
            meta: MessageMeta::default(),
        };
        assert!(m.edit_text("新话"));
        let Message::Assistant { content, .. } = &m else {
            unreachable!()
        };
        assert!(matches!(&content[0], AssistantContent::Thinking { .. }));
        assert!(matches!(&content[1], AssistantContent::Text { text } if text == "新话"));
        assert!(matches!(&content[2], AssistantContent::ToolUse { .. }));
    }

    /// 用户消息的编辑保留附件（图和 `@` 文件）：用户改的是字，不是图。
    #[test]
    fn edit_text_keeps_user_attachments() {
        let mut m = Message::User {
            id: MessageId::from_raw("m1"),
            content: vec![
                UserContent::Text {
                    text: "旧话".into(),
                },
                UserContent::Attachment(Attachment::Image {
                    media_type: "image/png".into(),
                    data: "x".into(),
                }),
            ],
            meta: MessageMeta::default(),
        };
        assert!(m.edit_text("新话"));
        let Message::User { content, .. } = &m else {
            unreachable!()
        };
        assert_eq!(content.len(), 2);
        assert!(matches!(&content[0], UserContent::Text { text } if text == "新话"));
        assert!(matches!(&content[1], UserContent::Attachment(_)));
    }

    /// 轮边界的判定：真实输入（文字/图/`@` 文件）算提问，工具结果的
    /// 合成消息和纯系统注入不算。
    ///
    /// 判错的代价在两头：把 tool_result 当提问，成对删除会把一轮从中间
    /// 劈开（配对断、下一轮 400）；把带图无文字的输入不当提问，那一轮
    /// 会被并进上一轮，删上一轮把它也吞掉。
    #[test]
    fn user_prompt_boundary() {
        let prompt = Message::User {
            id: MessageId::from_raw("m1"),
            content: vec![UserContent::Text { text: "问题".into() }],
            meta: MessageMeta::default(),
        };
        assert!(prompt.is_user_prompt());

        let image_only = Message::User {
            id: MessageId::from_raw("m2"),
            content: vec![UserContent::Attachment(Attachment::Image {
                media_type: "image/png".into(),
                data: "x".into(),
            })],
            meta: MessageMeta::default(),
        };
        assert!(image_only.is_user_prompt(), "只发图也是提问");

        let tool_result = Message::User {
            id: MessageId::from_raw("m3"),
            content: vec![UserContent::ToolResult {
                tool_use_id: ToolUseId::from_raw("t1"),
                content: ToolResultContent::text("ok"),
                is_error: false,
            }],
            meta: MessageMeta::default(),
        };
        assert!(!tool_result.is_user_prompt(), "工具结果不是提问");

        let reminder_only = Message::User {
            id: MessageId::from_raw("m4"),
            content: vec![UserContent::Attachment(Attachment::SystemReminder {
                text: "提醒".into(),
            })],
            meta: MessageMeta::default(),
        };
        assert!(!reminder_only.is_user_prompt(), "纯系统注入不是提问");

        let system = Message::System {
            id: MessageId::from_raw("m5"),
            level: SystemLevel::Info,
            text: "提示".into(),
        };
        assert!(!system.is_user_prompt());
    }

    /// 老 transcript 里没有 `created_at_ms`，照样要能读回来。
    ///
    /// 缺字段让整份 transcript 解析失败，用户看到的是"升级之后聊天记录
    /// 全没了"—— 比不显示时间严重得多。
    #[test]
    fn old_transcript_without_timestamp_still_loads() {
        let line = r#"{"role":"assistant","id":"m1","content":[{"type":"text","text":"hi"}],"meta":{"interrupted":true}}"#;
        let m: Message = serde_json::from_str(line).expect("老格式缺 created_at_ms 也要能读");
        let Message::Assistant { meta, .. } = &m else {
            unreachable!()
        };
        assert!(meta.interrupted);
        assert_eq!(meta.created_at_ms, None, "老消息不该被编出一个时间");
    }

    /// 打戳只补空缺。重放、修复、定稿都会把同一条消息再写一次历史，
    /// 每次盖新值的话，三小时前的提问会在重启后显示成"刚刚"。
    #[test]
    fn stamp_does_not_overwrite() {
        let mut m = Message::User {
            id: MessageId::from_raw("m1"),
            content: vec![UserContent::Text { text: "问题".into() }],
            meta: MessageMeta::default(),
        };
        m.stamp(1_000);
        m.stamp(9_999);
        let Message::User { meta, .. } = &m else {
            unreachable!()
        };
        assert_eq!(meta.created_at_ms, Some(1_000));

        // System 没有 meta，打戳是空操作而不是 panic。
        let mut sys = Message::System {
            id: MessageId::from_raw("m2"),
            level: SystemLevel::Info,
            text: "提示".into(),
        };
        sys.stamp(1_000);
    }
}
