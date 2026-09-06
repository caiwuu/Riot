//! 网页版与宿主之间的 WebSocket 线协议。
//!
//! 设计目标只有一个：**让前端的 bridge 在 Tauri IPC 和这条线之间可以无感
//! 切换**。所以这里的每个概念都对着 Tauri IPC 的一个概念：
//!
//! | Tauri IPC                         | 这里                              |
//! |-----------------------------------|-----------------------------------|
//! | `invoke(cmd, args)`               | [`ClientFrame::Call`]             |
//! | 命令返回 JSON                     | [`ServerFrame::Ok`]               |
//! | 命令返回 `ipc::Response`（二进制）| 二进制帧，`kind = 1`               |
//! | 命令报错（字符串）                | [`ServerFrame::Err`]              |
//! | `Channel<T>` 参数                 | args 里一个 `__RIOT_CHANNEL__:<id>` 串 |
//! | `Channel::send(JSON)`             | [`ServerFrame::Channel`]          |
//! | `Channel::send(Raw)`              | 二进制帧，`kind = 2`               |
//! | `app.emit(event)` / `listen`      | [`ServerFrame::Event`]            |
//!
//! 文本帧是 JSON，`t` 字段区分类型。二进制帧只从宿主发往前端，格式是
//! `[kind: u8][id: u32 小端][payload]`：kind 1 的 id 是请求号，kind 2 的 id
//! 是通道号。二进制不走 JSON 的理由同 Tauri 那边（浏览器面板一帧几百 KB，
//! base64 再 `JSON.parse` 一遍是主线程上最贵的一刀）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 通道参数的前缀。前端把 `Channel` 序列化成 `__RIOT_CHANNEL__:<id>` 放进
/// args，宿主看到这个前缀就换成一条真正的 [`tauri::ipc::Channel`]。
///
/// 和 Tauri 自己的 `__CHANNEL__:` 刻意不同名 —— 两套 id 空间独立，
/// 万一混用（前端拿 Tauri 的 Channel 对象走了这条线）要能一眼认出来。
pub const CHANNEL_PREFIX: &str = "__RIOT_CHANNEL__:";

/// 二进制帧：命令的原始字节结果（对应 Tauri 的 `ipc::Response`）。
pub const BIN_CALL_RESULT: u8 = 1;
/// 二进制帧：通道上的一条原始字节消息。
pub const BIN_CHANNEL: u8 = 2;

/// 前端 → 宿主。
#[derive(Debug, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientFrame {
    /// 连接后的第一帧。不先过这一步，其它任何帧都会让连接被关掉。
    Auth { token: String },
    /// 调一条命令。`id` 由前端分配、单调递增，应答原样带回。
    Call {
        id: u64,
        cmd: String,
        #[serde(default)]
        args: Value,
    },
    /// 心跳。宿主回 `pong`。浏览器里的 WebSocket 发不了协议层的 ping，
    /// 前端靠这一来一回判断连接是不是已经悄悄死了（Wi-Fi 切换、手机锁屏）。
    Ping,
}

/// 宿主 → 前端（文本帧部分）。
#[derive(Debug, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerFrame<'a> {
    /// 鉴权通过。`viewer` 是这条连接在宿主里的观看者 id；`boot` 是宿主
    /// 本次启动的随机标识 —— 重连时它变了，说明宿主重启过，前端的一切
    /// 本地状态（终端 id、浏览器面板）都作废，直接整页刷新最稳。
    Ready {
        viewer: &'a str,
        boot: &'a str,
        version: &'a str,
    },
    /// 鉴权失败，随后连接关闭。
    Denied { reason: &'a str },
    Ok { id: u64, result: Value },
    Err { id: u64, error: String },
    Event { name: &'a str, payload: Value },
    Pong,
    // 还有一种文本帧不在这个枚举里：通道上的 JSON 消息
    // `{"t":"channel","ch":<u32>,"data":<json>}`。它由 conn.rs 手拼 ——
    // `data` 是 `Channel::send` 给的现成 JSON 串，在 token 流上再 parse 一遍
    // 只为重新序列化不值。前端按同一个形状解。
}

/// 组一条二进制帧。
pub fn binary_frame(kind: u8, id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + payload.len());
    out.push(kind);
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// args 里的一个值是不是通道占位。是就给出通道号。
pub fn channel_id(v: &Value) -> Option<u32> {
    v.as_str()?
        .strip_prefix(CHANNEL_PREFIX)?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 二进制帧头是五个字节() {
        let f = binary_frame(BIN_CHANNEL, 0x0102_0304, b"ab");
        assert_eq!(f, vec![2, 4, 3, 2, 1, b'a', b'b']);
    }

    #[test]
    fn 通道占位只认自己的前缀() {
        assert_eq!(channel_id(&Value::from("__RIOT_CHANNEL__:7")), Some(7));
        assert_eq!(channel_id(&Value::from("__CHANNEL__:7")), None);
        assert_eq!(channel_id(&Value::from("__RIOT_CHANNEL__:x")), None);
        assert_eq!(channel_id(&Value::from(7)), None);
    }

    #[test]
    fn 客户端帧按_t_字段区分() {
        let f: ClientFrame =
            serde_json::from_str(r#"{"t":"call","id":3,"cmd":"list_sessions"}"#).expect("解析");
        match f {
            ClientFrame::Call { id, cmd, args } => {
                assert_eq!(id, 3);
                assert_eq!(cmd, "list_sessions");
                assert!(args.is_null(), "没带 args 就是 null，分发那边当空表");
            }
            other => panic!("解析成了 {other:?}"),
        }
    }
}
