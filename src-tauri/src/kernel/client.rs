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
use riot_protocol::rpc::{RpcNotification, RpcRequest, RpcResponse};

use super::coalesce::{Coalescer, FRAME};
use super::supervisor::{Kernel, KernelError, KernelHandle};

/// 前端事件出口表:session_id → 最新的 Channel。窗口刷新会 attach 新的,
/// 旧 channel 发送失败即自然淘汰。
type Sinks = Arc<Mutex<HashMap<String, Channel<AgentEvent>>>>;

pub struct KernelClient {
    exe: PathBuf,
    sessions_dir: PathBuf,
    kernel: Mutex<Option<Kernel>>,
    /// 请求端缓存。std RwLock 而非 tokio:clone 出来立刻放锁、不跨 await,
    /// 这样并发 RPC 不会被生命周期锁串行化。
    handle: std::sync::RwLock<Option<KernelHandle>>,
    sinks: Sinks,
}

impl KernelClient {
    pub fn new(exe: PathBuf, sessions_dir: PathBuf) -> Self {
        Self {
            exe,
            sessions_dir,
            kernel: Mutex::new(None),
            handle: std::sync::RwLock::new(None),
            sinks: Arc::default(),
        }
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
        spawn_dispatch(rx, Arc::clone(&self.sinks));
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
fn spawn_dispatch(mut rx: mpsc::UnboundedReceiver<Value>, sinks: Sinks) {
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
                    match serde_json::from_value::<RpcNotification>(v) {
                        Ok(RpcNotification::Agent { session_id, event }) => {
                            let sid = session_id.as_str().to_owned();
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
