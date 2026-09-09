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

use riot_protocol::event::{AgentError, AgentEvent, TerminalReason};
use riot_protocol::hostcall::{HostCallErrorKind, HostRequest, HostResponse};
use riot_protocol::rpc::{RpcNotification, RpcRequest, RpcResponse};

use super::coalesce::{Coalescer, FRAME};
use super::supervisor::{Kernel, KernelError, KernelHandle, RestartPolicy};

/// 宿主对内核反向请求(终端/浏览器)的处理端。AppState 实现它 ——
/// 真正的 PTY 和 Chromium 都登记在那边。
#[async_trait::async_trait]
pub trait HostCallHandler: Send + Sync {
    async fn handle(&self, req: HostRequest) -> HostResponse;
}

/// 一个观看者的出口。
struct Viewer {
    ch: Channel<AgentEvent>,
    /// 自这条出口挂上以来往里发过多少条事件。诊断用:前端因为"静默太久"
    /// 重新订阅时,这个数告诉我们上一条出口是**真的没东西可发**,还是
    /// 事件发出去了而 JS 那头没收到(通道死了 —— `Channel::send` 在 JS
    /// 不听时照样成功,宿主没有别的办法知道)。
    sent: u64,
}

/// 前端事件出口表:session_id → 观看者 → 出口。
///
/// 一个会话可以同时有多个观看者(桌面窗口 + 手机上的网页版),每个观看者
/// 各自一条出口;同一观看者重新订阅会替换自己那条(窗口刷新)。发送失败的
/// 出口(网页连接断了)在分发时顺手摘掉。
type Sinks = Arc<Mutex<HashMap<String, HashMap<String, Viewer>>>>;

/// 把一条事件发给会话的每个观看者,发不出去的出口当场摘掉。
///
/// 摘掉是必要的:远程连接断开后那条 Channel 的 send 会一直失败,留着只是
/// 每条事件白克隆一份;而桌面 webview 的 Channel 永不报错,不受影响。
fn fan_out(viewers: &mut HashMap<String, Viewer>, event: AgentEvent) {
    if viewers.len() == 1 {
        // 单观看者是常态,省掉一次 clone。
        let (viewer, v) = viewers.iter_mut().next().expect("非空");
        v.sent += 1;
        let dead = v.ch.send(event).is_err().then(|| viewer.clone());
        if let Some(dead) = dead {
            viewers.remove(&dead);
        }
        return;
    }
    let mut dead = Vec::new();
    for (viewer, v) in viewers.iter_mut() {
        v.sent += 1;
        if v.ch.send(event.clone()).is_err() {
            dead.push(viewer.clone());
        }
    }
    for v in dead {
        viewers.remove(&v);
    }
}

/// 事件流里宿主自己也要消费的那几件事。
///
/// 事件的主要去向是前端 Channel,但 AppState 需要跟着更新自己的登记:
/// busy 指示点、SwitchMode 在内核改掉的权限模式。全量事件都发给宿主
/// 太重(token 流每秒上百条),只挑这几样。
#[derive(Debug)]
pub enum HostNotice {
    /// 一轮开始了。宿主在 turn.submit 时已经把 busy 置上,这条只为**内核
    /// 自己发起的轮**:后台子 agent 跑完唤醒父会话那一轮没有经过宿主,
    /// 不接这条的话侧栏的运行指示点直到轮子结束都不亮。
    Started { session_id: String },
    /// 一轮结束(会话空闲了)。`error` = 这一轮是以失败收场的,原因是什么。
    ///
    /// 失败不进历史(见 riot-core agent_loop「错误不进历史」),宿主把它当
    /// 会话状态记着,切回会话时随快照给前端;下一轮开始就清掉。
    Done {
        session_id: String,
        error: Option<riot_protocol::event::AgentError>,
    },
    /// 内核侧改了权限模式(用户在 SwitchMode 的卡片上同意)。宿主是设置权威,要记下来
    /// 并持久化 —— 否则下一轮 TurnConfig 又把旧模式传回去。
    ModeChanged {
        session_id: String,
        mode: riot_protocol::permission::PermissionMode,
    },
    /// 内核撤回了一条还没被回答的提问(用户在模型开口前按了停止)。
    ///
    /// 撤完会话空了的话,自动标题也要跟着撤 —— 它正是从那句话取的,
    /// 留着就是一个空会话顶着一句从没发出去的话,而且之后真正的第一句
    /// 再也改不动它了(标题只定一次)。
    PromptWithdrawn {
        session_id: String,
        session_empty: bool,
    },
    /// 内核进程没了(崩溃或退出)。宿主要把所有会话的"已水合"标记清掉 ——
    /// 重启后的内核是一张白纸,带着旧标记会把 turn.submit 发到一个
    /// 不存在的会话上。
    KernelGone,
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
    /// 内核死亡标志。分发循环结束(stdout 关闭 = 进程没了)时置位,
    /// 下一次 ensure_running 看到它就收尸旧进程、按退避重启。
    dead: Arc<std::sync::atomic::AtomicBool>,
    restart: Mutex<RestartPolicy>,
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
            dead: Arc::default(),
            restart: Mutex::new(RestartPolicy::default()),
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
    ///
    /// 内核崩溃后(dead 置位)走这里自动重启:收尸旧进程树,按
    /// [`RestartPolicy`] 的退避序列等待;连续崩太多次就放弃并报错 ——
    /// 无限重启会把"内核起不来"的 bug 变成 CPU 打满的死循环。
    pub async fn ensure_running(&self) -> Result<(), KernelError> {
        use std::sync::atomic::Ordering;

        let mut guard = self.kernel.lock().await;
        if guard.is_some() && !self.dead.load(Ordering::SeqCst) {
            return Ok(());
        }
        if let Some(old) = guard.take() {
            // 崩溃路径:进程已经没了,但进程组里可能还有它 spawn 的子进程,
            // 无条件清一遍(kill_now 对空组是无害的 ESRCH)。
            *self.handle.write().expect("handle 锁") = None;
            old.kill_now().await;
        }
        if self.dead.load(Ordering::SeqCst) {
            match self.restart.lock().await.next_delay() {
                Some(d) => {
                    tracing::warn!(delay = ?d, "内核死了,退避后重启");
                    tokio::time::sleep(d).await;
                    self.dead.store(false, Ordering::SeqCst);
                }
                None => {
                    // 不清 dead:之后每次调用都走到这里、立刻报错,
                    // 直到用户重启应用。
                    let n = self.restart.lock().await.failures();
                    return Err(KernelError::RestartExhausted(n));
                }
            }
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
            Arc::clone(&self.dead),
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
        // 完成过一次真正的往返才算"这次启动成功"——起来就崩的循环
        // 不该重置退避计数。
        self.restart.lock().await.record_success();
        match serde_json::from_value::<RpcResponse>(result)? {
            RpcResponse::Error { error } => Err(KernelError::Rpc(error.error)),
            other => Ok(other),
        }
    }

    /// 挂上一个会话的前端事件出口。同一观看者再挂就是替换。
    pub async fn attach_sink(&self, session_id: &str, viewer: &str, ch: Channel<AgentEvent>) {
        let replaced = self
            .sinks
            .lock()
            .await
            .entry(session_id.to_owned())
            .or_default()
            .insert(viewer.to_owned(), Viewer { ch, sent: 0 });
        if let Some(old) = replaced {
            // 前端重新订阅了(切回会话、看门狗判定静默太久、睡眠唤醒)。
            // `sent_since_attach` 大于 0 而前端是因为收不到事件才来重订阅的话,
            // 说明上一条通道已经死了 —— 事件发出去了,JS 那头没收到。
            tracing::info!(
                session_id,
                viewer,
                sent_since_attach = old.sent,
                "事件出口被替换"
            );
        }
    }

    /// 摘掉一个会话的全部事件出口(删会话时)。
    pub async fn detach_sink(&self, session_id: &str) {
        self.sinks.lock().await.remove(session_id);
    }

    /// 摘掉一个观看者在所有会话上的出口(远程连接断开时)。
    pub async fn detach_viewer(&self, viewer: &str) {
        let mut g = self.sinks.lock().await;
        g.retain(|_, viewers| {
            viewers.remove(viewer);
            !viewers.is_empty()
        });
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
    dead: Arc<std::sync::atomic::AtomicBool>,
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
                                // 主循环的第一次请求是 turn 0(见 AgentState::new),
                                // 一轮里之后的每次模型调用 turn 递增 —— 只认第一次。
                                AgentEvent::RequestStart { turn: 0, .. } => {
                                    let _ = host_tx.send(HostNotice::Started { session_id: sid.clone() });
                                }
                                AgentEvent::Done { reason } => {
                                    let error = match reason {
                                        TerminalReason::Error { error } => Some(error.clone()),
                                        _ => None,
                                    };
                                    let _ = host_tx.send(HostNotice::Done {
                                        session_id: sid.clone(),
                                        error,
                                    });
                                }
                                AgentEvent::ModeChanged { mode } => {
                                    let _ = host_tx.send(HostNotice::ModeChanged {
                                        session_id: sid.clone(),
                                        mode: *mode,
                                    });
                                }
                                AgentEvent::PromptWithdrawn { session_empty, .. } => {
                                    let _ = host_tx.send(HostNotice::PromptWithdrawn {
                                        session_id: sid.clone(),
                                        session_empty: *session_empty,
                                    });
                                }
                                _ => {}
                            }
                            let ready = coalescers.entry(sid.clone()).or_default().push(event);
                            if !ready.is_empty() {
                                let mut sinks = sinks.lock().await;
                                if let Some(viewers) = sinks.get_mut(&sid) {
                                    for e in ready {
                                        fan_out(viewers, e);
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
                        let mut sinks = sinks.lock().await;
                        for (sid, e) in due {
                            if let Some(viewers) = sinks.get_mut(&sid) {
                                fan_out(viewers, e);
                            }
                        }
                    }
                }
            }
        }

        // 走到这里 = 内核进程没了(优雅关闭时 sinks 通常已空,发不发无妨;
        // 崩溃时这是**唯一**告诉前端"这一轮完了"的机会)。
        //
        // `[约束]` Done 必须出现(INV-4):消费者依赖它做清理,缺失的表现
        // 是 UI 永远转圈。先吐掉累积中的增量再发 Done,顺序反了 UI 会看到
        // "结束之后又来了半句话"。
        dead.store(true, std::sync::atomic::Ordering::SeqCst);
        let mut sinks_now = sinks.lock().await;
        for (sid, viewers) in sinks_now.iter_mut() {
            if let Some(c) = coalescers.get_mut(sid)
                && let Some(e) = c.tick()
            {
                fan_out(viewers, e);
            }
            let gone = AgentError::Internal {
                error: riot_protocol::ui_error!("host.kernel.gone"),
            };
            fan_out(
                viewers,
                AgentEvent::Done {
                    reason: TerminalReason::Error {
                        error: gone.clone(),
                    },
                },
            );
            let _ = host_tx.send(HostNotice::Done {
                session_id: sid.clone(),
                error: Some(gone),
            });
        }
        let _ = host_tx.send(HostNotice::KernelGone);
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
/// bundle 里它在宿主旁边（externalBin 进 Contents/MacOS）。dev 也在
/// `target/debug/`，但 `cargo run` 宿主**不会**连带编这个 bin —— 只编
/// 成依赖库。工具改了只重启 host、内核仍跑旧二进制，表现就是改了
/// precondition 界面上还是旧报错。所以 debug 启动前先编一次 sidecar。
/// 再试一层上级目录:测试二进制在 target/debug/deps/ 下,内核在上一级。
pub fn locate_kernel() -> Result<PathBuf, String> {
    #[cfg(debug_assertions)]
    rebuild_kernel_bin()?;

    let exe = std::env::current_exe().map_err(|e| format!("cannot get host exe path: {e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| "host exe path has no parent dir".to_owned())?;
    let name = format!("riot-kernel{}", std::env::consts::EXE_SUFFIX);
    for base in [Some(dir), dir.parent()].into_iter().flatten() {
        let candidate = base.join(&name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "kernel binary not found at {}; in dev run `cargo build -p riot-kernel` first",
        dir.join(&name).display()
    ))
}

/// `cargo run` 宿主不会产出 `riot-kernel` 这个可执行文件。
#[cfg(debug_assertions)]
fn rebuild_kernel_bin() -> Result<(), String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| "src-tauri has no parent dir".to_owned())?;
    let status = std::process::Command::new("cargo")
        .args(["build", "-p", "riot-kernel", "--quiet"])
        .current_dir(root)
        .status()
        .map_err(|e| format!("failed to run cargo build for the kernel: {e}"))?;
    if !status.success() {
        return Err("cargo build -p riot-kernel failed".to_owned());
    }
    Ok(())
}
