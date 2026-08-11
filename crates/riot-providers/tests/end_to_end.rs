//! 真实 Provider 接进主循环的端到端验证。
//!
//! # 这个测试文件存在的理由
//!
//! 主循环的黄金回放用 `ScriptedProvider` —— 它直接吐 `ProviderEvent`，
//! 跳过了 SSE 解析和解码。那套测试证明的是「主循环的状态机对不对」，
//! 证明不了「真实 provider 产出的东西主循环接不接得住」。
//!
//! 这里补上那一段：从**原始 SSE 字节**出发，走完整条链路 ——
//!
//! ```text
//! 字节流 → SseParser → StreamDecoder → AnthropicProvider → run_agent → AgentEvent
//! ```
//!
//! 两层之间的契约不匹配（比如 provider 吐了主循环不认的事件顺序，
//! 或者 tool_use 的 id 在转换中丢了）只有在这里才会暴露。
//!
//! 分片故意切得很碎，让每次运行都顺带压一遍 SSE 解析器的边界处理。

use std::sync::Arc;

use futures::StreamExt;
use riot_core::agent_loop::run_agent;
use riot_core::state::{AgentDeps, AgentState};
use riot_core::testing::{
    FakeCompactor, MockClock, ScriptedResult, ScriptedToolRunner, SeqIdGenerator,
};
use riot_protocol::event::AgentEvent;
use riot_protocol::id::MessageId;
use riot_protocol::id::SessionId;
use riot_protocol::message::{Message, MessageMeta, UserContent};
use riot_providers::anthropic::{AnthropicConfig, AnthropicProvider, SystemSection};
use riot_providers::transport::{HttpError, HttpTransport, ScriptedResponse, ScriptedTransport};
use riot_providers::watchdog::TokioClock;
use tokio_util::sync::CancellationToken;

/// 把 SSE 文本切成很碎的字节分片。
///
/// 7 字节是刻意选的质数：它跟事件里任何字段的长度都不对齐，所以每一帧的
/// 边界都会落在不同的相对位置上 —— 包括中文字符的中间。真实 TCP 分片就是
/// 这样，而这里的中文内容能保证每次运行都压到那条路径。
fn shred(sse: &str) -> Vec<Vec<u8>> {
    sse.as_bytes().chunks(7).map(<[u8]>::to_vec).collect()
}

fn frame(json: &str) -> String {
    format!("data: {json}\n\n")
}

/// 一段纯文本响应。
fn text_response(msg_id: &str, text: &str) -> String {
    [
        frame(&format!(
            r#"{{"type":"message_start","message":{{"id":"{msg_id}","model":"claude-x","usage":{{"input_tokens":100,"output_tokens":1}}}}}}"#
        )),
        frame(r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#),
        frame(&format!(
            r#"{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"{text}"}}}}"#
        )),
        frame(r#"{"type":"content_block_stop","index":0}"#),
        frame(r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":50}}"#),
        frame(r#"{"type":"message_stop"}"#),
    ]
    .concat()
}

/// 一段带工具调用的响应。参数被切成多个 delta，压 O(n²) 那条路径。
fn tool_response(msg_id: &str, tool_id: &str, tool: &str, args: &str) -> String {
    let mut parts = vec![
        frame(&format!(
            r#"{{"type":"message_start","message":{{"id":"{msg_id}","model":"claude-x","usage":{{"input_tokens":100,"output_tokens":1}}}}}}"#
        )),
        frame(&format!(
            r#"{{"type":"content_block_start","index":0,"content_block":{{"type":"tool_use","id":"{tool_id}","name":"{tool}"}}}}"#
        )),
    ];

    // 参数按 3 字符切开 —— 单独每片都不是合法 JSON
    for piece in args
        .as_bytes()
        .chunks(3)
        .map(|c| String::from_utf8_lossy(c).into_owned())
    {
        let escaped = serde_json::to_string(&piece).expect("字符串可序列化");
        parts.push(frame(&format!(
            r#"{{"type":"content_block_delta","index":0,"delta":{{"type":"input_json_delta","partial_json":{escaped}}}}}"#
        )));
    }

    parts.push(frame(r#"{"type":"content_block_stop","index":0}"#));
    parts.push(frame(r#"{"type":"message_stop"}"#));
    parts.concat()
}

/// 构造工具结果表。
fn tool_results(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, ScriptedResult> {
    pairs
        .iter()
        .map(|(id, text)| {
            (
                (*id).to_owned(),
                ScriptedResult::Ok {
                    text: (*text).to_owned(),
                },
            )
        })
        .collect()
}

fn deps(transport: Arc<ScriptedTransport>, tools: ScriptedToolRunner) -> AgentDeps {
    let provider = AnthropicProvider::new(
        transport as Arc<dyn HttpTransport>,
        Arc::new(TokioClock),
        vec![SystemSection::stable("intro", "你是助手")],
        AnthropicConfig::default(),
    );

    AgentDeps {
        provider: Arc::new(provider),
        compactor: Arc::new(FakeCompactor::new(2)),
        clock: Arc::new(MockClock::new(0)),
        ids: Arc::new(SeqIdGenerator::default()),
        tools: Arc::new(tools),
    }
}

fn initial_state(prompt: &str) -> AgentState {
    AgentState {
        session_id: SessionId::from_raw("sess_e2e"),
        messages: vec![Message::User {
            id: MessageId::from_raw("m_user"),
            content: vec![UserContent::Text {
                text: prompt.into(),
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

async fn run(transport: Arc<ScriptedTransport>, tools: ScriptedToolRunner) -> Vec<AgentEvent> {
    run_agent(
        initial_state("帮我看看 a.rs"),
        deps(transport, tools),
        CancellationToken::new(),
    )
    .collect()
    .await
}

/// 只留下持久事件，扔掉 Delta / Progress。
fn durable(events: &[AgentEvent]) -> Vec<&AgentEvent> {
    events
        .iter()
        .filter(|e| !matches!(e, AgentEvent::Delta { .. } | AgentEvent::Progress { .. }))
        .collect()
}

#[tokio::test(start_paused = true)]
async fn 纯文本响应走完整条链路() {
    let t = Arc::new(ScriptedTransport::new(vec![ScriptedResponse::Chunks(
        shred(&text_response("msg_1", "看完了")),
    )]));

    let events = run(t, ScriptedToolRunner::new(tool_results(&[]))).await;

    assert!(
        matches!(events.last(), Some(AgentEvent::Done { .. })),
        "流必须以 Done 收尾（INV-4）"
    );

    let text: String = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Message(Message::Assistant { content, .. }) => Some(content),
            _ => None,
        })
        .flatten()
        .filter_map(|c| match c {
            riot_protocol::message::AssistantContent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(text, "看完了", "碎片化的 SSE 要能还原成完整文本");
}

#[tokio::test(start_paused = true)]
async fn 工具调用的参数在碎片化传输后仍然完整() {
    // 参数被切成 3 字符一片，单独每片都不是合法 JSON。
    // 这正是「只在 content_block_stop 时 parse 一次」要保证的场景。
    let args = r#"{"path":"src/main.rs","limit":100}"#;

    let t = Arc::new(ScriptedTransport::new(vec![
        ScriptedResponse::Chunks(shred(&tool_response("msg_1", "toolu_1", "Read", args))),
        ScriptedResponse::Chunks(shred(&text_response("msg_2", "读完了"))),
    ]));

    let tools = ScriptedToolRunner::new(tool_results(&[("toolu_1", "文件内容")]));
    let events = run(t, tools).await;

    // 找到主循环发出的工具调用
    let tool_uses: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Message(Message::Assistant { content, .. }) => Some(content),
            _ => None,
        })
        .flatten()
        .filter_map(|c| match c {
            riot_protocol::message::AssistantContent::ToolUse { name, input, .. } => {
                Some((name.as_str(), input))
            }
            _ => None,
        })
        .collect();

    assert_eq!(tool_uses.len(), 1);
    assert_eq!(tool_uses[0].0, "Read");
    assert_eq!(
        tool_uses[0].1,
        &serde_json::json!({ "path": "src/main.rs", "limit": 100 }),
        "碎片化的 partial_json 要能拼回完整参数"
    );

    assert!(matches!(events.last(), Some(AgentEvent::Done { .. })));
}

#[tokio::test(start_paused = true)]
async fn 工具结果配对在真实链路下成立() {
    // tool_use / tool_result 配对断了，下一次请求就是 400。
    // 黄金回放里 ScriptedProvider 直接吐结构化消息，绕过了 id 转换；
    // 这里的 id 是真的从 SSE 文本里解析出来的。
    let t = Arc::new(ScriptedTransport::new(vec![
        ScriptedResponse::Chunks(shred(&tool_response(
            "msg_1",
            "toolu_abc",
            "Read",
            r#"{"path":"a.rs"}"#,
        ))),
        ScriptedResponse::Chunks(shred(&text_response("msg_2", "好了"))),
    ]));

    let events = run(
        t,
        ScriptedToolRunner::new(tool_results(&[("toolu_abc", "内容")])),
    )
    .await;

    let mut issued = Vec::new();
    let mut resolved = Vec::new();

    for e in &events {
        if let AgentEvent::Message(message) = e {
            match message {
                Message::Assistant { content, .. } => {
                    for c in content {
                        if let riot_protocol::message::AssistantContent::ToolUse { id, .. } = c {
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
    }

    assert_eq!(issued, vec!["toolu_abc"], "id 要原样从 SSE 里带出来");
    assert_eq!(resolved, issued, "每个 tool_use 都要有对应的 tool_result");
}

#[tokio::test(start_paused = true)]
async fn provider_内部重试对主循环不可见() {
    // 主循环不该看到中间的失败 —— 它只关心最终结果。
    // 泄漏出去的话，主循环会以为模型报错了，然后启动它自己那套恢复逻辑，
    // 两套恢复叠在一起，行为就没人说得清了。
    let t = Arc::new(ScriptedTransport::new(vec![
        ScriptedResponse::Fail(HttpError::status(500, "boom")),
        ScriptedResponse::Fail(HttpError::status(503, "unavailable")),
        ScriptedResponse::Chunks(shred(&text_response("msg_1", "终于成功"))),
    ]));

    let events = run(Arc::clone(&t), ScriptedToolRunner::new(tool_results(&[]))).await;

    assert_eq!(t.call_count(), 3);

    let system_msgs: Vec<_> = durable(&events)
        .into_iter()
        .filter(|e| matches!(e, AgentEvent::Message(Message::System { .. })))
        .collect();

    assert!(
        system_msgs.is_empty(),
        "provider 内部重试不该产生任何系统消息：{system_msgs:?}"
    );
    assert!(matches!(events.last(), Some(AgentEvent::Done { .. })));
}

#[tokio::test(start_paused = true)]
async fn 不可恢复的错误变成对话内容而不是崩溃() {
    let t = Arc::new(ScriptedTransport::new(vec![ScriptedResponse::Fail(
        HttpError::status(400, "invalid tool schema"),
    )]));

    let events = run(t, ScriptedToolRunner::new(tool_results(&[]))).await;

    assert!(
        matches!(events.last(), Some(AgentEvent::Done { .. })),
        "错误是对话内容，不是流的终止方式（INV-4）"
    );

    let has_error_msg = events
        .iter()
        .any(|e| matches!(e, AgentEvent::Message(Message::System { .. })));
    assert!(has_error_msg, "用户要能看到出了什么事");
}

#[tokio::test(start_paused = true)]
async fn 截断的流不会让_agent_静默停住() {
    // 网关在半路断开。没有 decoder.finish() 兜底的话，主循环会拿到
    // 一个既没消息也没错误的空流，然后当成"模型没话说"正常结束 ——
    // 用户看到的是 agent 莫名其妙停下来了。
    let full = tool_response("msg_1", "toolu_1", "Read", r#"{"path":"a.rs"}"#);
    let truncated: String = full.chars().take(full.chars().count() / 2).collect();

    let t = Arc::new(ScriptedTransport::new(vec![ScriptedResponse::Chunks(
        shred(&truncated),
    )]));

    let events = run(t, ScriptedToolRunner::new(tool_results(&[]))).await;

    assert!(matches!(events.last(), Some(AgentEvent::Done { .. })));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::Message(Message::System { .. }))),
        "截断必须变成一条用户可见的消息，不能静默"
    );
}
