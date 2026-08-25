//! OpenAI 适配层的测试。
//!
//! 重点在**报文形状**。这一层的错误不会崩，只会让服务端回一句
//! `invalid request`，而那句话不告诉你是哪个字段错了。

use pretty_assertions::assert_eq;
use riot_protocol::id::{MessageId, ToolUseId};
use riot_protocol::message::{
    AssistantContent, Message, MessageMeta, ToolResultContent, UserContent,
};
use riot_protocol::provider::{ProviderEvent, ProviderRequest, ThinkingConfig, ToolSpec};

use super::decode::StreamDecoder;
use super::request::{RetryContext, build_request, convert_messages, wire_bytes};
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

/// 思考配置 → wire 参数的映射。
///
/// Off 必须一个参数都不发：这是升级后老用户请求不变的底线。
/// Effort 只发标准 `reasoning_effort`；非标准 `thinking` 对象只在显式
/// Disabled 时发 —— 它是 DeepSeek / GLM 的约定，OpenAI 官方收到会 400。
#[test]
fn 思考配置映射成_wire_参数() {
    use riot_protocol::provider::ThinkingEffort;

    let mut r = req(vec![user("你好")]);
    let w = build_request(&r, &[], &RetryContext::initial());
    assert_eq!(w.reasoning_effort, None, "Off 不发力度");
    assert_eq!(w.thinking, None, "Off 不发开关");

    r.thinking = ThinkingConfig::Effort {
        level: ThinkingEffort::Low,
    };
    let w = build_request(&r, &[], &RetryContext::initial());
    assert_eq!(w.reasoning_effort, Some("low"));
    assert_eq!(w.thinking, None, "力度档不捎非标准的开关字段");

    r.thinking = ThinkingConfig::Disabled;
    let w = build_request(&r, &[], &RetryContext::initial());
    assert_eq!(w.reasoning_effort, None);
    assert_eq!(
        serde_json::to_value(w.thinking).expect("序列化"),
        serde_json::json!({ "type": "disabled" }),
        "关闭思考走 DeepSeek / GLM 的 thinking.type 约定"
    );
}

/// Budget 在 OpenAI 兼容协议里没有对应参数，折算成最近的档位。
#[test]
fn 思考预算折算成档位() {
    let mut r = req(vec![user("你好")]);
    for (tokens, expect) in [(2_000, "low"), (10_000, "medium"), (30_000, "high")] {
        r.thinking = ThinkingConfig::Budget { tokens };
        let w = build_request(&r, &[], &RetryContext::initial());
        assert_eq!(w.reasoning_effort, Some(expect), "{tokens} tokens");
    }
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

/// 图片跟在 tool 消息后面单独发一条 user 消息。
///
/// `[约束]` 图片不能塞进 tool 消息:OpenAI 的 `tool` 消息 content 只收字符串。
/// 塞进去要么被服务方 400，要么被当成一段字面 JSON 文本 —— 后者更糟，模型
/// 会拿到一坨 base64 当正文读。
///
/// 而 tool 消息本身也不能为空:空结果会让一部分模型误判任务已经结束。
#[test]
fn 工具结果里的图片跟在后面的_user_消息里() {
    let msgs = vec![Message::User {
        id: MessageId::from_raw("u1"),
        content: vec![UserContent::ToolResult {
            tool_use_id: ToolUseId::from_raw("call_1"),
            content: ToolResultContent::Image {
                media_type: "image/jpeg".into(),
                data: "AAAA".into(),
                path: None,
            },
            is_error: false,
        }],
        meta: MessageMeta::default(),
    }];

    let out = convert_messages(&msgs);
    assert_eq!(out.len(), 2, "一条 tool + 一条带图的 user：{out:?}");

    match &out[0] {
        WireMessage::Tool { content, .. } => {
            assert!(!content.trim().is_empty(), "tool 消息不能为空");
            assert!(!content.contains("AAAA"), "base64 不能出现在 tool 消息里");
        }
        other => panic!("第一条应该是 tool：{other:?}"),
    }

    match &out[1] {
        WireMessage::UserParts { content } => {
            let has_image = content.iter().any(|p| {
                matches!(p, crate::openai::wire::WirePart::ImageUrl { image_url }
                    if image_url.url == "data:image/jpeg;base64,AAAA")
            });
            assert!(has_image, "图片要以 data URL 形式带上：{content:?}");
        }
        other => panic!("第二条应该是带内容块的 user：{other:?}"),
    }

    // 线格式也要对。role 必须还是 user，content 必须是数组 —— 这两点错了
    // 都是服务方 400，而错误信息不会指向这里。
    let json = serde_json::to_value(&out[1]).expect("序列化");
    assert_eq!(json["role"], "user");
    assert!(
        json["content"].is_array(),
        "content 必须是内容块数组：{json}"
    );
    assert_eq!(json["content"][1]["type"], "image_url");
}

/// 用户附的图和文字进同一条 user 消息，而且图在前。
///
/// `[约束]` 顺序要和 user_content 摆进历史的顺序一致:两家的文档都建议
/// 图片在前，实测差别在"先看图再读问题"和"读完问题回头找图"之间 ——
/// 后者更容易答偏。以前这里把图挪到文字后面单发一条，等于把上游特意
/// 排好的顺序又翻了回去。
#[test]
fn 用户附图和文字同一条消息且图在前() {
    use riot_protocol::message::Attachment;

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

    let out = convert_messages(&msgs);
    assert_eq!(out.len(), 1, "图和文字不该拆成两条消息：{out:?}");

    let WireMessage::UserParts { content } = &out[0] else {
        panic!("应该是带内容块的 user：{:?}", out[0]);
    };
    assert!(
        matches!(&content[0], crate::openai::wire::WirePart::ImageUrl { image_url }
            if image_url.url == "data:image/png;base64,IMG1"),
        "图片要排在最前：{content:?}"
    );
    assert!(
        matches!(&content[1], crate::openai::wire::WirePart::Text { text }
            if text == "这里为什么错位"),
        "文字跟在图后面：{content:?}"
    );
}

/// 系统提醒类附件渲染成 `<system-reminder>` 文本，不是字面 JSON。
///
/// 以前直接 `serde_json::to_string`，模型读到的是
/// `{"type":"attachment","kind":"system_reminder",...}` —— 它会把这坨
/// 当成数据而不是指示，而 Anthropic 那条路给的是正常文本，两边行为不一致。
#[test]
fn 系统提醒附件渲染成文本不是_json() {
    use riot_protocol::message::Attachment;

    let msgs = vec![Message::User {
        id: MessageId::from_raw("u1"),
        content: vec![
            UserContent::Attachment(Attachment::SystemReminder {
                text: "这是一条带外提示".into(),
            }),
            UserContent::Text {
                text: "继续".into(),
            },
        ],
        meta: MessageMeta::default(),
    }];

    let out = convert_messages(&msgs);
    let WireMessage::User { content } = &out[0] else {
        panic!("没有图片时应该还是纯文本 user：{:?}", out[0]);
    };
    assert!(content.contains("<system-reminder>"), "{content}");
    assert!(content.contains("这是一条带外提示"), "{content}");
    assert!(!content.contains("\"kind\""), "不该是字面 JSON：{content}");
}

/// 视觉兼容那张图只发转述，base64 一个字节都不能出去。
///
/// `[约束]` 附件里带着图片本体是给界面留的（切回会话要能重画用户发过的
/// 图）。发出去的话，收不了图的模型会拿到一条它看不懂的 image_url —— 而
/// 这正是当初把图片转成文字要避免的那个 400。
#[test]
fn 视觉兼容的图只发转述() {
    use riot_protocol::message::Attachment;

    let msgs = vec![Message::User {
        id: MessageId::from_raw("u1"),
        content: vec![
            UserContent::Attachment(Attachment::DescribedImage {
                media_type: "image/jpeg".into(),
                data: "BASE64PAYLOAD".into(),
                text: "用户附的第 1 张图：\n图里是一个两栏布局".into(),
            }),
            UserContent::Text {
                text: "这里为什么错位".into(),
            },
        ],
        meta: MessageMeta::default(),
    }];

    let out = convert_messages(&msgs);
    let WireMessage::User { content } = &out[0] else {
        panic!("不发图片时应该是纯文本 user：{:?}", out[0]);
    };
    assert!(content.contains("两栏布局"), "转述要发给模型：{content}");
    assert!(
        !content.contains("BASE64PAYLOAD"),
        "图片本体不能发出去：{content}"
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

// ── token 估算 ────────────────────────────────────────
//
// 这一组盯着的是「压缩来得太早」。估算量的必须是发出去的那份:按历史算的话，
// 一张只给界面看的截图能凭空变出几万个 token，用户在实际只用掉一半窗口的
// 时候就被压一次，而每次压缩都是一次有损的历史改写加一次真实的模型调用。

/// 视觉兼容路径下那张图不占模型预算。
///
/// 主模型收不了图时，图片走辅助模型转成文字（见 vision 模块），
/// `DescribedImage` 里的 base64 只留给界面显示 —— provider 一个字节都不发。
/// 早先的估算按整条消息的 JSON 算，于是这张图既不进请求、又照样吃掉预算:
/// 实测一个会话报"96,737 token"的时候，其中 44% 是这种根本不存在的用量。
#[test]
fn 转述图的_base64_不计入估算() {
    let base64 = "A".repeat(100_000);
    let described = |data: String| {
        vec![Message::User {
            id: MessageId::from_raw("u1"),
            content: vec![UserContent::ToolResult {
                tool_use_id: ToolUseId::from_raw("call_1"),
                content: ToolResultContent::DescribedImage {
                    media_type: "image/jpeg".into(),
                    data,
                    path: None,
                    text: "页面上是一张登录表单".into(),
                },
                is_error: false,
            }],
            meta: MessageMeta::default(),
        }]
    };

    let with_image = wire_bytes(&described(base64));
    let without = wire_bytes(&described(String::new()));
    assert_eq!(with_image, without, "base64 不随请求发出去，就不能算进预算");
    assert!(with_image < 200, "只该剩下那句转述：{with_image} 字节");
}

/// 真视觉路径下的图片按张计价:base64 要能从报文字节里**精确**扣掉。
///
/// `count_tokens` 的公式是 `estimate_tokens(wire_bytes - b64) + 按张成本`。
/// 扣减靠"base64 在报文里原样出现一次"这个前提 —— base64 字符集不含需要
/// JSON 转义的字符。差一个字节都说明它被转义或者出现了两次，那时扣减
/// 失效，图片就悄悄回到字节口径（一张 200 KB 的图折五万 token，两三张
/// 顶穿压缩阈值，正是这组测试盯着的"压缩来得太早"）。
#[test]
fn 视觉图的_base64_能从报文字节里精确扣掉() {
    let with_image = |content: ToolResultContent| {
        vec![Message::User {
            id: MessageId::from_raw("u1"),
            content: vec![UserContent::ToolResult {
                tool_use_id: ToolUseId::from_raw("call_1"),
                content,
                is_error: false,
            }],
            meta: MessageMeta::default(),
        }]
    };
    let image = |data: String| {
        with_image(ToolResultContent::Image {
            media_type: "image/jpeg".into(),
            data,
            path: None,
        })
    };
    let marked = |data: String| {
        with_image(ToolResultContent::MarkedImage {
            media_type: "image/jpeg".into(),
            data,
            path: None,
            text: "编号清单".into(),
        })
    };

    // 用合法 base64 字符集造两种大小的图
    let small = "QUFBQQ==".repeat(100);
    let large = "QUFBQQ==".repeat(50_000);

    for (name, msgs_of) in [
        ("Image", &image as &dyn Fn(String) -> Vec<Message>),
        ("MarkedImage", &marked),
    ] {
        let (n_small, b_small) = riot_protocol::provider::wire_images(&msgs_of(small.clone()));
        let (n_large, b_large) = riot_protocol::provider::wire_images(&msgs_of(large.clone()));
        assert_eq!((n_small, n_large), (1, 1), "{name} 该数出一张图");
        assert_eq!(
            wire_bytes(&msgs_of(small.clone())) - b_small,
            wire_bytes(&msgs_of(large.clone())) - b_large,
            "{name} 扣掉 base64 后剩的只有固定结构，不该随图片大小变"
        );
    }
}

/// 思考内容不占预算 —— 它按协议要求根本不回传（见 `思考内容不回传`）。
#[test]
fn 思考内容不计入估算() {
    let thinking = |text: &str| {
        vec![Message::Assistant {
            id: MessageId::from_raw("a1"),
            content: vec![
                AssistantContent::Thinking {
                    text: text.into(),
                    signature: None,
                },
                AssistantContent::Text {
                    text: "答案是 42".into(),
                },
            ],
            usage: None,
            meta: MessageMeta::default(),
        }]
    };

    assert_eq!(
        wire_bytes(&thinking(&"想".repeat(10_000))),
        wire_bytes(&thinking("")),
        "推理过程不回传，就不能算进预算"
    );
}

/// 只给用户看的系统提醒不占预算。
#[test]
fn system_消息不计入估算() {
    let msgs = vec![Message::System {
        id: MessageId::from_raw("s1"),
        level: riot_protocol::message::SystemLevel::Warning,
        text: "上次请求失败了".repeat(100),
    }];
    assert_eq!(wire_bytes(&msgs), 0, "System 消息不进请求");
}

/// 消息 id 和 usage 不占预算。
///
/// 它们是本地簿记，线协议里没有对应字段。一条消息二三十字节，几百条就是
/// 上万个凭空多出来的 token。
#[test]
fn 消息_id_和_usage_不计入估算() {
    let plain = wire_bytes(&[Message::Assistant {
        id: MessageId::from_raw("a1"),
        content: vec![AssistantContent::Text {
            text: "答案".into(),
        }],
        usage: None,
        meta: MessageMeta::default(),
    }]);
    let bookkept = wire_bytes(&[Message::Assistant {
        id: MessageId::from_raw("msg_01JQRSTUVWXYZ0123456789"),
        content: vec![AssistantContent::Text {
            text: "答案".into(),
        }],
        usage: Some(riot_protocol::message::Usage {
            input_tokens: 12345,
            output_tokens: 6789,
            cache_read_tokens: 4096,
            cache_creation_tokens: 2048,
        }),
        meta: MessageMeta::default(),
    }]);
    assert_eq!(plain, bookkept, "本地簿记不该影响预算");
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
    d.push(&sse(
        r#"{"id":"c1","choices":[{"delta":{"content":"前"}}]}"#,
    ));
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
