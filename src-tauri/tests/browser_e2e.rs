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

use riot_host_lib::browser::{Browser, ops};
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
    let result = browser
        .cdp(
            "Runtime.evaluate",
            serde_json::json!({ "expression": "document.title", "returnByValue": true }),
        )
        .await
        .expect("CDP 调用");

    assert_eq!(
        result["result"]["value"], "Example Domain",
        "CDP 应当取回真实页面标题，实际：{result}"
    );

    browser.shutdown().await;
}

#[tokio::test]
async fn cdp_按_id_配对_并发调用不会串() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    let (tx, mut rx) = mpsc::unbounded_channel();
    let browser = Browser::spawn(app, Some(profile("pair")), tx)
        .await
        .expect("起浏览器");
    wait_for(&mut rx, 30, "ready", |e| matches!(e, Event::Ready)).await;

    // `[约束]` 并发发出去的命令，响应必须回到各自的调用方。
    //
    // id 一旦撞车，响应就会派给错误的等待者 —— 而那种错乱只在并发时出现，
    // 单条一条地测永远测不到。这里同时发五条、每条算一个不同的算式，
    // 谁拿错了结果就对不上。
    let calls = (1..=5).map(|n| {
        let b = &browser;
        async move {
            let r = b
                .cdp(
                    "Runtime.evaluate",
                    serde_json::json!({
                        "expression": format!("{n} * 100"),
                        "returnByValue": true,
                    }),
                )
                .await
                .expect("CDP 调用");
            (n, r["result"]["value"].as_i64().expect("数值结果"))
        }
    });

    let results = futures::future::join_all(calls).await;
    for (n, got) in results {
        assert_eq!(got, n * 100, "第 {n} 条调用拿到了别人的响应");
    }

    browser.shutdown().await;
}

#[tokio::test]
async fn cdp_的错误会翻出来而不是当成成功() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    let (tx, mut rx) = mpsc::unbounded_channel();
    let browser = Browser::spawn(app, Some(profile("cdperr")), tx)
        .await
        .expect("起浏览器");
    wait_for(&mut rx, 30, "ready", |e| matches!(e, Event::Ready)).await;

    // CDP 把错误放在响应体的 `error` 字段里，传输层是成功的。
    // 不翻出来的话，上层拿到一个没有 result 的对象，只能自己猜哪儿不对。
    let err = browser
        .cdp("NoSuch.method", serde_json::json!({}))
        .await
        .expect_err("不存在的方法应当报错");

    assert!(
        matches!(err, riot_host_lib::browser::BrowserError::Cdp { .. }),
        "应当是 CDP 错误，实际：{err}"
    );

    browser.shutdown().await;
}

#[tokio::test]
async fn 高层操作在真实页面上成立() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    let (tx, mut rx) = mpsc::unbounded_channel();
    let browser = Browser::spawn(app, Some(profile("ops")), tx)
        .await
        .expect("起浏览器");
    wait_for(&mut rx, 30, "ready", |e| matches!(e, Event::Ready)).await;

    // console 钩子要在导航**之前**装。页面加载期间的报错最有价值，
    // 而那时候如果还没装钩子就永远抓不到了。
    ops::install_console_hook(&browser).await.expect("装钩子");

    // 用 data: URL 而不是真实站点:这条测的是操作本身，不该被网络波动
    // 或者某个站改版搞红。
    // `[约束]` charset 必须写。不写的话 Chromium 按 Latin-1 解，中文全变
    // 成乱码 —— 而快照里的乱码看起来像是 a11y 提取写错了。
    //
    // `[约束]` 里面的 `#` 要写成 `%23`。data: URL 里 `#` 开始 fragment，
    // 一个字面的 `href='#x'` 会把文档从那里截断，后面的标签根本不进 DOM
    // —— 现象是"某些元素在快照里神秘消失"。
    let page = "data:text/html;charset=utf-8,\
        <html><body>\
        <h1>Riot 测试页</h1>\
        <button>提交</button>\
        <a href='%23x'>帮助链接</a>\
        <script>console.warn('来自页面的警告');</script>\
        </body></html>";
    ops::navigate(&browser, page).await.expect("导航");
    wait_for(&mut rx, 30, "页面加载完成", |e| {
        matches!(e, Event::LoadEnd { url, .. } if url.starts_with("data:"))
    })
    .await;

    // 快照:要能看见可交互元素，且不该被结构性节点淹没。
    let snap = ops::snapshot(&browser).await.expect("快照");
    assert!(snap.contains("提交"), "快照里应当有按钮名：{snap}");
    assert!(snap.contains("帮助链接"), "快照里应当有链接名：{snap}");
    assert!(
        !snap.contains("generic"),
        "结构性节点不该出现在快照里：{snap}"
    );

    // 截图:PNG 的 base64。只验非空和能解码 —— 像素内容不该被断言，
    // 那会让用例随字体渲染的细微变化而红。
    let shot = ops::screenshot(&browser).await.expect("截图");
    assert!(shot.len() > 1000, "截图太小，可能是白屏：{} 字节", shot.len());

    // console:钩子装在导航前，所以页面脚本里的 warn 应当被抓到。
    let logs = ops::console(&browser).await.expect("取 console");
    assert!(
        logs.iter().any(|l| l.contains("来自页面的警告")),
        "应当抓到页面加载期间的 console：{logs:?}"
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
