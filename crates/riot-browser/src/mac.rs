//! CEF 要求的 `NSApplication` 子类。
//!
//! `[约束]` 必须在任何 CEF 调用**之前**建好。
//!
//! CEF 在 macOS 上要求 `NSApp` 实现 `CefAppProtocol` —— 它需要知道当前
//! 是否正处在 `sendEvent:` 的调用栈里，否则事件分发的重入判断会错。少了
//! 这一步的表现不是"事件不好使"，而是初始化阶段就死在
//! `icudtl.dat not found in bundle`:`[NSApplication sharedApplication]`
//! 是 Cocoa 侧 main bundle 就位的时机，CEF 的资源定位挂在它后面。
//!
//! 这个错误信息完全没指向真正的原因，实测走了一圈才定位到。

use std::cell::Cell;

use cef::application_mac::{CefAppProtocol, CrAppControlProtocol, CrAppProtocol};
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObjectProtocol};
use objc2::{ClassType, DefinedClass, define_class, extern_methods, msg_send};
use objc2_app_kit::{NSApplication, NSEvent};

#[derive(Default)]
pub struct RiotApplicationIvars {
    handling_send_event: Cell<Bool>,
}

define_class!(
    /// 实现了 CEF 所需协议的 `NSApplication` 子类。
    #[unsafe(super(NSApplication))]
    #[ivars = RiotApplicationIvars]
    pub struct RiotApplication;

    impl RiotApplication {
        #[unsafe(method(sendEvent:))]
        unsafe fn send_event(&self, event: &NSEvent) {
            // 记录"正在分发事件"。CEF 靠这个标志判断重入 ——
            // 嵌套的 sendEvent: 不能把标志提前清掉。
            let was_sending = self.ivars().handling_send_event.get().as_bool();
            if !was_sending {
                self.ivars().handling_send_event.set(Bool::YES);
            }
            let _: () = unsafe { msg_send![super(self), sendEvent: event] };
            if !was_sending {
                self.ivars().handling_send_event.set(Bool::NO);
            }
        }
    }

    unsafe impl CrAppControlProtocol for RiotApplication {
        #[unsafe(method(setHandlingSendEvent:))]
        unsafe fn _set_handling_send_event(&self, handling: Bool) {
            self.ivars().handling_send_event.set(handling);
        }
    }

    unsafe impl CrAppProtocol for RiotApplication {
        #[unsafe(method(isHandlingSendEvent))]
        unsafe fn _is_handling_send_event(&self) -> Bool {
            self.ivars().handling_send_event.get()
        }
    }

    unsafe impl CefAppProtocol for RiotApplication {}
);

impl RiotApplication {
    extern_methods!(
        #[unsafe(method(sharedApplication))]
        fn shared_application() -> Retained<Self>;
    );
}

/// 建立 `NSApp` 单例。
///
/// `[约束]` 这之前不能有任何人碰过 `NSApp` —— `sharedApplication` 只认第一次
/// 调用时的类，之后再调拿到的还是那个实例。断言守着这一点，因为一旦顺序错了，
/// 后果是 CEF 在完全不相干的地方报错。
pub fn setup_application() {
    let _ = RiotApplication::shared_application();
    let mtm = objc2::MainThreadMarker::new().expect("必须在主线程上初始化 NSApp");
    assert!(
        objc2_app_kit::NSApp(mtm).isKindOfClass(RiotApplication::class()),
        "NSApp 不是 RiotApplication —— 说明在此之前已经有人初始化过 NSApp"
    );
}
