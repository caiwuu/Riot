//! OpenAI 兼容适配。
//!
//! 覆盖 DeepSeek、Kimi、Qwen、OpenRouter、vLLM、Ollama —— 它们用的都是
//! `/v1/chat/completions` 这套报文，差别只在 base URL 和模型名。

pub mod decode;
pub mod provider;
pub mod request;
pub mod wire;

pub use provider::{OpenAiConfig, OpenAiProvider};

#[cfg(test)]
mod tests;
