//! MCP 的线上格式：JSON-RPC 2.0 帧 + 协议消息类型。
//!
//! 只定义我们真正收发的那部分。MCP 规范还有 resources / prompts /
//! sampling 等能力，这里刻意不建类型 —— 空类型不是"为将来做准备"，
//! 是让读代码的人以为它们被支持了。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 我们声明的协议版本。服务器返回自己的版本，双方按较旧的语义走 ——
/// 实践里各版本的 tools 语义兼容，这里不做版本协商拒绝。
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// 出站帧：请求（带 id）或通知（不带）。
#[derive(Serialize)]
pub struct Outgoing<'a> {
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    pub method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// 回给服务器的响应（服务器也会向客户端发请求：ping、roots/list…）。
#[derive(Serialize)]
pub struct OutgoingResponse {
    pub jsonrpc: &'static str,
    /// 原样带回 —— 服务器的 id 可能是数字也可能是字符串。
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<OutgoingError>,
}

#[derive(Serialize)]
pub struct OutgoingError {
    pub code: i64,
    pub message: String,
}

/// JSON-RPC 的"method not found"。
pub const METHOD_NOT_FOUND: i64 = -32601;

/// 进站帧的骨架。响应、服务器请求、通知先统一解出来再分流 ——
/// 三者各建类型再 untagged 猜的话，一个缺字段的帧会被猜进错误的分支。
#[derive(Debug, Deserialize)]
pub struct Incoming {
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

// ── MCP 消息体 ─────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    #[serde(default)]
    pub protocol_version: String,
    #[serde(default)]
    pub server_info: Implementation,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Implementation {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListToolsResult {
    #[serde(default)]
    pub tools: Vec<ToolDef>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// 服务器声明的一个工具。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDef {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// 原样透传给模型。不用 schemars 重建 —— 服务器给什么就发什么，
    /// 重建丢掉的每个关键字（oneOf、format）都是模型少的一分约束。
    #[serde(default = "empty_object_schema")]
    pub input_schema: Value,
    #[serde(default)]
    pub annotations: Option<ToolAnnotations>,
}

fn empty_object_schema() -> Value {
    serde_json::json!({ "type": "object" })
}

/// 行为提示。`[约束]` 这些是**提示**不是保证（规范原话），只能用来
/// 放宽展示，不能用来放宽权限判定 —— 判定仍走 fail-closed：
/// 没说自己只读的一律当会写。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    #[serde(default)]
    pub read_only_hint: Option<bool>,
    #[serde(default)]
    pub destructive_hint: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult {
    /// 内容块。用 `Value` 而不是 enum：MCP 的块类型还在增加
    /// （text/image/audio/resource/resource_link…），enum 会让一个不认识
    /// 的块类型弄失败整个反序列化。渲染时逐块按 `type` 认，不认识的说明。
    #[serde(default)]
    pub content: Vec<Value>,
    #[serde(default)]
    pub is_error: Option<bool>,
}
