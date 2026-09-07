//! 系统通知：任务在后台跑完、定时任务结束时，往系统通知中心弹一条。
//!
//! 文案由前端按界面语言给（宿主没有词典），这层只管"以 Riot 的名义弹出来"。
//! 失败只记日志：通知是锦上添花，平台不支持、用户关了通知，都不值得打断调用方。
//!
//! macOS / Linux 走 tauri-plugin-notification。**Windows 不走它**：
//!
//! Windows 的横幅必须挂在一个 AppUserModelID（AUMID）下面，左上角的名字和图标
//! 都按这个 ID 去查。插件只在"exe 不在 `target/debug|release` 下"时才填应用的
//! identifier，其余情况退回 PowerShell 的 ID —— `pnpm tauri dev` 跑出来的通知
//! 顶着「Windows PowerShell」的名字和图标。填了 identifier 的安装版也只是把
//! 查名字的活交给开始菜单快捷方式（NSIS 模板给快捷方式挂的 AUMID）：解压即用、
//! 或者用户删了快捷方式，Windows 找不到这个 ID，横幅**干脆不弹**。
//!
//! 所以这里自己把 AUMID 注册进 `HKCU\Software\Classes\AppUserModelId\<identifier>`
//! （显示名 + 图标文件），再直接用 WinRT 的 Toast 按这个 ID 发。这是微软给
//! "没打包、也不靠快捷方式的桌面程序"指的路，dev / 便携 / 安装版三种跑法弹出来
//! 的一模一样，Windows 的通知设置里也会出现「Riot」这一项让用户自己调。
//! AUMID 用 `tauri.conf.json` 的 identifier，和快捷方式上挂的是同一个 ——
//! Windows 才把通知和任务栏上那个 Riot 当成同一个应用。
//!
//! 点横幅回到 Riot：通知说的是"回来看看结果"，点了却只是横幅消失就说不通。

use tauri::AppHandle;

/// 发一条系统通知。任何失败都在这里收掉，只留日志。
pub fn send(app: &AppHandle, title: &str, body: &str) {
    if let Err(e) = platform::send(app, title, body) {
        tracing::warn!(error = %e, "system notification not shown");
    }
}

#[cfg(windows)]
mod platform {
    use std::path::Path;
    use std::sync::OnceLock;

    use tauri::{AppHandle, Manager};
    use tauri_winrt_notification::{Duration, Sound, Toast};

    /// 横幅左上角的图。注册表里 `IconUri` 要的是磁盘上的图片文件：图标资源编在
    /// exe 里取不出来，源码目录只有开发机才有 —— 所以把图嵌进二进制，发通知前
    /// 写到应用的本地数据目录（WebView2 的数据也在那）。128px 够用，横幅上画出来
    /// 是 48 逻辑像素。
    const ICON_PNG: &[u8] = include_bytes!("../icons/128x128.png");

    /// 图标文件名。放在 `app_local_data_dir` 下，dev 和安装版指向同一个文件。
    const ICON_FILE: &str = "notification-icon.png";

    /// 注册只做一次，结果记下来。失败也记：注册表写不进去多半是策略锁死，
    /// 每条通知都重试只是刷日志。
    static REGISTERED: OnceLock<Result<(), RegisterError>> = OnceLock::new();

    #[derive(Debug, thiserror::Error)]
    enum RegisterError {
        #[error("cannot resolve app local data dir: {0}")]
        DataDir(#[from] tauri::Error),
        #[error("cannot write notification icon: {0}")]
        Icon(#[from] std::io::Error),
        // windows-registry 只导出 Result 不导出 Error；它的错误就是 windows-result
        // 的那个，和 windows crate 的 `core::Error` 是同一个类型。
        #[error("cannot write AppUserModelId registry key: {0}")]
        Registry(#[from] windows::core::Error),
    }

    pub fn send(
        app: &AppHandle,
        title: &str,
        body: &str,
    ) -> Result<(), tauri_winrt_notification::Error> {
        let app_id = match REGISTERED.get_or_init(|| register(app)) {
            Ok(()) => app.config().identifier.as_str(),
            // 没注册上的 ID Windows 不认，按它发等于没发。PowerShell 的 ID 系统
            // 一定认识：名字和图标是错的，但话总归送到了。
            Err(e) => {
                tracing::warn!(error = %e, "AUMID not registered; toast will be attributed to PowerShell");
                Toast::POWERSHELL_APP_ID
            }
        };

        let handle = app.clone();
        Toast::new(app_id)
            .title(title)
            .text1(body)
            .duration(Duration::Short)
            // 系统默认提示音。用户不在窗口前才会收到这条，光有画面多半错过；
            // 嫌吵可以在 Windows 的通知设置里按应用关掉 —— 注册了 AUMID 才有这一项。
            .sound(Some(Sound::Default))
            .on_activated(move |_action| {
                focus_main(&handle);
                Ok(())
            })
            .show()
    }

    /// 把主窗口拉回前台。回调跑在 WinRT 的线程上；Tauri 的窗口操作自己会送到
    /// 主线程执行，这里不用管。
    fn focus_main(app: &AppHandle) {
        let Some(window) = app.get_webview_window("main") else {
            return;
        };
        // 顺序有讲究：tao 的 set_focus 对最小化的窗口不做事，得先还原。
        if let Err(e) = window.unminimize() {
            tracing::debug!(error = %e, "unminimize failed");
        }
        if let Err(e) = window.show() {
            tracing::debug!(error = %e, "show failed");
        }
        if let Err(e) = window.set_focus() {
            tracing::debug!(error = %e, "set_focus failed");
        }
    }

    /// 把显示名和图标登记到 HKCU。幂等：每次启动后第一条通知前重写一遍，
    /// 应用改名、换图标都会自动跟上。
    fn register(app: &AppHandle) -> Result<(), RegisterError> {
        let icon = app.path().app_local_data_dir()?.join(ICON_FILE);
        write_icon(&icon)?;

        let key = windows_registry::CURRENT_USER.create(format!(
            r"Software\Classes\AppUserModelId\{}",
            app.config().identifier
        ))?;
        key.set_string("DisplayName", &app.package_info().name)?;
        key.set_hstring("IconUri", &icon.as_path().into())?;
        // 图标自带形状和底色，不要系统再垫一块色板。
        key.set_string("IconBackgroundColor", "0")?;
        Ok(())
    }

    /// 图标已经是这份就不动。
    fn write_icon(path: &Path) -> std::io::Result<()> {
        // 豁免理由：宿主层，写的是自己本地数据目录里的一张图。
        #[allow(clippy::disallowed_methods)]
        {
            if std::fs::read(path).is_ok_and(|current| current == ICON_PNG) {
                return Ok(());
            }
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(path, ICON_PNG)
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use tauri::AppHandle;
    use tauri_plugin_notification::NotificationExt;

    /// macOS 上插件会把通知挂到应用的 bundle id 下（dev 时挂 Terminal），
    /// Linux 走 D-Bus，两边都没有 Windows 那个问题。
    pub fn send(
        app: &AppHandle,
        title: &str,
        body: &str,
    ) -> Result<(), tauri_plugin_notification::Error> {
        app.notification()
            .builder()
            .title(title)
            .body(body)
            .show()
    }
}
