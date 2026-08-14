//! CDP 的回向通道。
//!
//! 发出去的部分在 [`crate::dispatch`]，这里只管收:CEF 把 DevTools 的
//! 响应和事件交给观察者，我们原样转成 [`Event::Cdp`] 发回主应用。
//!
//! # 为什么只实现 `on_dev_tools_message`
//!
//! CEF 提供三个回调:`on_dev_tools_message`(原始报文)、
//! `on_dev_tools_method_result`(方法结果)、`on_dev_tools_event`(事件)。
//! 后两个是前者的拆分版本 —— 前者返回非零表示"我处理了"，CEF 就不再调
//! 那两个。
//!
//! 只走原始报文这一条，理由是**上层要的正是 CDP 的原始线格式**。拆开再
//! 拼回去等于把 Chromium 的协议抄一遍到 Rust 里，而那份东西每个版本都在动。

use cef::rc::Rc;
use cef::*;

use riot_protocol::browser::{Event, TabId};

// 一个标签页的 CDP 回向通道。
//
// `[约束]` 每个标签页一个观察者，而且要记住自己的号。CEF 回调里给的
// `browser` 参数认不出标签页（号是我们自己发的），而报文不标明来源的话，
// 主应用没法把响应派给正确的等待者 —— 多标签并发时会拿到别的页面的结果。
//
// 文档注释只能写在宏外面:宏的匹配规则不接受 struct 前面的 `#[doc]`，
// 报出来是 `no rules expected #`，和注释本身看起来毫无关系。
cef::wrap_dev_tools_message_observer! {
    pub struct CdpObserver {
        tab: TabId,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_message(
            &self,
            _browser: Option<&mut Browser>,
            message: Option<&[u8]>,
        ) -> ::std::os::raw::c_int {
            let Some(bytes) = message else { return 1 };

            // CDP 的报文本来就是 JSON。解析一遍再序列化一遍是多余的，
            // 但换来的是"坏报文在这里就被发现"，而不是让主应用那边的
            // NDJSON 流吞下一段非法内容。
            match serde_json::from_slice::<serde_json::Value>(bytes) {
                Ok(payload) => crate::wire::emit(&Event::Cdp { tab: self.tab, payload }),
                Err(e) => crate::wire::emit(&Event::Error {
                    message: format!("CDP 报文不是合法 JSON: {e}"),
                }),
            }

            // 1 = 已处理。CEF 不会再调 on_dev_tools_method_result /
            // on_dev_tools_event，避免同一条消息报两遍。
            1
        }

        fn on_dev_tools_agent_detached(&self, _browser: Option<&mut Browser>) {
            // 主动关闭时 renderer 必然消失、agent 必然断开，那是正常收尾，
            // 报出来只会让人以为退出出了问题。
            if crate::dispatch::is_shutting_down() {
                return;
            }
            // 非预期的断开要报:之后发出去的 CDP 命令会静默失败，
            // 上层会一直等一个永远不来的响应。
            crate::wire::emit(&Event::Error {
                message: "DevTools agent 已断开，CDP 暂时不可用".into(),
            });
        }
    }
}
