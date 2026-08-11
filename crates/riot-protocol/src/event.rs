//! 内核对外的唯一输出通道。
//!
//! UI、持久化、黄金回放测试都消费这一个流。不要为了方便再加平行的
//! 事件类型 —— 多一个通道就多一份保持同步的负担。

use crate::id::{MessageId, RequestId, ToolUseId};
use crate::message::Message;
use crate::permission::{DecisionReason, PermissionAsk};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// 一轮 API 请求开始。UI 用于显示 spinner。
    ///
    /// `after` 说明这一轮是**为什么**开始的。它同时服务两个目的：UI 可以显示
    /// "正在重试（上下文已压缩）" 而不是干转圈；黄金回放靠它区分"模型正常
    /// 要求继续"和"因错误在重试"—— 没有这个字段，两者产生的事件序列
    /// 完全一样，恢复逻辑写错了测试也发现不了。
    RequestStart {
        turn: u32,
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<Transition>,
    },

    /// 流式增量。高频（每秒可能上百条），仅用于打字机效果。
    ///
    /// **不进 transcript，黄金回放测试也会忽略它** —— 断言 Delta
    /// 会让用例极其脆弱，改一点流式切分逻辑就全红。
    Delta(StreamDelta),

    /// 一条完整消息。可持久化、可回放、可送回模型。
    Message(Message),

    /// 工具执行进度。不进 transcript。
    Progress {
        tool_use_id: ToolUseId,
        payload: ProgressPayload,
    },

    /// 权限请求。内核在此暂停，等宿主回应。
    ///
    /// 等待必须带超时和取消，超时默认 **deny**。见 ARCHITECTURE.md §12.4
    PermissionRequest {
        request_id: RequestId,
        detail: Box<PermissionAsk>,
    },

    /// 某个权限请求已经**不需要回答了** —— 超时或被取消。
    ///
    /// 用户回答产生的关闭不走这条:那种情况下界面自己就把弹窗收了。
    /// 这条专门覆盖"没人回答"的那一半 —— 请求在宿主侧已经作废，界面
    /// 却不知道，弹窗会一直挂着。用户过一会儿点了"允许",什么都不会
    /// 发生（宿主早已按拒绝处理并继续往下走了），而界面表现得像成功了。
    ///
    /// 等待上限缩到 60 秒之后这条路径变得常见,不能再靠 `Done` 兜底。
    PermissionResolved {
        request_id: RequestId,
        reason: DecisionReason,
    },

    /// 上下文压缩发生。
    Compacted {
        before_tokens: u32,
        after_tokens: u32,
        strategy: CompactStrategy,
    },

    /// 终止。
    ///
    /// **必须是流的最后一个事件，且必须出现。** 即使内核 panic 被捕获，
    /// 也要合成一条 `Done { reason: Error }`。消费者依赖这一点做资源清理；
    /// 缺失会导致 UI 永远转圈。由 INV-4 断言保证。
    Done { reason: TerminalReason },
}

impl AgentEvent {
    pub fn is_done(&self) -> bool {
        matches!(self, AgentEvent::Done { .. })
    }

    /// 该事件是否进 transcript / 参与黄金回放断言。
    pub fn is_durable(&self) -> bool {
        !matches!(self, AgentEvent::Delta(_) | AgentEvent::Progress { .. })
    }
}

/// 一轮结束后为什么继续。
///
/// `[约束]` 每次主循环 `continue` 之前必须设置它，并带进下一个
/// [`AgentEvent::RequestStart`]。这是把「恢复路径」变成可观测行为的唯一手段。
///
/// 放在 protocol 而不是 core，是因为前端也要用 —— 用户需要知道
/// 「转了 30 秒是因为在压缩上下文」，而不是以为卡住了。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Transition {
    /// 模型给了 tool_use，执行完继续。
    NextTurn,
    /// 上下文溢出，压缩后重试。
    ReactiveCompactRetry,
    /// 输出 token 耗尽，调低上限后重试。
    OutputLimitRecovery,
    /// stop hook 拦下了结束，注入内容后继续。
    StopHookBlocking,
    /// 接近预算，注入提醒后继续。
    TokenBudgetNudge,
}

/// 终止原因。
///
/// 在 TS 版本里这是 AsyncGenerator 的 return 值，控制流与数据流分离。
/// Rust 的 `async_stream::stream!` 要求块返回 `()`，所以降级成事件变体。
/// 好处是终止原因现在可序列化、可持久化、可被回放测试断言。
/// 详见 ARCHITECTURE.md §4.2
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum TerminalReason {
    /// 模型说完了，没有待执行的工具。
    Completed,
    MaxTurns {
        limit: u32,
    },
    Aborted {
        by: AbortSource,
    },
    /// 中断发生时有工具正在执行，已全部取消并补齐 tool_result。
    AbortedTools {
        cancelled: usize,
    },
    StopHookPrevented {
        message: String,
    },
    /// 不可恢复错误。可恢复的错误不会走到这里 —— 它们被扣留并触发恢复循环。
    /// 见 ARCHITECTURE.md §5.4
    Error {
        error: AgentError,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AbortSource {
    /// 用户按 Esc。
    User,
    /// 用户中途插话。UI **不**显示"已中断"文案 ——
    /// 后续排队消息自带上下文，提示反而是噪声。
    UserInterjection,
    /// 同批兄弟工具失败导致的级联取消。
    SiblingFailure,
    /// 权限被拒且需要结束整轮。
    PermissionDenied,
    /// 进程关闭。
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentError {
    /// 上下文溢出且所有压缩手段都失败了。
    ContextExhausted { used: u32, limit: u32 },
    /// 压缩连续失败触发熔断。
    CompactCircuitOpen { attempts: u8 },
    /// Provider 层不可恢复错误（重试耗尽、认证失败等）。
    Provider { message: String, retryable: bool },
    /// 内核内部错误（含被捕获的 panic）。
    Internal { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompactStrategy {
    /// 大结果落盘，消息里留预览。无信息损失。
    Spill,
    /// 单消息内并行工具结果合计超预算，按 id 替换。
    AggregateBudget,
    /// 旧 tool_result 清成占位符。仅在 prompt cache 大概率已冷时执行。
    MicroCompact,
    /// LLM 全量总结。最后手段。
    FullSummary,
}

/// 流式增量的种类。
///
/// `[约束]` tag 必须是 `kind`，不能是 `type`。
///
/// `AgentEvent::Delta` 是 newtype variant，serde 的 internally-tagged 表示会把
/// 这里的字段**摊平**到 AgentEvent 那一层。两边都叫 `type` 的话，序列化产物是
/// `{"type":"delta","type":"text",...}` —— 重复 key，反序列化直接报
/// `duplicate field`，前端一个 token 都收不到。
///
/// 由 `every_event_variant_roundtrips` 断言。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamDelta {
    /// 助手文本增量。
    Text { message_id: MessageId, text: String },
    /// 思考过程增量。
    Thinking { message_id: MessageId, text: String },
    /// 工具参数增量。
    ///
    /// 这里刻意只传原始字符串片段，**不做增量 JSON 解析** ——
    /// 每个 delta 都 parse 是 O(n²)，大工具参数下很明显。
    /// 完整参数在 Message 事件里给出。见 ARCHITECTURE.md §11.3
    ToolInput {
        tool_use_id: ToolUseId,
        partial_json: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProgressPayload {
    /// 一行输出（Bash 的 stdout/stderr 流式）。
    Line { stream: OutputStream, text: String },
    /// 已知总量的进度。
    Fraction {
        done: u64,
        total: u64,
        label: String,
    },
    /// 无法量化的状态更新。
    Status { text: String },
    /// 子 agent 的嵌套事件（套娃显示）。
    Nested { event: Box<AgentEvent> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_and_progress_are_not_durable() {
        let d = AgentEvent::Delta(StreamDelta::Text {
            message_id: MessageId::from_raw("m1"),
            text: "hi".into(),
        });
        assert!(!d.is_durable());

        let done = AgentEvent::Done {
            reason: TerminalReason::Completed,
        };
        assert!(done.is_durable() && done.is_done());
    }

    /// 每个变体都要能原样往返。
    ///
    /// 这条测试存在的理由是一个真实踩到的坑：`Delta(StreamDelta)` 是 newtype
    /// variant，serde 的 internally-tagged 表示会把内层字段**摊平**到同一层。
    /// 当时内外层都用 `tag = "type"`，序列化出来是两个同名 key，
    /// 反序列化直接失败。改内层 tag 为 `kind` 才修好。
    ///
    /// 凡是新增 newtype variant，都要在这里补一条 —— 嵌套 tag 撞名
    /// 在类型层面看不出来，只有 roundtrip 能抓到。
    #[test]
    fn every_event_variant_roundtrips() {
        use crate::message::{AssistantContent, Message, MessageMeta};

        let cases = vec![
            AgentEvent::RequestStart {
                turn: 1,
                model: "claude-x".into(),
                after: None,
            },
            AgentEvent::RequestStart {
                turn: 2,
                model: "claude-x".into(),
                after: Some(Transition::ReactiveCompactRetry),
            },
            AgentEvent::Delta(StreamDelta::Text {
                message_id: MessageId::from_raw("m1"),
                text: "hi".into(),
            }),
            AgentEvent::Delta(StreamDelta::Thinking {
                message_id: MessageId::from_raw("m1"),
                text: "hmm".into(),
            }),
            AgentEvent::Delta(StreamDelta::ToolInput {
                tool_use_id: ToolUseId::from_raw("u1"),
                partial_json: "{\"a\":".into(),
            }),
            AgentEvent::Message(Message::Assistant {
                id: MessageId::from_raw("m1"),
                content: vec![AssistantContent::Text { text: "hi".into() }],
                usage: Default::default(),
                meta: MessageMeta::default(),
            }),
            AgentEvent::Progress {
                tool_use_id: ToolUseId::from_raw("u1"),
                payload: ProgressPayload::Status {
                    text: "工作中".into(),
                },
            },
            AgentEvent::Compacted {
                before_tokens: 100,
                after_tokens: 10,
                strategy: CompactStrategy::MicroCompact,
            },
            AgentEvent::Done {
                reason: TerminalReason::Completed,
            },
        ];

        for original in cases {
            let json = serde_json::to_string(&original).expect("序列化");
            let back: AgentEvent = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("反序列化失败: {e}\n  JSON: {json}"));
            assert_eq!(back, original, "往返后变了: {json}");
        }
    }

    #[test]
    fn terminal_reason_roundtrips() {
        let r = TerminalReason::Aborted {
            by: AbortSource::UserInterjection,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#"{"reason":"aborted","by":"user_interjection"}"#);
        assert_eq!(serde_json::from_str::<TerminalReason>(&json).unwrap(), r);
    }
}
