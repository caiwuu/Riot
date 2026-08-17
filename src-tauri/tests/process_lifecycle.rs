//! 内核进程生命周期测试。
//!
//! 这是宿主层第一个该写的测试。理由：进程树清理是唯一没有官方方案的部分
//! （Tauri 的 sidecar 至今不做这件事），而且做错了不会立刻报错 ——
//! 它表现为「开发机上跑了一天之后风扇狂转」，很难归因。
//!
//! 假内核用 shell 脚本模拟，因为真内核还没写，而这一层要验证的是**进程语义**，
//! 与内核内部逻辑无关。

#![cfg(unix)]
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use riot_host_lib::kernel::Kernel;
use tokio::sync::mpsc;

/// 写一个假内核脚本。
///
/// 它做三件事：spawn 一个长命子进程（用来验证进程树清理）、
/// 应答 JSON-RPC、收到 EOF 后留下痕迹。
///
/// `tag` 必须每轮不同。复用同一个文件名会和上一轮尚未退出的 sh 抢同一个
/// inode —— `std::fs::write` 是截断写不是原子替换，正在执行它的进程会读到
/// 半截脚本或直接 ETXTBSY。
fn write_fake_kernel(dir: &Path, tag: &str, child_pid_file: &Path, eof_file: &Path) -> PathBuf {
    let script = dir.join(format!("fake_kernel_{tag}.sh"));
    let body = format!(
        r#"#!/bin/sh
# 长命子进程。父进程被杀之后它如果还活着，就是孤儿泄漏。
sleep 300 &
echo $! > "{child_pid}"

# 应答任何请求。id 硬编码为 1 —— 测试里只发一个请求。
while IFS= read -r line; do
  printf '{{"jsonrpc":"2.0","id":1,"result":null}}\n'
done

# 走到这里说明 stdin 关了（EOF）。这是优雅关闭的信号。
echo done > "{eof}"
"#,
        child_pid = child_pid_file.display(),
        eof = eof_file.display(),
    );
    std::fs::write(&script, body).expect("写脚本");
    let mut perm = std::fs::metadata(&script).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
    std::fs::set_permissions(&script, perm).expect("加执行位");
    script
}

fn is_alive(pid: u32) -> bool {
    // kill -0 只探测存在性，不发信号。
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 等一个条件成立，最多 `limit`。轮询而不是固定 sleep —— 固定 sleep
/// 要么慢要么在负载高的 CI 上偶发失败。
async fn eventually(limit: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + limit;
    while std::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    cond()
}

/// 等假内核把孙子进程的 PID 写出来。
///
/// 等的是「能解析出一个 PID」，不是「文件存在」。`echo $! > f` 的 open 和
/// write 是两步，文件出现之后内容可能还是空的 —— 按 exists() 判断会随机
/// 读到空串。这类竞态在快速循环里才显形，单跑一次永远看不到。
async fn read_child_pid(path: &Path) -> Option<u32> {
    let mut found = None;
    // 10 秒不是"这一步要花这么久"，是给满载的机器留余量：全量测试并行
    // 跑时，一个新起的 shell 拿到 CPU 可能要好几秒。条件一满足就立刻
    // 返回，所以放宽只影响失败路径的等待时间。
    eventually(Duration::from_secs(10), || {
        found = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok());
        found.is_some()
    })
    .await;
    found
}

#[tokio::test]
async fn 关闭时内核收到_eof_有机会收尾() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("child.pid");
    let eof_file = dir.path().join("eof.marker");
    let script = write_fake_kernel(dir.path(), "eof", &pid_file, &eof_file);

    let (tx, _rx) = mpsc::unbounded_channel();
    let kernel = Kernel::spawn(script, &[], tx).await.expect("启动假内核");
    read_child_pid(&pid_file).await.expect("假内核该起来了");

    kernel.shutdown().await;

    assert!(
        eventually(Duration::from_secs(3), || eof_file.exists()).await,
        "内核没收到 EOF —— 说明 stdin 没被 drop。\
         这正是 Tauri 官方 sidecar 做不到的事（CommandChild 没有关 stdin 的方法），\
         也是我们不用它的首要原因。"
    );
}

#[tokio::test]
async fn 内核退出后不留孤儿子进程() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("child.pid");
    let eof_file = dir.path().join("eof.marker");
    let script = write_fake_kernel(dir.path(), "orphan", &pid_file, &eof_file);

    let (tx, _rx) = mpsc::unbounded_channel();
    let kernel = Kernel::spawn(script, &[], tx).await.expect("启动假内核");
    let child_pid = read_child_pid(&pid_file).await.expect("拿到孙子进程 PID");
    assert!(is_alive(child_pid), "前置条件：孙子进程该活着");

    kernel.shutdown().await;

    assert!(
        eventually(Duration::from_secs(5), || !is_alive(child_pid)).await,
        "孙子进程 {child_pid} 在内核退出后仍然存活 —— 进程组没生效。\
         检查 supervisor 里的 ProcessGroup::leader() / JobObject 包装还在不在。"
    );
}

#[tokio::test]
async fn 强杀内核也不留孤儿() {
    // 模拟 `tauri dev` 停止时的 SIGKILL、以及 NSIS 升级时的 TerminateProcess。
    // 这两条路径上应用层的清理钩子根本不会执行，只有 OS 层的进程组能兜住。
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("child.pid");
    let eof_file = dir.path().join("eof.marker");
    let script = write_fake_kernel(dir.path(), "kill", &pid_file, &eof_file);

    let (tx, _rx) = mpsc::unbounded_channel();
    let kernel = Kernel::spawn(script, &[], tx).await.expect("启动假内核");
    let child_pid = read_child_pid(&pid_file).await.expect("拿到孙子进程 PID");

    kernel.kill_now().await;

    assert!(
        eventually(Duration::from_secs(5), || !is_alive(child_pid)).await,
        "跳过优雅关闭直接强杀时，孙子进程 {child_pid} 泄漏了。\
         这条路径没有任何应用层兜底，只能靠进程组。"
    );
    assert!(
        !eof_file.exists(),
        "强杀路径不该走到 EOF 收尾 —— 如果走到了，说明这个测试没测到它想测的东西"
    );
}

#[tokio::test]
async fn 反复启停不累积残留() {
    const ROUNDS: usize = 20;
    let dir = tempfile::tempdir().unwrap();
    let mut leaked = Vec::new();

    for i in 0..ROUNDS {
        let pid_file = dir.path().join(format!("child_{i}.pid"));
        let eof_file = dir.path().join(format!("eof_{i}.marker"));
        let script = write_fake_kernel(dir.path(), &i.to_string(), &pid_file, &eof_file);

        let (tx, _rx) = mpsc::unbounded_channel();
        let kernel = Kernel::spawn(script, &[], tx).await.expect("启动假内核");
        let pid = read_child_pid(&pid_file).await.expect("拿到孙子进程 PID");

        // 交替走优雅关闭和强杀，两条路径都要干净。
        if i % 2 == 0 {
            kernel.shutdown().await;
        } else {
            kernel.kill_now().await;
        }

        if !eventually(Duration::from_secs(5), || !is_alive(pid)).await {
            leaked.push((i, pid));
        }
    }

    assert!(
        leaked.is_empty(),
        "{ROUNDS} 轮启停后有 {} 个孤儿：{leaked:?}。\
         偶发泄漏比稳定泄漏更危险 —— 它在开发机上要跑很久才显形。",
        leaked.len()
    );
}

/// 当前用户的进程数。用来发现「每轮都回收不干净」这类缓慢累积。
fn process_count() -> usize {
    std::process::Command::new("sh")
        .args(["-c", "ps -u $(id -u) -o pid= | wc -l"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// 千次启停。只在 nightly 跑（`cargo test --release -- --ignored chaos_soak`）。
///
/// 20 轮抓得到稳定泄漏，抓不到千分之几的竞态 —— 而那正是最难查的那种：
/// 用户抱怨「用久了变卡」，本地怎么都复现不了。
///
/// 这里除了查孤儿，还盯着进程总数。第一次跑这个测试就撞上了
/// `ulimit -u`（macOS 默认 2666）—— 不是逻辑错，是 reap 跟不上创建速度。
/// 断言进程数不随轮次增长，比只断言「没有孤儿」更早发现问题。
#[tokio::test]
#[ignore = "长跑，只在 nightly"]
async fn chaos_soak_启停千次() {
    const ROUNDS: usize = 1000;
    let dir = tempfile::tempdir().unwrap();
    let mut leaked = Vec::new();

    let baseline = process_count();
    let mut peak = baseline;

    for i in 0..ROUNDS {
        let pid_file = dir.path().join(format!("child_{i}.pid"));
        let eof_file = dir.path().join(format!("eof_{i}.marker"));
        let script = write_fake_kernel(dir.path(), &i.to_string(), &pid_file, &eof_file);

        let (tx, _rx) = mpsc::unbounded_channel();
        let kernel = Kernel::spawn(script, &[], tx).await.expect("启动假内核");
        let Some(pid) = read_child_pid(&pid_file).await else {
            panic!(
                "第 {i} 轮假内核没起来。当前进程数 {}（基线 {baseline}，峰值 {peak}）。\
                 如果这个数接近 `ulimit -u`，说明前面几轮的进程还没被回收完。",
                process_count()
            );
        };

        if i % 2 == 0 {
            kernel.shutdown().await;
        } else {
            kernel.kill_now().await;
        }

        if !eventually(Duration::from_secs(5), || !is_alive(pid)).await {
            leaked.push((i, pid));
        }
        let _ = std::fs::remove_file(&pid_file);
        let _ = std::fs::remove_file(&eof_file);

        if i % 100 == 0 {
            let now = process_count();
            peak = peak.max(now);
            eprintln!("  第 {i} 轮：进程数 {now}（基线 {baseline}）");
        }
    }

    let ending = process_count();
    assert!(
        leaked.is_empty(),
        "{ROUNDS} 轮后有 {} 个孤儿（前几个：{:?}）",
        leaked.len(),
        &leaked[..leaked.len().min(5)]
    );
    assert!(
        ending < baseline + 50,
        "进程数从 {baseline} 涨到 {ending} —— 每轮都有回收不掉的残留"
    );
}
