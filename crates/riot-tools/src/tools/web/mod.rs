//! 联网工具：[`WebSearch`] 搜，[`WebFetch`] 读。
//!
//! # 分工
//!
//! 搜索只返回标题、链接和摘要；要读正文得再调一次 WebFetch。这个拆分
//! 不是为了让模型多干一步，而是因为**搜索结果可以指向任何域名** ——
//! 自动抓取它们等于绕开 WebFetch 的域名权限。详见 [`search`] 顶部。
//!
//! # 分层
//!
//! ```text
//! fetch.rs / search.rs   工具外壳：schema、权限、给模型的措辞
//! pipeline.rs            抓取 → 转换 → 蒸馏
//! url.rs                 准入与重定向策略   ← 安全边界，纯函数
//! markdown.rs            字符集解码 + HTML→Markdown
//! cache.rs               TTL + LRU
//! preapproved.rs         免确认域名白名单
//! date.rs                写进提示词的当前年月
//! ```
//!
//! 除了 `pipeline` 要一个 [`riot_protocol::tool::ToolContext`]，
//! 下面四层全是纯函数，可以脱离网络测。真正的 HTTP 在
//! [`riot_protocol::web::WebAccess`] 后面，由宿主装配。

pub mod cache;
pub mod consent;
pub mod date;
pub mod fetch;
pub mod markdown;
pub mod pipeline;
pub mod preapproved;
pub mod search;
pub mod url;

pub use cache::PageCache;
pub use date::year_month;
pub use fetch::{WEB_FETCH, WebFetch};
pub use search::{WEB_SEARCH, WebSearch};

#[cfg(test)]
mod tests;
