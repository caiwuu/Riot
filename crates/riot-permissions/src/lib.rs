//! 权限系统。
//!
//! 三个部分，按"写错了后果最严重"排序：
//!
//! 1. **决策链**（[`chain`]）—— 七步优先级。任意两步交换，绝大多数用例
//!    都还是绿的，所以每一步的相对位置都有专门的测试。
//! 2. **安全检查**（[`safety`]）—— 对 bypass 模式免疫的那一层。
//! 3. **路径围栏**（[`fence`]）—— symlink 逃逸与路径别名。
//!
//! 规则匹配（[`rules`]）是前三者的基础设施。
//!
//! 见 ARCHITECTURE.md §9

pub mod bash;
pub mod chain;
pub mod fence;
pub mod rules;
pub mod safety;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use chain::decide;
pub use fence::FenceViolation;
pub use rules::{MatchMode, RuleSet, matches_pattern};
pub use safety::SafetyFinding;
