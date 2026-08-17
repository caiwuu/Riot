//! 具体工具。
//!
//! 共享的三块基础设施先看:
//!
//! - [`text`] —— 编码与换行。读-改-写链路上信息保不住就拒绝，不猜。
//! - [`path`] —— 路径解析与围栏复查。执行时再查一遍防 TOCTOU。
//! - [`precondition`] —— 先读后写协议。

pub mod ask;
pub mod browser;
pub mod diagnostics;
pub mod bash;
pub mod edit;
pub mod glob;
pub mod grep;
#[cfg(any(test, feature = "testing"))]
pub mod fakeproc;
#[cfg(any(test, feature = "testing"))]
pub mod memfs;
pub mod path;
pub mod search;
pub mod pentest;
pub mod plan;
pub mod precondition;
pub mod read;
pub mod shrink;
pub mod skill;
pub mod todo;
pub mod tool_search;
pub mod text;
pub mod web;
pub mod terminal;
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

    let mut tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(Read),
        Arc::new(Edit),
        Arc::new(Write),
        Arc::new(Bash),
        Arc::new(Grep),
        Arc::new(Glob),
        Arc::new(WebSearch),
        Arc::new(WebFetch::new(page_cache)),
    ];
    // 浏览器工具单独一组:它们依赖宿主注入 BrowserAccess，没注入时
    // 会明确说"用不了"，而不是悄悄换个行为。
    tools.extend(browser::tools());
    // 追加在末尾（prompt cache 前缀稳定性，见函数注释）。
    tools.push(Arc::new(todo::TodoWrite));
    // 长期服务的读与停。起服务在 Bash 的 background 参数上 —— 那是同一件事
    // 的入口，不该再多一个工具。
    tools.push(Arc::new(terminal::TerminalOutput));
    tools.push(Arc::new(terminal::TerminalKill));
    // 让模型能把决定交回给用户。放末尾同上：追加不动前缀。
    tools.push(Arc::new(ask::AskUserQuestion));
    tools.push(Arc::new(diagnostics::Diagnostics));
    tools
}

#[cfg(test)]
mod bash_tests;
#[cfg(test)]
mod glob_tests;
#[cfg(test)]
mod grep_tests;
#[cfg(test)]
mod tests;
