//! 给助手消息盖「这条是哪个模型答的」（`MessageMeta::model_origin`）。
//!
//! `[约束]` 用 provider **实际发出去的** model 名（含过载降级换成的那个），
//! 不用服务端回显的。回显名经常和配置名对不上：OpenAI 把 `gpt-5` 回成
//! `gpt-5-2025-08-07`，Anthropic 4.6 之前的别名 `claude-sonnet-4-5` 回成带
//! 日期的快照，网关还会改写成上游名。而 INV-9 / `strip_foreign_thinking_
//! signatures` 拿 `model_origin` 和会话当前模型名**按字符串相等**比 —— 对不上
//! 就判成"外模型"，把自己的 thinking signature 每轮剥掉：Anthropic 侧思考退化
//! 成 text、缓存全 miss、开思考的工具续轮直接被拒；Responses 侧加密推理从不
//! 回传。用发出去的名字，两边比的才是同一个东西。

use riot_protocol::message::Message;
use riot_protocol::provider::ProviderEvent;

/// 助手消息盖上 `model`；别的事件原样返回。
pub(crate) fn stamp_model_origin(ev: ProviderEvent, model: &str) -> ProviderEvent {
    match ev {
        ProviderEvent::Message(Message::Assistant {
            id,
            content,
            usage,
            mut meta,
        }) => {
            meta.model_origin = Some(model.to_owned());
            ProviderEvent::Message(Message::Assistant {
                id,
                content,
                usage,
                meta,
            })
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use riot_protocol::id::MessageId;
    use riot_protocol::message::{AssistantContent, MessageMeta};

    use super::*;

    #[test]
    fn 服务端回显的名字被发出去的名字盖掉() {
        let ev = ProviderEvent::Message(Message::Assistant {
            id: MessageId::from_raw("a"),
            content: vec![AssistantContent::Text { text: "hi".into() }],
            usage: None,
            meta: MessageMeta {
                model_origin: Some("gpt-5-2025-08-07".into()),
                ..Default::default()
            },
        });
        match stamp_model_origin(ev, "gpt-5") {
            ProviderEvent::Message(Message::Assistant { meta, .. }) => {
                assert_eq!(meta.model_origin.as_deref(), Some("gpt-5"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn 非助手消息原样放过() {
        let ev = ProviderEvent::Usage(Default::default());
        assert!(matches!(
            stamp_model_origin(ev, "m"),
            ProviderEvent::Usage(_)
        ));
    }
}
