//! 主循环的状态与依赖。
//!
//! 见 ARCHITECTURE.md §5.5、§5.6

use std::sync::Arc;

pub use riot_protocol::compact::Compactor;
pub use riot_protocol::event::Transition;
use riot_protocol::id::{IdGenerator, SessionId};
use riot_protocol::message::{AssistantContent, Message};
use riot_protocol::provider::{Provider, ThinkingPolicy};
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
    /// 思考策略。存策略而不是解析好的配置：`Adaptive` 要按 `turn` 逐请求
    /// 解析（首请求 vs 工具续轮不同档），存配置的话整个 run 只能一档到底。
    pub thinking: ThinkingPolicy,

    /// 本 run 内距上次 TodoWrite 的工具调用数。到线且清单还有没做完的项
    /// 就往工具结果后面塞一条提醒（见 [`crate::todo_nudge`]），提醒后归零。
    /// 只在 run 内计：新一句用户话开的 run 从零起，上一轮欠的账不带过来。
    pub tool_calls_since_todo: usize,

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
            thinking: ThinkingPolicy::Default,
            tool_calls_since_todo: 0,
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
    ///
    /// 换模型后，不属于当前模型的 thinking signature 一并剥掉
    /// （INV-9：签名与模型绑定，带着旧签名去请求会 400）。历史原文
    /// 不动 —— 用户切回原来的模型时，签名还能用。
    pub fn model_messages(&self) -> Vec<Message> {
        let mut messages: Vec<Message> = self
            .messages
            .iter()
            .filter(|m| m.goes_to_model())
            .cloned()
            .collect();
        strip_foreign_thinking_signatures(&mut messages, &self.model);
        messages
    }
}

/// 剥掉不属于 `current_model` 的 thinking signature。
///
/// 只动副本。同模型是 no-op（前缀缓存按字节匹配，多改一个字节就 miss）。
/// 块类型不改：OpenAI 兼容端不回传思考；Anthropic 的 wire 层会把无签
/// 名 thinking 退化成 text。这里越权改成 Text，等于换到 grok / DeepSeek
/// 时把旧模型的内心戏灌进正文。
pub fn strip_foreign_thinking_signatures(messages: &mut [Message], current_model: &str) {
    for m in messages {
        let Message::Assistant { content, meta, .. } = m else {
            continue;
        };
        let origin = meta.model_origin.as_deref().unwrap_or(current_model);
        if origin == current_model {
            continue;
        }
        for c in content {
            if let AssistantContent::Thinking { signature, .. } = c {
                *signature = None;
            }
        }
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
/// 两个取用点对应两种**注入时机**，是这个契约的全部内容：
///
/// - [`Self::drain`] —— 轮次收尾前（模型不再调工具）。用户插话走这里：
///   排队的消息等当前任务**完全跑完**，中途蹦出来是惊吓（Cursor 语义）。
/// - [`Self::drain_out_of_band`] —— 每批工具结果就位后。带外消息走这里：
///   界面按钮的提醒（「转到后台」）和后台子 agent 的完成通知都是对
///   **正在进行的工作**说话，等整轮跑完等于没有生效。
///
/// 两者都是同步的，队列实现只是弹出已经准备好的消息 —— 消息的组装
///（图片转述等异步工作）在入队一侧完成。
pub trait InputQueue: Send + Sync {
    /// 取走当前排队的全部消息（先进先出）。没有就空。
    fn drain(&self) -> Vec<riot_protocol::message::Message>;

    /// 取走等着**在工具轮边界**注入的带外消息（先进先出）。没有就空。
    ///
    /// 默认没有 —— 只有宿主队列区分这两类，子 agent 和多数测试用不到。
    fn drain_out_of_band(&self) -> Vec<riot_protocol::message::Message> {
        Vec::new()
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use riot_protocol::id::MessageId;
    use riot_protocol::message::MessageMeta;

    fn assistant_thinking(origin: &str, sig: Option<&str>) -> Message {
        Message::Assistant {
            id: MessageId::from_raw("a1"),
            content: vec![
                AssistantContent::Thinking {
                    text: "先想想".into(),
                    signature: sig.map(str::to_owned),
                },
                AssistantContent::Text {
                    text: "好的，马上生成。".into(),
                },
            ],
            usage: None,
            meta: MessageMeta {
                model_origin: Some(origin.into()),
                ..Default::default()
            },
        }
    }

    #[test]
    fn 换模型时剥掉别人的_thinking_签名_历史原文不动() {
        let original = assistant_thinking("anthropic/claude-fable-5.1", Some("sig"));
        let state = AgentState::new(SessionId::from_raw("s"), "grok-4.6")
            .with_messages(vec![original.clone()]);

        let outgoing = state.model_messages();
        match &outgoing[0] {
            Message::Assistant { content, .. } => {
                assert!(
                    matches!(
                        &content[0],
                        AssistantContent::Thinking {
                            signature: None,
                            ..
                        }
                    ),
                    "发给新模型的副本必须去掉旧签名"
                );
                assert!(
                    matches!(&content[1], AssistantContent::Text { text } if text == "好的，马上生成。"),
                    "正文和块类型都不动"
                );
            }
            other => panic!("该是助手消息，得到 {other:?}"),
        }

        match &state.messages[0] {
            Message::Assistant { content, .. } => {
                assert!(
                    matches!(
                        &content[0],
                        AssistantContent::Thinking {
                            signature: Some(s),
                            ..
                        } if s == "sig"
                    ),
                    "历史原文留下签名，切回原模型还能用"
                );
            }
            other => panic!("该是助手消息，得到 {other:?}"),
        }

        crate::invariants::check_thinking_signatures(&outgoing, "grok-4.6");
        assert!(
            crate::invariants::take_violations().is_empty(),
            "剥完之后 INV-9 不该再响"
        );
    }

    #[test]
    fn 同一模型保留_thinking_签名() {
        let origin = "anthropic/claude-fable-5.1";
        let state = AgentState::new(SessionId::from_raw("s"), origin)
            .with_messages(vec![assistant_thinking(origin, Some("sig"))]);
        let outgoing = state.model_messages();
        match &outgoing[0] {
            Message::Assistant { content, .. } => {
                assert!(
                    matches!(
                        &content[0],
                        AssistantContent::Thinking {
                            signature: Some(s),
                            ..
                        } if s == "sig"
                    ),
                    "没换模型就不该动签名，否则 Anthropic 缓存和续思考都会坏"
                );
            }
            other => panic!("该是助手消息，得到 {other:?}"),
        }
    }

    #[test]
    fn 没有_model_origin_的签名按当前模型保留() {
        let mut m = assistant_thinking("unused", Some("sig"));
        if let Message::Assistant { meta, .. } = &mut m {
            meta.model_origin = None;
        }
        let state = AgentState::new(SessionId::from_raw("s"), "grok-4.6").with_messages(vec![m]);
        let outgoing = state.model_messages();
        match &outgoing[0] {
            Message::Assistant { content, .. } => {
                assert!(
                    matches!(
                        &content[0],
                        AssistantContent::Thinking {
                            signature: Some(s),
                            ..
                        } if s == "sig"
                    ),
                    "origin 缺失时 INV-9 把签名当成当前模型的，剥了反而误伤"
                );
            }
            other => panic!("该是助手消息，得到 {other:?}"),
        }
    }
}
