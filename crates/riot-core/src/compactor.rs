//! 基础压缩器：清理旧的工具结果。
//!
//! 这是「轻 → 重」阶梯里的第一档，也是目前唯一实现的一档。它不调 LLM、
//! 不丢消息结构，只把老的 tool_result 内容换成占位符。
//!
//! 为什么先做这一档：会话里绝大部分 token 是工具输出（文件内容、命令输出、
//! 搜索结果），而其中绝大部分在几轮之后就没用了 —— 模型已经从里面提取过
//! 需要的信息。相比之下对话文本本身很省。
//!
//! `[约束]` 清理时**留下 `Cleared` 占位符，不删消息**。整条删掉会破坏
//! tool_use / tool_result 配对，下一次请求直接 400。

use async_trait::async_trait;
use riot_protocol::compact::{CompactBudget, CompactResult, Compactor};
use riot_protocol::event::CompactStrategy;
use riot_protocol::message::{Message, ToolResultContent, UserContent};

/// 最近多少条消息不动。
///
/// 模型正在处理的东西必须留着。清掉当前这一轮刚拿到的文件内容，
/// 它下一步就会重新读一遍 —— 压缩反而让 token 变多。
const KEEP_RECENT: usize = 8;

pub struct ClearOldResults {
    keep_recent: usize,
}

impl Default for ClearOldResults {
    fn default() -> Self {
        Self {
            keep_recent: KEEP_RECENT,
        }
    }
}

impl ClearOldResults {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn keeping(keep_recent: usize) -> Self {
        Self { keep_recent }
    }
}

#[async_trait]
impl Compactor for ClearOldResults {
    async fn compact(&self, messages: Vec<Message>, budget: CompactBudget) -> CompactResult {
        let before = budget.current_tokens;
        let cutoff = messages.len().saturating_sub(self.keep_recent);

        let mut cleared = 0usize;
        let mut freed_bytes = 0usize;

        let out: Vec<Message> = messages
            .into_iter()
            .enumerate()
            .map(|(i, m)| {
                if i >= cutoff {
                    return m;
                }
                match m {
                    Message::User { id, content, meta } => {
                        let content = content
                            .into_iter()
                            .map(|c| match c {
                                UserContent::ToolResult {
                                    tool_use_id,
                                    content,
                                    is_error,
                                } => {
                                    // 已经清过的不重复计数，否则反复压缩会
                                    // 一直"成功"，熔断永远触发不了
                                    if matches!(content, ToolResultContent::Cleared) {
                                        return UserContent::ToolResult {
                                            tool_use_id,
                                            content,
                                            is_error,
                                        };
                                    }
                                    cleared += 1;
                                    freed_bytes += result_bytes(&content);
                                    UserContent::ToolResult {
                                        tool_use_id,
                                        content: ToolResultContent::Cleared,
                                        is_error,
                                    }
                                }
                                other => other,
                            })
                            .collect();
                        Message::User { id, content, meta }
                    }
                    other => other,
                }
            })
            .collect();

        if cleared == 0 {
            // 说清楚是哪一种"压不动"。主循环会累加熔断计数，而排查时
            // "没有可清理的" 和 "清了但没瘦下来" 要采取的动作完全不同。
            return CompactResult::Failed {
                reason: "没有可清理的历史工具结果 —— 占用 token 的是对话本身，需要摘要式压缩".into(),
            };
        }

        // 估算方式和 Provider::count_tokens 保持一致（4 字节 ≈ 1 token）。
        // 用不同的口径会让"压缩后仍然超预算"这个判断出现漂移。
        let after = before.saturating_sub((freed_bytes / 4) as u32);

        tracing::info!(cleared, freed_bytes, before, after, "清理了旧工具结果");

        CompactResult::Compacted {
            messages: out,
            before_tokens: before,
            after_tokens: after,
            strategy: CompactStrategy::MicroCompact,
        }
    }
}

fn result_bytes(c: &ToolResultContent) -> usize {
    match c {
        ToolResultContent::Text { text } => text.len(),
        ToolResultContent::Spilled { preview, .. } => preview.len(),
        ToolResultContent::Image { data, .. } => data.len(),
        ToolResultContent::Cleared => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use riot_protocol::id::{MessageId, ToolUseId};
    use riot_protocol::message::MessageMeta;

    fn result_msg(n: usize, text: &str) -> Message {
        Message::User {
            id: MessageId::from_raw(format!("u{n}")),
            content: vec![UserContent::ToolResult {
                tool_use_id: ToolUseId::from_raw(format!("t{n}")),
                content: ToolResultContent::text(text),
                is_error: false,
            }],
            meta: MessageMeta::default(),
        }
    }

    fn budget() -> CompactBudget {
        CompactBudget {
            target_tokens: 100,
            current_tokens: 1000,
        }
    }

    #[tokio::test]
    async fn 旧结果被替换成占位符() {
        let msgs: Vec<_> = (0..12).map(|i| result_msg(i, "很长的文件内容……")).collect();
        let r = ClearOldResults::keeping(4).compact(msgs, budget()).await;

        let CompactResult::Compacted { messages, .. } = r else {
            panic!("应该压缩成功");
        };

        let cleared = messages
            .iter()
            .filter(|m| {
                matches!(m, Message::User { content, .. }
                    if content.iter().any(|c| matches!(c,
                        UserContent::ToolResult { content: ToolResultContent::Cleared, .. })))
            })
            .count();
        assert_eq!(cleared, 8, "12 条留 4 条，应该清掉 8 条");
    }

    #[tokio::test]
    async fn 配对不被破坏() {
        // `[约束]` 删消息会让 tool_use 变成孤儿，下次请求 400
        let msgs: Vec<_> = (0..12).map(|i| result_msg(i, "内容")).collect();
        let before = msgs.len();

        let CompactResult::Compacted { messages, .. } =
            ClearOldResults::keeping(4).compact(msgs, budget()).await
        else {
            panic!("应该压缩成功");
        };

        assert_eq!(messages.len(), before, "消息条数不能变");
        for m in &messages {
            assert_eq!(m.tool_result_ids().len(), 1, "每条的 tool_result 都要还在");
        }
    }

    #[tokio::test]
    async fn 最近的结果不动() {
        // 清掉刚读到的文件，模型下一步就会重读一遍，压缩反而变贵
        let msgs: Vec<_> = (0..6).map(|i| result_msg(i, "内容")).collect();

        let CompactResult::Compacted { messages, .. } =
            ClearOldResults::keeping(4).compact(msgs, budget()).await
        else {
            panic!("应该压缩成功");
        };

        for m in messages.iter().skip(2) {
            match m {
                Message::User { content, .. } => assert!(
                    matches!(
                        &content[0],
                        UserContent::ToolResult {
                            content: ToolResultContent::Text { .. },
                            ..
                        }
                    ),
                    "最近的不该被清"
                ),
                _ => unreachable!(),
            }
        }
    }

    #[tokio::test]
    async fn 没有可清理的就报失败() {
        let msgs = vec![Message::User {
            id: MessageId::from_raw("u1"),
            content: vec![UserContent::Text {
                text: "只是聊天".into(),
            }],
            meta: MessageMeta::default(),
        }];

        assert!(matches!(
            ClearOldResults::keeping(0).compact(msgs, budget()).await,
            CompactResult::Failed { .. }
        ));
    }

    #[tokio::test]
    async fn 重复压缩第二次会失败() {
        // `[约束]` 已清理的不再计数。否则每次压缩都"成功"，熔断永远
        // 触发不了，主循环会在压缩和重试之间转圈烧钱。
        let msgs: Vec<_> = (0..6).map(|i| result_msg(i, "内容")).collect();
        let c = ClearOldResults::keeping(0);

        let CompactResult::Compacted { messages, .. } = c.compact(msgs, budget()).await else {
            panic!("第一次应该成功");
        };

        assert!(
            matches!(
                c.compact(messages, budget()).await,
                CompactResult::Failed { .. }
            ),
            "第二次没有新东西可清，必须报失败"
        );
    }

    #[tokio::test]
    async fn 压缩后的估算变小() {
        let msgs: Vec<_> = (0..6)
            .map(|i| result_msg(i, &"x".repeat(4000)))
            .collect();

        let CompactResult::Compacted {
            before_tokens,
            after_tokens,
            ..
        } = ClearOldResults::keeping(0).compact(msgs, budget()).await
        else {
            panic!("应该成功");
        };
        assert!(after_tokens < before_tokens);
    }
}
