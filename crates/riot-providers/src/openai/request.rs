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

use riot_protocol::message::{
    AssistantContent, Attachment, Message, ToolResultContent, UserContent,
};
use riot_protocol::provider::{ProviderRequest, ThinkingConfig, ThinkingEffort};

use super::wire::{
    StreamOptions, WireFunctionCall, WireImageUrl, WireMessage, WirePart, WireRequest,
    WireThinkingToggle, WireTool, WireToolCall, WireToolFunction,
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

    // 分段边界只对 Anthropic 的分块缓存有意义，这边一条 system 消息装完，
    // 把两段原序接回去 —— 顺序不能动，服务端的自动前缀缓存靠稳定前缀命中。
    // 不切的话那个标记会原样念给模型听。
    let (stable, project) = crate::anthropic::request::split_request_system(&req.system);
    let req_system = if project.is_empty() {
        stable.to_owned()
    } else {
        format!("{stable}\n\n{project}")
    };

    let full_system = match (system_text.is_empty(), req_system.is_empty()) {
        (true, true) => String::new(),
        (true, false) => req_system,
        (false, true) => system_text,
        (false, false) => format!("{system_text}\n\n{req_system}"),
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

    // Effort 只发标准的 reasoning_effort：思考默认就是开的（DeepSeek / GLM），
    // 再捎上非标准的 thinking 对象只会把 OpenAI 官方端点也搭进去 400。
    let (reasoning_effort, thinking) = match req.thinking {
        ThinkingConfig::Off => (None, None),
        ThinkingConfig::Disabled => (None, Some(WireThinkingToggle::disabled())),
        ThinkingConfig::Effort { level } => (Some(level.as_openai_str()), None),
        // OpenAI 兼容协议没有预算参数，折算成最近的档位。
        ThinkingConfig::Budget { tokens } => (
            Some(match tokens {
                0..=4_096 => ThinkingEffort::Low.as_openai_str(),
                4_097..=16_384 => ThinkingEffort::Medium.as_openai_str(),
                _ => ThinkingEffort::High.as_openai_str(),
            }),
            None,
        ),
    };

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
        reasoning_effort,
        thinking,
    }
}

/// 这些消息发出去有多少字节。
///
/// `[约束]` 走 [`convert_messages`] 而不是直接量 `Message` —— 那是
/// [`riot_protocol::provider::Provider::count_tokens`] 那条约束的落地方式。
/// 复用转换（而不是另写一遍"哪些字段会发"）是刻意的:两套规则各自演化，
/// 迟早有一天线协议改了而估算没跟上，而那种偏差没有任何报错，只会表现成
/// 压缩时机变得莫名其妙。
pub fn wire_bytes(messages: &[Message]) -> usize {
    convert_messages(messages)
        .iter()
        .map(|m| serde_json::to_string(m).map(|s| s.len()).unwrap_or(0))
        .sum()
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
                // 用户附的图。和文字同一条消息、排在文字前面（见下）。
                let mut user_images: Vec<WirePart> = Vec::new();
                // 工具结果里的图片。`tool` 消息装不了，攒起来跟在后面单发。
                let mut tool_images: Vec<WirePart> = Vec::new();
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
                            // `path` 是给界面的（落盘原图）；发模型的只有
                            // `data` 里的压缩图。
                            // Image 和 MarkedImage 都带一张给模型的图，走同一条
                            // "图放下一条 user 消息"的路（OpenAI 的 tool 消息本身
                            // 塞不了图）。
                            if let ToolResultContent::Image {
                                media_type, data, ..
                            }
                            | ToolResultContent::MarkedImage {
                                media_type, data, ..
                            } = content
                            {
                                tool_images.push(WirePart::Text {
                                    text: format!("上一个工具结果（{tool_use_id}）的图片："),
                                });
                                tool_images.push(image_part(media_type, data));
                            }
                        }
                        UserContent::Text { text } => texts.push(text.clone()),
                        UserContent::Attachment(a) => {
                            if let Attachment::Image { media_type, data } = a {
                                user_images.push(image_part(media_type, data));
                            } else if let Some(t) = render_attachment(a) {
                                texts.push(t);
                            }
                        }
                    }
                }
                let joined = texts.join("\n");
                if user_images.is_empty() {
                    if !joined.trim().is_empty() {
                        out.push(WireMessage::User { content: joined });
                    }
                } else {
                    // `[约束]` 用户附的图排在文字**前面**，和 user_content
                    // 摆进历史的顺序一致（两家的文档都建议这个顺序）。以前
                    // 图片被挪到文字后面单发一条，等于把上游特意排好的顺序
                    // 又翻了回去。
                    let mut parts = user_images;
                    if !joined.trim().is_empty() {
                        parts.push(WirePart::Text { text: joined });
                    }
                    out.push(WireMessage::UserParts { content: parts });
                }
                // `[约束]` 工具结果的图必须排在 tool 消息**之后**，不能塞进
                // tool 消息里。OpenAI 要求 tool 消息紧跟着对应的 assistant，
                // 而它的 content 只收字符串 —— 唯一能带图的位置就是后面
                // 这条 user 消息。
                if !tool_images.is_empty() {
                    out.push(WireMessage::UserParts {
                        content: tool_images,
                    });
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

/// 一张图的内容块。
fn image_part(media_type: &str, data: &str) -> WirePart {
    WirePart::ImageUrl {
        image_url: WireImageUrl {
            url: format!("data:{media_type};base64,{data}"),
        },
    }
}

fn render_result(content: &ToolResultContent, is_error: bool) -> String {
    let body = match content {
        ToolResultContent::Text { text } => text.clone(),
        ToolResultContent::Spilled {
            path,
            preview,
            total_bytes,
        } => format!(
            "Result too large ({total_bytes} bytes); written to {}. First part:\n{preview}",
            path.display()
        ),
        ToolResultContent::Cleared => "[result cleared to save context]".to_owned(),
        // 图片本身跟在这条 tool 消息后面的那条 user 消息里（见
        // convert_messages）。这里留一句话是因为 tool 消息不能为空 ——
        // 空结果会让一部分模型误判任务结束。
        ToolResultContent::Image { media_type, .. } => {
            format!("(the {media_type} image is in the next message)")
        }
        // 转述代替图片。图片是给界面的，不随请求发 —— 这个变体本来就
        // 产生于"模型看不了图"的会话（见协议注释）。
        ToolResultContent::DescribedImage { text, .. } => text.clone(),
        // 图跟在下一条 user 消息里（见 convert_messages 的 tool_images）；
        // 这里给配套文字 + 一句指路，tool 消息不能为空。措辞保持中性 ——
        // 这个变体不只装 Set-of-Marks 截图，也装 MCP 的图文混合结果。
        ToolResultContent::MarkedImage {
            media_type, text, ..
        } => {
            format!(
                "{text}\n(the image accompanying this result is in the next message, {media_type})"
            )
        }
    };

    // 空的 tool 结果会让部分模型误判任务结束。见 ARCHITECTURE.md §6.7
    let body = if body.trim().is_empty() {
        "(completed with no output)".to_owned()
    } else {
        body
    };

    if is_error {
        format!("Error: {body}")
    } else {
        body
    }
}

/// 附件转成给模型的文字。和 Anthropic 那条路（`convert_attachment`）保持
/// 同一套措辞 —— 模型换协议不该看到两种不同的注入格式。
///
/// 以前这里直接 `serde_json::to_string`，模型读到的是一坨
/// `{"type":"attachment","kind":...}` 的字面 JSON。
fn render_attachment(a: &riot_protocol::message::Attachment) -> Option<String> {
    use riot_protocol::message::Attachment;
    Some(match a {
        Attachment::Memory { path, content } => format!(
            "<system-reminder>\nProject memory {}:\n{content}\n</system-reminder>",
            path.display()
        ),
        Attachment::RestoredFile { path, content } => format!(
            "<system-reminder>\nYou read {} before compaction:\n{content}\n</system-reminder>",
            path.display()
        ),
        Attachment::UserFile { path, content } => format!(
            "<system-reminder>\nThe user referenced {} in their message; its contents \
             follow:\n{content}\n</system-reminder>",
            path.display()
        ),
        Attachment::Environment { text } | Attachment::SystemReminder { text } => {
            format!("<system-reminder>\n{text}\n</system-reminder>")
        }
        // 视觉兼容：模型读转述，图片本体（`data`）只给界面，不发出去。
        Attachment::DescribedImage { text, .. } => {
            format!("<system-reminder>\n{text}\n</system-reminder>")
        }
        // 图片由调用方单独走内容块，不在这里变成文字。
        Attachment::Image { .. } => return None,
    })
}
