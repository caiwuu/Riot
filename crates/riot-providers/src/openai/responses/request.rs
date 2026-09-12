//! 内部消息格式 → OpenAI Responses 请求。
//!
//! 内部仍是 Anthropic 风格的 content block。Responses 用 `input` 项：
//! `message` / `function_call` / `function_call_output` / `reasoning`。
//!
//! 几条不能违反的规矩，违反了服务端只会回一句 `invalid request`：
//!
//! 1. `function_call_output` 必须紧跟对应的 `function_call`，`call_id` 配对；
//! 2. `arguments` 是 JSON **字符串**，不是对象；
//! 3. 不要带 Chat Completions 字段（`messages` / `max_tokens` / `stream_options`）；
//! 4. `store: false` 时推理项必须把 `encrypted_content` 放回 input；
//! 5. 回传的推理项 `id` / `summary` 必填（空也发 `[]`），且后面必须紧跟它
//!    配套的 message 或 function_call —— 单独一个推理项服务端拒收
//!    （`provided without its required following item`）。

use riot_protocol::message::{
    AssistantContent, Attachment, Message, ToolResultContent, UserContent,
};
use riot_protocol::provider::{ProviderRequest, ThinkingConfig, ThinkingEffort};

use super::wire::{
    ReasoningSig, WireInputItem, WirePart, WireReasoning, WireRequest, WireSummary, WireTool,
};
use crate::anthropic::request::SystemSection;
use crate::openai::request::RetryContext;
use crate::openai::text::{assemble_system, data_url, render_attachment, render_result};

pub fn build_request(
    req: &ProviderRequest,
    system: &[SystemSection],
    ctx: &RetryContext,
) -> WireRequest {
    let instructions = assemble_system(system, &req.system);
    let input = convert_input(&req.messages, ctx.strip_thinking_signatures);

    let mut tools: Vec<WireTool> = req
        .tools
        .iter()
        .map(|t| WireTool {
            kind: "function",
            name: t.name.clone(),
            description: t.description.clone(),
            parameters: t.input_schema.clone(),
        })
        .collect();
    tools.sort_by(|a, b| a.name.cmp(&b.name));

    // 开了档位就顺手要摘要（`summary: "auto"`）和加密推理：前者让界面能
    // 展示思考过程，后者是下一轮回传的凭据。
    let (reasoning, include) = match req.thinking {
        ThinkingConfig::Off => (None, Vec::new()),
        // `"none"` 只有 gpt-5.1 起认，gpt-5 / o 系列会 400。`Disabled` 只能
        // 来自用户显式选择（见 `ThinkingConfig` 的约束），这里不替它兜。
        ThinkingConfig::Disabled => (
            Some(WireReasoning {
                effort: "none",
                summary: None,
            }),
            Vec::new(),
        ),
        ThinkingConfig::Effort { level } => (
            Some(WireReasoning {
                effort: level.as_openai_str(),
                summary: Some("auto"),
            }),
            vec!["reasoning.encrypted_content"],
        ),
        ThinkingConfig::Budget { tokens } => (
            Some(WireReasoning {
                effort: match tokens {
                    0..=4_096 => ThinkingEffort::Low.as_openai_str(),
                    4_097..=16_384 => ThinkingEffort::Medium.as_openai_str(),
                    _ => ThinkingEffort::High.as_openai_str(),
                },
                summary: Some("auto"),
            }),
            vec!["reasoning.encrypted_content"],
        ),
    };

    WireRequest {
        model: ctx
            .model_override
            .clone()
            .unwrap_or_else(|| req.model.clone()),
        input,
        stream: true,
        instructions: (!instructions.is_empty()).then_some(instructions),
        max_output_tokens: ctx.max_tokens_override.or(req.max_output_tokens),
        temperature: None,
        top_p: None,
        tools,
        reasoning,
        store: false,
        include,
    }
}

pub fn wire_bytes(messages: &[Message]) -> usize {
    convert_input(messages, false)
        .iter()
        .map(|m| serde_json::to_string(m).map(|s| s.len()).unwrap_or(0))
        .sum()
}

pub fn convert_input(messages: &[Message], strip_thinking_signatures: bool) -> Vec<WireInputItem> {
    let mut out = Vec::new();

    for m in messages {
        match m {
            Message::User { content, .. } => {
                let mut texts: Vec<String> = Vec::new();
                let mut user_images: Vec<WirePart> = Vec::new();
                let mut tool_images: Vec<WirePart> = Vec::new();
                for c in content {
                    match c {
                        UserContent::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
                            out.push(WireInputItem::FunctionCallOutput {
                                call_id: tool_use_id.as_str().to_owned(),
                                output: render_result(content, *is_error),
                            });
                            if let ToolResultContent::Image {
                                media_type, data, ..
                            }
                            | ToolResultContent::MarkedImage {
                                media_type, data, ..
                            } = content
                            {
                                tool_images.push(WirePart::InputText {
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
                        out.push(WireInputItem::Message {
                            role: "user",
                            content: vec![WirePart::InputText { text: joined }],
                        });
                    }
                } else {
                    let mut parts = user_images;
                    if !joined.trim().is_empty() {
                        parts.push(WirePart::InputText { text: joined });
                    }
                    out.push(WireInputItem::Message {
                        role: "user",
                        content: parts,
                    });
                }
                if !tool_images.is_empty() {
                    out.push(WireInputItem::Message {
                        role: "user",
                        content: tool_images,
                    });
                }
            }

            Message::Assistant { content, .. } => {
                let mut text = String::new();
                let mut calls = Vec::new();
                let mut reasonings = Vec::new();

                for c in content {
                    match c {
                        AssistantContent::Text { text: t } => text.push_str(t),
                        AssistantContent::ToolUse { id, name, input } => {
                            calls.push(WireInputItem::FunctionCall {
                                call_id: id.as_str().to_owned(),
                                name: name.clone(),
                                arguments: serde_json::to_string(input)
                                    .unwrap_or_else(|_| "{}".to_owned()),
                            });
                        }
                        AssistantContent::Thinking {
                            text: thinking,
                            signature,
                        } => {
                            if strip_thinking_signatures {
                                continue;
                            }
                            if let Some(item) = reasoning_item(signature.as_deref(), thinking) {
                                reasonings.push(item);
                            }
                        }
                    }
                }

                if text.trim().is_empty() && calls.is_empty() {
                    // 只剩推理项（比如输出撞上限时被截断的那一轮）。单独回传
                    // 服务端拒收（规矩 5），整条丢掉。
                    continue;
                }

                out.extend(reasonings);
                if !text.trim().is_empty() {
                    out.push(WireInputItem::Message {
                        role: "assistant",
                        content: vec![WirePart::OutputText { text }],
                    });
                }
                out.extend(calls);
            }

            Message::System { .. } => {}
        }
    }

    out
}

fn image_part(media_type: &str, data: &str) -> WirePart {
    WirePart::InputImage {
        image_url: data_url(media_type, data),
    }
}

fn reasoning_item(signature: Option<&str>, text: &str) -> Option<WireInputItem> {
    let raw = signature?.trim();
    if raw.is_empty() {
        return None;
    }
    let sig: ReasoningSig = serde_json::from_str(raw).ok()?;
    if sig.encrypted_content.is_empty() {
        return None;
    }
    // 没有 id 的推理项发出去也是 400（规矩 5），不如不发 —— 少一段推理
    // 上下文，比整轮被拒好。
    let id = sig.id.filter(|s| !s.is_empty())?;
    Some(WireInputItem::Reasoning {
        id,
        encrypted_content: sig.encrypted_content,
        summary: if text.trim().is_empty() {
            Vec::new()
        } else {
            vec![WireSummary::text(text)]
        },
    })
}
