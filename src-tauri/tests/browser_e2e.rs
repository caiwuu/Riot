//! 宿主 ↔ 浏览器子进程的端到端验证。
//!
//! 这条链路跨进程、跨 crate、跨 workspace，中间任何一环对不上都不会有
//! 编译错误 —— 只会表现为"发了命令没反应"。所以必须真的起一个进程。
//!
//! `[前提]` 需要先跑 `scripts/build-browser.sh`（Windows 上是
//! `scripts/build-browser.ps1`）打包。没打包时用例跳过而不是失败:
//! CEF 的二进制有 355MB，不该成为跑一次 `cargo test` 的前置条件。

// 这里等的是**真实进程**的真实往返。确定性时钟那条约束（见 clippy.toml）
// 针对的是内核逻辑 —— 那里的时间必须可控才能做黄金回放；而这个用例的
// 全部意义就是验证真实 IPC，注入时钟只会让它测不到该测的东西。
#![allow(clippy::disallowed_methods)]

use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
use riot_host_lib::browser::access::TabInfo;
use riot_host_lib::browser::{Browser, Tab, ops};
use riot_protocol::browser::{Command, Event, TabId};
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
    // 打包布局按平台走:macOS 是 .app，Windows 是平铺目录。
    // 跳过判据 = 可执行文件存在，与 Browser::spawn 同源
    // （riot_host_lib::browser::executable_in）—— 只查目录的话，CI 为了
    // 过 tauri-build 资源检查造的空占位目录会让这批用例不跳过、全失败。
    #[cfg(windows)]
    const BUNDLE: &str = "../crates/riot-browser/target/bundle/riot-browser";
    #[cfg(not(windows))]
    const BUNDLE: &str = "../crates/riot-browser/target/bundle/riot-browser.app";
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(BUNDLE);
    riot_host_lib::browser::executable_in(&p)
        .is_file()
        .then_some(p)
}

/// 开第一个标签页，等它就绪。
///
/// `[前提]` 浏览器进程起来时**没有任何标签页** —— 开哪些页是主应用的决定。
/// 所以每个直接驱动 `Browser` 的用例都得先开一个，不然所有命令都会以
/// "标签页不存在"被忽略。
async fn open_tab(browser: &Browser, rx: &mut mpsc::UnboundedReceiver<Event>) -> TabId {
    const FIRST: TabId = 1;
    browser
        .send(&Command::OpenTab { tab: FIRST })
        .expect("发开标签页");
    wait_for(
        rx,
        30,
        "标签页就绪",
        |e| matches!(e, Event::TabOpened { tab } if *tab == FIRST),
    )
    .await;
    FIRST
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
    let id = open_tab(&browser, &mut rx).await;
    let tab = Tab {
        browser: &browser,
        id,
    };

    // 帧能出来，说明离屏渲染这条路是活的。
    wait_for(&mut rx, 30, "首帧", |e| matches!(e, Event::Frame { .. })).await;

    browser
        .send(&Command::Navigate {
            tab: id,
            url: "https://example.com/".into(),
        })
        .expect("发导航");

    let ev = wait_for(
        &mut rx,
        40,
        "example.com 加载完成",
        |e| matches!(e, Event::LoadEnd { url, .. } if url.contains("example.com")),
    )
    .await;
    let Event::LoadEnd { status, .. } = ev else {
        unreachable!()
    };
    assert_eq!(status, 200, "example.com 应当 200");

    // CDP 打在真实页面上。这一条同时验证了发送（send_dev_tools_message）
    // 和接收（DevToolsMessageObserver）两个方向。
    let result = tab
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

/// 进程没了之后，句柄要说得出自己废了。
///
/// 这是崩溃自愈的地基（见 `HostBrowser::get`）。没有这个信号的话，长期
/// 持有句柄的一方分不出"这条命令没发出去"和"这个进程已经不在了" ——
/// 于是 CEF 崩一次就等于整个会话的浏览器永久不可用，用户唯一的出路是
/// 重启应用。
#[tokio::test]
async fn 进程退出后句柄不再声称自己活着() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    let (tx, mut rx) = mpsc::unbounded_channel();
    let browser = Browser::spawn(app, Some(profile("alive")), tx)
        .await
        .expect("起浏览器");
    wait_for(&mut rx, 30, "ready", |e| matches!(e, Event::Ready)).await;
    assert!(browser.alive(), "刚就绪就说自己没了");

    // 一个标签页都不开:那条路上子进程会直接退出消息循环，不必等 CEF
    // 逐页销毁 —— 这个用例要的只是"进程走掉"。
    browser.send(&Command::Shutdown).expect("发关闭");

    // 事件流断掉 = 子进程的 stdout 到了 EOF。存活标志在那之前一步翻，
    // 所以收到 None 的时候它已经是 false 了，不用睡着等。
    tokio::time::timeout(Duration::from_secs(30), async {
        while rx.recv().await.is_some() {}
    })
    .await
    .expect("等进程退出超时");

    assert!(
        !browser.alive(),
        "进程都退了还说自己活着 —— 上层会一直拿着这个死句柄，永远不重开"
    );

    // 命令也要当场失败。通道本身还开着（写任务停在等下一条命令上），所以
    // 光看 `tx.send` 的话这一条会"发送成功"然后消失 —— 而调用方据此去等一个
    // 永远不来的 `TabOpened`，白等满十秒。
    let err = browser
        .send(&Command::OpenTab { tab: 99 })
        .expect_err("死句柄不该收命令");
    assert!(
        matches!(err, riot_host_lib::browser::BrowserError::NotRunning),
        "要说得出是进程没了，而不是别的毛病：{err}"
    );
}

/// 浏览器崩掉之后，下一次调用要自己把它重开。
///
/// `[前提]` 只能单独跑:它 SIGKILL 掉机器上所有 riot-browser 进程，
/// 而这个文件里别的用例各自也有一个。所以挂了 `#[ignore]` ——
/// `cargo test --test browser_e2e -- --ignored 崩掉之后` 单独验。
///
/// 这一条盯着的是「浏览器崩一次，整个会话的浏览器就永久废了」:那个进程
/// 句柄一旦填进槽位就没有别的地方会清它，交出死句柄的结果是面板和模型的
/// 每个 Browser* 工具都报"浏览器进程未运行"，而用户唯一的出路是重启应用。
#[tokio::test]
#[ignore = "会杀掉机器上所有 riot-browser，只能单独跑"]
async fn 崩掉之后下一次调用会自己重开() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("heal"));

    let before = host.open_tab().await.expect("开第一页");
    assert_eq!(before.tabs.len(), 1);

    // 模拟崩溃。走 SIGKILL 而不是发 Shutdown —— 后者是优雅退出，而这里
    // 要复现的正是"来不及说一声就没了"。
    let killed = std::process::Command::new("pkill")
        .args(["-9", "-f", "riot-browser"])
        .status()
        .expect("发 pkill");
    assert!(killed.success(), "没杀到任何 riot-browser 进程");

    // 等它察觉。SIGKILL 之后 stdout 的 EOF 要走一趟内核，不是同步的。
    //
    // 拿 `state` 当观测点:它刻意**不**重开进程（那是每秒一次的轮询，
    // 见它的文档），崩掉之后回的是空清单 —— 面板据此显示「正在启动」的
    // 占位，而不是一排点不动的幻影标签。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if host.state().await.expect("查状态").tabs.is_empty() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "进程都被杀了，这一层还以为标签页在"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // 真正的那一下。
    let after = host.open_tab().await.expect("崩掉之后要能自己重开");
    assert_eq!(
        after.tabs.len(),
        1,
        "清单里只该有新开的那一页 —— 旧号在新进程里不存在，留着的话面板上每一下点击都会静默失败"
    );
    assert_ne!(
        after.tabs[0].id, before.tabs[0].id,
        "号不能从头再发：新页和刚消失的那页同号，按号索引的表分不出两者"
    );
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
    let id = open_tab(&browser, &mut rx).await;
    // 这个用例只用低层命令，不需要 Tab 句柄。

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
                    id,
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
    let id = open_tab(&browser, &mut rx).await;
    let tab = Tab {
        browser: &browser,
        id,
    };

    // CDP 把错误放在响应体的 `error` 字段里，传输层是成功的。
    // 不翻出来的话，上层拿到一个没有 result 的对象，只能自己猜哪儿不对。
    let err = tab
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
    let id = open_tab(&browser, &mut rx).await;
    let tab = Tab {
        browser: &browser,
        id,
    };

    // console 钩子要在导航**之前**装。页面加载期间的报错最有价值，
    // 而那时候如果还没装钩子就永远抓不到了。
    ops::install_console_hook(tab).await.expect("装钩子");

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
    ops::navigate(tab, page).await.expect("导航");
    wait_for(
        &mut rx,
        30,
        "页面加载完成",
        |e| matches!(e, Event::LoadEnd { url, .. } if url.starts_with("data:")),
    )
    .await;

    // 快照:要能看见可交互元素，且不该被结构性节点淹没。
    let (snap, refs) = ops::snapshot(tab).await.expect("快照");
    assert!(snap.contains("提交"), "快照里应当有按钮名：{snap}");
    assert!(snap.contains("帮助链接"), "快照里应当有链接名：{snap}");
    assert!(
        !snap.contains("generic"),
        "结构性节点不该出现在快照里：{snap}"
    );
    // 交互靠编号指名元素:按钮那一行必须带 [n]，而且 n 在映射里。
    let button_ref = refs
        .iter()
        .find(|(_, r)| r.label.contains("提交"))
        .map(|(n, _)| *n)
        .expect("按钮该有编号");
    assert!(
        snap.contains(&format!("[{button_ref}] ")),
        "编号要出现在文本里：{snap}"
    );

    // 截图:PNG 的 base64。只验非空和能解码 —— 像素内容不该被断言，
    // 那会让用例随字体渲染的细微变化而红。
    let shot = ops::screenshot(tab).await.expect("截图");
    assert!(
        shot.len() > 1000,
        "截图太小，可能是白屏：{} 字节",
        shot.len()
    );

    // console:钩子装在导航前，所以页面脚本里的 warn 应当被抓到。
    let logs = ops::console(tab).await.expect("取 console");
    assert!(
        logs.iter().any(|l| l.contains("来自页面的警告")),
        "应当抓到页面加载期间的 console：{logs:?}"
    );

    // 禁回弹的样式跟钩子一起注入。滚动边缘的橡皮筋在面板里看是"画面在
    // 窗口里晃"，macOS 上没有开关能关（Chromium 对 Apple 写死启用），
    // 只有这条注入路。断言 computed style —— 注入失败时它是默认的 auto。
    let overscroll = ops::evaluate(
        tab,
        "getComputedStyle(document.documentElement).overscrollBehaviorY",
    )
    .await
    .expect("查 overscroll");
    assert_eq!(overscroll, "none", "html 上应当钉着 overscroll-behavior");

    // 换一个文档还得在:注入走的是 addScriptToEvaluateOnNewDocument，
    // 只对当前文档 evaluate 一次的话，第一次跳转就失效了。
    ops::navigate(tab, "data:text/html;charset=utf-8,<body>第二页</body>")
        .await
        .expect("再导航");
    let overscroll = ops::evaluate(
        tab,
        "getComputedStyle(document.documentElement).overscrollBehaviorY",
    )
    .await
    .expect("查 overscroll");
    assert_eq!(overscroll, "none", "跨导航之后禁回弹要还在");

    browser.shutdown().await;
}

/// Set-of-Marks 的 ops 层在真实页面上成立:几何按 backendId 对齐、叠框
/// 截图能出图、overlay 截完撤干净。
///
/// 盯着的是三个只在真浏览器里才暴露的点:a11y 的 `backendDOMNodeId` 能不能
/// 和 DOMSnapshot 的 `backendNodeId` 对上（对不上 rect 全空、整个特性废）、
/// 注入 overlay 后 `Page.captureScreenshot` 出的图非空、以及截完那层红框
/// 有没有留在页面上（留了的话之后每张普通截图都带框）。
#[tokio::test]
async fn 叠框截图在真实页面成立() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    let (tx, mut rx) = mpsc::unbounded_channel();
    let browser = Browser::spawn(app, Some(profile("marks")), tx)
        .await
        .expect("起浏览器");
    wait_for(&mut rx, 30, "ready", |e| matches!(e, Event::Ready)).await;
    let id = open_tab(&browser, &mut rx).await;
    let tab = Tab {
        browser: &browser,
        id,
    };

    let page = "data:text/html;charset=utf-8,\
        <html><body>\
        <button>提交</button>\
        <a href='%23x'>帮助链接</a>\
        </body></html>";
    ops::navigate(tab, page).await.expect("导航");
    wait_for(
        &mut rx,
        30,
        "页面加载完成",
        |e| matches!(e, Event::LoadEnd { url, .. } if url.starts_with("data:")),
    )
    .await;

    // 几何:按钮的 backendId 要能在 DOMSnapshot 里查到一个正矩形。这一步
    // 就是 a11y 编号和 DOMSnapshot backendNodeId 对齐的真实验证。
    let (_snap, refs) = ops::snapshot(tab).await.expect("快照");
    let button = refs
        .iter()
        .find(|(_, r)| r.label.contains("提交"))
        .map(|(n, _)| *n)
        .expect("按钮该有编号");
    let rect = refs[&button]
        .rect
        .expect("按钮该有几何（backendId 没对上就会是 None）");
    assert!(rect.w > 0.0 && rect.h > 0.0, "矩形该是正的：{rect:?}");

    // 叠框截图:注入 overlay → 截视口 → 撤 overlay，出的图非空。
    let shot = ops::screenshot_marked(tab, &[(button, rect)])
        .await
        .expect("叠框截图");
    assert!(
        shot.len() > 1000,
        "叠框截图太小，可能白屏：{} 字节",
        shot.len()
    );

    // overlay 必须撤干净 —— 留一层红框在页面上，之后每张普通截图都会带框。
    let left = ops::evaluate(tab, "!!document.getElementById('__riot_marks__')")
        .await
        .expect("查残留");
    assert_eq!(left, "false", "overlay 截完没撤干净：{left}");

    browser.shutdown().await;
}

/// 工具层走完整条链路:注册表 → BrowserAccess → 子进程 → CDP。
///
/// 中间每一环都在不同的 crate 里，靠 trait 对象连起来 —— 装配漏一环
/// 不会有编译错误，只会在运行时变成"工具说浏览器不可用"。
#[tokio::test]
async fn 工具层能真的驱动浏览器() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    let profile = profile("tools");
    let browser: std::sync::Arc<dyn riot_protocol::browser::BrowserAccess> =
        riot_host_lib::browser::access::HostBrowser::new(app, profile);

    // 惰性启动:这一刻进程还没起。第一次调用才起。
    let page = "data:text/html;charset=utf-8,\
        <html><body><button>点我</button>\
        <script>console.error('页面报错了');</script></body></html>";
    browser.navigate(page).await.expect("导航");

    let snap = browser.snapshot().await.expect("快照");
    assert!(snap.contains("点我"), "快照要能看见按钮：{snap}");

    let logs = browser.console().await.expect("console");
    assert!(
        logs.iter().any(|l| l.contains("页面报错了")),
        "要抓到加载期间的报错：{logs:?}"
    );

    let shot = browser.screenshot().await.expect("截图");
    assert!(shot.len() > 1000, "截图应当是有内容的 base64");

    let url = browser.current_url().await;
    assert!(
        url.starts_with("data:"),
        "当前地址应当是刚打开的那个：{url}"
    );
}

/// 交互走完整条链路:快照发号 → 编号换坐标 → 合成输入 → 页面真的反应。
///
/// 点击和输入的效果都打进 console 再读回来 —— 和滚轮、IME 的用例同一个
/// 思路:断言"页面收到了"，而不是"命令发出去了"。
#[tokio::test]
async fn 点击和输入能驱动真实页面() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    use riot_protocol::browser::{BrowserAccess as _, InteractError, Target, WaitCondition};
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("interact"));

    // `[约束]` onsubmit 里不能用 `+` 拼串。data: URL 里加号是保留字符，
    // 某些解析路径会把它变成空格 —— 用 concat 绕开，别赌。
    let page = "data:text/html;charset=utf-8,\
        <html><body>\
        <button id='go' onclick=\"console.log('btn-clicked')\">点我</button>\
        <form onsubmit=\"console.log('submitted:'.concat(document.querySelector('input').value));return false;\">\
        <input aria-label='搜索词'>\
        </form>\
        <div style='height:3000px'>占位</div>\
        </body></html>";
    host.navigate(page).await.expect("导航");

    // 还没拍快照就用编号点 —— 要一个说清"先快照"的错误，不是 CDP 报错。
    let early = host
        .click(Target::Ref(1))
        .await
        .expect_err("没快照不该能点");
    assert!(
        matches!(&early, InteractError::Target(m) if m.contains("BrowserSnapshot")),
        "要指引先拍快照：{early:?}"
    );

    let snap = host.snapshot().await.expect("快照");
    let button = ref_in_snapshot(&snap, "点我");
    let input = ref_in_snapshot(&snap, "搜索词");

    // 点击（编号定位）:页面的 onclick 真的跑了。
    let msg = host.click(Target::Ref(button)).await.expect("点击");
    assert!(msg.contains("点我"), "结果要说点了什么：{msg}");
    wait_console(&host, "log: btn-clicked").await;

    // 输入并回车（编号定位）:值原样到达，表单提交真的发生。
    host.type_text(Target::Ref(input), "riot 测试", true)
        .await
        .expect("输入");
    wait_console(&host, "log: submitted:riot 测试").await;

    // 选择器定位:不用快照编号也能点中同一个按钮。
    let by_sel = host
        .click(Target::Selector("#go".into()))
        .await
        .expect("按选择器点击");
    assert!(by_sel.contains("选择器"), "结果要指明是选择器：{by_sel}");
    wait_console(&host, "log: btn-clicked").await;

    // 文本定位:按可见文字点。
    host.click(Target::Text("点我".into()))
        .await
        .expect("按文本点击");

    // 选择器匹配不到 —— 要一个明确的"没匹配到"，不是 CDP 报错。
    let miss = host
        .click(Target::Selector("#does-not-exist".into()))
        .await
        .expect_err("匹配不到不该成功");
    assert!(
        matches!(&miss, InteractError::Target(m) if m.contains("没匹配到")),
        "要说清没匹配到：{miss:?}"
    );

    // 往按钮里打字要被拦下来，并且指引用点击。
    let wrong = host
        .type_text(Target::Ref(button), "x", false)
        .await
        .expect_err("按钮不该能输入");
    assert!(
        matches!(&wrong, InteractError::Target(m) if m.contains("BrowserClick")),
        "要指引改用点击：{wrong:?}"
    );

    // 编号超出快照范围 —— 指引重新快照。
    let stale = host
        .click(Target::Ref(9999))
        .await
        .expect_err("越界编号不该能点");
    assert!(
        matches!(&stale, InteractError::Target(m) if m.contains("BrowserSnapshot")),
        "要指引重新快照：{stale:?}"
    );

    // 等待:选择器已经在页面上，立刻满足；不存在的超时报错。
    let waited = host
        .wait_for(WaitCondition::Selector("#go".into()), 3000)
        .await
        .expect("等已存在的元素应当立刻成立");
    assert!(waited.contains("等到了"), "{waited}");
    let timed_out = host
        .wait_for(WaitCondition::Selector("#never".into()), 800)
        .await
        .expect_err("等不存在的元素该超时");
    assert!(matches!(&timed_out, InteractError::Target(m) if m.contains("仍未发生")));

    // 网络空闲:静态页没有在途请求，应当很快判定空闲。
    let idle = host
        .wait_for(WaitCondition::NetworkIdle, 5000)
        .await
        .expect("应当空闲");
    assert!(idle.contains("空闲"), "{idle}");

    // 滚动:位置真的动了，消息里带得有进度。
    let scrolled = host.scroll(600.0).await.expect("滚动");
    assert!(scrolled.contains("滚动"), "{scrolled}");

    // 不认识的键名当场拒绝，不发一个页面不会理的事件。
    let bad_key = host
        .press_key("Meta+Q")
        .await
        .expect_err("怪键名不该发出去");
    assert!(matches!(&bad_key, InteractError::Target(m) if m.contains("Enter")));
}

/// 扩展交互:对话框自动放行、下拉、组合键、标签管理。
///
/// 副作用一律经页面里的 `console.log` 回读（HostBrowser 没暴露 evaluate）——
/// 和交互主用例同一个思路:断言"页面真的收到了"，而不是"命令发出去了"。
#[tokio::test]
async fn 扩展交互在真实页面成立() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    use riot_protocol::browser::{Action, BrowserAccess as _, Nav, Target};
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("automation2"));

    // confirm() 会阻塞页面直到应答 —— 事件循环必须自动放行，否则这一次
    // 点击和之后的一切都超时。这是这个用例最重要的一条。
    let page = "data:text/html;charset=utf-8,\
        <html><body>\
        <button id='ask' onclick=\"console.log('confirmed:'.concat(confirm('去吗')))\">问</button>\
        <select id='sel' onchange=\"console.log('sel:'.concat(this.value))\">\
        <option value='a'>A</option><option value='b'>B</option></select>\
        <input id='box' value='old' \
          onkeydown=\"if(event.key==='a'&&event.metaKey)console.log('chord:meta-a')\">\
        </body></html>";
    host.navigate(page).await.expect("导航");

    // 点触发 confirm 的按钮:不该卡住，且页面拿到了 accept（true）。
    host.click(Target::Selector("#ask".into()))
        .await
        .expect("点击带 confirm 的按钮");
    wait_console(&host, "log: confirmed:true").await;

    // 下拉选择:设值并派发 change，onchange 回读确认。
    host.act(Action::SelectOption {
        target: Target::Selector("#sel".into()),
        value: "b".into(),
    })
    .await
    .expect("下拉选择");
    wait_console(&host, "log: sel:b").await;

    // 组合键:聚焦输入框后按 Meta+a。断言的是"带 metaKey 的 a 事件送达了
    // 页面"——这才是工具的契约。至于全选这个**编辑命令**要不要执行，是
    // CEF 平台加速键的事（离屏渲染下合成按键未必触发），不归工具管。
    host.click(Target::Selector("#box".into()))
        .await
        .expect("聚焦输入框");
    host.act(Action::KeyChord("Meta+a".into()))
        .await
        .expect("组合键");
    wait_console(&host, "log: chord:meta-a").await;

    // 标签管理:开新标签 → 列表里有两个 → 切回、关掉都不报错。
    host.browse(Nav::NewTab).await.expect("新开标签");
    let tabs = host.browse(Nav::ListTabs).await.expect("列标签");
    assert_eq!(tabs.matches('[').count(), 2, "该有两个标签页：{tabs}");

    // 执行 JS:算个值、读 DOM，结果整形成文本回来。
    let sum = host.evaluate("1 + 41").await.expect("算术");
    assert_eq!(sum, "42");
    let title_present = host
        .evaluate("typeof document.querySelector('#ask')")
        .await
        .expect("查 DOM");
    assert_eq!(title_present, "object", "#ask 是个元素");

    // 脚本抛异常:要拿到异常信息（Target 错误），不是"浏览器不可用"。
    let boom = host
        .evaluate("throw new Error('boom')")
        .await
        .expect_err("异常该冒出来");
    assert!(
        matches!(&boom, riot_protocol::browser::InteractError::Target(m) if m.contains("boom")),
        "异常信息要给模型看：{boom:?}"
    );
}

/// 抓包:开着累积、刷新后能列出请求，并能审计响应头。
#[tokio::test]
async fn 抓包在真实页面成立() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };
    use riot_protocol::browser::{BrowserAccess as _, NetQuery};
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("netcap"));

    // 页面自己再发一个子请求（fetch 一张 data: 图），好让列表里不止主文档。
    let page = "data:text/html;charset=utf-8,\
        <html><body><script>fetch('data:text/plain,hi')</script>子请求页</body></html>";
    host.navigate(page).await.expect("导航");

    // 第一次 list 开启累积。之后刷新，让加载流量进桶。
    let _ = host
        .network(NetQuery::List { filter: None })
        .await
        .expect("开抓包");
    host.reload().await.expect("刷新");
    // 给子请求一点时间落地。
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let list = host
        .network(NetQuery::List { filter: None })
        .await
        .expect("列请求");
    assert!(list.contains('#'), "该列出至少一条请求：{list}");

    // 审计不报错，给出结论（data: 页多半缺各种安全头，或说没抓到主文档）。
    let audit = host.network(NetQuery::Audit).await.expect("审计");
    assert!(!audit.is_empty(), "审计要有输出：{audit}");
}

/// 拦截:阻断一个 fetch，页面拿到失败、且**不卡死**（漏放 paused 请求
/// 会让页面永久挂起，这是这个用例最重要的一条）。重放也顺带验证。
#[tokio::test]
async fn 拦截与重放在真实页面成立() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };
    use riot_protocol::browser::{BrowserAccess as _, InterceptOp};
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("intercept"));

    host.navigate("data:text/html;charset=utf-8,<body>拦截页</body>")
        .await
        .expect("导航");

    // 加一条拦截规则:URL 含 blocked 的请求一律阻断。
    host.intercept(InterceptOp::Block {
        url_pattern: "blocked".into(),
    })
    .await
    .expect("加拦截规则");

    // 页面里 fetch 一个被拦的地址 + 一个不被拦的地址。被拦的应当 reject，
    // 不被拦的应当成功 —— 后者证明"不匹配的请求被正常放行"，没有卡死。
    let probe = host
        .evaluate(
            "(async () => { \
                let blocked = 'ok'; \
                try { await fetch('https://x.test/blocked/api'); } catch (e) { blocked = 'rejected'; } \
                let allowed = 'fail'; \
                try { await fetch('data:text/plain,pass'); allowed = 'ok'; } catch (e) {} \
                return blocked + ',' + allowed; \
            })()",
        )
        .await
        .expect("探测 fetch");
    assert_eq!(
        probe, "rejected,ok",
        "被拦的失败、放行的成功且不卡死：{probe}"
    );

    // 清空拦截:之后被拦的地址也能发出去（这里只验证 clear 不报错）。
    host.intercept(InterceptOp::Clear).await.expect("清空拦截");

    // 重放:同源 data: 不好演示带会话，改验证命令在真实浏览器上跑通、
    // 返回结构化结果（跨源会是 CORS 错误消息，也算跑通）。
    let replayed = host
        .replay("data:text/plain,hi", "GET", serde_json::Value::Null, None)
        .await
        .expect("重放");
    assert!(
        replayed.contains("状态") || replayed.contains("重放失败"),
        "{replayed}"
    );
}

/// Cookie 读取带安全属性:HttpOnly 的 cookie 也要能看到。
#[tokio::test]
async fn 读cookie带安全属性() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };
    use riot_protocol::browser::BrowserAccess as _;
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("cookies"));

    // data: URL 设不了 cookie（opaque origin）—— 用页面脚本往一个真实的
    // http 源写不现实，这里退而验证"没有 cookie 时给的是明确的空说明"，
    // 以及命令本身在真实浏览器上不报错。真实站点的属性解析由单测覆盖。
    host.navigate("data:text/html,<body>cookie 页</body>")
        .await
        .expect("导航");
    let out = host.cookies().await.expect("读 cookie");
    assert!(
        out.contains("没有 Cookie") || out.contains("="),
        "要么空、要么列出：{out}"
    );
}

/// 探针:密钥扫描、接口发现在真实页面上跑通并给出结果。
///
/// fuzz 要真实的 HTTP 端点才有意义（data: 页没有服务端），这里只覆盖两个
/// 被动探针 —— fuzz 的判定逻辑由 pentest 纯逻辑单测覆盖。
#[tokio::test]
async fn 被动探针在真实页面成立() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };
    use riot_protocol::browser::BrowserAccess as _;
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("probe"));

    // 页面里藏一个 AWS key、一个表单、一个链接。
    let page = "data:text/html;charset=utf-8,\
        <html><body>\
        <form action='/login' method='post'><input name='user'><input name='pw'></form>\
        <a href='https://x.test/admin'>后台</a>\
        <script>const k='AKIAIOSFODNN7EXAMPLE'</script>\
        </body></html>";
    host.navigate(page).await.expect("导航");

    // 密钥扫描:抓到 AWS key，且打码（原值不出现）。
    let secrets = host
        .evaluate("document.documentElement.outerHTML")
        .await
        .expect("取 HTML");
    let found = riot_tools::tools::pentest::scan_secrets(&secrets);
    assert!(
        found.iter().any(|f| f.starts_with("AWS Access Key")),
        "该扫到 key：{found:?}"
    );
    assert!(
        !found.iter().any(|f| f.contains("AKIAIOSFODNN7EXAMPLE")),
        "要打码"
    );

    // 接口发现:页面里的表单 action 能被 JS 枚举到。
    let forms = host
        .evaluate("JSON.stringify([...document.forms].map(f => f.getAttribute('action')))")
        .await
        .expect("枚举表单");
    assert!(forms.contains("/login"), "该发现登录表单：{forms}");
}

/// 文件上传:给 file input 设文件后，页面读到 files[0].name。
#[tokio::test]
async fn 文件上传在真实页面成立() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };
    use riot_protocol::browser::{BrowserAccess as _, Target};
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("upload"));

    // 用本进程的可执行文件当"要上传的文件"——一定存在的真实路径。
    let real_file = std::env::current_exe().expect("当前可执行文件路径");
    let path = real_file.to_string_lossy().to_string();

    host.navigate(
        "data:text/html;charset=utf-8,<body><input id='f' type='file' \
         onchange=\"console.log('uploaded:'.concat(this.files[0]?.name||''))\"></body>",
    )
    .await
    .expect("导航");

    host.upload(Target::Selector("#f".into()), vec![path.clone()])
        .await
        .expect("设置上传文件");

    let want = format!(
        "log: uploaded:{}",
        std::path::Path::new(&path)
            .file_name()
            .unwrap()
            .to_string_lossy()
    );
    wait_console(&host, &want).await;
}

/// 爬虫:两页互链，站点地图应当把两页都访问到。
#[tokio::test]
async fn 爬虫生成站点地图() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };
    use riot_protocol::browser::BrowserAccess as _;
    use riot_tools::tools::pentest::{crawl_next, link_host};

    // 爬虫工具本身要 scope + 真实 host（data: 没有 host），这里直接验证
    // 驱动爬虫的纯逻辑在真实链接上成立:同 host 的链接会被挑出来续爬。
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("crawl"));
    host.navigate(
        "data:text/html,<body><a href='https://x.test/a'>a</a>\
         <a href='https://x.test/b'>b</a><a href='https://y.test/c'>c</a></body>",
    )
    .await
    .expect("导航");

    // 从页面取链接（绝对 URL），交给爬虫的挑选逻辑。
    let raw = host
        .evaluate("JSON.stringify([...document.querySelectorAll('a')].map(a => a.href))")
        .await
        .expect("取链接");
    let links: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
    assert_eq!(link_host("https://x.test/a").as_deref(), Some("x.test"));
    let next = crawl_next(&links, "x.test", &std::collections::HashSet::new());
    assert_eq!(next.len(), 2, "只跟同 host 的两条：{next:?}");
    assert!(next.iter().all(|u| u.contains("x.test")), "{next:?}");
}

/// 从快照文本里找某个元素的编号。
fn ref_in_snapshot(snap: &str, needle: &str) -> u32 {
    snap.lines()
        .filter(|l| l.contains(needle))
        .find_map(|l| {
            let end = l.find(']')?;
            l.strip_prefix('[')?.get(..end - 1)?.parse().ok()
        })
        .unwrap_or_else(|| panic!("快照里找不到 {needle} 的编号：\n{snap}"))
}

/// 探路:CDP 的 screencast 在离屏渲染下能不能出帧。
///
/// 如果能，面板的画面通道就不需要共享内存 —— Chromium 直接给 JPEG，
/// 比原始 BGRA 小二十倍，而且编码是它自己做的。这决定了下一步的架构，
/// 所以先花一个用例问清楚。
#[tokio::test]
async fn screencast_在离屏模式下能出帧() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    let (tx, mut rx) = mpsc::unbounded_channel();
    let browser = Browser::spawn(app, Some(profile("cast")), tx)
        .await
        .expect("起浏览器");
    wait_for(&mut rx, 30, "ready", |e| matches!(e, Event::Ready)).await;
    let id = open_tab(&browser, &mut rx).await;
    let tab = Tab {
        browser: &browser,
        id,
    };

    ops::navigate(
        tab,
        "data:text/html;charset=utf-8,<body style='background:%23c00'><h1>CAST</h1></body>",
    )
    .await
    .expect("导航");

    tab.cdp(
        "Page.startScreencast",
        serde_json::json!({
            "format": "jpeg",
            "quality": 60,
            "maxWidth": 1280,
            "maxHeight": 800,
        }),
    )
    .await
    .expect("开 screencast");

    // screencastFrame 是不带 id 的 CDP 事件，走事件流。
    let ev = wait_for(&mut rx, 30, "screencast 帧", |e| {
        matches!(e, Event::Cdp { payload, .. }
            if payload.get("method").and_then(|m| m.as_str()) == Some("Page.screencastFrame"))
    })
    .await;

    let Event::Cdp { payload, .. } = ev else {
        unreachable!()
    };
    let data = payload["params"]["data"].as_str().expect("帧数据");
    assert!(data.len() > 500, "帧太小，可能是空白：{} 字节", data.len());

    // JPEG 的 base64 一定以 /9j/ 开头（FF D8 FF）。验一下确实是图，
    // 而不是某个恰好非空的字符串。
    assert!(
        data.starts_with("/9j/"),
        "应当是 JPEG：{}",
        &data[..20.min(data.len())]
    );

    browser.shutdown().await;
}

/// 画面通道要能**持续**出帧，不是只出一帧。
///
/// `[约束]` Chromium 只在上一帧被 ack 之后才发下一帧。漏 ack 的表现是
/// 画面永久定格在第一帧，而且不报任何错 —— 看起来像页面卡住了。所以
/// 这条必须断言"至少两帧"，只验一帧的用例挡不住这个 bug。
#[tokio::test]
async fn 画面能持续推送而不是只出一帧() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    // navigate 是 trait 方法，要 trait 在作用域里。
    use riot_protocol::browser::BrowserAccess as _;
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("cast2"));

    // 页面自己动起来，保证有新帧可推 —— 静止页面 Chromium 不会重复发。
    let page = "data:text/html;charset=utf-8,\
        <body style='background:%23111'>\
        <h1 id=t style='color:%230f0'>0</h1>\
        <script>let n=0;setInterval(()=>{t.textContent=++n},100)</script>\
        </body>";
    host.navigate(page).await.expect("导航");

    let (tx, mut rx) = mpsc::unbounded_channel();
    host.start_screencast(tx).await.expect("开 screencast");

    let mut got = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while got.len() < 3 {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!left.is_zero(), "20 秒内只收到 {} 帧", got.len());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(f)) => got.push(f),
            Ok(None) => panic!("帧通道断了，只收到 {} 帧", got.len()),
            Err(_) => panic!("等帧超时，只收到 {} 帧", got.len()),
        }
    }

    assert!(got[0].width > 0 && got[0].height > 0, "帧要带尺寸");
    assert!(
        got.iter().all(|f| f.data.starts_with("/9j/")),
        "每帧都该是 JPEG"
    );

    host.stop_screencast().await;
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
    let id = open_tab(&browser, &mut rx).await;
    // 这个用例只用低层命令，不需要 Tab 句柄。

    browser
        .send(&Command::Resize {
            tab: id,
            width: 640,
            height: 480,
            scale: 1.0,
        })
        .expect("发 resize");

    // `[约束]` 尺寸必须真的传到 CEF。只改本地变量而没调 was_resized 的话，
    // 帧会一直是旧尺寸，面板上表现为"拖动没反应"。
    wait_for(&mut rx, 30, "640x480 的帧", |e| {
        matches!(
            e,
            Event::Frame {
                width: 640,
                height: 480,
                ..
            }
        )
    })
    .await;

    browser.shutdown().await;
}

/// screencast 开着的时候把面板拖大，画面要跟上。
///
/// `[约束]` 这条盯的是"只能缩不能放"。用户拖窗口时 screencast 已经在推，
/// resize 是打在一个进行中的会话上的 —— 和"先 resize 再开 screencast"
/// （下面那条用例）走的不是同一条时序。放大跟不上的现象:面板变大后
/// 画面还是旧尺寸，等比铺放后右侧/下侧留黑边，而缩小看起来一切正常。
#[tokio::test]
async fn screencast_进行中放大面板画面要跟上() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    use riot_protocol::browser::BrowserAccess as _;
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("cast-grow"));

    // `[约束]` 页面必须是静止的。带动画的页面每 100ms 就有新 damage，
    // capturer 错过一帧还有下一帧兜着 —— 而用户看的多数页面（文档、
    // 聊天记录）加载完就不动了，resize 之后要是没抓到重排后的那一帧，
    // 画面就永远停在中间态上。动画页面测不出这个。
    let page = "data:text/html;charset=utf-8,\
        <body style='background:%23111;color:%230f0'><h1>STATIC</h1></body>";
    host.navigate(page).await.expect("导航");

    // 从一块小面板开始推。scale=2 对齐 Retina —— 放大的回归只在高密度下
    // 出现过，1x 一直是好的。
    host.resize(600, 500, 2.0).await.expect("初始视口");
    let (tx, mut rx) = mpsc::unbounded_channel();
    host.start_screencast(tx).await.expect("开 screencast");

    // 等画面稳定在初始尺寸上 —— 确认会话真的按 600x500 在推。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!left.is_zero(), "20 秒内没等到 600x500 的帧");
        let Ok(Some(f)) = tokio::time::timeout(left, rx.recv()).await else {
            panic!("等初始帧失败");
        };
        if (f.width, f.height) == (600, 500) {
            break;
        }
    }

    // 先缩小 —— 用户拖窗口通常先把它压窄，这一步看起来总是正常的。
    host.resize(400, 500, 2.0).await.expect("缩小视口");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!left.is_zero(), "20 秒内没等到缩小后 400x500 的帧");
        let Ok(Some(f)) = tokio::time::timeout(left, rx.recv()).await else {
            panic!("等缩小帧失败");
        };
        if (f.width, f.height) == (400, 500) {
            break;
        }
    }

    // 再拖大。此刻 screencast 还开着 —— 正是用户拖窗口的路径。
    host.resize(1100, 900, 2.0).await.expect("放大视口");

    // 排版视口要真的变大。metadata 跟上了、页面还按旧宽度排版的话，
    // 帧的右侧全是黑 —— 用户看到的正是"面板拖大了、页面缩在左边"。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let got = host
            .evaluate("`${innerWidth}x${innerHeight}`")
            .await
            .unwrap_or_default();
        if got == "1100x900" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "放大后 20 秒排版视口还是 {got}，没到 1100x900"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !left.is_zero(),
            "放大后 20 秒内没等到 1100x900 的帧，收到过：{seen:?}"
        );
        let Ok(Some(f)) = tokio::time::timeout(left, rx.recv()).await else {
            panic!("等放大后的帧失败，收到过：{seen:?}");
        };
        if (f.width, f.height) == (1100, 900) {
            // 元数据跟上还不够:JPEG 的真实像素也要是新表面的大小。
            // 表面没放大的话，这里解出来还是旧的 1200x1000。
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&f.data)
                .expect("解 base64");
            let real = jpeg_size(&bytes);
            assert_eq!(
                real,
                (2200, 1800),
                "元数据说 1100x900@2x，JPEG 却是 {real:?} —— 表面没跟着放大"
            );
            break;
        }
        let size = (f.width, f.height);
        if !seen.contains(&size) {
            seen.push(size);
        }
    }

    host.stop_screencast().await;
}

/// 推给面板的画面要按面板的尺寸出。
///
/// `[约束]` 这条盯的是黑边。面板把画面等比缩放后铺进自己那块地方，帧的
/// 比例和面板对不上时，短的那一边两侧就空出来 —— 视口钉死在 1280×800、
/// 面板又是竖着的一条时，上下各留两百多像素，用户看到的是"页面只占中间
/// 一截"。
///
/// 上一条用例验的是 OSR 的 `Frame` 元数据，这条验的是 screencast 的帧 ——
/// 面板显示的是后者。两者由 Chromium 里不同的东西驱动，OSR 跟上了不等于
/// screencast 也跟上了。
#[tokio::test]
async fn 画面尺寸跟着面板走() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    use riot_protocol::browser::BrowserAccess as _;
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("cast-size"));

    // 页面要一直在动，否则 Chromium 认为没有新内容，不会再发帧。
    let page = "data:text/html;charset=utf-8,\
        <body style='background:%23111'>\
        <h1 id=t style='color:%230f0'>0</h1>\
        <script>let n=0;setInterval(()=>{t.textContent=++n},100)</script>\
        </body>";
    host.navigate(page).await.expect("导航");

    // 一块竖着的面板。这个比例（0.8）和默认的 1280×800（1.6）差得足够远，
    // 视口没跟上的话帧的比例一眼就能看出不对。
    host.resize(720, 900, 1.0).await.expect("改视口");

    let (tx, mut rx) = mpsc::unbounded_channel();
    host.start_screencast(tx).await.expect("开 screencast");

    // 头几帧可能还是旧尺寸 —— 改视口到渲染跟上之间隔着一次重排。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !left.is_zero(),
            "20 秒内没等到 720x900 的帧，收到过：{seen:?}"
        );
        let Ok(Some(f)) = tokio::time::timeout(left, rx.recv()).await else {
            panic!("等帧失败，收到过：{seen:?}");
        };
        if (f.width, f.height) == (720, 900) {
            break;
        }
        let size = (f.width, f.height);
        if !seen.contains(&size) {
            seen.push(size);
        }
    }

    host.stop_screencast().await;
}

/// 帧要按屏幕的像素密度出，不是按 CSS 像素。
///
/// `[约束]` 这条盯的是"糊"。面板在 Retina 上占的是两倍物理像素，帧按一倍
/// 出的话，浏览器要放大一倍才铺得满，文字边缘全是虚的。而尺寸、比例、点击
/// 位置全都是对的 —— 这种糊看起来只像是 JPEG 质量调低了，几乎不会有人往
/// "渲染分辨率"上想。
///
/// 两个数一起断言:元数据要留在 CSS 像素（面板的点击换算依赖它），JPEG 的
/// 真实像素要是它的两倍（清晰度靠它）。只验一个的话，另一个错了照样绿。
#[tokio::test]
async fn 帧按屏幕的像素密度出() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    use riot_protocol::browser::BrowserAccess as _;
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("dpi"));

    // 页面要一直在动，否则 Chromium 认为没有新内容，不会再发帧。
    let page = "data:text/html;charset=utf-8,\
        <body style='background:%23fff'>\
        <h1 id=t>0</h1>\
        <script>let n=0;setInterval(()=>{t.textContent=++n},100)</script>\
        </body>";
    host.navigate(page).await.expect("导航");

    const W: u32 = 700;
    const H: u32 = 900;
    const SCALE: u32 = 2;
    host.resize(W as i32, H as i32, SCALE as f32)
        .await
        .expect("改视口");

    let (tx, mut rx) = mpsc::unbounded_channel();
    host.start_screencast(tx).await.expect("开 screencast");

    // 抓拍器是按开的那一刻的画面配的，头几帧可能还是旧尺寸。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut seen: Vec<((u32, u32), (u16, u16))> = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !left.is_zero(),
            "20 秒内没等到 {W}x{H} 的 CSS 尺寸配 {}x{} 的真实像素，收到过：{seen:?}",
            W * SCALE,
            H * SCALE,
        );
        let Ok(Some(f)) = tokio::time::timeout(left, rx.recv()).await else {
            panic!("等帧失败，收到过：{seen:?}");
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&f.data)
            .expect("帧是 base64");
        let real = jpeg_size(&bytes);
        if (f.width, f.height) == (W, H) && real == ((W * SCALE) as u16, (H * SCALE) as u16) {
            break;
        }
        let pair = ((f.width, f.height), real);
        if !seen.contains(&pair) {
            seen.push(pair);
        }
    }

    host.stop_screencast().await;
}

/// 从 JPEG 头里读真实像素尺寸。
///
/// screencast 的元数据只报 CSS 尺寸，倍率对不对得把图本身解出来看 ——
/// 倍率的定义就是这两者的比。
fn jpeg_size(b: &[u8]) -> (u16, u16) {
    let mut i = 2; // 跳过 SOI
    while i + 9 < b.len() {
        if b[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = b[i + 1];
        let len = usize::from(u16::from_be_bytes([b[i + 2], b[i + 3]]));
        // SOFn 是帧头，尺寸在段内第 3..7 字节。0xC4/0xC8/0xCC 长得像但是
        // 霍夫曼表之类，不带尺寸。
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let h = u16::from_be_bytes([b[i + 5], b[i + 6]]);
            let w = u16::from_be_bytes([b[i + 7], b[i + 8]]);
            return (w, h);
        }
        i += 2 + len;
    }
    panic!("JPEG 里没找到 SOF 段");
}

/// 面板上的滚轮要真的能把页面滚起来，**两个方向都要**。
///
/// 走的是前端原样发过来的那条 JSON。这一段坏过两次，现象都是"滚轮没反应"：
///
/// - 竖轴：字段名 `deltaY` 对不上 Rust 侧的 `delta_y`，命令在 Tauri 解析
///   参数时就失败了，前端又把 reject 吞掉 —— 看起来像 Chromium 不支持滚动。
/// - 横轴：这一层压根没收 `deltaX`，宿主往 CDP 里写死一个 0 —— 看起来像
///   页面自己没有横向滚动条。而面板通常只有半个窗口宽，页面比它宽是常态。
///
/// 单元测试盯字段名，这条盯的是"页面真的动了":CDP 的 `mouseWheel` 在离屏
/// 渲染下有没有效，只有真的滚一次才知道。
#[tokio::test]
async fn 面板的滚轮能横竖两个方向滚动页面() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    use riot_protocol::browser::BrowserAccess as _;
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("wheel"));

    // 页面把滚动位置打进 console，再从 console 工具读回来 —— 那是这一层
    // 唯一现成的"页面内部状态"窗口。
    //
    // 别改成写 location.hash：data: 页面是不透明来源，Chromium 直接拦掉顶层
    // 的 data URL 导航，改 hash 也不例外。用它做判据的话，滚动明明生效了，
    // 用例照样红。
    let page = "data:text/html;charset=utf-8,\
        <body style='width:5000px;height:5000px;margin:0'>\
        <script>onscroll=()=>{\
            console.log('at='+Math.round(scrollX)+','+Math.round(scrollY))}</script>\
        </body>";
    host.navigate(page).await.expect("导航");

    // 照抄面板发出去的那条 JSON，字段名一个不改。
    let input: riot_host_lib::browser::access::Input = serde_json::from_value(serde_json::json!({
        "kind": "scroll", "x": 100.0, "y": 100.0, "deltaX": 400.0, "deltaY": 600.0,
    }))
    .expect("解析面板输入");
    host.send_input(input).await.expect("发滚动");

    // 滚动要经过合成器再回到主线程，不是同步的。轮询等它落地。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let logs = host.console().await.expect("读 console");
        let moved = logs
            .iter()
            .filter_map(|l| l.split_once("at="))
            .filter_map(|(_, at)| at.trim().split_once(','))
            .filter_map(|(x, y)| Some((x.parse::<i32>().ok()?, y.parse::<i32>().ok()?)))
            .fold((0, 0), |(mx, my), (x, y)| (mx.max(x), my.max(y)));
        // 两个轴分开断言。合起来判"动了没"的话，横轴那个 bug 会被竖轴的
        // 成功盖过去 —— 那正是它上次溜过去的方式。
        if moved.0 > 0 && moved.1 > 0 {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "10 秒内页面没有朝两个方向都滚动（最远到 {moved:?}），console 里只有：{logs:?}"
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// 中文要能输进页面里。
///
/// `[约束]` 这条盯的是"只能输入英文"。中文走输入法：先组字（拼音在候选窗口
/// 里）、再确认（汉字上屏）。面板把键盘挂在一个普通 div 上的时候，输入法压根
/// 挂不上来 —— 拼音的原始字母被当成普通字符送进页面，于是英文正常、中文永远
/// 是一串字母。
///
/// 组字和确认要分别验:
///
/// - 组字要能显示。只发最终结果也"能用"，但带自动补全的搜索框在你打完之前
///   一个字都不显示，那不叫能用。
/// - 确认要**替掉**临时内容，不是追加。insertText 底下是 ImeCommitText，
///   所以这件事是免费的 —— 但一旦有人把它改成 dispatchKeyEvent，页面里就会
///   变成"ni你"，而这种错只有中文用户看得见。
#[tokio::test]
async fn 输入法的组字和确认都落进页面() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    use riot_host_lib::browser::access::Input;
    use riot_protocol::browser::BrowserAccess as _;
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("ime"));

    // 输入框把自己的值打进 console —— 这一层唯一现成的"页面内部状态"窗口。
    //
    // 加载时也打一条:它同时是同步点和判据。等到它说明文档在跑、console
    // 钩子也装上了 —— 少了这一条，"文字没送到"和"钩子没装上"在结果里
    // 长得一模一样，都是一个空数组。
    let page = "data:text/html;charset=utf-8,\
        <body style='margin:0'>\
        <input id=i style='position:absolute;left:0;top:0;width:300px;height:40px' \
         oninput='console.log(\"值=\"+i.value)'>\
        <script>console.log('页面就绪')</script>\
        </body>";
    host.navigate(page).await.expect("导航");
    wait_console(&host, "log: 页面就绪").await;

    // 点进输入框，而不是靠 autofocus。用户就是这么做的，而 autofocus 在
    // 离屏渲染下不保证能把焦点真的落到元素上 —— 那时候后面发的文字会
    // 落进虚空，现象和"输入法没接上"一模一样。
    host.resize(600, 400, 1.0).await.expect("定视口");
    host.send_input(Input::Click {
        x: 50.0,
        y: 20.0,
        button: "left".into(),
    })
    .await
    .expect("点输入框");

    // 组字：拼音还没确认，页面里就该有临时内容了。
    host.send_input(Input::Compose { text: "ni".into() })
        .await
        .expect("发组字");
    wait_console(&host, "log: 值=ni").await;

    // 确认。
    host.send_input(Input::Text { text: "你".into() })
        .await
        .expect("发确认");
    // 精确匹配:追加而不是替换的话这里是"值=ni你"，包含式判断会放过它。
    wait_console(&host, "log: 值=你").await;
}

/// 等 console 里出现某一条完整的记录。
async fn wait_console(host: &riot_host_lib::browser::access::HostBrowser, want: &str) {
    use riot_protocol::browser::BrowserAccess as _;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let logs = host.console().await.expect("读 console");
        if logs.iter().any(|l| l == want) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "10 秒内 console 里没出现「{want}」，只有：{logs:?}"
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// 整页截图必须把折叠下方的内容真的渲染出来，而不是拿视口平铺。
///
/// `[约束]` 离屏渲染下不能用 `captureBeyondViewport`：视口外的区域不会
/// 真正排版，Chromium 拿当前视口的帧重复填充 —— 用户拿到的截图是同一屏
/// 内容摞了十几遍（真实发生过，一张 cursor.com 的整页截图里首屏重复了
/// 十次）。这条用四段纯色带钉住:每种颜色只出现在自己的高度区间，平铺时
/// 中下段采到的全是首屏的颜色，立刻红。
#[tokio::test]
async fn 整页截图不平铺视口() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    use riot_protocol::browser::BrowserAccess as _;
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("fullpage"));

    // 四段 1200px 的纯色，总高 4800 —— 远超视口，逼出"视口外"的渲染路径。
    let page = "data:text/html;charset=utf-8,\
        <body style='margin:0'>\
        <div style='height:1200px;background:%23e53935'></div>\
        <div style='height:1200px;background:%2343a047'></div>\
        <div style='height:1200px;background:%231e88e5'></div>\
        <div style='height:1200px;background:%23fdd835'></div>\
        </body>";
    host.navigate(page).await.expect("导航");

    let shot = host.screenshot().await.expect("截图");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(shot)
        .expect("合法 base64");
    let img = image::load_from_memory(&bytes)
        .expect("解得开 JPEG")
        .to_rgb8();
    let (w, h) = img.dimensions();

    // 高度 = 页面内容高度（CSS 像素、1× 出图）。矮一截说明整页路径没走到。
    assert!(
        (4700..=4900).contains(&h),
        "整页截图高度该约 4800，实际 {w}×{h}"
    );

    // 每段中点采一个像素。JPEG 有损，±32 足够分辨这四种颜色。
    let near = |got: image::Rgb<u8>, want: [u8; 3]| {
        got.0.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 32)
    };
    let bands: [(u32, [u8; 3], &str); 4] = [
        (600, [0xe5, 0x39, 0x35], "红"),
        (1800, [0x43, 0xa0, 0x47], "绿"),
        (3000, [0x1e, 0x88, 0xe5], "蓝"),
        (4200, [0xfd, 0xd8, 0x35], "黄"),
    ];
    for (y, want, name) in bands {
        let got = *img.get_pixel(w / 2, y);
        assert!(
            near(got, want),
            "y={y} 该是{name}色带 {want:?}，实际 {:?} —— 视口外的内容没有真的渲染",
            got.0
        );
    }
}

/// 给模型的截图不能跟着面板的屏幕密度长大。
///
/// `[约束]` 这条盯的是"模型截不了图"。面板在 Retina 上按 2× 渲染（为了给人
/// 看清），而截图工具有体积上限 —— 出图跟着密度走的话，同一个页面在外接屏上
/// 能截、在内置屏上就撞上限失败。而模型收到"截图太大"之后不会重试，它会
/// 换一条路:去 shell 里 screencapture 整个屏幕，然后拿着一张截错的图分析。
/// 那正是这个 bug 被发现的方式。
///
/// 判据是两个密度下的体积**接近**，不是精确相等 —— JPEG 编码在不同缩放
/// 路径下会差几个百分点。
#[tokio::test]
async fn 截图体积不随屏幕密度变化() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    use riot_protocol::browser::BrowserAccess as _;
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("shot"));

    // 一个"几屏高、有配色"的页面，接近真实站点 —— 纯白页压得太狠，
    // 密度带来的差别会被压缩率吃掉，测不出问题。
    let page = "data:text/html;charset=utf-8,\
        <body style='font:14px system-ui;margin:0'><div id=box></div>\
        <script>box.innerHTML=[...Array(80)].map((_,i)=>\
          '<p style=\"margin:6px;padding:8px;background:hsl('+(i*7%360)+',60%,85%)\">\
           第 '+i+' 行 some english text 混排 '+i+'</p>').join('');</script></body>";

    let mut sizes = Vec::new();
    for scale in [1.0_f32, 2.0] {
        host.resize(900, 1000, scale).await.expect("视口");
        host.navigate(page).await.expect("导航");
        let shot = host.screenshot().await.expect("截图");
        // 工具那边的上限是 2_000_000 字节。留出余量:真实页面比这个测试页
        // 更花，而撞上限的表现是工具直接失败。
        assert!(
            shot.len() < 1_000_000,
            "scale={scale} 时截图 {} KB，离工具上限太近了",
            shot.len() / 1024
        );
        sizes.push(shot.len());
    }

    let (one, two) = (sizes[0] as f64, sizes[1] as f64);
    assert!(
        (two / one) < 1.5,
        "2× 的截图是 1× 的 {:.1} 倍（{} KB vs {} KB）—— 出图跟着屏幕密度走了",
        two / one,
        sizes[1] / 1024,
        sizes[0] / 1024,
    );
}

/// 多个标签页同时活着，各有各的页面和历史。
///
/// `[约束]` 这条盯的是"标签页真的是独立页面"。同一个进程里开多个 CEF
/// browser 是这个功能的地基 —— 要是它们其实共用一个页面，现象会是"切标签
/// 之后两个标签显示同一个网站"，而所有单标签的用例照常绿。
///
/// 顺带钉住三条界面语义:新开的页要变成活动页、关掉活动页要顺位接替、
/// 关掉最后一页回空清单（面板据此把自己收起来，和关掉最后一个标签页等于
/// 关窗口一个道理）。
#[tokio::test]
async fn 多个标签页各自独立() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    use riot_protocol::browser::BrowserAccess as _;
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("tabs"));

    let page = |tag: &str| format!("data:text/html;charset=utf-8,<h1>{tag}</h1>");

    // 第一页。工具层的 navigate 作用在活动页上，而这时候活动页是自动开的
    // 那一个 —— 面板还没开，标签页也是这一刻才现开的。
    host.navigate(&page("AAA")).await.expect("导航到 A");
    let s = host.state().await.expect("取状态");
    assert_eq!(s.tabs.len(), 1, "第一次用到时该自动开一页");
    let first = s.tabs[0].id;
    assert!(s.tabs[0].url.contains("AAA"));

    // 第二页。
    let s = host.open_tab().await.expect("开第二页");
    assert_eq!(s.tabs.len(), 2, "应当有两页了");
    let second = s.active;
    assert_ne!(second, first, "新页要有自己的号");
    assert_eq!(s.active_tab().url, "", "新页停在空白页");
    assert!(!s.active_tab().can_back, "新页身后什么都没有");

    host.navigate(&page("BBB")).await.expect("导航到 B");
    let s = host.state().await.expect("取状态");
    let a = s.tabs.iter().find(|t| t.id == first).expect("第一页还在");
    let b = s.tabs.iter().find(|t| t.id == second).expect("第二页在");
    // 这两条断言是整条用例的核心:两个标签页停在**各自**的地址上。
    assert!(
        a.url.contains("AAA"),
        "第一页不该被第二页的导航带走：{}",
        a.url
    );
    assert!(b.url.contains("BBB"), "第二页应当在 B：{}", b.url);

    // 切回第一页，工具栏和模型的工具都要跟着换过去。
    let s = host.select_tab(first).await.expect("切回第一页");
    assert_eq!(s.active, first);
    assert!(s.active_tab().url.contains("AAA"));
    let snap = host.snapshot().await.expect("快照");
    assert!(
        snap.contains("AAA") && !snap.contains("BBB"),
        "模型看到的应当是当前活动页：{snap}"
    );

    // 关掉活动页 —— 剩下的那页接替。
    let s = host.close_tab(first).await.expect("关第一页");
    assert_eq!(s.tabs.len(), 1);
    assert_eq!(s.active, second, "关掉活动页要顺位接替");
    assert!(s.active_tab().url.contains("BBB"));

    // 关掉最后一页 —— 空清单，不补新页。补一页的话，只剩一页时那个关闭键
    // 就变成了"清空当前页"，按下去看起来什么都没发生。
    let s = host.close_tab(second).await.expect("关最后一页");
    assert!(s.tabs.is_empty(), "最后一页关掉后不该再有页：{:?}", s.tabs);
    assert_eq!(s.active_tab(), TabInfo::default(), "没有活动页");

    // 但浏览器进程还活着:模型的工具下次用到时应当现开一页，而不是报
    // "浏览器不可用"。用户关掉面板不等于关掉模型的浏览器。
    host.navigate(&page("CCC"))
        .await
        .expect("关完之后模型还能用");
    let s = host.state().await.expect("取状态");
    assert_eq!(s.tabs.len(), 1, "该现开一页");
    assert!(s.active_tab().url.contains("CCC"));
}

/// 页面开新窗口的请求要被拦成一条事件，而且不能碰到母页面。
///
/// `[约束]` 离屏渲染下**不能**让 CEF 按默认行为创建弹窗，理由有两层，
/// 而第二层是真正咬人的那个（见 `OsrLifeSpan::on_before_popup`）：
///
/// 1. 离屏渲染是**每个 browser** 的设置。CEF 给弹窗的 `WindowInfo` 是默认值，
///    于是它走有窗模式 —— 一个飘在 Riot 外面的原生浏览器窗口。
/// 2. 弹窗默认共用母页面的 client，而 client 里钉着标签页号。弹窗的
///    `on_after_created` / `on_before_close` 报的都是**母页面的号**：前者把
///    表里母页面的句柄换成弹窗的，后者（用户关掉那个窗口时）把母页面整条
///    抹掉。母页面的 CEF browser 还活着、画面还在，但从此没人能对它发命令。
///
/// 所以这条盯三件事：请求变成了 `PopupRequested`；号没有被顶掉（不该再来
/// 一条 `TabOpened`）；母页面照旧能收命令。第 2 条坏掉时前两条都还是绿的，
/// 而现象是"面板卡死"，唯一线索是一串指向明明开着的号的"标签页不存在"。
#[tokio::test]
async fn 弹窗被拦成事件而母页面还能用() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    let (tx, mut rx) = mpsc::unbounded_channel();
    let browser = Browser::spawn(app, Some(profile("popup")), tx)
        .await
        .expect("起浏览器");
    wait_for(&mut rx, 30, "ready", |e| matches!(e, Event::Ready)).await;
    let id = open_tab(&browser, &mut rx).await;
    let tab = Tab {
        browser: &browser,
        id,
    };

    // `userGesture` 不能省：没有手势的 `window.open` 会被 Chromium 自己的
    // 弹窗拦截挡在 CEF 之前，那样这条用例测的就是拦截器而不是我们的处理。
    tab.cdp(
        "Runtime.evaluate",
        serde_json::json!({
            "expression": "window.open('https://example.com/opened')",
            "userGesture": true,
        }),
    )
    .await
    .expect("发 window.open");

    let ev = wait_for(&mut rx, 20, "弹窗请求", |e| {
        matches!(e, Event::PopupRequested { .. })
    })
    .await;
    let Event::PopupRequested {
        source,
        url,
        background,
    } = ev
    else {
        unreachable!("上面的谓词只放 PopupRequested 过")
    };
    assert_eq!(source, id, "要报出是哪一页发起的 —— 新页要排在它右边");
    assert!(url.contains("example.com/opened"), "地址要带过来：{url}");
    assert!(!background, "普通的 window.open 是前台打开");

    // 号没被顶掉。母页面自己既没有重开也没有关掉 —— 这一段在修好之前
    // 会收到弹窗那个 browser 报出来的 `TabOpened { tab: 1 }`。
    let quiet = tokio::time::Instant::now() + Duration::from_millis(1500);
    loop {
        let left = quiet.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            break;
        }
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(Event::TabOpened { tab })) => panic!("标签页 {tab} 不该再开一次"),
            Ok(Some(Event::TabClosed { tab })) => panic!("标签页 {tab} 不该被关掉"),
            Ok(Some(_)) => continue,
            Ok(None) => panic!("浏览器进程没了"),
            Err(_) => break,
        }
    }

    // 母页面照旧能收命令。句柄被顶掉的话这里等不到 LoadEnd，
    // 换来的是一条 "标签页 1 不存在"。
    //
    // 标记用 ASCII：Chromium 报回来的地址是百分号转义过的，拿中文去比
    // 永远比不上（同一个坑写在 `BLANK_PAGE` 上）。
    let page = "data:text/html;charset=utf-8,<h1>ALIVE</h1>";
    browser
        .send(&Command::Navigate {
            tab: id,
            url: page.into(),
        })
        .expect("发导航");
    wait_for(
        &mut rx,
        20,
        "母页面加载完",
        |e| matches!(e, Event::LoadEnd { tab, url, .. } if *tab == id && url.contains("ALIVE")),
    )
    .await;
}

/// 点外链要开在新标签页里，而且原来那页留在原地。
///
/// 这是 [`弹窗被拦成事件而母页面还能用`] 那条事件的另一半：子进程只报告，
/// 号由这一层发、页由这一层开。少了这一半的现象是"点外链什么都没发生"。
#[tokio::test]
async fn 点外链开在新标签页里() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    use riot_protocol::browser::{BrowserAccess as _, Target};
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("popup-tab"));

    // 走真的鼠标点击（`Input.dispatchMouseEvent`），因为用户就是这么点的 ——
    // 而且真实点击自带手势，不会撞上 Chromium 的弹窗拦截。
    //
    // `[约束]` 页面里不能出现 `#`。这一整段是 data: URL 的内容，`#` 在那里
    // 是片段分隔符 —— 页面会从那儿被截断，现象是"点不到那个链接"。
    let page = "data:text/html;charset=utf-8,\
        <a target=_blank href='data:text/plain,BBB'>去别处</a>\
        <h1>AAA</h1>";
    host.navigate(page).await.expect("导航到 A");
    let first = host.state().await.expect("取状态").active;

    host.click(Target::Text("去别处".into()))
        .await
        .expect("点链接");

    // 开页是异步的（子进程报事件 → 这一层开页 → 等它就绪），点击返回时
    // 通常还没开完。
    let s = wait_tabs(&host, 2).await;
    assert_eq!(s.tabs.len(), 2, "外链该开成第二页：{:?}", s.tabs);
    assert_eq!(s.tabs[0].id, first, "新页要排在发起那页的右边");
    assert_eq!(s.active, s.tabs[1].id, "前台打开 —— 新页就是当前页");
    assert!(
        s.tabs[0].url.contains("AAA"),
        "原来那页要留在原地：{}",
        s.tabs[0].url
    );

    // 两页各自独立:切回去还是 A，说明新页没有顶掉旧页的 browser。
    let s = host.select_tab(first).await.expect("切回第一页");
    assert!(s.active_tab().url.contains("AAA"));
}

/// 页面自己关掉一页之后，面板不能卡住。
///
/// `[约束]` 这一层必须处理 [`Event::TabClosed`]，不能只认自己发出去的
/// `CloseTab`。页面自己 `window.close()`、渲染进程崩掉、脚本开的那页被关掉，
/// 都会让一个号在子进程那边消失而这一层不知情 —— 而那个号很可能正是
/// "当前页"。之后每条命令都以"标签页不存在"被丢掉，每次 CDP 调用要等满
/// 30 秒超时：面板彻底卡住，且没有任何一条报错说得出"那一页已经没了"。
#[tokio::test]
async fn 页面自己关掉一页之后还能继续用() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    use riot_protocol::browser::BrowserAccess as _;
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("selfclose"));

    host.navigate("data:text/html;charset=utf-8,<h1>AAA</h1>")
        .await
        .expect("导航到 A");
    let first = host.state().await.expect("取状态").active;

    // 第二页停在空白页上 —— 历史里只有一条，Chromium 才允许脚本关它
    // （"Scripts may close only the windows that were opened by them"）。
    let s = host.open_tab().await.expect("开第二页");
    let second = s.active;
    assert_ne!(second, first);

    // `[约束]` 要等空白页真的提交了再让它关。`open_tab` 等的是 CEF 建好
    // browser，那一刻页面还可能停在初始空文档上 —— 那时候发出的关闭请求会
    // 被 Chromium 拒掉，而拒绝是静默的（连 console 都不留）。现象是"这一页
    // 就是不关"，只在机器忙的时候偶发。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let href = host.evaluate("location.href").await.expect("读地址");
        if href == riot_protocol::browser::BLANK_PAGE {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "第二页 10 秒内没停在空白页上，实际：{href}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // 用 `setInterval` 反复试，而不是调一次就算：上面那条竞态还有别的形态
    // （提交正好落在两次 CDP 往返之间），让页面自己重试比在这一层重试干净。
    // 一次都不该成功的话，下面的等待照样会失败 —— 重试只兜时机，不兜逻辑。
    host.evaluate("setInterval(() => window.close(), 100); 'ok'")
        .await
        .expect("让页面关掉自己");

    let s = wait_tabs(&host, 1).await;
    assert_eq!(s.tabs.len(), 1, "关掉的那页要从清单里消失：{:?}", s.tabs);
    assert_eq!(s.tabs[0].id, first, "剩下的该是第一页");
    assert_eq!(s.active, first, "当前页要顺位落到还活着的那页上");

    // 真正的验收：还能用。挂着一个已经不存在的当前页时，这一条会等满
    // CDP 超时然后失败。
    host.navigate("data:text/html;charset=utf-8,<h1>CCC</h1>")
        .await
        .expect("关完之后还能导航");
    let s = host.state().await.expect("取状态");
    assert_eq!(s.tabs.len(), 1, "不该凭空多出一页：{:?}", s.tabs);
    assert!(s.active_tab().url.contains("CCC"));
}

/// 等标签页数量变成 `want`。
///
/// 开页和关页都是"子进程报事件 → 这一层处理"，命令返回时还没落地。
async fn wait_tabs(
    host: &riot_host_lib::browser::access::HostBrowser,
    want: usize,
) -> riot_host_lib::browser::access::PanelState {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let s = host.state().await.expect("取状态");
        if s.tabs.len() == want {
            return s;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "15 秒内标签页没变成 {want} 个，实际：{:?}",
            s.tabs
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// 只该有一页的时候不能冒出两页。
///
/// `[约束]` "没有页就开一页"必须是原子的。并发的两个调用都看到"一页都没有"
/// 的话，会各开一个 —— 现象是打开面板出现两个空标签页。
///
/// 这不是理论上的竞态:React 的 StrictMode 在 dev 下把 effect 跑两遍，
/// `browser_open` 连着发两次，每次都问一遍活动页。这里就照那个形状并发。
#[tokio::test]
async fn 并发问活动页只会开出一页() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    use riot_protocol::browser::BrowserAccess as _;
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("once"));

    // current_url 不开页，用它当"轻量的活动页查询"没意义 —— 这里要的是
    // 真的会开页的那条路。navigate 是最短的一条。
    let page = "data:text/html;charset=utf-8,<h1>AAA</h1>";
    let (a, b) = tokio::join!(host.navigate(page), host.navigate(page));
    a.expect("第一次导航");
    b.expect("第二次导航");

    let s = host.state().await.expect("取状态");
    assert_eq!(s.tabs.len(), 1, "并发导航只该开出一页，实际：{:?}", s.tabs);
}

/// 画面跟着活动标签页走。
///
/// `[约束]` 切标签是"停旧页的 screencast、开新页的"两条命令，中间旧页还会
/// 再来几帧。这条盯的是切完之后**新页的帧真的在来** —— 漏了开那一步，或者
/// 过滤器还盯着旧页号，现象都是"切过去之后画面定在原地"，而标签栏、地址栏
/// 全都正常，看起来像页面卡住了。
#[tokio::test]
async fn 画面跟着活动标签页走() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    use riot_protocol::browser::BrowserAccess as _;
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("tab-cast"));

    // 两页都要一直在动:静止页面 Chromium 不会重复发帧，那样"没收到帧"
    // 就分不清是切换坏了还是页面本来就没变化。
    let moving = |tag: &str| {
        format!(
            "data:text/html;charset=utf-8,\
             <body style='background:%23111'><h1 id=t style='color:%230f0'>{tag}</h1>\
             <script>let n=0;setInterval(()=>{{t.textContent='{tag} '+(++n)}},100)</script></body>"
        )
    };

    host.navigate(&moving("AAA")).await.expect("导航到 A");
    let first = host.state().await.expect("取状态").active;

    let (tx, mut rx) = mpsc::unbounded_channel();
    host.start_screencast(tx).await.expect("开 screencast");
    wait_frames(&mut rx, 2, "第一页").await;

    // 开第二页 —— 它会成为活动页，画面该跟过去。
    let s = host.open_tab().await.expect("开第二页");
    let second = s.active;
    host.navigate(&moving("BBB")).await.expect("导航到 B");
    wait_frames(&mut rx, 2, "第二页").await;

    // 切回去也一样。
    host.select_tab(first).await.expect("切回第一页");
    wait_frames(&mut rx, 2, "切回第一页").await;

    host.select_tab(second).await.expect("再切到第二页");
    wait_frames(&mut rx, 2, "再切到第二页").await;

    host.stop_screencast().await;
}

/// 等收到 `n` 帧，超时就失败。
async fn wait_frames(
    rx: &mut mpsc::UnboundedReceiver<riot_host_lib::browser::access::Frame>,
    n: usize,
    what: &str,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    for got in 0..n {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!left.is_zero(), "等 {what} 的帧超时，只收到 {got} 帧");
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(f)) => assert!(f.data.starts_with("/9j/"), "{what} 的帧该是 JPEG"),
            Ok(None) => panic!("{what}：帧通道断了，只收到 {got} 帧"),
            Err(_) => panic!("等 {what} 的帧超时，只收到 {got} 帧"),
        }
    }
}

/// 空白页不能出现在地址栏里。
///
/// `[约束]` 这条盯的是那个常量和 Chromium 报回来的地址**逐字节相等**。
/// 判等靠的是字符串，而 Chromium 会把 data URL 里的 `<` `>` 之类转义掉 ——
/// 常量里带上那些字符的话，这里比不上，用户打开面板第一眼就会在地址栏里
/// 看到一串 `data:text/html,%3Chtml%3E...`。单元测试比的是我们自己写的
/// 两个字符串，测不到这件事。
///
/// 不拿**启动时**那个空白页当测试对象：起始导航还没提交时收到下一条
/// navigate，Chromium 会直接取消它，历史里根本不会有那一条 —— 是否发生
/// 取决于机器负载（全量跑测试时几乎必现）。这里自己把空白页导航进历史，
/// 每一步都等到可观测的提交信号，没有赌的成分。
#[tokio::test]
async fn 空白页在地址栏里是空的() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    use riot_protocol::browser::BrowserAccess as _;
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("blank"));

    let page = |tag: &str| format!("data:text/html;charset=utf-8,<h1>{tag}</h1>");

    // 锚点页。等它出现在地址栏 = 它已提交，此后地址栏变空只能是下一步
    // 空白页的提交，不会跟"历史还没建立时的空状态"混淆。
    host.navigate(&page("AAA")).await.expect("导航到 A");
    wait_url(&host, "AAA").await;

    // 把空白页本身导航进历史，然后等它提交。current_url 是给模型工具用的
    // **原始**地址（不抹空白页），所以这里等到的值和常量逐字节相等这件事，
    // 本身就是这条用例要盯的那次比较。
    host.navigate(riot_protocol::browser::BLANK_PAGE)
        .await
        .expect("导航到空白页");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let url = host.current_url().await;
        if url == riot_protocol::browser::BLANK_PAGE {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "10 秒内空白页都没提交，地址栏还是：{url}"
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
    }

    // 再压一页，让空白页成为"上一条"。
    host.navigate(&page("BBB")).await.expect("导航到 B");
    wait_url(&host, "BBB").await;

    // 退回空白页。go 返回的 url 来自 Chromium 的历史条目 —— 这里断言的
    // 就是那次逐字节比较的结果。
    let back = host.go(-1).await.expect("退回空白页");
    assert_eq!(back.url, "", "空白页该显示成空");
    assert!(back.can_back, "身后还有锚点页");
    assert!(back.can_forward, "前面还有刚压的那一页");
}

/// 工具栏的前进后退要真的在历史里走。
///
/// `[约束]` CDP 没有 goBack/goForward，只有「跳到某个历史条目」，而条目的
/// `id` 是 Chromium 发的号。这条盯的就是那个换算:拿下标当 id 用的时候，
/// 页面会跳到一个用户没去过的地方，或者干脆不动 —— 两种都不会报错。
///
/// 前进后退还是那种"命令发出去了、页面没动"也完全静默的操作，所以判据必须
/// 是页面**真实的** location，不能是我们自己算出来的那个状态。
#[tokio::test]
async fn 工具栏能在历史里前进后退() {
    let Some(app) = bundle() else {
        eprintln!("跳过：还没打包");
        return;
    };

    use riot_protocol::browser::BrowserAccess as _;
    let host = riot_host_lib::browser::access::HostBrowser::new(app, profile("history"));

    let page = |tag: &str| format!("data:text/html;charset=utf-8,<body><h1>{tag}</h1></body>");
    host.navigate(&page("AAA")).await.expect("导航到 A");
    host.navigate(&page("BBB")).await.expect("导航到 B");

    let here = host.state().await.expect("取状态").active_tab();
    assert!(here.url.contains("BBB"), "当前应当在 B：{}", here.url);
    assert!(here.can_back, "身后有 A，后退键该是亮的");
    assert!(!here.can_forward, "最新的一条前面没有东西");

    let back = host.go(-1).await.expect("后退");
    assert!(
        back.url.contains("AAA"),
        "回来的状态应当指向 A：{}",
        back.url
    );
    assert!(back.can_forward, "退回来之后前进键该亮");
    wait_url(&host, "AAA").await;

    let forward = host.go(1).await.expect("前进");
    assert!(forward.url.contains("BBB"), "应当回到 B：{}", forward.url);
    wait_url(&host, "BBB").await;

    // 到头了再按不该出事。按钮那时候是灰的，这里兜的是状态还没同步过来的
    // 那一瞬间 —— 越界没拦住的话会跳到一个别的条目上去。
    let past_end = host.go(1).await.expect("前进到头");
    assert!(
        past_end.url.contains("BBB"),
        "越界应当原地不动：{}",
        past_end.url
    );

    // 刷新只验它不报错。CDP 的方法名写错会在这里翻出来（见
    // `cdp_的错误会翻出来而不是当成成功`），而"页面确实重新加载了一遍"
    // 在 data: 页面上没有便宜的判据 —— 每次加载都是一份全新的文档，
    // 连 console 缓冲区都是新的，看不出前后。
    host.reload().await.expect("刷新");
}

/// 等页面真的落到带 `tag` 的地址上。
///
/// `navigateToHistoryEntry` 发出去就返回，页面换文档还要一会儿。
async fn wait_url(host: &riot_host_lib::browser::access::HostBrowser, tag: &str) {
    use riot_protocol::browser::BrowserAccess as _;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let url = host.current_url().await;
        if url.contains(tag) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "10 秒内页面没有走到 {tag}，还停在：{url}"
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}
