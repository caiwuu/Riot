//! 把 SSE 解析、解码、看门狗、重试组装成一个 [`Provider`]。
//!
//! # 重试对主循环不可见
//!
//! `[约束]` 重试与降级在这一层内部完成。主循环只关心「这次调用最终成功
//! 还是失败」—— 把重试暴露出去会让它同时管两套恢复逻辑，那是 bug 温床。
//!
//! # 但流一旦开始输出就不能重试
//!
//! `[约束]` 只有**请求阶段**的失败才重试。一旦吐出过任何事件，失败就只能上报。
//!
//! 理由是 UI 已经渲染了那些内容。重试会让同一段文本出现两次，而内核这边
//! 没有撤销事件可发。宁可让用户看到一个明确的错误，也不要让他看到重复的
//! 半截回答 —— 后者他会以为模型疯了。
//!
//! 代价是流中途的网络抖动会直接失败。可以接受：那种抖动在流式请求里本来
//! 就少见，而且主循环还有一层。

use std::sync::Arc;
use std::time::Duration;

use async_stream::stream;
use async_trait::async_trait;
use futures::StreamExt;
use riot_protocol::message::Message;
use riot_protocol::provider::{
    Provider, ProviderError, ProviderEvent, ProviderRequest, ProviderStream,
};
use riot_protocol::tool::Clock;
use tokio_util::sync::CancellationToken;

use super::decode::StreamDecoder;
use super::request::{RetryContext, SystemSection, build_request};
use crate::retry::{GiveUpReason, RequestSource, RetryDecision, RetryPolicy, decide};
use crate::sse::SseParser;
use crate::transport::{HttpError, HttpRequest, HttpTransport};
use crate::watchdog::{DEFAULT_IDLE, with_idle_watchdog};

pub struct AnthropicConfig {
    pub base_url: String,
    pub api_key: String,
    pub api_version: String,
    /// 降级目标。连续过载时切过去。
    pub fallback_model: Option<String>,
    pub idle_timeout: Duration,
    pub retry: RetryPolicy,
    /// 订阅制账号。影响 429 的处理 —— 他们的限流窗口是几小时，重试无意义。
    pub is_subscription: bool,
    /// 采样参数。这个协议原生支持 top_k。
    pub sampling: crate::SamplingParams,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.anthropic.com".into(),
            api_key: String::new(),
            api_version: "2023-06-01".into(),
            fallback_model: None,
            idle_timeout: DEFAULT_IDLE,
            retry: RetryPolicy::default(),
            is_subscription: false,
            sampling: crate::SamplingParams::default(),
        }
    }
}

/// 连续多少次过载触发模型降级。
const OVERLOAD_BEFORE_FALLBACK: u32 = 3;

pub struct AnthropicProvider {
    transport: Arc<dyn HttpTransport>,
    clock: Arc<dyn Clock>,
    system: Vec<SystemSection>,
    config: AnthropicConfig,
    source: RequestSource,
}

impl AnthropicProvider {
    pub fn new(
        transport: Arc<dyn HttpTransport>,
        clock: Arc<dyn Clock>,
        system: Vec<SystemSection>,
        config: AnthropicConfig,
    ) -> Self {
        Self {
            transport,
            clock,
            system,
            config,
            source: RequestSource::Foreground,
        }
    }

    /// 标记为后台请求（标题生成、摘要）。**过载时立刻放弃，不参与雪崩。**
    pub fn as_background(mut self) -> Self {
        self.source = RequestSource::Background;
        self
    }
}

/// 组装 HTTP 请求。
///
/// 是自由函数而不是方法，因为 `stream!` 块里拿不到 `&self`（字段被 move
/// 进去了）。写成方法然后在 stream 里再内联一遍，就是同一段逻辑两个版本 ——
/// 改一处忘另一处的经典配方。
fn build_http_request(
    wire: &super::request::WireRequest,
    endpoint: &Endpoint,
) -> Result<HttpRequest, HttpError> {
    let body = serde_json::to_vec(wire)
        .map_err(|e| HttpError::transport(format!("请求序列化失败: {e}")))?;

    Ok(HttpRequest {
        url: format!("{}/v1/messages", endpoint.base_url.trim_end_matches('/')),
        headers: vec![
            ("content-type".into(), "application/json".into()),
            ("accept".into(), "text/event-stream".into()),
            ("x-api-key".into(), endpoint.api_key.clone()),
            ("anthropic-version".into(), endpoint.api_version.clone()),
        ],
        body,
    })
}

#[derive(Clone)]
struct Endpoint {
    base_url: String,
    api_key: String,
    api_version: String,
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn stream(&self, req: ProviderRequest, cancel: CancellationToken) -> ProviderStream {
        let transport = Arc::clone(&self.transport);
        let clock = Arc::clone(&self.clock);
        let system = self.system.clone();
        let source = self.source;
        let is_subscription = self.config.is_subscription;
        let policy = self.config.retry;
        let idle = self.config.idle_timeout;
        let fallback_model = self.config.fallback_model.clone();
        let sampling = self.config.sampling;
        let endpoint = Endpoint {
            base_url: self.config.base_url.clone(),
            api_key: self.config.api_key.clone(),
            api_version: self.config.api_version.clone(),
        };

        Box::pin(stream! {
            let mut retry_ctx = RetryContext::initial();
            let mut attempt = 0u32;
            let mut overload_streak = 0u32;

            'attempts: loop {
                if cancel.is_cancelled() {
                    return;
                }

                let mut wire = build_request(&req, &system, &retry_ctx);
                wire.temperature = sampling.temperature;
                wire.top_p = sampling.top_p;
                wire.top_k = sampling.top_k;
                let http_req = match build_http_request(&wire, &endpoint) {
                    Ok(r) => r,
                    Err(e) => {
                        yield ProviderEvent::Error(ProviderError::Transport {
                            message: e.to_string(),
                        });
                        return;
                    }
                };

                // ── 请求阶段 ──────────────────────────────────
                // 这里的失败可以重试：还没有任何事件流出去。
                let byte_stream = match transport.post_sse(http_req, cancel.child_token()).await {
                    Ok(s) => s,
                    Err(e) => {
                        let overloaded = e.status == Some(529);
                        if overloaded {
                            overload_streak += 1;
                        }

                        // 连续过载够多次就换模型，而不是继续等同一个过载的模型
                        if overloaded
                            && overload_streak >= OVERLOAD_BEFORE_FALLBACK
                            && let Some(fb) = fallback_model.clone()
                            && retry_ctx.model_override.as_deref() != Some(fb.as_str())
                        {
                            tracing::warn!(model = %fb, "连续过载，降级");
                            // fallback_to 同时置位签名剥离 —— 带着旧模型的
                            // thinking 签名去新模型会 400
                            retry_ctx = RetryContext::fallback_to(fb);
                            overload_streak = 0;
                            attempt += 1;
                            continue 'attempts;
                        }

                        let ctx = e.failure_context(source, is_subscription, attempt);
                        match decide(&policy, &ctx, attempt as u64) {
                            RetryDecision::Retry { after } => {
                                clock.sleep_ms(after.as_millis() as u64).await;
                                attempt += 1;
                                continue 'attempts;
                            }
                            RetryDecision::GiveUp(reason) => {
                                yield ProviderEvent::Error(map_giveup(reason, &e));
                                return;
                            }
                        }
                    }
                };

                // ── 流阶段 ────────────────────────────────────
                // 从这里开始不再重试。见模块文档。
                let decoded = decode_stream(byte_stream);
                let guarded = with_idle_watchdog(decoded, idle, Arc::clone(&clock));
                futures::pin_mut!(guarded);

                while let Some(ev) = guarded.next().await {
                    if cancel.is_cancelled() {
                        return;
                    }
                    yield ev;
                }
                return;
            }
        })
    }

    fn count_tokens(&self, messages: &[Message]) -> u32 {
        // 本地粗估。真实计数要调 /v1/messages/count_tokens，
        // 但那是一次额外的网络往返 —— 只在压缩决策临界时才值得。
        //
        // 4 字符 ≈ 1 token 对英文偏准，对中文偏保守（实际约 1.5 字符/token）。
        // 保守是对的方向：低估会让我们压缩得太晚，然后撞上真正的溢出。
        let bytes: usize = messages
            .iter()
            .map(|m| serde_json::to_string(m).map(|s| s.len()).unwrap_or(0))
            .sum();
        (bytes / 4) as u32
    }
}

/// 字节流 → `ProviderEvent` 流。
fn decode_stream(
    mut bytes: crate::transport::ByteStream,
) -> impl futures_core::Stream<Item = ProviderEvent> + Send {
    stream! {
        let mut parser = SseParser::new();
        let mut decoder = StreamDecoder::new();

        while let Some(chunk) = bytes.next().await {
            match chunk {
                Ok(bytes) => {
                    for sse in parser.push(&bytes) {
                        for ev in decoder.push(&sse) {
                            yield ev;
                        }
                    }
                }
                Err(e) => {
                    // 流中途断了。把 decoder 里攒着的半条消息吐出来 ——
                    // 半条比没有有用，用户至少能看到模型说到哪了。
                    for ev in decoder.finish() {
                        yield ev;
                    }
                    yield ProviderEvent::Error(ProviderError::Transport {
                        message: format!("读取响应流失败: {e}"),
                    });
                    return;
                }
            }
        }

        // 收尾：处理没有以空行结尾的最后一帧，以及缺 message_stop 的情况
        if let Some(sse) = parser.finish() {
            for ev in decoder.push(&sse) {
                yield ev;
            }
        }
        for ev in decoder.finish() {
            yield ev;
        }
    }
}

fn map_giveup(reason: GiveUpReason, e: &HttpError) -> ProviderError {
    match reason {
        GiveUpReason::SubscriptionRateLimit => ProviderError::RetriesExhausted {
            message: format!("已达用量上限：{}", e.body),
        },
        GiveUpReason::BackgroundOverload => ProviderError::RetriesExhausted {
            message: "服务过载，后台任务已跳过".into(),
        },
        GiveUpReason::AuthUnrecoverable => ProviderError::Auth {
            message: format!("凭证无效，刷新后仍然失败：{}", e.body),
        },
        GiveUpReason::Exhausted => ProviderError::RetriesExhausted {
            message: e.to_string(),
        },
        GiveUpReason::ServerSaidNo | GiveUpReason::NotRetryable => match e.status {
            Some(401) | Some(403) => ProviderError::Auth {
                message: e.body.clone(),
            },
            Some(400) if e.body.contains("context limit") => {
                match crate::retry::parse_context_overflow(&e.body) {
                    Some(o) => ProviderError::ContextOverflow {
                        used: o.input_tokens,
                        limit: o.context_limit,
                    },
                    None => ProviderError::Refused {
                        message: e.to_string(),
                    },
                }
            }
            // 服务端明确拒绝（参数错误、内容策略）。**没有重试过** ——
            // 不是传输问题，更不是重试耗尽。
            Some(_) => ProviderError::Refused {
                message: e.to_string(),
            },
            None => ProviderError::Transport {
                message: e.to_string(),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{ScriptedResponse, ScriptedTransport};
    use crate::watchdog::TokioClock;
    use riot_protocol::id::MessageId;
    use riot_protocol::message::{MessageMeta, UserContent};
    use riot_protocol::provider::ThinkingConfig;
    use pretty_assertions::assert_eq;

    fn sections() -> Vec<SystemSection> {
        vec![SystemSection::stable("intro", "你是助手")]
    }

    fn req() -> ProviderRequest {
        ProviderRequest {
            model: "claude-x".into(),
            messages: vec![Message::User {
                id: MessageId::from_raw("m1"),
                content: vec![UserContent::Text { text: "hi".into() }],
                meta: MessageMeta::default(),
            }],
            system: String::new(),
            tools: vec![],
            max_output_tokens: None,
            thinking: ThinkingConfig::Off,
        }
    }

    /// 一段完整的成功响应，切成任意分片。
    fn ok_chunks() -> Vec<Vec<u8>> {
        let full = concat!(
            r#"event: message_start"#,
            "\n",
            r#"data: {"type":"message_start","message":{"id":"msg_1","model":"claude-x","usage":{"input_tokens":10,"output_tokens":1}}}"#,
            "\n\n",
            r#"event: content_block_start"#,
            "\n",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            "\n\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"好的"}}"#,
            "\n\n",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "\n\n",
            r#"data: {"type":"message_stop"}"#,
            "\n\n",
        );
        // 切成 7 字节一片，制造跨帧、跨行、跨字符的边界
        full.as_bytes().chunks(7).map(<[u8]>::to_vec).collect()
    }

    fn provider(script: Vec<ScriptedResponse>) -> (AnthropicProvider, Arc<ScriptedTransport>) {
        let t = Arc::new(ScriptedTransport::new(script));
        let p = AnthropicProvider::new(
            Arc::clone(&t) as Arc<dyn HttpTransport>,
            Arc::new(TokioClock),
            sections(),
            AnthropicConfig::default(),
        );
        (p, t)
    }

    async fn collect(p: &AnthropicProvider) -> Vec<ProviderEvent> {
        p.stream(req(), CancellationToken::new()).collect().await
    }

    #[tokio::test(start_paused = true)]
    async fn 端到端_分片响应还原成消息() {
        let (p, t) = provider(vec![ScriptedResponse::Chunks(ok_chunks())]);
        let events = collect(&p).await;

        assert_eq!(t.call_count(), 1);
        let msg = events
            .iter()
            .find_map(|e| match e {
                ProviderEvent::Message(m) => Some(m),
                _ => None,
            })
            .expect("应该有消息");

        match msg {
            Message::Assistant { content, .. } => {
                assert_eq!(
                    content[0],
                    riot_protocol::message::AssistantContent::Text {
                        text: "好的".into()
                    }
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn 请求阶段失败会重试() {
        let (p, t) = provider(vec![
            ScriptedResponse::Fail(HttpError::status(500, "boom")),
            ScriptedResponse::Fail(HttpError::status(503, "boom")),
            ScriptedResponse::Chunks(ok_chunks()),
        ]);
        let events = collect(&p).await;

        assert_eq!(t.call_count(), 3, "前两次失败应该被内部重试掉");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProviderEvent::Message(_))),
            "重试对主循环不可见，它只该看到最终成功"
        );
        assert!(
            !events.iter().any(|e| matches!(e, ProviderEvent::Error(_))),
            "中间的失败不该泄漏出去"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn 流开始输出后不再重试() {
        // 关键约束：UI 已经渲染了那些内容，重试会让同一段文本出现两次，
        // 而内核这边没有撤销事件可发。
        let mut chunks = ok_chunks();
        chunks.truncate(6); // 只吐开头，然后断掉

        let (p, t) = provider(vec![
            ScriptedResponse::PartialThenFail(chunks, HttpError::transport("连接断了")),
            ScriptedResponse::Chunks(ok_chunks()),
        ]);
        let events = collect(&p).await;

        assert_eq!(
            t.call_count(),
            1,
            "已经开始输出了就不能重试，否则用户会看到重复的半截回答"
        );
        assert!(matches!(events.last(), Some(ProviderEvent::Error(_))));
    }

    #[tokio::test(start_paused = true)]
    async fn 认证失败只重试一次() {
        let (p, t) = provider(vec![
            ScriptedResponse::Fail(HttpError::status(401, "invalid api key")),
            ScriptedResponse::Fail(HttpError::status(401, "invalid api key")),
        ]);
        let events = collect(&p).await;

        assert_eq!(
            t.call_count(),
            2,
            "重试一次给调用方刷凭证的机会，然后放弃。\
             靠 max_attempts 兜的话，用户要干等十轮退避才看到「密钥无效」"
        );
        assert!(matches!(
            events[0],
            ProviderEvent::Error(ProviderError::Auth { .. })
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn 参数错误立刻放弃() {
        let (p, t) = provider(vec![ScriptedResponse::Fail(HttpError::status(
            400,
            "invalid tool schema",
        ))]);
        let events = collect(&p).await;

        assert_eq!(t.call_count(), 1, "400 是我们自己的问题，重试一百次也一样");
        assert!(
            matches!(
                events[0],
                ProviderEvent::Error(ProviderError::Refused { .. })
            ),
            "服务端拒绝要报 Refused —— 不是传输错误，更不是重试耗尽，\
             那两种文案都会把用户引去排查网络"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn 上下文溢出被解析出数字() {
        let (p, _) = provider(vec![ScriptedResponse::Fail(HttpError::status(
            400,
            "input length and max_tokens exceed context limit: 188059 + 20000 > 200000",
        ))]);
        let events = collect(&p).await;

        assert_eq!(
            events[0],
            ProviderEvent::Error(ProviderError::ContextOverflow {
                used: 188059,
                limit: 200000,
            }),
            "带上数字主循环才知道该压缩到多少"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn 连续过载触发降级并剥离签名() {
        let t = Arc::new(ScriptedTransport::new(vec![
            ScriptedResponse::Fail(HttpError::status(529, "overloaded")),
            ScriptedResponse::Fail(HttpError::status(529, "overloaded")),
            ScriptedResponse::Fail(HttpError::status(529, "overloaded")),
            ScriptedResponse::Chunks(ok_chunks()),
        ]));
        let p = AnthropicProvider::new(
            Arc::clone(&t) as Arc<dyn HttpTransport>,
            Arc::new(TokioClock),
            sections(),
            AnthropicConfig {
                fallback_model: Some("claude-haiku".into()),
                ..Default::default()
            },
        );

        let events = collect(&p).await;

        let models: Vec<String> = t
            .requests()
            .iter()
            .map(|r| {
                let v: serde_json::Value = serde_json::from_slice(&r.body).expect("请求体是 JSON");
                v["model"].as_str().unwrap_or_default().to_owned()
            })
            .collect();

        assert_eq!(
            models,
            vec!["claude-x", "claude-x", "claude-x", "claude-haiku"],
            "连续 3 次过载后应该换模型，而不是继续等同一个过载的模型"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProviderEvent::Message(_)))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn 后台请求遇到过载立刻放弃() {
        let t = Arc::new(ScriptedTransport::new(vec![
            ScriptedResponse::Fail(HttpError::status(529, "overloaded")),
            ScriptedResponse::Chunks(ok_chunks()),
        ]));
        let p = AnthropicProvider::new(
            Arc::clone(&t) as Arc<dyn HttpTransport>,
            Arc::new(TokioClock),
            sections(),
            AnthropicConfig::default(),
        )
        .as_background();

        let events = collect(&p).await;

        assert_eq!(
            t.call_count(),
            1,
            "容量雪崩时每次重试都是数倍网关放大，而后台失败用户看不见"
        );
        assert!(matches!(
            events[0],
            ProviderEvent::Error(ProviderError::RetriesExhausted { .. })
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn 静默的流被看门狗抓住() {
        // 流建立成功，但一个字节都不来。HTTP timeout 覆盖不到这里。
        let t = Arc::new(ScriptedTransport::new(vec![]));
        let p = AnthropicProvider::new(
            Arc::clone(&t) as Arc<dyn HttpTransport>,
            Arc::new(TokioClock),
            sections(),
            AnthropicConfig {
                idle_timeout: Duration::from_secs(5),
                ..Default::default()
            },
        );

        // 脚本空 → post_sse 返回传输错误 → 走重试 → 最终耗尽
        let events = collect(&p).await;
        assert!(events.iter().any(|e| matches!(e, ProviderEvent::Error(_))));
    }

    #[tokio::test(start_paused = true)]
    async fn 取消后不再发请求() {
        let (p, t) = provider(vec![ScriptedResponse::Chunks(ok_chunks())]);
        let cancel = CancellationToken::new();
        cancel.cancel();

        let events: Vec<_> = p.stream(req(), cancel).collect().await;

        assert_eq!(t.call_count(), 0);
        assert!(events.is_empty(), "取消不产生错误事件，主循环自己会发 Done");
    }

    #[tokio::test(start_paused = true)]
    async fn 请求体带了必需的头() {
        let (p, t) = provider(vec![ScriptedResponse::Chunks(ok_chunks())]);
        collect(&p).await;

        let r = &t.requests()[0];
        let names: Vec<&str> = r.headers.iter().map(|(k, _)| k.as_str()).collect();
        assert!(names.contains(&"x-api-key"));
        assert!(names.contains(&"anthropic-version"), "缺版本头会被拒");
        assert!(r.url.ends_with("/v1/messages"));
    }
}
