//! 具体工具。
//!
//! 共享的三块基础设施先看:
//!
//! - [`text`] —— 编码与换行。读-改-写链路上信息保不住就拒绝，不猜。
//! - [`path`] —— 路径解析与围栏复查。执行时再查一遍防 TOCTOU。
//! - [`precondition`] —— 先读后写协议。

pub mod bash;
pub mod edit;
pub mod glob;
pub mod grep;
#[cfg(any(test, feature = "testing"))]
pub mod fakeproc;
#[cfg(any(test, feature = "testing"))]
pub mod memfs;
pub mod path;
pub mod precondition;
pub mod read;
pub mod text;
pub mod web;
pub mod write;

pub use bash::Bash;
pub use edit::Edit;
pub use glob::Glob;
pub use grep::Grep;
pub use read::Read;
pub use web::{WebFetch, WebSearch};
pub use write::Write;

use std::sync::Arc;

use riot_protocol::tool::Tool;

/// 内建工具集。
///
/// 顺序就是发给模型的顺序，而这个顺序进 prompt cache 的前缀。改动它会让
/// 所有活跃会话的缓存失效 —— 追加放末尾，不要往中间插。
///
/// Read 排在最前面是因为 Write 和 Edit 都要求"先读过"。工具描述在上下文里
/// 的先后顺序会影响模型的调用习惯。
pub fn builtin() -> Vec<Arc<dyn Tool>> {
    // 两个联网工具共用一份抓取缓存：WebSearch 里出现过的链接，模型接着
    // 用 WebFetch 去读时不该再发一次请求。
    let page_cache = Arc::new(web::PageCache::default());

    vec![
        Arc::new(Read),
        Arc::new(Edit),
        Arc::new(Write),
        Arc::new(Bash),
        Arc::new(Grep),
        Arc::new(Glob),
        Arc::new(WebSearch),
        Arc::new(WebFetch::new(page_cache)),
    ]
}

#[cfg(test)]
mod bash_tests;
#[cfg(test)]
mod glob_tests;
#[cfg(test)]
mod grep_tests;
#[cfg(test)]
mod tests;
