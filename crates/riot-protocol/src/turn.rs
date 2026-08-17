//! 一轮任务的跨进程配置契约。
//!
//! 阶段 B 里内核是独立进程,不读 `config.json` / `auth.json`(那是宿主的
//! 职责,见 ARCHITECTURE.md §2.2 决策)。每轮所需的模型端点、采样参数、
//! 明文密钥都由宿主解析好,作为 RPC 参数经这里的类型传给内核。
//!
//! `[约束]` 明文 `api_key` 只在本地进程间(stdio)传输。它不落盘、不进日志、
//! 不进事件 —— 和宿主 `auth.json` 的处理同一条线。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// 说话用的协议。决定请求格式与认证头。
///
/// 和宿主 `config` 里的 `Protocol` 同构 —— 那个是配置侧(会序列化进
/// `config.json`),这个是传输侧(宿主↔内核 RPC)。分开是因为配置类型
/// 属于宿主、不该进 protocol 这个叶子 crate。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApiProtocol {
    /// OpenAI Chat Completions 兼容。
    Openai,
    /// Anthropic Messages。
    Anthropic,
}

/// 采样参数。`None` = 用端点默认。
///
/// 独立于 `riot-providers` 的 `SamplingParams`(那个不含 `max_output_tokens`,
/// 因为输出上限在主循环单独走恢复路径)—— 这里是"宿主配置的完整快照",
/// 由内核在建 Provider 和设置输出上限时各取所需。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct EndpointSampling {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

/// 一个已解析的模型端点:宿主把 provider 配置和明文 key 都填好,内核直接
/// 拿它建 Provider。
///
/// 这是 `config::ResolvedModel` 的"传输版" —— 区别在于 `api_key` 是**明文**
/// (宿主已从环境变量 / auth.json 解析出来),而不是一个待查的变量名。
/// 拆进程后内核拿不到 auth.json,key 必须在宿主这一侧解析完再传进来。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModelEndpoint {
    pub protocol: ApiProtocol,
    pub base_url: String,
    /// 接口路径,空 = 按主机猜(见 `riot_providers::endpoint`)。
    pub api_path: String,
    /// 明文密钥。见模块文档的约束。
    pub api_key: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_model: Option<String>,
    #[serde(default)]
    pub sampling: EndpointSampling,
}

impl ModelEndpoint {
    pub fn is_anthropic(&self) -> bool {
        self.protocol == ApiProtocol::Anthropic
    }
}
