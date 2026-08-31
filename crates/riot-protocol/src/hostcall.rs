//! 反向 RPC:内核 → 宿主的调用契约。
//!
//! 终端面板(PTY)和面板浏览器(Chromium)是**宿主能力** —— 用户看得见、
//! 管得着的东西必须活在宿主进程里(ARCHITECTURE.md §2.4)。内核里的
//! agent 工具要用它们,就得反过来调宿主。
//!
//! # 传输
//!
//! 走同一条 stdio 管道:内核往 stdout 写 `{"jsonrpc","id","method","params"}`
//! (和宿主发请求同形,方向相反),宿主处理后把 `{"jsonrpc","id","result"}`
//! 写回内核 stdin。两个方向的 id 空间各自独立 —— 配对表各自维护,靠
//! "有没有 method 字段"区分请求与应答,不会串。
//!
//! # 为什么不进 generated.ts
//!
//! 这套类型只在两个 Rust 进程之间走,前端不消费 —— 所以不挂 JsonSchema、
//! 不进 schema 根。Rust↔Rust 的一致性由"两边 depend 同一个 crate"保证。

use serde::{Deserialize, Serialize};

use crate::browser::{Action, InterceptOp, Nav, NetQuery, TabId, Target, WaitCondition};
use crate::id::SessionId;
use crate::terminal::TerminalInfo;

/// 内核发给宿主的请求。
///
/// 都带 `session_id`:浏览器实例按会话隔离(profile 目录就是会话 id),
/// 终端 spawn 的 cwd 是会话的项目根 —— 宿主按它查登记表。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum HostRequest {
    /// 在终端面板起一条长期命令(dev server 之类),立刻返回终端号。
    #[serde(rename = "terminal.spawn")]
    TerminalSpawn {
        session_id: SessionId,
        command: String,
        title: String,
    },
    /// 读某个终端最近 `lines` 行输出(已去 ANSI)。
    #[serde(rename = "terminal.read")]
    TerminalRead {
        session_id: SessionId,
        id: u32,
        lines: usize,
    },
    /// 停掉一个终端。幂等。
    #[serde(rename = "terminal.kill")]
    TerminalKill { session_id: SessionId, id: u32 },
    /// 模型起过的终端清单。
    #[serde(rename = "terminal.list")]
    TerminalList { session_id: SessionId },

    /// 浏览器操作。方法多且形状一致(作用于会话的面板浏览器、回一段文本),
    /// 收成一个变体 + [`BrowserCall`] 枚举,不然这里要摊十八个变体。
    #[serde(rename = "browser.call")]
    BrowserCall {
        session_id: SessionId,
        call: BrowserCall,
    },

    /// 环境快照:终端与浏览器现状 + 新告警。内核在轮首采样,差分注入,
    /// 见 docs/ENV_DESIGN.md §3。
    #[serde(rename = "env.snapshot")]
    EnvSnapshot { session_id: SessionId },
}

/// [`crate::browser::BrowserAccess`] 各方法的序列化形状,一一对应。
/// (tag 叫 `kind` 而不是 `op`:Intercept 变体有个同名字段 `op`,serde
/// 的 internal tag 不允许和变体字段撞名。)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserCall {
    Navigate {
        url: String,
    },
    Screenshot {
        deterministic: bool,
    },
    Snapshot,
    SnapshotMarked,
    Console,
    CurrentUrl,
    Click {
        target: Target,
    },
    TypeText {
        target: Target,
        text: String,
        submit: bool,
    },
    PressKey {
        key: String,
    },
    Scroll {
        delta_y: f64,
    },
    WaitFor {
        cond: WaitCondition,
        timeout_ms: u64,
    },
    Act {
        action: Action,
    },
    Browse {
        nav: Nav,
    },
    Evaluate {
        expr: String,
    },
    SourceOf {
        target: Target,
    },
    SnapshotTab {
        tab: TabId,
    },
    Upload {
        target: Target,
        paths: Vec<String>,
    },
    Cookies,
    Network {
        query: NetQuery,
    },
    Replay {
        url: String,
        method: String,
        headers: serde_json::Value,
        body: Option<String>,
    },
    Intercept {
        op: InterceptOp,
    },
}

/// 宿主对 [`HostRequest`] 的应答。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", content = "data", rename_all = "snake_case")]
pub enum HostResponse {
    /// terminal.spawn:新终端的号。
    TerminalId { id: u32 },
    /// terminal.list。
    Terminals { items: Vec<TerminalInfo> },
    /// 文本结果(terminal.read 和大多数浏览器方法)。
    Text { text: String },
    /// browser.console。
    Lines { lines: Vec<String> },
    /// browser.snapshot_marked:编号清单 + 带框视口截图(base64 JPEG)。
    Marked { listing: String, screenshot: String },
    /// env.snapshot。
    Env { snapshot: crate::env::EnvSnapshot },
    /// 无返回数据的成功(terminal.kill、browser.navigate)。
    Ok,
    /// 失败。`kind` 必须区分开 —— 工具层对两种失败给模型的指引相反:
    /// 不可用 → 别重试、换别的工具;目标失效 → 重拍快照再来。
    Error {
        kind: HostCallErrorKind,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostCallErrorKind {
    /// 能力整个不可用(没装配/没打包/进程没起来)。
    Unavailable,
    /// 目标指不到(编号过期、元素消失、终端号不存在)。
    Target,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 请求走点分方法名_便于宿主按前缀分发() {
        let req = HostRequest::TerminalList {
            session_id: SessionId::from_raw("s1"),
        };
        let v = serde_json::to_value(&req).expect("序列化");
        assert_eq!(v["method"], "terminal.list");
        assert_eq!(v["params"]["session_id"], "s1");
    }

    #[test]
    fn 浏览器调用带完整目标参数() {
        let req = HostRequest::BrowserCall {
            session_id: SessionId::from_raw("s1"),
            call: BrowserCall::Click {
                target: Target::Ref(3),
            },
        };
        let line = serde_json::to_string(&req).expect("序列化");
        let back: HostRequest = serde_json::from_str(&line).expect("往返");
        assert_eq!(back, req, "跨进程往返不能丢信息");
    }

    #[test]
    fn 错误应答分得清不可用和目标失效() {
        // 工具层对这两种失败的指引相反,kind 混了会把模型引进死胡同。
        let e = HostResponse::Error {
            kind: HostCallErrorKind::Target,
            message: "编号 [3] 不在最近一次快照里".into(),
        };
        let v = serde_json::to_value(&e).expect("序列化");
        assert_eq!(v["data"]["kind"], "target");
    }
}
