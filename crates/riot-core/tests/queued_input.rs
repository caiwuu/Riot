//! 跑轮中插话（队列消息）的主循环语义。
//!
//! 语义按 Cursor：排队的消息等当前任务**完全跑完**才进对话 ——
//! - 工具轮**不注入**（哪怕工具结果已就位、对 API 是安全的）：排队面板
//!   里的消息中途蹦进对话是惊吓，插队只能由用户显式中断触发；
//! - 模型正常收尾（没有 tool_use）→ 收尾前 drain，非空就当新一轮继续，
//!   不加任何包装 —— 对模型来说这就是普通的下一句话；
//! - 队列注入照常计轮，max_turns 不因插话而失效。

use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt;
use riot_protocol::event::{AgentEvent, TerminalReason};
use riot_protocol::id::SessionId;
use riot_protocol::message::{Attachment, Message, UserContent};
use riot_protocol::provider::ProviderEvent;
use tokio_util::sync::CancellationToken;

use riot_core::state::AgentState;
use riot_core::testing::{
    FakeCompactor, ScriptedProvider, ScriptedQueue, ScriptedResult, ScriptedToolRunner,
    assistant_text, assistant_tool_use, mock_deps_with, user_text,
};

fn state() -> AgentState {
    AgentState::new(SessionId::from_raw("queued"), "test-model")
        .with_max_turns(8)
        .with_messages(vec![user_text("m_in", "干活")])
}

async fn collect(state: AgentState, deps: riot_core::state::AgentDeps) -> Vec<AgentEvent> {
    let s = riot_core::run_agent(state, deps, CancellationToken::new());
    futures::pin_mut!(s);
    let mut out = Vec::new();
    while let Some(ev) = s.next().await {
        out.push(ev);
    }
    out
}

fn done_reason(events: &[AgentEvent]) -> &TerminalReason {
    match events.last() {
        Some(AgentEvent::Done { reason }) => reason,
        other => panic!("最后一个事件该是 Done，得到 {other:?}"),
    }
}

/// 事件流里的用户消息（带文本的那些）。
fn user_messages(events: &[AgentEvent]) -> Vec<&Message> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Message(m @ Message::User { .. }) => Some(m),
            _ => None,
        })
        .collect()
}

fn has_reminder(m: &Message) -> bool {
    match m {
        Message::User { content, .. } => content.iter().any(|c| {
            matches!(
                c,
                UserContent::Attachment(Attachment::SystemReminder { .. })
            )
        }),
        _ => false,
    }
}

#[tokio::test]
async fn 模型收尾时的插话当新一轮继续跑() {
    // 第一响应没有 tool_use → 本该 Completed，但队列里有插话 → 继续；
    // 第二响应收尾时队列空 → Completed。
    let provider = Arc::new(ScriptedProvider::new(vec![
        vec![ProviderEvent::Message(assistant_text("m_a1", "做完了"))],
        vec![ProviderEvent::Message(assistant_text(
            "m_a2",
            "补充也做完了",
        ))],
    ]));
    let tools = Arc::new(ScriptedToolRunner::new(HashMap::new()));
    let mut deps = mock_deps_with(
        Arc::clone(&provider) as _,
        tools,
        Arc::new(FakeCompactor::default()),
    );
    let queue = Arc::new(ScriptedQueue::new(vec![vec![user_text(
        "m_q1",
        "顺便把测试也跑一下",
    )]]));
    deps.queue = Arc::clone(&queue) as _;

    let events = collect(state(), deps).await;

    assert!(
        matches!(done_reason(&events), TerminalReason::Completed),
        "插话处理完才收尾"
    );
    assert_eq!(provider.call_count(), 2, "插话该触发第二次请求");

    let users = user_messages(&events);
    assert_eq!(users.len(), 1, "事件流里该有那条插话");
    assert!(
        !has_reminder(users[0]),
        "收尾时注入等价于新一轮的普通输入，不该带途中插话提醒"
    );

    // 第二次请求必须带上插话 —— 注入进的是发给模型的历史，不只是事件流。
    let second = &provider.requests()[1];
    assert!(
        second.messages.iter().any(|m| m.id().as_str() == "m_q1"),
        "第二次请求的历史里没有插话"
    );
    assert!(riot_core::invariants::take_violations().is_empty());
}

#[tokio::test]
async fn 工具轮不注入插话_等任务跑完才发() {
    // 第一响应带 tool_use；插话在整个工具轮期间都不该进对话 ——
    // 直到第二响应（纯文本）收尾时才注入，然后第三响应答它。
    let provider = Arc::new(ScriptedProvider::new(vec![
        vec![ProviderEvent::Message(assistant_tool_use(
            "m_a1",
            "tu_1",
            "Read",
            serde_json::json!({}),
        ))],
        vec![ProviderEvent::Message(assistant_text("m_a2", "任务做完了"))],
        vec![ProviderEvent::Message(assistant_text("m_a3", "回答插话"))],
    ]));
    let tools = Arc::new(ScriptedToolRunner::new(HashMap::from([(
        "Read".to_owned(),
        ScriptedResult::Ok {
            text: "内容".into(),
        },
    )])));
    let mut deps = mock_deps_with(
        Arc::clone(&provider) as _,
        tools,
        Arc::new(FakeCompactor::default()),
    );
    let queue = Arc::new(ScriptedQueue::new(vec![vec![user_text(
        "m_q1",
        "顺便看下测试",
    )]]));
    deps.queue = Arc::clone(&queue) as _;

    let events = collect(state(), deps).await;

    assert!(matches!(done_reason(&events), TerminalReason::Completed));
    assert_eq!(provider.call_count(), 3, "工具轮 → 收尾注入 → 答插话");

    // 顺序不变量：插话必须出现在"任务做完了"（m_a2）**之后** ——
    // 早于它就是中途蹦进对话，正是这条语义要防的事。
    let msgs: Vec<&Message> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Message(m) => Some(m),
            _ => None,
        })
        .collect();
    let final_answer_at = msgs
        .iter()
        .position(|m| m.id().as_str() == "m_a2")
        .expect("该有任务收尾的回答");
    let queued_at = msgs
        .iter()
        .position(|m| m.id().as_str() == "m_q1")
        .expect("该有插话");
    assert!(
        queued_at > final_answer_at,
        "插话({queued_at})出现在任务收尾({final_answer_at})之前 —— 中途注入了"
    );
    assert!(
        !has_reminder(msgs[queued_at]),
        "收尾注入等价于新一轮普通输入，不带包装"
    );

    // 工具轮中途没有 drain：唯一一次拿到消息的 drain 发生在收尾。
    assert!(riot_core::invariants::take_violations().is_empty());
}

#[tokio::test]
async fn 插话不豁免轮数上限() {
    // max_turns = 1：唯一的一轮收尾时有插话，也只能停在 MaxTurns ——
    // 插话要是能无限续轮，失控的自动化就有了永动机。
    let provider = Arc::new(ScriptedProvider::new(vec![vec![ProviderEvent::Message(
        assistant_text("m_a1", "做完了"),
    )]]));
    let tools = Arc::new(ScriptedToolRunner::new(HashMap::new()));
    let mut deps = mock_deps_with(
        Arc::clone(&provider) as _,
        tools,
        Arc::new(FakeCompactor::default()),
    );
    deps.queue = Arc::new(ScriptedQueue::new(vec![vec![user_text("m_q1", "再来")]])) as _;

    let events = collect(
        AgentState::new(SessionId::from_raw("queued-max"), "test-model")
            .with_max_turns(1)
            .with_messages(vec![user_text("m_in", "干活")]),
        deps,
    )
    .await;

    assert!(
        matches!(done_reason(&events), TerminalReason::MaxTurns { limit: 1 }),
        "插话不该绕过 max_turns，实际 {:?}",
        done_reason(&events)
    );
    assert_eq!(provider.call_count(), 1, "到线之后不该再发请求");
    assert!(riot_core::invariants::take_violations().is_empty());
}

#[tokio::test]
async fn 队列空时行为与没有队列完全一致() {
    // 回归护栏：接入队列不能改变"没人插话"这条最常走的路。
    let provider = Arc::new(ScriptedProvider::new(vec![vec![ProviderEvent::Message(
        assistant_text("m_a1", "做完了"),
    )]]));
    let tools = Arc::new(ScriptedToolRunner::new(HashMap::new()));
    let mut deps = mock_deps_with(
        Arc::clone(&provider) as _,
        tools,
        Arc::new(FakeCompactor::default()),
    );
    let queue = Arc::new(ScriptedQueue::new(vec![]));
    deps.queue = Arc::clone(&queue) as _;

    let events = collect(state(), deps).await;

    assert!(matches!(done_reason(&events), TerminalReason::Completed));
    assert_eq!(provider.call_count(), 1);
    assert_eq!(queue.drain_count(), 1, "纯文本轮只有收尾前一个 drain 点");
    assert!(user_messages(&events).is_empty());
}
