//! 真实的 HTTP 客户端。
//!
//! 这个文件的全部职责是**把 reqwest 的错误翻译成 [`HttpError`] 的字段**。
//! 重试决策看的是那些字段，翻译漏了什么，重试就会做错决定 —— 而那种错误
//! 不会有报错，只会表现为"偶尔多重试几次"或者"该退避的时候没退避"。

use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::transport::{ByteStream, HttpError, HttpRequest, HttpTransport};

/// 建立连接的超时。
///
/// `[约束]` 只设连接超时，**不设整体超时**。流式响应本来就可能持续几分钟，
/// 加个总超时等于给长回答判死刑。"流建立之后卡住"由 idle watchdog 负责
/// （见 `watchdog.rs`），它看的是两个事件之间的间隔，不是总时长。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> Result<Self, HttpError> {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            // 连接池。每轮对话都是一次新请求，复用连接省掉 TLS 握手 ——
            // 在多轮工具调用的会话里这个差别很明显。
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .map_err(|e| HttpError::transport(format!("初始化 HTTP 客户端失败：{e}")))?;
        Ok(Self { client })
    }
}

#[async_trait]
impl HttpTransport for ReqwestTransport {
    async fn post_sse(
        &self,
        req: HttpRequest,
        cancel: CancellationToken,
    ) -> Result<ByteStream, HttpError> {
        let mut builder = self.client.post(&req.url).body(req.body);
        for (k, v) in &req.headers {
            builder = builder.header(k, v);
        }

        let sending = builder.send();
        let resp = tokio::select! {
            r = sending => r.map_err(from_reqwest)?,
            _ = cancel.cancelled() => {
                return Err(HttpError::transport("请求已取消"));
            }
        };

        let status = resp.status();
        if !status.is_success() {
            // 非 2xx 的 body 里有服务端给的原因（配额、模型名写错、
            // 内容策略），必须读出来。丢掉它的话用户看到的只有一个数字。
            let retry_after = header_u64(&resp, "retry-after");
            let should_retry = header_bool(&resp, "x-should-retry");
            let body = resp.text().await.unwrap_or_default();

            return Err(HttpError {
                status: Some(status.as_u16()),
                retry_after_secs: retry_after,
                x_should_retry: should_retry,
                body,
                transport: false,
            });
        }

        let bytes = resp.bytes_stream();
        let stream = async_stream::stream! {
            futures::pin_mut!(bytes);
            loop {
                tokio::select! {
                    next = bytes.next() => match next {
                        Some(Ok(chunk)) => yield Ok(chunk.to_vec()),
                        // 流建立之后断开。这跟请求阶段失败的可重试性不同，
                        // 所以走 stream item 的 Err 而不是返回值的 Err。
                        Some(Err(e)) => {
                            yield Err(from_reqwest(e));
                            return;
                        }
                        None => return,
                    },
                    _ = cancel.cancelled() => {
                        // 直接结束流。reqwest 的响应体在这里被 drop，
                        // 底层连接会被中止 —— 不用额外做什么。
                        return;
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }
}

fn from_reqwest(e: reqwest::Error) -> HttpError {
    // 有 status 说明拿到了响应，那不是传输层问题。
    if let Some(s) = e.status() {
        return HttpError {
            status: Some(s.as_u16()),
            body: e.to_string(),
            ..Default::default()
        };
    }

    // 超时、连接被拒、DNS 失败、读到一半断了 —— 都是可重试的传输错误。
    // `[约束]` 这里必须置 transport=true。漏了的话 retry 层会把它当成
    // "拿到了响应但没有状态码"，走不可重试分支，于是一次网络抖动就
    // 让整轮对话失败。
    HttpError::transport(describe(&e))
}

fn describe(e: &reqwest::Error) -> String {
    // reqwest 的 Display 只说最外层，真正的原因在 source 链的末端
    // （"connection refused"、"dns error"）。对着一句 "error sending
    // request" 没人能判断该改什么。
    let mut parts = vec![e.to_string()];
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        parts.push(s.to_string());
        src = s.source();
    }
    if e.is_timeout() {
        parts.push("连接超时".to_owned());
    }
    if e.is_connect() {
        parts.push("无法建立连接，请检查网络或 base URL".to_owned());
    }
    parts.join("：")
}

fn header_u64(resp: &reqwest::Response, name: &str) -> Option<u64> {
    resp.headers()
        .get(name)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

fn header_bool(resp: &reqwest::Response, name: &str) -> Option<bool> {
    let raw = resp.headers().get(name)?.to_str().ok()?.trim();
    match raw {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 客户端能建起来() {
        assert!(ReqwestTransport::new().is_ok());
    }

    #[tokio::test]
    async fn 连不上的地址报传输错误() {
        // `[约束]` transport=true 决定了这个错误是可重试的。置错的话
        // 一次网络抖动就让整轮对话失败。
        let t = ReqwestTransport::new().expect("建客户端");
        let err = t
            .post_sse(
                HttpRequest {
                    // 保留给文档示例的 TEST-NET-1，不会有人真的监听
                    url: "http://192.0.2.1:9/v1/x".into(),
                    headers: vec![],
                    body: b"{}".to_vec(),
                },
                CancellationToken::new(),
            )
            .await
            .err()
            .expect("应该连不上");

        assert!(err.transport, "必须标成传输错误，否则不会重试");
        assert!(err.status.is_none());
    }

    #[tokio::test]
    async fn 取消后立即返回() {
        let t = ReqwestTransport::new().expect("建客户端");
        let cancel = CancellationToken::new();
        cancel.cancel();

        let err = t
            .post_sse(
                HttpRequest {
                    url: "http://192.0.2.1:9/v1/x".into(),
                    headers: vec![],
                    body: b"{}".to_vec(),
                },
                cancel,
            )
            .await
            .err()
            .expect("已取消");

        assert!(err.body.contains("取消"));
    }
}
