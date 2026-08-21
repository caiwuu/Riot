//! 反向 RPC 的内核端:发请求的桥 + 终端/浏览器的远程代理。
//!
//! 终端面板和面板浏览器活在宿主进程(用户看得见、管得着的东西必须在
//! 那边,见 ARCHITECTURE.md §2.4)。内核里的工具通过 [`TerminalAccess`] /
//! [`BrowserAccess`] trait 用它们 —— 这里是那两个 trait 的"跨进程"实现:
//! 每个方法serialize 成 [`HostRequest`] 写 stdout,等宿主把应答写回 stdin。
//!
//! `[约束]` 代理不做超时。宿主的处理端对每个请求**必回**(错误也回);
//! 宿主整个没了的话 stdin 会 EOF,内核随之退出,悬着的等待自然作废。
//! 在这里加一层超时只会在慢操作(浏览器 wait_for 可以合法地等几十秒)
//! 上制造假失败。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use tokio::sync::{Mutex, oneshot};

use riot_protocol::browser::{
    Action, BrowserAccess, BrowserUnavailable, InteractError, InterceptOp, MarkedView, Nav,
    NetQuery, Target, WaitCondition,
};
use riot_protocol::env::{EnvProbe, EnvSnapshot};
use riot_protocol::hostcall::{BrowserCall, HostCallErrorKind, HostRequest, HostResponse};
use riot_protocol::id::SessionId;
use riot_protocol::terminal::{TerminalAccess, TerminalInfo, TerminalUnavailable};

use crate::manager::Outbound;

/// 发反向请求、按 id 配对应答的桥。serve 建一个,stdin 读循环用
/// [`Self::resolve`] 送回应答,各会话的远程代理共享同一座桥。
pub struct HostBridge {
    out: Outbound,
    pending: Mutex<HashMap<u64, oneshot::Sender<HostResponse>>>,
    next_id: AtomicU64,
}

impl HostBridge {
    pub fn new(out: Outbound) -> Arc<Self> {
        Arc::new(Self {
            out,
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        })
    }

    /// 发一个请求并等宿主应答。Err = 传输层断了(宿主没了)。
    pub async fn call(&self, req: HostRequest) -> Result<HostResponse, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        // HostRequest 是 {method, params} 形状,包上 JSON-RPC 信封。
        let v = serde_json::to_value(&req).map_err(|e| format!("反向请求序列化失败:{e}"))?;
        let line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": v.get("method"),
            "params": v.get("params"),
        })
        .to_string();
        if self.out.send(line).is_err() {
            self.pending.lock().await.remove(&id);
            return Err("内核出站通道已关,反向调用发不出去".to_owned());
        }
        rx.await.map_err(|_| "宿主没有应答(通道关闭)".to_owned())
    }

    /// stdin 读循环看到 `{id, result}`(无 method)时调。
    /// false = 没人在等这个 id(重复应答或已放弃)。
    pub async fn resolve(&self, id: u64, response: HostResponse) -> bool {
        match self.pending.lock().await.remove(&id) {
            Some(tx) => tx.send(response).is_ok(),
            None => false,
        }
    }
}

/// 宿主终端面板的远程代理。
pub struct RemoteTerminal {
    pub session_id: SessionId,
    pub bridge: Arc<HostBridge>,
}

impl RemoteTerminal {
    async fn call(&self, req: HostRequest) -> Result<HostResponse, TerminalUnavailable> {
        self.bridge.call(req).await.map_err(TerminalUnavailable)
    }
}

#[async_trait]
impl TerminalAccess for RemoteTerminal {
    async fn spawn(&self, command: &str, title: &str) -> Result<u32, TerminalUnavailable> {
        match self
            .call(HostRequest::TerminalSpawn {
                session_id: self.session_id.clone(),
                command: command.to_owned(),
                title: title.to_owned(),
            })
            .await?
        {
            HostResponse::TerminalId { id } => Ok(id),
            HostResponse::Error { message, .. } => Err(TerminalUnavailable(message)),
            other => Err(TerminalUnavailable(format!("宿主回了意外形状:{other:?}"))),
        }
    }

    async fn read(&self, id: u32, lines: usize) -> Result<String, TerminalUnavailable> {
        match self
            .call(HostRequest::TerminalRead {
                session_id: self.session_id.clone(),
                id,
                lines,
            })
            .await?
        {
            HostResponse::Text { text } => Ok(text),
            HostResponse::Error { message, .. } => Err(TerminalUnavailable(message)),
            other => Err(TerminalUnavailable(format!("宿主回了意外形状:{other:?}"))),
        }
    }

    async fn kill(&self, id: u32) -> Result<(), TerminalUnavailable> {
        match self
            .call(HostRequest::TerminalKill {
                session_id: self.session_id.clone(),
                id,
            })
            .await?
        {
            HostResponse::Ok => Ok(()),
            HostResponse::Error { message, .. } => Err(TerminalUnavailable(message)),
            other => Err(TerminalUnavailable(format!("宿主回了意外形状:{other:?}"))),
        }
    }

    async fn list(&self) -> Vec<TerminalInfo> {
        match self
            .call(HostRequest::TerminalList {
                session_id: self.session_id.clone(),
            })
            .await
        {
            Ok(HostResponse::Terminals { items }) => items,
            // 清单拿不到就是空的 —— trait 这个方法没有错误通道。
            _ => Vec::new(),
        }
    }
}

/// 宿主环境探针的远程代理。
pub struct RemoteEnv {
    pub session_id: SessionId,
    pub bridge: Arc<HostBridge>,
}

#[async_trait]
impl EnvProbe for RemoteEnv {
    async fn sample(&self) -> Option<EnvSnapshot> {
        match self
            .bridge
            .call(HostRequest::EnvSnapshot {
                session_id: self.session_id.clone(),
            })
            .await
        {
            Ok(HostResponse::Env { snapshot }) => Some(snapshot),
            // 拿不到就是"这一轮没有感知"。工具都还在，模型不因此变盲，
            // 不值得为一次采样失败打断轮次。
            _ => None,
        }
    }
}

/// 宿主面板浏览器的远程代理。
pub struct RemoteBrowser {
    pub session_id: SessionId,
    pub bridge: Arc<HostBridge>,
}

fn interact_error(kind: HostCallErrorKind, message: String) -> InteractError {
    match kind {
        HostCallErrorKind::Unavailable => InteractError::Unavailable(BrowserUnavailable(message)),
        HostCallErrorKind::Target => InteractError::Target(message),
    }
}

impl RemoteBrowser {
    async fn call(&self, call: BrowserCall) -> Result<HostResponse, String> {
        self.bridge
            .call(HostRequest::BrowserCall {
                session_id: self.session_id.clone(),
                call,
            })
            .await
    }

    /// 只有"不可用"一种失败形态的方法(navigate/截图/快照)。
    async fn simple(&self, call: BrowserCall) -> Result<HostResponse, BrowserUnavailable> {
        match self.call(call).await {
            Ok(HostResponse::Error { message, .. }) => Err(BrowserUnavailable(message)),
            Ok(other) => Ok(other),
            Err(e) => Err(BrowserUnavailable(e)),
        }
    }

    /// 期待文本应答的交互方法,错误按 kind 还原。
    async fn text(&self, call: BrowserCall) -> Result<String, InteractError> {
        match self.call(call).await {
            Ok(HostResponse::Text { text }) => Ok(text),
            Ok(HostResponse::Error { kind, message }) => Err(interact_error(kind, message)),
            Ok(other) => Err(InteractError::Target(format!("宿主回了意外形状:{other:?}"))),
            Err(e) => Err(InteractError::Unavailable(BrowserUnavailable(e))),
        }
    }
}

#[async_trait]
impl BrowserAccess for RemoteBrowser {
    async fn navigate(&self, url: &str) -> Result<(), BrowserUnavailable> {
        match self
            .simple(BrowserCall::Navigate {
                url: url.to_owned(),
            })
            .await?
        {
            HostResponse::Ok => Ok(()),
            other => Err(BrowserUnavailable(format!("宿主回了意外形状:{other:?}"))),
        }
    }

    async fn screenshot(&self) -> Result<String, BrowserUnavailable> {
        match self.simple(BrowserCall::Screenshot).await? {
            HostResponse::Text { text } => Ok(text),
            other => Err(BrowserUnavailable(format!("宿主回了意外形状:{other:?}"))),
        }
    }

    async fn snapshot(&self) -> Result<String, BrowserUnavailable> {
        match self.simple(BrowserCall::Snapshot).await? {
            HostResponse::Text { text } => Ok(text),
            other => Err(BrowserUnavailable(format!("宿主回了意外形状:{other:?}"))),
        }
    }

    async fn snapshot_marked(&self) -> Result<MarkedView, BrowserUnavailable> {
        match self.simple(BrowserCall::SnapshotMarked).await? {
            HostResponse::Marked {
                listing,
                screenshot,
            } => Ok(MarkedView {
                listing,
                screenshot,
            }),
            other => Err(BrowserUnavailable(format!("宿主回了意外形状:{other:?}"))),
        }
    }

    async fn console(&self) -> Result<Vec<String>, BrowserUnavailable> {
        match self.simple(BrowserCall::Console).await? {
            HostResponse::Lines { lines } => Ok(lines),
            other => Err(BrowserUnavailable(format!("宿主回了意外形状:{other:?}"))),
        }
    }

    async fn current_url(&self) -> String {
        match self.call(BrowserCall::CurrentUrl).await {
            Ok(HostResponse::Text { text }) => text,
            // 这个方法没有错误通道,拿不到就是空 —— 和 NoBrowser 一致。
            _ => String::new(),
        }
    }

    async fn click(&self, target: Target) -> Result<String, InteractError> {
        self.text(BrowserCall::Click { target }).await
    }

    async fn type_text(
        &self,
        target: Target,
        text: &str,
        submit: bool,
    ) -> Result<String, InteractError> {
        self.text(BrowserCall::TypeText {
            target,
            text: text.to_owned(),
            submit,
        })
        .await
    }

    async fn press_key(&self, key: &str) -> Result<String, InteractError> {
        self.text(BrowserCall::PressKey {
            key: key.to_owned(),
        })
        .await
    }

    async fn scroll(&self, delta_y: f64) -> Result<String, InteractError> {
        self.text(BrowserCall::Scroll { delta_y }).await
    }

    async fn wait_for(
        &self,
        cond: WaitCondition,
        timeout_ms: u64,
    ) -> Result<String, InteractError> {
        self.text(BrowserCall::WaitFor { cond, timeout_ms }).await
    }

    async fn act(&self, action: Action) -> Result<String, InteractError> {
        self.text(BrowserCall::Act { action }).await
    }

    async fn browse(&self, nav: Nav) -> Result<String, InteractError> {
        self.text(BrowserCall::Browse { nav }).await
    }

    async fn evaluate(&self, expr: &str) -> Result<String, InteractError> {
        self.text(BrowserCall::Evaluate {
            expr: expr.to_owned(),
        })
        .await
    }

    async fn upload(&self, target: Target, paths: Vec<String>) -> Result<String, InteractError> {
        self.text(BrowserCall::Upload { target, paths }).await
    }

    async fn cookies(&self) -> Result<String, InteractError> {
        self.text(BrowserCall::Cookies).await
    }

    async fn network(&self, query: NetQuery) -> Result<String, InteractError> {
        self.text(BrowserCall::Network { query }).await
    }

    async fn replay(
        &self,
        url: &str,
        method: &str,
        headers: serde_json::Value,
        body: Option<String>,
    ) -> Result<String, InteractError> {
        self.text(BrowserCall::Replay {
            url: url.to_owned(),
            method: method.to_owned(),
            headers,
            body,
        })
        .await
    }

    async fn intercept(&self, op: InterceptOp) -> Result<String, InteractError> {
        self.text(BrowserCall::Intercept { op }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn 应答按_id_配对_不认识的_id_不崩() {
        let (out, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let bridge = HostBridge::new(out);

        let b = Arc::clone(&bridge);
        let call = tokio::spawn(async move {
            b.call(HostRequest::TerminalList {
                session_id: SessionId::from_raw("s1"),
            })
            .await
        });

        // 收到出站行,取它的 id 回一条应答。
        let line = rx.recv().await.expect("请求该发出来");
        let v: serde_json::Value = serde_json::from_str(&line).expect("合法 JSON");
        assert_eq!(v["method"], "terminal.list");
        let id = v["id"].as_u64().expect("有 id");

        assert!(
            !bridge.resolve(id + 100, HostResponse::Ok).await,
            "不认识的 id 只是返回 false"
        );
        assert!(
            bridge
                .resolve(id, HostResponse::Terminals { items: vec![] })
                .await
        );

        let got = call.await.expect("任务").expect("应答");
        assert_eq!(got, HostResponse::Terminals { items: vec![] });
    }
}
