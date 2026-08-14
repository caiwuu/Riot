//! 主循环的状态与依赖。
//!
//! 见 ARCHITECTURE.md §5.5、§5.6

use std::sync::Arc;

pub use riot_protocol::compact::Compactor;
pub use riot_protocol::event::Transition;
use riot_protocol::id::{IdGenerator, SessionId};
use riot_protocol::message::Message;
use riot_protocol::provider::Provider;
use riot_protocol::tool::Clock;

use crate::invariants::RecoveryCounters;

#[derive(Debug, Clone)]
pub struct AgentState {
    pub session_id: SessionId,
    pub messages: Vec<Message>,
    pub model: String,
    pub system: String,
    pub turn: u32,
    pub max_turns: u32,

    // 恢复计数器 —— 这些字段的重置时机是 bug 高发区，改动前读 ARCHITECTURE.md §5.4
    pub output_limit_recovery_count: u8,
    pub attempted_reactive_compact: bool,
    pub compact_failure_streak: u8,
    pub max_output_tokens_override: Option<u32>,
    /// 本 run 内 stop hook 阻止收尾的累计次数。**从不重置**（run 的生命周期
    /// 内单调递增）：它既是熔断依据，也透传给 hook（CC 的 stop_hook_active
    /// 同义）让脚本自己防循环。
    pub stop_hook_blocks: u32,

    /// 上一轮为何继续。仅用于测试与观测，不参与决策。
    pub transition: Option<Transition>,
}

impl AgentState {
    pub fn new(session_id: SessionId, model: impl Into<String>) -> Self {
        Self {
            session_id,
            messages: Vec::new(),
            model: model.into(),
            system: String::new(),
            turn: 0,
            max_turns: 32,
            output_limit_recovery_count: 0,
            attempted_reactive_compact: false,
            compact_failure_streak: 0,
            max_output_tokens_override: None,
            stop_hook_blocks: 0,
            transition: None,
        }
    }

    pub fn with_messages(mut self, messages: Vec<Message>) -> Self {
        self.messages = messages;
        self
    }

    pub fn with_max_turns(mut self, n: u32) -> Self {
        self.max_turns = n;
        self
    }

    /// 正常推进一轮。
    ///
    /// `[约束]` **只有这里能重置恢复标志位。**恢复重试路径（`continue` 回循环
    /// 开头）绝不能调它 —— 那会让「压缩 → 还是溢出 → 又压缩」变成无限循环，
    /// 每一圈都烧一次 API 调用。这个 bug 在 Claude Code 的注释里有记录。
    ///
    /// 区分两类计数器：
    /// - `attempted_reactive_compact` / `output_limit_recovery_count` 防的是
    ///   **单轮内**的死循环，新一轮的输出需求不同，该重置。
    /// - `compact_failure_streak` 防的是**跨轮**反复压缩失败，只有压缩真正
    ///   成功时才清零。混在一起会让熔断永远触发不了。
    pub fn advance_turn(&mut self) {
        self.turn += 1;
        self.attempted_reactive_compact = false;
        self.output_limit_recovery_count = 0;
        self.max_output_tokens_override = None;
        self.transition = Some(Transition::NextTurn);
    }

    pub fn counters(&self) -> RecoveryCounters {
        RecoveryCounters {
            turn: self.turn,
            output_limit_recovery: self.output_limit_recovery_count,
            attempted_reactive_compact: self.attempted_reactive_compact,
            compact_failure_streak: self.compact_failure_streak,
        }
    }

    /// 只发给模型的消息。System 消息在这里被过滤掉。
    pub fn model_messages(&self) -> Vec<Message> {
        self.messages
            .iter()
            .filter(|m| m.goes_to_model())
            .cloned()
            .collect()
    }
}

/// 主循环的全部外部依赖。
///
/// 这里的每一项都是为了让黄金回放能替换掉非确定性来源。
/// 见 docs/VERIFICATION.md §4.2
#[derive(Clone)]
pub struct AgentDeps {
    pub provider: Arc<dyn Provider>,
    pub compactor: Arc<dyn Compactor>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
    pub tools: Arc<dyn ToolRunner>,
    /// 跑轮中用户插话的队列。主循环只在**一个点** drain：模型正常收尾
    /// （没有 tool_use）、准备报 Completed 之前 —— 排队的消息等当前任务
    /// 完全跑完才进对话（Cursor 语义），要插队由宿主中断本轮来实现。
    /// 这个位置天然满足 API 约束（tool_use / tool_result 配对之间不能
    /// 夹用户消息，否则 400）。
    pub queue: Arc<dyn InputQueue>,
    /// 收尾闸。模型正常说完、准备报 Completed 之前问一声 —— 宿主用它跑
    /// Stop hooks（用户配置的"产出检查"脚本）。
    ///
    /// `[约束]` 只在**正常收尾**时问：错误路径、中断路径都不问。在错误
    /// 消息上跑 stop hook 是 error → hook 注入 → 重试 → error 的死循环
    /// （INV-6 同源的教训，CC 的注释里有记录）。
    pub stop_gate: Arc<dyn StopGate>,
}

/// 收尾闸的裁决。
#[derive(Debug, Clone, PartialEq)]
pub enum StopDecision {
    /// 放行，正常收尾。
    Allow,
    /// 阻止收尾：理由注入对话（模型可见），强制再跑一轮把活干完。
    Block { reason: String },
}

/// 收尾闸。见 [`AgentDeps::stop_gate`]。
#[async_trait::async_trait]
pub trait StopGate: Send + Sync {
    /// `blocks_so_far`：本 run 已被阻止的次数。透传给 hook 让脚本自己
    /// 防循环（内核另有硬熔断兜底，见主循环的 MAX_STOP_HOOK_BLOCKS）。
    async fn check(&self, blocks_so_far: u32) -> StopDecision;
}

/// 默认实现：没有 stop hooks（子 agent、多数测试）。
pub struct NoStopGate;

#[async_trait::async_trait]
impl StopGate for NoStopGate {
    async fn check(&self, _blocks_so_far: u32) -> StopDecision {
        StopDecision::Allow
    }
}

/// 跑轮中插话的队列。宿主实现（真实队列），测试注入脚本。
///
/// `drain` 是同步的：主循环在两个确定的位置调用它，队列实现只是弹出
/// 已经准备好的消息 —— 消息的组装（图片转述等异步工作）在入队一侧完成。
pub trait InputQueue: Send + Sync {
    /// 取走当前排队的全部消息（先进先出）。没有就空。
    fn drain(&self) -> Vec<riot_protocol::message::Message>;
}

/// 默认实现：没有队列（子 agent、多数测试）。
pub struct NoQueue;

impl InputQueue for NoQueue {
    fn drain(&self) -> Vec<riot_protocol::message::Message> {
        Vec::new()
    }
}

// 工具执行契约住在协议层，和 Provider 对称 —— 这样 riot-tools 能直接
// 实现它，不用反向依赖 core。这里重新导出，调用方的 import 路径不变。
pub use riot_protocol::runner::{
    BatchContext, BatchEvent, BatchOutcome, BatchStream, ToolCall, ToolRunner,
};
