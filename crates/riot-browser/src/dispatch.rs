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

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

use cef::*;

use crate::protocol::{Command, Event};

/// 是不是正在主动关闭。
///
/// 关浏览器时 renderer 必然消失，CDP 必然断开 —— 那时候报
/// "DevTools agent 已断开"是纯噪音，而且会让人以为退出过程出了问题。
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

pub fn is_shutting_down() -> bool {
    SHUTTING_DOWN.load(Ordering::Relaxed)
}

thread_local! {
    /// 当前浏览器。只在 UI 线程上有值。
    static BROWSER: RefCell<Option<Browser>> = const { RefCell::new(None) };
    /// CDP 观察者的注册凭据。
    ///
    /// `[约束]` 必须留着。drop 掉就等于注销观察者 —— 表现是发出去的 CDP
    /// 命令永远收不到响应，而且没有任何报错。
    static CDP_REG: RefCell<Option<Registration>> = const { RefCell::new(None) };
}

/// 取出浏览器句柄的一份克隆。
///
/// `[约束]` **绝不能**跨 CEF 调用持有 `RefCell` 的借用。
///
/// CEF 的很多方法是同步回调的:`close_browser` 会当场调 `on_before_close`，
/// 而那里要 `set_browser(None)` 拿可变借用。借用还没放就重入 →
/// `RefCell already borrowed` → panic → 整个浏览器进程消失。
///
/// 这个 panic 在外面看起来完全不像自己的错:stdout 上先冒出
/// `Inspector.detached / Render process gone`，像是 Chromium 的渲染器崩了，
/// 排查方向会整个跑偏。所以句柄一律先克隆出来再用 —— 引用计数 +1 很便宜。
fn browser() -> Option<Browser> {
    BROWSER.with_borrow(|b| b.clone())
}

/// 在 UI 线程上记下浏览器句柄，并挂上 CDP 观察者。由 `on_after_created` 调。
pub fn set_browser(b: Option<Browser>) {
    debug_assert_ne!(currently_on(ThreadId::UI), 0, "必须在 UI 线程");

    let reg = b.as_ref().and_then(|browser| {
        let mut observer = crate::cdp::CdpObserver::new();
        browser
            .host()
            .and_then(|host| host.add_dev_tools_message_observer(Some(&mut observer)))
    });

    BROWSER.with_borrow_mut(|slot| *slot = b);
    CDP_REG.with_borrow_mut(|slot| *slot = reg);
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
                Err(e) => Event::Error {
                    message: format!("命令解析失败: {e}"),
                }
                .emit(),
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
    // 先把句柄克隆出来，借用当场结束。见 browser() 的说明。
    let Some(browser) = browser() else {
        if !matches!(cmd, Command::Shutdown) {
            Event::Error {
                message: "还没有浏览器，命令被忽略".into(),
            }
            .emit();
            return;
        }
        quit_message_loop();
        return;
    };

    match cmd {
        Command::Navigate { url } => {
            if let Some(frame) = browser.main_frame() {
                frame.load_url(Some(&CefString::from(url.as_str())));
            }
        }

        Command::Resize { width, height } => {
            crate::osr::set_view_size(width, height);
            // 通知 CEF 视口变了，它会重新问 view_rect 并重画。
            if let Some(host) = browser.host() {
                host.was_resized();
            }
        }

        Command::Cdp { payload } => {
            let Some(host) = browser.host() else { return };
            let Ok(bytes) = serde_json::to_vec(&payload) else {
                Event::Error {
                    message: "CDP 载荷无法序列化".into(),
                }
                .emit();
                return;
            };
            // 返回 0 表示消息没被接受（通常是 JSON 不符合 CDP 的形状）。
            // 静默丢掉会让上层一直等一个永远不来的响应。
            if host.send_dev_tools_message(Some(&bytes)) == 0 {
                Event::Error {
                    message: "CDP 消息被拒绝，检查 id/method 字段".into(),
                }
                .emit();
            }
        }

        Command::Shutdown => {
            SHUTTING_DOWN.store(true, Ordering::Relaxed);
            if let Some(host) = browser.host() {
                // force = true:不给页面弹 onbeforeunload。
                // agent 驱动的浏览器不该被一个确认框挡住退出。
                host.close_browser(1);
            }
            quit_message_loop();
        }
    }
}
