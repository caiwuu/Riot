//! 离屏渲染:页面画在内存缓冲里，不上屏。
//!
//! 主应用要的是"把页面显示在自己的面板里"，而 CEF 的原生视图在另一个
//! 进程,没法直接嵌进 Tauri 的窗口。离屏渲染把这件事变成纯数据传输:
//! CEF 交给我们像素,我们交给主应用,主应用画在 canvas 上。
//!
//! 顺带解决了输入:面板里的点击变成 `send_mouse_click_event`，和真实
//! 浏览器里的点击走同一条路径 —— 不是合成 DOM 事件那种近似。

use std::cell::Cell;

use cef::rc::Rc;
use cef::*;

/// 视口尺寸。真实实现里由主应用告知，这里先固定。
pub const WIDTH: i32 = 1280;
pub const HEIGHT: i32 = 800;

cef::wrap_render_handler! {
    pub struct OsrRenderHandler {
        painted: Cell<u64>,
        rect_calls: Cell<u64>,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            // `[约束]` 必须填非零尺寸。给 0 的话 CEF 认为视口不可见，
            // 永远不会调 on_paint —— 表现是"页面加载了但一帧都收不到"。
            if let Some(r) = rect {
                r.x = 0;
                r.y = 0;
                r.width = WIDTH;
                r.height = HEIGHT;
            }
            let n = self.rect_calls.get() + 1;
            self.rect_calls.set(n);
            if n <= 2 {
                eprintln!("[osr] view_rect 被调用 #{n} —— render handler 已接上");
            }
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
            let n = self.painted.get() + 1;
            self.painted.set(n);
            // 里程碑 1 只证明帧在产出。缓冲区的搬运留给下一步 ——
            // 那里要连着做节流和编码，现在打日志会把 stdout 冲爆。
            if n <= 3 || n % 60 == 0 {
                eprintln!("[osr] frame #{n} {width}x{height}");
            }
        }
    }
}

cef::wrap_client! {
    pub struct OsrClient {
        render_handler: RenderHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render_handler.clone())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(OsrLifeSpan::new())
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(OsrLoad::new())
        }
    }
}

// 诊断用。帧收不到时，最先要分清的是"浏览器没建起来"、"页面没加载"、
// 还是"渲染回调没接上" —— 这三种的修法完全不同。
cef::wrap_life_span_handler! {
    pub struct OsrLifeSpan;

    impl LifeSpanHandler {
        fn on_after_created(&self, _browser: Option<&mut Browser>) {
            eprintln!("[osr] 浏览器已创建");
        }
    }
}

cef::wrap_load_handler! {
    pub struct OsrLoad;

    impl LoadHandler {
        fn on_load_end(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            status_code: ::std::os::raw::c_int,
        ) {
            eprintln!("[osr] 加载完成 status={status_code}");
        }

        fn on_load_error(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            error_code: Errorcode,
            error_text: Option<&CefString>,
            failed_url: Option<&CefString>,
        ) {
            eprintln!(
                "[osr] 加载失败 code={error_code:?} text={:?} url={:?}",
                error_text.map(ToString::to_string),
                failed_url.map(ToString::to_string),
            );
        }
    }
}
