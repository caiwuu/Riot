//! 真实调度器接进主循环的端到端验证（L5）。
//!
//! 主循环的黄金回放用 `ScriptedToolRunner` —— 它按名字查表返回预设结果，
//! 不分批、不并发、不级联。那套测试证明"主循环怎么用工具结果"，
//! 证明不了"真实调度器产出的结果主循环接不接得住"。
//!
//! 这里把 [`Scheduler`] 塞进 `AgentDeps`，跑完整的多轮对话。
//!
//! # 关注点只有一个
//!
//! **`tool_use` / `tool_result` 配对。**它是整个系统里最脆弱的不变量：
//! 断了之后下一次 API 请求直接 400，而错误信息不会告诉你是哪个 id 缺了。
//! 调度器有一堆能让它断掉的路径 —— 分批、级联、中断、panic、未注册工具。
//! 每一条都在下面。

use std::sync::Arc;

use futures::StreamExt;
use riot_core::agent_loop::run_agent;
use riot_core::state::{AgentDeps, AgentState};
use riot_core::testing::{FakeCompactor, MockClock, ScriptedProvider, SeqIdGenerator};
use riot_protocol::event::AgentEvent;
use riot_protocol::id::{MessageId, SessionId};
use riot_protocol::message::{
    AssistantContent, Message, MessageMeta, ToolResultContent, UserContent,
};
use riot_protocol::provider::ProviderEvent;
use riot_protocol::runner::ToolRunner;
use riot_protocol::tool::Tool;
use riot_tools::testing::{FakeTool, test_scheduler};
use tokio_util::sync::CancellationToken;

/// 一条包含多个 tool_use 的助手消息。
fn assistant_tools(msg_id: &str, calls: &[(&str, &str)]) -> Message {
    Message::Assistant {
        id: MessageId::from_raw(msg_id),
        content: calls
            .iter()
            .map(|(id, name)| AssistantContent::ToolUse {
                id: riot_protocol::id::ToolUseId::from_raw(*id),
                name: (*name).into(),
                input: serde_json::json!({}),
            })
            .collect(),
        usage: None,
        meta: MessageMeta::default(),
    }
}

fn assistant_text(msg_id: &str, text: &str) -> Message {
    Message::Assistant {
        id: MessageId::from_raw(msg_id),
        content: vec![AssistantContent::Text { text: text.into() }],
        usage: None,
        meta: MessageMeta::default(),
    }
}

fn deps(tools: Vec<Arc<dyn Tool>>, responses: Vec<Vec<ProviderEvent>>) -> AgentDeps {
    AgentDeps {
        provider: Arc::new(ScriptedProvider::new(responses)),
        compactor: Arc::new(FakeCompactor::new(2)),
        clock: Arc::new(MockClock::new(0)),
        ids: Arc::new(SeqIdGenerator::default()),
        tools: Arc::new(test_scheduler(tools)) as Arc<dyn ToolRunner>,
    }
}

fn initial() -> AgentState {
    AgentState {
        session_id: SessionId::from_raw("sess_tools"),
        messages: vec![Message::User {
            id: MessageId::from_raw("m_user"),
            content: vec![UserContent::Text {
                text: "干活".into(),
            }],
            meta: MessageMeta::default(),
        }],
        model: "claude-x".into(),
        system: "你是助手".into(),
        turn: 0,
        max_turns: 10,
        output_limit_recovery_count: 0,
        attempted_reactive_compact: false,
        compact_failure_streak: 0,
        max_output_tokens_override: None,
        transition: None,
    }
}

async fn run(
    tools: Vec<Arc<dyn Tool>>,
    responses: Vec<Vec<ProviderEvent>>,
    cancel: CancellationToken,
) -> Vec<AgentEvent> {
    run_agent(initial(), deps(tools, responses), cancel)
        .collect()
        .await
}

/// 把事件流里的 tool_use id 和 tool_result id 抽出来。
///
/// 这两个序列必须完全相等 —— 顺序也要一样。
fn pairing(events: &[AgentEvent]) -> (Vec<String>, Vec<String>) {
    let mut issued = Vec::new();
    let mut resolved = Vec::new();

    for e in events {
        let AgentEvent::Message(msg) = e else {
            continue;
        };
        match msg {
            Message::Assistant { content, .. } => {
                for c in content {
                    if let AssistantContent::ToolUse { id, .. } = c {
                        issued.push(id.as_str().to_owned());
                    }
                }
            }
            Message::User { content, .. } => {
                for c in content {
                    if let UserContent::ToolResult { tool_use_id, .. } = c {
                        resolved.push(tool_use_id.as_str().to_owned());
                    }
                }
            }
            Message::System { .. } => {}
        }
    }

    (issued, resolved)
}

fn result_texts(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Message(Message::User { content, .. }) => Some(content),
            _ => None,
        })
        .flatten()
        .filter_map(|c| match c {
            UserContent::ToolResult { content, .. } => match content {
                ToolResultContent::Text { text } => Some(text.clone()),
                other => Some(format!("{other:?}")),
            },
            _ => None,
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn 混合读写的多批次配对成立() {
    // [read, read, edit, read] 会被切成三批。跨批次的结果顺序和配对
    // 都要成立 —— 这是 ScriptedToolRunner 完全测不到的路径。
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(FakeTool::read_only("Read")),
        Arc::new(FakeTool::writer("Edit")),
    ];

    let events = run(
        tools,
        vec![
            vec![ProviderEvent::Message(assistant_tools(
                "msg_1",
                &[
                    ("t1", "Read"),
                    ("t2", "Read"),
                    ("t3", "Edit"),
                    ("t4", "Read"),
                ],
            ))],
            vec![ProviderEvent::Message(assistant_text("msg_2", "干完了"))],
        ],
        CancellationToken::new(),
    )
    .await;

    let (issued, resolved) = pairing(&events);
    assert_eq!(issued, vec!["t1", "t2", "t3", "t4"]);
    assert_eq!(
        resolved, issued,
        "跨批次的结果顺序必须与调用顺序一致，缺一个下次请求就是 400"
    );
    assert!(matches!(events.last(), Some(AgentEvent::Done { .. })));
}

#[tokio::test(start_paused = true)]
async fn 慢工具不会打乱结果顺序() {
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(FakeTool::read_only("Slow").slow(800)),
        Arc::new(FakeTool::read_only("Fast")),
    ];

    let events = run(
        tools,
        vec![
            vec![ProviderEvent::Message(assistant_tools(
                "msg_1",
                &[("t1", "Slow"), ("t2", "Fast"), ("t3", "Slow")],
            ))],
            vec![ProviderEvent::Message(assistant_text("msg_2", "好了"))],
        ],
        CancellationToken::new(),
    )
    .await;

    let (issued, resolved) = pairing(&events);
    assert_eq!(
        resolved, issued,
        "完成顺序是 t2 t1 t3，输出顺序必须是 t1 t2 t3"
    );
}

#[tokio::test(start_paused = true)]
async fn 级联取消后配对仍然成立() {
    // 级联是最容易漏结果的路径：被跳过的工具没有执行，
    // 很容易就忘了给它补一条 tool_result。
    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(FakeTool::read_only("Bash").failing("命令失败").cascading()),
        Arc::new(FakeTool::read_only("Hang").hanging()),
    ];

    let events = run(
        tools,
        vec![
            vec![ProviderEvent::Message(assistant_tools(
                "msg_1",
                &[("t1", "Bash"), ("t2", "Hang"), ("t3", "Hang")],
            ))],
            vec![ProviderEvent::Message(assistant_text("msg_2", "收工"))],
        ],
        CancellationToken::new(),
    )
    .await;

    let (issued, resolved) = pairing(&events);
    assert_eq!(issued.len(), 3);
    assert_eq!(resolved, issued, "被级联跳过的工具也要有结果");

    let texts = result_texts(&events);
    assert!(texts[1].contains("跳过"), "{}", texts[1]);
    assert!(matches!(events.last(), Some(AgentEvent::Done { .. })));
}

#[tokio::test(start_paused = true)]
async fn 未注册的工具不会让配对断掉() {
    // 模型偶尔会调用不存在的工具（记错名字、跨版本 transcript）。
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(FakeTool::read_only("Read"))];

    let events = run(
        tools,
        vec![
            vec![ProviderEvent::Message(assistant_tools(
                "msg_1",
                &[("t1", "Read"), ("t2", "Reed"), ("t3", "Read")],
            ))],
            vec![ProviderEvent::Message(assistant_text("msg_2", "改好了"))],
        ],
        CancellationToken::new(),
    )
    .await;

    let (issued, resolved) = pairing(&events);
    assert_eq!(resolved, issued);

    let texts = result_texts(&events);
    assert!(
        texts[1].contains("Read"),
        "要告诉模型有哪些工具可用它才能改：{}",
        texts[1]
    );
}

#[tokio::test(start_paused = true)]
async fn 工具_panic_不会让配对断掉() {
    struct Exploding;

    #[async_trait::async_trait]
    impl Tool for Exploding {
        fn name(&self) -> &'static str {
            "Boom"
        }
        fn input_schema(&self) -> schemars::Schema {
            schemars::json_schema!({ "type": "object" })
        }
        fn prompt(&self, _: &riot_protocol::tool::PromptContext) -> String {
            "boom".into()
        }
        fn describe(&self, _: &serde_json::Value) -> String {
            "boom".into()
        }
        async fn call(
            &self,
            _: serde_json::Value,
            _: riot_protocol::tool::ToolContext,
        ) -> riot_protocol::tool::ToolOutcome {
            panic!("工具炸了");
        }
        fn is_concurrency_safe(&self, _: &serde_json::Value) -> bool {
            true
        }
    }

    let tools: Vec<Arc<dyn Tool>> =
        vec![Arc::new(Exploding), Arc::new(FakeTool::read_only("Read"))];

    let events = run(
        tools,
        vec![
            vec![ProviderEvent::Message(assistant_tools(
                "msg_1",
                &[("t1", "Boom"), ("t2", "Read")],
            ))],
            vec![ProviderEvent::Message(assistant_text("msg_2", "继续"))],
        ],
        CancellationToken::new(),
    )
    .await;

    let (issued, resolved) = pairing(&events);
    assert_eq!(
        resolved, issued,
        "panic 让 tool_use 没有结果的话，下次请求直接 400"
    );
    assert!(matches!(events.last(), Some(AgentEvent::Done { .. })));
}

#[tokio::test(start_paused = true)]
async fn 中断时所有工具都有结果() {
    let cancel = CancellationToken::new();
    cancel.cancel();

    let tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(FakeTool::read_only("Read")),
        Arc::new(FakeTool::writer("Edit")),
    ];

    let events = run(
        tools,
        vec![vec![ProviderEvent::Message(assistant_tools(
            "msg_1",
            &[("t1", "Read"), ("t2", "Edit")],
        ))]],
        cancel,
    )
    .await;

    assert!(
        matches!(events.last(), Some(AgentEvent::Done { .. })),
        "中断也要以 Done 收尾（INV-4）"
    );
}

#[tokio::test(start_paused = true)]
async fn 大批量调用被正确切分且不丢结果() {
    // 25 个只读工具 → 10 + 10 + 5 三批
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(FakeTool::read_only("Read"))];

    let calls: Vec<(String, &str)> = (0..25).map(|i| (format!("t{i}"), "Read")).collect();
    let call_refs: Vec<(&str, &str)> = calls.iter().map(|(id, n)| (id.as_str(), *n)).collect();

    let events = run(
        tools,
        vec![
            vec![ProviderEvent::Message(assistant_tools("msg_1", &call_refs))],
            vec![ProviderEvent::Message(assistant_text("msg_2", "全读完了"))],
        ],
        CancellationToken::new(),
    )
    .await;

    let (issued, resolved) = pairing(&events);
    assert_eq!(issued.len(), 25);
    assert_eq!(resolved, issued, "分批不能丢结果，也不能打乱顺序");
}

#[tokio::test(start_paused = true)]
async fn 进度事件透传到主循环() {
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(
        FakeTool::read_only("Bash").with_progress(&["编译中", "链接中"]),
    )];

    let events = run(
        tools,
        vec![
            vec![ProviderEvent::Message(assistant_tools(
                "msg_1",
                &[("t1", "Bash")],
            ))],
            vec![ProviderEvent::Message(assistant_text("msg_2", "构建完成"))],
        ],
        CancellationToken::new(),
    )
    .await;

    let progress = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::Progress { .. }))
        .count();
    assert_eq!(progress, 2, "Bash 的实时输出要能到 UI");
}
