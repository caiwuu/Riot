//! 真实的网页抓取客户端。
//!
//! 这一层只做三件工具层做不到的事：
//!
//! 1. 真的发请求；
//! 2. **在连接前拒绝解析到内网的主机** —— DNS 解析只有这里看得见；
//! 3. 边下边判大小上限。
//!
//! URL 准入、重定向策略、HTML 转换全在 `riot-tools` 里，那些是纯函数。
//!
//! # SSRF 防护为什么必须在这里
//!
//! 工具层能拦住 `http://169.254.169.254/`（字面量地址），拦不住
//! `http://metadata.attacker.com/`（解析到 169.254.169.254 的域名）。
//! Claude Code 靠调 `api.anthropic.com/api/web/domain_info` 做域名黑名单
//! 预检来兜这一层，我们没有那个服务，只能在本地做。
//!
//! 做法是给 reqwest 换一个自定义 DNS 解析器：解析出来的地址逐个过筛，
//! 全部是内网就直接失败。**关键是让筛选发生在解析器里而不是解析器之外** ——
//! 在外面先 `lookup_host` 查一遍再让 reqwest 自己查第二遍的话，两次查询
//! 之间结果可以变（DNS rebinding），筛过的和连上的不是同一个地址。
//!
//! # 光有解析器不够
//!
//! `[约束]` 主机是 **IP 字面量**（`http://127.0.0.1/`）时 reqwest 根本
//! 不会调解析器 —— 没有域名要解析，它直接连。所以 [`reject_private_literal`]
//! 还要在发请求前单独查一道。
//!
//! 这两道各管各的一半，缺哪一半都是完整的绕过路径。工具层的 URL 准入
//! 也拦字面量，但那是**另一层**：这一层不能依赖上一层做过检查，
//! 否则哪天多一个调用方（比如设置页的连通性测试）就漏了。

#![allow(clippy::disallowed_methods)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use riot_protocol::web::{WebError, WebRequest, WebResponse};
use tokio_util::sync::CancellationToken;

/// 建立连接的超时。整体超时由 [`WebRequest::timeout_ms`] 控制。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

pub struct SystemWebClient {
    client: reqwest::Client,
}

impl SystemWebClient {
    pub fn new() -> Result<Self, WebError> {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .pool_idle_timeout(Duration::from_secs(30))
            // `[约束]` 一跳都不跟。跟随策略在工具层，见
            // `riot_tools::tools::web::pipeline` 顶部的说明。
            .redirect(reqwest::redirect::Policy::none())
            .dns_resolver(Arc::new(PublicOnlyResolver))
            // 站点常见的 TLS 配置问题不该变成"抓不了"，但证书错误必须报错。
            // 这里不放宽任何校验，只是显式写出来防止以后有人图省事加上。
            .https_only(false)
            .build()
            .map_err(|e| WebError::Transport {
                message: format!("初始化 HTTP 客户端失败：{e}"),
            })?;
        Ok(Self { client })
    }
}

#[async_trait]
impl riot_protocol::web::WebAccess for SystemWebClient {
    async fn get(
        &self,
        req: WebRequest,
        cancel: &CancellationToken,
    ) -> Result<WebResponse, WebError> {
        reject_private_literal(&req.url)?;

        let mut builder = self
            .client
            .get(&req.url)
            .timeout(Duration::from_millis(req.timeout_ms));
        for (k, v) in &req.headers {
            builder = builder.header(k, v);
        }

        let resp = tokio::select! {
            r = builder.send() => r.map_err(to_web_error)?,
            _ = cancel.cancelled() => return Err(WebError::Cancelled),
        };

        let status = resp.status();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();

        // Location 在这里解析成绝对地址。相对跳转（`Location: /foo`）很常见，
        // 让工具层再解析一次就得给它一个 URL 库和请求 URL 的上下文。
        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|loc| resp.url().join(loc).ok())
            .map(|u| u.to_string());

        let body = read_capped(resp, req.max_bytes, cancel).await?;

        Ok(WebResponse {
            status: status.as_u16(),
            status_text: status.canonical_reason().unwrap_or("").to_owned(),
            content_type,
            body,
            location,
        })
    }

    async fn search(
        &self,
        _query: riot_protocol::web::SearchQuery,
        _cancel: &CancellationToken,
    ) -> Result<Vec<riot_protocol::web::SearchHit>, WebError> {
        // 搜索后端要读用户配置，那是宿主的事。这个客户端只管抓取。
        Err(WebError::NotConfigured {
            what: "搜索后端".to_owned(),
        })
    }

    async fn distill(
        &self,
        _req: riot_protocol::web::DistillRequest,
        _cancel: &CancellationToken,
    ) -> Result<String, WebError> {
        Err(WebError::NotConfigured {
            what: "辅助模型".to_owned(),
        })
    }
}

/// 边下边判上限。
///
/// `[约束]` 不能等下完再看长度 —— 服务端可以不报 Content-Length，
/// 甚至可以故意无限吐（zip bomb 的网络版）。那样"下完再看"等于没有上限。
async fn read_capped(
    mut resp: reqwest::Response,
    max_bytes: u64,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, WebError> {
    // Content-Length 说超了就直接放弃，省掉整次传输。它可能撒谎，
    // 所以下面的逐块检查不能省。
    if resp.content_length().is_some_and(|n| n > max_bytes) {
        return Err(WebError::TooLarge { limit: max_bytes });
    }

    let mut buf: Vec<u8> = Vec::with_capacity(16 * 1024);
    loop {
        let chunk = tokio::select! {
            c = resp.chunk() => c.map_err(to_web_error)?,
            _ = cancel.cancelled() => return Err(WebError::Cancelled),
        };
        let Some(chunk) = chunk else { break };

        if buf.len() as u64 + chunk.len() as u64 > max_bytes {
            return Err(WebError::TooLarge { limit: max_bytes });
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

fn to_web_error(e: reqwest::Error) -> WebError {
    if e.is_timeout() {
        return WebError::Transport {
            message: "请求超时".to_owned(),
        };
    }
    if let Some(status) = e.status() {
        return WebError::Status {
            code: status.as_u16(),
            body: e.to_string(),
        };
    }
    // 自定义解析器返回的拒绝理由会包在连接错误里。捞出来，否则用户看到的
    // 是"connection error"而不是"这个地址指向内网"。
    let msg = chain_message(&e);
    if msg.contains(BLOCKED_MARKER) {
        return WebError::Blocked { reason: msg };
    }
    WebError::Transport { message: msg }
}

/// reqwest 的错误只在最外层说 "error sending request"，真正的原因在 source 链里。
fn chain_message(e: &reqwest::Error) -> String {
    let mut parts = vec![e.to_string()];
    let mut src: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(e);
    while let Some(s) = src {
        parts.push(s.to_string());
        src = s.source();
    }
    parts.join("：")
}

const BLOCKED_MARKER: &str = "指向内网地址";

/// 主机写成 IP 字面量时的内网检查。
///
/// `[约束]` 这道检查不能省。reqwest 只对**域名**调自定义解析器，
/// `http://169.254.169.254/latest/meta-data/` 里没有域名可解析，
/// 它会直接连过去，[`PublicOnlyResolver`] 一次都不会被调用。
///
/// URL 解析不了时放行：那不是这个函数该管的事，交给 reqwest 去报
/// 一个说得清楚的地址错误。
fn reject_private_literal(url: &str) -> Result<(), WebError> {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return Ok(());
    };
    // Url::host() 已经把 `[::1]` 的方括号剥掉了，字符串再 parse 一遍
    // 会失败 —— 所以这里认它给出的 Host 枚举，不要自己切字符串。
    let ip = match parsed.host() {
        Some(url::Host::Ipv4(v4)) => std::net::IpAddr::V4(v4),
        Some(url::Host::Ipv6(v6)) => std::net::IpAddr::V6(v6),
        // 域名走解析器那条路。
        _ => return Ok(()),
    };

    if riot_protocol::web::is_private_ip(ip) {
        return Err(WebError::Blocked {
            reason: format!("{ip} {BLOCKED_MARKER}"),
        });
    }
    Ok(())
}

// ────────────────────────────────────────────────────────────
// 只解析到公网地址的 DNS 解析器
// ────────────────────────────────────────────────────────────

struct PublicOnlyResolver;

impl reqwest::dns::Resolve for PublicOnlyResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let host = name.as_str().to_owned();
            // 端口 0 只是为了凑成 lookup_host 要的形式，真正的端口由 reqwest 填。
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::other(format!("解析 {host} 失败：{e}")))
                })?
                .collect();

            if addrs.is_empty() {
                return Err(Box::new(std::io::Error::other(format!(
                    "{host} 没有解析到任何地址"
                ))) as Box<dyn std::error::Error + Send + Sync>);
            }

            // 全部过筛。只要有一个内网地址就整体拒绝，而不是"挑公网的连" ——
            // 一个域名同时解析到公网和内网，本身就是 DNS rebinding 的形状。
            let public: Vec<SocketAddr> = addrs
                .iter()
                .copied()
                .filter(|a| !riot_protocol::web::is_private_ip(a.ip()))
                .collect();

            if public.len() != addrs.len() {
                return Err(Box::new(std::io::Error::other(format!(
                    "{host} {BLOCKED_MARKER}（{}）",
                    addrs
                        .iter()
                        .map(|a| a.ip().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))) as Box<dyn std::error::Error + Send + Sync>);
            }

            Ok(Box::new(public.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use riot_protocol::web::WebAccess;

    #[tokio::test]
    async fn 解析到本机的域名会被拒() {
        // localhost 一定解析到 127.0.0.1 / ::1，不需要联网就能测到这条路径。
        // 注意 URL 准入层会先拦掉单段主机名，所以这里直接调客户端绕过它 ——
        // 测的就是"万一准入层被绕过，解析器这层还在不在"。
        let c = SystemWebClient::new().expect("构造客户端");
        let err = c
            .get(
                WebRequest {
                    url: "http://localhost:9/".to_owned(),
                    headers: vec![],
                    max_bytes: 1024,
                    timeout_ms: 3000,
                },
                &CancellationToken::new(),
            )
            .await
            .expect_err("解析到本机的地址必须被拒");

        assert!(
            matches!(err, WebError::Blocked { .. }),
            "应当明确报『已拦截』而不是笼统的连接失败，实际：{err:?}"
        );
    }

    #[tokio::test]
    async fn 字面量内网地址会被拒() {
        // reqwest 对 IP 字面量根本不调自定义解析器 —— 没有域名要解析，
        // 它直接连。所以这条路径靠的是发请求前那道单独的检查，不是
        // PublicOnlyResolver。少了它，云上的元数据服务就是敞开的。
        let c = SystemWebClient::new().expect("构造客户端");
        for url in [
            "http://169.254.169.254/latest/meta-data/",
            "http://127.0.0.1:9/",
            "http://10.0.0.1/",
            "http://[::1]:9/",
            "http://[::ffff:169.254.169.254]/",
        ] {
            let err = c
                .get(
                    WebRequest {
                        url: url.to_owned(),
                        headers: vec![],
                        max_bytes: 1024,
                        timeout_ms: 3000,
                    },
                    &CancellationToken::new(),
                )
                .await
                .unwrap_err();
            assert!(
                matches!(err, WebError::Blocked { .. }),
                "{url} 必须被拦，实际：{err:?}"
            );
        }
    }

    #[test]
    fn 公网字面量和域名照常放行() {
        // 拦得太宽的后果同样严重，只是表现成"什么都抓不了"
        for url in ["http://1.1.1.1/", "https://example.com/", "https://[2606:4700::1111]/"] {
            assert!(reject_private_literal(url).is_ok(), "{url} 不该被拦");
        }
        // 解析不了的地址不归这个函数管，交给 reqwest 去报错
        assert!(reject_private_literal("不是个地址").is_ok());
    }

    #[tokio::test]
    async fn 取消能立刻生效() {
        let c = SystemWebClient::new().expect("构造客户端");
        let cancel = CancellationToken::new();
        cancel.cancel();

        let err = c
            .get(
                WebRequest {
                    url: "https://example.com/".to_owned(),
                    headers: vec![],
                    max_bytes: 1024,
                    timeout_ms: 30_000,
                },
                &cancel,
            )
            .await
            .expect_err("已取消的请求不该继续");
        assert_eq!(err, WebError::Cancelled);
    }

    #[test]
    fn 搜索与蒸馏在这一层未配置() {
        // 这个客户端只管抓取。搜索后端和辅助模型要读用户配置，是宿主的事。
        // 忘了在宿主里接上的话，用户会看到"尚未配置"而不是静默失败。
        let c = SystemWebClient::new().expect("构造客户端");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("建 runtime");
        let cancel = CancellationToken::new();

        let e = rt.block_on(c.search(Default::default(), &cancel));
        assert!(matches!(e, Err(WebError::NotConfigured { .. })));
    }
}
