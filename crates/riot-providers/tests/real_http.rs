//! 真实 HTTP 端到端。
//!
//! 前面所有 provider 测试用的都是 `ScriptedTransport` —— 它验证解码逻辑，
//! 但绕过了整条网络路径。这里起一个真的 TCP 服务器，让 reqwest 真的发包。
//!
//! 这一层能抓到替身抓不到的东西：请求头拼错、body 没序列化对、chunked
//! 编码没处理、状态码映射反了、连接没复用。那些问题在替身下全是绿的，
//! 到了真实 API 面前就是一个 400 加一句"invalid request"。

#![allow(clippy::disallowed_methods)]

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use riot_protocol::id::MessageId;
use riot_protocol::message::{AssistantContent, Message, MessageMeta, UserContent};
use riot_protocol::provider::{
    Provider, ProviderError, ProviderEvent, ProviderRequest, ThinkingConfig, ToolSpec,
};
use riot_providers::watchdog::TokioClock;
use riot_providers::{OpenAiConfig, OpenAiProvider, ReqwestTransport};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// 一次性 HTTP 服务器。返回 (base_url, 收到的请求体)。
async fn serve(responses: Vec<String>) -> (String, tokio::sync::oneshot::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定端口");
    let port = listener.local_addr().expect("取端口").port();
    let (tx, rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        let mut tx = Some(tx);
        for body in responses {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };

            // 读请求。够读到 body 就行 —— 我们只断言 JSON 内容。
            let mut buf = vec![0u8; 64 * 1024];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
            if let Some(t) = tx.take() {
                let json = raw.split("\r\n\r\n").nth(1).unwrap_or("").to_owned();
                let _ = t.send(json);
            }

            let _ = sock.write_all(body.as_bytes()).await;
            let _ = sock.flush().await;
            let _ = sock.shutdown().await;
        }
    });

    (format!("http://127.0.0.1:{port}"), rx)
}

fn sse_ok(events: &[&str]) -> String {
    let body: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn http_err(status: u16, extra: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}",
        body.len()
    )
}

fn provider(base_url: String) -> OpenAiProvider {
    OpenAiProvider::new(
        Arc::new(ReqwestTransport::new().expect("建客户端")),
        Arc::new(TokioClock),
        Vec::new(),
        OpenAiConfig {
            base_url,
            api_key: "sk-test".into(),
            idle_timeout: Duration::from_secs(5),
            // 退避调到几乎为零。默认策略要退避到 32 秒、试 10 次，
            // 那样一个"连不上"的用例要跑一分多钟 —— 慢测试等于没测试，
            // 它会被 --skip 掉然后没人再看。退避算法本身在 retry.rs
            // 的单测里验证。
            retry: riot_providers::RetryPolicy {
                max_attempts: 3,
                base: Duration::from_millis(1),
                cap: Duration::from_millis(5),
                jitter: 0.0,
            },
            ..Default::default()
        },
    )
}

fn request() -> ProviderRequest {
    ProviderRequest {
        model: "deepseek-chat".into(),
        messages: vec![Message::User {
            id: MessageId::from_raw("u1"),
            content: vec![UserContent::Text {
                text: "你好".into(),
            }],
            meta: MessageMeta::default(),
        }],
        system: "你是助手".into(),
        tools: vec![],
        max_output_tokens: Some(1024),
        thinking: ThinkingConfig::Off,
    }
}

async fn collect(p: &OpenAiProvider, req: ProviderRequest) -> Vec<ProviderEvent> {
    let s = p.stream(req, CancellationToken::new());
    futures::pin_mut!(s);
    let mut out = Vec::new();
    while let Some(e) = s.next().await {
        out.push(e);
    }
    out
}

#[tokio::test]
async fn 真实_http_跑通一次对话() {
    let (url, got) = serve(vec![sse_ok(&[
        r#"{"id":"chatcmpl-1","model":"deepseek-chat","choices":[{"delta":{"role":"assistant","content":"你"}}]}"#,
        r#"{"choices":[{"delta":{"content":"好"},"finish_reason":"stop"}]}"#,
        r#"{"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":2}}"#,
        "[DONE]",
    ])])
    .await;

    let events = collect(&provider(url), request()).await;

    let msg = events
        .iter()
        .find_map(|e| match e {
            ProviderEvent::Message(m) => Some(m),
            _ => None,
        })
        .expect("要有完整消息");

    match msg {
        Message::Assistant { content, .. } => assert_eq!(
            content[0],
            AssistantContent::Text {
                text: "你好".into()
            }
        ),
        other => panic!("{other:?}"),
    }

    // 请求体的形状也要对 —— 这是替身测不到的部分
    let body = got.await.expect("收到请求");
    let json: serde_json::Value = serde_json::from_str(&body).expect("请求体是 JSON");
    assert_eq!(json["model"], "deepseek-chat");
    assert_eq!(json["stream"], true);
    assert_eq!(json["stream_options"]["include_usage"], true);
    assert_eq!(json["messages"][0]["role"], "system");
    assert_eq!(json["messages"][1]["content"], "你好");
}

#[tokio::test]
async fn 用量走到了事件里() {
    let (url, _) = serve(vec![sse_ok(&[
        r#"{"id":"c1","choices":[{"delta":{"content":"x"}}]}"#,
        r#"{"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":5}}"#,
        "[DONE]",
    ])])
    .await;

    let usage = collect(&provider(url), request())
        .await
        .into_iter()
        .find_map(|e| match e {
            ProviderEvent::Usage(u) => Some(u),
            _ => None,
        })
        .expect("要有用量，否则上下文管理没有数据");

    assert_eq!(usage.input_tokens, 100);
}

#[tokio::test]
async fn 工具调用穿过真实_http() {
    let (url, got) = serve(vec![sse_ok(&[
        r#"{"id":"c1","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"Read","arguments":""}}]}}]}"#,
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":\"a.rs\"}"}}]},"finish_reason":"tool_calls"}]}"#,
        "[DONE]",
    ])])
    .await;

    let mut req = request();
    req.tools = vec![ToolSpec {
        name: "Read".into(),
        description: "读文件".into(),
        input_schema: serde_json::json!({ "type": "object" }),
    }];

    let events = collect(&provider(url), req).await;

    let msg = events
        .iter()
        .find_map(|e| match e {
            ProviderEvent::Message(m) => Some(m),
            _ => None,
        })
        .expect("要有消息");

    match msg {
        Message::Assistant { content, .. } => match &content[0] {
            AssistantContent::ToolUse { id, name, input } => {
                assert_eq!(id.as_str(), "call_1");
                assert_eq!(name, "Read");
                assert_eq!(input["path"], "a.rs");
            }
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }

    // 工具定义也要正确地送出去
    let json: serde_json::Value =
        serde_json::from_str(&got.await.expect("收到请求")).expect("是 JSON");
    assert_eq!(json["tools"][0]["type"], "function");
    assert_eq!(json["tools"][0]["function"]["name"], "Read");
}

#[tokio::test]
async fn 认证失败不重试() {
    // `[约束]` 401 只会为"凭证可能刚过期"再试一次，之后立刻放弃 ——
    // 重试一百次不会让密钥变对，只会拖长用户看到错误的时间。
    // 这里备了三个响应，用来确认它没有把重试次数用满。
    let bad = || http_err(401, "", r#"{"error":{"message":"Authentication Fails"}}"#);
    let (url, _) = serve(vec![bad(), bad(), bad()]).await;

    let events = collect(&provider(url), request()).await;

    match events.last().expect("要有事件") {
        ProviderEvent::Error(ProviderError::Auth { message }) => {
            assert!(
                message.contains("Authentication"),
                "错误原文要带上：{message}"
            );
        }
        other => panic!("401 应该映射成 Auth 错误：{other:?}"),
    }
}

#[tokio::test]
async fn 限流之后会重试并成功() {
    // 服务器第一次回 429，第二次正常。retry-after 用 0 秒免得测试真的等。
    let (url, _) = serve(vec![
        http_err(429, "retry-after: 0\r\n", r#"{"error":"rate limited"}"#),
        sse_ok(&[
            r#"{"id":"c1","choices":[{"delta":{"content":"重试成功"}}]}"#,
            "[DONE]",
        ]),
    ])
    .await;

    let events = collect(&provider(url), request()).await;

    let msg = events
        .iter()
        .find_map(|e| match e {
            ProviderEvent::Message(m) => Some(m),
            _ => None,
        })
        .expect("重试之后应该拿到消息");

    match msg {
        Message::Assistant { content, .. } => assert_eq!(
            content[0],
            AssistantContent::Text {
                text: "重试成功".into()
            }
        ),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn 连不上的服务器报传输错误() {
    // TEST-NET-1，保留给文档示例，不会有人监听
    let events = collect(&provider("http://192.0.2.1:9".into()), request()).await;

    assert!(
        matches!(
            events.last(),
            Some(ProviderEvent::Error(
                ProviderError::RetriesExhausted { .. } | ProviderError::Transport { .. }
            ))
        ),
        "连不上必须报错而不是静默结束：{events:?}"
    );
}

#[tokio::test]
async fn 分片切在多字节字符中间也不乱码() {
    // TCP 不保证按帧切。"好" 的 UTF-8 是 3 字节，这里让它跨两个 TCP 段。
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定");
    let port = listener.local_addr().expect("端口").port();

    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.expect("接受连接");
        let mut buf = vec![0u8; 64 * 1024];
        let _ = sock.read(&mut buf).await;

        let full = format!(
            "data: {}\n\ndata: [DONE]\n\n",
            r#"{"id":"c1","choices":[{"delta":{"content":"你好世界"}}]}"#
        );
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            full.len()
        );
        let _ = sock.write_all(head.as_bytes()).await;

        // 在 "好" 的三个字节中间切开
        let bytes = full.as_bytes();
        let cut = full.find("你好").expect("找到位置") + 4;
        let _ = sock.write_all(&bytes[..cut]).await;
        let _ = sock.flush().await;
        tokio::time::sleep(Duration::from_millis(30)).await;
        let _ = sock.write_all(&bytes[cut..]).await;
        let _ = sock.shutdown().await;
    });

    let events = collect(&provider(format!("http://127.0.0.1:{port}")), request()).await;

    let msg = events
        .iter()
        .find_map(|e| match e {
            ProviderEvent::Message(m) => Some(m),
            _ => None,
        })
        .expect("要有消息");

    match msg {
        Message::Assistant { content, .. } => assert_eq!(
            content[0],
            AssistantContent::Text {
                text: "你好世界".into()
            },
            "多字节字符被切开后重组错了"
        ),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn 取消之后流立刻停下() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定");
    let port = listener.local_addr().expect("端口").port();

    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.expect("接受");
        let mut buf = vec![0u8; 64 * 1024];
        let _ = sock.read(&mut buf).await;
        let _ = sock
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n")
            .await;
        // 一直吐，直到对端断开
        loop {
            if sock
                .write_all(
                    b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n",
                )
                .await
                .is_err()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    let p = provider(format!("http://127.0.0.1:{port}"));
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        c.cancel();
    });

    let started = std::time::Instant::now();
    let s = p.stream(request(), cancel);
    futures::pin_mut!(s);
    while s.next().await.is_some() {}

    assert!(
        started.elapsed() < Duration::from_secs(3),
        "取消之后还在收：{:?}",
        started.elapsed()
    );
}
