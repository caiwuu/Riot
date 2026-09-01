//! 传输无关的 MCP 客户端。
//!
//! 读写各一个后台任务，请求靠 id → oneshot 路由 —— 和宿主权限系统的
//! `PendingAsks` 同一个形状。传输只要求 `AsyncRead + AsyncWrite`，
//! 测试用 `tokio::io::duplex` 就能搭一个确定性的假服务器，不用起进程。
//!
//! 豁免理由：等待的是**外部服务器进程**，超时用真实时钟。黄金回放
//! 不经过 MCP —— 它是宿主侧能力，和 provider 的网络 I/O 同一类。
#![allow(clippy::disallowed_methods)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::lines;
use crate::wire::{
    self, CallToolResult, Incoming, InitializeResult, ListToolsResult, Outgoing, OutgoingError,
    OutgoingResponse, RpcError, ToolDef,
};

/// 单条 JSON-RPC 帧的上限。
///
/// `[约束]` 这是内存闸门。服务器进程是第三方的，一条永不换行的输出流
/// 就能把宿主吃到 OOM，而表象只是"应用越来越慢然后被系统杀掉"。
///
/// 16 MiB 是给图片留的余量：工具结果里的图片是 base64，交付上限
/// （`tool::MAX_IMAGE_B64`，2 MB）之上还要容下"先收下来再判超限"的那一份。
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// 一个服务器最多认多少个工具。
///
/// 512 远高于真实规模（大型服务器几十个工具已经算多），它拦的是
/// "一页塞几十万条"这种明显不正常的清单。
const MAX_TOOLS: usize = 512;

/// 各阶段的等待上限。
///
/// `[取舍]` 连接给 60 秒而不是常见的 30：`npx -y` 冷启动要现下包，
/// 慢网络下 30 秒不够，而超时的报错("没有响应")完全不指向"包还在下"。
/// 调用给 10 分钟 —— MCP 工具可能在跑真实任务（爬虫、构建），
/// 掐太短等于替用户决定"这类工具不能用"；等不及可以按停止键，
/// 取消会转成 cancelled 通知发给服务器。
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub connect: Duration,
    pub request: Duration,
    pub call: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(60),
            request: Duration::from_secs(30),
            call: Duration::from_secs(600),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("连接已断开（服务器进程可能退出了）")]
    Closed,
    #[error("{method} 等了 {secs} 秒没有响应")]
    Timeout { method: String, secs: u64 },
    #[error("服务器报错（{code}）：{message}")]
    Rpc { code: i64, message: String },
    #[error("已取消")]
    Cancelled,
    #[error("响应不是预期的形状：{0}")]
    Protocol(String),
}

impl From<RpcError> for ClientError {
    fn from(e: RpcError) -> Self {
        Self::Rpc {
            code: e.code,
            message: e.message,
        }
    }
}

/// 服务器自报的身份，握手结束时拿到。
#[derive(Debug, Clone)]
pub struct ServerHello {
    pub name: String,
    pub version: String,
    pub protocol_version: String,
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>>>;

pub struct Client {
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: AtomicI64,
    timeouts: Timeouts,
    alive: Arc<AtomicBool>,
    /// 服务器发过 `tools/list_changed` 通知。消费方（hub）在下次取工具
    /// 清单时看到它就重新 list —— 不在通知里立刻拉，通知可能连发多条。
    list_changed: Arc<AtomicBool>,
}

impl Client {
    /// 建立连接：起读写任务、完成 initialize 握手。
    ///
    /// 失败时读写任务自然退出（通道与流被 drop），不留悬挂任务。
    pub async fn connect<R, W>(
        reader: R,
        writer: W,
        timeouts: Timeouts,
    ) -> Result<(Arc<Client>, ServerHello), ClientError>
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let alive = Arc::new(AtomicBool::new(true));
        let list_changed = Arc::new(AtomicBool::new(false));

        // 写任务：行缓冲。每条消息一行，逐条 flush —— 攒批在这里没有意义，
        // 消息频率是"每次工具调用几条"的量级。
        tokio::spawn(async move {
            let mut w = writer;
            while let Some(line) = out_rx.recv().await {
                if w.write_all(line.as_bytes()).await.is_err()
                    || w.write_all(b"\n").await.is_err()
                    || w.flush().await.is_err()
                {
                    break; // 进程退出了。读任务那头会把 pending 全部失败掉。
                }
            }
        });

        // 读任务：解析、路由、代答服务器请求。
        {
            let pending = Arc::clone(&pending);
            let alive = Arc::clone(&alive);
            let list_changed = Arc::clone(&list_changed);
            let out = out_tx.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(reader);
                let mut buf = Vec::new();
                // `[约束]` 按字节读行、有损解码，不能用 `lines()`：它一遇到
                // 非 UTF-8 字节就返回 Err，整条连接被判死。Windows 上
                // Python / node 混进 stdout 的日志走 ANSI 码页（GBK），
                // 一行坏字节只是坏行；真正的 JSON-RPC 帧规范保证是 UTF-8，
                // 有损解码不会碰坏它。
                // EOF 或读错误退出循环：连接结束。挂着的请求全部立刻失败 ——
                // 让它们等到超时的话，用户看到的是工具卡片转满十分钟。
                loop {
                    match lines::read_line_capped(&mut reader, &mut buf, MAX_FRAME_BYTES).await {
                        lines::ReadLine::Eof => break,
                        lines::ReadLine::TooLong => {
                            tracing::warn!(
                                limit = MAX_FRAME_BYTES,
                                "MCP 服务器发来一行超过上限的数据，已丢弃"
                            );
                        }
                        lines::ReadLine::Line => {
                            let line = String::from_utf8_lossy(&buf);
                            route_line(&line, &pending, &list_changed, &out).await;
                        }
                    }
                }
                alive.store(false, Ordering::SeqCst);
                // 直接 drop 发送端：等待方收到 RecvError → ClientError::Closed。
                // 不伪造一个 RpcError —— "服务器报错"和"连接断了"是两种病。
                pending.lock().await.clear();
            });
        }

        let client = Arc::new(Client {
            out: out_tx,
            pending,
            next_id: AtomicI64::new(1),
            timeouts,
            alive,
            list_changed,
        });

        // 握手：initialize → initialized 通知。
        let init = client
            .request(
                "initialize",
                json!({
                    "protocolVersion": wire::PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": { "name": "riot", "version": env!("CARGO_PKG_VERSION") },
                }),
                timeouts.connect,
                None,
            )
            .await?;
        let init: InitializeResult = serde_json::from_value(init)
            .map_err(|e| ClientError::Protocol(format!("initialize 响应：{e}")))?;
        client.notify("notifications/initialized", None);

        let hello = ServerHello {
            name: init.server_info.name,
            version: init.server_info.version,
            protocol_version: init.protocol_version,
        };
        Ok((client, hello))
    }

    /// 连接是否还活着。断了的连接上所有请求会立刻失败。
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// 取出并清除"工具清单变过"标记。
    pub fn take_list_changed(&self) -> bool {
        self.list_changed.swap(false, Ordering::SeqCst)
    }

    /// 列出服务器的全部工具（翻完分页）。
    pub async fn list_tools(&self) -> Result<Vec<ToolDef>, ClientError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        // 页数上限防的是服务器 bug（cursor 原地打转），不是正常规模。
        for _ in 0..32 {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let raw = self
                .request("tools/list", params, self.timeouts.request, None)
                .await?;
            let page: ListToolsResult = serde_json::from_value(raw)
                .map_err(|e| ClientError::Protocol(format!("tools/list 响应：{e}")))?;
            tools.extend(page.tools);

            // `[约束]` 页数上限拦不住"一页塞几十万条"。每条都带描述和
            // schema，而这些还要进工具注册表 —— 不设条数上限的话，一个
            // 服务器就能把宿主的内存和上下文预算一起吃掉。
            if tools.len() >= MAX_TOOLS {
                tracing::warn!(
                    limit = MAX_TOOLS,
                    got = tools.len(),
                    "MCP 服务器声明的工具数超过上限，只取前面这些"
                );
                tools.truncate(MAX_TOOLS);
                return Ok(tools);
            }

            match page.next_cursor {
                Some(c) if !c.is_empty() => cursor = Some(c),
                _ => return Ok(tools),
            }
        }
        tracing::warn!("tools/list 翻了 32 页还没到底，就用已拿到的这些");
        Ok(tools)
    }

    /// 调用一个工具。取消时给服务器发 `notifications/cancelled` ——
    /// 不发的话服务器还在跑，白烧它的资源，下一次调用还可能被排队拖慢。
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        cancel: &CancellationToken,
    ) -> Result<CallToolResult, ClientError> {
        let raw = self
            .request(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
                self.timeouts.call,
                Some(cancel),
            )
            .await?;
        serde_json::from_value(raw)
            .map_err(|e| ClientError::Protocol(format!("tools/call 响应：{e}")))
    }

    async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: Option<&CancellationToken>,
    ) -> Result<Value, ClientError> {
        if !self.is_alive() {
            return Err(ClientError::Closed);
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let line = serde_json::to_string(&Outgoing {
            jsonrpc: "2.0",
            id: Some(id),
            method,
            params: Some(params),
        })
        .expect("出站帧都是可序列化的普通数据");
        if self.out.send(line).is_err() {
            self.pending.lock().await.remove(&id);
            return Err(ClientError::Closed);
        }

        let waited = async {
            match tokio::time::timeout(timeout, rx).await {
                Ok(Ok(Ok(v))) => Ok(v),
                Ok(Ok(Err(rpc))) => Err(ClientError::from(rpc)),
                // 发送端被 drop：读任务收尾时已经清了 pending
                Ok(Err(_)) => Err(ClientError::Closed),
                Err(_) => Err(ClientError::Timeout {
                    method: method.to_owned(),
                    secs: timeout.as_secs(),
                }),
            }
        };

        let result = match cancel {
            None => waited.await,
            Some(c) => {
                tokio::select! {
                    r = waited => r,
                    _ = c.cancelled() => {
                        self.notify("notifications/cancelled", Some(json!({
                            "requestId": id,
                            "reason": "用户中断",
                        })));
                        Err(ClientError::Cancelled)
                    }
                }
            }
        };
        // 超时/取消的话响应可能还会迟到，把登记摘掉别让 map 越长越大。
        if result.is_err() {
            self.pending.lock().await.remove(&id);
        }
        result
    }

    fn notify(&self, method: &str, params: Option<Value>) {
        let line = serde_json::to_string(&Outgoing {
            jsonrpc: "2.0",
            id: None,
            method,
            params,
        })
        .expect("出站帧都是可序列化的普通数据");
        let _ = self.out.send(line);
    }
}

/// 处理一条进站消息。
async fn route_line(
    line: &str,
    pending: &Pending,
    list_changed: &AtomicBool,
    out: &mpsc::UnboundedSender<String>,
) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let msg: Incoming = match serde_json::from_str(line) {
        Ok(m) => m,
        Err(e) => {
            // 有些服务器把日志错打到 stdout。跳过而不是断连 ——
            // 断连的代价是整个服务器不可用，而坏行只是坏行。
            tracing::debug!(error = %e, "MCP 通道上有一行不是 JSON-RPC，跳过");
            return;
        }
    };

    // 响应：按 id 找等待者。
    if msg.result.is_some() || msg.error.is_some() {
        let Some(id) = msg.id.as_ref().and_then(Value::as_i64) else {
            return;
        };
        if let Some(tx) = pending.lock().await.remove(&id) {
            let _ = tx.send(match msg.error {
                Some(e) => Err(e),
                None => Ok(msg.result.unwrap_or(Value::Null)),
            });
        }
        return;
    }

    let Some(method) = msg.method.as_deref() else {
        return;
    };

    // 服务器发起的请求：必须回应，不回应的服务器可能一直等下去。
    // 只实现 ping；其余明确说不支持 —— 静默不答和答错都更糟。
    if let Some(id) = msg.id {
        let response = if method == "ping" {
            OutgoingResponse {
                jsonrpc: "2.0",
                id,
                result: Some(json!({})),
                error: None,
            }
        } else {
            OutgoingResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(OutgoingError {
                    code: wire::METHOD_NOT_FOUND,
                    message: format!("riot 不支持 {method}"),
                }),
            }
        };
        if let Ok(line) = serde_json::to_string(&response) {
            let _ = out.send(line);
        }
        return;
    }

    // 通知。
    if method == "notifications/tools/list_changed" {
        list_changed.store(true, Ordering::SeqCst);
    } else {
        tracing::debug!(method, "忽略 MCP 通知");
    }
}
