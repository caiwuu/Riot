//! 找到 CEF 的框架和 helper。
//!
//! # 为什么这件事需要一个模块
//!
//! CEF 在 macOS 上**必须**从一个真正的 `.app` 里加载:资源(`icudtl.dat`、
//! `.pak`、locales)是通过 `[NSBundle mainBundle]` 定位的，而裸二进制没有
//! main bundle。实测过 `framework_dir_path` / `main_bundle_path` 都救不回来
//! —— ICU 的加载发生在这些设置生效之前，直接就是
//! `icudtl.dat not found in bundle`。
//!
//! 这条限制正是 `riot-browser` 独立成一个进程的原因:主应用在 `tauri dev`
//! 下跑的是裸二进制，永远满足不了它。

use std::path::{Path, PathBuf};

/// `.framework` 目录名。
const FRAMEWORK: &str = "Chromium Embedded Framework.framework";
/// 框架二进制在 `.framework` 里的相对位置。
const FRAMEWORK_BIN: &str = "Chromium Embedded Framework.framework/Chromium Embedded Framework";

/// `framework_dir_path` 该填的值。
///
/// `[约束]` 是 **`.framework` 目录本身**，不是它所在的 `Contents/Frameworks`。
///
/// 填错一层的报错是 `icudtl.dat not found in bundle` —— CEF 会去
/// `<你给的路径>/Resources/icudtl.dat` 找资源，指到上一层就什么都找不到，
/// 而错误信息完全不提路径，看起来像是打包漏了文件。
pub fn framework_dir(frameworks: &Path) -> PathBuf {
    frameworks.join(FRAMEWORK)
}

/// 本进程所在的 `Contents/Frameworks` 目录。
///
/// 布局是 `X.app/Contents/MacOS/riot-browser`，所以从可执行文件往上两层
/// 再进 `Frameworks`。用 `current_exe` 的 parent 而不是它本身，是为了
/// 兼容通过符号链接启动的情况。
pub fn frameworks_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.join("../Frameworks").canonicalize().ok()?;
    dir.is_dir().then_some(dir)
}

/// 加载 CEF 框架。返回 `false` 表示没找到或加载失败。
///
/// `[约束]` 必须在任何其它 CEF 调用**之前**完成。框架是动态加载的，
/// 早一步调 `execute_process` 就是空指针解引用。
pub fn load_framework(frameworks: &Path) -> bool {
    let bin = frameworks.join(FRAMEWORK_BIN);
    let Ok(c) = std::ffi::CString::new(bin.to_string_lossy().as_bytes()) else {
        return false;
    };
    // SAFETY: 传的是以 NUL 结尾的合法路径字符串，CEF 只读不持有。
    unsafe { cef::load_library(Some(&*c.as_ptr().cast())) == 1 }
}

/// helper 可执行文件的路径。
///
/// macOS 上 CEF 的每种子进程都要单独的 `.app`（各自的 Info.plist 让它们
/// 不出现在 Dock 里）。这里指向主 helper，CEF 会按 `--type=` 自己挑。
pub fn helper_exe(frameworks: &Path) -> PathBuf {
    frameworks.join("riot-browser Helper.app/Contents/MacOS/riot-browser Helper")
}

/// 浏览器数据目录。
///
/// `[约束]` **绝不能**指向用户真实的浏览器 profile。
///
/// agent 驱动的浏览器一旦带上用户的登录态，一次 prompt injection 就能读走
/// 邮箱、代码仓库、银行页面里的内容。Codex 的内置浏览器同样明确不支持
/// 认证和 cookie，要登录得另走扩展 —— 那是刻意的产品边界，不是缺功能。
pub fn cache_dir() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map_or_else(
            || {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                PathBuf::from(home).join("Library/Application Support")
            },
            PathBuf::from,
        );
    base.join("riot").join("browser-profile")
}
