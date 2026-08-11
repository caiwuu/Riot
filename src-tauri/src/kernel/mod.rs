//! 内核进程的启动、通信与关闭。

pub mod coalesce;
pub mod supervisor;

pub use supervisor::{Kernel, KernelError, RestartPolicy};
