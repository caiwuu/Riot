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

        // 换算走 protocol 里那个共享函数 —— 和 Provider::count_tokens 同一个
        // 口径。各写一遍 `/ 4` 会漂移，而漂移的表现是"压缩后仍然超预算"这个
        // 判断时对时错。
        let after = before.saturating_sub(riot_protocol::provider::estimate_tokens(freed_bytes));

        tracing::info!(cleared, freed_bytes, before, after, "清理了旧工具结果");

        CompactResult::Compacted {
            messages: out,
            before_tokens: before,
            after_tokens: after,
            strategy: CompactStrategy::MicroCompact,
        }
    }
}

/// 「轻 → 重」阶梯压缩器：先清旧工具结果，不够再 LLM 全量总结。
///
/// 这是 [`Compactor`] 契约里"必须按轻到重"那条约束的完整实现。挂在
/// 反应式路径上（413 溢出重试）：清结果是无损的，够用就不动用总结；
/// 总结有损而且要花一次真实调用，是最后手段。
///
/// `[约束]` 总结的输入是**原始消息**，不是轻档清理后的产物 —— 清掉的
/// 工具结果里可能有总结需要的细节（"文件与代码段"那一节）。反正总结
/// 请求本身是一次性的，输入大一点没关系；输出才是要控的。
pub struct Layered {
    light: ClearOldResults,
    provider: std::sync::Arc<dyn riot_protocol::provider::Provider>,
    model: String,
    ids: std::sync::Arc<dyn riot_protocol::id::IdGenerator>,
    /// 本轮的取消令牌（子令牌）。用户按停止时总结请求跟着断。
    cancel: tokio_util::sync::CancellationToken,
}

impl Layered {
    pub fn new(
        provider: std::sync::Arc<dyn riot_protocol::provider::Provider>,
        model: impl Into<String>,
        ids: std::sync::Arc<dyn riot_protocol::id::IdGenerator>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Self {
        Self {
            light: ClearOldResults::new(),
            provider,
            model: model.into(),
            ids,
            cancel,
        }
    }
}

#[async_trait]
impl Compactor for Layered {
    async fn compact(&self, messages: Vec<Message>, budget: CompactBudget) -> CompactResult {
        // ── 轻档：清旧工具结果（无损） ──────────────────
        let light = self.light.compact(messages.clone(), budget).await;
        if let CompactResult::Compacted { after_tokens, .. } = &light
            && *after_tokens <= budget.target_tokens
        {
            return light;
        }

        // ── 重档：LLM 全量总结（有损，最后手段） ─────────
        let summary = crate::summarize::summarize_history(
            &self.provider,
            &self.model,
            &messages,
            self.cancel.child_token(),
        )
        .await;

        match summary {
            Ok(text) => {
                // 反应式路径没有宿主能力，记忆和工作集不在这里重注 ——
                // 那是宿主主动压缩的事。这里保命优先：先让请求能发出去。
                let msg = crate::summarize::continuation_message(
                    &text,
                    Vec::new(),
                    Vec::new(),
                    self.ids.message_id(),
                );
                let after = self.provider.count_tokens(std::slice::from_ref(&msg));
                tracing::info!(before = budget.current_tokens, after, "LLM 总结完成");
                CompactResult::Compacted {
                    messages: vec![msg],
                    before_tokens: budget.current_tokens,
                    after_tokens: after,
                    strategy: CompactStrategy::FullSummary,
                }
            }
            // 总结失败但轻档清出过东西：交出轻档结果。有进步就别当失败 ——
            // 重试可能刚好够用，而失败会把熔断计数往前推一格。
            Err(reason) => match light {
                CompactResult::Compacted { .. } => {
                    tracing::warn!(reason, "总结失败，退回轻档的清理结果");
                    light
                }
                CompactResult::Failed { reason: light_reason } => CompactResult::Failed {
                    reason: format!("轻档：{light_reason}；总结：{reason}"),
                },
            },
        }
    }
}

fn result_bytes(c: &ToolResultContent) -> usize {
    match c {
        ToolResultContent::Text { text } => text.len(),
        ToolResultContent::Spilled { preview, .. } => preview.len(),
        ToolResultContent::Image { data, .. } => data.len(),
        // 模型只收到转述文字（provider 不发图），上下文占用按文字算。
        // 图片的 base64 只活在本地 transcript 里，不占模型预算。
        ToolResultContent::DescribedImage { text, .. } => text.len(),
        // 模型两路都收:图（data）+ 编号清单（text），都算进预算。
        ToolResultContent::MarkedImage { data, text, .. } => data.len() + text.len(),
        ToolResultContent::Cleared => 0,
    }
}

#[cfg(test)]
mod layered_tests {
    use super::*;
    use crate::testing::ScriptedProvider;
    use riot_protocol::id::MessageId;
    use riot_protocol::message::MessageMeta;
    use riot_protocol::provider::ProviderEvent;
    use std::sync::Arc;

    fn user_text(id: &str, text: &str) -> Message {
        Message::User {
            id: MessageId::from_raw(id),
            content: vec![UserContent::Text { text: text.into() }],
            meta: MessageMeta::default(),
        }
    }

    fn assistant_text(text: &str) -> Message {
        Message::Assistant {
            id: MessageId::from_raw("a1"),
            content: vec![riot_protocol::message::AssistantContent::Text { text: text.into() }],
            usage: None,
            meta: MessageMeta::default(),
        }
    }

    fn ids() -> Arc<dyn riot_protocol::id::IdGenerator> {
        Arc::new(riot_protocol::id::NanoIdGenerator)
    }

    #[tokio::test]
    async fn 对话本身超预算时升级到总结() {
        // 没有可清理的工具结果（全是对话文本）→ 轻档报"压不动" →
        // 必须升级总结而不是把 Failed 透传出去。
        let provider = Arc::new(ScriptedProvider::new(vec![vec![ProviderEvent::Message(
            assistant_text("<analysis>过一遍</analysis><summary>1. 主要请求：聊了很多</summary>"),
        )]]));
        let layered = Layered::new(
            Arc::clone(&provider) as Arc<dyn riot_protocol::provider::Provider>,
            "test-model",
            ids(),
            tokio_util::sync::CancellationToken::new(),
        );

        let messages = vec![user_text("m1", &"话很多。".repeat(500))];
        let r = layered
            .compact(messages, CompactBudget { target_tokens: 10, current_tokens: 1000 })
            .await;

        let CompactResult::Compacted { messages, strategy, .. } = r else {
            panic!("该升级到总结：{r:?}");
        };
        assert_eq!(strategy, CompactStrategy::FullSummary);
        assert_eq!(messages.len(), 1, "总结替换全部历史");
        assert!(
            format!("{:?}", messages[0]).contains("聊了很多"),
            "总结正文要进续接消息"
        );
        // 总结请求本身不带工具
        let reqs = provider.requests();
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0].tools.is_empty(), "总结模型调工具等于烧掉它唯一的一轮");
    }

    #[tokio::test]
    async fn 总结失败但轻档有进步时退回轻档() {
        // 有进步就别当失败 —— Failed 会把熔断计数往前推一格。
        let provider = Arc::new(ScriptedProvider::new(vec![vec![ProviderEvent::Error(
            riot_protocol::provider::ProviderError::Transport { message: "断网".into() },
        )]]));
        let layered = Layered::new(
            Arc::clone(&provider) as Arc<dyn riot_protocol::provider::Provider>,
            "test-model",
            ids(),
            tokio_util::sync::CancellationToken::new(),
        );

        // 造一段带可清理工具结果的历史（老到会被清），但目标定到清完也
        // 不够 —— 轻档 Compacted 但没到 target → 升级 → 总结失败 → 退轻档。
        let mut messages = vec![Message::User {
            id: MessageId::from_raw("m0"),
            content: vec![UserContent::ToolResult {
                tool_use_id: riot_protocol::id::ToolUseId::from_raw("t1"),
                content: ToolResultContent::text("很长的结果".repeat(100)),
                is_error: false,
            }],
            meta: MessageMeta::default(),
        }];
        for i in 0..10 {
            messages.push(user_text(&format!("m{}", i + 1), "填充"));
        }

        let r = layered
            .compact(messages, CompactBudget { target_tokens: 1, current_tokens: 10_000 })
            .await;
        let CompactResult::Compacted { strategy, .. } = r else {
            panic!("轻档有进步就该交出去：{r:?}");
        };
        assert_eq!(strategy, CompactStrategy::MicroCompact, "退回的是轻档产物");
    }

    #[tokio::test]
    async fn 两档都不行才算失败() {
        let provider = Arc::new(ScriptedProvider::new(vec![vec![ProviderEvent::Error(
            riot_protocol::provider::ProviderError::Transport { message: "断网".into() },
        )]]));
        let layered = Layered::new(
            Arc::clone(&provider) as Arc<dyn riot_protocol::provider::Provider>,
            "test-model",
            ids(),
            tokio_util::sync::CancellationToken::new(),
        );
        let r = layered
            .compact(
                vec![user_text("m1", "没有工具结果可清")],
                CompactBudget { target_tokens: 1, current_tokens: 100 },
            )
            .await;
        let CompactResult::Failed { reason } = r else {
            panic!("两档都不行必须如实报失败：{r:?}");
        };
        assert!(reason.contains("总结"), "失败原因要包含两档各自的说法：{reason}");
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
