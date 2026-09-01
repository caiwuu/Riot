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
            // `[约束]` 一次重定向都不跟。
            //
            // 用户可以把 base_url 指向任意中转（`endpoint` 整个模块就是为
            // 这件事存在的），而鉴权头里有 `x-api-key`（Anthropic 侧）。
            // reqwest 默认跟随最多 10 跳，跨主机时只剥它认识的那几个头
            // （Authorization / Cookie / Proxy-Authorization）—— `x-api-key`
            // 不在那张表里。于是一个被攻陷或本身恶意的中转只要回
            // `302 https://attacker/`，密钥就原样送上门，而用户这边一切正常。
            //
            // 代价接近零：对话接口不需要重定向，真遇到 3xx 会当成非 2xx
            // 报出来（带状态码和 Location 之外的 body），用户看得见。
            .redirect(reqwest::redirect::Policy::none())
            // `[取舍]` 不开 `https_only`。本地中转、自建网关、Ollama 这类
            // 用法普遍是明文 http（`http://127.0.0.1:11434`），一刀切会把
            // 合法配置直接判死，而且报错发生在建连之前，用户只看到"连不上"。
            // 明文传输的风险由用户自己选的 base_url 承担；密钥跟着重定向
            // 跑到第三方则是我们的实现细节造成的，两者性质不同。
            //
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
    let status = e.status();
    // `[约束]` 先剥 URL 再取文案。reqwest 的 Display 会把请求 URL 整条
    // 拼进去，而少数中转把密钥放在查询串里 —— 那种配置下 URL 本身就是
    // 密钥，而这段文案会进 UI 和日志。
    let body = describe(&e.without_url());

    // 有 status 说明拿到了响应，那不是传输层问题。
    if let Some(s) = status {
        return HttpError {
            status: Some(s.as_u16()),
            body,
            ..Default::default()
        };
    }

    // 超时、连接被拒、DNS 失败、读到一半断了 —— 都是可重试的传输错误。
    // `[约束]` 这里必须置 transport=true。漏了的话 retry 层会把它当成
    // "拿到了响应但没有状态码"，走不可重试分支，于是一次网络抖动就
    // 让整轮对话失败。
    HttpError::transport(body)
}

fn describe(e: &reqwest::Error) -> String {
    // reqwest 的 Display 只说最外层，真正的原因在 source 链的末端
    // （"connection refused"、"dns error"）。对着一句 "error sending
    // request" 没人能判断该改什么。
    //
    // source 链里的底层错误（hyper / rustls / io）只带 host:port，不带
    // 查询串 —— 主机名要留着，用户排查 base URL 配错时全靠它。
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    /// 起一个只回一次响应的 TCP 服务器，返回它的地址和"收到几个连接"的计数。
    ///
    /// 用真 socket 而不是替身：要验的正是 reqwest 自己的重定向行为，
    /// 替身在这一层什么都证明不了。
    fn one_shot(response: String) -> (String, Arc<AtomicUsize>) {
        let hits = Arc::new(AtomicUsize::new(0));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑端口");
        let addr = listener.local_addr().expect("取地址");
        let hits_for_task = Arc::clone(&hits);

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { break };
                hits_for_task.fetch_add(1, Ordering::SeqCst);
                // 读掉请求头再回应，否则客户端可能先撞上 RST 而不是读到响应。
                let mut buf = [0u8; 4096];
                let _ = std::io::Read::read(&mut s, &mut buf);
                let _ = std::io::Write::write_all(&mut s, response.as_bytes());
            }
        });

        (format!("http://{addr}"), hits)
    }

    #[tokio::test]
    async fn 重定向不跟随_密钥不会送到第三方() {
        // 用户可以把 base_url 指向任意中转，而 `x-api-key` 不在 reqwest
        // 跨主机剥离的那张表里。中转只要回一个 302，密钥就跟着请求发给
        // 攻击者，本地看到的是一次完全正常的对话。
        let (attacker_url, attacker_hits) =
            one_shot("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_owned());
        let (proxy_url, proxy_hits) = one_shot(format!(
            "HTTP/1.1 302 Found\r\nLocation: {attacker_url}/v1/messages\r\nContent-Length: 0\r\n\r\n"
        ));

        let t = ReqwestTransport::new().expect("建客户端");
        let err = t
            .post_sse(
                HttpRequest {
                    url: format!("{proxy_url}/v1/messages"),
                    headers: vec![("x-api-key".into(), "sk-ant-绝密".into())],
                    body: b"{}".to_vec(),
                },
                CancellationToken::new(),
            )
            .await
            .err()
            .expect("302 不是成功响应");

        assert_eq!(err.status, Some(302), "3xx 要原样报出来，不能悄悄跟过去");
        assert_eq!(proxy_hits.load(Ordering::SeqCst), 1);
        // send() 已经返回：真跟随的话第二个请求必然已经发出去了。
        assert_eq!(
            attacker_hits.load(Ordering::SeqCst),
            0,
            "重定向目标收到了请求 —— x-api-key 已经泄漏"
        );
    }

    #[tokio::test]
    async fn 错误文案不带查询串里的密钥() {
        // 少数中转把 key 放在查询串里，那种配置下 URL 就是密钥，
        // 而这段文案会进 UI 和日志。
        let dead = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").expect("绑端口");
            l.local_addr().expect("取地址")
            // listener 在这里 drop：端口没人听，连接立刻被拒。
        };

        let t = ReqwestTransport::new().expect("建客户端");
        let err = t
            .post_sse(
                HttpRequest {
                    url: format!("http://{dead}/v1/messages?api_key=sk-绝密-在-URL-里"),
                    headers: vec![],
                    body: b"{}".to_vec(),
                },
                CancellationToken::new(),
            )
            .await
            .err()
            .expect("连不上");

        assert!(err.transport, "连接被拒是可重试的传输错误");
        assert!(
            !err.body.contains("sk-绝密"),
            "错误文案泄漏了密钥：{}",
            err.body
        );
    }
}
