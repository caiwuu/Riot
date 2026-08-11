//! 内部消息格式 → OpenAI chat completions 请求。
//!
//! 内部用的是 Anthropic 风格的 content block（一条消息里可以同时有文本和
//! 若干 tool_use）。OpenAI 是另一套形状：工具结果是**独立的 role=tool 消息**，
//! 工具调用挂在 assistant 消息的 `tool_calls` 上。
//!
//! 这个文件就是那层翻译。它有几条不能违反的规矩，违反了服务端只会回一句
//! 语焉不详的 `invalid request`：
//!
//! 1. `role=tool` 的消息必须紧跟在带 `tool_calls` 的 assistant 消息之后，
//!    并且每个 `tool_call_id` 都要有恰好一条对应的 tool 消息；
//! 2. 消息的 `content` 不能是空字符串；
//! 3. `tool_calls[].function.arguments` 是 JSON **字符串**，不是对象；
//! 4. DeepSeek 的 `reasoning_content` 不能回传。

use riot_protocol::message::{AssistantContent, Message, ToolResultContent, UserContent};
use riot_protocol::provider::ProviderRequest;

use super::wire::{
    StreamOptions, WireFunctionCall, WireMessage, WireRequest, WireTool, WireToolCall,
    WireToolFunction,
};
use crate::anthropic::request::SystemSection;

/// 重试时对请求的调整。
#[derive(Debug, Clone, Default)]
pub struct RetryContext {
    /// 降级到别的模型。
    pub model_override: Option<String>,
    /// 上下文溢出恢复时调低。
    pub max_tokens_override: Option<u32>,
}

impl RetryContext {
    pub fn initial() -> Self {
        Self::default()
    }

    pub fn fallback_to(model: impl Into<String>) -> Self {
        Self {
            model_override: Some(model.into()),
            ..Self::default()
        }
    }
}

pub fn build_request(
    req: &ProviderRequest,
    system: &[SystemSection],
    ctx: &RetryContext,
) -> WireRequest {
    let mut messages = Vec::new();

    // OpenAI 只有一条 system 消息，没有分段缓存的概念。
    // 段落拼起来时保持顺序 —— 前缀稳定，服务端的自动前缀缓存才有得命中。
    let system_text = system
        .iter()
        .map(|s| s.text.as_str())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    let full_system = match (system_text.is_empty(), req.system.is_empty()) {
        (true, true) => String::new(),
        (true, false) => req.system.clone(),
        (false, true) => system_text,
        (false, false) => format!("{system_text}\n\n{}", req.system),
    };
    if !full_system.is_empty() {
        messages.push(WireMessage::System {
            content: full_system,
        });
    }

    messages.extend(convert_messages(&req.messages));

    // 工具按名字排序。顺序不稳的话，服务端的前缀缓存每轮都会失效。
    let mut tools: Vec<WireTool> = req
        .tools
        .iter()
        .map(|t| WireTool {
            kind: "function",
            function: WireToolFunction {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.input_schema.clone(),
            },
        })
        .collect();
    tools.sort_by(|a, b| a.function.name.cmp(&b.function.name));

    WireRequest {
        model: ctx
            .model_override
            .clone()
            .unwrap_or_else(|| req.model.clone()),
        messages,
        stream: true,
        max_tokens: ctx.max_tokens_override.or(req.max_output_tokens),
        temperature: None,
        top_p: None,
        tools,
        stream_options: Some(StreamOptions {
            include_usage: true,
        }),
    }
}

pub fn convert_messages(messages: &[Message]) -> Vec<WireMessage> {
    let mut out = Vec::new();

    for m in messages {
        match m {
            Message::User { content, .. } => {
                // 工具结果先出。一条内部 User 消息里可能既有工具结果又有
                // 用户新说的话（用户在工具跑的时候插了一句），而 OpenAI 要求
                // tool 消息紧跟 assistant，中间不能插 user。
                let mut texts: Vec<String> = Vec::new();
                for c in content {
                    match c {
                        UserContent::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
                            out.push(WireMessage::Tool {
                                tool_call_id: tool_use_id.as_str().to_owned(),
                                content: render_result(content, *is_error),
                            });
                        }
                        UserContent::Text { text } => texts.push(text.clone()),
                        UserContent::Attachment(a) => {
                            if let Some(t) = render_attachment(a) {
                                texts.push(t);
                            }
                        }
                    }
                }
                let joined = texts.join("\n");
                if !joined.trim().is_empty() {
                    out.push(WireMessage::User { content: joined });
                }
            }

            Message::Assistant { content, .. } => {
                let mut text = String::new();
                let mut calls = Vec::new();

                for c in content {
                    match c {
                        AssistantContent::Text { text: t } => text.push_str(t),
                        AssistantContent::ToolUse { id, name, input } => {
                            calls.push(WireToolCall {
                                id: id.as_str().to_owned(),
                                kind: "function",
                                function: WireFunctionCall {
                                    name: name.clone(),
                                    // 序列化失败时给 `{}` 而不是丢掉这次调用。
                                    // 丢掉的话 tool_calls 和后面的 tool 消息
                                    // 对不上，服务端直接拒整个请求。
                                    arguments: serde_json::to_string(input)
                                        .unwrap_or_else(|_| "{}".to_owned()),
                                },
                            });
                        }
                        // `[约束]` 思考内容不回传。DeepSeek 的文档明确要求，
                        // 带上 reasoning_content 会 400；OpenAI 格式里也没有
                        // 对应字段可放。
                        AssistantContent::Thinking { .. } => {}
                    }
                }

                // 空消息会被服务端拒。这在压缩清理过历史之后是真实会发生的。
                if text.trim().is_empty() && calls.is_empty() {
                    continue;
                }

                out.push(WireMessage::Assistant {
                    content: (!text.trim().is_empty()).then_some(text),
                    tool_calls: calls,
                });
            }

            // 系统提醒不入模型请求。由 INV-7 保证这里拿不到。
            Message::System { .. } => {}
        }
    }

    out
}

fn render_result(content: &ToolResultContent, is_error: bool) -> String {
    let body = match content {
        ToolResultContent::Text { text } => text.clone(),
        ToolResultContent::Spilled {
            path,
            preview,
            total_bytes,
        } => format!(
            "结果过大（{total_bytes} 字节），已写入 {}。开头部分：\n{preview}",
            path.display()
        ),
        ToolResultContent::Cleared => "（历史结果已清理）".to_owned(),
        // OpenAI 的 tool 消息只收文本。图片要走 user 消息的 image_url，
        // 那是另一条路径，先不做。
        ToolResultContent::Image { media_type, .. } => {
            format!("（{media_type} 图片，当前模型不支持在工具结果里返回图片）")
        }
    };

    // 空的 tool 结果会让部分模型误判任务结束。见 ARCHITECTURE.md §6.7
    let body = if body.trim().is_empty() {
        "（命令完成，没有输出）".to_owned()
    } else {
        body
    };

    if is_error {
        format!("错误：{body}")
    } else {
        body
    }
}

fn render_attachment(a: &riot_protocol::message::Attachment) -> Option<String> {
    serde_json::to_string(a).ok()
}
