//! 浏览器子进程的生命周期与通信。
//!
//! CEF 跑在 `riot-browser.app` 这个独立进程里。拆出去的理由见那个 crate 的
//! 文档 —— 简单说是 macOS 上 CEF 必须从 `.app` 启动，而主应用在 `tauri dev`
//! 下是裸二进制。
//!
//! 这里的职责和 [`crate::kernel::supervisor`] 是同一类:spawn、包进程组、
//! 收发 NDJSON、崩了能重启。约定也照抄那边 —— 两套进程监管用不同的思路
//! 会让"进程为什么没退干净"这类问题每次都要重新查一遍。

// 宿主层不参与黄金回放，确定性约束（见 clippy.toml）只针对内核。
#![allow(clippy::disallowed_methods)]

use std::path::PathBuf;
use std::process::Stdio;

use process_wrap::tokio::{ChildWrapper, CommandWrap};
use riot_protocol::browser::{Command, Event};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    #[error("浏览器进程未运行")]
    NotRunning,
    #[error(
        "找不到浏览器程序 {0}。\
         开发时先跑 scripts/build-browser.sh 打包 —— CEF 在 macOS 上必须从 .app 启动。"
    )]
    NotBundled(PathBuf),
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

        // 事件读取。一行一条 NDJSON。
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                match serde_json::from_str::<Event>(&line) {
                    Ok(ev) => {
                        if events.send(ev).is_err() {
                            break; // 接收端没了，读下去没意义
                        }
                    }
                    // 不中断循环。子进程偶尔漏一行脏数据（比如某个依赖
                    // 直接往 stdout 写），不该让整条事件流就此哑掉。
                    Err(e) => tracing::warn!(error = %e, line, "浏览器事件解析失败"),
                }
            }
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
        });

        Ok(Self { child, tx })
    }

    /// 发一条命令。
    ///
    /// 不等回应 —— 协议是异步的，结果通过事件流回来。想要"发了之后拿结果"
    /// 的语义，得在 CDP 那一层按 `id` 配对，那是上层的事。
    pub fn send(&self, cmd: &Command) -> Result<(), BrowserError> {
        let mut line = serde_json::to_vec(cmd)?;
        line.push(b'\n');
        self.tx.send(line).map_err(|_| BrowserError::NotRunning)
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
