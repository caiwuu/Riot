//! stdio 传输：把 MCP 服务器作为子进程拉起来。
//!
//! 豁免理由：这里的职责就是操作真实进程。MCP 服务器是长驻子进程，
//! 不走 `ProcessRunner`（那个抽象是"跑一条命令等它结束"，形状不对）。
#![allow(clippy::disallowed_methods)]

use std::process::Stdio;
use std::time::Duration;

use process_wrap::tokio::{ChildWrapper, CommandWrap};
use tokio::io::AsyncBufReadExt as _;

use crate::hub::ServerSpec;

/// SIGTERM 到 SIGKILL 之间的宽限期。和 riot-runtime 的命令执行器一致。
const KILL_GRACE: Duration = Duration::from_millis(500);

#[cfg(unix)]
const SIGTERM: i32 = 15;

pub(crate) struct SpawnedServer {
    pub child: Box<dyn ChildWrapper>,
    pub stdout: tokio::process::ChildStdout,
    pub stdin: tokio::process::ChildStdin,
}

/// 把 `CREATE_SUSPENDED | CREATE_NO_WINDOW` 焊在 spawn 前的最后一笔，
/// 否则打包后的 GUI 主程序每连一个 MCP 服务器就弹一个黑色控制台窗
/// （dev 下继承终端的控制台，看不出来）。
///
/// `[约束]` 必须注册在 JobObject **之后**、不能用直接 creation_flags 或
/// 库自带的 CreationFlags 包装器 —— 前者被 JobObject 整个改写，后者
/// 在 process-wrap 9.1.0 里因为 spawn 时包装器表被 mem::take 拿空而
/// 永远读不到。完整分析见 riot-runtime 的命令执行器。
#[cfg(windows)]
#[derive(Debug)]
struct NoWindow;

#[cfg(windows)]
impl process_wrap::tokio::CommandWrapper for NoWindow {
    fn pre_spawn(
        &mut self,
        command: &mut tokio::process::Command,
        _core: &CommandWrap,
    ) -> std::io::Result<()> {
        use windows::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};
        command.creation_flags((CREATE_NO_WINDOW | CREATE_SUSPENDED).0);
        Ok(())
    }
}

/// 拉起服务器进程。stderr 由后台任务转进日志 —— MCP 规范允许服务器往
/// stderr 打日志，不接的话管道写满 64KB 后服务器会整个卡住。
pub(crate) fn spawn_server(spec: &ServerSpec) -> std::io::Result<SpawnedServer> {
    let mut cmd = tokio::process::Command::new(&spec.command);
    cmd.args(&spec.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // 兜底：句柄被 drop 时至少杀掉直接子进程。整组清理走 terminate。
        .kill_on_drop(true);
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }

    // `[约束]` 必须包进程组 / Job Object。`npx foo` 这种启动器会再 spawn
    // 真正的服务器进程 —— 只杀 npx 的话，真身被 init 收养，一直活到关机。
    // 和 riot-runtime 的命令执行器同一条规矩（ARCHITECTURE.md §2.3）。
    let mut wrap = CommandWrap::from(cmd);
    #[cfg(unix)]
    wrap.wrap(process_wrap::tokio::ProcessGroup::leader());
    #[cfg(windows)]
    {
        wrap.wrap(process_wrap::tokio::JobObject);
        wrap.wrap(NoWindow);
    }

    let mut child = wrap.spawn()?;
    let stdout = child.stdout().take().expect("stdout 已设为 piped");
    let stdin = child.stdin().take().expect("stdin 已设为 piped");
    let stderr = child.stderr().take().expect("stderr 已设为 piped");

    let server_id = spec.id.clone();
    tokio::spawn(async move {
        let mut reader = tokio::io::BufReader::new(stderr);
        let mut buf = Vec::new();
        // `[约束]` 按字节读行、有损解码，不能用 `lines()`：它一遇到非 UTF-8
        // 字节就返回 Err，循环退出、stderr 读端被 drop —— 服务器下一次写
        // stderr 直接撞上断掉的管道，Python 会带着打不出来的 traceback
        // 整个退出（真实案例：FastMCP 的启动横幅经 Windows ANSI 码页编码，
        // 横幅打到一半进程就死，表象是"握手前连接断开"）。排水必须活到 EOF。
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf);
                    tracing::debug!(server = %server_id, "{}", line.trim_end());
                }
            }
        }
    });

    Ok(SpawnedServer {
        child,
        stdout,
        stdin,
    })
}

/// 停掉整个进程组：先 SIGTERM 给它收尾的机会，再无条件 SIGKILL。
///
/// `[约束]` 不能只在异常路径杀。服务器正常退出 ≠ 它 spawn 的子进程也
/// 退出了 —— 那些被收养的孤儿比"卡死"隐蔽得多。
pub(crate) async fn terminate(mut child: Box<dyn ChildWrapper>) {
    #[cfg(unix)]
    {
        if child.signal(SIGTERM).is_ok() {
            let _ = tokio::time::timeout(KILL_GRACE, child.wait()).await;
        }
    }
    if let Err(e) = child.start_kill() {
        tracing::debug!(error = %e, "清理 MCP 服务器进程组");
    }
    let _ = tokio::time::timeout(KILL_GRACE, child.wait()).await;
}
