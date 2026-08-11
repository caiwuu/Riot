//! Anthropic Messages API 适配。
//!
//! `[约束]` 内部规范格式贴 Anthropic 的 `content_block` 结构。
//! OpenAI 兼容协议通过适配器转换过来，而不是反过来 —— thinking、
//! prompt caching、并行工具调用这些设计都基于这套结构，反向转换会丢信息。

pub mod decode;
pub mod provider;
pub mod request;
pub mod wire;

pub use decode::StreamDecoder;
pub use provider::{AnthropicConfig, AnthropicProvider};
pub use request::{RetryContext, SystemSection, build_request};
