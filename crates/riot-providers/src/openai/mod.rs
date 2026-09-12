//! OpenAI 兼容适配。
//!
//! Chat Completions（DeepSeek、Kimi、Qwen、中转）和 Responses（官方
//! `/v1/responses` 及任意供应商路径）共用这一层。形态由
//! [`riot_protocol::OpenaiApi`] 决定，路径只负责拼 URL。

pub mod decode;
pub mod provider;
pub mod request;
pub mod responses;
pub(crate) mod text;
pub mod wire;

pub use provider::{OpenAiConfig, OpenAiProvider};

#[cfg(test)]
mod tests;
