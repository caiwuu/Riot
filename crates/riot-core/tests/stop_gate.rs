//! 收尾闸（Stop hooks）的主循环语义。
//!
//! - 模型正常收尾（没有 tool_use）前问一声；Block 就注入反馈强制再跑一轮，
//!   反馈两条：System 给用户解释（不进模型）、SystemReminder 给模型整改；
//! - 只在正常收尾问 —— 错误/中断路径不问（INV-6 同源：错误消息上跑 stop
//!   hook 是死循环的起点）；
//! - 连续 Block 有硬熔断（MAX_STOP_HOOK_BLOCKS），超了以 StopHookPrevented
//!   终止而不是无限烧 API；
//! - Block 优先于插话队列：活没干完不开始处理插话。

use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt;
use riot_protocol::event::{AgentEvent, TerminalReason};
use riot_protocol::id::SessionId;
use riot_protocol::message::{Attachment, Message, UserContent};
use riot_protocol::provider::ProviderEvent;
use tokio_util::sync::CancellationToken;

use riot_core::state::{AgentState, StopDecision};
use riot_core::testing::{
    FakeCompactor, ScriptedProvider, ScriptedQueue, ScriptedStopGate, ScriptedToolRunner,
    assistant_text, mock_deps_with, user_text,
};

fn state() -> AgentState {
    AgentState::new(SessionId::from_raw("stop"), "test-model")
        .with_max_turns(20)
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

fn text_responses(n: usize) -> Vec<Vec<ProviderEvent>> {
    (0..n)
        .map(|i| {
            vec![ProviderEvent::Message(assistant_text(
                &format!("m_a{i}"),
                "做完了",
            ))]
        })
        .collect()
}

#[tokio::test]
async fn 被拦一次后修完放行() {
    let provider = Arc::new(ScriptedProvider::new(text_responses(2)));
    let tools = Arc::new(ScriptedToolRunner::new(HashMap::new()));
    let mut deps = mock_deps_with(
        Arc::clone(&provider) as _,
        tools,
        Arc::new(FakeCompactor::default()),
    );
    let gate = Arc::new(ScriptedStopGate::new(vec![StopDecision::Block {
        reason: "测试还没跑".into(),
    }]));
    deps.stop_gate = Arc::clone(&gate) as _;

    let events = collect(state(), deps).await;

    assert!(matches!(done_reason(&events), TerminalReason::Completed));
    assert_eq!(provider.call_count(), 2, "被拦后该强制再跑一轮");
    assert_eq!(gate.seen(), vec![0, 1], "blocks_so_far 该逐次递增地透传");

    // 反馈两条两个读者：System 给用户看，SystemReminder 给模型看。
    let has_notice = events.iter().any(|e| {
        matches!(e, AgentEvent::Message(Message::System { text, .. }) if text.contains("测试还没跑"))
    });
    assert!(has_notice, "用户该看到一条'为什么没停'的说明");
    let feedback = events.iter().find_map(|e| match e {
        AgentEvent::Message(m @ Message::User { content, .. })
            if content.iter().any(|c| {
                matches!(
                    c,
                    UserContent::Attachment(Attachment::SystemReminder { text })
                        if text.contains("测试还没跑")
                )
            }) =>
        {
            Some(m)
        }
        _ => None,
    });
    let feedback = feedback.expect("模型该收到 system-reminder 形式的整改要求");
    match feedback {
        Message::User { meta, .. } => {
            assert!(meta.synthetic, "hook 反馈是系统合成，不是用户说的话")
        }
        _ => unreachable!(),
    }
    assert!(riot_core::invariants::take_violations().is_empty());
}

#[tokio::test]
async fn 连续拦截触发熔断() {
    // 永远 Block 的 hook（判据坏了）。熔断在 MAX_STOP_HOOK_BLOCKS=5 之后，
    // 以 StopHookPrevented 终止 —— 不能让它无限烧 API。
    let provider = Arc::new(ScriptedProvider::new(text_responses(8)));
    let tools = Arc::new(ScriptedToolRunner::new(HashMap::new()));
    let mut deps = mock_deps_with(
        Arc::clone(&provider) as _,
        tools,
        Arc::new(FakeCompactor::default()),
    );
    let gate = Arc::new(ScriptedStopGate::new(
        (0..8)
            .map(|_| StopDecision::Block {
                reason: "永远不满意".into(),
            })
            .collect(),
    ));
    deps.stop_gate = Arc::clone(&gate) as _;

    let events = collect(state(), deps).await;

    match done_reason(&events) {
        TerminalReason::StopHookPrevented { message } => {
            assert!(message.contains("永远不满意"), "熔断时该带上 hook 的理由");
        }
        other => panic!("该以 StopHookPrevented 熔断，得到 {other:?}"),
    }
    // 初始 1 次 + 5 次被拦重跑 = 6 次请求；第 6 次收尾时熔断，不再发第 7 次。
    assert_eq!(provider.call_count(), 6, "熔断后不该再发请求");
    assert!(riot_core::invariants::take_violations().is_empty());
}

#[tokio::test]
async fn 拦截优先于插话队列() {
    // 活没干完（hook 拦着）就不处理插话 —— 插话等真正收尾。
    let provider = Arc::new(ScriptedProvider::new(text_responses(3)));
    let tools = Arc::new(ScriptedToolRunner::new(HashMap::new()));
    let mut deps = mock_deps_with(
        Arc::clone(&provider) as _,
        tools,
        Arc::new(FakeCompactor::default()),
    );
    let gate = Arc::new(ScriptedStopGate::new(vec![StopDecision::Block {
        reason: "先把活干完".into(),
    }]));
    deps.stop_gate = Arc::clone(&gate) as _;
    let queue = Arc::new(ScriptedQueue::new(vec![vec![user_text("m_q1", "插一句")]]));
    deps.queue = Arc::clone(&queue) as _;

    let events = collect(state(), deps).await;

    assert!(matches!(done_reason(&events), TerminalReason::Completed));
    // 第 1 收尾被拦（不 drain）→ 第 2 收尾放行 → drain 注入插话 → 第 3 答完。
    assert_eq!(provider.call_count(), 3);
    let msgs: Vec<&Message> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Message(m) => Some(m),
            _ => None,
        })
        .collect();
    let feedback_at = msgs
        .iter()
        .position(|m| matches!(m, Message::User { meta, .. } if meta.synthetic))
        .expect("该有 hook 反馈");
    let queued_at = msgs
        .iter()
        .position(|m| m.id().as_str() == "m_q1")
        .expect("该有插话");
    assert!(
        feedback_at < queued_at,
        "hook 反馈({feedback_at})该先于插话({queued_at}) —— 活没干完不处理插话"
    );
    assert!(riot_core::invariants::take_violations().is_empty());
}
