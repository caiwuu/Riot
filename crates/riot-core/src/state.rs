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
}

// 工具执行契约住在协议层，和 Provider 对称 —— 这样 riot-tools 能直接
// 实现它，不用反向依赖 core。这里重新导出，调用方的 import 路径不变。
pub use riot_protocol::runner::{
    BatchContext, BatchEvent, BatchOutcome, BatchStream, ToolCall, ToolRunner,
};
