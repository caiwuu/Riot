//! Responses 适配层的测试。重点在报文形状：错一个字段就是语焉不详的 400。

use pretty_assertions::assert_eq;
use riot_protocol::id::{MessageId, ToolUseId};
use riot_protocol::message::{
    AssistantContent, Attachment, Message, MessageMeta, ToolResultContent, UserContent,
};
use riot_protocol::provider::{ProviderEvent, ProviderRequest, ThinkingConfig, ToolSpec};

use super::decode::StreamDecoder;
use super::request::{build_request, convert_input};
use super::wire::{WireInputItem, WirePart};
use crate::openai::request::RetryContext;
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
        model: "gpt-5".into(),
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

#[test]
fn 基本请求形状() {
    let w = build_request(&req(vec![user("你好")]), &[], &RetryContext::initial());

    assert_eq!(w.model, "gpt-5");
    assert!(w.stream);
    assert_eq!(w.max_output_tokens, Some(4096));
    assert!(!w.store);
    assert!(w.instructions.is_none());
    assert_eq!(
        w.input,
        vec![WireInputItem::Message {
            role: "user",
            content: vec![WirePart::InputText {
                text: "你好".into()
            }],
        }]
    );

    let json = serde_json::to_value(&w).expect("序列化");
    assert!(json.get("messages").is_none(), "{json}");
    assert!(json.get("stream_options").is_none(), "{json}");
    assert!(json.get("max_tokens").is_none(), "{json}");
    assert_eq!(json["store"], false);
}

#[test]
fn system_进_instructions() {
    let mut r = req(vec![user("hi")]);
    r.system = "你是助手".into();
    let w = build_request(&r, &[], &RetryContext::initial());
    assert_eq!(w.instructions.as_deref(), Some("你是助手"));
}

#[test]
fn 思考配置映射() {
    use riot_protocol::provider::ThinkingEffort;

    let mut r = req(vec![user("你好")]);
    let w = build_request(&r, &[], &RetryContext::initial());
    assert_eq!(w.reasoning, None);
    assert!(w.include.is_empty());

    r.thinking = ThinkingConfig::Effort {
        level: ThinkingEffort::Low,
    };
    let w = build_request(&r, &[], &RetryContext::initial());
    assert_eq!(w.reasoning.as_ref().map(|x| x.effort), Some("low"));
    assert_eq!(
        w.reasoning.as_ref().and_then(|x| x.summary),
        Some("auto"),
        "开了档位要顺手要摘要，否则界面上永远看不到思考"
    );
    assert_eq!(w.include, vec!["reasoning.encrypted_content"]);

    r.thinking = ThinkingConfig::Disabled;
    let w = build_request(&r, &[], &RetryContext::initial());
    assert_eq!(w.reasoning.as_ref().map(|x| x.effort), Some("none"));
    assert_eq!(w.reasoning.as_ref().and_then(|x| x.summary), None);
    assert!(w.include.is_empty());
}

#[test]
fn 扁平工具且按名字排序() {
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
    let names: Vec<_> = w.tools.iter().map(|t| t.name.clone()).collect();
    assert_eq!(names, vec!["Bash", "Write"]);
    let json = serde_json::to_value(&w.tools[0]).expect("序列化");
    assert_eq!(json["type"], "function");
    assert_eq!(json["name"], "Bash");
    assert!(
        json.get("function").is_none(),
        "不能再包一层 function：{json}"
    );
}

#[test]
fn 工具调用和结果配对() {
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

    let out = convert_input(&msgs, false);
    assert_eq!(out.len(), 2);
    match &out[0] {
        WireInputItem::FunctionCall {
            call_id,
            name,
            arguments,
        } => {
            assert_eq!(call_id, "call_1");
            assert_eq!(name, "Read");
            assert_eq!(arguments, r#"{"path":"a.rs"}"#);
        }
        other => panic!("{other:?}"),
    }
    match &out[1] {
        WireInputItem::FunctionCallOutput { call_id, output } => {
            assert_eq!(call_id, "call_1");
            assert_eq!(output, "文件内容");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn 工具结果排在同批用户文本之前() {
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
    let out = convert_input(&msgs, false);
    assert!(
        matches!(out[0], WireInputItem::FunctionCallOutput { .. }),
        "{out:?}"
    );
    assert!(matches!(
        out[1],
        WireInputItem::Message { role: "user", .. }
    ));
}

#[test]
fn 用户附图用_input_image() {
    let msgs = vec![Message::User {
        id: MessageId::from_raw("u1"),
        content: vec![
            UserContent::Attachment(Attachment::Image {
                media_type: "image/png".into(),
                data: "IMG1".into(),
            }),
            UserContent::Text {
                text: "这里为什么错位".into(),
            },
        ],
        meta: MessageMeta::default(),
    }];
    let out = convert_input(&msgs, false);
    let WireInputItem::Message { content, .. } = &out[0] else {
        panic!("{out:?}");
    };
    assert_eq!(
        content[0],
        WirePart::InputImage {
            image_url: "data:image/png;base64,IMG1".into()
        }
    );
    assert!(matches!(&content[1], WirePart::InputText { text } if text == "这里为什么错位"));
}

#[test]
fn 转述图不发_base64() {
    let msgs = vec![Message::User {
        id: MessageId::from_raw("u1"),
        content: vec![
            UserContent::Attachment(Attachment::DescribedImage {
                media_type: "image/jpeg".into(),
                data: "BASE64PAYLOAD".into(),
                text: "图里是一个两栏布局".into(),
            }),
            UserContent::Text {
                text: "这里为什么错位".into(),
            },
        ],
        meta: MessageMeta::default(),
    }];
    let json = serde_json::to_string(&convert_input(&msgs, false)).expect("序列化");
    assert!(json.contains("两栏布局"), "{json}");
    assert!(!json.contains("BASE64PAYLOAD"), "{json}");
}

#[test]
fn 思考签名回传_没有则不发() {
    let with_sig = vec![Message::Assistant {
        id: MessageId::from_raw("a1"),
        content: vec![
            AssistantContent::Thinking {
                text: "先分析".into(),
                signature: Some(r#"{"id":"rs_1","encrypted_content":"enc"}"#.into()),
            },
            AssistantContent::Text {
                text: "答案".into(),
            },
        ],
        usage: None,
        meta: MessageMeta::default(),
    }];
    let out = convert_input(&with_sig, false);
    assert!(
        matches!(
            &out[0],
            WireInputItem::Reasoning {
                id,
                encrypted_content,
                ..
            } if id == "rs_1" && encrypted_content == "enc"
        ),
        "{out:?}"
    );

    let no_sig = vec![Message::Assistant {
        id: MessageId::from_raw("a1"),
        content: vec![AssistantContent::Thinking {
            text: "先分析".into(),
            signature: None,
        }],
        usage: None,
        meta: MessageMeta::default(),
    }];
    assert!(convert_input(&no_sig, false).is_empty());

    assert!(
        convert_input(&with_sig, true)
            .iter()
            .all(|i| !matches!(i, WireInputItem::Reasoning { .. })),
        "降级必须剥签名"
    );
}

/// 回传推理项的 JSON 必须带 `id` 和 `summary`（空也发 `[]`）。
///
/// 这条盯的是真实的 400：服务端不回摘要时 `summary` 是空的，用
/// `skip_serializing_if` 省掉它，第一轮一切正常、第二轮起每次请求都被拒
/// `Missing required parameter: 'input[N].summary'`。比结构体抓不到这个，
/// 得比序列化后的 JSON。
#[test]
fn 回传推理项_json_里_summary_空也要发() {
    let msgs = vec![Message::Assistant {
        id: MessageId::from_raw("a1"),
        content: vec![
            AssistantContent::Thinking {
                // 没请求摘要时正文就是空的，这是最常见的形状
                text: String::new(),
                signature: Some(r#"{"id":"rs_1","encrypted_content":"enc"}"#.into()),
            },
            AssistantContent::Text {
                text: "答案".into(),
            },
        ],
        usage: None,
        meta: MessageMeta::default(),
    }];
    let json = serde_json::to_value(convert_input(&msgs, false)).expect("序列化");
    let item = &json[0];
    assert_eq!(item["type"], "reasoning", "{json}");
    assert_eq!(item["id"], "rs_1", "{json}");
    assert_eq!(item["encrypted_content"], "enc", "{json}");
    assert_eq!(
        item["summary"],
        serde_json::json!([]),
        "summary 必填，空也得是 []：{json}"
    );
}

/// 签名里没有 id 的推理项发出去同样 400，宁可不发。
#[test]
fn 签名缺_id_的推理项不回传() {
    let msgs = vec![Message::Assistant {
        id: MessageId::from_raw("a1"),
        content: vec![
            AssistantContent::Thinking {
                text: "想了想".into(),
                signature: Some(r#"{"encrypted_content":"enc"}"#.into()),
            },
            AssistantContent::Text {
                text: "答案".into(),
            },
        ],
        usage: None,
        meta: MessageMeta::default(),
    }];
    let out = convert_input(&msgs, false);
    assert!(
        !out.iter()
            .any(|i| matches!(i, WireInputItem::Reasoning { .. })),
        "{out:?}"
    );
    assert!(
        matches!(
            &out[0],
            WireInputItem::Message {
                role: "assistant",
                ..
            }
        ),
        "正文照发：{out:?}"
    );
}

/// 一条助手消息里只剩推理项（比如撞上输出上限被截断的那轮）：单独回传
/// 服务端拒收，整条丢掉，后面的对话才发得出去。
#[test]
fn 只有推理项没有后继项的助手消息整条丢掉() {
    let msgs = vec![
        Message::Assistant {
            id: MessageId::from_raw("a1"),
            content: vec![AssistantContent::Thinking {
                text: String::new(),
                signature: Some(r#"{"id":"rs_1","encrypted_content":"enc"}"#.into()),
            }],
            usage: None,
            meta: MessageMeta::default(),
        },
        user("继续"),
    ];
    let out = convert_input(&msgs, false);
    assert_eq!(out.len(), 1, "{out:?}");
    assert!(matches!(
        &out[0],
        WireInputItem::Message { role: "user", .. }
    ));
}

/// 流里的出错事件有两种形态，都要认；漏一种的后果是半截正文被当成
/// 正常回答定稿，用户看不到任何错误。
#[test]
fn 出错事件两种形态都报错不定稿() {
    // 文档写的扁平形态
    let mut d = StreamDecoder::new();
    d.push(&sse(
        r#"{"type":"response.output_text.delta","delta":"半截"}"#,
    ));
    d.push(&sse(
        r#"{"type":"error","code":"server_error","message":"boom","param":null,"sequence_number":3}"#,
    ));
    let out = d.finish();
    assert!(
        matches!(&out[0], ProviderEvent::Error(e) if format!("{e:?}").contains("boom")),
        "{out:?}"
    );
    assert!(
        !out.iter().any(|e| matches!(e, ProviderEvent::Message(_))),
        "出错了就不能把半截正文当成回答：{out:?}"
    );

    // 服务端实际常发的嵌套形态
    let mut d = StreamDecoder::new();
    d.push(&sse(
        r#"{"type":"error","sequence_number":2,"error":{"type":"too_many_requests","code":"no_capacity","message":"busy"}}"#,
    ));
    let out = d.finish();
    assert!(
        matches!(&out[0], ProviderEvent::Error(e) if format!("{e:?}").contains("busy")),
        "{out:?}"
    );
}

#[test]
fn 文本增量累积() {
    let mut d = StreamDecoder::new();
    d.push(&sse(
        r#"{"type":"response.created","response":{"id":"resp_1","model":"gpt-5"}}"#,
    ));
    d.push(&sse(
        r#"{"type":"response.output_text.delta","delta":"你"}"#,
    ));
    d.push(&sse(
        r#"{"type":"response.output_text.delta","delta":"好"}"#,
    ));
    match &d.finish()[0] {
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
fn 工具开始早于参数() {
    let mut d = StreamDecoder::new();
    let start = d.push(&sse(
        r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_a","name":"Read","arguments":""}}"#,
    ));
    assert!(
        matches!(
            &start[0],
            ProviderEvent::Delta(riot_protocol::event::StreamDelta::ToolStart { name, .. })
                if name == "Read"
        ),
        "{start:?}"
    );
    let args = d.push(&sse(
        r#"{"type":"response.function_call_arguments.delta","item_id":"fc_1","output_index":0,"delta":"{\"p"}"#,
    ));
    assert!(
        matches!(
            &args[0],
            ProviderEvent::Delta(riot_protocol::event::StreamDelta::ToolInput { .. })
        ),
        "{args:?}"
    );
    d.push(&sse(
        r#"{"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":"ath\":\"a.rs\"}"}"#,
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
fn completed_usage_扣缓存() {
    let mut d = StreamDecoder::new();
    d.push(&sse(
        r#"{"type":"response.output_text.delta","delta":"hi"}"#,
    ));
    d.push(&sse(
        r#"{"type":"response.completed","response":{"id":"resp_1","usage":{"input_tokens":100,"output_tokens":5,"input_tokens_details":{"cached_tokens":40}},"output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hi"}]}]}}"#,
    ));
    let out = d.finish();
    let usage = out.iter().find_map(|e| match e {
        ProviderEvent::Usage(u) => Some(*u),
        _ => None,
    });
    let usage = usage.expect("要有 usage");
    assert_eq!(usage.input_tokens, 60);
    assert_eq!(usage.output_tokens, 5);
    assert_eq!(usage.cache_read_tokens, 40);
}

#[test]
fn 截断报成可恢复错误() {
    let mut d = StreamDecoder::new();
    d.push(&sse(
        r#"{"type":"response.output_text.delta","delta":"半"}"#,
    ));
    d.push(&sse(
        r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"},"output":[{"type":"message","content":[{"type":"output_text","text":"半"}]}]}}"#,
    ));
    let out = d.finish();
    assert!(
        out.iter().any(|e| matches!(
            e,
            ProviderEvent::Error(riot_protocol::provider::ProviderError::OutputLimit)
        )),
        "{out:?}"
    );
}

#[test]
fn 畸形帧不中断() {
    let mut d = StreamDecoder::new();
    d.push(&sse("not-json"));
    d.push(&sse(
        r#"{"type":"response.output_text.delta","delta":"还在"}"#,
    ));
    match &d.finish()[0] {
        ProviderEvent::Message(Message::Assistant { content, .. }) => {
            assert!(matches!(&content[0], AssistantContent::Text { text } if text == "还在"));
        }
        other => panic!("{other:?}"),
    }
}
