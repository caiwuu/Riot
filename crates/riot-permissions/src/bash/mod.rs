//! Bash 命令分析。
//!
//! 见 ARCHITECTURE.md §9.3、§9.4

pub mod ast;
pub mod decide;
pub mod readonly;

pub use ast::{Analysis, ComplexReason, Complexity, SubCommand, analyze};
pub use decide::decide;
pub use readonly::is_read_only;

#[cfg(test)]
mod tests;
