//! 历史自愈：为悬空的 tool_use 就地补上错误结果。
//!
//! # 悬空 tool_use 从哪来
//!
//! 内核自己的 `state.messages` 每轮都有配对保证（INV-1），但**宿主持久化
//! 的历史**走的是另一条路 —— 逐条追加事件流里的 Message。三条真实路径
//! 会让两边分叉，留下带 tool_calls 却永远等不到结果的 assistant 消息：
//!
//! 1. **流中途断开**：provider 先把解码器里攒了一半的消息吐出去（可能带
//!    完整的 tool_calls），再报不可恢复的传输错误。宿主在错误到达之前
//!    就已经把那条消息写进历史和 transcript；
//! 2. **输出截断重试**：`discard_for_retry` 只清内核状态，已经 yield 的
//!    消息宿主照收不误；
//! 3. **工具执行途中进程被杀**：tool_calls 落了盘，结果永远没写。
//!
//! # 为什么必须修
//!
//! 带着孤儿 tool_calls 的历史发给严格校验的服务端（DeepSeek 等）是必然
//! 400（"An assistant message with 'tool_calls' must be followed by tool
//! messages…"），而且**每次重试都 400** —— 会话等于永久废掉。宽松的服务端
//! （智谱等）不校验，同一份历史能跑，这正是"换模型好了、换回来又坏"的
//! 表象来源。
//!
//! # 为什么就地插入而不是补在末尾
//!
//! OpenAI 协议要求 tool 消息**紧跟**在带 tool_calls 的 assistant 消息之后。
//! 补在历史末尾的话，中间隔着后来的对话，序列照样非法 —— 等于没补。

use std::collections::BTreeSet;

use riot_protocol::id::{MessageId, ToolUseId};
use riot_protocol::message::{Message, MessageMeta, ToolResultContent, UserContent};

/// 合成结果的正文。措辞面向模型：说清结果没了、为什么、该怎么办。
const LOST_RESULT_TEXT: &str =
    "结果丢失：这次工具调用被异常中断（网络断开或程序退出），没有产生结果。仍然需要的话请重新调用。";

/// 为每个悬空的 tool_use 合成错误结果，插在它所在的 assistant 消息之后。
/// 返回修复的 tool_use 个数；0 = 历史本来就配对，未做任何改动。
///
/// 幂等：补上的结果让下一次调用找不到孤儿。合成消息的 id 从孤儿所在的
/// assistant 消息 id 派生 —— 确定性，重启后对同一份脏 transcript 重修
/// 会得到完全相同的历史（黄金回放安全）。
pub fn repair_tool_pairing(messages: &mut Vec<Message>) -> usize {
    let results: BTreeSet<ToolUseId> = messages
        .iter()
        .flat_map(|m| m.tool_result_ids())
        .cloned()
        .collect();

    // 先收集再倒序插入，避免边遍历边挪下标。
    let fixes: Vec<(usize, Vec<ToolUseId>)> = messages
        .iter()
        .enumerate()
        .filter_map(|(i, m)| {
            let orphans: Vec<ToolUseId> = m
                .tool_use_ids()
                .into_iter()
                .filter(|id| !results.contains(id))
                .cloned()
                .collect();
            (!orphans.is_empty()).then_some((i, orphans))
        })
        .collect();

    let mut repaired = 0;
    for (i, orphans) in fixes.into_iter().rev() {
        repaired += orphans.len();
        let msg = Message::User {
            id: MessageId::from_raw(format!("{}_repair", messages[i].id().as_str())),
            content: orphans
                .into_iter()
                .map(|id| UserContent::ToolResult {
                    tool_use_id: id,
                    content: ToolResultContent::text(LOST_RESULT_TEXT),
                    is_error: true,
                })
                .collect(),
            meta: MessageMeta {
                synthetic: true,
                ..Default::default()
            },
        };
        messages.insert(i + 1, msg);
    }
    repaired
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::invariants;
    use crate::testing::{assistant_tool_use, user_text};

    fn result_of(m: &Message) -> Vec<&ToolUseId> {
        m.tool_result_ids()
    }

    #[test]
    fn 干净历史一个字不动() {
        let mut msgs = vec![
            user_text("m1", "你好"),
            assistant_tool_use("a1", "tu_1", "Read", serde_json::json!({})),
            Message::User {
                id: MessageId::from_raw("m2"),
                content: vec![UserContent::ToolResult {
                    tool_use_id: ToolUseId::from_raw("tu_1"),
                    content: ToolResultContent::text("内容"),
                    is_error: false,
                }],
                meta: MessageMeta::default(),
            },
        ];
        let before = msgs.clone();
        assert_eq!(repair_tool_pairing(&mut msgs), 0);
        assert_eq!(msgs, before);
    }

    #[test]
    fn 孤儿结果插在_assistant_之后而不是末尾() {
        // 复现客户现场：流中断留下孤儿 tool_calls，之后用户又说了话。
        // 补在末尾的话 tool 消息和 assistant 之间隔着 user 文本，
        // 序列照样非法 —— 必须插在孤儿所在的 assistant 后面。
        let mut msgs = vec![
            user_text("m1", "帮我看下"),
            assistant_tool_use("a1", "tu_lost", "Read", serde_json::json!({})),
            user_text("m2", "怎么没反应，再试试"),
        ];
        assert_eq!(repair_tool_pairing(&mut msgs), 1);

        assert_eq!(msgs.len(), 4);
        assert_eq!(
            result_of(&msgs[2]),
            vec![&ToolUseId::from_raw("tu_lost")],
            "合成结果必须紧跟孤儿 assistant：{msgs:?}"
        );
        invariants::check_tool_pairing(&msgs);
        invariants::check_message_sequence(&msgs);
        assert!(invariants::take_violations().is_empty());
    }

    #[test]
    fn 修复是幂等且确定的() {
        let mut msgs = vec![
            assistant_tool_use("a1", "tu_1", "Read", serde_json::json!({})),
            user_text("m1", "后话"),
        ];
        repair_tool_pairing(&mut msgs);
        let first = msgs.clone();

        assert_eq!(repair_tool_pairing(&mut msgs), 0, "修过的不再修");
        assert_eq!(msgs, first);
        assert_eq!(
            msgs[1].id().as_str(),
            "a1_repair",
            "id 从 assistant 派生，重启后重修得到同一份历史"
        );
    }

    #[test]
    fn 部分结果只补缺的那几个() {
        // 批次跑到一半崩溃：tu_1 有结果，tu_2 没有。
        let mut msgs = vec![
            Message::Assistant {
                id: MessageId::from_raw("a1"),
                content: vec![
                    riot_protocol::message::AssistantContent::ToolUse {
                        id: ToolUseId::from_raw("tu_1"),
                        name: "Read".into(),
                        input: serde_json::json!({}),
                    },
                    riot_protocol::message::AssistantContent::ToolUse {
                        id: ToolUseId::from_raw("tu_2"),
                        name: "Read".into(),
                        input: serde_json::json!({}),
                    },
                ],
                usage: None,
                meta: MessageMeta::default(),
            },
            Message::User {
                id: MessageId::from_raw("m1"),
                content: vec![UserContent::ToolResult {
                    tool_use_id: ToolUseId::from_raw("tu_1"),
                    content: ToolResultContent::text("好了"),
                    is_error: false,
                }],
                meta: MessageMeta::default(),
            },
        ];
        assert_eq!(repair_tool_pairing(&mut msgs), 1);
        assert_eq!(result_of(&msgs[1]), vec![&ToolUseId::from_raw("tu_2")]);
        invariants::check_tool_pairing(&msgs);
        assert!(invariants::take_violations().is_empty());
    }

    #[test]
    fn 多处孤儿各自修复() {
        let mut msgs = vec![
            assistant_tool_use("a1", "tu_1", "Read", serde_json::json!({})),
            user_text("m1", "中间的话"),
            assistant_tool_use("a2", "tu_2", "Grep", serde_json::json!({})),
        ];
        assert_eq!(repair_tool_pairing(&mut msgs), 2);
        assert_eq!(msgs.len(), 5);
        assert_eq!(result_of(&msgs[1]), vec![&ToolUseId::from_raw("tu_1")]);
        assert_eq!(result_of(&msgs[4]), vec![&ToolUseId::from_raw("tu_2")]);
        invariants::check_tool_pairing(&msgs);
        assert!(invariants::take_violations().is_empty());
    }
}
