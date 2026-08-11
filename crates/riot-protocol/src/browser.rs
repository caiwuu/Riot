//! 主应用 ↔ 浏览器进程（`riot-browser`）的线格式。
//!
//! stdin/stdout 上跑 NDJSON:一行一条消息。选它而不是长度前缀的二进制，
//! 理由是可读 —— 出问题时能直接把 stdout 重定向到文件读。
//!
//! `[约束]` 这套类型放在协议层，两个进程各自 depend 同一份。
//!
//! 它们跨进程、分别编译，没有任何编译期检查能兜住不一致 —— 改了字段名而
//! 只更新一边，表现是"命令发过去没反应"，不报错也不崩。共享一份定义是
//! 唯一能让编译器管这件事的办法。
//!
//! `[约束]` 浏览器进程的 stdout **只能**用来传这些消息。CEF 和 Chromium
//! 自己会往 stderr 写大量日志，任何一行漏进 stdout 都会把 NDJSON 流冲坏，
//! 而表现是主应用这边"某条消息解析失败"，完全指不回真正的源头。
//!
//! # 帧不走这条通道
//!
//! 1280×800 的 BGRA 一帧是 4MB，按 base64 塞进 JSON 是 5.5MB，30fps 就是
//! 165MB/s —— 光是序列化就吃满一个核。帧走单独的共享内存，这里只传
//! "第几帧、多大"。见 [`Event::Frame`]。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// 主应用发给浏览器进程的命令。
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// 导航到一个地址。
    Navigate { url: String },
    /// 改视口尺寸。面板被拖动时发。
    Resize { width: i32, height: i32 },
    /// 原始 CDP 消息，直接转给 `send_dev_tools_message`。
    ///
    /// 不在这里定义 CDP 的方法枚举:CDP 的域和参数由 Chromium 定义且随版本
    /// 变化，抄一份到 Rust 里只会多一个必须同步维护的副本。上层想调什么就
    /// 拼什么 JSON，这里只负责搬。
    Cdp { payload: serde_json::Value },
    /// 关掉浏览器，进程退出。
    Shutdown,
}

/// 浏览器进程发给主应用的事件。
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// CEF 就绪，可以接命令了。
    ///
    /// 主应用必须等到这条再发命令。CEF 的初始化是异步的，早发的命令会
    /// 落在还不存在的浏览器上。
    Ready,
    /// 新的一帧可用。像素在共享内存里，这里只给元数据。
    Frame {
        seq: u64,
        width: i32,
        height: i32,
    },
    /// 页面加载结束。
    LoadEnd { status: i32, url: String },
    /// 页面加载失败。
    LoadError { code: i32, text: String, url: String },
    /// CDP 的响应或事件，原样回传。
    Cdp { payload: serde_json::Value },
    /// 进程内部出错。不致命的也报 —— 静默降级比崩溃难查。
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 命令的线格式是稳定的() {
        // 这个格式跨进程，两边各自编译。改了 tag 名或字段名而没同步改
        // 主应用，表现是"命令发过去没反应"，不会有任何编译错误。
        let json = r#"{"cmd":"navigate","url":"https://example.com/"}"#;
        let cmd: Command = serde_json::from_str(json).expect("解析");
        assert!(matches!(cmd, Command::Navigate { url } if url == "https://example.com/"));
    }

    #[test]
    fn cdp_载荷不做任何解释() {
        // 上层拼什么就传什么。这里加一层枚举等于把 Chromium 的协议抄一遍，
        // 而那份东西每个版本都在动。
        let json = r#"{"cmd":"cdp","payload":{"id":1,"method":"Page.captureScreenshot"}}"#;
        let Command::Cdp { payload } = serde_json::from_str(json).expect("解析") else {
            panic!("应该是 Cdp");
        };
        assert_eq!(payload["method"], "Page.captureScreenshot");
    }

    #[test]
    fn 事件序列化成单行() {
        // NDJSON 的前提是一条消息一行。多行会把流切错位。
        let line = serde_json::to_string(&Event::Frame {
            seq: 7,
            width: 1280,
            height: 800,
        })
        .expect("序列化");
        assert!(!line.contains('\n'), "事件不能跨行: {line}");
        assert!(line.contains("\"event\":\"frame\""));
    }
}
