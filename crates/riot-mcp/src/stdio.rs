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
    wrap.wrap(process_wrap::tokio::JobObject);

    let mut child = wrap.spawn()?;
    let stdout = child.stdout().take().expect("stdout 已设为 piped");
    let stdin = child.stdin().take().expect("stdin 已设为 piped");
    let stderr = child.stderr().take().expect("stderr 已设为 piped");

    let server_id = spec.id.clone();
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::debug!(server = %server_id, "{line}");
        }
    });

    Ok(SpawnedServer { child, stdout, stdin })
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
