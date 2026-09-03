//! 跑轮中插话（队列消息）的主循环语义。
//!
//! 语义按 Cursor：排队的消息等当前任务**完全跑完**才进对话 ——
//! - 工具轮**不注入**（哪怕工具结果已就位、对 API 是安全的）：排队面板
//!   里的消息中途蹦进对话是惊吓，插队只能由用户显式中断触发；
//! - 模型正常收尾（没有 tool_use）→ 收尾前 drain，非空就当新一轮继续，
//!   不加任何包装 —— 对模型来说这就是普通的下一句话；
//! - 队列注入照常计轮，max_turns 不因插话而失效。
//!
//! **带外消息走另一条通道，时机相反**：界面按钮的提醒（「转到后台」）和
//! 后台子 agent 的完成通知说的都是"你手上这件事"，必须在下一批工具结果
//! 就位时就注入，等整轮跑完等于按钮没生效。见文件末尾那两条。

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

/// 带外消息的形状：synthetic 的 user 消息 + SystemReminder 附件 ——
/// 内核的 `Session::nudge` 和后台任务完成通知都长这样。
fn reminder(msg_id: &str, text: &str) -> Message {
    Message::User {
        id: riot_protocol::id::MessageId::from_raw(msg_id),
        content: vec![UserContent::Attachment(Attachment::SystemReminder {
            text: text.into(),
        })],
        meta: riot_protocol::message::MessageMeta {
            synthetic: true,
            ..Default::default()
        },
    }
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
    assert_eq!(
        queue.out_of_band_drain_count(),
        0,
        "没跑工具就没有工具轮边界，带外通道不该被碰"
    );
    assert!(user_messages(&events).is_empty());
}

#[tokio::test]
async fn 带外提醒在工具结果就位时就注入() {
    // 这是「转到后台」的整条路径。用户在模型跑工具的时候点按钮，提醒必须
    // 赶上**下一次**请求 —— 不然模型把手头的活从头做到尾、写完总结，收尾
    // 才读到"请转到后台"，然后去分叉一个做已经做完的活的子 agent。
    let provider = Arc::new(ScriptedProvider::new(vec![
        vec![ProviderEvent::Message(assistant_tool_use(
            "m_a1",
            "tu_1",
            "Read",
            serde_json::json!({}),
        ))],
        vec![ProviderEvent::Message(assistant_text(
            "m_a2",
            "已分叉到后台",
        ))],
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
    let queue = Arc::new(
        ScriptedQueue::new(vec![])
            .with_out_of_band(vec![vec![reminder("m_oob1", "用户点了「转到后台」。")]]),
    );
    deps.queue = Arc::clone(&queue) as _;

    let events = collect(state(), deps).await;

    assert!(matches!(done_reason(&events), TerminalReason::Completed));
    assert_eq!(provider.call_count(), 2);
    assert_eq!(queue.out_of_band_drain_count(), 1, "工具轮边界该取一次");

    // 这条断言就是这个 bug 的回归护栏：提醒要在**第二次**请求的历史里，
    // 而不是等到收尾之后的第三次。
    let second = &provider.requests()[1];
    assert!(
        second.messages.iter().any(|m| m.id().as_str() == "m_oob1"),
        "第二次请求还看不到带外提醒 —— 它被推到整轮跑完之后了"
    );

    let msgs: Vec<&Message> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Message(m) => Some(m),
            _ => None,
        })
        .collect();
    let oob_at = msgs
        .iter()
        .position(|m| m.id().as_str() == "m_oob1")
        .expect("事件流里该有带外提醒");
    let tool_result_at = msgs
        .iter()
        .position(|m| match m {
            Message::User { content, .. } => content
                .iter()
                .any(|c| matches!(c, UserContent::ToolResult { .. })),
            _ => false,
        })
        .expect("该有工具结果");
    assert!(
        oob_at > tool_result_at,
        "带外提醒({oob_at})插到了工具结果({tool_result_at})之前 —— 会夹进 tool_use/tool_result"
    );
    // INV-2（消息序列）由主循环下一圈的组装检查抓，这里确认它没告警。
    assert!(riot_core::invariants::take_violations().is_empty());
}

#[tokio::test]
async fn 带外通道和用户插话互不干扰() {
    // 两条通道同时有东西：提醒赶上工具轮边界，插话仍然等收尾。搞混了的
    // 表现是插话中途蹦出来（惊吓）或者提醒等到最后（失效）。
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
    deps.queue = Arc::new(
        ScriptedQueue::new(vec![vec![user_text("m_q1", "顺便看下测试")]])
            .with_out_of_band(vec![vec![reminder("m_oob1", "用户点了「转到后台」。")]]),
    ) as _;

    let events = collect(state(), deps).await;

    assert!(matches!(done_reason(&events), TerminalReason::Completed));
    let msgs: Vec<&Message> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Message(m) => Some(m),
            _ => None,
        })
        .collect();
    let at = |id: &str| {
        msgs.iter()
            .position(|m| m.id().as_str() == id)
            .unwrap_or_else(|| panic!("事件流里没有 {id}"))
    };
    assert!(
        at("m_oob1") < at("m_a2"),
        "带外提醒该赶在这一批工具之后、模型下次开口之前"
    );
    assert!(
        at("m_q1") > at("m_a2"),
        "用户插话仍然要等任务收尾，不能跟着带外提醒一起中途注入"
    );
    assert!(riot_core::invariants::take_violations().is_empty());
}
