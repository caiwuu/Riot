//! CEF 浏览器宿主。主应用 spawn 它，通过 stdio 驱动。
//!
//! # 为什么是独立进程
//!
//! CEF 在 macOS 上必须活在 `.app` 里（见 [`paths`]），而主应用在
//! `tauri dev` 下是裸二进制。把 CEF 塞进主进程意味着放弃 `tauri dev`。
//!
//! 拆出来之后顺带拿到三件事:CEF 和 tao 不再抢主线程的 run loop;
//! 浏览器崩了带不倒聊天;开发和生产下浏览器都是 bundle 形态，不会出现
//! "dev 好好的、打包就 panic"那类只在发版时才暴露的问题。

mod cdp;
mod dispatch;
#[cfg(target_os = "macos")]
mod mac;
mod osr;
mod paths;
mod wire;

use cef::*;

fn main() {
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("riot-browser: 目前只实现了 macOS");
        std::process::exit(1);
    }

    #[cfg(target_os = "macos")]
    {
        let Some(frameworks) = paths::frameworks_dir() else {
            eprintln!(
                "riot-browser: 找不到 Frameworks 目录。\n\
                 这个二进制必须从打包好的 .app 里启动 —— CEF 在 macOS 上\n\
                 通过 main bundle 定位资源，裸二进制会卡在 icudtl.dat。\n\
                 用 scripts/build-browser.sh 打包后再跑。"
            );
            std::process::exit(1);
        };
        if !paths::load_framework(&frameworks) {
            eprintln!("riot-browser: 加载 CEF 框架失败");
            std::process::exit(1);
        }
        run(&frameworks);
    }
}

#[cfg(target_os = "macos")]
fn run(frameworks: &std::path::Path) {
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    // `[约束]` 必须在 execute_process / initialize 之前。见 mac 模块的说明 ——
    // 少了这一步，报出来的是 `icudtl.dat not found in bundle`，和真正的
    // 原因（NSApp 没就位）看起来毫无关系。
    mac::setup_application();

    let args = args::Args::new();

    // `[约束]` execute_process 必须在 initialize 之前调，且它的返回值
    // 决定本进程的身份:>= 0 说明这是 CEF 派生的子进程，已经跑完该跑的，
    // 直接退出。只有返回 -1 的那个才是浏览器进程。
    //
    // 这里其实走不到子进程分支（子进程是另一个可执行文件），但保留判断:
    // 少了它，一旦 helper 配置出错，子进程会误以为自己是浏览器进程，
    // 然后递归地再 spawn 一批子进程。
    let ret = execute_process(
        Some(args.as_main_args()),
        None::<&mut App>,
        std::ptr::null_mut(),
    );
    if ret >= 0 {
        std::process::exit(ret);
    }

    let cache = paths::cache_dir();
    if let Err(e) = std::fs::create_dir_all(&cache) {
        eprintln!("riot-browser: 建缓存目录失败 {}: {e}", cache.display());
    }

    let settings = Settings {
        // helper 是独立可执行文件，必须显式指路 —— 默认值假设的是
        // Chromium 官方那套命名。
        browser_subprocess_path: CefString::from(
            paths::helper_exe(frameworks).to_string_lossy().as_ref(),
        ),
        framework_dir_path: CefString::from(
            paths::framework_dir(frameworks).to_string_lossy().as_ref(),
        ),
        // 离屏渲染。不开的话 on_paint 永远不会被调。
        windowless_rendering_enabled: 1,
        // `[约束]` 独立 profile。见 paths::cache_dir 的注释。
        root_cache_path: CefString::from(cache.to_string_lossy().as_ref()),
        // 沙箱要求 helper 有额外的签名和 entitlement，先关掉。
        // TODO: 发版前打开，agent 驱动的浏览器更需要这层。
        no_sandbox: 1,
        ..Default::default()
    };

    let mut app = HostApp::new();
    assert_eq!(
        initialize(
            Some(args.as_main_args()),
            Some(&settings),
            Some(&mut app),
            std::ptr::null_mut(),
        ),
        1,
        "cef_initialize 失败"
    );

    run_message_loop();
    shutdown();
}

wrap_app! {
    struct HostApp;

    impl App {
        /// 在 CEF 解析命令行之前，关掉会把无窗口浏览器弄崩的 Chrome 功能。
        ///
        /// `[约束]` `ImmersiveReadAnything`（Chromium 151 默认开启）必须禁用。
        /// 它给每次页面加载挂一个 `ReadAnythingSoftNavigationObserver`，SPA
        /// 软导航（一次点击引发 pushState + DOM 变化，Discourse 点进帖子就是
        /// 这个形状）触发时，它去取 WebContents 上 Chrome 标签条挂的
        /// `tabs::TabInterface` —— 这里的浏览器没有标签条，取出来是空，而那个
        /// getter 不判空直接解引用，**整个浏览器进程 SIGSEGV**。
        ///
        /// 症状极难倒推：点一下帖子，面板"回到新标签页"（进程没了，宿主惰性
        /// 重开），崩溃报告的栈全在 CEF 内部，一个字不提阅读模式；同一页面
        /// 光加载不点不崩，data: 页面上点烂了也不崩（pushState 对 data: 不可
        /// 用，凑不齐软导航的条件）。面板的点击和模型工具的点击走的是同一条
        /// 输入注入，所以两边都踩得中。
        ///
        /// 禁用整个 feature 而不是绕着走：观察者在 feature 检查后第一步就崩，
        /// 而"阅读模式"这套 UI 在无窗口模式下本来就无处安放。
        fn on_before_command_line_processing(
            &self,
            _process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            let Some(cl) = command_line else { return };
            let key = CefString::from("disable-features");
            // 合并而不是覆盖。同名开关取后写的那份，直接 append 会把外部
            // 传进来的清单顶掉；反过来漏了合并，我们这份就被 CEF 稍后追加
            // 自家清单（LensOverlay 那些）时接不上 —— CEF 是按"已有值 +
            // 自家值"拼的，链条断在哪一环都等于没禁。
            let mut features = String::from("ImmersiveReadAnything");
            if cl.has_switch(Some(&key)) != 0 {
                let existing = CefString::from(&cl.switch_value(Some(&key))).to_string();
                if !existing.is_empty() {
                    features = format!("{existing},{features}");
                }
            }
            cl.append_switch_with_value(
                Some(&key),
                Some(&CefString::from(features.as_str())),
            );
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(HostProcessHandler::new())
        }
    }
}

wrap_browser_process_handler! {
    struct HostProcessHandler;

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            // CEF 就绪，但**不建浏览器**。开哪些标签页由主应用决定 ——
            // 这里自己开一个就等于替它做主，而它可能正要恢复上次的几个页面，
            // 于是第一个页面永远是多出来的那个。
            //
            // 读 stdin 的线程要等 CEF 就绪之后再起 —— 早起的话，命令会投到
            // 一个还没有 UI 线程消息循环的地方。
            crate::dispatch::spawn_stdin_reader();
            crate::wire::emit(&riot_protocol::browser::Event::Ready);
        }
    }
}
