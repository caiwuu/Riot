//! 上下文压缩的契约。
//!
//! 见 ARCHITECTURE.md §10

use async_trait::async_trait;

use crate::event::CompactStrategy;
use crate::message::Message;

#[async_trait]
pub trait Compactor: Send + Sync {
    /// 把消息序列压到预算之内。
    ///
    /// `[约束]` 实现必须按「轻 → 重」顺序尝试策略：先落盘大结果（无损），
    /// 再清理旧 tool_result，最后才动用 LLM 总结。顺序反了会在本可无损处理的
    /// 场景下丢信息，而且贵得多。
    ///
    /// `[约束]` 压缩后**必须保持 tool_use / tool_result 配对**。清理 tool_result
    /// 时要留 `ToolResultContent::Cleared` 占位符，不能整条删掉。
    async fn compact(&self, messages: Vec<Message>, budget: CompactBudget) -> CompactResult;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactBudget {
    /// 目标 token 数。
    pub target_tokens: u32,
    /// 当前 token 数。
    pub current_tokens: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CompactResult {
    Compacted {
        messages: Vec<Message>,
        before_tokens: u32,
        after_tokens: u32,
        strategy: CompactStrategy,
    },
    /// 压不动了。主循环据此累加熔断计数。
    Failed { reason: String },
}
