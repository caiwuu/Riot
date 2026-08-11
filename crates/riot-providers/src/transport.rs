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
}

fn describe(e: &HttpError) -> String {
    match e.status {
        Some(s) => format!("HTTP {s}: {}", truncate(&e.body, 300)),
        None if e.transport => format!("连接失败: {}", truncate(&e.body, 300)),
        None => truncate(&e.body, 300),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
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
                "脚本已耗尽 —— 要么用例缺响应，要么 provider 多发了一次请求",
            )),
        }
    }
}
