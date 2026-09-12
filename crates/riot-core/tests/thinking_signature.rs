//! 换模型后续跑：必须剥掉上一模型的 thinking signature（INV-9）。
//!
//! 用户额度不够换模型再点继续，历史里还留着旧签名。不剥的话 debug
//! 会 invariant panic，release 会把签名发给新模型被 API 400。

use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt;
use riot_protocol::event::{AgentEvent, TerminalReason};
use riot_protocol::id::{MessageId, SessionId};
use riot_protocol::message::{AssistantContent, Message, MessageMeta};
use riot_protocol::provider::ProviderEvent;
use tokio_util::sync::CancellationToken;

use riot_core::state::AgentState;
use riot_core::testing::{
    ScriptedProvider, ScriptedToolRunner, assistant_text, mock_deps, user_text,
};

#[tokio::test]
async fn 换模型后续跑不因旧_thinking_签名而中断() {
    let history = vec![
        user_text("u1", "写个东西"),
        Message::Assistant {
            id: MessageId::from_raw("a1"),
            content: vec![
                AssistantContent::Thinking {
                    text: "先想想".into(),
                    signature: Some("claude-sig".into()),
                },
                AssistantContent::Text {
                    text: "好的，马上生成。".into(),
                },
            ],
            usage: None,
            meta: MessageMeta {
                model_origin: Some("anthropic/claude-fable-5.1".into()),
                ..Default::default()
            },
        },
        user_text("u2", "继续"),
    ];

    let provider = Arc::new(ScriptedProvider::new(vec![vec![ProviderEvent::Message(
        assistant_text("a2", "接着做"),
    )]]));
    let tools = Arc::new(ScriptedToolRunner::new(HashMap::new()));
    let deps = mock_deps(Arc::clone(&provider), tools);
    let state = AgentState::new(SessionId::from_raw("s"), "grok-4.6")
        .with_max_turns(4)
        .with_messages(history);

    let stream = riot_core::run_agent(state, deps, CancellationToken::new());
    futures::pin_mut!(stream);
    let mut events = Vec::new();
    while let Some(ev) = stream.next().await {
        events.push(ev);
    }

    match events.last() {
        Some(AgentEvent::Done {
            reason: TerminalReason::Completed,
        }) => {}
        other => panic!("换模型后续跑应当正常结束，得到 {other:?}"),
    }

    let req = &provider.requests()[0];
    assert_eq!(req.model, "grok-4.6");
    let still_signed = req.messages.iter().any(|m| match m {
        Message::Assistant { content, .. } => content.iter().any(|c| {
            matches!(
                c,
                AssistantContent::Thinking {
                    signature: Some(_),
                    ..
                }
            )
        }),
        _ => false,
    });
    assert!(
        !still_signed,
        "发给新模型的请求不得携带旧 thinking signature"
    );
}
