//! OpenAI Responses 适配：`input` / `instructions` + 命名 SSE 事件。
//!
//! 形态由 [`riot_protocol::OpenaiApi::Responses`] 显式选择。路径可以是
//! 官方的 `/v1/responses`，也可以是供应商自己的任何尾巴。

pub mod decode;
pub mod request;
pub mod wire;

pub use decode::{StreamDecoder, decode_stream};
pub use request::{build_request, convert_input, wire_bytes};

#[cfg(test)]
mod tests;
