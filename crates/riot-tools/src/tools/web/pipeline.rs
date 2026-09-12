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

/// 抓回来的一张图。字节是原始响应体，没压缩、没编码。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedImage {
    /// 规范化后的 MIME（小写、去掉参数），如 `image/png`。
    ///
    /// 这里只负责认出"这是一张图"；模型收不收这种类型由工具层判断 ——
    /// 那份清单要和 Read 保持一致，不该在两处各写一遍。
    pub media_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    Page(CachedPage),
    /// 图片。不转 Markdown、不蒸馏，也不进缓存 —— 缓存按转成 Markdown
    /// 的正文计量，装不下二进制；重抓一张图的代价也远小于重抓一页。
    Image(FetchedImage),
    /// 跨站跳转。没有自动跟随，把目标交回模型。
    CrossHost {
        from: String,
        to: String,
        status: u16,
    },
}

/// 响应是不是一张图。返回规范化的 MIME。
///
/// 先看 content-type；服务端没给、或者给的是笼统的 `application/octet-stream`
/// 时（CDN 上的静态文件常这样），再看文件头。四种格式的魔数都很稳。
///
/// `[约束]` content-type 明确是别的类型（`text/html`）时**不嗅探**：那是
/// 服务端的表态，一个碰巧以 PNG 魔数开头的正文按图处理只会更糟。
///
/// svg 不算图：它是 XML 文本，视觉模型也不收，走正文那条路让模型读源码
/// 反而更有用。
pub(crate) fn image_media_type(content_type: &str, body: &[u8]) -> Option<String> {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    match mime.as_str() {
        // 非标准写法很常见，归到标准名下，工具层的清单只认标准名。
        "image/jpg" | "image/pjpeg" => Some("image/jpeg".to_owned()),
        "image/svg+xml" => None,
        "" | "application/octet-stream" | "binary/octet-stream" => {
            sniff_image(body).map(ToOwned::to_owned)
        }
        m if m.starts_with("image/") => Some(m.to_owned()),
        _ => None,
    }
}

/// 按文件头认四种格式。认不出返回 None —— 这里不是"是不是二进制"的判断。
fn sniff_image(body: &[u8]) -> Option<&'static str> {
    if body.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if body.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if body.starts_with(b"GIF87a") || body.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if body.len() >= 12 && &body[0..4] == b"RIFF" && &body[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
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

        // 图片在解码文本之前拦下。落进下面那条路的话，二进制会被按
        // UTF-8 解成一堆替换字符交给模型 —— 它拿着乱码也会言之凿凿。
        if let Some(media_type) = image_media_type(&resp.content_type, &resp.body) {
            return Ok(Fetched::Image(FetchedImage {
                media_type,
                bytes: resp.body,
            }));
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

    const PNG_HEAD: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    #[test]
    fn 按_content_type_认图片并规范化() {
        // 大小写、参数、非标准写法都是线上真会遇到的
        assert_eq!(
            image_media_type("image/png; charset=binary", b"x"),
            Some("image/png".to_owned())
        );
        assert_eq!(
            image_media_type("IMAGE/JPEG", b"x"),
            Some("image/jpeg".to_owned())
        );
        assert_eq!(
            image_media_type("image/jpg", b"x"),
            Some("image/jpeg".to_owned()),
            "非标准的 image/jpg 要归到标准名下，否则工具层的清单认不出"
        );
        // 不在模型清单里的图片类型也先认成图 —— 收不收由工具层说清楚，
        // 落到文本那条路只会把二进制解成乱码。
        assert_eq!(
            image_media_type("image/bmp", b"x"),
            Some("image/bmp".to_owned())
        );
    }

    #[test]
    fn svg_按文本处理() {
        // XML 文本，视觉模型也不收；让模型读源码比报"不支持的图片"有用
        assert_eq!(image_media_type("image/svg+xml", b"<svg/>"), None);
    }

    #[test]
    fn 没有类型或_octet_stream_时按文件头认() {
        assert_eq!(image_media_type("", PNG_HEAD), Some("image/png".to_owned()));
        assert_eq!(
            image_media_type("application/octet-stream", PNG_HEAD),
            Some("image/png".to_owned())
        );
        assert_eq!(
            image_media_type("application/octet-stream", &[0xFF, 0xD8, 0xFF, 0xE0]),
            Some("image/jpeg".to_owned())
        );
        assert_eq!(
            image_media_type("application/octet-stream", b"GIF89a...."),
            Some("image/gif".to_owned())
        );
        assert_eq!(
            image_media_type("application/octet-stream", b"RIFF\0\0\0\0WEBPVP8 "),
            Some("image/webp".to_owned())
        );
        assert_eq!(
            image_media_type("application/octet-stream", b"just text"),
            None,
            "认不出文件头的 octet-stream 照旧走文本路"
        );
    }

    #[test]
    fn 明确的文本类型不嗅探() {
        // 服务端说了是 HTML，就是 HTML —— 碰巧的魔数不能压过表态
        assert_eq!(image_media_type("text/html", PNG_HEAD), None);
    }
}
