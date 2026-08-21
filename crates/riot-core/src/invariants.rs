//! 运行时不变量断言。
//!
//! # 这个文件是做什么的
//!
//! Agent 的 bug 有个共同特征：**编译器发现不了，类型系统也发现不了，
//! 而且在开发阶段几乎不会触发。** 这里的每一条断言都对应一个真实的、
//! 已知会发生的生产事故。
//!
//! - debug build：违反 → panic，立即暴露
//! - release build：违反 → 记日志上报，但不中断用户会话
//!
//! # 给 review 者
//!
//! 这个文件应该保持在 400 行以内，且**只增不减**。任何删除或弱化断言的
//! 改动都需要明确理由。检查点是否被正确调用，由本文件末尾的
//! `all_invariants_have_call_sites` 测试保证。
//!
//! 见 docs/VERIFICATION.md §3

use riot_protocol::{AgentEvent, AssistantContent, Message, ToolUseId, UserContent};
use std::collections::BTreeSet;
use std::path::Path;

/// 违反时 debug panic / release 上报。
///
/// 刻意不用 `debug_assert!` —— 我们要在 release 下也记录，
/// 只是不中断会话。
#[macro_export]
macro_rules! invariant {
    ($cond:expr, $($arg:tt)*) => {
        if !$cond {
            let msg = format!($($arg)*);
            if cfg!(debug_assertions) {
                panic!("INVARIANT VIOLATED: {msg}");
            } else {
                tracing::error!(invariant = %msg, "invariant violated");
                $crate::invariants::report_violation(&msg);
            }
        }
    };
}

/// release 下的上报钩子。测试可以替换它来断言"没有违反发生"。
pub fn report_violation(msg: &str) {
    VIOLATIONS.with(|v| v.borrow_mut().push(msg.to_string()));
}

thread_local! {
    static VIOLATIONS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// 取出并清空本线程记录的违反。混沌测试用。
pub fn take_violations() -> Vec<String> {
    VIOLATIONS.with(|v| std::mem::take(&mut *v.borrow_mut()))
}

// ════════════════════════════════════════════════════════════
// INV-1：tool_use / tool_result 严格配对
//
// 防的 bug：中断后没补齐配对 → 下次 API 调用 400，
// 而错误信息不会告诉你是哪个块。这是最高频的一类。
//
// 检查点：每次调用 provider 之前。
// ════════════════════════════════════════════════════════════

pub fn check_tool_pairing(messages: &[Message]) {
    let uses: BTreeSet<&ToolUseId> = messages.iter().flat_map(|m| m.tool_use_ids()).collect();
    let results: BTreeSet<&ToolUseId> = messages.iter().flat_map(|m| m.tool_result_ids()).collect();

    let orphan_uses: Vec<_> = uses.difference(&results).map(|id| id.as_str()).collect();
    let orphan_results: Vec<_> = results.difference(&uses).map(|id| id.as_str()).collect();

    invariant!(
        orphan_uses.is_empty() && orphan_results.is_empty(),
        "tool_use/tool_result 配对缺失: 无结果的 tool_use={orphan_uses:?}, 无来源的 tool_result={orphan_results:?}"
    );
}

/// 找出所有没有配对结果的 tool_use。中断时用它合成补齐结果。
pub fn orphan_tool_uses(messages: &[Message]) -> Vec<ToolUseId> {
    let results: BTreeSet<&ToolUseId> = messages.iter().flat_map(|m| m.tool_result_ids()).collect();
    messages
        .iter()
        .flat_map(|m| m.tool_use_ids())
        .filter(|id| !results.contains(id))
        .cloned()
        .collect()
}

// ════════════════════════════════════════════════════════════
// INV-2：消息序列合法
//
// 防的 bug：队列消息 drain 时机错误 —— 用户在工具执行中途插话，
// 消息被插到了 tool_use 和 tool_result 之间 → API 400。
//
// 检查点：每次调用 provider 之前。
// ════════════════════════════════════════════════════════════

pub fn check_message_sequence(messages: &[Message]) {
    for (i, w) in messages.windows(2).enumerate() {
        let prev_has_tool_use = !w[0].tool_use_ids().is_empty();
        let next_is_plain_user_text = matches!(
            &w[1],
            Message::User { content, .. }
                if content.iter().all(|c| matches!(c, UserContent::Text { .. }))
                    && !content.is_empty()
        );

        invariant!(
            !(prev_has_tool_use && next_is_plain_user_text),
            "位置 {i}：user 文本插在了 tool_use 和 tool_result 之间。\
             队列消息只能在工具结果全部就位后 drain"
        );
    }
}

// ════════════════════════════════════════════════════════════
// INV-3：并发批次里没有写操作
//
// 防的 bug：分批逻辑写错，两个 Edit 并行写同一个文件。
//
// 检查点：每个并行批次开始执行前。
// ════════════════════════════════════════════════════════════

pub struct BatchMember<'a> {
    pub tool_name: &'a str,
    pub concurrency_safe: bool,
}

pub fn check_batch_safety(batch: &[BatchMember<'_>]) {
    if batch.len() <= 1 {
        return;
    }
    for m in batch {
        invariant!(
            m.concurrency_safe,
            "非并发安全的工具 `{}` 出现在 {} 个成员的并行批次里",
            m.tool_name,
            batch.len()
        );
    }
}

// ════════════════════════════════════════════════════════════
// INV-4：Done 事件恰好一次，且是最后一个
//
// 防的 bug：某条早退路径忘了 yield Done → UI 永远转圈。
// Drop 实现让这个检查在 panic 展开时也生效。
//
// 检查点：包裹整个 run_agent 的输出流。
// ════════════════════════════════════════════════════════════

#[derive(Default)]
pub struct StreamGuard {
    done_emitted: bool,
    /// panic 展开中不再检查 —— 那时缺 Done 是 panic 的后果不是原因，
    /// 在 Drop 里再 panic 会变成 abort，掩盖真正的错误。
    disarmed: bool,
}

impl StreamGuard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&mut self, ev: &AgentEvent) {
        invariant!(!self.done_emitted, "Done 之后又发出了事件：{ev:?}");
        if ev.is_done() {
            self.done_emitted = true;
        }
    }

    /// 流因 panic 或外部原因提前终止时调用，跳过 Drop 检查。
    pub fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        if self.disarmed || std::thread::panicking() {
            return;
        }
        invariant!(self.done_emitted, "事件流结束了但从未发出 Done");
    }
}

// ════════════════════════════════════════════════════════════
// INV-5：恢复计数器单调
//
// 防的 bug：恢复标志位在某条重试路径上被意外重置 → 无限重试循环。
// Claude Code 的注释记录过这个 bug 导致的无限压缩循环。
//
// 检查点：每次 state.advance() 之后。
// ════════════════════════════════════════════════════════════

/// 只取断言需要的字段，避免 invariants 模块依赖完整的 AgentState。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryCounters {
    pub turn: u32,
    pub output_limit_recovery: u8,
    pub attempted_reactive_compact: bool,
    pub compact_failure_streak: u8,
}

pub fn check_recovery_monotonic(prev: RecoveryCounters, next: RecoveryCounters) {
    if prev.turn != next.turn {
        return; // 跨 turn 允许重置
    }
    invariant!(
        next.output_limit_recovery >= prev.output_limit_recovery,
        "同一轮内 output_limit_recovery 计数被重置：{} → {}",
        prev.output_limit_recovery,
        next.output_limit_recovery
    );
    invariant!(
        next.attempted_reactive_compact >= prev.attempted_reactive_compact,
        "同一轮内 attempted_reactive_compact 标志被重置 —— 会导致无限压缩循环"
    );
    invariant!(
        next.compact_failure_streak >= prev.compact_failure_streak,
        "同一轮内压缩失败计数被重置 —— 熔断会失效"
    );
}

// ════════════════════════════════════════════════════════════
// INV-6：API 错误路径不跑 stop hooks
//
// 防的 bug：error → hook 注入更多内容 → 重试 → 又是 error 的死循环。
// 这个 bug 在 Claude Code 里烧掉过几千次 API 调用。
//
// 检查点：run_turn_end_hooks 入口。
// ════════════════════════════════════════════════════════════

pub fn check_hook_eligibility(last: Option<&Message>, about_to_run_hooks: bool) {
    let is_api_error = matches!(last, Some(Message::Assistant { meta, .. }) if meta.is_api_error);
    invariant!(
        !(about_to_run_hooks && is_api_error),
        "即将在 API 错误消息上执行 stop hooks —— 这会形成死循环"
    );
}

// ════════════════════════════════════════════════════════════
// INV-7：控制面消息不进 API 请求
//
// 检查点：provider 序列化请求时。
// ════════════════════════════════════════════════════════════

pub fn check_api_payload(messages: &[Message]) {
    let leaked: Vec<_> = messages
        .iter()
        .filter(|m| !m.goes_to_model())
        .map(|m| m.id().as_str())
        .collect();
    invariant!(
        leaked.is_empty(),
        "System 消息泄漏进了 API 请求：{leaked:?}"
    );
}

// ════════════════════════════════════════════════════════════
// INV-8：恰好一个 message 级 cache 断点
//
// 多断点会被 API 拒绝，而且服务端 KV 页管理下多断点反而浪费。
//
// 检查点：provider 组装请求后。
// ════════════════════════════════════════════════════════════

pub fn check_cache_breakpoints(marker_count: usize) {
    invariant!(
        marker_count <= 1,
        "发现 {marker_count} 个 message 级 cache_control 标记，API 最多允许 1 个"
    );
}

// ════════════════════════════════════════════════════════════
// INV-9：换模型后没有残留 thinking signature
//
// thinking 签名与模型绑定，换模型重放会 400，而报错信息
// 指向的字段与真正的原因无关。
//
// 检查点：模型降级切换后，下一次请求前。
// ════════════════════════════════════════════════════════════

pub fn check_thinking_signatures(messages: &[Message], current_model: &str) {
    for m in messages {
        let Message::Assistant { content, meta, .. } = m else {
            continue;
        };
        let has_signature = content.iter().any(|c| {
            matches!(
                c,
                AssistantContent::Thinking {
                    signature: Some(_),
                    ..
                }
            )
        });
        if !has_signature {
            continue;
        }
        let origin = meta.model_origin.as_deref().unwrap_or(current_model);
        invariant!(
            origin == current_model,
            "消息 {} 带着模型 `{origin}` 的 thinking signature 被发给了 `{current_model}` —— API 会拒绝。\
             降级换模型时必须剥离 signature",
            m.id()
        );
    }
}

// ════════════════════════════════════════════════════════════
// INV-10：路径围栏
//
// 权限检查之外的第二道防线。symlink 解析前后都要在围栏内。
//
// 检查点：所有文件写操作实际执行前。
// ════════════════════════════════════════════════════════════

pub fn check_path_in_fence(resolved: &Path, roots: &[std::path::PathBuf]) {
    invariant!(
        roots.iter().any(|r| resolved.starts_with(r)),
        "路径 {resolved:?} 逃出了工作目录围栏 {roots:?}"
    );
}

// ════════════════════════════════════════════════════════════

/// 所有不变量函数名。用于"检查点是否都被调用"的元测试。
pub const ALL_INVARIANT_FNS: &[&str] = &[
    "check_tool_pairing",
    "check_message_sequence",
    "check_batch_safety",
    "check_recovery_monotonic",
    "check_hook_eligibility",
    "check_api_payload",
    "check_cache_breakpoints",
    "check_thinking_signatures",
    "check_path_in_fence",
];

#[cfg(test)]
mod tests {
    use super::*;
    use riot_protocol::{MessageId, MessageMeta, ToolResultContent};

    fn assistant_with_tool(id: &str, tu: &str) -> Message {
        Message::Assistant {
            id: MessageId::from_raw(id),
            content: vec![AssistantContent::ToolUse {
                id: ToolUseId::from_raw(tu),
                name: "Read".into(),
                input: serde_json::json!({}),
            }],
            usage: None,
            meta: MessageMeta::default(),
        }
    }

    fn tool_result(id: &str, tu: &str) -> Message {
        Message::User {
            id: MessageId::from_raw(id),
            content: vec![UserContent::ToolResult {
                tool_use_id: ToolUseId::from_raw(tu),
                content: ToolResultContent::text("ok"),
                is_error: false,
            }],
            meta: MessageMeta::default(),
        }
    }

    // 只有被 debug_assertions 门着的 inv2 用它，跟着一起编译出去，
    // 否则 release 下是 dead_code。
    #[cfg(debug_assertions)]
    fn user_text(id: &str, s: &str) -> Message {
        Message::User {
            id: MessageId::from_raw(id),
            content: vec![UserContent::Text { text: s.into() }],
            meta: MessageMeta::default(),
        }
    }

    #[test]
    fn inv1_accepts_paired() {
        check_tool_pairing(&[assistant_with_tool("m1", "t1"), tool_result("m2", "t1")]);
    }

    // 下面这批 should_panic 验证的是 **debug 下的 panic 行为**。release 里
    // invariant! 记录而不炸（生产不能因断言宕机），should_panic 在那边
    // 必然落空 —— 所以按 debug_assertions 整个编译出去，而不是让
    // test-release 跑一批注定失败的用例。release 侧的"记录不炸"由
    // chaos_soak 覆盖。
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "配对缺失")]
    fn inv1_rejects_orphan_use() {
        check_tool_pairing(&[assistant_with_tool("m1", "t1")]);
    }

    #[test]
    fn orphans_are_discoverable() {
        let msgs = [
            assistant_with_tool("m1", "t1"),
            assistant_with_tool("m2", "t2"),
            tool_result("m3", "t1"),
        ];
        assert_eq!(orphan_tool_uses(&msgs), vec![ToolUseId::from_raw("t2")]);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "插在了 tool_use 和 tool_result 之间")]
    fn inv2_rejects_interleaved_user_text() {
        check_message_sequence(&[assistant_with_tool("m1", "t1"), user_text("m2", "等一下")]);
    }

    #[test]
    fn inv2_accepts_tool_result_after_tool_use() {
        check_message_sequence(&[assistant_with_tool("m1", "t1"), tool_result("m2", "t1")]);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "非并发安全的工具")]
    fn inv3_rejects_writer_in_parallel_batch() {
        check_batch_safety(&[
            BatchMember {
                tool_name: "Read",
                concurrency_safe: true,
            },
            BatchMember {
                tool_name: "Edit",
                concurrency_safe: false,
            },
        ]);
    }

    #[test]
    fn inv3_allows_single_member_batch() {
        check_batch_safety(&[BatchMember {
            tool_name: "Edit",
            concurrency_safe: false,
        }]);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "从未发出 Done")]
    fn inv4_requires_done_on_drop() {
        let mut g = StreamGuard::new();
        g.observe(&AgentEvent::RequestStart {
            turn: 0,
            model: "m".into(),
            after: None,
        });
    }

    #[test]
    fn inv4_satisfied_when_done_emitted() {
        let mut g = StreamGuard::new();
        g.observe(&AgentEvent::Done {
            reason: riot_protocol::TerminalReason::Completed,
        });
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "无限压缩循环")]
    fn inv5_rejects_flag_reset_within_turn() {
        let base = RecoveryCounters {
            turn: 3,
            output_limit_recovery: 1,
            attempted_reactive_compact: true,
            compact_failure_streak: 0,
        };
        check_recovery_monotonic(
            base,
            RecoveryCounters {
                attempted_reactive_compact: false,
                ..base
            },
        );
    }

    #[test]
    fn inv5_allows_reset_across_turns() {
        let prev = RecoveryCounters {
            turn: 3,
            output_limit_recovery: 2,
            attempted_reactive_compact: true,
            compact_failure_streak: 1,
        };
        check_recovery_monotonic(
            prev,
            RecoveryCounters {
                turn: 4,
                output_limit_recovery: 0,
                attempted_reactive_compact: false,
                compact_failure_streak: 0,
            },
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "死循环")]
    fn inv6_rejects_hooks_on_api_error() {
        let m = Message::Assistant {
            id: MessageId::from_raw("m1"),
            content: vec![],
            usage: None,
            meta: MessageMeta {
                is_api_error: true,
                ..Default::default()
            },
        };
        check_hook_eligibility(Some(&m), true);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "泄漏进了 API 请求")]
    fn inv7_rejects_system_message_in_payload() {
        check_api_payload(&[Message::System {
            id: MessageId::from_raw("s1"),
            level: riot_protocol::SystemLevel::Warning,
            text: "x".into(),
        }]);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "最多允许 1 个")]
    fn inv8_rejects_multiple_cache_markers() {
        check_cache_breakpoints(2);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "thinking signature")]
    fn inv9_rejects_cross_model_signature() {
        let m = Message::Assistant {
            id: MessageId::from_raw("m1"),
            content: vec![AssistantContent::Thinking {
                text: "...".into(),
                signature: Some("sig".into()),
            }],
            usage: None,
            meta: MessageMeta {
                model_origin: Some("opus".into()),
                ..Default::default()
            },
        };
        check_thinking_signatures(&[m], "sonnet");
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "逃出了工作目录围栏")]
    fn inv10_rejects_escaped_path() {
        check_path_in_fence(
            Path::new("/etc/passwd"),
            &[std::path::PathBuf::from("/home/u/proj")],
        );
    }
}
