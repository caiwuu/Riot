//! 宿主 ↔ 浏览器子进程的端到端验证。
//!
//! 这条链路跨进程、跨 crate、跨 workspace，中间任何一环对不上都不会有
//! 编译错误 —— 只会表现为"发了命令没反应"。所以必须真的起一个进程。
//!
//! `[前提]` 需要先跑 `scripts/build-browser.sh` 打包。没打包时用例跳过而
//! 不是失败:CEF 的二进制有 355MB，不该成为跑一次 `cargo test` 的前置条件。

// 这里等的是**真实进程**的真实往返。确定性时钟那条约束（见 clippy.toml）
// 针对的是内核逻辑 —— 那里的时间必须可控才能做黄金回放；而这个用例的
// 全部意义就是验证真实 IPC，注入时钟只会让它测不到该测的东西。
#![allow(clippy::disallowed_methods)]

use std::path::PathBuf;
use std::time::Duration;

use riot_host_lib::browser::Browser;
use riot_protocol::browser::{Command, Event};
use tokio::sync::mpsc;

/// 每个用例一个独立 profile。
///
/// `[约束]` 共用一个目录的话，并行跑的第二个实例会因为拿不到 Chromium 的
/// profile 锁而直接退出，报出来是"事件流断了" —— 看起来像通信坏了，
/// 实际是两个进程在抢同一份用户数据。
fn profile(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("riot-browser-test-{}-{tag}", std::process::id()));
    let _ = std::fs::create_dir_all(&p);
    p
}

fn bundle() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../crates/riot-browser/target/bundle/riot-browser.app");
    p.is_dir().then_some(p)
}

/// 等一个满足条件的事件，超时就失败。
///
/// 不能只等"下一条" —— 帧事件随时可能插进来，按顺序断言会随机红。
async fn wait_for(
    rx: &mut mpsc::UnboundedReceiver<Event>,
    secs: u64,
    what: &str,
    pred: impl Fn(&Event) -> bool,
) -> Event {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!left.is_zero(), "等 {what} 超时");
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(ev)) if pred(&ev) => return ev,
            Ok(Some(_)) => continue,
            Ok(None) => panic!("等 {what} 时事件流断了"),
            Err(_) => panic!("等 {what} 超时"),
        }
    }
}

#[tokio::test]
async fn 宿主能驱动浏览器加载页面并跑_cdp() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包，先跑 scripts/build-browser.sh");
        return;
    };

    let (tx, mut rx) = mpsc::unbounded_channel();
    let browser = Browser::spawn(app, Some(profile("cdp")), tx)
        .await
        .expect("起浏览器");

    wait_for(&mut rx, 30, "ready", |e| matches!(e, Event::Ready)).await;

    // 帧能出来，说明离屏渲染这条路是活的。
    wait_for(&mut rx, 30, "首帧", |e| matches!(e, Event::Frame { .. })).await;

    browser
        .send(&Command::Navigate {
            url: "https://example.com/".into(),
        })
        .expect("发导航");

    let ev = wait_for(&mut rx, 40, "example.com 加载完成", |e| {
        matches!(e, Event::LoadEnd { url, .. } if url.contains("example.com"))
    })
    .await;
    let Event::LoadEnd { status, .. } = ev else {
        unreachable!()
    };
    assert_eq!(status, 200, "example.com 应当 200");

    // CDP 打在真实页面上。这一条同时验证了发送（send_dev_tools_message）
    // 和接收（DevToolsMessageObserver）两个方向。
    browser
        .send(&Command::Cdp {
            payload: serde_json::json!({
                "id": 1,
                "method": "Runtime.evaluate",
                "params": { "expression": "document.title", "returnByValue": true },
            }),
        })
        .expect("发 CDP");

    let ev = wait_for(&mut rx, 30, "CDP 响应", |e| {
        matches!(e, Event::Cdp { payload } if payload.get("id") == Some(&serde_json::json!(1)))
    })
    .await;
    let Event::Cdp { payload } = ev else {
        unreachable!()
    };
    assert_eq!(
        payload["result"]["result"]["value"], "Example Domain",
        "CDP 应当取回真实页面标题，实际：{payload}"
    );

    browser.shutdown().await;
}

#[tokio::test]
async fn 改视口之后帧尺寸跟着变() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    let (tx, mut rx) = mpsc::unbounded_channel();
    let browser = Browser::spawn(app, Some(profile("resize")), tx)
        .await
        .expect("起浏览器");
    wait_for(&mut rx, 30, "ready", |e| matches!(e, Event::Ready)).await;

    browser
        .send(&Command::Resize {
            width: 640,
            height: 480,
        })
        .expect("发 resize");

    // `[约束]` 尺寸必须真的传到 CEF。只改本地变量而没调 was_resized 的话，
    // 帧会一直是旧尺寸，面板上表现为"拖动没反应"。
    wait_for(&mut rx, 30, "640x480 的帧", |e| {
        matches!(e, Event::Frame { width: 640, height: 480, .. })
    })
    .await;

    browser.shutdown().await;
}
