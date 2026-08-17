//! 端到端验证内核二进制的 stdio JSON-RPC 协议。
//!
//! 起真内核(`CARGO_BIN_EXE_riot-kernel` 由 cargo 在测试时注入),用换行
//! 分隔的 JSON 和它对话,断言:请求-应答按 id 配对、无参方法能解析、
//! stdin EOF 触发进程退出。
//!
//! 进程树清理(孤儿、进程组、强杀)不在这里 —— 那是宿主 supervisor 的
//! 职责,在 `src-tauri/tests/process_lifecycle.rs`。这里只管协议本身。

// 端到端起真内核进程:需要真实进程和真实时钟。确定性约束
// (clippy.toml 的 disallowed_methods)针对内核**主循环逻辑**,不是驱动它的
// 测试脚手架 —— 和 supervisor.rs / process_lifecycle.rs 同一处理。
#![allow(clippy::disallowed_methods)]

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

/// 起一个内核子进程,stdin/stdout 都接管。
fn spawn_kernel() -> tokio::process::Child {
    Command::new(env!("CARGO_BIN_EXE_riot-kernel"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // 内核日志走 stderr,继承给测试进程(cargo 会捕获),失败时能看现场。
        .stderr(Stdio::inherit())
        .spawn()
        .expect("内核二进制该能起来")
}

#[tokio::test]
async fn ping_roundtrips_over_stdio() {
    let mut child = spawn_kernel();
    let mut stdin = child.stdin.take().expect("stdin");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();

    stdin
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"kernel.ping\"}\n")
        .await
        .expect("写请求");
    stdin.flush().await.expect("flush");

    let line = timeout(Duration::from_secs(5), lines.next_line())
        .await
        .expect("5 秒内该收到应答")
        .expect("读 stdout")
        .expect("stdout 不该在应答前结束");

    let v: serde_json::Value = serde_json::from_str(&line).expect("应答是合法 JSON");
    assert_eq!(v["id"], 1, "id 要原样回传:{line}");
    assert_eq!(v["result"]["result"], "pong", "应答体是 Pong:{line}");
    assert!(
        v["result"]["data"]["version"].as_str().is_some(),
        "pong 要带版本:{line}"
    );

    // drop stdin → EOF → 内核该自己退出(优雅关闭序列的核心信号)。
    drop(stdin);
    let status = timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("EOF 之后 5 秒内该退出")
        .expect("wait");
    assert!(status.success(), "EOF 关闭应当是干净退出:{status:?}");
}

#[tokio::test]
async fn multiple_requests_pair_by_id() {
    // 并发发两条,应答可能乱序到达,靠 id 配对。这正是 supervisor 的
    // pending 表所依赖的语义。
    let mut child = spawn_kernel();
    let mut stdin = child.stdin.take().expect("stdin");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();

    stdin
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":10,\"method\":\"kernel.ping\"}\n")
        .await
        .unwrap();
    stdin
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":11,\"method\":\"kernel.ping\"}\n")
        .await
        .unwrap();
    stdin.flush().await.unwrap();

    let mut seen = std::collections::HashSet::new();
    for _ in 0..2 {
        let line = timeout(Duration::from_secs(5), lines.next_line())
            .await
            .expect("超时")
            .expect("读")
            .expect("提前结束");
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["result"]["result"], "pong");
        seen.insert(v["id"].as_u64().expect("id 是数字"));
    }
    assert_eq!(seen, [10, 11].into_iter().collect(), "两条 id 都要各回一次");

    drop(stdin);
    let _ = timeout(Duration::from_secs(5), child.wait()).await;
}

async fn write_line(stdin: &mut tokio::process::ChildStdin, v: &serde_json::Value) {
    let mut s = v.to_string();
    s.push('\n');
    stdin.write_all(s.as_bytes()).await.expect("写请求");
    stdin.flush().await.expect("flush");
}

async fn read_json<R: tokio::io::AsyncBufRead + Unpin>(
    lines: &mut tokio::io::Lines<R>,
) -> serde_json::Value {
    let line = timeout(Duration::from_secs(5), lines.next_line())
        .await
        .expect("5 秒内该有一行")
        .expect("读 stdout")
        .expect("stdout 不该提前结束");
    serde_json::from_str(&line).expect("每行都是合法 JSON")
}

/// 端到端:内核作为独立进程,通过 stdio 建会话、跑一轮,事件从 stdout 回流。
///
/// 用空 key 让这一轮在 provider 建构处立即失败(不真打网络),重点验的是
/// **链路**:session.create 拿到 id、turn.submit 被接受、这一轮的 Done 事件
/// 经 event.agent 通知推回来。这就是"内核能作为进程跑会话"的证据。
#[tokio::test]
async fn create_then_submit_streams_done_event() {
    let mut child = spawn_kernel();
    let mut stdin = child.stdin.take().expect("stdin");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();

    // 1. 建会话
    write_line(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "session.create",
            "params": { "cwd": "/tmp", "model": "m" }
        }),
    )
    .await;
    let resp = read_json(&mut lines).await;
    assert_eq!(resp["id"], 1);
    let session_id = resp["result"]["data"]["session_id"]
        .as_str()
        .expect("session.create 要回 session_id")
        .to_owned();

    // 2. 提交一轮(空 api_key → provider 建构立即失败 → Done error 事件)
    let config = serde_json::json!({
        "model": {
            "protocol": "openai", "base_url": "https://api.deepseek.com",
            "api_path": "", "api_key": "", "model": "deepseek-chat"
        },
        "web": { "fetch_enabled": false, "search_enabled": false },
        "vision": { "accepts_images": false },
        "limits": {
            "ask_timeout_secs": 60, "max_turns": 4,
            "compact_threshold_tokens": 100000, "sandbox": "off"
        },
        "mode": "default"
    });
    write_line(
        &mut stdin,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "turn.submit",
            "params": { "session_id": session_id, "input": { "text": "hi" }, "config": config }
        }),
    )
    .await;

    // 后续会混着 turn.submit 的应答(带 id）和事件通知（event.agent，无 id）。
    // 找到这一轮的 Done 事件即算链路打通。
    let mut saw_done = false;
    for _ in 0..12 {
        let v = read_json(&mut lines).await;
        if v["event"] == "event.agent"
            && v["data"]["session_id"] == serde_json::json!(session_id)
            && v["data"]["event"]["type"] == "done"
        {
            saw_done = true;
            break;
        }
    }
    assert!(
        saw_done,
        "内核该把这一轮的 Done 事件经 event.agent 通知推回来"
    );

    drop(stdin);
    let _ = timeout(Duration::from_secs(5), child.wait()).await;
}
