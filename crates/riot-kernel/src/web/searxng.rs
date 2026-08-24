//! SearXNG 后端。
//!
//! SearXNG 是一个自托管的元搜索引擎：它自己不索引，把查询转发给
//! Google/Bing/DuckDuckGo 等一堆引擎再合并结果。选它的理由很实际 ——
//! 不需要 API key、没有额度、可以整个跑在内网，而且国内能直连。
//!
//! # 需要用户在 SearXNG 那边改一处配置
//!
//! 官方 docker 镜像默认**只开 HTML 输出**，`format=json` 会被拒。这是
//! 接入 SearXNG 最常见的一个坑，所以 [`parse`] 专门认这种情况并给出
//! 改哪个文件、加哪一行 —— 让用户看到 "expected value at line 1" 这种
//! serde 报错等于让他自己去猜。

use riot_protocol::web::{SearchHit, SearchQuery, WebError};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

/// 单次搜索的超时。
///
/// 比抓网页宽松：SearXNG 要等它背后**一批**引擎都回话，慢的那个决定总时长。
const TIMEOUT_MS: u64 = 25_000;

/// 响应体上限。一页 JSON 结果撑死几百 KB，超过说明地址指错了地方。
const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    results: Vec<Raw>,
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    /// SearXNG 管摘要叫 `content`。
    #[serde(default)]
    content: String,
}

/// 查一次 SearXNG。
///
/// `client` 由调用方传入且**不带**内网拦截解析器 —— 见
/// [`crate::config::WebConfig::searxng_url`] 上的说明。
pub async fn search(
    client: &reqwest::Client,
    base_url: &str,
    q: &SearchQuery,
    cancel: &CancellationToken,
) -> Result<Vec<SearchHit>, WebError> {
    let url = format!("{}/search", base_url.trim_end_matches('/'));

    // 域名过滤交给引擎而不是自己过滤返回结果：后者拿到的是"十条里筛剩
    // 两条"，前者拿到的是"符合条件的十条"。下面 `retain` 那步只是兜底，
    // 因为不是每个引擎都认 site: 语法。
    let mut query = q.query.clone();
    for d in &q.allowed_domains {
        query.push_str(&format!(" site:{d}"));
    }
    for d in &q.blocked_domains {
        query.push_str(&format!(" -site:{d}"));
    }

    #[allow(clippy::disallowed_methods)] // 等外部服务，真实时钟
    let send = client
        .get(&url)
        .query(&[
            ("q", query.as_str()),
            ("format", "json"),
            // `all` = 不限语言。
            //
            // `[约束]` 不能用 SearXNG 的默认值 `auto` —— 它的含义是
            // "从浏览器信息推断"，实际读的是 `Accept-Language` 请求头。
            // 我们不是浏览器，不发那个头，于是 `auto` 没有依据，各实例
            // 的回退行为不一样。实测同一个查询在 `auto` 下拿到的结果
            // 和查询词毫无关系。
            //
            // 也不锁成 zh：技术问题的答案八成在英文页面上，锁中文会把
            // 官方文档排到后面去。
            ("language", "all"),
            ("safesearch", "0"),
        ])
        // SearXNG 会挡掉一部分它认为是爬虫的 UA。
        .header(reqwest::header::USER_AGENT, super::USER_AGENT)
        .header(reqwest::header::ACCEPT, "application/json")
        .timeout(std::time::Duration::from_millis(TIMEOUT_MS))
        .send();

    let resp = tokio::select! {
        r = send => r.map_err(|e| WebError::Transport { message: transport_hint(&e, base_url) })?,
        _ = cancel.cancelled() => return Err(WebError::Cancelled),
    };

    let status = resp.status();
    let body = tokio::select! {
        b = resp.text() => b.map_err(|e| WebError::Transport { message: e.to_string() })?,
        _ = cancel.cancelled() => return Err(WebError::Cancelled),
    };

    if !status.is_success() {
        return Err(WebError::Status {
            code: status.as_u16(),
            body: status_hint(status.as_u16(), &body),
        });
    }
    if body.len() > MAX_BYTES {
        return Err(WebError::TooLarge {
            limit: MAX_BYTES as u64,
        });
    }

    let mut hits = parse(&body)?;
    hits.truncate(q.max_results);
    retain_by_domain(&mut hits, q);
    Ok(hits)
}

/// 解析响应体，顺带认出"JSON 输出没开"这一种失败。
fn parse(body: &str) -> Result<Vec<SearchHit>, WebError> {
    let env: Envelope = serde_json::from_str(body).map_err(|e| {
        // 拿到 HTML 说明实例活着但没开 JSON 格式，这是最常见的情况，
        // 值得一条能照着做的提示而不是 serde 的原始报错。
        if body.trim_start().starts_with('<') {
            return WebError::Status {
                code: 200,
                body: "SearXNG 返回的是 HTML 而不是 JSON。\
                       在实例的 settings.yml 里加上：\n\
                       search:\n  formats:\n    - html\n    - json\n\
                       然后重启 SearXNG。"
                    .to_owned(),
            };
        }
        WebError::Transport {
            message: format!("看不懂 SearXNG 的响应：{e}"),
        }
    })?;

    Ok(env
        .results
        .into_iter()
        // 没有 URL 的结果对模型毫无用处 —— 它既不能引用也不能抓。
        .filter(|r| !r.url.trim().is_empty())
        .map(|r| SearchHit {
            title: if r.title.trim().is_empty() {
                r.url.clone()
            } else {
                r.title
            },
            url: r.url,
            snippet: r.content,
            // SearXNG 只给摘要。正文由模型选中之后用 WebFetch 抓。
            raw_content: None,
        })
        .collect())
}

/// `site:` 语法的兜底。不是每个上游引擎都认它。
fn retain_by_domain(hits: &mut Vec<SearchHit>, q: &SearchQuery) {
    if !q.allowed_domains.is_empty() {
        hits.retain(|h| q.allowed_domains.iter().any(|d| host_matches(&h.url, d)));
    }
    if !q.blocked_domains.is_empty() {
        hits.retain(|h| !q.blocked_domains.iter().any(|d| host_matches(&h.url, d)));
    }
}

/// URL 的主机是不是 `domain` 或它的子域。
///
/// 后缀匹配必须卡在点上：`evil-github.com` 不是 `github.com` 的子域，
/// 而裸 `ends_with` 会说它是。
fn host_matches(url: &str, domain: &str) -> bool {
    let d = domain.trim().trim_start_matches('.').to_ascii_lowercase();
    if d.is_empty() {
        return false;
    }
    let Some(host) = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url)
        .split(['/', '?', '#'])
        .next()
        .map(|h| h.rsplit('@').next().unwrap_or(h))
        .map(|h| h.split(':').next().unwrap_or(h).to_ascii_lowercase())
    else {
        return false;
    };
    host == d || host.ends_with(&format!(".{d}"))
}

fn status_hint(code: u16, body: &str) -> String {
    let brief: String = body.chars().take(200).collect();
    match code {
        403 => "SearXNG 拒绝了这次请求（403）。多半是实例的 settings.yml 里\
                没把 json 加进 search.formats，或者 limiter 拦了本机请求。"
            .to_owned(),
        404 => "地址下没有 /search（404）。填实例根地址就行，比如 \
                http://127.0.0.1:8080，不要带路径。"
            .to_owned(),
        429 => "SearXNG 说请求太频繁（429）。它背后的引擎在限流，等一会儿再试。".to_owned(),
        _ => brief,
    }
}

fn transport_hint(e: &reqwest::Error, base_url: &str) -> String {
    let shown = crate::config::searxng_error_label(base_url);
    if e.is_timeout() {
        return format!("连 {shown} 超时。");
    }
    if e.is_connect() {
        return format!("连不上 {shown}。确认搜索后端在跑，端口和设置里填的一致。");
    }
    crate::config::redact_searxng_url(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析正常响应() {
        let hits = parse(
            r#"{"results":[
                {"title":"Tokio","url":"https://tokio.rs","content":"异步运行时"},
                {"title":"","url":"https://docs.rs/tokio","content":""}
            ]}"#,
        )
        .expect("解析");

        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "Tokio");
        assert_eq!(hits[0].snippet, "异步运行时");
        // 没标题时拿 URL 顶上，总比在结果列表里显示一个空链接强
        assert_eq!(hits[1].title, "https://docs.rs/tokio");
    }

    #[test]
    fn 丢掉没有链接的结果() {
        // 没 URL 的结果模型既不能引用也不能抓，留着只是占位置
        let hits = parse(r#"{"results":[{"title":"x","url":"  ","content":"y"}]}"#).expect("解析");
        assert!(hits.is_empty());
    }

    #[test]
    fn 空结果不是错误() {
        assert!(parse(r#"{"results":[]}"#).expect("解析").is_empty());
        // 有些版本压根不返回 results 字段
        assert!(parse(r#"{"query":"x"}"#).expect("解析").is_empty());
    }

    #[test]
    fn 拿到html时提示去开json输出() {
        // 官方 docker 镜像默认只开 HTML。这是接入 SearXNG 最常见的坑，
        // 报 serde 的 "expected value at line 1" 等于让用户自己猜。
        let e = parse("<!DOCTYPE html><html><body>...</body></html>").expect_err("必须报错");
        let msg = e.to_string();
        assert!(msg.contains("formats"), "要告诉用户改哪里：{msg}");
        assert!(msg.contains("json"), "{msg}");
    }

    #[test]
    fn 子域名匹配卡在点上() {
        assert!(host_matches("https://docs.github.com/x", "github.com"));
        assert!(host_matches("https://github.com", "github.com"));
        assert!(host_matches("https://github.com:8443/x", "github.com"));
        // 裸 ends_with 会把这个判成子域，那是一条完整的白名单绕过
        assert!(!host_matches("https://evil-github.com/x", "github.com"));
        assert!(!host_matches("https://githubXcom/x", "github.com"));
        // 用户可能带着点填
        assert!(host_matches("https://docs.rs/a", ".docs.rs"));
    }

    #[test]
    fn 域名过滤兜底() {
        // site: 交给引擎，但不是每个上游引擎都认，所以本地还要筛一遍
        let mut hits = vec![
            SearchHit {
                title: "a".into(),
                url: "https://docs.rs/tokio".into(),
                snippet: String::new(),
                raw_content: None,
            },
            SearchHit {
                title: "b".into(),
                url: "https://spam.example/x".into(),
                snippet: String::new(),
                raw_content: None,
            },
        ];
        retain_by_domain(
            &mut hits,
            &SearchQuery {
                allowed_domains: vec!["docs.rs".into()],
                ..Default::default()
            },
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://docs.rs/tokio");
    }

    #[test]
    fn 常见状态码给的是能照着做的提示() {
        for (code, want) in [(403u16, "formats"), (404, "根地址"), (429, "限流")] {
            let h = status_hint(code, "");
            assert!(h.contains(want), "HTTP {code} 的提示没说到点上：{h}");
        }
    }
}
