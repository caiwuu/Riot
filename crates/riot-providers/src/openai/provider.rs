//! OpenAI 兼容 Provider。
//!
//! DeepSeek、Kimi、Qwen、vLLM、Ollama、OpenRouter 都是这套接口，换的只是
//! base URL 和模型名。
//!
//! 重试、退避、看门狗全部复用 Anthropic 那边的实现 —— 那些逻辑跟具体
//! 厂商的报文格式无关，只跟 HTTP 状态码有关。这一层只负责报文形状。

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
use super::request::{RetryContext, build_request, wire_bytes};
use crate::anthropic::request::SystemSection;
use crate::retry::{GiveUpReason, RequestSource, RetryDecision, RetryPolicy, decide};
use crate::sse::SseParser;
use crate::transport::{ByteStream, HttpError, HttpRequest, HttpTransport};
use crate::watchdog::{DEFAULT_IDLE, with_idle_watchdog};

#[derive(Clone)]
pub struct OpenAiConfig {
    /// 不带路径，例如 `https://api.deepseek.com`。
    pub base_url: String,
    /// 对话接口的路径。空 = 按 base 猜（见 [`crate::endpoint::api_url`]）。
    ///
    /// 可配置的理由:各家的根路径对不上，猜不全。智谱的对话在
    /// `/api/paas/v4/chat/completions`，中转和自建网关的花样更多。
    pub api_path: String,
    pub api_key: String,
    /// 连续过载时切过去的模型。
    pub fallback_model: Option<String>,
    pub idle_timeout: Duration,
    pub retry: RetryPolicy,
    /// 采样参数。top_k 在这个协议下**不发送**，见 [`crate::SamplingParams`]。
    pub sampling: crate::SamplingParams,
    /// 用户配置的额外请求头（模板已在宿主展开）。
    pub extra_headers: Vec<(String, String)>,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.deepseek.com".into(),
            // 空 = 按 base 猜。默认不写死路径:写死之后"用户没配"和
            // "用户配的正好等于默认值"就分不开了。
            api_path: String::new(),
            api_key: String::new(),
            fallback_model: None,
            idle_timeout: DEFAULT_IDLE,
            retry: RetryPolicy::default(),
            sampling: crate::SamplingParams::default(),
            extra_headers: Vec::new(),
        }
    }
}

/// `[约束]` 手写而不是 derive：这个结构体里有明文 API key，而 `Debug`
/// 只要存在，任何一处 `tracing::debug!(?config)` 就会把密钥写进日志文件 ——
/// 日志会被用户贴进 issue，密钥就此公开。
///
/// 字段有增删时这里要跟着改，代价是记得住的：漏掉一个非密字段只是少打
/// 一行，而把 `api_key` 加回去需要有人主动写出那个字段名。
impl std::fmt::Debug for OpenAiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiConfig")
            .field("base_url", &self.base_url)
            .field("api_path", &self.api_path)
            .field("api_key", &"<redacted>")
            .field("fallback_model", &self.fallback_model)
            .field("idle_timeout", &self.idle_timeout)
            .field("retry", &self.retry)
            .field("sampling", &self.sampling)
            .field("extra_headers", &self.extra_headers)
            .finish()
    }
}

impl OpenAiConfig {
    pub fn deepseek(api_key: impl Into<String>) -> Self {
        Self {
            base_url: "https://api.deepseek.com".into(),
            api_key: api_key.into(),
            ..Default::default()
        }
    }
}

const OVERLOAD_BEFORE_FALLBACK: u32 = 3;

pub struct OpenAiProvider {
    transport: Arc<dyn HttpTransport>,
    clock: Arc<dyn Clock>,
    system: Vec<SystemSection>,
    config: OpenAiConfig,
    source: RequestSource,
}

impl OpenAiProvider {
    pub fn new(
        transport: Arc<dyn HttpTransport>,
        clock: Arc<dyn Clock>,
        system: Vec<SystemSection>,
        config: OpenAiConfig,
    ) -> Self {
        Self {
            transport,
            clock,
            system,
            config,
            source: RequestSource::Foreground,
        }
    }

    pub fn as_background(mut self) -> Self {
        self.source = RequestSource::Background;
        self
    }
}

#[derive(Clone)]
struct Endpoint {
    base_url: String,
    api_path: String,
    api_key: String,
    extra_headers: Vec<(String, String)>,
}

fn build_http_request(
    wire: &super::wire::WireRequest,
    endpoint: &Endpoint,
) -> Result<HttpRequest, HttpError> {
    let body = serde_json::to_vec(wire)
        .map_err(|e| HttpError::transport(format!("failed to serialize request: {e}")))?;

    Ok(HttpRequest {
        // 路径优先用用户配的；空着才按 base 猜。见 endpoint 模块。
        url: crate::endpoint::api_url_with(
            &endpoint.base_url,
            &endpoint.api_path,
            "v1",
            "chat/completions",
        ),
        headers: crate::headers::merge_headers(
            vec![
                ("content-type".into(), "application/json".into()),
                ("accept".into(), "text/event-stream".into()),
                (
                    "authorization".into(),
                    format!("Bearer {}", endpoint.api_key),
                ),
            ],
            &endpoint.extra_headers,
        ),
        body,
    })
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn stream(&self, req: ProviderRequest, cancel: CancellationToken) -> ProviderStream {
        let transport = Arc::clone(&self.transport);
        let clock = Arc::clone(&self.clock);
        let system = self.system.clone();
        let source = self.source;
        let policy = self.config.retry;
        let idle = self.config.idle_timeout;
        let fallback_model = self.config.fallback_model.clone();
        let sampling = self.config.sampling;
        let endpoint = Endpoint {
            base_url: self.config.base_url.clone(),
            api_path: self.config.api_path.clone(),
            api_key: self.config.api_key.clone(),
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
                wire.temperature = sampling.temperature;
                wire.top_p = sampling.top_p;
                // top_k 刻意不注入：OpenAI 官方端点会以 400 拒绝未知参数
                let http_req = match build_http_request(&wire, &endpoint) {
                    Ok(r) => r,
                    Err(e) => {
                        yield ProviderEvent::Error(ProviderError::transport(e));
                        return;
                    }
                };

                // ── 请求阶段：还没吐过事件，可以重试 ──────────
                let byte_stream = match transport.post_sse(http_req, cancel.child_token()).await {
                    Ok(s) => s,
                    Err(e) => {
                        // OpenAI 系用 503 表示过载，没有 Anthropic 的 529
                        let overloaded = matches!(e.status, Some(503) | Some(529));
                        if overloaded {
                            overload_streak += 1;
                        }

                        if overloaded
                            && overload_streak >= OVERLOAD_BEFORE_FALLBACK
                            && let Some(fb) = fallback_model.clone()
                            && retry_ctx.model_override.as_deref() != Some(fb.as_str())
                        {
                            tracing::warn!(model = %fb, "连续过载，降级");
                            retry_ctx = RetryContext::fallback_to(fb);
                            overload_streak = 0;
                            attempt += 1;
                            continue 'attempts;
                        }

                        let ctx = e.failure_context(source, false, attempt);
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

                // ── 流阶段：不再重试 ─────────────────────────
                // 理由见 anthropic/provider.rs 的模块文档 —— UI 已经渲染了
                // 吐出去的内容，重试会让同一段文字出现两次。
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
        // 真实计数打底 + 其后粗估，理由同 Anthropic 那边。
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

fn decode_stream(mut bytes: ByteStream) -> impl futures_core::Stream<Item = ProviderEvent> + Send {
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
                    // 半条消息也要吐出去 —— 用户至少能看到模型说到哪了
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
/// OpenAI 系认上下文超长的那一手：400 + 特定文案。各家措辞不同，认几个
/// 最常见的；认不出来就当普通拒绝 —— 那样主循环不会尝试压缩恢复，
/// 但至少不会误判。
fn map_giveup(reason: GiveUpReason, e: &HttpError) -> ProviderError {
    crate::errors::map_giveup(reason, e, |body| {
        is_context_overflow(body).then_some(ProviderError::ContextOverflow { used: 0, limit: 0 })
    })
}

fn is_context_overflow(body: &str) -> bool {
    let b = body.to_ascii_lowercase();
    b.contains("context length")
        || b.contains("context_length_exceeded")
        || b.contains("maximum context")
        || b.contains("too long")
}

#[cfg(test)]
mod giveup_tests {
    use super::*;

    fn http(status: Option<u16>, body: &str) -> HttpError {
        HttpError {
            status,
            body: body.into(),
            transport: status.is_none(),
            ..Default::default()
        }
    }

    #[test]
    fn 参数错误映射为拒绝_不是重试耗尽() {
        // 400 根本没有重试过。报"重试耗尽"会让用户以为是网络问题，
        // 往完全错误的方向排查。
        let e = http(Some(400), r#"{"error":{"message":"bad model name"}}"#);
        match map_giveup(GiveUpReason::NotRetryable, &e) {
            ProviderError::Refused { error } => {
                assert_eq!(error.key(), "kernel.provider.refused");
                assert_eq!(error.text.args["status"], "400");
                assert!(
                    error
                        .detail
                        .as_deref()
                        .unwrap_or_default()
                        .contains("bad model name")
                );
            }
            other => panic!("400 应该是 Refused，得到 {other:?}"),
        }
    }

    #[test]
    fn 没有状态码的放弃仍然是传输错误() {
        let e = http(None, "");
        assert!(matches!(
            map_giveup(GiveUpReason::NotRetryable, &e),
            ProviderError::Transport { .. }
        ));
    }

    #[test]
    fn 配置的_debug_不打印密钥() {
        // 现在没有打印点，所以这不是现实泄漏 —— 但只要 Debug 存在，
        // 哪天有人加一句 `tracing::debug!(?config)` 就够了，而那行代码
        // 在 review 里看起来毫无问题。
        let cfg = OpenAiConfig::deepseek("sk-绝密");
        let printed = format!("{cfg:?}");
        assert!(!printed.contains("sk-绝密"), "{printed}");
        assert!(printed.contains("<redacted>"), "{printed}");
        assert!(
            printed.contains("api.deepseek.com"),
            "非密字段要照常打出来，否则调试时这个 Debug 没用：{printed}"
        );
    }

    #[test]
    fn 上下文超长的_400_仍然可恢复() {
        let e = http(Some(400), "This model's maximum context length is 65536");
        assert!(matches!(
            map_giveup(GiveUpReason::NotRetryable, &e),
            ProviderError::ContextOverflow { .. }
        ));
    }
}

#[cfg(test)]
mod header_tests {
    use std::sync::Arc;

    use futures::StreamExt;
    use riot_protocol::id::MessageId;
    use riot_protocol::message::{Message, MessageMeta, UserContent};
    use riot_protocol::provider::{Provider, ProviderRequest, ThinkingConfig};
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::transport::{ScriptedResponse, ScriptedTransport};
    use crate::watchdog::TokioClock;

    fn ping() -> ProviderRequest {
        ProviderRequest {
            model: "m".into(),
            messages: vec![Message::User {
                id: MessageId::from_raw("u1"),
                content: vec![UserContent::Text {
                    text: "ping".into(),
                }],
                meta: MessageMeta::default(),
            }],
            system: String::new(),
            tools: vec![],
            max_output_tokens: Some(16),
            thinking: ThinkingConfig::Off,
        }
    }

    #[tokio::test]
    async fn 额外头和默认_ua_会发出去() {
        let t = Arc::new(ScriptedTransport::new(vec![ScriptedResponse::Fail(
            HttpError::status(400, "no"),
        )]));
        let p = OpenAiProvider::new(
            Arc::clone(&t) as Arc<dyn HttpTransport>,
            Arc::new(TokioClock),
            Vec::new(),
            OpenAiConfig {
                extra_headers: vec![("x-opencode-session".into(), "ses_abc".into())],
                api_key: "sk-test".into(),
                ..Default::default()
            },
        );
        let _ = p
            .stream(ping(), CancellationToken::new())
            .collect::<Vec<_>>()
            .await;

        let r = &t.requests()[0];
        assert!(
            r.headers
                .iter()
                .any(|(k, v)| k == "x-opencode-session" && v == "ses_abc"),
            "{:?}",
            r.headers
        );
        assert!(
            r.headers
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("user-agent") && v.starts_with("Riot/")),
            "{:?}",
            r.headers
        );
        assert!(
            r.headers
                .iter()
                .any(|(k, v)| k == "authorization" && v == "Bearer sk-test")
        );
    }
}
