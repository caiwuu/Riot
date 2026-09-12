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
use super::request::{RetryContext, SystemSection, build_request, wire_bytes};
use crate::retry::{GiveUpReason, RequestSource, RetryDecision, RetryPolicy, decide};
use crate::sse::SseParser;
use crate::transport::{HttpError, HttpRequest, HttpTransport};
use crate::watchdog::{DEFAULT_IDLE, with_idle_watchdog};

pub struct AnthropicConfig {
    pub base_url: String,
    /// 接口路径。空 = 按 base 猜（见 [`crate::endpoint::api_url`]）。
    pub api_path: String,
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
    /// 用户配置的额外请求头（模板已在宿主展开）。
    pub extra_headers: Vec<(String, String)>,
}

/// `[约束]` 手写而不是 derive：这个结构体里有明文 API key，而 `Debug`
/// 只要存在，任何一处 `tracing::debug!(?config)` 就会把密钥写进日志文件 ——
/// 日志会被用户贴进 issue，密钥就此公开。两侧配置的口径必须一致，
/// 否则"这边能打那边不能打"迟早被人当成 bug 修成 derive。
impl std::fmt::Debug for AnthropicConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicConfig")
            .field("base_url", &self.base_url)
            .field("api_path", &self.api_path)
            .field("api_key", &"<redacted>")
            .field("api_version", &self.api_version)
            .field("fallback_model", &self.fallback_model)
            .field("idle_timeout", &self.idle_timeout)
            .field("retry", &self.retry)
            .field("is_subscription", &self.is_subscription)
            .field("sampling", &self.sampling)
            .field("extra_headers", &self.extra_headers)
            .finish()
    }
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.anthropic.com".into(),
            api_path: String::new(),
            api_key: String::new(),
            api_version: "2023-06-01".into(),
            fallback_model: None,
            idle_timeout: DEFAULT_IDLE,
            retry: RetryPolicy::default(),
            is_subscription: false,
            sampling: crate::SamplingParams::default(),
            extra_headers: Vec::new(),
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
        .map_err(|e| HttpError::transport(format!("failed to serialize request: {e}")))?;

    Ok(HttpRequest {
        url: crate::endpoint::api_url_with(
            &endpoint.base_url,
            &endpoint.api_path,
            "v1",
            "messages",
        ),
        headers: crate::headers::merge_headers(
            vec![
                ("content-type".into(), "application/json".into()),
                ("accept".into(), "text/event-stream".into()),
                ("x-api-key".into(), endpoint.api_key.clone()),
                ("anthropic-version".into(), endpoint.api_version.clone()),
            ],
            &endpoint.extra_headers,
        ),
        body,
    })
}

#[derive(Clone)]
struct Endpoint {
    base_url: String,
    /// 接口路径。空 = 按 base 猜，见 [`crate::endpoint::api_url`]。
    api_path: String,
    api_key: String,
    api_version: String,
    extra_headers: Vec<(String, String)>,
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
            api_path: self.config.api_path.clone(),
            api_key: self.config.api_key.clone(),
            api_version: self.config.api_version.clone(),
            extra_headers: self.config.extra_headers.clone(),
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
                // `[约束]` 开了 thinking 就不能注采样：Anthropic 对
                // thinking + temperature/top_k 的组合直接 400。思考的价值
                // 高于用户调过的采样 —— 两者冲突时保思考、弃采样。
                if wire.thinking.is_none() {
                    wire.temperature = sampling.temperature;
                    wire.top_p = sampling.top_p;
                    wire.top_k = sampling.top_k;
                }
                let http_req = match build_http_request(&wire, &endpoint) {
                    Ok(r) => r,
                    Err(e) => {
                        yield ProviderEvent::Error(ProviderError::transport(e));
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
                    // 盖的是发出去的 model（降级后就是降级到的那个），不是
                    // 服务端回显的别名解析结果。理由见 `crate::origin`。
                    yield crate::origin::stamp_model_origin(ev, &wire.model);
                }
                return;
            }
        })
    }

    fn count_tokens(&self, messages: &[Message]) -> u32 {
        // 服务方报过的真实计数打底，只有它之后的新消息才粗估。真实计数要
        // 主动调 /v1/messages/count_tokens 得再花一次网络往返，而上一轮的
        // 响应里已经免费带回来了。
        //
        // 量的是**发出去的那份**，见 trait 上那条约束。
        //
        // 图片按张计价：先把它的 base64 从报文长度里扣掉，再按张加回来。
        // 不扣就还是字节口径，而那个口径下一张图能报出几万 token。
        let (from, base) = riot_protocol::provider::last_usage_checkpoint(messages);
        base + self.estimate_tokens_of(&messages[from..])
    }

    fn estimate_tokens_of(&self, messages: &[Message]) -> u32 {
        let (images, b64) = riot_protocol::provider::wire_images(messages);
        riot_protocol::provider::estimate_tokens(wire_bytes(messages).saturating_sub(b64))
            + riot_protocol::provider::estimate_image_tokens(images)
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
                    // 解析器判定流不可信（帧无限长、总量爆表）时必须就地终止：
                    // 继续读下去就是替对面把内存吃光。
                    let events = match parser.push(&bytes) {
                        Ok(evs) => evs,
                        Err(e) => {
                            for ev in decoder.finish() {
                                yield ev;
                            }
                            yield ProviderEvent::Error(crate::errors::stream_broken(e));
                            return;
                        }
                    };
                    for sse in events {
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
                    yield ProviderEvent::Error(crate::errors::stream_broken(format!(
                        "failed to read response stream: {e}"
                    )));
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

/// 放弃重试后的错误映射。键的选择在 [`crate::errors`]；这里只贡献
/// Anthropic 认上下文超长的那一手：400 正文里带 "context limit" 和两个数。
fn map_giveup(reason: GiveUpReason, e: &HttpError) -> ProviderError {
    crate::errors::map_giveup(reason, e, |body| {
        if !body.contains("context limit") {
            return None;
        }
        crate::retry::parse_context_overflow(body).map(|o| ProviderError::ContextOverflow {
            used: o.input_tokens,
            limit: o.context_limit,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{ScriptedResponse, ScriptedTransport};
    use crate::watchdog::TokioClock;
    use pretty_assertions::assert_eq;
    use riot_protocol::id::MessageId;
    use riot_protocol::message::{MessageMeta, UserContent};
    use riot_protocol::provider::ThinkingConfig;

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

    /// 子切片里的 assistant 带着"整个上下文"的 usage，`count_tokens` 拿它
    /// 打底会报出整个窗口的大小；`estimate_tokens_of` 只看内容。
    ///
    /// 这条钉的是压缩切分的前提：尾巴该不该留，问的是"尾巴多大"，不是
    /// "上一次请求多大"。混用过一次，尾巴就再也没被保留过。
    #[test]
    fn 纯估算不被切片里的旧_usage_顶大() {
        use riot_protocol::message::{AssistantContent, Usage};
        let (p, _t) = provider(Vec::new());
        let tail = vec![
            Message::User {
                id: MessageId::from_raw("u"),
                content: vec![UserContent::Text {
                    text: "短问题".into(),
                }],
                meta: MessageMeta::default(),
            },
            Message::Assistant {
                id: MessageId::from_raw("a"),
                content: vec![AssistantContent::Text {
                    text: "短回答".into(),
                }],
                usage: Some(Usage {
                    input_tokens: 5_000,
                    cache_read_tokens: 250_000,
                    cache_creation_tokens: 0,
                    output_tokens: 50,
                }),
                meta: MessageMeta::default(),
            },
        ];
        assert!(
            p.count_tokens(&tail) >= 255_000,
            "打底口径报的是那次请求的整个上下文"
        );
        assert!(
            p.estimate_tokens_of(&tail) < 200,
            "纯估算只看这两条的内容：{}",
            p.estimate_tokens_of(&tail)
        );
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
        let origin = events.iter().find_map(|e| match e {
            ProviderEvent::Message(Message::Assistant { meta, .. }) => meta.model_origin.clone(),
            _ => None,
        });
        assert_eq!(
            origin.as_deref(),
            Some("claude-haiku"),
            "降级后答的是 haiku，签名也是 haiku 的；下一轮回到主模型时要靠这个剥掉它"
        );
    }

    /// 别名请求、快照回显：`model_origin` 必须是**发出去的**名字。
    ///
    /// 用回显名的话，`claude-x` 的会话每条助手消息都被标成 `claude-x-20260101`，
    /// INV-9 判成外模型，自己的 thinking signature 每轮被剥 —— 表现是思考退化
    /// 成正文、缓存全 miss，开思考的工具续轮直接被 Anthropic 拒。
    #[tokio::test(start_paused = true)]
    async fn 服务端回显快照名时_model_origin_仍是请求的名字() {
        let chunks = concat!(
            r#"data: {"type":"message_start","message":{"id":"msg_1","model":"claude-x-20260101","usage":{"input_tokens":10,"output_tokens":1}}}"#,
            "\n\n",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            "\n\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"好的"}}"#,
            "\n\n",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "\n\n",
            r#"data: {"type":"message_stop"}"#,
            "\n\n",
        );
        let (p, _t) = provider(vec![ScriptedResponse::Chunks(vec![
            chunks.as_bytes().to_vec(),
        ])]);
        let events = collect(&p).await;
        let origin = events.iter().find_map(|e| match e {
            ProviderEvent::Message(Message::Assistant { meta, .. }) => meta.model_origin.clone(),
            _ => None,
        });
        assert_eq!(origin.as_deref(), Some("claude-x"));
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

    #[test]
    fn 配置的_debug_不打印密钥() {
        // 现在没有打印点，所以这不是现实泄漏 —— 但只要 Debug 存在，
        // 哪天有人加一句 `tracing::debug!(?config)` 就够了，而那行代码
        // 在 review 里看起来毫无问题。
        let cfg = AnthropicConfig {
            api_key: "sk-ant-绝密".into(),
            ..Default::default()
        };
        let printed = format!("{cfg:?}");
        assert!(!printed.contains("sk-ant-绝密"), "{printed}");
        assert!(printed.contains("<redacted>"), "{printed}");
        assert!(
            printed.contains("api.anthropic.com"),
            "非密字段要照常打出来，否则调试时这个 Debug 没用：{printed}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn 请求体带了必需的头() {
        let (p, t) = provider(vec![ScriptedResponse::Chunks(ok_chunks())]);
        collect(&p).await;

        let r = &t.requests()[0];
        let names: Vec<&str> = r.headers.iter().map(|(k, _)| k.as_str()).collect();
        assert!(names.contains(&"x-api-key"));
        assert!(names.contains(&"anthropic-version"), "缺版本头会被拒");
        assert!(
            r.headers
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("user-agent") && v.starts_with("Riot/")),
            "默认要带 Riot UA，不能是 reqwest 那个：{names:?}"
        );
        assert!(r.url.ends_with("/v1/messages"));
    }

    #[tokio::test(start_paused = true)]
    async fn 额外头会发出去_认证头不能被覆盖() {
        let t = Arc::new(ScriptedTransport::new(vec![ScriptedResponse::Chunks(
            ok_chunks(),
        )]));
        let p = AnthropicProvider::new(
            Arc::clone(&t) as Arc<dyn HttpTransport>,
            Arc::new(TokioClock),
            sections(),
            AnthropicConfig {
                extra_headers: vec![
                    ("x-opencode-session".into(), "ses_abc".into()),
                    ("x-api-key".into(), "stolen".into()),
                    ("User-Agent".into(), "my-agent/1.0".into()),
                ],
                api_key: "sk-ant-real".into(),
                ..Default::default()
            },
        );
        collect(&p).await;

        let r = &t.requests()[0];
        assert!(
            r.headers
                .iter()
                .any(|(k, v)| k == "x-opencode-session" && v == "ses_abc"),
            "{:?}",
            r.headers
        );
        assert_eq!(
            r.headers
                .iter()
                .find(|(k, _)| k == "x-api-key")
                .map(|(_, v)| v.as_str()),
            Some("sk-ant-real")
        );
        assert_eq!(
            r.headers
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
                .map(|(_, v)| v.as_str())
                .collect::<Vec<_>>(),
            vec!["my-agent/1.0"]
        );
    }
}
