//! 内核进程的启动、通信与关闭。

pub mod client;
pub mod coalesce;
pub mod supervisor;

pub use client::{HostNotice, KernelClient, locate_kernel};
pub use supervisor::{Kernel, KernelError, KernelHandle, RestartPolicy};
