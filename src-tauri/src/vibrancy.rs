//! 系统材质，以及把窗口外观钉在页面正在用的那一档上。
//!
//! 界面有深浅两套配色（前端 `src/theme.ts` 解析用户选择，挂成
//! `<html data-theme>`）。窗口的原生部分得跟着页面走，不然两边打架：
//! - macOS 的 sidebar 材质（`NSVisualEffectMaterialSidebar`）跟着
//!   `effectiveAppearance` 走 —— 页面深色、系统浅色时不钉住，侧栏会变成系统
//!   浅色侧栏那种浅灰；反过来页面浅色、系统深色，侧栏就是一块黑。
//! - Windows 的 mica 明暗跟 `DWMWA_USE_IMMERSIVE_DARK_MODE` 走，标题栏同理：
//!   深色页面配浅色标志，侧栏底下垫一层白雾。
//!
//! 所以外观不是"钉成深色"而是"钉成 [`Appearance`] 说的那一档"；用户选"跟随
//! 系统"时**解除**钉死（appearance = nil / 标志按系统算），材质和 webview 里的
//! `prefers-color-scheme` 一起回到系统设置。
//!
//! `[约束]` "跟随系统"必须真的解除钉死，不能用"读一次系统值再钉上"代替：只要
//! NSApp 外观被钉住，WKWebView 里的 `prefers-color-scheme` 就永远报钉住的那档，
//! 前端再也收不到系统切换的 `change` 事件，"跟随"就成了"钉在启动那一刻"。
//!
//! 启动那一帧：`tauri.conf.json` 的 `theme: Dark` 在建窗时先把 NSApp 钉成
//! DarkAqua，等前端跑起来再调 `set_appearance` 已经晚了一帧 —— 选了浅色的用户
//! 每次启动都会看到侧栏从深翻到浅。所以宿主自己记一份上次应用的外观
//! （`appearance` 文件，和 config.json 同目录，见 [`remembered`]），setup 里按它
//! 重钉一次。这份记录只是首帧的提示，不是事实来源：前端每次启动和每次切换
//! 都会再调一遍 [`apply`]。缺文件（首次启动、老版本升上来）按深色，和以前的
//! 行为一致，第一次启动最多闪一下。
//!
//! macOS：配置里 `windowEffects: sidebar` 铺材质，本模块把 NSApp、窗口和内容
//! 视图树的 appearance 一起钉 —— 语义材质读的是视图自己的 effectiveAppearance，
//! 只钉 NSApp 有时罩不住 overlay 标题栏底下那层 NSVisualEffectView。解除钉死
//! 也要走同一棵树：哪一层还留着显式 appearance，那一层就还是旧样子。
//!
//! Windows：让侧栏透出系统材质（DWM 的 mica）。
//!
//! 选 mica 不选 acrylic：mica 采样桌面壁纸，色调稳定、跟窗口后面压着什么无关，
//! 也是 Codex / WinUI 窗体的默认观感；acrylic 模糊的是紧贴窗后的内容，背后是
//! 白文档侧栏就发白，背后是深色就发黑，观感跟着别的窗口走。
//!
//! Windows 要四步，而配置只表达得了第一步 —— 只做第一步的话，客户区里透明像素
//! 合成出来是一片黑：材质只画在玻璃帧上，帧又没扩进客户区。
//!
//! ① 关掉 tao 给透明窗口开的逐像素 alpha —— 它建窗时拿一个空区域调
//!    `DwmEnableBlurBehindWindow` 开出来的（tao 的 window.rs）。留着它，透明像素
//!    会穿过 DWM 那层直接落到桌面上：背后窗口的原始画面一点没模糊，材质等于没有。
//! ② `DWMWA_SYSTEMBACKDROP_TYPE = mica`：让 DWM 在窗口底下画一层云母材质。
//!    配置里写 `windowEffects: acrylic` 也能设上，但 Tauri 吞掉返回值 —— 而系统
//!    支不支持全看那个 HRESULT（见下面对 ③ 的门控），所以这一步自己调。
//!    配置里只留 macOS 的 `sidebar`，Windows 这条路整条归这个模块。
//! ③ 把 DWM 的玻璃帧扩到整个客户区。系统材质只画在帧上，不扩一个像素都看不见。
//!    WinUI / Electron 的 acrylic 也都得自己扩这一下，window-vibrancy 没做。
//! ④ `SWP_FRAMECHANGED` 踢一脚。窗口已经在屏上了，上面几个属性改完 DWM 不会
//!    自己重建合成树 —— 不踢的话前三步全部"调用成功、画面没变"，看起来就像
//!    这个模块不存在。切换明暗时同样要踢：标志改了，材质的色调要等重算才换。
//!
//! `[约束]` ① 无条件做，且旧系统上只做 ①。②失败（DWMSBT 要 Win11 22523+）时
//! 玻璃帧不能扩 —— Win10 上没有材质可画，扩开只会得到一块黑。只关 alpha 的话
//! 透明像素落在窗口自己的底上，侧栏退化成一块纯色，而不是把背后的窗口原样透出来。
//!
//! 四步都是幂等的，所以运行时切换明暗直接把整套重跑一遍，不另写一条"只改
//! 标志"的窄路 —— 两条路迟早有一条漏掉某一步。
//!
//! Linux：没有要钉的材质。明暗交给 Tauri 的 `set_theme`（落到 GTK 的
//! prefer-dark），此外没有事做。
//!
//! 页面得先把背景让开才看得见材质，见 `src/main.tsx` 的 `[data-vibrancy]`。

use std::path::Path;

use tauri::{AppHandle, Theme, WebviewWindow};

#[cfg(windows)]
use windows::Win32::Foundation::HWND;
#[cfg(windows)]
use windows::Win32::Graphics::Dwm::{
    DWM_BB_ENABLE, DWM_BLURBEHIND, DWMSBT_MAINWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DwmEnableBlurBehindWindow, DwmExtendFrameIntoClientArea,
    DwmSetWindowAttribute,
};
#[cfg(windows)]
use windows::Win32::Graphics::Gdi::HRGN;
#[cfg(windows)]
use windows::Win32::UI::Controls::MARGINS;
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
};

/// 窗口该钉成哪一档外观。前端 `set_appearance` 的参数（JSON 小写），也是
/// `appearance` 文件里存的词。
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    Light,
    Dark,
    /// 不钉，跟系统走。见模块说明里为什么这一档必须是"解除"而不是"读一次"。
    System,
}

impl Appearance {
    /// 没有记录时的外观。深色 —— 和 `tauri.conf.json` 建窗用的 `theme: Dark`、
    /// 以及只有深色那个年代的行为一致。
    pub const DEFAULT: Appearance = Appearance::Dark;

    /// 交给 Tauri `set_theme` 的形态：`None` 就是跟系统。
    fn tauri_theme(self) -> Option<Theme> {
        match self {
            Appearance::Light => Some(Theme::Light),
            Appearance::Dark => Some(Theme::Dark),
            Appearance::System => None,
        }
    }

    /// 文件里存的词。和 serde 的 `lowercase` 一致，但不经 JSON —— 一个词不值得
    /// 带引号。
    fn as_str(self) -> &'static str {
        match self {
            Appearance::Light => "light",
            Appearance::Dark => "dark",
            Appearance::System => "system",
        }
    }

    fn parse(s: &str) -> Option<Appearance> {
        match s {
            "light" => Some(Appearance::Light),
            "dark" => Some(Appearance::Dark),
            "system" => Some(Appearance::System),
            _ => None,
        }
    }
}

/// 上次应用的外观。读不到、认不出都按 [`Appearance::DEFAULT`] —— 这只是首帧的
/// 提示，前端起来会再对一次，错了最多闪一下。
pub fn remembered(path: &Path) -> Appearance {
    // 豁免理由：宿主持久化层，读的是自己写的一个词。
    #[allow(clippy::disallowed_methods)]
    match std::fs::read_to_string(path) {
        Ok(raw) => Appearance::parse(raw.trim()).unwrap_or_else(|| {
            tracing::warn!(value = raw.trim(), "appearance file has an unknown value; using default");
            Appearance::DEFAULT
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Appearance::DEFAULT,
        Err(e) => {
            tracing::warn!(error = %e, "failed to read appearance file; using default");
            Appearance::DEFAULT
        }
    }
}

/// 记下这次应用的外观，下次建窗后照它钉。失败只记日志：后果是下次启动闪一下，
/// 不值得报给用户。
pub fn remember(path: &Path, appearance: Appearance) {
    // 豁免理由：宿主持久化层，写的是自己的一个词。不做临时文件 + rename：
    // 内容只有一个词，撕裂写出来的东西 parse 不过，回落到默认，无害。
    #[allow(clippy::disallowed_methods)]
    let result = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(path, appearance.as_str()));
    if let Err(e) = result {
        tracing::warn!(error = %e, "failed to remember appearance; next launch may flash");
    }
}

/// 把窗口的原生外观切到 `appearance`。启动（setup）和前端每次切换都走这里。
///
/// 先过 Tauri 的 `set_theme`：它管 NSApp 外观（macOS）、窗口与事件循环两级的
/// 首选主题（Windows —— 两级都得改，tao 解析时窗口那级缺省会回落到事件循环
/// 那级，setup 里曾经 `app.set_theme(Dark)` 留下的值不清掉，`None` 就不是
/// "跟系统"）、GTK 的 prefer-dark（Linux），顺带让 `window.theme()` 和
/// `ThemeChanged` 事件保持一致。然后平台分支只补 Tauri 罩不住的那部分：
/// macOS 的窗口 + 视图树，Windows 的 mica 四步。
///
/// 平台分支放到主线程跑：AppKit 的外观只能在主线程改（`MainThreadMarker`），
/// 而调用方可能是 setup（已在主线程，闭包就地执行）也可能是 IPC 命令。
/// 主线程上的消息按序处理，`set_theme` 一定先于闭包落地 —— Windows 分支里
/// "跟随系统"要读 `window.theme()`，读到的才是按系统重算过的值。
pub fn apply(app: &AppHandle, window: &WebviewWindow, appearance: Appearance) {
    let theme = appearance.tauri_theme();
    app.set_theme(theme);
    if let Err(e) = window.set_theme(theme) {
        tracing::warn!(error = %e, "window.set_theme failed; native chrome may lag behind the page");
    }
    let target = window.clone();
    if let Err(e) = window.run_on_main_thread(move || pin(&target, appearance)) {
        tracing::warn!(error = %e, "cannot reach the main thread; appearance not pinned");
    }
}

#[cfg(windows)]
fn pin(window: &WebviewWindow, appearance: Appearance) {
    let Ok(handle) = window.hwnd() else {
        tracing::warn!("拿不到窗口句柄，跳过系统材质");
        return;
    };
    // tauri 和宿主各自依赖一份 windows crate。HWND 两边都是 `*mut c_void` 的
    // newtype，同构，所以拆开重装而不是直接传。
    let hwnd = HWND(handle.0);

    // ① 关逐像素 alpha。
    let off = DWM_BLURBEHIND {
        dwFlags: DWM_BB_ENABLE,
        fEnable: false.into(),
        hRgnBlur: HRGN(std::ptr::null_mut()),
        fTransitionOnMaximized: false.into(),
    };
    if let Err(e) = unsafe { DwmEnableBlurBehindWindow(hwnd, &off) } {
        tracing::warn!(error = %e, "逐像素 alpha 没关掉，材质会被桌面顶掉");
    }

    // ② 云母。材质明暗跟这个标志走，所以标志要和页面一致："跟随系统"按此刻
    // 的系统设置算 —— 上面 set_theme(None) 之后 tao 已经按系统重算过窗口主题，
    // window.theme() 给的就是它。tao 自己也会写这个标志，这里再写一次是为了
    // 不依赖它的时序，四步里的 ④ 才能确定是在标志改完之后踢的。
    let dark = match appearance {
        Appearance::Dark => true,
        Appearance::Light => false,
        Appearance::System => match window.theme() {
            Ok(t) => t == Theme::Dark,
            Err(e) => {
                tracing::warn!(error = %e, "cannot read system theme; assuming dark");
                true
            }
        },
    };
    let dark = windows::core::BOOL::from(dark);
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            std::ptr::from_ref(&dark).cast(),
            std::mem::size_of_val(&dark) as u32,
        )
    };
    let backdrop = DWMSBT_MAINWINDOW;
    let supported = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            std::ptr::from_ref(&backdrop).cast(),
            std::mem::size_of_val(&backdrop) as u32,
        )
    };

    // ③ 扩玻璃帧，只在材质真设上了才扩。
    match supported {
        Ok(()) => {
            if let Err(e) = unsafe { DwmExtendFrameIntoClientArea(hwnd, &WHOLE_CLIENT_AREA) } {
                tracing::warn!(error = %e, "玻璃帧没扩开，侧栏会是纯色");
            }
        }
        // 不是错误：Win10 和早期 Win11 就是没有这层材质。
        Err(e) => tracing::debug!(error = %e, "系统没有 DWM 材质，侧栏用纯色"),
    }

    // ④ 让 DWM 重算这扇窗。位置、尺寸、Z 序全部原样，只要 FRAMECHANGED。
    if let Err(e) = unsafe {
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        )
    } {
        tracing::warn!(error = %e, "窗口帧没重算，材质可能不生效");
    }
}

/// 整个客户区都当玻璃帧。-1 是 DWM 约定的"铺满"。
#[cfg(windows)]
const WHOLE_CLIENT_AREA: MARGINS = MARGINS {
    cxLeftWidth: -1,
    cxRightWidth: -1,
    cyTopHeight: -1,
    cyBottomHeight: -1,
};

/// 把 NSApp / NSWindow / 内容视图树的 appearance 一起钉成要的那档；`System`
/// 就一起清成 nil，sidebar 材质和 webview 的 `prefers-color-scheme` 才会回到系统。
#[cfg(target_os = "macos")]
fn pin(window: &WebviewWindow, appearance: Appearance) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSApplication, NSView,
    };

    let Ok(ptr) = window.ns_view() else {
        tracing::warn!("拿不到 NSView，跳过外观钉死");
        return;
    };
    let Some(mtm) = MainThreadMarker::new() else {
        tracing::warn!("不在主线程，跳过外观钉死");
        return;
    };
    let target = match appearance {
        Appearance::System => None,
        Appearance::Light | Appearance::Dark => {
            // SAFETY: 这两个是 AppKit 导出的常量字符串，进程期内一直有效。
            let name = unsafe {
                if appearance == Appearance::Dark {
                    NSAppearanceNameDarkAqua
                } else {
                    NSAppearanceNameAqua
                }
            };
            let Some(named) = NSAppearance::appearanceNamed(name) else {
                tracing::warn!(?appearance, "系统没有这档外观，跳过钉死");
                return;
            };
            Some(named)
        }
    };
    let target = target.as_deref();

    NSApplication::sharedApplication(mtm).setAppearance(target);

    // SAFETY: ns_view 是本窗口的 AppKit 内容视图，窗口活着它就活着。
    let view = unsafe { &*ptr.cast::<NSView>() };
    if let Some(ns_window) = view.window() {
        ns_window.setAppearance(target);
    }
    pin_view_tree(view, target);
}

#[cfg(target_os = "macos")]
fn pin_view_tree(view: &objc2_app_kit::NSView, appearance: Option<&objc2_app_kit::NSAppearance>) {
    use objc2_app_kit::NSAppearanceCustomization;

    view.setAppearance(appearance);
    for sub in view.subviews() {
        pin_view_tree(&sub, appearance);
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn pin(_window: &WebviewWindow, _appearance: Appearance) {
    // Linux 没有要钉的材质；明暗已经由 apply 里的 set_theme 交给 GTK。
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 外观文件_写什么读回什么_坏值回默认() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("appearance");
        // 没文件：默认
        assert_eq!(remembered(&path), Appearance::DEFAULT);
        for a in [Appearance::Light, Appearance::Dark, Appearance::System] {
            remember(&path, a);
            assert_eq!(remembered(&path), a);
        }
        // 认不出的词回默认，不 panic
        #[allow(clippy::disallowed_methods)]
        std::fs::write(&path, "sepia\n").unwrap();
        assert_eq!(remembered(&path), Appearance::DEFAULT);
    }

    #[test]
    fn json_形态是小写_和前端一致() {
        assert_eq!(serde_json::to_string(&Appearance::System).unwrap(), "\"system\"");
        let parsed: Appearance = serde_json::from_str("\"light\"").unwrap();
        assert_eq!(parsed, Appearance::Light);
    }
}
