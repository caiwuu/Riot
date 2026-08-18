//! CEF 的子进程可执行文件。
//!
//! renderer / GPU / utility 都是这一个二进制，CEF 用命令行上的 `--type=`
//! 区分。它不做任何业务逻辑 —— 加载框架，把控制权交回 CEF，结束。
//!
//! `[约束]` 这里不能有任何耗时初始化。每开一个标签页就会起一个 renderer，
//! 在这里多做一件事就是每个标签页都多付一次。

// 理由同主进程（见 src/main.rs）:console 子系统的 helper 每被 CEF
// spawn 一次就弹一个黑窗，而 renderer 是每个标签页一个。
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(target_os = "macos")]
use std::path::PathBuf;

fn main() {
    // 布局是 `Frameworks/riot-browser Helper.app/Contents/MacOS/<exe>`，
    // 框架在 `Frameworks/` 下，所以往上三层。
    //
    // Windows 没有对应的步骤:libcef.dll 是链接期挂上的，helper 和它
    // 同目录，加载器自己就能找到。
    #[cfg(target_os = "macos")]
    {
        let Some(frameworks) = helper_frameworks_dir() else {
            eprintln!("riot-browser-helper: 找不到 Frameworks 目录");
            std::process::exit(1);
        };
        let bin = frameworks
            .join("Chromium Embedded Framework.framework/Chromium Embedded Framework");
        let Ok(c) = std::ffi::CString::new(bin.to_string_lossy().as_bytes()) else {
            std::process::exit(1);
        };
        // SAFETY: 合法的 NUL 结尾路径；CEF 只读。
        if unsafe { cef::load_library(Some(&*c.as_ptr().cast())) } != 1 {
            eprintln!("riot-browser-helper: 加载 CEF 框架失败: {}", bin.display());
            std::process::exit(1);
        }
    }

    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);

    let args = cef::args::Args::new();
    // 子进程走完自己那一轮就返回；返回值 >= 0 是进程退出码。
    let code = cef::execute_process(
        Some(args.as_main_args()),
        None::<&mut cef::App>,
        std::ptr::null_mut(),
    );
    std::process::exit(code.max(0));
}

#[cfg(target_os = "macos")]
fn helper_frameworks_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.parent()?.join("../../..").canonicalize().ok()
}
