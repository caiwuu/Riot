//! 离屏渲染:页面画在内存缓冲里，不上屏。
//!
//! 主应用要的是"把页面显示在自己的面板里"，而 CEF 的原生视图在另一个
//! 进程，没法直接嵌进 Tauri 的窗口。离屏渲染把这件事变成纯数据传输:
//! CEF 交给我们像素，我们交给主应用，主应用画在 canvas 上。
//!
//! 顺带解决了输入:面板里的点击变成 `send_mouse_click_event`，和真实
//! 浏览器里的点击走同一条路径 —— 不是合成 DOM 事件那种近似。

use std::cell::Cell;
use std::sync::atomic::{AtomicI32, Ordering};

use cef::rc::Rc;
use cef::*;

use crate::dispatch;
use riot_protocol::browser::Event;

/// 视口尺寸。
///
/// 用原子量而不是 `Cell`:`view_rect` 在 UI 线程被调，而尺寸是主应用通过
/// `Resize` 命令改的（也在 UI 线程，但中间隔了 post_task）。原子量省掉了
/// 一层同步推理。
static VIEW_W: AtomicI32 = AtomicI32::new(1280);
static VIEW_H: AtomicI32 = AtomicI32::new(800);

pub fn set_view_size(width: i32, height: i32) {
    // `[约束]` 不接受 0 或负数。CEF 拿到 0 尺寸会认为视口不可见，
    // 从此不再调 on_paint —— 表现是"拖一下面板页面就死了"，而且不报错。
    VIEW_W.store(width.max(1), Ordering::Relaxed);
    VIEW_H.store(height.max(1), Ordering::Relaxed);
}

cef::wrap_render_handler! {
    pub struct OsrRenderHandler {
        seq: Cell<u64>,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(r) = rect {
                r.x = 0;
                r.y = 0;
                r.width = VIEW_W.load(Ordering::Relaxed);
                r.height = VIEW_H.load(Ordering::Relaxed);
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
            let seq = self.seq.get() + 1;
            self.seq.set(seq);

            // TODO: 把 buffer 搬到共享内存。现在只报元数据 ——
            // 4MB/帧 走 JSON 是不可行的，见 protocol 模块的说明。
            crate::wire::emit(&Event::Frame { seq, width, height });
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

cef::wrap_life_span_handler! {
    pub struct OsrLifeSpan;

    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            // 句柄记在 UI 线程上，之后所有命令都投到这里执行。
            dispatch::set_browser(browser.map(|b| b.clone()));
            crate::wire::emit(&Event::Ready);
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            // 不清掉的话，句柄会比浏览器活得久，后续命令打在已销毁的对象上。
            dispatch::set_browser(None);
        }
    }
}

cef::wrap_load_handler! {
    pub struct OsrLoad;

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
            crate::wire::emit(&Event::LoadEnd { status: status_code, url });
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
                code: error_code.get_raw(),
                text: error_text.map(ToString::to_string).unwrap_or_default(),
                url: failed_url.map(ToString::to_string).unwrap_or_default(),
            });
        }
    }
}
