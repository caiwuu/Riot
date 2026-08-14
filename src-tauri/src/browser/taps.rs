//! CDP 事件累积器。
//!
//! # 为什么需要它
//!
//! CDP 有两类消息:带 `id` 的是某次调用的响应（`Browser::cdp` 按 id 认领），
//! 不带 `id` 的是页面推来的**事件**（`Network.requestWillBeSent`、
//! `Page.javascriptDialogOpening` 之类）。事件走的是 [`super::access`] 里
//! 那条事件循环，而在这块累积器出现之前，那里除了 screencast 帧一律丢弃。
//!
//! 丢弃对"点一下、看一眼"的用法没问题，但抓包、拦截、对话框处理都建立在
//! "把过去一段时间的事件留住、供工具读回"之上 —— 网络流量在浏览器进程里，
//! 页面 JS 看不到完整的请求头/响应/时序，绕不过事件这条路。
//!
//! # 只累积订阅过的 domain
//!
//! `[取舍]` 不是所有事件都留，只留调用方明确 `subscribe` 过的 domain。
//!
//! 一个开着 screencast 的页面每秒推几十条 `Page.*`，无差别累积等于给每个
//! 标签页挂一个持续涨的内存泄漏。按需订阅把"要花内存记住的东西"收敛到
//! 工具真正会读的那几个 domain。

use std::collections::{HashMap, VecDeque};

use serde_json::Value;

/// 每个 domain 桶最多留多少条事件。
///
/// 一个中等复杂页面加载一次能产生上千条 `Network.*`（每个资源四五条）。
/// 留太多既吃内存，又把模型关心的"最近发生了什么"埋进历史；留太少会丢掉
/// 早期请求。2000 够覆盖一次典型页面加载，超出丢最旧的。
const MAX_PER_DOMAIN: usize = 2000;

/// 一个标签页上累积的 CDP 事件，按 domain 分桶。
///
/// domain 是 CDP method 里 `.` 前那段:`Network.requestWillBeSent` 的 domain
/// 是 `Network`。分桶让"读回网络事件"不必在一堆 `Log` 事件里筛。
#[derive(Default)]
pub struct EventTaps {
    buckets: HashMap<String, VecDeque<Value>>,
}

impl EventTaps {
    /// 订阅一个 domain:建一个空桶开始接事件。已订阅则不动。
    ///
    /// 返回 `true` 表示这是**新**订阅 —— 调用方据此决定要不要真的发一条
    /// `<Domain>.enable` 给浏览器。返回 `false` 说明早就在接了，重复 enable
    /// 虽然幂等，但省一次往返。
    pub fn subscribe(&mut self, domain: &str) -> bool {
        if self.buckets.contains_key(domain) {
            return false;
        }
        self.buckets.insert(domain.to_owned(), VecDeque::new());
        true
    }

    /// 一条 CDP 事件进来，按它的 domain 归桶。
    ///
    /// 没订阅那个 domain 就丢掉 —— 见模块头"只累积订阅过的"。桶满了丢最旧
    /// 的一条（环形），保证内存有上限而最近的事件总在。
    pub fn ingest(&mut self, payload: &Value) {
        let Some(method) = payload.get("method").and_then(Value::as_str) else {
            return;
        };
        let domain = method.split('.').next().unwrap_or(method);
        let Some(bucket) = self.buckets.get_mut(domain) else {
            return;
        };
        if bucket.len() >= MAX_PER_DOMAIN {
            bucket.pop_front();
        }
        bucket.push_back(payload.clone());
    }

    /// 读回某 domain 累积的全部事件（克隆，不清空）。
    ///
    /// 不清空是因为工具经常要多次读同一批（先看请求列表、再按 id 取某条的
    /// 细节）。要主动丢弃历史用 [`Self::clear`]。
    pub fn read(&self, domain: &str) -> Vec<Value> {
        self.buckets
            .get(domain)
            .map(|b| b.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// 清空某 domain 的历史，但保留订阅（继续接新事件）。
    ///
    /// 典型用法是"从现在开始重新观察":重放一个请求前先 clear，之后 read
    /// 到的就只有这次重放引发的流量。
    pub fn clear(&mut self, domain: &str) {
        if let Some(b) = self.buckets.get_mut(domain) {
            b.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(method: &str, tag: i64) -> Value {
        json!({ "method": method, "params": { "tag": tag } })
    }

    #[test]
    fn 只累积订阅过的_domain() {
        // 没订阅的 domain 一律丢 —— 否则 screencast 会把内存灌满。
        let mut taps = EventTaps::default();
        taps.subscribe("Network");

        taps.ingest(&ev("Network.requestWillBeSent", 1));
        taps.ingest(&ev("Log.entryAdded", 2)); // 没订阅 Log
        taps.ingest(&ev("Page.screencastFrame", 3)); // 没订阅 Page

        assert_eq!(taps.read("Network").len(), 1);
        assert!(taps.read("Log").is_empty(), "没订阅的 domain 不该留");
        assert!(taps.read("Page").is_empty());
    }

    #[test]
    fn 按_domain_分桶互不串() {
        let mut taps = EventTaps::default();
        taps.subscribe("Network");
        taps.subscribe("Log");

        taps.ingest(&ev("Network.responseReceived", 1));
        taps.ingest(&ev("Log.entryAdded", 2));

        assert_eq!(taps.read("Network").len(), 1);
        assert_eq!(taps.read("Log").len(), 1);
        assert_eq!(taps.read("Log")[0]["params"]["tag"], 2);
    }

    #[test]
    fn 重复订阅只有第一次算新() {
        let mut taps = EventTaps::default();
        assert!(taps.subscribe("Network"), "第一次是新订阅");
        assert!(!taps.subscribe("Network"), "第二次不该再发 enable");
    }

    #[test]
    fn 桶满丢最旧保留最近() {
        // 环形缓冲的意义:内存有上限，而最近的事件（模型最关心的）总在。
        let mut taps = EventTaps::default();
        taps.subscribe("Network");
        for i in 0..(MAX_PER_DOMAIN as i64 + 10) {
            taps.ingest(&ev("Network.requestWillBeSent", i));
        }
        let got = taps.read("Network");
        assert_eq!(got.len(), MAX_PER_DOMAIN, "不能超过上限");
        assert_eq!(got[0]["params"]["tag"], 10, "最旧的 10 条被挤掉了");
        assert_eq!(
            got[MAX_PER_DOMAIN - 1]["params"]["tag"],
            MAX_PER_DOMAIN as i64 + 9,
            "最新的一条要在"
        );
    }

    #[test]
    fn clear_丢历史但保留订阅() {
        let mut taps = EventTaps::default();
        taps.subscribe("Network");
        taps.ingest(&ev("Network.requestWillBeSent", 1));
        taps.clear("Network");
        assert!(taps.read("Network").is_empty(), "历史清掉");

        // 订阅还在:新事件照样进桶。
        taps.ingest(&ev("Network.requestWillBeSent", 2));
        assert_eq!(taps.read("Network").len(), 1, "clear 不该退订");
    }
}
