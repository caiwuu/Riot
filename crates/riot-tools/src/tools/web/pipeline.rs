//! 抓取 → 转换 → 蒸馏。`WebFetch` 和 `WebSearch` 共用这条链路。
//!
//! # 重定向为什么要自己跟
//!
//! HTTP 客户端默认会自动跟随重定向，那对这里是错的。用户点"允许访问
//! docs.trusted.com"之后，如果 `docs.trusted.com/r?to=http://evil.com`
//! 这种开放重定向能被自动跟掉，那次授权就变成了对全网的授权。
//!
//! 所以：客户端一跳都不跟（[`riot_protocol::web::WebAccess`] 的约束），
//! 由这里逐跳判断。同源（含 `www.` 增减）的跳转自动跟，跨站的**不跟**，
//! 而是把目标交回模型 —— 模型重新发一次请求，那一次会重新过域名权限。

use riot_protocol::tool::ToolContext;
use riot_protocol::web::{DistillRequest, WebError, WebRequest};
use url::Url;

use super::cache::{CachedPage, PageCache};
use super::markdown::{self, MAX_CONTENT_CHARS};
use super::url as weburl;

/// 单个响应的字节上限。
///
/// `[约束]` 必须在流式读取时逐块判断，不能等下完再看长度 —— 服务端可以
/// 不报 Content-Length，那样"下完再看"等于没有上限。约束落在
/// `riot-runtime` 的实现里。
pub const MAX_HTTP_BYTES: u64 = 10 * 1024 * 1024;

/// 单跳超时。
///
/// 注意是**单跳**：重定向链上每一跳都重新计时，所以还要靠
/// [`MAX_REDIRECTS`] 限制总跳数，否则一个 `/a → /b → /a` 的循环能把
/// 工具挂到用户手动中断为止。
pub const FETCH_TIMEOUT_MS: u64 = 60_000;

/// 同源重定向的最大跳数。
pub const MAX_REDIRECTS: usize = 10;

fn user_agent() -> String {
    format!(
        "Riot/{} (+https://github.com/riot; AI coding assistant)",
        env!("CARGO_PKG_VERSION")
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    Page(CachedPage),
    /// 跨站跳转。没有自动跟随，把目标交回模型。
    CrossHost {
        from: String,
        to: String,
        status: u16,
    },
}

/// 抓一个页面并转成 Markdown。命中缓存就不发请求。
pub async fn fetch_page(
    start: &Url,
    ctx: &ToolContext,
    cache: &PageCache,
) -> Result<Fetched, WebError> {
    let cache_key = start.as_str().to_owned();
    let now = ctx.clock.now_ms();

    if let Some(hit) = cache.get(&cache_key, now) {
        return Ok(Fetched::Page(hit));
    }

    let mut current = start.clone();

    for _hop in 0..=MAX_REDIRECTS {
        if ctx.cancel.is_cancelled() {
            return Err(WebError::Cancelled);
        }

        let resp = ctx
            .web
            .get(
                WebRequest {
                    url: current.to_string(),
                    headers: vec![
                        (
                            "accept".into(),
                            "text/markdown, text/html, text/plain, */*".into(),
                        ),
                        ("user-agent".into(), user_agent()),
                        // 不接受压缩以外的编码协商。让客户端自己处理 gzip，
                        // 这里拿到的是解压后的字节。
                        ("accept-language".into(), "zh-CN,zh;q=0.9,en;q=0.8".into()),
                    ],
                    max_bytes: MAX_HTTP_BYTES,
                    timeout_ms: FETCH_TIMEOUT_MS,
                },
                &ctx.cancel,
            )
            .await?;

        if resp.is_redirect() {
            let Some(loc) = resp.location.as_deref() else {
                return Err(WebError::Transport {
                    message: format!("{} 重定向但没有 Location 头", resp.status),
                });
            };

            // 跳转目标要重新过一遍准入检查。同源判断保证不了协议和端口
            // 没变，而 `https://a.com → http://a.com` 这种降级不该被跟。
            let next = weburl::normalize(loc).map_err(|e| WebError::Blocked {
                reason: format!("重定向目标 {loc} 被拒：{e}"),
            })?;

            if weburl::is_permitted_redirect(&current, &next) {
                current = next;
                continue;
            }

            return Ok(Fetched::CrossHost {
                from: current.to_string(),
                to: next.to_string(),
                status: resp.status,
            });
        }

        if !resp.is_success() {
            return Err(WebError::Status {
                code: resp.status,
                body: first_line(&String::from_utf8_lossy(&resp.body)),
            });
        }

        let raw_bytes = resp.body.len() as u64;
        let text = markdown::decode_body(&resp.body, &resp.content_type);
        let content = if resp.content_type.contains("html") {
            markdown::html_to_markdown(&text)
        } else {
            text
        };

        let page = CachedPage {
            content,
            content_type: resp.content_type,
            status: resp.status,
            status_text: resp.status_text,
            raw_bytes,
        };
        // 用**起始 URL** 做键，不是重定向后的地址 —— 模型下次还是会用
        // 它手上那个 URL 来问。
        cache.put(cache_key, page.clone(), now);
        return Ok(Fetched::Page(page));
    }

    Err(WebError::Blocked {
        reason: format!("重定向超过 {MAX_REDIRECTS} 跳，可能是跳转循环"),
    })
}

/// 用辅助模型按 `prompt` 提炼正文；没配辅助模型就截断后原样返回。
///
/// `[约束]` 蒸馏失败**不能**让整个工具失败。拿到未提炼的正文总比什么都
/// 拿不到强，而且模型自己也能从原文里读出答案 —— 只是费些 token。
pub async fn distill_or_truncate(
    content: &str,
    prompt: &str,
    trusted_source: bool,
    ctx: &ToolContext,
) -> String {
    let truncated = markdown::truncate(content, MAX_CONTENT_CHARS);

    let req = DistillRequest {
        system: DISTILL_SYSTEM.to_owned(),
        user: distill_prompt(&truncated, prompt, trusted_source),
        max_output_tokens: Some(4096),
    };

    match ctx.web.distill(req, &ctx.cancel).await {
        Ok(s) if !s.trim().is_empty() => s,
        Ok(_) => fallback(&truncated, "辅助模型返回了空结果"),
        Err(WebError::Cancelled) => "已取消。".to_owned(),
        Err(WebError::NotConfigured { .. }) => {
            // 没配辅助模型是常态（用户可能就想省这一次调用），不该报错。
            // 直接给原文，只是费些上下文。
            truncated
        }
        Err(e) => fallback(&truncated, &e.to_string()),
    }
}

fn fallback(truncated: &str, why: &str) -> String {
    format!("[未能用辅助模型提炼（{why}），以下是页面原文]\n\n{truncated}")
}

const DISTILL_SYSTEM: &str =
    "你在为一个编程助手提炼网页内容。只依据给出的页面内容回答，不要补充页面里没有的信息。";

fn distill_prompt(content: &str, prompt: &str, trusted_source: bool) -> String {
    // 官方文档站要保留代码示例的原样。摘要过的代码示例基本就废了 ——
    // 模型会照着一段被改写过的示例写代码，然后编译不过。
    let guide = if trusted_source {
        "完整保留相关的代码示例、配置片段和 API 签名，不要改写它们。"
    } else {
        "用自己的话概括。需要原文时用引号标出，单段引用不超过 100 字。"
    };

    format!(
        "页面内容：\n---\n{content}\n---\n\n请求：{prompt}\n\n{guide}\n\
         页面里没有相关信息时，直接说没有，不要猜。"
    )
}

fn first_line(s: &str) -> String {
    let line = s
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let max = 200;
    if line.chars().count() <= max {
        return line.to_owned();
    }
    let head: String = line.chars().take(max).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn 只取错误体的第一行() {
        // 一整页 HTML 错误页塞进 tool_result 只会挤掉真正有用的上下文
        assert_eq!(first_line("\n\n  Not Found  \nmore\nlines"), "Not Found");
        assert_eq!(first_line(""), "");
    }

    #[test]
    fn 错误体按字符截断() {
        let long = "中".repeat(500);
        let out = first_line(&long);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 201);
    }

    #[test]
    fn 可信来源的蒸馏提示要求保留代码() {
        let p = distill_prompt("内容", "怎么配置", true);
        assert!(p.contains("完整保留"), "{p}");
        // 文档站被摘要过的代码示例会让模型写出编译不过的代码
        assert!(!p.contains("不超过 100 字"), "{p}");
    }

    #[test]
    fn 普通来源的蒸馏提示限制原文引用() {
        let p = distill_prompt("内容", "讲了什么", false);
        assert!(p.contains("不超过 100 字"), "{p}");
    }

    #[test]
    fn user_agent_里带版本() {
        let ua = user_agent();
        assert!(ua.starts_with("Riot/"), "{ua}");
        assert!(ua.contains(env!("CARGO_PKG_VERSION")), "{ua}");
    }
}
