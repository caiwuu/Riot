//! 工具层：注册表、并发分批、执行管线。
//!
//! 这一层最容易写错的三个地方：
//!
//! 1. **分批**（[`partition`]）—— 重排会破坏模型隐含的工具依赖
//! 2. **保序**（[`scheduler`]）—— 结果顺序必须可重放，否则黄金回放就废了
//! 3. **级联**（[`scheduler`]）—— 级联范围搞错会误杀无关工具

pub mod partition;
pub mod redact;
pub mod registry;
pub mod scheduler;
pub mod tools;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use partition::{Batch, DEFAULT_MAX_CONCURRENCY, partition};
pub use registry::{Registry, RegistryError};
pub use scheduler::Scheduler;
