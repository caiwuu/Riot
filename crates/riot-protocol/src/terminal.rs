//! 模型对**用户那个终端面板**的访问。
//!
//! # 为什么长期服务要跑在这里，而不是子进程执行器里
//!
//! Bash 工具跑完会无条件清掉整个进程组（见 `riot-runtime` 的 proc）。
//! 对 `cargo test` 这种跑完就结束的命令，这是对的 —— 不这么做会漏下
//! 一堆孤儿。但 dev server 不是那种命令：它就该一直活着。
//!
//! 模型于是只剩两条路，两条都糟：普通后台（`&`）跟着被杀；`setsid`
//! 脱离进程组则彻底失控 —— 用户看不见、停不掉，模型自己也读不到它的
//! 输出。那就是个幽灵服务。
//!
//! 把它放进终端面板，三个问题一起没了：用户看得见、能 Ctrl-C、能关；
//! 模型能读输出、能停。它活在一个**有人管**的地方。
//!
//! # 边界：模型只能碰自己起的那些
//!
//! `[约束]` 实现必须拒绝对用户自己开的 shell 做任何操作。那里面有用户
//! 敲过的密码、私有仓库地址、和这次任务无关的一切。模型想要用户终端里
//! 的内容，得由用户主动选中发过来。

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// 一个终端的概况。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TerminalInfo {
    pub id: u32,
    /// 给人看的名字。模型起的用它自己写的说明。
    pub title: String,
    /// 起它的那条命令。用户自己开的 shell 没有这个，模型也碰不了。
    pub command: Option<String>,
    /// 进程还活着。退出之后终端条目还留着，输出可以继续读。
    pub running: bool,
    /// 这是用户共享给模型看的终端（不是模型自己起的）。共享只给读：
    /// 停它的请求会被宿主拒绝。`default` 兼容旧数据。
    #[serde(default)]
    pub shared: bool,
}

/// 终端能力不可用（宿主没装配，或这个终端不归模型管）。
#[derive(Debug, Clone)]
pub struct TerminalUnavailable(pub String);

impl std::fmt::Display for TerminalUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[async_trait]
pub trait TerminalAccess: Send + Sync {
    /// 在终端面板里起一条长期命令，立刻返回终端 id（**不等它结束**）。
    async fn spawn(&self, command: &str, title: &str) -> Result<u32, TerminalUnavailable>;

    /// 读某个终端最近 `lines` 行输出。已经去掉 ANSI 转义 —— 模型读的是
    /// 文本，不是给屏幕看的控制序列。
    async fn read(&self, id: u32, lines: usize) -> Result<String, TerminalUnavailable>;

    /// 停掉一个终端。幂等：已经退了的再停一次不是错误。
    async fn kill(&self, id: u32) -> Result<(), TerminalUnavailable>;

    /// 模型起过的终端。用户自己开的不在里面。
    async fn list(&self) -> Vec<TerminalInfo>;
}

/// 没有终端的占位实现。
///
/// `[约束]` 默认必须是它，而不是某个"尽力而为"的兜底 —— 宿主忘了装配的
/// 表现应该是工具明确说"终端用不了"，而不是悄悄退回那条会把服务杀掉的
/// 老路。和 [`crate::browser::NoBrowser`] 同一个规矩。
pub struct NoTerminal;

#[async_trait]
impl TerminalAccess for NoTerminal {
    async fn spawn(&self, _command: &str, _title: &str) -> Result<u32, TerminalUnavailable> {
        Err(unavailable())
    }
    async fn read(&self, _id: u32, _lines: usize) -> Result<String, TerminalUnavailable> {
        Err(unavailable())
    }
    async fn kill(&self, _id: u32) -> Result<(), TerminalUnavailable> {
        Err(unavailable())
    }
    async fn list(&self) -> Vec<TerminalInfo> {
        Vec::new()
    }
}

fn unavailable() -> TerminalUnavailable {
    TerminalUnavailable("这个环境没有终端面板，长期服务起不了。".to_owned())
}
