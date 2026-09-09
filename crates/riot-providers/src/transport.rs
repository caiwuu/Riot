//! HTTP 注入点。
//!
//! 真实的 HTTP 客户端在这个 trait 后面。这么做有两个理由，第二个更重要：
//!
//! 1. 测试不需要起服务器；
//! 2. **失败的形状被固定下来了。**`HttpError` 的字段就是重试决策需要的
//!    全部输入 —— 状态码、`retry-after`、`x-should-retry`、是不是传输层错误。
//!    换 HTTP 库时，编译器会指着每一个没填的字段。
//!
//! 如果直接在 provider 里用 reqwest，这些信息会散落在各处的 `match`
//! 里，而「某个错误分支忘了读 retry-after」这种问题没有任何反馈。

use std::pin::Pin;

use async_trait::async_trait;
use futures_core::Stream;
use tokio_util::sync::CancellationToken;

/// 响应体的字节流。
///
/// `[约束]` item 是**原始字节**，不是 `String`。分片可以落在任意位置 ——
/// 包括一个 UTF-8 字符的中间。重组由 [`crate::sse::SseParser`] 统一负责，
/// 每个 transport 实现都不用操心。
///
/// 反过来（要求 transport 交付完整字符）试过，是错的：责任分散到每个
/// HTTP 客户端实现，漏掉的那个会产生乱码，而且不报错、不崩溃。
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Vec<u8>, HttpError>> + Send>>;

#[async_trait]
pub trait HttpTransport: Send + Sync {
    /// 发一个 POST 并返回 SSE 流。
    ///
    /// 返回 `Err` 表示**请求阶段**就失败了（连不上、非 2xx 响应）。
    /// 流建立之后的失败通过 stream item 的 `Err` 上报 —— 这两种失败
    /// 的可重试性完全不同，见 `provider::stream` 的注释。
    async fn post_sse(
        &self,
        req: HttpRequest,
        cancel: CancellationToken,
    ) -> Result<ByteStream, HttpError>;
}

#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// HTTP 层的失败。
///
/// 字段就是 [`crate::retry::FailureContext`] 需要的输入 —— 刻意对齐，
/// 这样重试决策不用去猜任何东西。
#[derive(Debug, Clone, Default, thiserror::Error)]
#[error("{}", describe(self))]
pub struct HttpError {
    /// None 表示压根没拿到响应（DNS、连接、TLS）。
    pub status: Option<u16>,
    /// `retry-after` 响应头，秒。
    pub retry_after_secs: Option<u64>,
    /// `x-should-retry` 响应头。服务端对这个请求的明确指令。
    pub x_should_retry: Option<bool>,
    pub body: String,
    /// 传输层错误（连接被拒、DNS 失败、读到一半断了）。
    pub transport: bool,
    /// 传输层错误里的超时那一种。单独标出来是因为给用户的解释不同：
    /// "连不上"要查地址和网络，"超时"多半等一等重试就好。
    pub timed_out: bool,
}

/// 给人看的一行描述。进 `UiError.detail`，界面上折在「详情」里。
///
/// 正文不原样贴：服务方回的是给程序读的 JSON 或给浏览器看的整张网页，
/// 两者直接上屏都是一大块噪音。这里只留真正说明原因的那一句
/// （见 [`summarize_body`]），状态码放在前面 —— 它是最先要核对的数字。
fn describe(e: &HttpError) -> String {
    let body = summarize_body(&e.body);
    match e.status {
        Some(s) if body.is_empty() => format!("HTTP {s}"),
        Some(s) => format!("HTTP {s}: {body}"),
        None if e.transport => format!("connection failed: {body}"),
        None => body,
    }
}

/// 详情里最多留多少字。够放一句错误说明；再长的是整段 JSON 或网页源码，
/// 那不是"详情"而是垃圾。
const DETAIL_MAX_CHARS: usize = 200;

/// 从响应正文里挑出值得给人看的那一句。
///
/// - JSON：取 `error.message`（OpenAI / Anthropic / 大多数网关的形状），
///   退而求其次 `message` / `error` / `detail` 字符串字段。整段 JSON 里
///   只有这一句是写给人的，其余是给程序的。
/// - HTML：整页都是给浏览器看的，只留 `<title>`（"Attention Required! |
///   Cloudflare"、"403 Forbidden"）—— 那是页面自己对这次拒绝的一句话概括。
/// - 其他：压掉换行和连续空白后截断。
pub fn summarize_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Some(msg) = json_error_message(trimmed) {
        return truncate(&collapse_ws(&msg), DETAIL_MAX_CHARS);
    }
    if is_html(trimmed) {
        return match html_title(trimmed) {
            Some(title) => truncate(&collapse_ws(&title), DETAIL_MAX_CHARS),
            None => "HTML page".to_owned(),
        };
    }
    truncate(&collapse_ws(trimmed), DETAIL_MAX_CHARS)
}

/// 正文是不是一整张网页（而不是接口回的 JSON / 文本）。
///
/// 只认开头：`<!DOCTYPE html` 或 `<html`。正文里某处出现 `<html>` 不算 ——
/// 错误说明里引用一段 HTML 是有可能的，整页才是网页。
pub fn is_html(body: &str) -> bool {
    let head: String = body
        .trim_start()
        .chars()
        .take(64)
        .collect::<String>()
        .to_ascii_lowercase();
    head.starts_with("<!doctype html") || head.starts_with("<html")
}

fn json_error_message(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let non_empty = |s: &str| {
        let s = s.trim();
        (!s.is_empty()).then(|| s.to_owned())
    };
    // `error` 是对象：取里面的 message；是字符串：它自己就是说明。
    if let Some(err) = v.get("error") {
        if let Some(m) = err.get("message").and_then(|m| m.as_str())
            && let Some(s) = non_empty(m)
        {
            return Some(s);
        }
        if let Some(s) = err.as_str().and_then(non_empty) {
            return Some(s);
        }
    }
    ["message", "detail", "error_description"]
        .iter()
        .find_map(|k| v.get(k).and_then(|m| m.as_str()).and_then(non_empty))
}

fn html_title(body: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    let open = lower.find("<title")?;
    let start = open + lower[open..].find('>')? + 1;
    let end = start + lower[start..].find("</title>")?;
    let title = decode_html_entities(body[start..end].trim());
    (!title.is_empty()).then_some(title)
}

/// 只解标题里常见的那几个实体。整套 HTML 实体表为一行标题不值得引依赖。
fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{}…", cut.trim_end())
}

impl HttpError {
    pub fn transport(msg: impl Into<String>) -> Self {
        Self {
            transport: true,
            body: msg.into(),
            ..Default::default()
        }
    }

    pub fn status(code: u16, body: impl Into<String>) -> Self {
        Self {
            status: Some(code),
            body: body.into(),
            ..Default::default()
        }
    }

    /// 转成重试决策需要的上下文。
    pub fn failure_context<'a>(
        &'a self,
        source: crate::retry::RequestSource,
        is_subscription: bool,
        attempt: u32,
    ) -> crate::retry::FailureContext<'a> {
        crate::retry::FailureContext {
            status: self.status,
            transport_error: self.transport,
            retry_after_secs: self.retry_after_secs,
            x_should_retry: self.x_should_retry,
            source,
            is_subscription,
            attempt,
            error_body: &self.body,
        }
    }
}

// ────────────────────────────────────────────────────────────
// 测试替身
// ────────────────────────────────────────────────────────────

/// 按脚本逐次返回预录响应的 transport。
///
/// 每次 `post_sse` 消费脚本里的下一项。这让「第 1、2 次 429，第 3 次成功」
/// 这类重试场景能完整测出来，而不用起服务器。
pub struct ScriptedTransport {
    script: std::sync::Mutex<std::collections::VecDeque<ScriptedResponse>>,
    calls: std::sync::atomic::AtomicUsize,
    seen: std::sync::Mutex<Vec<HttpRequest>>,
}

pub enum ScriptedResponse {
    /// 成功，按给定的分片吐出。分片边界故意可控 —— SSE 解析器必须
    /// 能应付任意切分，**包括切在多字节字符中间**。
    Chunks(Vec<Vec<u8>>),
    /// 请求阶段失败。
    Fail(HttpError),
    /// 流建立成功，但吐到一半断了。
    PartialThenFail(Vec<Vec<u8>>, HttpError),
}

impl ScriptedTransport {
    pub fn new(script: Vec<ScriptedResponse>) -> Self {
        Self {
            script: std::sync::Mutex::new(script.into()),
            calls: std::sync::atomic::AtomicUsize::new(0),
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn call_count(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn requests(&self) -> Vec<HttpRequest> {
        self.seen.lock().expect("seen poisoned").clone()
    }
}

#[async_trait]
impl HttpTransport for ScriptedTransport {
    async fn post_sse(
        &self,
        req: HttpRequest,
        _cancel: CancellationToken,
    ) -> Result<ByteStream, HttpError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.seen.lock().expect("seen poisoned").push(req);

        let next = self.script.lock().expect("script poisoned").pop_front();

        match next {
            Some(ScriptedResponse::Chunks(chunks)) => {
                Ok(Box::pin(futures::stream::iter(chunks.into_iter().map(Ok))))
            }
            Some(ScriptedResponse::Fail(e)) => Err(e),
            Some(ScriptedResponse::PartialThenFail(chunks, e)) => {
                let items: Vec<Result<Vec<u8>, HttpError>> = chunks
                    .into_iter()
                    .map(Ok)
                    .chain(std::iter::once(Err(e)))
                    .collect();
                Ok(Box::pin(futures::stream::iter(items)))
            }
            // 刻意用不可重试的 400 而不是传输错误。传输错误是可重试的，
            // 会让 provider 在脚本耗尽后继续空转 —— 那样「重试了多少次」
            // 这类断言就永远测不准，问题会被替身掩盖掉。
            None => Err(HttpError::status(
                400,
                "script exhausted: the test case is missing a response, or the provider sent one request too many",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLOUDFLARE_PAGE: &str = r#"<!DOCTYPE html>
<!--[if lt IE 7]> <html class="no-js ie6 oldie" lang="en-US"> <![endif]-->
<!--[if gt IE 8]><!--> <html class="no-js" lang="en-US"> <!--<![endif]-->
<head>
<title>Attention Required! | Cloudflare</title>
<meta charset="UTF-8" />
</head>
<body><div id="cf-wrapper">Sorry, you have been blocked</div></body>
</html>"#;

    #[test]
    fn json_正文只留_error_message() {
        let openai = r#"{"error":{"message":"Incorrect API key provided: sk-abc","type":"invalid_request_error","code":"invalid_api_key"}}"#;
        assert_eq!(
            describe(&HttpError::status(401, openai)),
            "HTTP 401: Incorrect API key provided: sk-abc"
        );
        let anthropic = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
        assert_eq!(summarize_body(anthropic), "invalid x-api-key");
        // 网关常见的扁平形状
        assert_eq!(
            summarize_body(r#"{"error":"Unauthorized"}"#),
            "Unauthorized"
        );
        assert_eq!(summarize_body(r#"{"message":"Forbidden"}"#), "Forbidden");
        assert_eq!(summarize_body(r#"{"detail":"Not Found"}"#), "Not Found");
    }

    #[test]
    fn 认不出说明字段的_json_原样截断() {
        let s = summarize_body(r#"{"code":42,"ok":false}"#);
        assert_eq!(s, r#"{"code":42,"ok":false}"#);
    }

    #[test]
    fn html_页面只留标题() {
        assert!(is_html(CLOUDFLARE_PAGE));
        assert_eq!(
            describe(&HttpError::status(403, CLOUDFLARE_PAGE)),
            "HTTP 403: Attention Required! | Cloudflare"
        );
        assert_eq!(
            summarize_body("<html><head><TITLE>403 &amp; Forbidden</TITLE></head></html>"),
            "403 & Forbidden"
        );
        assert_eq!(
            summarize_body("<html><body>nope</body></html>"),
            "HTML page"
        );
        // 说明文字里引用一段 html 不算网页
        assert!(!is_html(r#"{"error":"unexpected <html> in upstream"}"#));
    }

    #[test]
    fn 纯文本压空白并截断() {
        assert_eq!(summarize_body("  bad\n\n  request  \n"), "bad request");
        let long = "x".repeat(500);
        let s = summarize_body(&long);
        assert_eq!(s.chars().count(), DETAIL_MAX_CHARS + 1);
        assert!(s.ends_with('…'));
        // 多字节字符不能被切在中间
        let cjk = "错".repeat(300);
        assert!(summarize_body(&cjk).ends_with("错…"));
    }

    #[test]
    fn 空正文只报状态码() {
        assert_eq!(describe(&HttpError::status(502, "   ")), "HTTP 502");
        assert_eq!(
            describe(&HttpError::transport("dns error: no such host")),
            "connection failed: dns error: no such host"
        );
    }
}
