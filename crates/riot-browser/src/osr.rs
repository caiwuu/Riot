//! 离屏渲染:页面画在内存缓冲里，不上屏。
//!
//! 主应用要的是"把页面显示在自己的面板里"，而 CEF 的原生视图在另一个
//! 进程，没法直接嵌进 Tauri 的窗口。离屏渲染把这件事变成纯数据传输:
//! CEF 交给我们像素，我们交给主应用，主应用画在 canvas 上。
//!
//! 顺带解决了输入:面板里的点击变成 `send_mouse_click_event`，和真实
//! 浏览器里的点击走同一条路径 —— 不是合成 DOM 事件那种近似。
//!
//! # 每个标签页一套
//!
//! 一个标签页就是一个 CEF browser，各带一个 client、一套 handler。视口尺寸
//! 也是各自的:标签页在面板里轮流显示，但**尺寸对所有页都适用** —— 切过去
//! 才发现它还是上一次的尺寸，页面会先按旧尺寸闪一下再重排。

use std::cell::Cell;
use std::collections::HashMap;
use std::cell::RefCell;

use cef::rc::Rc;
use cef::*;

use crate::dispatch;
use riot_protocol::browser::{Event, TabId};

/// 一个标签页的视口。
#[derive(Clone, Copy)]
struct View {
    width: i32,
    height: i32,
    /// 像素密度。1 是普通屏，Retina 上是 2。
    scale: f32,
}

/// 面板还没给过尺寸时按这个来。撑得开大多数页面的桌面版布局。
const DEFAULT_VIEW: View = View {
    width: 1280,
    height: 800,
    scale: 1.0,
};

/// 密度的上下限。
///
/// 下限 1 挡的是 0 和负数:那两个值会让 Chromium 在算画布尺寸时除以零，
/// 渲染进程直接崩，而主应用只看到"浏览器没了"。上限 3 是止损 —— 画布是
/// 按密度的平方涨的，一个手滑传进来的 10 会让一帧变成一百倍面积。
const SCALE_RANGE: (f32, f32) = (1.0, 3.0);

thread_local! {
    /// 每个标签页的视口。
    ///
    /// `[约束]` 只在 UI 线程上碰。`view_rect` / `screen_info` 是 CEF 在 UI
    /// 线程上调的，而改尺寸的命令也是 post 到 UI 线程执行的 —— 用
    /// thread_local 把这件事变成编译期可见的，省掉一层同步推理。
    static VIEWS: RefCell<HashMap<TabId, View>> = RefCell::new(HashMap::new());
}

/// 记下某个标签页的视口。
pub fn set_view(tab: TabId, width: i32, height: i32, scale: f32) {
    // `[约束]` 不接受 0 或负数。CEF 拿到 0 尺寸会认为视口不可见，
    // 从此不再调 on_paint —— 表现是"拖一下面板页面就死了"，而且不报错。
    //
    // NaN 走 clamp 会 panic，先挡掉。
    let scale = if scale.is_finite() { scale } else { 1.0 };
    let view = View {
        width: width.max(1),
        height: height.max(1),
        scale: scale.clamp(SCALE_RANGE.0, SCALE_RANGE.1),
    };
    VIEWS.with_borrow_mut(|m| m.insert(tab, view));
}

/// 标签页关掉时把它的尺寸一起丢掉 —— 号会重用，留着就会串到新页上。
pub fn forget_view(tab: TabId) {
    VIEWS.with_borrow_mut(|m| m.remove(&tab));
}

fn view_of(tab: TabId) -> View {
    VIEWS.with_borrow(|m| m.get(&tab).copied().unwrap_or(DEFAULT_VIEW))
}

cef::wrap_render_handler! {
    pub struct OsrRenderHandler {
        tab: TabId,
        seq: Cell<u64>,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(r) = rect {
                let v = view_of(self.tab);
                r.x = 0;
                r.y = 0;
                r.width = v.width;
                r.height = v.height;
            }
        }

        /// 屏幕信息。这里真正有用的只有像素密度。
        ///
        /// `[约束]` 不实现这个函数，CEF 就按默认的 1.0 渲染 —— 一个 CSS
        /// 像素画一个物理像素。而面板在 Retina 屏上占的是两倍物理像素，
        /// 中间隔着一次放大，文字边缘全是虚的。这种糊很难归因：内容、比例、
        /// 点击位置全都是对的，看起来只像是"JPEG 质量调低了"。
        ///
        /// `view_rect` 给的是 CSS 像素，密度是在它之上再乘的 —— 两者一起
        /// 决定画布的物理尺寸，页面的排版尺寸只看前者。所以调密度不会让
        /// 页面重新排版，只是画得更细。
        ///
        /// 返回 0 表示"没填，用默认值"。填了就必须返回 1。
        fn screen_info(
            &self,
            _browser: Option<&mut Browser>,
            screen_info: Option<&mut ScreenInfo>,
        ) -> ::std::os::raw::c_int {
            let Some(info) = screen_info else {
                return 0;
            };
            let v = view_of(self.tab);
            let rect = Rect { x: 0, y: 0, width: v.width, height: v.height };
            info.device_scale_factor = v.scale;
            info.depth = 24;
            info.depth_per_component = 8;
            info.is_monochrome = 0;
            // 离屏渲染没有真正的屏幕。报成和视口一样大 —— 页面读
            // window.screen 时至少拿到一组自洽的数，而不是 0。
            info.rect = rect.clone();
            info.available_rect = rect;
            1
        }

        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            _dirty_rects: Option<&[Rect]>,
            _buffer: *const u8,
            width: ::std::os::raw::c_int,
            height: ::std::os::raw::c_int,
        ) {
            // 弹层（下拉菜单之类）走 POPUP，和主视图分开合成。
            // 先只报主视图，弹层留到面板真正要交互时再处理。
            if type_ != PaintElementType::VIEW {
                return;
            }
            let seq = self.seq.get() + 1;
            self.seq.set(seq);

            // TODO: 把 buffer 搬到共享内存。现在只报元数据 ——
            // 4MB/帧 走 JSON 是不可行的，见 protocol 模块的说明。
            crate::wire::emit(&Event::Frame { tab: self.tab, seq, width, height });
        }
    }
}

cef::wrap_client! {
    pub struct OsrClient {
        tab: TabId,
        render_handler: RenderHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render_handler.clone())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(OsrLifeSpan::new(self.tab))
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(OsrLoad::new(self.tab))
        }
    }
}

/// 建一个标签页用的 client。
///
/// 包装宏生成的 `new` 回的是 CEF 的 `Client`，不是那个 struct 本身 ——
/// 它已经是被引用计数包好的形态了。
pub fn client_for(tab: TabId) -> Client {
    OsrClient::new(tab, OsrRenderHandler::new(tab, Cell::new(0)))
}

/// `window.open()` 不带地址时 CEF 给的地址。
///
/// 不能拿它去导航:见 [`riot_protocol::browser::BLANK_PAGE`] —— 从
/// `about:blank` 跳到 https 会让渲染进程直接消失。当成"没给地址"处理，
/// 新标签页就停在协议规定的那张空白页上。
const ABOUT_BLANK: &str = "about:blank";

cef::wrap_life_span_handler! {
    pub struct OsrLifeSpan {
        tab: TabId,
    }

    impl LifeSpanHandler {
        /// 页面要开一个新的浏览上下文。一律拦下来，交给主应用开成标签页。
        ///
        /// `[约束]` 这个函数必须实现。默认实现返回 0 = 放行，而放行的后果在
        /// 离屏渲染下相当糟糕，且完全不像是弹窗处理的问题:
        ///
        /// 1. **弹窗会变成一个独立的原生窗口。** 离屏渲染是**每个 browser**
        ///    的设置，不是全局的 —— 它来自建 browser 时那个 `WindowInfo`
        ///    （见 [`dispatch::open_tab`]）。CEF 给弹窗的 `WindowInfo` 是一份
        ///    默认值，`windowless_rendering_enabled` 是 0。于是这个 browser
        ///    走的是有窗模式，CEF 给它开一个真的 NSWindow:一个飘在 Riot
        ///    外面的浏览器窗口，不在标签栏里，面板管不着它。
        ///
        /// 2. **它会顶掉母页面的标签页号。** CEF 传进来的 `client` 预置成
        ///    母页面的 client，不改就是共用 —— 而我们的 client 里钉着一个
        ///    `tab`（见 [`OsrClient`]）。于是弹窗的 `on_after_created` 和
        ///    `on_before_close` 报的都是**母页面的号**。前者把表里母页面的
        ///    句柄换成弹窗的，后者（用户关掉那个窗口时）把母页面整条抹掉。
        ///
        /// 第 2 条是真正致命的那个:母页面的 CEF browser 还活着、画面还在，
        /// 但表里已经没有它的句柄了。此后每条命令都以"标签页不存在"被丢掉，
        /// 每次 CDP 调用都要等满超时 —— 面板卡死，而唯一的线索是一串
        /// "标签页 N 不存在"，指向的号看上去明明就开着。
        ///
        /// 返回 1 = 取消。代价是 `window.open()` 的返回值变成 null，靠
        /// `w.postMessage(...)` 和弹窗通信的页面（部分 OAuth 流程）会失效 ——
        /// 这和浏览器拦弹窗时的代价一样，换来的是"所有页面都在面板里"。
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: ::std::os::raw::c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            target_disposition: WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            let url = target_url.map(ToString::to_string).unwrap_or_default();
            let url = if url == ABOUT_BLANK { String::new() } else { url };

            // 不按 disposition 分支决定"拦不拦"，只用它决定开在前台还是后台。
            // 任何一条放行的分支都会掉进上面那两个坑里，而 disposition 的取值
            // 由 Chromium 定义、会随版本增加 —— 漏掉一个新值的代价是弹窗
            // 又变回原生窗口。画中画、分屏这些也一样开成普通标签页:面板只有
            // 一种承载形式，硬要还原那些形态不如老实开一页。
            crate::wire::emit(&Event::PopupRequested {
                source: self.tab,
                url,
                background: target_disposition == WindowOpenDisposition::NEW_BACKGROUND_TAB,
            });
            1
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            // 句柄记在 UI 线程上，之后所有命令都投到这里执行。
            dispatch::tab_created(self.tab, browser.map(|b| b.clone()));
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            // 不清掉的话，句柄会比浏览器活得久，后续命令打在已销毁的对象上。
            //
            // 带上 CEF 的 browser 号:这个 client 可能被别的 browser 共用，
            // 只按标签页号清会清错人，见 [`dispatch::tab_closed`]。
            dispatch::tab_closed(self.tab, browser.map(|b| b.identifier()));
        }
    }
}

cef::wrap_load_handler! {
    pub struct OsrLoad {
        tab: TabId,
    }

    impl LoadHandler {
        fn on_load_end(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            status_code: ::std::os::raw::c_int,
        ) {
            // 只报主框架。iframe 的加载完成对上层没有意义，而且一个页面
            // 可能有几十个 —— 全报会把事件流冲满。
            let is_main = frame.as_ref().is_some_and(|f| f.is_main() != 0);
            if !is_main {
                return;
            }
            let url = frame
                .map(|f| CefString::from(&f.url()).to_string())
                .unwrap_or_default();
            crate::wire::emit(&Event::LoadEnd { tab: self.tab, status: status_code, url });
        }

        fn on_load_error(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            error_code: Errorcode,
            error_text: Option<&CefString>,
            failed_url: Option<&CefString>,
        ) {
            crate::wire::emit(&Event::LoadError {
                tab: self.tab,
                code: error_code.get_raw(),
                text: error_text.map(ToString::to_string).unwrap_or_default(),
                url: failed_url.map(ToString::to_string).unwrap_or_default(),
            });
        }
    }
}
