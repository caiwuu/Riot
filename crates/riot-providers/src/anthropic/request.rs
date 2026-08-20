//! 请求组装。
//!
//! # paramsFromContext 模式
//!
//! `[约束]` 整个 crate 里**只有 [`build_request`] 一个地方组装请求**。
//! 重试时想换模型、调低 `max_tokens`、剥离签名，一律改 [`RetryContext`]
//! 重新调用它，不允许在重试分支里复制一份组包逻辑。
//!
//! 这条约束防的是一类很难查的 bug：重试路径的参数和首次请求悄悄不一致。
//! 表现是「重试之后行为变了」，但两边代码看起来都对。
//!
//! # 缓存断点
//!
//! `[约束]` **整个请求恰好一个 messages 断点**，打在最后一条消息上。
//! 多打几个不会让缓存更好用 —— 服务端会 400，而且 KV 页管理下多断点
//! 反而浪费。这个约束由 [`validate_cache_breakpoints`] 强制。
//!
//! 见 ARCHITECTURE.md §11.2、§11.5

use riot_protocol::message::{AssistantContent, Message, ToolResultContent, UserContent};
use riot_protocol::provider::{ProviderRequest, ThinkingConfig, ToolSpec};
use serde::Serialize;

/// 重试与降级时会变的参数。
///
/// 首次请求用 [`RetryContext::initial`]，重试时改这个结构再调
/// [`build_request`]，**不要**手动改已经组装好的请求。
#[derive(Debug, Clone)]
pub struct RetryContext {
    /// 覆盖模型。降级时用。
    pub model_override: Option<String>,
    /// 覆盖输出上限。输出耗尽或上下文溢出时调低。
    pub max_output_tokens_override: Option<u32>,
    /// 剥离 thinking 签名。
    ///
    /// `[约束]` 换模型时**必须**置位。签名与模型绑定，拿 A 模型的签名
    /// 去 B 模型重放会直接 400，而错误信息不会提到签名两个字。
    pub strip_thinking_signatures: bool,
    /// 不把自己的尾巴写进缓存。
    ///
    /// 摘要、fork 这类一次性请求用。它们的末尾内容不会被复用，
    /// 写进去只是把别人的缓存挤掉。此时断点移到倒数第二条。
    pub skip_cache_write: bool,
}

impl RetryContext {
    pub fn initial() -> Self {
        Self {
            model_override: None,
            max_output_tokens_override: None,
            strip_thinking_signatures: false,
            skip_cache_write: false,
        }
    }

    /// 降级到另一个模型。自动置位签名剥离 —— 这两件事必须一起做，
    /// 分开写迟早会漏。
    pub fn fallback_to(model: impl Into<String>) -> Self {
        Self {
            model_override: Some(model.into()),
            strip_thinking_signatures: true,
            ..Self::initial()
        }
    }
}

/// system prompt 的一段。
#[derive(Debug, Clone)]
pub struct SystemSection {
    pub name: &'static str,
    pub text: String,
    /// 这一段的内容在会话中途会不会变。
    ///
    /// `[约束]` 默认应该是 `false`（可缓存）。确实每轮都变的段落要显式标注
    /// 并写清理由 —— 「缓存是默认、不缓存要报备」这个方向，让缓存命中率
    /// 变成架构约束而不是事后优化。
    pub volatile: bool,
}

impl SystemSection {
    pub fn stable(name: &'static str, text: impl Into<String>) -> Self {
        Self {
            name,
            text: text.into(),
            volatile: false,
        }
    }

    /// 每轮都变的段落。名字起得长是故意的 —— 让 review 时一眼看见。
    pub fn dangerous_volatile(name: &'static str, text: impl Into<String>) -> Self {
        Self {
            name,
            text: text.into(),
            volatile: true,
        }
    }
}

// ────────────────────────────────────────────────────────────
// 线上格式
// ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WireRequest {
    pub model: String,
    pub max_tokens: u32,
    pub system: Vec<WireSystemBlock>,
    pub messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<WireTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<WireThinking>,
    /// 采样参数由 provider 配置注入。None 不发送。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    pub stream: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WireSystemBlock {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// `global` 让静态段跨会话跨用户共享。动态段不能用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<&'static str>,
}

impl CacheControl {
    fn global() -> Self {
        Self {
            kind: "ephemeral",
            scope: Some("global"),
        }
    }

    fn local() -> Self {
        Self {
            kind: "ephemeral",
            scope: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WireMessage {
    pub role: &'static str,
    pub content: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WireTool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireThinking {
    Enabled { budget_tokens: u32 },
}

/// 没有指定时的输出上限。
const DEFAULT_MAX_TOKENS: u32 = 8192;

// ────────────────────────────────────────────────────────────
// 组装
// ────────────────────────────────────────────────────────────

/// 力度档 → 思考预算。Anthropic 没有档位参数，只有 `budget_tokens`。
///
/// 数值参照 Claude Code 的 think/megathink 量级；high 刻意没顶到 CC 的
/// ultrathink（32k）—— 那是用户喊口令才给的量，当作常规档每轮都发太贵。
fn effort_budget(level: riot_protocol::provider::ThinkingEffort) -> u32 {
    use riot_protocol::provider::ThinkingEffort;
    match level {
        ThinkingEffort::Low => 2_048,
        ThinkingEffort::Medium => 8_192,
        ThinkingEffort::High => 24_576,
    }
}

/// 把预算夹进 API 的硬约束：`1024 ≤ budget < max_tokens`。
///
/// 挤不下（max_tokens 太小，思考和输出没法都留出最低空间）就返回 None
/// 不开思考 —— 发一个非法组合出去是整个请求 400，而不是思考被忽略。
fn clamp_thinking_budget(budget: u32, max_tokens: u32) -> Option<u32> {
    const MIN_BUDGET: u32 = 1_024;
    // 给最终输出至少留 MIN_BUDGET：预算贴着 max_tokens 的话，思考一满
    // 输出就被截断，表现为"想了半天一句话没说完"。
    let cap = max_tokens.saturating_sub(MIN_BUDGET);
    if cap < MIN_BUDGET {
        return None;
    }
    Some(budget.clamp(MIN_BUDGET, cap))
}

/// 组装一次请求。**这是唯一的组装入口。**
pub fn build_request(
    req: &ProviderRequest,
    system: &[SystemSection],
    retry: &RetryContext,
) -> WireRequest {
    let max_tokens = retry
        .max_output_tokens_override
        .or(req.max_output_tokens)
        .unwrap_or(DEFAULT_MAX_TOKENS);
    let out = WireRequest {
        model: retry
            .model_override
            .clone()
            .unwrap_or_else(|| req.model.clone()),
        max_tokens,
        system: build_system(system),
        messages: build_messages(&req.messages, retry),
        tools: build_tools(&req.tools),
        thinking: match req.thinking {
            // Anthropic 默认就不思考，Disabled 等价于不发。
            ThinkingConfig::Off | ThinkingConfig::Disabled => None,
            ThinkingConfig::Effort { level } => clamp_thinking_budget(effort_budget(level), max_tokens)
                .map(|budget_tokens| WireThinking::Enabled { budget_tokens }),
            ThinkingConfig::Budget { tokens } => clamp_thinking_budget(tokens, max_tokens)
                .map(|budget_tokens| WireThinking::Enabled { budget_tokens }),
        },
        temperature: None,
        top_p: None,
        top_k: None,
        stream: true,
    };

    debug_assert!(
        validate_cache_breakpoints(&out).is_ok(),
        "{:?}",
        validate_cache_breakpoints(&out)
    );
    out
}

/// 静态段合并成一块打全局缓存，动态段各自成块不打。
///
/// `[约束]` 静态段里不许放任何会在会话中途变化的东西（feature flag、
/// 时间戳、MCP 工具列表）。变一个字节，全局缓存就整个作废 —— 而这个
/// 损失是跨用户的，不只影响你自己。
fn build_system(sections: &[SystemSection]) -> Vec<WireSystemBlock> {
    let (stable, volatile): (Vec<_>, Vec<_>) = sections.iter().partition(|s| !s.volatile);

    let mut out = Vec::with_capacity(2);

    if !stable.is_empty() {
        out.push(WireSystemBlock {
            kind: "text",
            text: stable
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n"),
            cache_control: Some(CacheControl::global()),
        });
    }

    if !volatile.is_empty() {
        out.push(WireSystemBlock {
            kind: "text",
            text: volatile
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n"),
            cache_control: None,
        });
    }

    out
}

/// 工具声明。
///
/// `[约束]` 必须按名字排序。schema 的顺序抖一下，整个工具块的缓存就失效了 ——
/// 而顺序抖动在 HashMap 迭代下是随机发生的，表现为「缓存命中率时高时低」。
fn build_tools(tools: &[ToolSpec]) -> Vec<WireTool> {
    let mut out: Vec<WireTool> = tools
        .iter()
        .map(|t| WireTool {
            name: t.name.clone(),
            description: t.description.clone(),
            input_schema: t.input_schema.clone(),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 这些消息发出去有多少字节。
///
/// `[约束]` 走 [`build_messages`] 而不是直接量 `Message` —— 那是
/// [`riot_protocol::provider::Provider::count_tokens`] 那条约束的落地方式。
/// 复用组装（而不是另写一遍"哪些字段会发"）是刻意的:两套规则各自演化，
/// 迟早有一天线协议改了而估算没跟上，而那种偏差没有任何报错，只会表现成
/// 压缩时机变得莫名其妙。
///
/// 用 [`RetryContext::initial`]:重试那几个开关只影响模型名、输出上限和
/// 断点位置，对字节数的影响可以忽略，而估算发生在决定要不要压缩的时候 ——
/// 那时还没有任何重试上下文。
pub fn wire_bytes(messages: &[Message]) -> usize {
    build_messages(messages, &RetryContext::initial())
        .iter()
        .map(|m| serde_json::to_string(m).map(|s| s.len()).unwrap_or(0))
        .sum()
}

fn build_messages(messages: &[Message], retry: &RetryContext) -> Vec<WireMessage> {
    let mut out: Vec<WireMessage> = messages
        .iter()
        .filter_map(|m| convert_message(m, retry))
        .collect();

    // 断点打在哪一条：正常打最后一条，skip_cache_write 时打倒数第二条
    // （共享前缀的末尾），这样自己的尾巴不会写进缓存。
    let target = if retry.skip_cache_write {
        out.len().checked_sub(2)
    } else {
        out.len().checked_sub(1)
    };

    if let Some(idx) = target
        && let Some(msg) = out.get_mut(idx)
        && let Some(last_block) = msg.content.last_mut()
        && let Some(obj) = last_block.as_object_mut()
    {
        obj.insert(
            "cache_control".into(),
            serde_json::to_value(CacheControl::local()).expect("CacheControl 可序列化"),
        );
    }

    out
}

/// 转换一条消息。返回 `None` 表示这条不进请求。
///
/// `[约束]` **空 content 的消息必须丢掉。**Anthropic 拒绝空 content 数组，
/// 报错是笼统的 400，不会告诉你是第几条消息。而空消息不是假想 —— 模型返回
/// 空响应时，主循环就会产生一条 `Assistant { content: [] }`。
fn convert_message(m: &Message, retry: &RetryContext) -> Option<WireMessage> {
    let (role, content) = match m {
        // System 消息不送回模型。让模型看到「你上次请求失败了」这类元信息，
        // 它会开始为错误道歉而不是继续干活。由 INV-7 断言。
        Message::System { .. } => return None,

        Message::User { content, .. } => (
            "user",
            content.iter().map(convert_user_content).collect::<Vec<_>>(),
        ),

        Message::Assistant { content, .. } => (
            "assistant",
            content
                .iter()
                .map(|c| convert_assistant_content(c, retry))
                .collect::<Vec<_>>(),
        ),
    };

    (!content.is_empty()).then_some(WireMessage { role, content })
}

fn convert_user_content(c: &UserContent) -> serde_json::Value {
    match c {
        UserContent::Text { text } => serde_json::json!({ "type": "text", "text": text }),

        UserContent::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => serde_json::json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id.as_str(),
            "is_error": is_error,
            "content": convert_tool_result_content(content),
        }),

        UserContent::Attachment(a) => convert_attachment(a),
    }
}

fn convert_tool_result_content(c: &ToolResultContent) -> serde_json::Value {
    match c {
        ToolResultContent::Text { text } => {
            serde_json::json!([{ "type": "text", "text": text }])
        }
        ToolResultContent::Spilled {
            path,
            preview,
            total_bytes,
        } => serde_json::json!([{
            "type": "text",
            "text": format!(
                "结果过大（{total_bytes} 字节），已写入 {}。\n预览：\n{preview}",
                path.display()
            ),
        }]),
        // 压缩后的占位符。**不能整条删掉** —— 删了 tool_use 就成了孤儿。
        ToolResultContent::Cleared => {
            serde_json::json!([{ "type": "text", "text": "[结果已清理以节省上下文]" }])
        }
        // `path` 指向落盘的原图，是给界面的 —— 发给模型的只有 `data`
        // 里的压缩图（产出方已压好，见 riot-tools 的 shrink 模块）。
        ToolResultContent::Image {
            media_type,
            data,
            path: _,
        } => serde_json::json!([{
            "type": "image",
            "source": { "type": "base64", "media_type": media_type, "data": data },
        }]),
        // 转述代替图片：这个变体只在"当时的模型看不了图"的会话里产生，
        // 图片是给界面的，模型这边自始至终只有文字。中途切到能看图的模型
        // 也照发文字 —— 对话前文都建立在这段转述上，换成图反而变了口径。
        ToolResultContent::DescribedImage { text, .. } => {
            serde_json::json!([{ "type": "text", "text": text }])
        }
        // Set-of-Marks:一条 result 里图文都发。text 在前 —— 模型先读编号
        // 清单建立"[n] 是什么"，再看图上的框定位，两路对齐。
        ToolResultContent::MarkedImage { media_type, data, path: _, text } => {
            serde_json::json!([
                { "type": "text", "text": text },
                {
                    "type": "image",
                    "source": { "type": "base64", "media_type": media_type, "data": data },
                },
            ])
        }
    }
}

fn convert_attachment(a: &riot_protocol::message::Attachment) -> serde_json::Value {
    use riot_protocol::message::Attachment;
    let text = match a {
        Attachment::Memory { path, content } => {
            format!(
                "<system-reminder>\n项目记忆 {}：\n{content}\n</system-reminder>",
                path.display()
            )
        }
        Attachment::RestoredFile { path, content } => {
            format!(
                "<system-reminder>\n压缩前你读过 {}：\n{content}\n</system-reminder>",
                path.display()
            )
        }
        Attachment::UserFile { path, content } => {
            format!(
                "<system-reminder>\n用户在消息里引用了 {}，内容如下：\n{content}\n</system-reminder>",
                path.display()
            )
        }
        Attachment::Environment { text } => {
            format!("<system-reminder>\n{text}\n</system-reminder>")
        }
        Attachment::SystemReminder { text } => {
            format!("<system-reminder>\n{text}\n</system-reminder>")
        }
        // 视觉兼容：模型读转述，图片本体（`data`）只给界面，不发出去。
        Attachment::DescribedImage { text, .. } => {
            format!("<system-reminder>\n{text}\n</system-reminder>")
        }
        Attachment::Image { media_type, data } => {
            return serde_json::json!({
                "type": "image",
                "source": { "type": "base64", "media_type": media_type, "data": data },
            });
        }
    };
    serde_json::json!({ "type": "text", "text": text })
}

fn convert_assistant_content(c: &AssistantContent, retry: &RetryContext) -> serde_json::Value {
    match c {
        AssistantContent::Text { text } => serde_json::json!({ "type": "text", "text": text }),

        AssistantContent::Thinking { text, signature } => {
            // 换模型时必须剥离签名。签名与模型绑定，拿 A 的签名去 B 重放
            // 会 400，而错误信息里不会提到签名两个字。
            let sig = if retry.strip_thinking_signatures {
                None
            } else {
                signature.as_deref()
            };
            match sig {
                Some(s) => {
                    serde_json::json!({ "type": "thinking", "thinking": text, "signature": s })
                }
                // 没有签名的 thinking 块服务端不收，退化成普通文本。
                None => serde_json::json!({ "type": "text", "text": text }),
            }
        }

        AssistantContent::ToolUse { id, name, input } => serde_json::json!({
            "type": "tool_use",
            "id": id.as_str(),
            "name": name,
            "input": input,
        }),
    }
}

// ────────────────────────────────────────────────────────────
// 校验
// ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CacheError {
    #[error("messages 里有 {0} 个缓存断点，只允许 1 个")]
    TooManyMessageBreakpoints(usize),
    #[error("system 里有 {0} 个缓存断点，只允许 1 个")]
    TooManySystemBreakpoints(usize),
    #[error("动态 system 段打了缓存断点 —— 它每轮都变，缓存必然 miss")]
    VolatileSystemCached,
}

/// 断点数量校验。
///
/// 多打断点不是「缓存更多」，服务端会直接 400。而且这个错误只在
/// 真实请求时才暴露，本地测试完全看不到。
pub fn validate_cache_breakpoints(req: &WireRequest) -> Result<(), CacheError> {
    let system_marks = req
        .system
        .iter()
        .filter(|b| b.cache_control.is_some())
        .count();
    if system_marks > 1 {
        return Err(CacheError::TooManySystemBreakpoints(system_marks));
    }

    // 打了断点的 system 块必须是第一块（静态段）
    if let Some(pos) = req.system.iter().position(|b| b.cache_control.is_some())
        && pos != 0
    {
        return Err(CacheError::VolatileSystemCached);
    }

    let msg_marks: usize = req
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter(|c| c.get("cache_control").is_some())
        .count();
    if msg_marks > 1 {
        return Err(CacheError::TooManyMessageBreakpoints(msg_marks));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use riot_protocol::id::{MessageId, ToolUseId};
    use riot_protocol::message::MessageMeta;
    use pretty_assertions::assert_eq;

    fn user(text: &str) -> Message {
        Message::User {
            id: MessageId::from_raw("m"),
            content: vec![UserContent::Text { text: text.into() }],
            meta: MessageMeta::default(),
        }
    }

    fn assistant(content: Vec<AssistantContent>) -> Message {
        Message::Assistant {
            id: MessageId::from_raw("m"),
            content,
            usage: None,
            meta: MessageMeta::default(),
        }
    }

    fn req(messages: Vec<Message>) -> ProviderRequest {
        ProviderRequest {
            model: "claude-x".into(),
            messages,
            system: String::new(),
            tools: vec![],
            max_output_tokens: None,
            thinking: ThinkingConfig::Off,
        }
    }

    fn sections() -> Vec<SystemSection> {
        vec![
            SystemSection::stable("intro", "你是一个编码助手。"),
            SystemSection::stable("tools_policy", "搜索用 Grep，不要在 Bash 里跑 grep。"),
            SystemSection::dangerous_volatile("env", "当前时间：12:00"),
        ]
    }

    fn breakpoint_positions(r: &WireRequest) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (i, m) in r.messages.iter().enumerate() {
            for (j, c) in m.content.iter().enumerate() {
                if c.get("cache_control").is_some() {
                    out.push((i, j));
                }
            }
        }
        out
    }

    /// 视觉兼容那张图只发转述，base64 一个字节都不能出去。
    ///
    /// `[约束]` 附件里带着图片本体是给界面留的（切回会话要能重画用户发过
    /// 的图）。当成 image 块发出去的话，收不了图的模型这边会被服务方拒。
    #[test]
    fn 视觉兼容的图只发转述() {
        use riot_protocol::message::Attachment;

        let msg = Message::User {
            id: MessageId::from_raw("m"),
            content: vec![
                UserContent::Attachment(Attachment::DescribedImage {
                    media_type: "image/jpeg".into(),
                    data: "BASE64PAYLOAD".into(),
                    text: "用户附的第 1 张图：\n图里是一个两栏布局".into(),
                }),
                UserContent::Text {
                    text: "这里为什么错位".into(),
                },
            ],
            meta: MessageMeta::default(),
        };

        let r = build_request(&req(vec![msg]), &sections(), &RetryContext::initial());
        let wire = serde_json::to_string(&r.messages).expect("序列化");
        assert!(wire.contains("两栏布局"), "转述要发给模型：{wire}");
        assert!(
            !wire.contains("BASE64PAYLOAD"),
            "图片本体不能发出去：{wire}"
        );
    }

    #[test]
    fn 静态段合并打全局缓存_动态段不打() {
        let r = build_request(
            &req(vec![user("hi")]),
            &sections(),
            &RetryContext::initial(),
        );

        assert_eq!(r.system.len(), 2, "静态段合成一块，动态段一块");
        assert_eq!(r.system[0].cache_control, Some(CacheControl::global()));
        assert!(r.system[0].text.contains("编码助手"));
        assert!(r.system[0].text.contains("Grep"));

        assert_eq!(
            r.system[1].cache_control, None,
            "动态段每轮都变，打断点等于每轮写一次废缓存"
        );
        assert!(r.system[1].text.contains("12:00"));
    }

    #[test]
    fn messages_恰好一个断点且在最后() {
        let r = build_request(
            &req(vec![
                user("a"),
                assistant(vec![AssistantContent::Text { text: "ok".into() }]),
                user("b"),
            ]),
            &sections(),
            &RetryContext::initial(),
        );

        assert_eq!(
            breakpoint_positions(&r),
            vec![(2, 0)],
            "多打断点服务端会 400，而这个错误只在真实请求时才暴露"
        );
        assert_eq!(validate_cache_breakpoints(&r), Ok(()));
    }

    #[test]
    fn skip_cache_write_把断点移到倒数第二条() {
        let retry = RetryContext {
            skip_cache_write: true,
            ..RetryContext::initial()
        };
        let r = build_request(
            &req(vec![
                user("a"),
                assistant(vec![AssistantContent::Text { text: "ok".into() }]),
                user("b"),
            ]),
            &sections(),
            &retry,
        );

        assert_eq!(
            breakpoint_positions(&r),
            vec![(1, 0)],
            "摘要类请求的尾巴不会被复用，写进缓存只是把别人的挤掉"
        );
    }

    #[test]
    fn 空_content_的消息被丢掉() {
        // 模型返回空响应时主循环就会产生这种消息。
        // 发出去是笼统的 400，不会告诉你是第几条。
        let r = build_request(
            &req(vec![user("a"), assistant(vec![]), user("b")]),
            &sections(),
            &RetryContext::initial(),
        );

        assert_eq!(r.messages.len(), 2, "空 content 数组 Anthropic 不收");
        assert_eq!(
            breakpoint_positions(&r),
            vec![(1, 0)],
            "断点要落在过滤后的最后一条上"
        );
    }

    #[test]
    fn 只有一条消息时_skip_cache_write_不打断点() {
        let retry = RetryContext {
            skip_cache_write: true,
            ..RetryContext::initial()
        };
        let r = build_request(&req(vec![user("a")]), &sections(), &retry);
        assert!(breakpoint_positions(&r).is_empty(), "没有倒数第二条可打");
    }

    #[test]
    fn 空消息列表不会崩() {
        let r = build_request(&req(vec![]), &sections(), &RetryContext::initial());
        assert!(r.messages.is_empty());
    }

    #[test]
    fn 工具按名字排序() {
        let mut r = req(vec![user("hi")]);
        r.tools = vec![
            ToolSpec {
                name: "Write".into(),
                description: "w".into(),
                input_schema: serde_json::json!({}),
            },
            ToolSpec {
                name: "Bash".into(),
                description: "b".into(),
                input_schema: serde_json::json!({}),
            },
            ToolSpec {
                name: "Read".into(),
                description: "r".into(),
                input_schema: serde_json::json!({}),
            },
        ];
        let built = build_request(&r, &sections(), &RetryContext::initial());
        let names: Vec<&str> = built.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Bash", "Read", "Write"],
            "顺序抖一下整个工具块缓存就失效，而 HashMap 迭代顺序是随机的"
        );
    }

    #[test]
    fn 降级时自动剥离_thinking_签名() {
        let msgs = vec![assistant(vec![AssistantContent::Thinking {
            text: "想了想".into(),
            signature: Some("sig_from_opus".into()),
        }])];

        // 正常请求：签名保留
        let normal = build_request(&req(msgs.clone()), &sections(), &RetryContext::initial());
        assert_eq!(normal.messages[0].content[0]["type"], "thinking");
        assert_eq!(normal.messages[0].content[0]["signature"], "sig_from_opus");

        // 降级：签名剥离，thinking 退化成 text
        let fallback = build_request(
            &req(msgs),
            &sections(),
            &RetryContext::fallback_to("claude-sonnet"),
        );
        assert_eq!(
            fallback.messages[0].content[0]["type"], "text",
            "带着 A 模型的签名去 B 模型会 400，而报错里不会提签名两个字"
        );
        assert_eq!(fallback.model, "claude-sonnet");
    }

    #[test]
    fn fallback_构造器把换模型和剥签名绑在一起() {
        // 这两件事必须一起做，分开写迟早会漏一个
        let r = RetryContext::fallback_to("claude-sonnet");
        assert!(r.strip_thinking_signatures);
        assert_eq!(r.model_override.as_deref(), Some("claude-sonnet"));
    }

    #[test]
    fn system_消息不进请求() {
        let msgs = vec![
            user("a"),
            Message::System {
                id: MessageId::from_raw("s1"),
                level: riot_protocol::message::SystemLevel::Error,
                text: "上次请求失败了".into(),
            },
            user("b"),
        ];
        let r = build_request(&req(msgs), &sections(), &RetryContext::initial());

        assert_eq!(r.messages.len(), 2, "System 消息必须被过滤");
        let json = serde_json::to_string(&r).expect("序列化");
        assert!(
            !json.contains("上次请求失败了"),
            "让模型看到失败元信息，它会开始道歉而不是干活"
        );
    }

    #[test]
    fn 清理过的_tool_result_保留占位而不是消失() {
        let msgs = vec![
            assistant(vec![AssistantContent::ToolUse {
                id: ToolUseId::from_raw("tu_1"),
                name: "Read".into(),
                input: serde_json::json!({}),
            }]),
            Message::User {
                id: MessageId::from_raw("m2"),
                content: vec![UserContent::ToolResult {
                    tool_use_id: ToolUseId::from_raw("tu_1"),
                    content: ToolResultContent::Cleared,
                    is_error: false,
                }],
                meta: MessageMeta::default(),
            },
        ];
        let r = build_request(&req(msgs), &sections(), &RetryContext::initial());

        let result_block = &r.messages[1].content[0];
        assert_eq!(result_block["type"], "tool_result");
        assert_eq!(
            result_block["tool_use_id"], "tu_1",
            "删掉整条会让 tool_use 变成孤儿，下次请求 400"
        );
    }

    #[test]
    fn 重试只改_context_不改结果结构() {
        // paramsFromContext 的核心保证：同样的输入 + 同样的 context = 同样的请求
        let r = req(vec![user("hi")]);
        let a = build_request(&r, &sections(), &RetryContext::initial());
        let b = build_request(&r, &sections(), &RetryContext::initial());
        assert_eq!(a, b, "组装必须是纯函数，否则重试路径会悄悄和首次不一致");
    }

    #[test]
    fn 输出上限的优先级() {
        let mut r = req(vec![user("hi")]);
        assert_eq!(
            build_request(&r, &sections(), &RetryContext::initial()).max_tokens,
            DEFAULT_MAX_TOKENS
        );

        r.max_output_tokens = Some(4096);
        assert_eq!(
            build_request(&r, &sections(), &RetryContext::initial()).max_tokens,
            4096
        );

        let retry = RetryContext {
            max_output_tokens_override: Some(2048),
            ..RetryContext::initial()
        };
        assert_eq!(
            build_request(&r, &sections(), &retry).max_tokens,
            2048,
            "重试的覆盖值优先级最高"
        );
    }

    #[test]
    fn 校验能抓到多断点() {
        let mut r = build_request(
            &req(vec![user("a"), user("b")]),
            &sections(),
            &RetryContext::initial(),
        );
        // 手动多打一个 —— 模拟有人在别处又加了一处
        r.messages[0].content[0]
            .as_object_mut()
            .expect("是对象")
            .insert(
                "cache_control".into(),
                serde_json::json!({ "type": "ephemeral" }),
            );

        assert_eq!(
            validate_cache_breakpoints(&r),
            Err(CacheError::TooManyMessageBreakpoints(2))
        );
    }

    #[test]
    fn 校验能抓到动态段被缓存() {
        let mut r = build_request(&req(vec![user("a")]), &sections(), &RetryContext::initial());
        r.system[0].cache_control = None;
        r.system[1].cache_control = Some(CacheControl::global());

        assert_eq!(
            validate_cache_breakpoints(&r),
            Err(CacheError::VolatileSystemCached)
        );
    }

    #[test]
    fn thinking_配置透传() {
        let mut r = req(vec![user("hi")]);
        // max_tokens 要给足：预算必须小于它，不给的话默认 8192 会把 10_000 夹掉。
        r.max_output_tokens = Some(32_000);
        r.thinking = ThinkingConfig::Budget { tokens: 10_000 };
        assert_eq!(
            build_request(&r, &sections(), &RetryContext::initial()).thinking,
            Some(WireThinking::Enabled {
                budget_tokens: 10_000
            })
        );
    }

    /// 预算必须夹在 `1024 ≤ budget < max_tokens` 里。
    ///
    /// 不夹的话，high 档（24576）配上默认 max_tokens（8192）就是一个
    /// 必然 400 的组合 —— 用户看到的只是"开了思考请求就失败"。
    #[test]
    fn thinking_预算夹进_max_tokens() {
        use riot_protocol::provider::ThinkingEffort;

        // 默认 max_tokens 8192：high 档被夹到 8192 - 1024。
        let mut r = req(vec![user("hi")]);
        r.thinking = ThinkingConfig::Effort { level: ThinkingEffort::High };
        assert_eq!(
            build_request(&r, &sections(), &RetryContext::initial()).thinking,
            Some(WireThinking::Enabled { budget_tokens: 7_168 })
        );

        // max_tokens 小到挤不下思考 + 输出：不开思考，而不是发非法组合。
        r.max_output_tokens = Some(1_500);
        assert_eq!(
            build_request(&r, &sections(), &RetryContext::initial()).thinking,
            None
        );
    }

    /// Disabled 在 Anthropic 侧等价于不发 —— 它默认就不思考。
    #[test]
    fn thinking_disabled_不发参数() {
        let mut r = req(vec![user("hi")]);
        r.thinking = ThinkingConfig::Disabled;
        assert_eq!(
            build_request(&r, &sections(), &RetryContext::initial()).thinking,
            None
        );
    }
}
