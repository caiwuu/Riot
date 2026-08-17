//! 内核进程的生命周期监管。
//!
//! # 为什么不用 Tauri 官方 sidecar
//!
//! `tauri-plugin-shell` 的 `sidecar().spawn()` 有三个硬伤，每一个单独都足以否决它：
//!
//! 1. **`CommandChild` 没有关闭 stdin 的能力**（只有 `kill` / `pid` / `write`）。
//!    而 `drop(stdin)` 制造 EOF 正是 stdio JSON-RPC 服务的标准退出握手。
//!    用官方 API 只能 `kill()`，内核没机会 flush 会话状态。
//! 2. **不做进程树清理。**`tauri::process` 到 2.11.5 为止只有 `current_binary`
//!    和 `restart`，`kill_process_tree` 的 PR 未合入。
//! 3. **有两条重要路径根本不执行清理钩子。**`tauri dev` 停止时对 cargo 发 SIGKILL，
//!    NSIS 安装器升级时用 `TerminateProcess`。所以「靠 `RunEvent::Exit` 清理」
//!    在开发期和升级期都失效。
//!
//! 我们只用 `externalBin` 做打包分发（让二进制进 app bundle、被签名），
//! 进程本身用 `tokio::process` + `process-wrap` 自己管。
//!
//! # 关闭序列
//!
//! ```text
//! 1. 发 kernel.shutdown RPC     内核 flush 会话、杀自己的子进程
//! 2. drop(stdin) → EOF          标准退出信号
//! 3. 等待退出，超时 5s
//! 4. 超时则 kill 整棵进程树      Job Object / 进程组兜底
//! ```
//!
//! 前三步都是「请求内核配合」，只有第 4 步是强制的。**这一步不能省。**

// 宿主层不参与黄金回放，确定性约束（见 clippy.toml）只针对内核。
// 这里需要真实的进程、真实的时钟。
#![allow(clippy::disallowed_methods)]

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use process_wrap::tokio::{ChildWrapper, CommandWrap};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, mpsc, oneshot};

/// 优雅关闭的等待上限。超过就走 OS 层强杀。
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// 崩溃重启的退避序列。连续崩溃到第 5 次就不再重启，报错给用户 ——
/// 无限重启会把一个「内核起不来」的 bug 变成 CPU 打满的死循环。
const RESTART_BACKOFF: [Duration; 4] = [
    Duration::from_millis(200),
    Duration::from_millis(1000),
    Duration::from_millis(3000),
    Duration::from_millis(10000),
];

#[derive(Debug, thiserror::Error)]
pub enum KernelError {
    #[error("内核未运行")]
    NotRunning,
    #[error("内核连续崩溃 {0} 次，已停止重启")]
    RestartExhausted(usize),
    #[error("内核响应超时: {method}")]
    Timeout { method: String },
    #[error("内核返回错误: {0}")]
    Rpc(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// 待响应的请求表。key 是 JSON-RPC id。
type Pending = Arc<Mutex<std::collections::HashMap<u64, oneshot::Sender<Value>>>>;

/// 请求端句柄。可以 Clone 到任意任务并发发请求 —— 进程的生命周期
/// (关闭/强杀)由 [`Kernel`] 独占管理。两者分开,请求才不会被
/// 生命周期锁串行化。
#[derive(Clone)]
pub struct KernelHandle {
    stdin_tx: mpsc::UnboundedSender<Vec<u8>>,
    pending: Pending,
    next_id: Arc<AtomicU64>,
}

impl KernelHandle {
    /// 发一个 JSON-RPC 请求并等响应。
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, KernelError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let payload = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        }))?;
        self.stdin_tx
            .send(payload)
            .map_err(|_| KernelError::NotRunning)?;

        match rx.await {
            Ok(msg) => {
                if let Some(err) = msg.get("error") {
                    return Err(KernelError::Rpc(err.to_string()));
                }
                Ok(msg.get("result").cloned().unwrap_or(Value::Null))
            }
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(KernelError::NotRunning)
            }
        }
    }

    /// 给内核的反向请求写回应答(`{jsonrpc, id, result}`,方向:宿主 → 内核)。
    pub fn respond(&self, id: u64, result: Value) -> Result<(), KernelError> {
        let payload = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0", "id": id, "result": result,
        }))?;
        self.stdin_tx
            .send(payload)
            .map_err(|_| KernelError::NotRunning)
    }
}

pub struct Kernel {
    child: Box<dyn ChildWrapper>,
    /// stdin 的关闭开关。drop 掉它会让写线程结束并 drop 真正的 ChildStdin，
    /// 内核那边收到 EOF。这是关闭序列的第 2 步。
    stdin_closer: Option<oneshot::Sender<()>>,
    handle: KernelHandle,
}

impl Kernel {
    /// 启动内核进程。
    ///
    /// `[约束]` 必须包 Job Object（Windows）或进程组（Unix）。这是让 **操作系统**
    /// 保证「父进程无论怎么死，子树跟着死」的唯一办法。Chromium 和 VS Code 都是这个路子。
    /// 应用层的 cleanup 钩子在 SIGKILL 面前是不存在的。
    pub async fn spawn(
        exe: PathBuf,
        envs: &[(String, String)],
        on_notification: mpsc::UnboundedSender<Value>,
    ) -> Result<Self, KernelError> {
        let mut cmd = tokio::process::Command::new(exe);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in envs {
            cmd.env(k, v);
        }

        let mut wrap = CommandWrap::from(cmd);
        #[cfg(windows)]
        wrap.wrap(process_wrap::tokio::JobObject);
        #[cfg(unix)]
        wrap.wrap(process_wrap::tokio::ProcessGroup::leader());

        let mut child = wrap.spawn()?;

        let mut stdin = child.stdin().take().expect("stdin was piped");
        let stdout = child.stdout().take().expect("stdout was piped");
        let stderr = child.stderr().take().expect("stderr was piped");

        let pending: Pending = Arc::default();

        // 写线程。持有真正的 ChildStdin —— 只有它退出，内核才收到 EOF。
        let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (closer_tx, mut closer_rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(bytes) = stdin_rx.recv() => {
                        if stdin.write_all(&bytes).await.is_err() { break; }
                        if stdin.write_all(b"\n").await.is_err() { break; }
                        if stdin.flush().await.is_err() { break; }
                    }
                    _ = &mut closer_rx => break,
                    else => break,
                }
            }
            drop(stdin); // ← 内核在这一刻收到 EOF
        });

        // 读线程：分发响应到 pending 表，通知与反向请求转发给上层。
        let pending_rx = Arc::clone(&pending);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                    tracing::warn!(raw = %line, "内核输出非法 JSON");
                    continue;
                };
                // 有 id 且有 method = 内核发来的**反向请求**(终端/浏览器,
                // 见 riot_protocol::hostcall)。和通知走同一条上行通道,
                // 由 KernelClient 那边按形状分流 —— 这里只管搬运。
                if msg.get("method").is_some() {
                    let _ = on_notification.send(msg);
                    continue;
                }
                match msg.get("id").and_then(Value::as_u64) {
                    Some(id) => {
                        if let Some(tx) = pending_rx.lock().await.remove(&id) {
                            let _ = tx.send(msg);
                        } else {
                            tracing::warn!(id, "收到无人等待的响应");
                        }
                    }
                    // 没有 id 就是通知（事件流走这条路）
                    None => {
                        let _ = on_notification.send(msg);
                    }
                }
            }
        });

        // stderr 直接进日志。内核的 panic backtrace 从这里出来，
        // 不接的话崩溃现场就丢了。
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::warn!(target: "kernel", "{line}");
            }
        });

        Ok(Self {
            child,
            stdin_closer: Some(closer_tx),
            handle: KernelHandle {
                stdin_tx,
                pending,
                next_id: Arc::new(AtomicU64::new(1)),
            },
        })
    }

    /// 请求端句柄(可 Clone,并发安全)。
    pub fn handle(&self) -> KernelHandle {
        self.handle.clone()
    }

    /// 四步关闭序列。见模块文档。
    pub async fn shutdown(mut self) {
        // 1. 给内核收尾的机会。失败也无所谓 —— 后面还有三步。
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            self.handle.request("kernel.shutdown", Value::Null),
        )
        .await;

        // 2. EOF
        drop(self.stdin_closer.take());

        // 3. 等**内核自己**退出。注意用 inner —— 外层 wait 等的是整个进程组，
        //    而组里可能有内核故意留下的长命进程，那会一直等下去。
        if tokio::time::timeout(SHUTDOWN_GRACE, self.child.inner_mut().wait())
            .await
            .is_err()
        {
            tracing::warn!("内核未在 {SHUTDOWN_GRACE:?} 内退出");
        }

        // 4. 无条件清理整个进程组。
        //
        //    [约束] 这一步不能写成「只在超时时才杀」。内核优雅退出 ≠ 它 spawn
        //    的后台子进程也退出了 —— 那些会被 init 收养成孤儿，一直活到关机。
        //    这类泄漏比「内核卡死」隐蔽得多：功能全对，只是机器越跑越慢。
        if let Err(e) = self.child.start_kill() {
            // 组已空时 killpg 返回 ESRCH，这是正常路径。
            tracing::debug!(error = %e, "清理进程组");
        }

        // 5. reap，避免留下僵尸。
        let _ = tokio::time::timeout(SHUTDOWN_GRACE, self.child.wait()).await;
    }

    /// 跳过握手直接杀整棵进程树。
    ///
    /// 内核卡死时用。正常退出走 `shutdown` —— 那条路给内核 flush 会话的机会。
    pub async fn kill_now(mut self) {
        let _ = Box::into_pin(self.child.kill()).await;
    }
}

/// 崩溃重启策略。
///
/// 独立成一个类型是为了能脱离真实进程做单元测试 —— 退避序列和放弃阈值
/// 是纯逻辑，不该只能靠杀真进程来验证。
#[derive(Debug, Default)]
pub struct RestartPolicy {
    consecutive_failures: usize,
}

impl RestartPolicy {
    /// 内核成功跑过一段有意义的时间后调用，重置计数。
    ///
    /// `[约束]` 判断「成功」不能只看进程起来了 —— 起来就崩的循环同样会打满 CPU。
    /// 要看内核是否完成过至少一次 RPC 往返。
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
    }

    /// 返回下次重启前该等多久。`None` 表示放弃。
    pub fn next_delay(&mut self) -> Option<Duration> {
        let delay = RESTART_BACKOFF.get(self.consecutive_failures).copied();
        self.consecutive_failures += 1;
        delay
    }

    pub fn failures(&self) -> usize {
        self.consecutive_failures
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 退避序列递增且最终放弃() {
        let mut p = RestartPolicy::default();
        assert_eq!(p.next_delay(), Some(Duration::from_millis(200)));
        assert_eq!(p.next_delay(), Some(Duration::from_millis(1000)));
        assert_eq!(p.next_delay(), Some(Duration::from_millis(3000)));
        assert_eq!(p.next_delay(), Some(Duration::from_millis(10000)));
        assert_eq!(
            p.next_delay(),
            None,
            "第 5 次必须放弃，否则起来就崩会打满 CPU"
        );
    }

    #[test]
    fn 成功后重置计数() {
        let mut p = RestartPolicy::default();
        p.next_delay();
        p.next_delay();
        assert_eq!(p.failures(), 2);
        p.record_success();
        assert_eq!(p.failures(), 0);
        assert_eq!(p.next_delay(), Some(Duration::from_millis(200)));
    }
}
