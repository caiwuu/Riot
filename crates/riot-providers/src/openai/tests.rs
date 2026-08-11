//! OpenAI 适配层的测试。
//!
//! 重点在**报文形状**。这一层的错误不会崩，只会让服务端回一句
//! `invalid request`，而那句话不告诉你是哪个字段错了。

use riot_protocol::id::{MessageId, ToolUseId};
use riot_protocol::message::{
    AssistantContent, Message, MessageMeta, ToolResultContent, UserContent,
};
use riot_protocol::provider::{ProviderEvent, ProviderRequest, ThinkingConfig, ToolSpec};
use pretty_assertions::assert_eq;

use super::decode::StreamDecoder;
use super::request::{RetryContext, build_request, convert_messages};
use super::wire::WireMessage;
use crate::sse::SseEvent;

fn user(text: &str) -> Message {
    Message::User {
        id: MessageId::from_raw("u1"),
        content: vec![UserContent::Text { text: text.into() }],
        meta: MessageMeta::default(),
    }
}

fn req(messages: Vec<Message>) -> ProviderRequest {
    ProviderRequest {
        model: "deepseek-chat".into(),
        messages,
        system: String::new(),
        tools: vec![],
        max_output_tokens: Some(4096),
        thinking: ThinkingConfig::Off,
    }
}

fn sse(data: &str) -> SseEvent {
    SseEvent {
        event: None,
        data: data.to_owned(),
    }
}

// ── 请求组装 ──────────────────────────────────────────

#[test]
fn 基本请求形状() {
    let w = build_request(&req(vec![user("你好")]), &[], &RetryContext::initial());

    assert_eq!(w.model, "deepseek-chat");
    assert!(w.stream);
    assert_eq!(w.max_tokens, Some(4096));
    assert_eq!(
        w.messages,
        vec![WireMessage::User {
            content: "你好".into()
        }]
    );
}

#[test]
fn 请求里要开_usage_上报() {
    // 不开的话流式响应没有 usage，上下文管理层就没有数据决定何时压缩
    let w = build_request(&req(vec![user("hi")]), &[], &RetryContext::initial());
    assert_eq!(
        w.stream_options.map(|o| o.include_usage),
        Some(true),
        "必须显式请求 usage"
    );
}

#[test]
fn 工具结果变成独立的_tool_消息() {
    // `[约束]` OpenAI 的工具结果是 role=tool 的独立消息，不是 user 消息里的块
    let msgs = vec![
        Message::Assistant {
            id: MessageId::from_raw("a1"),
            content: vec![AssistantContent::ToolUse {
                id: ToolUseId::from_raw("call_1"),
                name: "Read".into(),
                input: serde_json::json!({ "path": "a.rs" }),
            }],
            usage: None,
            meta: MessageMeta::default(),
        },
        Message::User {
            id: MessageId::from_raw("u2"),
            content: vec![UserContent::ToolResult {
                tool_use_id: ToolUseId::from_raw("call_1"),
                content: ToolResultContent::text("文件内容"),
                is_error: false,
            }],
            meta: MessageMeta::default(),
        },
    ];

    let out = convert_messages(&msgs);
    assert_eq!(out.len(), 2);

    match &out[0] {
        WireMessage::Assistant { tool_calls, .. } => {
            assert_eq!(tool_calls.len(), 1);
            assert_eq!(tool_calls[0].id, "call_1");
            assert_eq!(tool_calls[0].function.name, "Read");
            // `[约束]` arguments 是 JSON 字符串不是对象
            assert_eq!(tool_calls[0].function.arguments, r#"{"path":"a.rs"}"#);
        }
        other => panic!("应该是 assistant：{other:?}"),
    }

    assert_eq!(
        out[1],
        WireMessage::Tool {
            tool_call_id: "call_1".into(),
            content: "文件内容".into(),
        }
    );
}

#[test]
fn 工具结果排在同批用户文本之前() {
    // OpenAI 要求 tool 消息紧跟带 tool_calls 的 assistant，中间不能插 user。
    // 用户在工具执行期间插话时，内部会把两者放进同一条 User 消息。
    let msgs = vec![Message::User {
        id: MessageId::from_raw("u1"),
        content: vec![
            UserContent::Text {
                text: "顺便看下这个".into(),
            },
            UserContent::ToolResult {
                tool_use_id: ToolUseId::from_raw("call_1"),
                content: ToolResultContent::text("结果"),
                is_error: false,
            },
        ],
        meta: MessageMeta::default(),
    }];

    let out = convert_messages(&msgs);
    assert!(
        matches!(out[0], WireMessage::Tool { .. }),
        "tool 必须排在前面：{out:?}"
    );
    assert!(matches!(out[1], WireMessage::User { .. }));
}

#[test]
fn 思考内容不回传() {
    // `[约束]` DeepSeek 明确要求不要把 reasoning_content 传回去，会 400
    let msgs = vec![Message::Assistant {
        id: MessageId::from_raw("a1"),
        content: vec![
            AssistantContent::Thinking {
                text: "让我想想……".into(),
                signature: None,
            },
            AssistantContent::Text {
                text: "答案是 42".into(),
            },
        ],
        usage: None,
        meta: MessageMeta::default(),
    }];

    let out = convert_messages(&msgs);
    assert_eq!(out.len(), 1);
    match &out[0] {
        WireMessage::Assistant { content, .. } => {
            assert_eq!(content.as_deref(), Some("答案是 42"));
            assert!(
                !content.as_deref().unwrap_or_default().contains("让我想想"),
                "思考内容漏进请求了"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn 空消息被丢掉() {
    // 压缩清理过历史之后会出现内容为空的消息，服务端会拒收整个请求
    let msgs = vec![
        Message::Assistant {
            id: MessageId::from_raw("a1"),
            content: vec![AssistantContent::Text { text: "   ".into() }],
            usage: None,
            meta: MessageMeta::default(),
        },
        user("有内容"),
    ];

    let out = convert_messages(&msgs);
    assert_eq!(out.len(), 1);
    assert!(matches!(out[0], WireMessage::User { .. }));
}

#[test]
fn 空的工具结果被替换成占位文本() {
    // 空 tool_result 会让部分模型误判任务已经结束
    let msgs = vec![Message::User {
        id: MessageId::from_raw("u1"),
        content: vec![UserContent::ToolResult {
            tool_use_id: ToolUseId::from_raw("c1"),
            content: ToolResultContent::text(""),
            is_error: false,
        }],
        meta: MessageMeta::default(),
    }];

    match &convert_messages(&msgs)[0] {
        WireMessage::Tool { content, .. } => assert!(!content.trim().is_empty()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn 错误结果带上标记() {
    let msgs = vec![Message::User {
        id: MessageId::from_raw("u1"),
        content: vec![UserContent::ToolResult {
            tool_use_id: ToolUseId::from_raw("c1"),
            content: ToolResultContent::text("文件不存在"),
            is_error: true,
        }],
        meta: MessageMeta::default(),
    }];

    match &convert_messages(&msgs)[0] {
        WireMessage::Tool { content, .. } => assert!(content.contains("错误")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn 工具按名字排序() {
    // 顺序不稳的话服务端的前缀缓存每轮都会失效
    let mut r = req(vec![user("hi")]);
    r.tools = vec![
        ToolSpec {
            name: "Write".into(),
            description: "写".into(),
            input_schema: serde_json::json!({}),
        },
        ToolSpec {
            name: "Bash".into(),
            description: "跑".into(),
            input_schema: serde_json::json!({}),
        },
    ];

    let w = build_request(&r, &[], &RetryContext::initial());
    let names: Vec<_> = w.tools.iter().map(|t| t.function.name.clone()).collect();
    assert_eq!(names, vec!["Bash", "Write"]);
}

#[test]
fn 降级会换掉模型名() {
    let w = build_request(
        &req(vec![user("hi")]),
        &[],
        &RetryContext::fallback_to("deepseek-reasoner"),
    );
    assert_eq!(w.model, "deepseek-reasoner");
}

// ── 流解码 ────────────────────────────────────────────

#[test]
fn 文本增量累积成完整消息() {
    let mut d = StreamDecoder::new();
    d.push(&sse(
        r#"{"id":"chatcmpl-1","model":"deepseek-chat","choices":[{"delta":{"content":"你"}}]}"#,
    ));
    d.push(&sse(r#"{"choices":[{"delta":{"content":"好"}}]}"#));
    let out = d.finish();

    match &out[0] {
        ProviderEvent::Message(Message::Assistant { content, .. }) => {
            assert_eq!(
                content[0],
                AssistantContent::Text {
                    text: "你好".into()
                }
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn 每个文本片段都发一次增量() {
    // 前端靠这个逐字渲染
    let mut d = StreamDecoder::new();
    let evs = d.push(&sse(
        r#"{"id":"c1","choices":[{"delta":{"content":"abc"}}]}"#,
    ));
    assert_eq!(evs.len(), 1);
    assert!(matches!(
        &evs[0],
        ProviderEvent::Delta(riot_protocol::event::StreamDelta::Text { text, .. }) if text == "abc"
    ));
}

#[test]
fn reasoning_content_映射成思考() {
    let mut d = StreamDecoder::new();
    d.push(&sse(
        r#"{"id":"c1","choices":[{"delta":{"reasoning_content":"先分析一下"}}]}"#,
    ));
    d.push(&sse(r#"{"choices":[{"delta":{"content":"结论"}}]}"#));

    match &d.finish()[0] {
        ProviderEvent::Message(Message::Assistant { content, .. }) => {
            assert!(matches!(
                &content[0],
                AssistantContent::Thinking { text, .. } if text == "先分析一下"
            ));
            assert!(matches!(&content[1], AssistantContent::Text { .. }));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn 工具调用分片拼起来() {
    // 参数是一片一片来的，第一片带 id 和 name，后面只有 arguments
    let mut d = StreamDecoder::new();
    d.push(&sse(r#"{"id":"c1","choices":[{"delta":{"tool_calls":[
        {"index":0,"id":"call_a","function":{"name":"Read","arguments":"{\"pa"}}]}}]}"#));
    d.push(&sse(
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"a.rs\"}"}}]}}]}"#,
    ));

    match &d.finish()[0] {
        ProviderEvent::Message(Message::Assistant { content, .. }) => match &content[0] {
            AssistantContent::ToolUse { id, name, input } => {
                assert_eq!(id.as_str(), "call_a");
                assert_eq!(name, "Read");
                assert_eq!(input, &serde_json::json!({ "path": "a.rs" }));
            }
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
}

#[test]
fn 多个工具调用按_index_保序() {
    // 顺序就是调用顺序，错了会让有依赖的命令跑反
    let mut d = StreamDecoder::new();
    d.push(&sse(r#"{"id":"c1","choices":[{"delta":{"tool_calls":[
        {"index":1,"id":"call_b","function":{"name":"B","arguments":"{}"}},
        {"index":0,"id":"call_a","function":{"name":"A","arguments":"{}"}}]}}]}"#));

    match &d.finish()[0] {
        ProviderEvent::Message(Message::Assistant { content, .. }) => {
            let names: Vec<_> = content
                .iter()
                .filter_map(|c| match c {
                    AssistantContent::ToolUse { name, .. } => Some(name.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(names, vec!["A", "B"]);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn 无参工具的空参数当成空对象() {
    let mut d = StreamDecoder::new();
    d.push(&sse(
        r#"{"id":"c1","choices":[{"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"Now","arguments":""}}]}}]}"#,
    ));

    match &d.finish()[0] {
        ProviderEvent::Message(Message::Assistant { content, .. }) => match &content[0] {
            AssistantContent::ToolUse { input, .. } => {
                assert_eq!(input, &serde_json::json!({}));
            }
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
}

#[test]
fn 参数被截断时仍然产出_tool_use() {
    // 丢掉的话 tool_use / tool_result 配对就断了，下一次请求必定 400
    let mut d = StreamDecoder::new();
    d.push(&sse(
        r#"{"id":"c1","choices":[{"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"Read","arguments":"{\"pa"}}]}}]}"#,
    ));

    match &d.finish()[0] {
        ProviderEvent::Message(Message::Assistant { content, .. }) => {
            assert!(matches!(&content[0], AssistantContent::ToolUse { .. }));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn 用量被记录() {
    let mut d = StreamDecoder::new();
    d.push(&sse(r#"{"id":"c1","choices":[{"delta":{"content":"x"}}]}"#));
    d.push(&sse(
        r#"{"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":20,"prompt_cache_hit_tokens":80}}"#,
    ));

    let usage = d.finish().into_iter().find_map(|e| match e {
        ProviderEvent::Usage(u) => Some(u),
        _ => None,
    });
    let u = usage.expect("要有用量");
    assert_eq!(u.input_tokens, 100);
    assert_eq!(u.output_tokens, 20);
    assert_eq!(u.cache_read_tokens, 80);
}

#[test]
fn 全零的用量不覆盖已有数字() {
    // 兼容实现常在中间的 chunk 里发全零 usage
    let mut d = StreamDecoder::new();
    d.push(&sse(
        r#"{"id":"c1","choices":[],"usage":{"prompt_tokens":100,"completion_tokens":20}}"#,
    ));
    d.push(&sse(
        r#"{"choices":[],"usage":{"prompt_tokens":0,"completion_tokens":0}}"#,
    ));

    let u = d
        .finish()
        .into_iter()
        .find_map(|e| match e {
            ProviderEvent::Usage(u) => Some(u),
            _ => None,
        })
        .expect("要有用量");
    assert_eq!(u.input_tokens, 100, "被零覆盖了");
}

#[test]
fn 输出被截断报成可恢复错误() {
    // `[约束]` finish_reason=length 说明回答缺了一截。当成正常结束的话
    // 没有任何人会发现。
    let mut d = StreamDecoder::new();
    d.push(&sse(
        r#"{"id":"c1","choices":[{"delta":{"content":"半句"},"finish_reason":"length"}]}"#,
    ));

    let has_limit = d.finish().iter().any(|e| {
        matches!(
            e,
            ProviderEvent::Error(riot_protocol::provider::ProviderError::OutputLimit)
        )
    });
    assert!(has_limit);
}

#[test]
fn 正常结束不报错() {
    let mut d = StreamDecoder::new();
    d.push(&sse(
        r#"{"id":"c1","choices":[{"delta":{"content":"完整"},"finish_reason":"stop"}]}"#,
    ));

    assert!(
        !d.finish()
            .iter()
            .any(|e| matches!(e, ProviderEvent::Error(_)))
    );
}

#[test]
fn done_标记不产生内容() {
    let mut d = StreamDecoder::new();
    assert!(d.push(&sse("[DONE]")).is_empty());
}

#[test]
fn 畸形帧被跳过而不是中断整条流() {
    // 一帧坏了就作废前面所有内容，比丢一帧糟糕得多
    let mut d = StreamDecoder::new();
    d.push(&sse(r#"{"id":"c1","choices":[{"delta":{"content":"前"}}]}"#));
    d.push(&sse("{这不是合法 JSON"));
    d.push(&sse(r#"{"choices":[{"delta":{"content":"后"}}]}"#));

    match &d.finish()[0] {
        ProviderEvent::Message(Message::Assistant { content, .. }) => {
            assert_eq!(
                content[0],
                AssistantContent::Text {
                    text: "前后".into()
                }
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn 数据里的错误对象变成拒绝() {
    // 有些兼容实现不给 HTTP 状态码，把错误塞在 SSE 数据里
    let mut d = StreamDecoder::new();
    d.push(&sse(
        r#"{"error":{"message":"余额不足","type":"insufficient_quota"}}"#,
    ));

    match &d.finish()[0] {
        ProviderEvent::Error(riot_protocol::provider::ProviderError::Refused { message }) => {
            assert!(message.contains("余额不足"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn 没有内容时不产出空消息() {
    let mut d = StreamDecoder::new();
    d.push(&sse(r#"{"id":"c1","choices":[{"delta":{}}]}"#));

    assert!(
        !d.finish()
            .iter()
            .any(|e| matches!(e, ProviderEvent::Message(_))),
        "空消息发给下一轮请求会被服务端拒"
    );
}

#[test]
fn finish_只生效一次() {
    let mut d = StreamDecoder::new();
    d.push(&sse(r#"{"id":"c1","choices":[{"delta":{"content":"x"}}]}"#));

    assert!(!d.finish().is_empty());
    assert!(d.finish().is_empty(), "重复调用会让消息发两遍");
}

#[test]
fn 首帧没有_id_也不丢文本() {
    // 有些兼容实现第一帧不带 id
    let mut d = StreamDecoder::new();
    let evs = d.push(&sse(r#"{"choices":[{"delta":{"content":"开头"}}]}"#));
    assert_eq!(evs.len(), 1, "首帧的文本不能丢");
}
