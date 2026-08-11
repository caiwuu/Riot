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
mod protocol;

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
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(HostProcessHandler::new())
        }
    }
}

wrap_browser_process_handler! {
    struct HostProcessHandler;

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            // CEF 就绪。建一个离屏浏览器，加载一个固定页面 ——
            // 里程碑 1 只要证明帧能产出，URL 之后由 stdio 协议给。
            let mut client = osr::OsrClient::new(osr::OsrRenderHandler::new(Default::default()));

            let window_info = WindowInfo {
                windowless_rendering_enabled: 1,
                // `[约束]` 离屏渲染只有 Alloy 这套 runtime style 支持。
                // 用默认的 Chrome style 不会报错，它会正常创建浏览器、
                // 正常加载页面，只是 on_paint 一次都不调 —— 表现为
                // "什么都对，就是收不到帧"。
                runtime_style: RuntimeStyle::ALLOY,
                ..Default::default()
            };

            // 起来先停在空白页，等主应用发 Navigate。进程一起来就联网是不对的:
            // 用户可能只是打开了面板，还没决定看什么。
            //
            // `[约束]` 空白页用 `data:`，**不要用 `about:blank`**。
            //
            // 从 `about:blank` 导航到 https 会让 renderer 进程直接消失，页面
            // 报 `ERR_ABORTED`，紧接着 CDP 收到
            // `Inspector.detached / Render process gone`。而同一个导航从
            // `data:` 空页或任何真实页面出发都完全正常 —— 实测对比过三种起点。
            //
            // 看现象很容易误判成"创建后不能导航"或者"Chromium 崩了"，
            // 而实际只是起始页的选择问题。
            let url = CefString::from(
                std::env::var("RIOT_BROWSER_URL")
                    .unwrap_or_else(|_| "data:text/html,<html><body></body></html>".to_owned())
                    .as_str(),
            );
            let browser_settings = BrowserSettings::default();

            browser_host_create_browser(
                Some(&window_info),
                Some(&mut client),
                Some(&url),
                Some(&browser_settings),
                None,
                None,
            );

            // 读 stdin 的线程要等 CEF 就绪之后再起 —— 早起的话，命令会
            // 投到一个还没有浏览器的 UI 线程上，全部以"还没有浏览器"报错。
            crate::dispatch::spawn_stdin_reader();
        }
    }
}
