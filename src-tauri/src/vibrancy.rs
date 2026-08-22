//! Windows：让侧栏透出系统材质（DWM 的 mica）。
//!
//! 选 mica 不选 acrylic：mica 采样桌面壁纸，色调稳定、跟窗口后面压着什么无关，
//! 也是 Codex / WinUI 窗体的默认观感；acrylic 模糊的是紧贴窗后的内容，背后是
//! 白文档侧栏就发白，背后是深色就发黑，观感跟着别的窗口走。
//!
//! macOS 一行配置就够（`tauri.conf.json` 的 `windowEffects: sidebar`）。Windows
//! 要四步，而配置只表达得了第一步 —— 只做第一步的话，客户区里透明像素合成出来
//! 是一片黑：材质只画在玻璃帧上，帧又没扩进客户区。
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
//!    这个模块不存在。
//!
//! `[约束]` ① 无条件做，且旧系统上只做 ①。②失败（DWMSBT 要 Win11 22523+）时
//! 玻璃帧不能扩 —— Win10 上没有材质可画，扩开只会得到一块黑。只关 alpha 的话
//! 透明像素落在窗口自己的底上，侧栏退化成一块纯色，而不是把背后的窗口原样透出来。
//!
//! 页面得先把背景让开才看得见材质，见 `src/main.tsx` 的 `[data-vibrancy]`。

use tauri::WebviewWindow;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DWM_BB_ENABLE, DWM_BLURBEHIND, DWMSBT_MAINWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DwmEnableBlurBehindWindow, DwmExtendFrameIntoClientArea,
    DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::HRGN;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::WindowsAndMessaging::{
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
};

/// 整个客户区都当玻璃帧。-1 是 DWM 约定的"铺满"。
const WHOLE_CLIENT_AREA: MARGINS = MARGINS {
    cxLeftWidth: -1,
    cxRightWidth: -1,
    cyTopHeight: -1,
    cyBottomHeight: -1,
};

pub fn apply(window: &WebviewWindow) {
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

    // ② 云母。材质明暗跟窗口主题走，而这个应用的配色是写死的深色 ——
    // 浅色系统上不钉住的话，深色侧栏底下会垫一层白雾。
    let dark = windows::core::BOOL::from(true);
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
