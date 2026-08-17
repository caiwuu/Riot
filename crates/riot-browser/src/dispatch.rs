//! 把 stdin 上的命令送到 CEF 的 UI 线程执行。
//!
//! # 为什么要绕一圈
//!
//! `[约束]` CEF 的浏览器方法只能在它自己的 UI 线程上调。而读 stdin 是阻塞
//! 操作，不能放在 UI 线程上 —— 那会把消息循环卡死，页面直接不动。
//!
//! 所以是:后台线程读行 → 解析 → `post_task` 投到 UI 线程 → 在那儿动浏览器。
//!
//! # 浏览器句柄为什么用 thread_local
//!
//! CEF 的对象是引用计数的裸指针包装，不保证 `Send`。放进 `static Mutex`
//! 需要 unsafe 地宣称它跨线程安全，而那个宣称是假的。
//!
//! 换成 thread_local 之后约束变成编译期可见的:句柄只存在于 UI 线程，
//! 想碰它就必须先 post_task 过去。这正是 CEF 本来的要求。
//!
//! # 一个标签页一个 browser
//!
//! `[取舍]` 标签页做成同一个进程里的多个 CEF browser，而不是多个进程。
//!
//! 一个 profile 目录同时只能有一个 Chromium 实例（锁文件独占），所以"一个
//! 标签页一个进程"要么共享 profile 然后互相踢掉，要么每页一份 cookie 和
//! 登录状态 —— 后者根本不像标签页。而同进程多 browser 是 CEF 原本就支持的
//! 形态（弹窗走的就是这条路），renderer 仍然按站点分进程，隔离度不变。

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use cef::*;

use riot_protocol::browser::{Command, Event, TabId};

/// 是不是正在主动关闭。
///
/// 关浏览器时 renderer 必然消失，CDP 必然断开 —— 那时候报
/// "DevTools agent 已断开"是纯噪音，而且会让人以为退出过程出了问题。
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

pub fn is_shutting_down() -> bool {
    SHUTTING_DOWN.load(Ordering::Relaxed)
}

/// 一个开着的标签页。
struct Tab {
    browser: Browser,
    /// CEF 自己给这个 browser 的号。
    ///
    /// `[约束]` 要记下来，才能分辨"生命周期回调说的是不是表里这一个"。
    /// 标签页号是我们发的，一个 client 可以被多个 browser 共用（弹窗就是
    /// 这么来的，见 [`crate::osr::OsrLifeSpan::on_before_popup`]）—— 只按
    /// 标签页号认的话，一个我们没请求过的 browser 关掉时会把还活着的那一页
    /// 从表里抹掉，之后所有命令都以"标签页不存在"失败。
    cef_id: ::std::os::raw::c_int,
    /// CDP 观察者的注册凭据。
    ///
    /// `[约束]` 必须留着。drop 掉就等于注销观察者 —— 表现是发出去的 CDP
    /// 命令永远收不到响应，而且没有任何报错。
    _cdp: Option<Registration>,
}

thread_local! {
    /// 开着的标签页。只在 UI 线程上有值。
    static TABS: RefCell<HashMap<TabId, Tab>> = RefCell::new(HashMap::new());
}

/// 取出某个标签页浏览器句柄的一份克隆。
///
/// `[约束]` **绝不能**跨 CEF 调用持有 `RefCell` 的借用。
///
/// CEF 的很多方法是同步回调的:`close_browser` 会当场调 `on_before_close`，
/// 而那里要改这张表、拿可变借用。借用还没放就重入 →
/// `RefCell already borrowed` → panic → 整个浏览器进程消失。
///
/// 这个 panic 在外面看起来完全不像自己的错:stdout 上先冒出
/// `Inspector.detached / Render process gone`，像是 Chromium 的渲染器崩了，
/// 排查方向会整个跑偏。所以句柄一律先克隆出来再用 —— 引用计数 +1 很便宜。
fn browser_of(tab: TabId) -> Option<Browser> {
    TABS.with_borrow(|m| m.get(&tab).map(|t| t.browser.clone()))
}

/// 开一个标签页。
pub fn open_tab(tab: TabId) {
    debug_assert_ne!(currently_on(ThreadId::UI), 0, "必须在 UI 线程");
    if browser_of(tab).is_some() {
        crate::wire::emit(&Event::Error {
            message: format!("标签页 {tab} 已经开着了"),
        });
        return;
    }

    let mut client = crate::osr::client_for(tab);
    let window_info = WindowInfo {
        windowless_rendering_enabled: 1,
        // `[约束]` 离屏渲染只有 Alloy 这套 runtime style 支持。用默认的
        // Chrome style 不会报错，它会正常创建浏览器、正常加载页面，只是
        // on_paint 一次都不调 —— 表现为"什么都对，就是收不到帧"。
        runtime_style: RuntimeStyle::ALLOY,
        ..Default::default()
    };
    let url = CefString::from(riot_protocol::browser::BLANK_PAGE);

    // 这一步只是发起创建，`on_after_created` 稍后才来 —— 那时候才登记进表、
    // 才发 TabOpened。主应用等的是那一条。
    browser_host_create_browser(
        Some(&window_info),
        Some(&mut client),
        Some(&url),
        Some(&BrowserSettings::default()),
        None,
        None,
    );
}

/// 浏览器创建完成。登记句柄、挂 CDP 观察者，然后告诉主应用它能用了。
pub fn tab_created(tab: TabId, browser: Option<Browser>) {
    debug_assert_ne!(currently_on(ThreadId::UI), 0, "必须在 UI 线程");
    let Some(browser) = browser else { return };
    let cef_id = browser.identifier();

    // `[约束]` 已经有一个了就不能覆盖。覆盖掉的那个 browser 还活着、画面
    // 还在，只是从此没人能对它发命令 —— 面板看起来是卡住，而线索指向一个
    // 明明开着的标签页号。会走到这里说明有个我们没请求过的 browser 用了
    // 这个 client，见 [`crate::osr::OsrLifeSpan::on_before_popup`]。
    if let Some(old) = TABS.with_borrow(|m| m.get(&tab).map(|t| t.cef_id)) {
        crate::wire::emit(&Event::Error {
            message: format!(
                "标签页 {tab} 已经绑着 browser {old} 了，多出来的 browser {cef_id} 不登记"
            ),
        });
        return;
    }

    // 观察者带着标签页号 —— 回来的 CDP 报文要标明是哪个页面的，
    // 不然主应用没法把响应派给正确的等待者。
    let mut observer = crate::cdp::CdpObserver::new(tab);
    let reg = browser
        .host()
        .and_then(|host| host.add_dev_tools_message_observer(Some(&mut observer)));

    TABS.with_borrow_mut(|m| m.insert(tab, Tab { browser, cef_id, _cdp: reg }));
    crate::wire::emit(&Event::TabOpened { tab });
}

/// 浏览器要销毁了。
///
/// `cef_id` 是 CEF 给这个 browser 的号，`None` 表示回调没带 browser 进来。
/// 对不上表里那一项就什么都不做:那是个没登记的 browser（见
/// [`tab_created`] 的拒绝分支），照常报 `TabClosed` 会让主应用把还活着的
/// 那一页从清单里摘掉。
pub fn tab_closed(tab: TabId, cef_id: Option<::std::os::raw::c_int>) {
    // `[约束]` 句柄要在借用**结束之后**才 drop。drop 一个 CEF 对象可能
    // 同步回调进来（那里又要借这张表），在借用里 drop 就是重入 panic ——
    // 而 panic 在这个进程里等于整个浏览器消失。
    let taken = TABS.with_borrow_mut(|m| {
        let ours = m
            .get(&tab)
            .is_some_and(|t| cef_id.is_none_or(|id| t.cef_id == id));
        if ours { m.remove(&tab) } else { None }
    });
    let was_ours = taken.is_some();
    drop(taken);
    if !was_ours {
        return;
    }

    crate::osr::forget_view(tab);
    crate::wire::emit(&Event::TabClosed { tab });

    // 最后一个标签页关掉、而且是在关机流程里，才结束消息循环。
    // 用户手动关掉最后一个标签页不该让进程退出 —— 面板还开着，
    // 他随手就会再开一个，而重启一遍 CEF 要一两秒。
    if is_shutting_down() && TABS.with_borrow(HashMap::is_empty) {
        quit_message_loop();
    }
}

/// 起一个后台线程读 stdin，把命令投到 UI 线程。
///
/// stdin 关闭（主应用退出或崩溃）时结束消息循环 —— 没人看的页面不该
/// 继续烧 CPU 和内存。
pub fn spawn_stdin_reader() {
    std::thread::spawn(|| {
        use std::io::BufRead as _;
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<Command>(line) {
                Ok(cmd) => post_to_ui(cmd),
                Err(e) => crate::wire::emit(&Event::Error {
                    message: format!("命令解析失败: {e}"),
                }),
            }
        }
        // 管道断了。让 UI 线程退出消息循环，而不是在这里 exit ——
        // 直接 exit 会跳过 CEF 的 shutdown，留下一堆 helper 进程。
        post_to_ui(Command::Shutdown);
    });
}

fn post_to_ui(cmd: Command) {
    let mut task = CommandTask::new(RefCell::new(Some(cmd)));
    post_task(ThreadId::UI, Some(&mut task));
}

wrap_task! {
    struct CommandTask {
        // Option 是因为 execute 只能拿到 &self，而命令要被移出去用。
        // CEF 保证每个 Task 只执行一次，所以取空之后不会再有人来取。
        cmd: RefCell<Option<Command>>,
    }

    impl Task {
        fn execute(&self) {
            let Some(cmd) = self.cmd.borrow_mut().take() else { return };
            run_on_ui(cmd);
        }
    }
}

fn run_on_ui(cmd: Command) {
    // 开标签页和关机不针对已有的页面，先处理掉。
    match cmd {
        Command::OpenTab { tab } => return open_tab(tab),
        Command::Shutdown => return shutdown(),
        _ => {}
    }

    let tab = match cmd {
        Command::CloseTab { tab }
        | Command::Navigate { tab, .. }
        | Command::Resize { tab, .. }
        | Command::Cdp { tab, .. } => tab,
        Command::OpenTab { .. } | Command::Shutdown => unreachable!("上面处理过了"),
    };

    // 先把句柄克隆出来，借用当场结束。见 browser_of() 的说明。
    let Some(browser) = browser_of(tab) else {
        // 不是致命错误:关标签页和它自己关掉可能同时发生。但要报出来 ——
        // 一条命令悄悄消失比报错难查得多。
        crate::wire::emit(&Event::Error {
            message: format!("标签页 {tab} 不存在，命令被忽略"),
        });
        return;
    };

    match cmd {
        Command::CloseTab { .. } => {
            if let Some(host) = browser.host() {
                // force = true:不给页面弹 onbeforeunload。用户点的是关闭，
                // 不该被页面的确认框挡住。
                host.close_browser(1);
            }
        }

        Command::Navigate { url, .. } => {
            if let Some(frame) = browser.main_frame() {
                frame.load_url(Some(&CefString::from(url.as_str())));
            }
        }

        Command::Resize { width, height, scale, .. } => {
            crate::osr::set_view(tab, width, height, scale);
            if let Some(host) = browser.host() {
                // `[约束]` 两个都要通知，而且密度在前。
                //
                // was_resized 只让 CEF 重新问 view_rect，密度是 screen_info
                // 那条路上的东西，不通知的话它一直用建浏览器时那一次的值 ——
                // 面板从外接屏拖回内置屏，尺寸变了、清晰度没变。
                host.notify_screen_info_changed();
                host.was_resized();
                // `[约束]` 紧跟一次强制重绘，不能省。was_resized 让 CEF 进入
                // "resize hold"：冻结合成，等一帧尺寸恰好等于新视口的绘制来
                // 释放。CEF 126 起这个释放条件会因内部缓存的旧边界而永远
                // 凑不齐（cef#3856），此后每条 resize 都被 hold 吞掉 ——
                // 现象是面板拖大后页面永远停在旧宽度，右侧一条黑，而拖小
                // 看起来正常（旧帧被面板等比缩小，黑边不明显）。invalidate
                // 强制渲染进程立刻按当前视口出一帧，给 hold 一个能命中的
                // 释放时机；顺带也把 screencast 的抓拍器踢醒 —— 静止页面
                // resize 后若抓拍器错过了那一帧（cef#3826），没有新的合成
                // 提交它就再也不出帧。
                host.invalidate(PaintElementType::VIEW);
            }
        }

        Command::Cdp { payload, .. } => {
            let Some(host) = browser.host() else { return };
            let Ok(bytes) = serde_json::to_vec(&payload) else {
                crate::wire::emit(&Event::Error {
                    message: "CDP 载荷无法序列化".into(),
                });
                return;
            };
            // 返回 0 表示消息没被接受（通常是 JSON 不符合 CDP 的形状）。
            // 静默丢掉会让上层一直等一个永远不来的响应。
            if host.send_dev_tools_message(Some(&bytes)) == 0 {
                crate::wire::emit(&Event::Error {
                    message: "CDP 消息被拒绝，检查 id/method 字段".into(),
                });
            }
        }

        Command::OpenTab { .. } | Command::Shutdown => unreachable!("上面处理过了"),
    }
}

/// 关掉所有标签页，等它们都走完再结束消息循环。
///
/// `[约束]` 不能关完就 `quit_message_loop`。CEF 的关闭是异步的 ——
/// 立刻退出消息循环，`on_before_close` 就永远不会跑，那些 renderer 和 GPU
/// helper 进程会留在系统里。真正的收尾在 [`tab_closed`] 里:最后一个走完
/// 才退。
///
/// 一个标签页都没有的时候直接退 —— 否则没人来触发那个条件。
fn shutdown() {
    SHUTTING_DOWN.store(true, Ordering::Relaxed);

    let tabs: Vec<TabId> = TABS.with_borrow(|m| m.keys().copied().collect());
    if tabs.is_empty() {
        quit_message_loop();
        return;
    }
    for tab in tabs {
        if let Some(host) = browser_of(tab).and_then(|b| b.host()) {
            host.close_browser(1);
        }
    }
}
