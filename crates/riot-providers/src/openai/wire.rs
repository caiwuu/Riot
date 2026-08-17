//! OpenAI `/v1/chat/completions` 的线格式。
//!
//! 只写实际用到的字段。多余的字段照抄一遍没有价值 —— 它们不参与任何判断，
//! 却让"这个字段是干嘛的"变成每次读代码都要问一遍的问题。

use serde::{Deserialize, Serialize};

// ── 请求 ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WireRequest {
    pub model: String,
    pub messages: Vec<WireMessage>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// 采样参数由 provider 配置注入，不属于请求转换逻辑。
    /// None 不发送 —— 各家默认值不同，不替用户决定。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<WireTool>,
    /// 不带这个的话流式响应里没有 usage，上下文管理就没有数据可用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
    /// 思考力度。OpenAI 官方参数；DeepSeek / GLM 的兼容端点也认，
    /// 取值交集是 low/medium/high（它们把 medium 映射到 high）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<&'static str>,
    /// 思考开关。**非** OpenAI 标准字段 —— DeepSeek / GLM 的约定
    /// （`{"type": "enabled"/"disabled"}`），OpenAI 官方端点收到会 400。
    /// 只在用户显式选择"关闭思考"时才发送。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<WireThinkingToggle>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct WireThinkingToggle {
    #[serde(rename = "type")]
    pub kind: &'static str,
}

impl WireThinkingToggle {
    pub fn disabled() -> Self {
        Self { kind: "disabled" }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StreamOptions {
    pub include_usage: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum WireMessage {
    System {
        content: String,
    },
    User {
        content: String,
    },
    /// 带图片的 user 消息:content 是内容块数组而不是字符串。
    ///
    /// `[约束]` 图片只能走这条路。OpenAI 的 `tool` 消息的 content 只接受
    /// 字符串 —— 截图这类工具的结果因此没法直接带图，只能在工具结果之后
    /// 补一条 user 消息把图捎上（见 request.rs）。
    #[serde(rename = "user")]
    UserParts {
        content: Vec<WirePart>,
    },
    Assistant {
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<WireToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

/// user 消息里的一块内容。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WirePart {
    Text { text: String },
    /// `url` 用 data URL 形式:`data:image/jpeg;base64,...`。
    ///
    /// 不传外链是刻意的:截图是本地产物，没有可访问的 URL，而让服务方去拉
    /// 一个我们临时起的 HTTP 服务只会多一条会坏的链路。
    ImageUrl { image_url: WireImageUrl },
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WireImageUrl {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WireToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: WireFunctionCall,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WireFunctionCall {
    pub name: String,
    /// `[约束]` 是 **JSON 字符串**，不是对象。写成对象的话服务端解析失败，
    /// 而错误信息通常只说 "invalid request"，很难定位到这里。
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WireTool {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: WireToolFunction,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WireToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

// ── 响应 ──────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct WireChunk {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub choices: Vec<WireChoice>,
    /// 只在最后一个 chunk 出现，且要请求时开了 `include_usage`。
    #[serde(default)]
    pub usage: Option<WireUsage>,
    /// 有些兼容实现把错误塞在 SSE 数据里而不是 HTTP 状态码上。
    #[serde(default)]
    pub error: Option<WireError>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WireChoice {
    #[serde(default)]
    pub delta: WireDelta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WireDelta {
    #[serde(default)]
    pub content: Option<String>,
    /// DeepSeek-reasoner 的思考过程。
    ///
    /// `[约束]` 这段内容**不能回传**给下一轮请求 —— DeepSeek 的文档明确
    /// 要求，带上它会直接 400。见 `request::convert_messages`。
    #[serde(default)]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<WireToolCallDelta>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WireToolCallDelta {
    /// 分片靠它关联。第一个分片带 id 和 name，后续只有 arguments 片段。
    #[serde(default)]
    pub index: usize,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<WireFunctionDelta>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WireFunctionDelta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WireUsage {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    /// DeepSeek 的上下文缓存命中数。
    #[serde(default)]
    pub prompt_cache_hit_tokens: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WireError {
    #[serde(default)]
    pub message: String,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
}
