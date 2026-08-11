//! 真实的子进程执行器。
//!
//! 豁免理由：这是 [`ProcessRunner`] 的真身，职责就是操作真实进程和真实时间。
//! 确定性由调用方通过注入替身来保证，不是由这个文件保证。
#![allow(clippy::disallowed_methods)]

use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use riot_protocol::tool::{ProcessOutput, ProcessRunner, ProcessSpec};
use process_wrap::tokio::{ChildWrapper, CommandWrap};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

/// 单条命令的输出内存上限。
///
/// 这不是给模型看的上限（那个在工具层，小得多），是**内存保护**：
/// `cat /dev/urandom` 或者一个死循环的 `echo` 能在几秒内吃掉几个 GB。
const DEFAULT_MAX_OUTPUT: usize = 8 * 1024 * 1024;

/// SIGTERM 到 SIGKILL 之间的宽限期。
const DEFAULT_GRACE: Duration = Duration::from_millis(500);

/// 超时被杀时的退出码。跟 GNU `timeout` 的惯例保持一致。
const EXIT_TIMEOUT: i32 = 124;

/// 被取消时的退出码。128 + SIGINT。
const EXIT_CANCELLED: i32 = 130;

#[cfg(unix)]
const SIGTERM: i32 = 15;

pub struct SystemProcessRunner {
    max_output_bytes: usize,
    grace: Duration,
}

impl Default for SystemProcessRunner {
    fn default() -> Self {
        Self {
            max_output_bytes: DEFAULT_MAX_OUTPUT,
            grace: DEFAULT_GRACE,
        }
    }
}

impl SystemProcessRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_output(mut self, bytes: usize) -> Self {
        self.max_output_bytes = bytes;
        self
    }

    pub fn with_grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }
}

enum Ended {
    Exited(std::io::Result<std::process::ExitStatus>),
    TimedOut,
    Cancelled,
}

#[async_trait]
impl ProcessRunner for SystemProcessRunner {
    async fn run(
        &self,
        spec: ProcessSpec,
        cancel: CancellationToken,
    ) -> std::io::Result<ProcessOutput> {
        let started = Instant::now();

        let mut cmd = tokio::process::Command::new(&spec.program);
        cmd.args(&spec.args)
            .current_dir(&spec.cwd)
            // `[约束]` stdin 必须是 null。继承父进程 stdin 的话，一条
            // 读标准输入的命令（`cat`、`ssh`、等确认的安装脚本）会一直等，
            // 而那个 stdin 是内核的 JSON-RPC 通道 —— 它既不会有输入，
            // 被读走的字节还会破坏协议。null 让这类命令立即拿到 EOF。
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // 兜底：这个 future 被 drop（调用方超时、任务被取消）时至少
            // 杀掉直接子进程。整组清理走下面的显式路径。
            .kill_on_drop(true);

        // 只覆盖 spec 指定的变量，其余继承。命令需要 PATH、HOME、
        // 各种语言的工具链变量才能正常工作。
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }

        // `[约束]` 必须包进程组 / Job Object。让 **操作系统** 保证子树
        // 跟着一起死 —— 应用层的清理逻辑在 SIGKILL 面前不存在。
        // 见 ARCHITECTURE.md §2.3
        let mut wrap = CommandWrap::from(cmd);
        #[cfg(unix)]
        wrap.wrap(process_wrap::tokio::ProcessGroup::leader());
        #[cfg(windows)]
        wrap.wrap(process_wrap::tokio::JobObject);

        let mut child = wrap.spawn()?;

        let out_pipe = child.stdout().take().expect("stdout 已设为 piped");
        let err_pipe = child.stderr().take().expect("stderr 已设为 piped");

        // 两个管道必须并发读。顺序读会死锁：先读 stdout 到 EOF 的话，
        // 子进程写满 stderr 的管道缓冲区（通常 64KB）就会阻塞，
        // 于是它不再往 stdout 写，而我们还在等 stdout 的 EOF。
        let cap = self.max_output_bytes;
        let h_out = tokio::spawn(drain(out_pipe, cap));
        let h_err = tokio::spawn(drain(err_pipe, cap));

        let ended = {
            let timeout = spec.timeout_ms.map(Duration::from_millis);
            // 等的是 `inner_mut().wait()` —— 命令进程本身，不是整个组。
            //
            // 外层 `wait()` 语义上等全组。实测下来两者通常没有差别，原因是
            // `waitpid(-pgid)` 只能等**自己的**子进程：命令退出后它 fork 的
            // 后台进程被 init 收养，于是 waitpid 立刻拿到 ECHILD 返回。
            //
            // 但那是收养时机凑巧带来的结果，不是保证 —— 后台进程在父进程
            // 还活着时就被等到的话，全组 wait 会一直挂到超时。这里写明
            // "只等命令本身"是为了不依赖那个巧合。
            let inner = child.inner_mut();
            tokio::select! {
                r = inner.wait() => Ended::Exited(r),
                _ = sleep_opt(timeout) => Ended::TimedOut,
                _ = cancel.cancelled() => Ended::Cancelled,
            }
        };

        // 无条件清理整个进程组。
        //
        // `[约束]` 不能写成"只在超时时才杀"。命令正常退出 ≠ 它 spawn 的
        // 后台进程也退出了 —— 那些会被 init 收养成孤儿，一直活到关机。
        // 这类泄漏比"命令卡死"隐蔽得多：功能全对，只是机器越跑越慢。
        terminate_group(child.as_mut(), self.grace).await;

        // `[约束]` 读取任务只能在杀完组之后 await。管道的写端可能还被
        // 后台子进程持有着 —— `bash -c "sleep 100 &"` 会立即返回，但
        // 那个 sleep 继承了 stdout，不杀掉它这里就永远等不到 EOF。
        let (stdout, out_capped) = join(h_out).await?;
        let (stderr, err_capped) = join(h_err).await?;

        if out_capped || err_capped {
            tracing::warn!(
                program = %spec.program,
                cap = cap,
                "命令输出超过内存上限，已截断"
            );
        }

        let (exit_code, timed_out) = match ended {
            Ended::Exited(Ok(status)) => (exit_code_of(status), false),
            Ended::Exited(Err(e)) => return Err(e),
            Ended::TimedOut => (EXIT_TIMEOUT, true),
            Ended::Cancelled => (EXIT_CANCELLED, false),
        };

        Ok(ProcessOutput {
            // 这里用 lossy 是对的，和 `tools::text` 那条"绝不 lossy"的规矩
            // 不冲突：命令输出只给模型看，不会被原样写回任何地方，所以把
            // 无效字节换成 U+FFFD 不会损坏用户的数据。而且输出本来就可能
            // 在字符中间被上限切断。
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            exit_code,
            timed_out,
            duration_ms: started.elapsed().as_millis() as u64,
        })
    }
}

/// 没有超时就永远不醒。
async fn sleep_opt(d: Option<Duration>) {
    match d {
        Some(d) => tokio::time::sleep(d).await,
        None => std::future::pending().await,
    }
}

/// 读到 EOF 或读满上限。返回 (内容, 是否触到上限)。
async fn drain<R>(mut r: R, cap: usize) -> std::io::Result<(Vec<u8>, bool)>
where
    R: AsyncRead + Unpin,
{
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16 * 1024];

    loop {
        let n = r.read(&mut chunk).await?;
        if n == 0 {
            return Ok((buf, false));
        }
        let room = cap.saturating_sub(buf.len());
        if room == 0 {
            // 直接返回，reader 在这里被 drop。写端下一次写会拿到
            // EPIPE / SIGPIPE —— 这正是 `head -n 10` 让上游停下来的机制，
            // 比我们继续读到天荒地老要好。真的忽略 SIGPIPE 的程序由超时兜底。
            return Ok((buf, true));
        }
        buf.extend_from_slice(&chunk[..n.min(room)]);
    }
}

/// 读取任务 panic 属于 bug，但不该让整个会话崩掉。
async fn join(h: tokio::task::JoinHandle<std::io::Result<(Vec<u8>, bool)>>) -> std::io::Result<(Vec<u8>, bool)> {
    match h.await {
        Ok(r) => r,
        Err(e) => Err(std::io::Error::other(format!("读取输出的任务异常：{e}"))),
    }
}

/// 先礼后兵地清掉整个进程组。
async fn terminate_group(child: &mut dyn ChildWrapper, grace: Duration) {
    // 先 SIGTERM。很多程序靠它删临时文件、把缓冲刷到磁盘、
    // 结束自己的子任务。上来就 SIGKILL 会留下一地垃圾。
    #[cfg(unix)]
    {
        if child.signal(SIGTERM).is_ok() {
            // 组已经空了的话 killpg 返回 ESRCH，那是正常路径。
            let _ = tokio::time::timeout(grace, child.wait()).await;
        }
    }

    // 无条件 SIGKILL 整组。走到这里要么 SIGTERM 没用，要么组里还有
    // 不响应它的进程。
    if let Err(e) = child.start_kill() {
        tracing::debug!(error = %e, "清理进程组");
    }

    // reap，避免留下僵尸。
    let _ = tokio::time::timeout(grace, child.wait()).await;
}

fn exit_code_of(status: std::process::ExitStatus) -> i32 {
    if let Some(c) = status.code() {
        return c;
    }
    // Unix 下被信号杀死时 code() 是 None。用 shell 的惯例 128 + signum，
    // 这样模型看到的数字和它在终端里见过的一致（SIGKILL → 137）。
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    -1
}
