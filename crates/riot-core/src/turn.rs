//! 单轮响应的累积器。
//!
//! 存在的理由是**扣留机制**：可恢复错误在流式过程中先不 yield，等恢复
//! 尝试全部失败了才吐出去。
//!
//! 为什么不能直接 yield：UI 一看到错误事件就会结束会话渲染（转圈停掉、
//! 输入框解锁），而此时恢复循环还在跑，没人在听结果。恢复成功了 UI 也不知道，
//! 于是用户看到「出错了」然后又莫名其妙冒出新内容。
//!
//! 见 ARCHITECTURE.md §5.4

use riot_protocol::message::{AssistantContent, Message, Usage};
use riot_protocol::provider::ProviderError;

use crate::state::ToolCall;

#[derive(Debug, Default)]
pub struct TurnAccumulator {
    messages: Vec<Message>,
    withheld: Option<ProviderError>,
    usage: Usage,
}

impl TurnAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// 扣下一个可恢复错误，不让它进事件流。
    ///
    /// 只保留第一个。后续错误多半是同一根因的连锁反应，用第一个做恢复决策
    /// 更准 —— 比如上下文溢出之后紧跟的传输错误，按传输错误处理就走错路了。
    pub fn withhold(&mut self, error: ProviderError) {
        if self.withheld.is_none() {
            self.withheld = Some(error);
        }
    }

    pub fn withheld(&self) -> Option<&ProviderError> {
        self.withheld.as_ref()
    }

    pub fn merge_usage(&mut self, incoming: &Usage) {
        self.usage.merge(incoming);
    }

    pub fn usage(&self) -> Usage {
        self.usage
    }

    /// 循环是否继续，只看这一个判据。
    ///
    /// `[约束]` **不要用 `stop_reason == "tool_use"` 判断。**那个字段不可靠，
    /// 实测会导致循环提前退出或死循环。Provider 层可以记录它用于遥测，
    /// 但不得参与控制流。
    pub fn has_tool_use(&self) -> bool {
        self.messages.iter().any(|m| !m.tool_use_ids().is_empty())
    }

    /// 本轮的工具调用，按模型给出的顺序。
    pub fn tool_calls(&self) -> Vec<ToolCall> {
        self.messages
            .iter()
            .filter_map(|m| match m {
                Message::Assistant { content, .. } => Some(content),
                _ => None,
            })
            .flatten()
            .filter_map(|c| match c {
                AssistantContent::ToolUse { id, name, input } => Some(ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                }),
                _ => None,
            })
            .collect()
    }

    /// 取走本轮消息，准备 commit 进 `AgentState`。
    pub fn take_messages(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.messages)
    }

    /// 丢弃本轮累积的内容，准备重试。
    ///
    /// `[约束]` 恢复重试前必须调这个。被 OutputLimit 截断的响应里可能有
    /// **半个 tool_use** —— input 的 JSON 都没拼完。把它 commit 进 transcript
    /// 会产生一个永远等不到 tool_result 的 tool_use，下一次 API 请求直接 400。
    ///
    /// 已经 yield 出去的 Message 事件收不回来，UI 需要在收到下一个
    /// RequestStart 时清理未确认的消息。这是有意的取舍：让内核保持单向
    /// 事件流，比引入撤销事件简单得多。
    pub fn discard_for_retry(&mut self) {
        self.messages.clear();
        self.withheld = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use riot_protocol::id::{MessageId, ToolUseId};
    use riot_protocol::message::MessageMeta;

    fn assistant(content: Vec<AssistantContent>) -> Message {
        Message::Assistant {
            id: MessageId::from_raw("m1"),
            content,
            usage: None,
            meta: MessageMeta::default(),
        }
    }

    #[test]
    fn 有_tool_use_才继续循环() {
        let mut t = TurnAccumulator::new();
        t.push(assistant(vec![AssistantContent::Text {
            text: "做完了".into(),
        }]));
        assert!(!t.has_tool_use(), "纯文本响应必须结束循环");

        t.push(assistant(vec![AssistantContent::ToolUse {
            id: ToolUseId::from_raw("u1"),
            name: "Read".into(),
            input: serde_json::json!({}),
        }]));
        assert!(t.has_tool_use());
    }

    #[test]
    fn 工具调用保持模型给出的顺序() {
        let mut t = TurnAccumulator::new();
        t.push(assistant(vec![
            AssistantContent::ToolUse {
                id: ToolUseId::from_raw("u1"),
                name: "Read".into(),
                input: serde_json::json!({"path": "a"}),
            },
            AssistantContent::Text {
                text: "然后".into(),
            },
            AssistantContent::ToolUse {
                id: ToolUseId::from_raw("u2"),
                name: "Read".into(),
                input: serde_json::json!({"path": "b"}),
            },
        ]));

        let calls = t.tool_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, ToolUseId::from_raw("u1"));
        assert_eq!(calls[1].id, ToolUseId::from_raw("u2"), "顺序不能乱");
    }

    #[test]
    fn 只扣留第一个错误() {
        let mut t = TurnAccumulator::new();
        t.withhold(ProviderError::ContextOverflow {
            used: 200,
            limit: 100,
        });
        t.withhold(ProviderError::Transport {
            message: "连接断了".into(),
        });

        assert_eq!(
            t.withheld(),
            Some(&ProviderError::ContextOverflow {
                used: 200,
                limit: 100
            }),
            "后续错误多半是同一根因的连锁反应，按它做决策会走错恢复路径"
        );
    }

    #[test]
    fn 重试前丢弃半截响应() {
        let mut t = TurnAccumulator::new();
        // 被 OutputLimit 截断：tool_use 的 input 都没拼完
        t.push(assistant(vec![AssistantContent::ToolUse {
            id: ToolUseId::from_raw("u1"),
            name: "Edit".into(),
            input: serde_json::json!({"path": "a"}),
        }]));
        t.withhold(ProviderError::OutputLimit);

        t.discard_for_retry();

        assert!(
            !t.has_tool_use(),
            "半截 tool_use 进了 transcript 会让下次请求 400"
        );
        assert!(
            t.withheld().is_none(),
            "重试前要清掉扣留标记，否则会重复恢复"
        );
        assert!(t.take_messages().is_empty());
    }
}
