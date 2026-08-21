//! 环境快照：agent 对宿主环境（终端面板、内置浏览器）的感知。
//!
//! 设计与约束见 docs/ENV_DESIGN.md。要点：
//!
//! - **拉，不推**。内核在轮首经反向 RPC（`hostcall` 的 `env.snapshot`）采样，
//!   没有推送通道。采样天然合并抖动，快照随消息进 transcript，回放不需要
//!   特殊处理。
//! - **同意边界结构性继承**。快照的可见集 = `term_access` 的可见集：自己起的
//!   加用户共享的。未共享的终端在这里**只有数量** —— 标题和内容在类型上
//!   就不存在，不是靠组装方自觉。
//!
//! 这些类型只在内核和宿主两个 Rust 进程之间走，前端不消费 —— 不挂
//! JsonSchema、不进 schema 根，和 [`crate::hostcall`] 同一条规矩。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::terminal::TerminalInfo;

/// 一次环境采样的结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvSnapshot {
    /// 模型自己起的服务。
    pub mine: Vec<TerminalInfo>,
    /// 用户在面板上共享给模型的终端。共享只给读 —— 停不掉。
    pub shared: Vec<TerminalInfo>,
    /// 模型看不到的其余终端有几个。只有存在性：让模型能开口请用户共享，
    /// 又不泄露任何内容。
    pub unshared_count: u32,
    /// 面板浏览器的活动页。`None` = 浏览器没起来（或一页都没开）。
    pub browser: Option<BrowserGlance>,
    /// 自上次采样以来的新告警。去重在宿主做（按会话 × 终端记摘录哈希），
    /// 这里收到的每一条都是该告诉模型的。
    pub alerts: Vec<EnvAlert>,
}

impl EnvSnapshot {
    /// 完全没东西可说：没有可见终端、没有别的终端、没有浏览器、没有告警。
    /// 会话首轮遇到这种快照直接跳过注入 —— 对着空房间描述空房间是噪音。
    pub fn is_quiet(&self) -> bool {
        self.mine.is_empty()
            && self.shared.is_empty()
            && self.unshared_count == 0
            && self.browser.is_none()
            && self.alerts.is_empty()
    }
}

/// 浏览器面板的一瞥：活动页在哪、开了几页。细节（DOM、截图）按需走工具。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrowserGlance {
    pub url: String,
    /// 页面标题。加载完之前可能是空的。
    pub title: String,
    pub tabs: u32,
}

/// 可见终端尾部命中告警模式产生的一条摘录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvAlert {
    pub terminal_id: u32,
    pub title: String,
    /// 命中行附近的摘录。宿主裁好（≤3 行 / ≤240 字符），这里不再截。
    pub excerpt: String,
}

/// 环境探针。宿主实现（真实状态在那边），内核经反向 RPC 代理。
#[async_trait]
pub trait EnvProbe: Send + Sync {
    /// 采一次样。`None` = 拿不到（宿主没装配 / 传输断了）—— 调用方按
    /// "这一轮没有感知"处理，不报错、不重试。感知是锦上添花，
    /// 拿不到不该挡住用户发消息。
    async fn sample(&self) -> Option<EnvSnapshot>;
}

/// 没有探针的占位实现。
///
/// `[约束]` 默认必须是它 —— 宿主忘了装配的表现是"没有感知"，而不是某个
/// 尽力而为的兜底。和 [`crate::terminal::NoTerminal`] 同一个规矩。
pub struct NoEnvProbe;

#[async_trait]
impl EnvProbe for NoEnvProbe {
    async fn sample(&self) -> Option<EnvSnapshot> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 空快照才算安静() {
        let quiet = EnvSnapshot {
            mine: vec![],
            shared: vec![],
            unshared_count: 0,
            browser: None,
            alerts: vec![],
        };
        assert!(quiet.is_quiet());

        // 有未共享终端就不算安静：模型该知道"可以请用户共享"。
        let counted = EnvSnapshot {
            unshared_count: 2,
            ..quiet.clone()
        };
        assert!(!counted.is_quiet());
    }

    #[test]
    fn 快照跨进程往返不丢信息() {
        let snap = EnvSnapshot {
            mine: vec![TerminalInfo {
                id: 3,
                title: "dev server".into(),
                command: Some("pnpm dev".into()),
                running: true,
                shared: false,
            }],
            shared: vec![],
            unshared_count: 1,
            browser: Some(BrowserGlance {
                url: "http://localhost:5173".into(),
                title: "Riot".into(),
                tabs: 2,
            }),
            alerts: vec![EnvAlert {
                terminal_id: 3,
                title: "dev server".into(),
                excerpt: "Error: EADDRINUSE".into(),
            }],
        };
        let line = serde_json::to_string(&snap).expect("序列化");
        let back: EnvSnapshot = serde_json::from_str(&line).expect("反序列化");
        assert_eq!(back, snap);
    }
}
