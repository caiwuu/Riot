//! 宿主侧的内核 RPC 客户端:typed 请求/应答 + 事件分发 + 生命周期。
//!
//! [`Kernel`] 管进程和字节;这一层管**类型**:[`RpcRequest`] 进、
//! [`RpcResponse`] 出,事件通知按 session 过 [`Coalescer`](合帧)后分发给
//! 前端 Channel。AppState 拿着它对内核说话,不再直接持有会话。

// 宿主层:真实进程、真实时钟(合帧定时器)。见 clippy.toml。
#![allow(clippy::disallowed_methods)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tauri::ipc::Channel;
use tokio::sync::{Mutex, mpsc};

use riot_protocol::event::AgentEvent;
use riot_protocol::hostcall::{HostCallErrorKind, HostRequest, HostResponse};
use riot_protocol::rpc::{RpcNotification, RpcRequest, RpcResponse};

use super::coalesce::{Coalescer, FRAME};
use super::supervisor::{Kernel, KernelError, KernelHandle};

/// 宿主对内核反向请求(终端/浏览器)的处理端。AppState 实现它 ——
/// 真正的 PTY 和 Chromium 都登记在那边。
#[async_trait::async_trait]
pub trait HostCallHandler: Send + Sync {
    async fn handle(&self, req: HostRequest) -> HostResponse;
}

/// 前端事件出口表:session_id → 最新的 Channel。窗口刷新会 attach 新的,
/// 旧 channel 发送失败即自然淘汰。
type Sinks = Arc<Mutex<HashMap<String, Channel<AgentEvent>>>>;

/// 事件流里宿主自己也要消费的那几件事。
///
/// 事件的主要去向是前端 Channel,但 AppState 需要跟着更新自己的登记:
/// busy 指示点、ExitPlanMode 在内核改掉的权限模式。全量事件都发给宿主
/// 太重(token 流每秒上百条),只挑这几样。
#[derive(Debug)]
pub enum HostNotice {
    /// 一轮结束(会话空闲了)。
    Done { session_id: String },
    /// 内核侧改了权限模式(ExitPlanMode)。宿主是设置权威,要记下来
    /// 并持久化 —— 否则下一轮 TurnConfig 又把旧模式传回去。
    ModeChanged {
        session_id: String,
        mode: riot_protocol::permission::PermissionMode,
    },
}

pub struct KernelClient {
    exe: PathBuf,
    sessions_dir: PathBuf,
    kernel: Mutex<Option<Kernel>>,
    /// 请求端缓存。std RwLock 而非 tokio:clone 出来立刻放锁、不跨 await,
    /// 这样并发 RPC 不会被生命周期锁串行化。
    handle: std::sync::RwLock<Option<KernelHandle>>,
    sinks: Sinks,
    host_tx: mpsc::UnboundedSender<HostNotice>,
    host_rx: std::sync::Mutex<Option<mpsc::UnboundedReceiver<HostNotice>>>,
    /// 反向请求的处理端。RwLock<Option>:启动早期(还没注入)收到请求
    /// 就回"未就绪",不会丢应答。
    host_service: Arc<std::sync::RwLock<Option<Arc<dyn HostCallHandler>>>>,
}

impl KernelClient {
    pub fn new(exe: PathBuf, sessions_dir: PathBuf) -> Self {
        let (host_tx, host_rx) = mpsc::unbounded_channel();
        Self {
            exe,
            sessions_dir,
            kernel: Mutex::new(None),
            handle: std::sync::RwLock::new(None),
            sinks: Arc::default(),
            host_tx,
            host_rx: std::sync::Mutex::new(Some(host_rx)),
            host_service: Arc::default(),
        }
    }

    /// 取走宿主通知的接收端(只能取一次)。AppState 启动后用它跑一个
    /// 消费任务,更新 busy / mode 登记。
    pub fn take_host_notices(&self) -> Option<mpsc::UnboundedReceiver<HostNotice>> {
        self.host_rx.lock().expect("host_rx 锁").take()
    }

    /// 注入反向请求的处理端(AppState 启动时调一次)。
    pub fn set_host_service(&self, svc: Arc<dyn HostCallHandler>) {
        *self.host_service.write().expect("host_service 锁") = Some(svc);
    }

    /// 确保内核进程活着;没起过就 spawn 并接上事件分发。幂等。
    pub async fn ensure_running(&self) -> Result<(), KernelError> {
        let mut guard = self.kernel.lock().await;
        if guard.is_some() {
            return Ok(());
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let kernel = Kernel::spawn(
            self.exe.clone(),
            &[(
                "RIOT_SESSIONS_DIR".to_owned(),
                self.sessions_dir.display().to_string(),
            )],
            tx,
        )
        .await?;
        spawn_dispatch(
            rx,
            Arc::clone(&self.sinks),
            self.host_tx.clone(),
            kernel.handle(),
            Arc::clone(&self.host_service),
        );
        *self.handle.write().expect("handle 锁") = Some(kernel.handle());
        *guard = Some(kernel);
        tracing::info!(exe = %self.exe.display(), "内核进程已启动");
        Ok(())
    }

    /// 发一个 typed 请求。
    ///
    /// [`RpcRequest`] 是 adjacently-tagged(method/params),序列化后正好拆成
    /// JSON-RPC 的两个信封字段;应答信封的 result 字段就是 [`RpcResponse`]。
    /// 内核报的业务错误(`RpcResponse::Error`)在这里统一转成 `Err`,
    /// 调用方只 match 自己期望的成功变体。
    pub async fn call(&self, req: RpcRequest) -> Result<RpcResponse, KernelError> {
        self.ensure_running().await?;
        let handle = self
            .handle
            .read()
            .expect("handle 锁")
            .clone()
            .ok_or(KernelError::NotRunning)?;

        let v = serde_json::to_value(&req)?;
        let method = v
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let params = v.get("params").cloned().unwrap_or(Value::Null);

        let result = handle.request(&method, params).await?;
        match serde_json::from_value::<RpcResponse>(result)? {
            RpcResponse::Error { error } => Err(KernelError::Rpc(error.message)),
            other => Ok(other),
        }
    }

    /// 挂上一个会话的前端事件出口。
    pub async fn attach_sink(&self, session_id: &str, ch: Channel<AgentEvent>) {
        self.sinks.lock().await.insert(session_id.to_owned(), ch);
    }

    /// 摘掉一个会话的事件出口(删会话时)。
    pub async fn detach_sink(&self, session_id: &str) {
        self.sinks.lock().await.remove(session_id);
    }

    /// 四步关闭序列(转发给 [`Kernel::shutdown`])。App 退出时调。
    pub async fn shutdown(&self) {
        *self.handle.write().expect("handle 锁") = None;
        let kernel = self.kernel.lock().await.take();
        if let Some(k) = kernel {
            k.shutdown().await;
        }
    }
}

/// 事件分发循环:内核通知 → 按 session 合帧 → 前端 Channel。
///
/// 每个会话一个 [`Coalescer`]:token 流每秒上百条,合帧把 IPC 消息数降
/// 一个数量级;边界事件(工具调用、权限询问、Done)立发,见 coalesce
/// 模块的三条约束。
fn spawn_dispatch(
    mut rx: mpsc::UnboundedReceiver<Value>,
    sinks: Sinks,
    host_tx: mpsc::UnboundedSender<HostNotice>,
    handle: KernelHandle,
    host_service: Arc<std::sync::RwLock<Option<Arc<dyn HostCallHandler>>>>,
) {
    tokio::spawn(async move {
        let mut coalescers: HashMap<String, Coalescer> = HashMap::new();
        let mut tick = tokio::time::interval(FRAME);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    let Some(v) = msg else {
                        // 通道关闭 = 内核 stdout 结束(进程退出了)。
                        tracing::warn!("内核事件流结束");
                        break;
                    };
                    // 反向请求(id + method):内核要用宿主的终端/浏览器。
                    // 处理可能很慢(浏览器 wait_for 合法地等几十秒),
                    // 必须 spawn 出去 —— 堵住这里就是堵住整个事件流。
                    if v.get("method").is_some()
                        && let Some(id) = v.get("id").and_then(Value::as_u64)
                    {
                        let svc = host_service.read().expect("host_service 锁").clone();
                        let handle = handle.clone();
                        tokio::spawn(async move {
                            let resp = serve_host_call(svc, &v).await;
                            let result = serde_json::to_value(&resp).unwrap_or(Value::Null);
                            if let Err(e) = handle.respond(id, result) {
                                tracing::warn!(error = %e, id, "反向应答写不回内核");
                            }
                        });
                        continue;
                    }
                    match serde_json::from_value::<RpcNotification>(v) {
                        Ok(RpcNotification::Agent { session_id, event }) => {
                            let sid = session_id.as_str().to_owned();
                            // 宿主关心的那几件先拷贝一份出去(见 HostNotice)。
                            match &event {
                                AgentEvent::Done { .. } => {
                                    let _ = host_tx.send(HostNotice::Done { session_id: sid.clone() });
                                }
                                AgentEvent::ModeChanged { mode } => {
                                    let _ = host_tx.send(HostNotice::ModeChanged {
                                        session_id: sid.clone(),
                                        mode: *mode,
                                    });
                                }
                                _ => {}
                            }
                            let ready = coalescers.entry(sid.clone()).or_default().push(event);
                            if !ready.is_empty() {
                                let sinks = sinks.lock().await;
                                if let Some(ch) = sinks.get(&sid) {
                                    for e in ready {
                                        let _ = ch.send(e);
                                    }
                                }
                            }
                        }
                        Ok(RpcNotification::KernelError { message, fatal }) => {
                            tracing::error!(fatal, "内核报告错误:{message}");
                        }
                        Err(e) => tracing::warn!(error = %e, "内核通知解析失败"),
                    }
                }
                _ = tick.tick() => {
                    // 帧到期:把各会话累积中的增量吐出去。
                    let mut due = Vec::new();
                    for (sid, c) in &mut coalescers {
                        if let Some(e) = c.tick() {
                            due.push((sid.clone(), e));
                        }
                    }
                    if !due.is_empty() {
                        let sinks = sinks.lock().await;
                        for (sid, e) in due {
                            if let Some(ch) = sinks.get(&sid) {
                                let _ = ch.send(e);
                            }
                        }
                    }
                }
            }
        }
    });
}

/// 解析一条反向请求并交给处理端。任何一步失败都回一条 Error 应答 ——
/// 静默不回会让内核那边的工具调用永远挂着。
async fn serve_host_call(svc: Option<Arc<dyn HostCallHandler>>, envelope: &Value) -> HostResponse {
    let reconstructed = serde_json::json!({
        "method": envelope.get("method"),
        "params": envelope.get("params"),
    });
    let req: HostRequest = match serde_json::from_value(reconstructed) {
        Ok(r) => r,
        Err(e) => {
            return HostResponse::Error {
                kind: HostCallErrorKind::Unavailable,
                message: format!("宿主解析不了这条反向请求:{e}"),
            };
        }
    };
    match svc {
        Some(s) => s.handle(req).await,
        None => HostResponse::Error {
            kind: HostCallErrorKind::Unavailable,
            message: "宿主服务端还没就绪".to_owned(),
        },
    }
}

/// 定位内核二进制。
///
/// dev 和 bundle 两种情况下它都在宿主可执行文件旁边:dev 是 workspace 的
/// target/debug/(一起 build),bundle 是 externalBin 进 Contents/MacOS/。
pub fn locate_kernel() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("拿不到宿主路径:{e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| "宿主路径没有父目录".to_owned())?;
    let name = format!("riot-kernel{}", std::env::consts::EXE_SUFFIX);
    let candidate = dir.join(&name);
    if candidate.exists() {
        return Ok(candidate);
    }
    Err(format!(
        "找不到内核二进制 {}。开发模式请先 `cargo build -p riot-kernel`。",
        candidate.display()
    ))
}
