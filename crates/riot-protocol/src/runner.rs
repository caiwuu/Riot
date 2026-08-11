//! 工具批次的执行契约。
//!
//! 这是主循环的第二个接缝，和 [`crate::provider::Provider`] 对称：
//! 一个把消息变成模型响应，一个把工具调用变成工具结果。两者都在协议层，
//! 这样 `riot-tools` 能直接实现它，不需要反向依赖 `riot-core`。
//!
//! 见 ARCHITECTURE.md §7

use crate::event::ProgressPayload;
use crate::id::{SessionId, ToolUseId};
use crate::message::Message;
use crate::provider::ToolSpec;
use tokio_util::sync::CancellationToken;

/// 工具批次的执行入口。
///
/// 抽象成 trait 而不是具体类型，是为了让主循环的测试能在不搭整个工具注册表的
/// 情况下跑起来。真实实现在 `riot-tools`，它负责权限、并发分批、结果保序。
pub trait ToolRunner: Send + Sync {
    /// 进 API 请求的工具声明。
    fn specs(&self) -> Vec<ToolSpec>;

    /// 执行一批工具调用。
    ///
    /// 返回流而不是 Future，是因为工具执行期间要往外吐进度（Bash 的实时输出、
    /// 权限询问）。做成 Future 的话这些只能另开通道，主循环就得同时 select
    /// 两个源——顺序保证会立刻变得很难说清。
    ///
    /// `[约束]` 流必须以恰好一个 [`BatchEvent::Done`] 结束。
    fn run_batch(&self, calls: Vec<ToolCall>, ctx: BatchContext) -> BatchStream;
}

pub type BatchStream = std::pin::Pin<Box<dyn futures_core::Stream<Item = BatchEvent> + Send>>;

#[derive(Debug, Clone, PartialEq)]
pub enum BatchEvent {
    Progress {
        tool_use_id: ToolUseId,
        payload: ProgressPayload,
    },
    Done(BatchOutcome),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: ToolUseId,
    pub name: String,
    pub input: serde_json::Value,
}

#[derive(Clone)]
pub struct BatchContext {
    pub session_id: SessionId,
    pub cancel: CancellationToken,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BatchOutcome {
    /// 保序的 tool_result 消息。
    ///
    /// `[约束]` 里面的 tool_result 顺序必须与 `calls` 一致，且每个 tool_use_id
    /// 都要有对应结果 —— 包括被取消的。缺一个就会让下一次 API 请求 400。
    /// 由 INV-1 断言。
    pub results: Message,
    /// 工具产生的旁路消息（图片 metadata 等），不塞进 tool_result。
    pub side_messages: Vec<Message>,
    /// 本批有多少个工具因中断而未跑完。
    pub cancelled: usize,
}
