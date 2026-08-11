//! 模型适配层的契约。
//!
//! 这里只定义接口，实现在 `riot-providers`。放在 protocol 是因为
//! 黄金回放要用一个从磁盘读 SSE 的假 Provider 替换真实现，
//! 而 core 只能依赖 protocol。
//!
//! 见 ARCHITECTURE.md §11

use std::pin::Pin;

use async_trait::async_trait;
use futures_core::Stream;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::event::StreamDelta;
use crate::message::{Message, Usage};

pub type ProviderStream = Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>;

#[async_trait]
pub trait Provider: Send + Sync {
    /// 发起一次请求并返回事件流。
    ///
    /// 重试与降级在**实现内部**完成，对主循环不可见 —— 主循环只关心
    /// "这次调用最终成功了还是失败了"。把重试暴露出去会让主循环同时管
    /// 两套恢复逻辑，那是 bug 温床。
    fn stream(&self, req: ProviderRequest, cancel: CancellationToken) -> ProviderStream;

    /// 估算消息序列的 token 数。上下文管理层用它决定何时压缩。
    fn count_tokens(&self, messages: &[Message]) -> u32;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderRequest {
    pub model: String,
    /// 只包含 `goes_to_model() == true` 的消息。由 INV-7 断言。
    pub messages: Vec<Message>,
    pub system: String,
    pub tools: Vec<ToolSpec>,
    /// None = 用模型默认值。输出上限恢复时会被调低。
    pub max_output_tokens: Option<u32>,
    pub thinking: ThinkingConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ThinkingConfig {
    #[default]
    Off,
    /// 固定预算。
    Budget { tokens: u32 },
}

/// Provider 流里的一个事件。
///
/// 可序列化是为了黄金回放：用例把模型响应存成 JSON，测试时原样喂回主循环。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ProviderEvent {
    Delta(StreamDelta),
    /// 一条完整的助手消息。
    Message(Message),
    /// 用量更新。累计值，用 [`Usage::merge`] 合并。
    Usage(Usage),
    /// 出错。**流在此结束**，不会再有后续事件。
    Error(ProviderError),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderError {
    /// 上下文溢出。可恢复：压缩后重试。
    #[error("上下文溢出：用了 {used}，上限 {limit}")]
    ContextOverflow { used: u32, limit: u32 },

    /// 输出 token 耗尽。可恢复：调低 max_output_tokens 后重试。
    #[error("输出 token 耗尽")]
    OutputLimit,

    /// 附件过大。可恢复：剥离媒体后重试。
    #[error("媒体过大：{bytes} 字节")]
    MediaTooLarge { bytes: u64 },

    /// 重试耗尽。不可恢复 —— provider 内部已经试过了。
    #[error("重试耗尽：{message}")]
    RetriesExhausted { message: String },

    #[error("认证失败：{message}")]
    Auth { message: String },

    #[error("传输错误：{message}")]
    Transport { message: String },

    /// 模型拒绝服务（内容策略等）。
    #[error("请求被拒绝：{message}")]
    Refused { message: String },
}

impl ProviderError {
    /// 是否值得主循环尝试恢复。
    ///
    /// `[约束]` 这个判断决定了错误走扣留路径还是直接终止。判错的后果：
    /// 把不可恢复的判成可恢复会导致无谓重试（认证失败重试一百次也不会成功），
    /// 反过来会让本可自愈的上下文溢出直接终止会话。
    ///
    /// 注意 `RetriesExhausted` **不可恢复** —— provider 内部已经退避重试过了，
    /// 主循环再来一遍只是把同样的失败再走一遍。
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            ProviderError::ContextOverflow { .. }
                | ProviderError::OutputLimit
                | ProviderError::MediaTooLarge { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 只有三类错误值得主循环恢复() {
        assert!(ProviderError::ContextOverflow { used: 1, limit: 1 }.is_recoverable());
        assert!(ProviderError::OutputLimit.is_recoverable());
        assert!(ProviderError::MediaTooLarge { bytes: 1 }.is_recoverable());

        assert!(
            !ProviderError::RetriesExhausted {
                message: "502".into()
            }
            .is_recoverable(),
            "provider 内部已经退避重试过，主循环再试一遍只是重复同样的失败"
        );
        assert!(
            !ProviderError::Auth {
                message: "401".into()
            }
            .is_recoverable(),
            "认证失败重试一百次也不会成功"
        );
    }
}
