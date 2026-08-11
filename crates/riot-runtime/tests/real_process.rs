//! 真实进程测试。
//!
//! 这些测试起真的子进程、等真的时间。跑得比单元测试慢，但它们验证的东西
//! 替身测不出来：管道会不会死锁、孤儿进程有没有被收掉、`cat` 会不会因为
//! 继承 stdin 而挂住。
//!
//! 这一层抓到的问题有个共同特征 —— **本地开发时不会暴露**。孤儿进程不影响
//! 功能，只是机器越跑越慢；管道死锁只在输出够大时发生；stdin 挂住只在模型
//! 恰好跑了一条读标准输入的命令时发生。
// 这些测试的全部意义就是真跑 OS：真起进程、真等时间。禁用列表
// 针对的是内核逻辑，不是它的验证。
#![allow(clippy::disallowed_methods)]

#![cfg(unix)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use riot_protocol::tool::{ProcessRunner, ProcessSpec};
use riot_runtime::SystemProcessRunner;
use tokio_util::sync::CancellationToken;

fn spec(script: &str) -> ProcessSpec {
    ProcessSpec {
        program: "bash".into(),
        args: vec!["-c".into(), script.into()],
        cwd: std::env::temp_dir(),
        env: Vec::new(),
        timeout_ms: Some(10_000),
    }
}

async fn run(script: &str) -> riot_protocol::tool::ProcessOutput {
    SystemProcessRunner::new()
        .run(spec(script), CancellationToken::new())
        .await
        .expect("命令能起来")
}

// ── 基本行为 ──────────────────────────────────────────

#[tokio::test]
async fn 跑一条命令拿到输出() {
    let out = run("echo hello").await;
    assert_eq!(out.stdout.trim(), "hello");
    assert_eq!(out.exit_code, 0);
    assert!(!out.timed_out);
}

#[tokio::test]
async fn stdout_和_stderr_分开() {
    let out = run("echo 正常; echo 出错 >&2").await;
    assert_eq!(out.stdout.trim(), "正常");
    assert_eq!(out.stderr.trim(), "出错");
}

#[tokio::test]
async fn 退出码原样带回() {
    assert_eq!(run("exit 42").await.exit_code, 42);
}

#[tokio::test]
async fn 被信号杀死时用_128_加信号号() {
    // 模型在终端里见过的就是这个数字（SIGKILL → 137）
    let out = run("kill -9 $$").await;
    assert_eq!(out.exit_code, 137);
}

#[tokio::test]
async fn 工作目录生效() {
    let dir = tempfile::tempdir().expect("建临时目录");
    let mut s = spec("pwd");
    s.cwd = dir.path().to_path_buf();

    let out = SystemProcessRunner::new()
        .run(s, CancellationToken::new())
        .await
        .expect("能起来");

    // macOS 的 /var 是 /private/var 的 symlink，比较 canonical 形式
    let got = PathBuf::from(out.stdout.trim()).canonicalize().expect("解析");
    let want = dir.path().canonicalize().expect("解析");
    assert_eq!(got, want);
}

#[tokio::test]
async fn 环境变量生效且不清空继承的() {
    let mut s = spec("echo $MY_VAR; echo ${PATH:+有PATH}");
    s.env = vec![("MY_VAR".into(), "注入的值".into())];

    let out = SystemProcessRunner::new()
        .run(s, CancellationToken::new())
        .await
        .expect("能起来");

    assert!(out.stdout.contains("注入的值"));
    // 只覆盖指定的变量，其余继承 —— 命令需要 PATH 才能找到程序
    assert!(out.stdout.contains("有PATH"), "不能把环境清空：{}", out.stdout);
}

#[tokio::test]
async fn 程序不存在时是_not_found() {
    let s = ProcessSpec {
        program: "这个程序肯定不存在-zzz".into(),
        args: vec![],
        cwd: std::env::temp_dir(),
        env: Vec::new(),
        timeout_ms: Some(5000),
    };

    let err = SystemProcessRunner::new()
        .run(s, CancellationToken::new())
        .await
        .expect_err("应该失败");

    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

// ── stdin ─────────────────────────────────────────────

#[tokio::test]
async fn 读标准输入的命令立即结束而不是挂住() {
    // `[约束]` stdin 必须接 null。继承的话这条命令会一直等输入 ——
    // 而那个 stdin 是内核的 JSON-RPC 通道，既不会有输入，被读走的
    // 字节还会破坏协议。
    let started = Instant::now();
    let out = tokio::time::timeout(Duration::from_secs(5), run("cat"))
        .await
        .expect("不能挂住");

    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.is_empty());
    assert!(started.elapsed() < Duration::from_secs(2), "应该立即返回");
}

#[tokio::test]
async fn 等确认的脚本拿到_eof_而不是死等() {
    let out = tokio::time::timeout(
        Duration::from_secs(5),
        run("read -p '继续吗？' ans; echo \"读到:[$ans]\""),
    )
    .await
    .expect("不能挂住");

    assert!(out.stdout.contains("读到:[]"), "{}", out.stdout);
}

// ── 超时 ──────────────────────────────────────────────

#[tokio::test]
async fn 超时被杀掉() {
    let mut s = spec("sleep 30");
    s.timeout_ms = Some(300);

    let started = Instant::now();
    let out = SystemProcessRunner::new()
        .run(s, CancellationToken::new())
        .await
        .expect("能起来");

    assert!(out.timed_out);
    assert_eq!(out.exit_code, 124);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "不能真等 30 秒：{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn 超时前的输出要保留() {
    // 只说"超时了"等于让模型从零开始猜。超时前的输出往往正好
    // 指出卡在哪一步。
    let mut s = spec("echo 已经跑到第三步; sleep 30");
    s.timeout_ms = Some(500);

    let out = SystemProcessRunner::new()
        .run(s, CancellationToken::new())
        .await
        .expect("能起来");

    assert!(out.timed_out);
    assert!(
        out.stdout.contains("已经跑到第三步"),
        "超时前的输出不能丢：{:?}",
        out.stdout
    );
}

#[tokio::test]
async fn 没有超时设置时跑到自然结束() {
    let mut s = spec("sleep 0.3; echo 跑完了");
    s.timeout_ms = None;

    let out = SystemProcessRunner::new()
        .run(s, CancellationToken::new())
        .await
        .expect("能起来");

    assert!(!out.timed_out);
    assert_eq!(out.stdout.trim(), "跑完了");
}

// ── 取消 ──────────────────────────────────────────────

#[tokio::test]
async fn 取消立即返回() {
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        c.cancel();
    });

    let started = Instant::now();
    let out = SystemProcessRunner::new()
        .run(spec("sleep 30"), cancel)
        .await
        .expect("能起来");

    assert_eq!(out.exit_code, 130);
    assert!(!out.timed_out, "取消不是超时");
    assert!(started.elapsed() < Duration::from_secs(3));
}

// ── 进程组：这一层最容易出事 ──────────────────────────

#[tokio::test]
async fn 后台进程不会拖住返回() {
    // `[约束]` 等的必须是进程本身而不是整个组。等全组的话，这条命令
    // 明明立即结束了，却要等那个 sleep 30 —— 表现是"简单命令莫名卡住"。
    let started = Instant::now();
    let out = run("sleep 30 & echo 主命令结束").await;

    assert_eq!(out.stdout.trim(), "主命令结束");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "被后台进程拖住了：{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn 后台进程会被清理不留孤儿() {
    // 这是之前在内核监管那边真实踩到过的坑：进程正常退出 ≠ 它 spawn 的
    // 后台子进程也退出了。那些会被 init 收养，一直活到关机。
    //
    // 这类泄漏没有任何直接症状 —— 功能全对，只是机器越跑越慢。
    let out = run("sleep 60 & echo $!").await;
    let pid: i32 = out.stdout.trim().parse().expect("拿到后台进程 pid");

    // 给清理一点时间落地
    tokio::time::sleep(Duration::from_millis(300)).await;

    // kill -0 只探测存在性，不真的发信号
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .expect("kill 能跑")
        .status
        .success();

    assert!(!alive, "pid {pid} 还活着 —— 孤儿进程泄漏了");
}

#[tokio::test]
async fn 超时时整棵进程树都被杀() {
    let mut s = spec("sleep 60 & echo $!; sleep 30");
    s.timeout_ms = Some(400);

    let out = SystemProcessRunner::new()
        .run(s, CancellationToken::new())
        .await
        .expect("能起来");

    let pid: i32 = out.stdout.trim().parse().expect("拿到后台进程 pid");
    tokio::time::sleep(Duration::from_millis(300)).await;

    let alive = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .expect("kill 能跑")
        .status
        .success();

    assert!(!alive, "超时杀的是整组，不能只杀直接子进程");
}

// ── 管道 ──────────────────────────────────────────────

#[tokio::test]
async fn 两个流同时大量输出不死锁() {
    // `[约束]` stdout 和 stderr 必须并发读。顺序读的话：先读 stdout 到 EOF，
    // 子进程写满 stderr 的管道缓冲（通常 64KB）就阻塞，于是它不再往 stdout
    // 写，而我们还在等 stdout 的 EOF —— 双方都不动了。
    //
    // 缓冲区大小决定了这个 bug 只在输出够大时出现。小输出的测试全绿，
    // 然后某次 `cargo build` 的警告一多就挂死。
    let script = r#"
        for i in $(seq 1 4000); do
          echo "标准输出的第 $i 行，填充一些内容让它够长"
          echo "标准错误的第 $i 行，填充一些内容让它够长" >&2
        done
    "#;

    let out = tokio::time::timeout(Duration::from_secs(20), run(script))
        .await
        .expect("死锁了");

    assert!(out.stdout.len() > 64 * 1024, "stdout 要够大才有意义");
    assert!(out.stderr.len() > 64 * 1024, "stderr 要够大才有意义");
    assert!(out.stdout.contains("标准输出的第 4000 行"));
    assert!(out.stderr.contains("标准错误的第 4000 行"));
}

#[tokio::test]
async fn 输出超过上限时截断并让上游停下() {
    // `yes` 会一直写到天荒地老。读满上限后我们 drop 掉 reader，
    // 它下次写就拿到 EPIPE —— 这正是 `head -n 10` 让上游停下来的机制。
    let started = Instant::now();
    let out = SystemProcessRunner::new()
        .with_max_output(64 * 1024)
        .run(spec("yes 填充内容"), CancellationToken::new())
        .await
        .expect("能起来");

    assert!(
        out.stdout.len() <= 96 * 1024,
        "没有截断：{} 字节",
        out.stdout.len()
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "应该很快结束，不该等到超时：{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn 输出在字符中间被截断也不崩() {
    // 上限是按字节算的，多字节字符必然会被切开。
    let out = SystemProcessRunner::new()
        .with_max_output(1001)
        .run(
            spec("for i in $(seq 1 500); do printf '中文字符测试'; done"),
            CancellationToken::new(),
        )
        .await
        .expect("能起来");

    assert!(!out.stdout.is_empty());
    assert!(out.stdout.len() <= 1100);
}

#[tokio::test]
async fn 二进制输出不会让整条命令失败() {
    let out = run("printf '\\x00\\x01\\xff\\xfe'").await;
    assert_eq!(out.exit_code, 0);
}

// ── 耗时 ──────────────────────────────────────────────

#[tokio::test]
async fn 报告执行耗时() {
    let out = run("sleep 0.3").await;
    assert!(
        out.duration_ms >= 250 && out.duration_ms < 5000,
        "耗时不合理：{}ms",
        out.duration_ms
    );
}
