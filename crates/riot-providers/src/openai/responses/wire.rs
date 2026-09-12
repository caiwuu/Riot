//! OpenAI `/v1/responses` 的线格式。
//!
//! 只写实际用到的字段。Chat Completions 的 `messages` / `max_tokens` /
//! `stream_options` / `reasoning_effort` 不能出现 —— 官方端点对未知
//! 字段会 400。

use serde::{Deserialize, Serialize};

// ── 请求 ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WireRequest {
    pub model: String,
    pub input: Vec<WireInputItem>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<WireTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<WireReasoning>,
    /// 会话历史由 Riot 自己带，不在服务方落盘。
    pub store: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WireReasoning {
    pub effort: &'static str,
    /// `"auto"` = 让服务端回推理摘要。不带的话响应里的 reasoning 项
    /// `summary` 永远是空的，界面上看不到模型在想什么。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WireTool {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum WireInputItem {
    #[serde(rename = "message")]
    Message {
        role: &'static str,
        content: Vec<WirePart>,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    #[serde(rename = "function_call_output")]
    FunctionCallOutput { call_id: String, output: String },
    /// `[约束]` `id` 和 `summary` 在 input 侧是**必填**，`summary` 为空也
    /// 要发 `[]`。省掉任一个服务端回 `400 Missing required parameter:
    /// 'input[N].summary'`，而且只在第二轮才暴露 —— 第一轮没有历史推理项。
    #[serde(rename = "reasoning")]
    Reasoning {
        id: String,
        encrypted_content: String,
        summary: Vec<WireSummary>,
    },
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum WirePart {
    #[serde(rename = "input_text")]
    InputText { text: String },
    #[serde(rename = "output_text")]
    OutputText { text: String },
    #[serde(rename = "input_image")]
    InputImage { image_url: String },
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WireSummary {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: String,
}

impl WireSummary {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            kind: "summary_text",
            text: text.into(),
        }
    }
}

/// 存进 `AssistantContent::Thinking.signature` 的推理项。
///
/// `store: false` 时下一轮必须把 `encrypted_content` 原样放回 input，
/// 否则推理模型会拒或丢掉连贯性。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReasoningSig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub encrypted_content: String,
}

// ── 响应 ──────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct WireEvent {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub delta: Option<String>,
    #[serde(default)]
    pub item_id: Option<String>,
    #[serde(default)]
    pub output_index: Option<usize>,
    #[serde(default)]
    pub item: Option<WireOutputItem>,
    #[serde(default)]
    pub response: Option<WireResponse>,
    /// 嵌套形态：`{"type":"error","error":{"message":..}}`。服务端实际
    /// 常发这种（Azure 也是），虽然文档写的是下面那种。
    #[serde(default)]
    pub error: Option<WireStreamError>,
    /// 扁平形态：`{"type":"error","code":..,"message":..}`，文档定义的样子。
    /// 两种都得认 —— 漏一种的后果是出错后半截正文被当成正常回答定稿。
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WireStreamError {
    #[serde(default)]
    pub message: String,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct WireResponse {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub error: Option<WireStreamError>,
    #[serde(default)]
    pub incomplete_details: Option<WireIncomplete>,
    #[serde(default)]
    pub output: Vec<WireOutputItem>,
    #[serde(default)]
    pub usage: Option<WireUsage>,
}

#[derive(Debug, Deserialize)]
pub struct WireIncomplete {
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WireOutputItem {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub call_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
    #[serde(default)]
    pub encrypted_content: Option<String>,
    #[serde(default)]
    pub content: Vec<WireOutputPart>,
    #[serde(default)]
    pub summary: Vec<WireSummaryPart>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WireOutputPart {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WireSummaryPart {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct WireUsage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub input_tokens_details: Option<WireInputDetails>,
}

#[derive(Debug, Default, Deserialize)]
pub struct WireInputDetails {
    #[serde(default)]
    pub cached_tokens: u32,
}
