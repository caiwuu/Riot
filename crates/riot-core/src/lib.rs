//! Riot agent 内核。
//!
//! 这个 crate 不依赖任何 UI 或宿主代码 —— 它必须能在没有窗口的情况下
//! 跑完整测试。见 ARCHITECTURE.md §3.1
//!
//! # 非确定性禁令
//!
//! 本 crate 里不允许直接调 `std::fs`、`std::process`、`SystemTime::now()`、
//! 随机数。全部走 `riot_protocol` 里注入的 trait。这条由 `clippy.toml`
//! 的 `disallowed-methods` 强制，需要豁免时必须显式 `#[allow]` 并注明理由。
//!
//! 原因：黄金回放测试依赖行为完全确定。这条一旦破了，测试会开始随机失败，
//! 然后所有人都会忽略它 —— 那时整层防线就废了。
//! 见 docs/VERIFICATION.md §4.2

#![deny(clippy::disallowed_methods)]
#![deny(clippy::disallowed_types)]

pub mod agent_loop;
pub mod compactor;
pub mod guard;
pub mod invariants;
pub mod state;
pub mod summarize;
// 测试替身只进测试构建。下游要用的话在 dev-dependencies 里开
// `features = ["testing"]`（riot-tools 的同名 feature 是同一个模式）。
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod turn;

pub use agent_loop::run_agent;
pub use compactor::{ClearOldResults, Layered};
pub use state::{AgentDeps, AgentState, Transition};
