//! 子进程不继承父进程 stdin 的验证。
//!
//! # 为什么要单独一个测试二进制
//!
//! 这个测试要替换掉**进程自己的 fd 0**。和别的测试同住一个二进制的话会波及
//! 它们，而 Rust 的测试是同进程多线程跑的。单独一个文件就是单独一个二进制，
//! 副作用被进程边界关住。
//!
//! # 为什么不能在普通测试里验证
//!
//! `cargo test` 给测试进程的 stdin 通常已经是 EOF 或 /dev/null。那种环境下
//! `Stdio::null()` 和 `Stdio::inherit()` 的表现完全一样 —— `cat` 两种情况下
//! 都立即返回。于是"验证了 stdin 是 null"的测试实际上什么都没验证。
//!
//! 这是变异测试查出来的：把 `null()` 改成 `inherit()`，21 个真实进程测试
//! 一个都没红。**测试环境比生产环境宽容**，而宽容的那部分正好盖住了要测的东西。
//!
//! 解法是自己造一个永远不会 EOF 的 stdin：管道的写端留在手里不关。
// 这些测试的全部意义就是真跑 OS：真起进程、真等时间。禁用列表
// 针对的是内核逻辑，不是它的验证。
#![allow(clippy::disallowed_methods)]

#![cfg(unix)]

use std::time::Duration;

use riot_protocol::tool::{ProcessRunner, ProcessSpec};
use riot_runtime::SystemProcessRunner;
use tokio_util::sync::CancellationToken;

/// 把进程自己的 fd 0 换成一个永不 EOF 的管道读端。
///
/// 返回写端 —— 调用方必须持有它。一旦 drop，管道就 EOF 了，
/// 这个测试也就失去意义。
fn hijack_stdin() -> std::fs::File {
    use std::os::fd::FromRawFd;

    let mut fds = [0i32; 2];
    // SAFETY: fds 是长度 2 的合法数组，pipe 的标准用法。
    let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
    assert_eq!(rc, 0, "建管道失败");

    let (read_fd, write_fd) = (fds[0], fds[1]);

    // SAFETY: read_fd 来自刚建好的管道，0 是合法的目标 fd。
    let rc = unsafe { libc::dup2(read_fd, 0) };
    assert_eq!(rc, 0, "替换 fd 0 失败");

    // SAFETY: read_fd 已经复制到 fd 0，原来的可以关掉。
    unsafe { libc::close(read_fd) };
    // SAFETY: write_fd 的所有权转移给 File，由它负责关闭。
    unsafe { std::fs::File::from_raw_fd(write_fd) }
}

#[tokio::test(flavor = "multi_thread")]
async fn 子进程不继承父进程的_stdin() {
    // 写端留在手里：现在 fd 0 是一个永远不会 EOF 的管道。
    // 子进程要是继承了它，`cat` 就会一直读下去。
    let _keep_open = hijack_stdin();

    let spec = ProcessSpec {
        program: "bash".into(),
        args: vec!["-c".into(), "cat".into()],
        cwd: std::env::temp_dir(),
        env: Vec::new(),
        // 故意给一个很长的超时。要是靠超时才结束，说明 stdin 继承了 ——
        // 那种"能返回"不是我们要的。
        timeout_ms: Some(60_000),
    };

    let out = tokio::time::timeout(
        Duration::from_secs(5),
        SystemProcessRunner::new().run(spec, CancellationToken::new()),
    )
    .await
    .expect("stdin 必须是 null —— 继承的话 cat 会一直等这个永不 EOF 的管道")
    .expect("命令能起来");

    assert_eq!(out.exit_code, 0, "应该是正常读到 EOF 后退出");
    assert!(!out.timed_out, "不能是靠超时才结束的");
    assert!(out.stdout.is_empty());
}
