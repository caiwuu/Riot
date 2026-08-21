//! 模型适配层。
//!
//! 这一层的四个职责，按「最容易写错」排序：
//!
//! 1. **SSE 解析**（[`sse`]）—— 字节流可以在任意位置切断，网关会制造脏状态
//! 2. **流解码**（[`anthropic::decode`]）—— O(n²) 陷阱和 usage 累计值守卫
//! 3. **重试决策**（[`retry`]）—— 什么该重试、等多久、前台后台区别对待
//! 4. **请求组装**（[`anthropic::request`]）—— 缓存断点放错位置就是白花钱
//!
//! 这四块都是纯逻辑，不碰网络，所以每条规则都能单独摆进测试里。
//! 真正的 HTTP 在 [`transport::HttpTransport`] 后面 —— 换 HTTP 库时，
//! 编译器会指着每一个没填的字段。

pub mod anthropic;
pub mod endpoint;
pub mod http;
pub mod openai;
pub mod retry;
pub mod sse;
pub mod transport;
pub mod watchdog;

pub use anthropic::{AnthropicConfig, AnthropicProvider};
pub use http::ReqwestTransport;
pub use openai::{OpenAiConfig, OpenAiProvider};
pub use retry::{RequestSource, RetryPolicy};
pub use transport::{HttpError, HttpRequest, HttpTransport};

/// 采样参数。`None` = 不发送该字段，用服务端默认值。
///
/// `[约束]` 不要把 None 替换成"合理默认值"发出去 —— 各家模型的默认
/// temperature 不同（推理模型甚至禁止设置），替用户做决定只会更糟。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SamplingParams {
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    /// 只在 Anthropic 协议发送。OpenAI 官方端点收到未知参数会拒绝
    /// 整个请求，而不是忽略它。
    pub top_k: Option<u32>,
}
