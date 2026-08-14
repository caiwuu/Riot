//! 浏览器子进程的生命周期与通信。
//!
//! CEF 跑在 `riot-browser.app` 这个独立进程里。拆出去的理由见那个 crate 的
//! 文档 —— 简单说是 macOS 上 CEF 必须从 `.app` 启动，而主应用在 `tauri dev`
//! 下是裸二进制。
//!
//! 这里的职责和 [`crate::kernel::supervisor`] 是同一类:spawn、包进程组、
//! 收发 NDJSON、崩了能重启。约定也照抄那边 —— 两套进程监管用不同的思路
//! 会让"进程为什么没退干净"这类问题每次都要重新查一遍。
//!
//! 只有"崩了怎么办"这一条不一样。内核崩了立刻按退避序列拉起来:那是每个
//! 会话都要用的东西，不在就什么都干不了。浏览器是可选能力，而且崩溃常常
//! 发生在没人看的时候 —— 所以这一层只负责让句柄说得出自己废了
//! （[`Browser::alive`]），真正的重开等到下一次用到才做，由
//! [`access::HostBrowser::get`] 驱动。

// 宿主层不参与黄金回放，确定性约束（见 clippy.toml）只针对内核。
#![allow(clippy::disallowed_methods)]

pub mod access;
pub mod netlog;
pub mod ops;
pub mod taps;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use process_wrap::tokio::{ChildWrapper, CommandWrap};
use riot_protocol::browser::{Command, Event, TabId};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, mpsc, oneshot};

/// 单条 CDP 命令的等待上限。
///
/// 页面可能卡在一个永远不返回的脚本里，而 CDP 的响应是跟着页面走的。
/// 没有上限的话，一次 `Runtime.evaluate` 就能把调用它的工具永久挂住。
const CDP_TIMEOUT: Duration = Duration::from_secs(30);

/// 等响应的 CDP 请求表。key 是 CDP 的 `id`。
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    #[error("浏览器进程未运行")]
    NotRunning,
    #[error(
        "找不到浏览器程序 {0}。\
         开发时先跑 scripts/build-browser.sh 打包 —— CEF 在 macOS 上必须从 .app 启动。"
    )]
    NotBundled(PathBuf),
    #[error("CDP `{method}` 超过 {}s 没有响应", CDP_TIMEOUT.as_secs())]
    CdpTimeout { method: String },
    #[error("CDP `{method}` 返回错误：{message}")]
    Cdp { method: String, message: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub struct Browser {
    child: Box<dyn ChildWrapper>,
    /// 命令出口。写在单独的任务里 —— 直接持 `ChildStdin` 的话，
    /// 每个调用点都要拿到可变引用，而它们分布在不同的 async 上下文里。
    tx: mpsc::UnboundedSender<Vec<u8>>,
    pending: Pending,
    next_id: AtomicU64,
    /// 进程还在没有。管道断掉时由收发任务翻过来，见 [`Self::alive`]。
    ///
    /// `[约束]` 不能用 `child.try_wait()` 代替。那个要 `&mut self`，而这个
    /// 句柄是被 `Arc` 共享的 —— 拿不到可变引用。
    alive: Arc<AtomicBool>,
}

impl Browser {
    /// 启动浏览器进程。
    ///
    /// `events` 收到的是子进程吐出的每一条 [`Event`]。调用方必须一直排空它:
    /// 通道是无界的，不读的话内存会跟着帧事件一起涨。
    ///
    /// `[约束]` 必须包进程组。CEF 自己会 spawn 五六个 helper，主进程被
    /// SIGKILL 时应用层的清理逻辑不会执行，只有 OS 层的进程组能保证
    /// 那些 helper 跟着一起死。这条和内核那边同理，见 ARCHITECTURE.md §2.3。
    /// `profile` 是浏览器的数据目录。
    ///
    /// `[约束]` 一个目录同时只能有一个实例 —— Chromium 用锁文件独占 profile，
    /// 第二个进程拿不到锁会**直接退出**，这边看到的只是"事件流断了"。
    /// 要同时开多个浏览器（比如每个会话一个）就得给各自不同的目录。
    pub async fn spawn(
        app: PathBuf,
        profile: Option<PathBuf>,
        events: mpsc::UnboundedSender<Event>,
    ) -> Result<Self, BrowserError> {
        let exe = executable_in(&app);
        if !exe.is_file() {
            return Err(BrowserError::NotBundled(app));
        }

        let mut cmd = tokio::process::Command::new(&exe);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(dir) = profile {
            cmd.env("RIOT_BROWSER_PROFILE", dir);
        }

        let mut wrap = CommandWrap::from(cmd);
        #[cfg(unix)]
        wrap.wrap(process_wrap::tokio::ProcessGroup::leader());
        #[cfg(windows)]
        wrap.wrap(process_wrap::tokio::JobObject);

        let mut child = wrap.spawn()?;

        let stdout = child.stdout().take().expect("stdout 是 piped 的");
        let stderr = child.stderr().take().expect("stderr 是 piped 的");
        let mut stdin = child.stdin().take().expect("stdin 是 piped 的");

        // 进程一死，两条管道都会断。哪一条先断不确定 —— stdout 读到 EOF 和
        // stdin 写失败是同一件事的两个侧面，所以两边都翻这个标志。
        let alive = Arc::new(AtomicBool::new(true));

        // 事件读取。一行一条 NDJSON。
        let pending: Pending = Arc::default();
        let routing = Arc::clone(&pending);
        let reader_alive = Arc::clone(&alive);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                match serde_json::from_str::<Event>(&line) {
                    Ok(ev) => {
                        // 带 id 的 CDP 消息是某次调用的响应，交给等它的人；
                        // 不带 id 的是页面事件（Console、Network 之类），
                        // 广播出去。两者混在一条流里，靠 id 分。
                        if route_cdp_response(&routing, &ev).await {
                            continue;
                        }
                        if events.send(ev).is_err() {
                            break; // 接收端没了，读下去没意义
                        }
                    }
                    // 不中断循环。子进程偶尔漏一行脏数据（比如某个依赖
                    // 直接往 stdout 写），不该让整条事件流就此哑掉。
                    Err(e) => tracing::warn!(error = %e, line, "浏览器事件解析失败"),
                }
            }
            // 进程没了。
            //
            // 先记下来再叫醒等待者:反过来的话，被唤醒的一方拿着
            // `NotRunning` 去问「进程还在吗」会得到"在" —— 于是它按
            // "偶发失败"处理，而实际上这个句柄已经永久废了。
            //
            // 叫醒是必须的，否则它们要各自等满 30 秒才超时，而那三十秒里
            // 工具看起来只是"卡住"。
            reader_alive.store(false, Ordering::Relaxed);
            tracing::warn!("浏览器进程的事件流断了，这个句柄之后一律报「未运行」");
            routing.lock().await.clear();
        });

        // CEF 的日志全在 stderr，量很大。转成 debug 级别，默认不显示，
        // 但排查时打开 RIOT_LOG=debug 就能看到 —— 丢掉的话，浏览器出问题
        // 时这边完全是黑的。
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "riot_browser", "{line}");
            }
        });

        // 命令出口。
        let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let writer_alive = Arc::clone(&alive);
        tokio::spawn(async move {
            while let Some(buf) = rx.recv().await {
                if stdin.write_all(&buf).await.is_err() {
                    break;
                }
                if stdin.flush().await.is_err() {
                    break;
                }
            }
            // 循环结束 = 通道关了或管道断了。drop stdin 制造 EOF，
            // 子进程会自己走完 CEF 的关闭流程。
            //
            // 通道关了那种情况是 [`Self::shutdown`] 主动 drop 了 `tx`，那时
            // 句柄本来就被消费掉了 —— 标志翻不翻都没人再看。
            writer_alive.store(false, Ordering::Relaxed);
        });

        Ok(Self {
            child,
            tx,
            pending,
            next_id: AtomicU64::new(1),
            alive,
        })
    }

    /// 发一条 CDP 命令并等它的响应。
    ///
    /// CDP 的请求/响应靠 `id` 配对，而响应和页面事件混在同一条流里。这里
    /// 分配 id、登记等待者、由读取任务按 id 唤醒 —— 和内核那边的 JSON-RPC
    /// 是同一套结构。
    ///
    /// `[约束]` id 由这里独占分配，而且是**整个进程一套**、不是每个标签页
    /// 一套。让调用方自己填的话，两个工具撞上同一个 id 时，响应会被派给
    /// 错误的等待者；各标签页各发一套号的话，两个标签页的第 1 号响应会撞在
    /// 一起 —— 两种错乱都只在并发时出现，表现为"偶尔拿到别人的结果"。
    pub async fn cdp(
        &self,
        tab: TabId,
        method: &str,
        params: Value,
    ) -> Result<Value, BrowserError> {
        // `[约束]` 进程没了就别再登记等待者。读取任务只在断开的那一刻清一次
        // `pending`，之后登记进去的没有任何人会来叫醒 —— 每一条调用都干等满
        // [`CDP_TIMEOUT`]。那 30 秒里面板只是"卡住"，而且一次崩溃之后**每条**
        // 调用都要各付一遍。
        if !self.alive() {
            return Err(BrowserError::NotRunning);
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let sent = self.send(&Command::Cdp {
            tab,
            payload: serde_json::json!({ "id": id, "method": method, "params": params }),
        });
        if let Err(e) = sent {
            self.pending.lock().await.remove(&id);
            return Err(e);
        }

        let reply = match tokio::time::timeout(CDP_TIMEOUT, rx).await {
            Ok(Ok(v)) => v,
            // 通道被 drop = 浏览器进程没了。
            Ok(Err(_)) => {
                return Err(BrowserError::NotRunning);
            }
            Err(_) => {
                // 登记项要撤掉，否则进程活着但没人取的等待者会一直堆着。
                self.pending.lock().await.remove(&id);
                return Err(BrowserError::CdpTimeout {
                    method: method.to_owned(),
                });
            }
        };

        // CDP 的错误在响应体里，不是传输错误。不翻出来的话，上层拿到一个
        // 没有 result 的对象，只能自己猜是哪儿不对。
        if let Some(err) = reply.get("error") {
            let message = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("未知错误")
                .to_owned();
            return Err(BrowserError::Cdp {
                method: method.to_owned(),
                message,
            });
        }

        Ok(reply.get("result").cloned().unwrap_or(Value::Null))
    }

    /// 进程还在没有。
    ///
    /// `[约束]` 长期持有这个句柄的一方必须查它。子进程崩掉之后句柄不会
    /// 自己失效 —— 它照样能拿在手里，只是每条命令都以
    /// [`BrowserError::NotRunning`] 失败。分不出"这次没发出去"和"这个句柄
    /// 已经废了"的话，一次崩溃就等于整个会话的浏览器永久不可用，而唯一的
    /// 出路是重启应用。见 [`access::HostBrowser::get`]。
    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// 发一条命令。
    ///
    /// 不等回应 —— 协议是异步的，结果通过事件流回来。想要"发了之后拿结果"
    /// 的语义，得在 CDP 那一层按 `id` 配对，那是上层的事。
    pub fn send(&self, cmd: &Command) -> Result<(), BrowserError> {
        // 通道本身还开着不代表命令送得到:写任务此刻可能正停在等下一条命令上，
        // 要等它真的往一根断掉的管道里写才发现。少了这一道，死进程上的第一条
        // 命令会"发送成功"然后消失 —— 而调用方据此去等一个永远不来的事件。
        if !self.alive() {
            return Err(BrowserError::NotRunning);
        }
        let mut line = serde_json::to_vec(cmd)?;
        line.push(b'\n');
        self.tx.send(line).map_err(|_| BrowserError::NotRunning)
    }

    /// 发一条 CDP 命令，不等响应。
    ///
    /// 给那些"响应没有信息量、但频率很高"的调用用 —— 典型是 screencast
    /// 的逐帧 ack。走 [`Self::cdp`] 的话每帧都要登记一个等待者、收到响应
    /// 再唤醒，纯属为一个空对象做功。
    ///
    /// `[约束]` 仍然要占一个 id。CDP 不接受没有 id 的命令，而复用固定 id
    /// 会和真正在等结果的调用撞车。
    pub fn cdp_no_wait(
        &self,
        tab: TabId,
        method: &str,
        params: Value,
    ) -> Result<(), BrowserError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.send(&Command::Cdp {
            tab,
            payload: serde_json::json!({ "id": id, "method": method, "params": params }),
        })
    }

    /// 关闭。先请子进程自己收尾，超时再强杀整棵进程树。
    ///
    /// `[约束]` 强杀那一步不能省。CEF 的 helper 进程不受父进程退出影响，
    /// 漏掉就会在用户机器上堆积 —— 表现是"用久了越来越卡"，而且和本应用
    /// 完全对不上号。
    pub async fn shutdown(mut self) {
        let _ = self.send(&Command::Shutdown);

        // 让写任务收到通道关闭，从而 drop stdin 制造 EOF。
        drop(self.tx);

        let graceful = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.child.inner_mut().wait(),
        )
        .await;
        if graceful.is_err() {
            tracing::warn!("浏览器进程未在 5 秒内退出，强杀进程组");
        }

        if let Err(e) = self.child.start_kill() {
            // 进程组已空时返回 ESRCH，那是正常路径。
            tracing::debug!(error = %e, "清理浏览器进程组");
        }
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.child.wait(),
        )
        .await;
    }
}

/// 一个标签页的句柄:浏览器进程 + 标签页号。
///
/// `[取舍]` 有了它，上层不必在每个调用点重复"对哪个标签页"。多标签之后，
/// 每一条 CDP 都要指名页面，而漏传或传错一个号的症状是"命令打在了另一个
/// 页面上" —— 单标签时代永远不会出现的一类错，而且很难从现象倒推。
///
/// 借用而不是持有 `Arc`:标签页句柄的生命周期短（一次操作），而浏览器进程
/// 是长命的，让编译器盯着这个关系比运行时计数便宜。
#[derive(Clone, Copy)]
pub struct Tab<'a> {
    pub browser: &'a Browser,
    pub id: TabId,
}

impl Tab<'_> {
    pub async fn cdp(&self, method: &str, params: Value) -> Result<Value, BrowserError> {
        self.browser.cdp(self.id, method, params).await
    }

    pub fn cdp_no_wait(&self, method: &str, params: Value) -> Result<(), BrowserError> {
        self.browser.cdp_no_wait(self.id, method, params)
    }
}

/// 这条消息是不是某次 CDP 调用的响应；是的话唤醒等待者并返回 `true`。
///
/// 返回 `false` 的两种情况都要继续走事件流:根本不是 CDP 消息，或者是
/// 没有 `id` 的 CDP **事件**（Console、Network 那些推送）。响应和事件混在
/// 同一条流里，`id` 是唯一的区分依据。
async fn route_cdp_response(pending: &Pending, ev: &Event) -> bool {
    let Event::Cdp { payload, .. } = ev else {
        return false;
    };
    let Some(id) = payload.get("id").and_then(Value::as_u64) else {
        return false;
    };
    match pending.lock().await.remove(&id) {
        Some(waiter) => {
            let _ = waiter.send(payload.clone());
        }
        // 没人等的响应。两种来源：调用方超时后响应才姗姗来迟，或者
        // 本来就是 cdp_no_wait 发的。用 trace 而不是 debug —— screencast
        // 的 ack 每帧一条，debug 级别会把日志冲成一片。
        None => tracing::trace!(id, "CDP 响应没有等待者，丢弃"),
    }
    true
}

/// `.app` 里那个可执行文件的位置。
fn executable_in(app: &std::path::Path) -> PathBuf {
    let name = app
        .file_stem()
        .map_or_else(|| "riot-browser".into(), |s| s.to_string_lossy().into_owned());
    app.join("Contents/MacOS").join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 从_app_路径推出可执行文件() {
        let exe = executable_in(std::path::Path::new("/x/riot-browser.app"));
        assert_eq!(
            exe,
            PathBuf::from("/x/riot-browser.app/Contents/MacOS/riot-browser")
        );
    }

    #[tokio::test]
    async fn 没打包时报错要指出怎么修() {
        // 开发时最容易撞上的一条:忘了跑 build-browser.sh。
        // 报"文件不存在"没有用，得说清下一步做什么。
        let r = Browser::spawn(
            PathBuf::from("/nonexistent/riot-browser.app"),
            None,
            mpsc::unbounded_channel().0,
        )
        .await;
        let Err(err) = r else {
            panic!("路径不存在时不该起得来");
        };
        let msg = err.to_string();
        assert!(msg.contains("build-browser.sh"), "报错要指路：{msg}");
    }
}
