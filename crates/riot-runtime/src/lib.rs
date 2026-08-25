//! 注入 trait 的真实实现。
//!
//! `riot-core` 和 `riot-tools` 只依赖 `riot-protocol` 里的 trait，
//! 不知道文件系统和进程长什么样。这个 crate 提供那些 trait 的真身。
//!
//! # 为什么单独一个 crate
//!
//! workspace 的 clippy 禁用了 `Command::new`、`Instant::now`、`fs::*` 这一类
//! 破坏确定性的 API（见 `clippy.toml`）。真实实现必然要用它们，所以每个模块
//! 顶部都带 `#![allow(clippy::disallowed_methods)]`。
//!
//! 集中在一个 crate 里，这些豁免就有了明确边界：**只有这里能碰真实的 OS**。
//! 散在各处的话，"这个 allow 是必要的还是偷懒"就只能一个个看了。
//!
//! 对应地，这个 crate 不参与黄金回放 —— 回放用的是 `riot-tools` 里的替身。
//! 它的测试全是真跑：起真的进程、写真的文件、等真的时间。

pub mod fs;
pub mod proc;
pub mod sandbox;
// 跨平台：Windows spawn 的命令行 / 环境块拼接（纯字符串逻辑，见文件头）。
// 同 sandbox_labels，只有 Windows 后端用，但不门控平台好让 mac 测。
pub mod sandbox_cmdline;
// 跨平台：Low 标签清单的孤儿回收逻辑，任何平台都能测（见文件头）。
// 当前只有 Windows 后端会用它，但纯逻辑不门控平台，好让 mac 也跑测试。
pub mod sandbox_labels;
#[cfg(target_os = "macos")]
pub mod sandbox_macos;
#[cfg(windows)]
pub mod sandbox_win;
pub mod web;

pub use fs::{MemoryFileState, SystemFs};
pub use proc::SystemProcessRunner;
pub use sandbox::{
    ActiveSandbox, SandboxPolicy, SandboxSetup, SandboxedRunner, recover_orphan_labels,
};
pub use web::SystemWebClient;
