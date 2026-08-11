//! 抓取结果缓存：TTL + 总量上限 + LRU 淘汰。
//!
//! 模型读一个页面经常要读两三次（先看目录再看某一节），每次都重新抓一遍
//! 既慢又容易被站点限流。
//!
//! # 为什么不用现成的缓存库
//!
//! 时间是参数，不是内部状态。`moka` 之类的库自己持有时钟，而 workspace 的
//! clippy 禁掉了 `Instant::now`（见 clippy.toml）—— 缓存过期一旦依赖真实时间，
//! 黄金回放就没法复现"第二次调用命中缓存"这种行为。这里把 `now_ms` 提到
//! 参数上，缓存就退化成一个纯数据结构，过期和淘汰都能精确断言。

use std::collections::HashMap;
use std::sync::Mutex;

/// 缓存有效期。
///
/// 15 分钟是个折中：一次会话里的重复抓取几乎都落在这个窗口内，而用户改完
/// 文档想让模型重看时也不用等太久。
pub const DEFAULT_TTL_MS: u64 = 15 * 60 * 1000;

/// 缓存总量上限。按转换后的内容大小计，不是原始响应大小。
pub const DEFAULT_MAX_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedPage {
    /// 已经转成 Markdown 的正文。
    pub content: String,
    pub content_type: String,
    pub status: u16,
    pub status_text: String,
    /// 原始响应字节数。展示用，不参与淘汰计算。
    pub raw_bytes: u64,
}

struct Entry {
    page: CachedPage,
    size: u64,
    stored_at_ms: u64,
    /// 单调递增的使用序号。淘汰时选最小的那个。
    last_used: u64,
}

struct Inner {
    map: HashMap<String, Entry>,
    bytes: u64,
    tick: u64,
}

pub struct PageCache {
    inner: Mutex<Inner>,
    ttl_ms: u64,
    max_bytes: u64,
}

impl Default for PageCache {
    fn default() -> Self {
        Self::new(DEFAULT_TTL_MS, DEFAULT_MAX_BYTES)
    }
}

impl PageCache {
    pub fn new(ttl_ms: u64, max_bytes: u64) -> Self {
        Self {
            inner: Mutex::new(Inner {
                map: HashMap::new(),
                bytes: 0,
                tick: 0,
            }),
            ttl_ms,
            max_bytes,
        }
    }

    /// 取一条。过期的当作没有，并顺手删掉。
    pub fn get(&self, key: &str, now_ms: u64) -> Option<CachedPage> {
        let mut inner = self.lock();
        let expired = match inner.map.get(key) {
            None => return None,
            // saturating_sub：时钟回拨时当作"刚存进去"，而不是算出一个
            // 巨大的年龄把整个缓存判死。
            Some(e) => now_ms.saturating_sub(e.stored_at_ms) >= self.ttl_ms,
        };

        if expired {
            if let Some(e) = inner.map.remove(key) {
                inner.bytes -= e.size;
            }
            return None;
        }

        inner.tick += 1;
        let tick = inner.tick;
        let e = inner.map.get_mut(key)?;
        e.last_used = tick;
        Some(e.page.clone())
    }

    /// 存一条。超出总量上限就按 LRU 淘汰，直到装得下。
    pub fn put(&self, key: impl Into<String>, page: CachedPage, now_ms: u64) {
        let key = key.into();
        // 空响应也要占 1 字节，否则一堆零大小的条目能无限堆积。
        let size = (page.content.len() as u64).max(1);

        let mut inner = self.lock();

        // 单条就超上限的话，存了必然立刻被淘汰，白占一次写锁。
        if size > self.max_bytes {
            return;
        }

        if let Some(old) = inner.map.remove(&key) {
            inner.bytes -= old.size;
        }

        while inner.bytes + size > self.max_bytes {
            let Some(victim) = inner
                .map
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some(e) = inner.map.remove(&victim) {
                inner.bytes -= e.size;
            }
        }

        inner.tick += 1;
        let tick = inner.tick;
        inner.bytes += size;
        inner.map.insert(
            key,
            Entry {
                page,
                size,
                stored_at_ms: now_ms,
                last_used: tick,
            },
        );
    }

    pub fn clear(&self) {
        let mut inner = self.lock();
        inner.map.clear();
        inner.bytes = 0;
    }

    pub fn len(&self) -> usize {
        self.lock().map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 中毒的锁当作空缓存重建。
    ///
    /// 缓存是纯优化，为它 panic 掉整个会话不值得 —— 丢缓存的代价是多抓一次，
    /// panic 的代价是用户的对话没了。
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| {
            let mut g = e.into_inner();
            g.map.clear();
            g.bytes = 0;
            g
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn page(body: &str) -> CachedPage {
        CachedPage {
            content: body.to_owned(),
            content_type: "text/html".to_owned(),
            status: 200,
            status_text: "OK".to_owned(),
            raw_bytes: body.len() as u64,
        }
    }

    #[test]
    fn 存进去能取出来() {
        let c = PageCache::default();
        c.put("u1", page("hello"), 0);
        assert_eq!(c.get("u1", 0).expect("应当命中").content, "hello");
    }

    #[test]
    fn 到期后不再命中() {
        let c = PageCache::new(1000, DEFAULT_MAX_BYTES);
        c.put("u1", page("hello"), 0);

        assert!(c.get("u1", 999).is_some(), "还没到期");
        assert!(c.get("u1", 1000).is_none(), "正好到期就该失效");
        assert_eq!(c.len(), 0, "过期条目要顺手清掉，不能只是查不到");
    }

    #[test]
    fn 时钟回拨不会把缓存判死() {
        // 系统时间被 NTP 往回调过之后，如果用裸减法算年龄会得到一个
        // 极大的无符号数，整个缓存瞬间全部失效
        let c = PageCache::new(1000, DEFAULT_MAX_BYTES);
        c.put("u1", page("hello"), 10_000);
        assert!(c.get("u1", 5_000).is_some(), "时间回拨时应当仍然命中");
    }

    #[test]
    fn 超出上限时淘汰最久未用的() {
        let c = PageCache::new(DEFAULT_TTL_MS, 20);
        c.put("a", page("aaaaa"), 0); // 5
        c.put("b", page("bbbbb"), 0); // 10
        c.put("c", page("ccccc"), 0); // 15

        // 摸一下 a，让 b 成为最久未用
        assert!(c.get("a", 0).is_some());

        c.put("d", page("ddddd"), 0); // 20，还装得下
        c.put("e", page("eeeee"), 0); // 超了，淘汰 b

        assert!(c.get("b", 0).is_none(), "最久未用的 b 应当被淘汰");
        assert!(c.get("a", 0).is_some(), "刚用过的 a 应当留下");
        assert!(c.get("e", 0).is_some());
    }

    #[test]
    fn 覆盖同一个键不重复计入总量() {
        // 算漏了的话，反复抓同一个 URL 会把缓存"撑满"，然后开始
        // 淘汰其它本该留着的页面
        let c = PageCache::new(DEFAULT_TTL_MS, 20);
        for _ in 0..10 {
            c.put("a", page("aaaaa"), 0);
        }
        c.put("b", page("bbbbb"), 0);
        assert!(c.get("a", 0).is_some());
        assert!(c.get("b", 0).is_some());
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn 单条超过上限直接不存() {
        let c = PageCache::new(DEFAULT_TTL_MS, 4);
        c.put("big", page("aaaaaaaaaa"), 0);
        assert_eq!(c.len(), 0);
        assert!(c.get("big", 0).is_none());
    }

    #[test]
    fn 空内容也占位不会无限堆积() {
        let c = PageCache::new(DEFAULT_TTL_MS, 3);
        for i in 0..10 {
            c.put(format!("k{i}"), page(""), 0);
        }
        assert!(c.len() <= 3, "零大小条目也必须参与淘汰，实际 {}", c.len());
    }
}
