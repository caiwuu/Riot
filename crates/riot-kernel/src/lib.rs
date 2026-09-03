//! 内核进程的库部分:stdio 上的 JSON-RPC 服务循环。
//!
//! 阶段 B 里内核是一个独立进程,宿主通过换行分隔的 JSON-RPC over stdio
//! 和它说话(见 ARCHITECTURE.md §2.2、§12)。这个模块只负责**传输与派发**:
//! 从 stdin 读一行一条请求,派发给处理逻辑,把响应和事件通知写回 stdout。
//!
//! # 为什么出站要走一个 writer 任务
//!
//! 多个请求可以并发处理,而事件通知(会话事件流)是随时产生的 ——
//! 它们都要写同一个 stdout。直接让各个任务自己写会交错出半行 JSON。
//! 所有出站消息先汇聚到一个 mpsc,由**单个** writer 任务串行写出,
//! 天然保证每条消息占整整一行。
//!
//! # stdout 是协议通道,不是日志通道
//!
//! `[约束]` 任何 `tracing` / `println!` 都不能写 stdout —— 那会把一行日志
//! 混进 JSON-RPC 流,宿主的读取器解析到非法 JSON。日志一律走 stderr
//! (宿主的 supervisor 把内核 stderr 接进自己的 tracing)。

use std::io::Write as _;

// 会话装配与内核逻辑的模块。阶段 B 从 src-tauri 逐个搬进来
// (见 ARCHITECTURE.md §2.2):宿主内嵌期间通过 `pub use` re-export 维持
// 原有的 `crate::` 路径,拆进程后由 riot-kernel 二进制直接承载。
//
// crate 归属 ≠ 进程归属:宿主进程和内核进程都链接本 crate,只是入口不同。
// config 的文件读写代码在这里,宿主进程照常在自己进程内调用它(设置页归宿主);
// 内核进程不碰配置文件,每轮所需的配置值随 RPC 传入。
pub mod bridge;
pub mod changes;
pub mod classifier;
pub mod config;
pub mod content;
pub mod env;
pub mod gate;
pub mod git;
pub mod git_changes;
pub mod hooks;
pub mod manager;
pub mod memory;
pub mod mentions;
pub mod models;
pub mod packs;
pub mod prompt;
pub mod session;
pub mod skills;
pub mod slash;
pub mod subagent;
pub mod tasks;
pub mod vision;
pub mod web;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use riot_protocol::rpc::{RpcError, RpcErrorCode, RpcNotification, RpcRequest, RpcResponse};

/// 向宿主推一条内核级错误通知。
///
/// `fatal` 为真表示内核处于不可继续的状态,宿主应当重启它
/// (见 supervisor 的 `RestartPolicy`)。panic hook 用它把"内核要死了"
/// 这件事送出去,而不是让宿主对着一个突然沉默的进程超时。
///
/// `[约束]` 这里**同步直写 stdout**,不走 [`serve`] 内部的异步 writer 任务。
/// 两个原因:一是 panic hook 在任意线程/时刻触发,那时 tokio 的 writer 任务
/// 可能已经不再被调度;二是把 sender 存进全局给 hook 用,会让 writer 的
/// channel 永远不关闭(全局那份 clone 永不 drop),EOF 之后内核就卡着不退出。
/// panic 是异常路径、进程即将结束,best-effort 直写足够,和异步 writer
/// 偶发交错也不比"内核静默消失"更糟。
pub fn report_kernel_error(message: impl Into<String>, fatal: bool) {
    let note = RpcNotification::KernelError {
        message: message.into(),
        fatal,
    };
    if let Ok(mut line) = serde_json::to_string(&note) {
        line.push('\n');
        let mut out = std::io::stdout();
        let _ = out.write_all(line.as_bytes());
        let _ = out.flush();
    }
}

/// 跑 stdio JSON-RPC 服务循环,直到 stdin 收到 EOF。
///
/// EOF 是宿主发起的标准关闭信号(见 ARCHITECTURE.md §2.3 的关闭序列):
/// 宿主 drop 掉内核的 stdin,读循环拿到 `Ok(None)`,函数收尾返回,进程随后
/// 退出。这条路径给了内核 flush 会话、清理子进程的机会,而不是被强杀。
pub async fn serve<R, W>(reader: R, writer: W, sessions_dir: std::path::PathBuf)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();

    // 反向 RPC 的桥:内核 → 宿主(终端/浏览器是宿主能力)。请求走同一个
    // 出站通道,应答由下面的读循环按 id 送回来。
    let host_bridge = bridge::HostBridge::new(out_tx.clone());

    // 会话管理器:活会话 + turn 驱动 + MCP。它持有 out_tx 的一份 clone,
    // 用来给每个会话 attach 事件出口(会话事件 → event.agent 通知)。
    let manager = std::sync::Arc::new(manager::SessionManager::new(
        out_tx.clone(),
        sessions_dir,
        std::sync::Arc::clone(&host_bridge),
    ));

    // 单个 writer 任务串行写出,保证每条消息独占一行。
    let writer_task = tokio::spawn(async move {
        let mut w = writer;
        while let Some(line) = out_rx.recv().await {
            if w.write_all(line.as_bytes()).await.is_err()
                || w.write_all(b"\n").await.is_err()
                || w.flush().await.is_err()
            {
                // stdout 断了 = 宿主没了。再写也是白写。
                break;
            }
        }
    });

    let mut lines = BufReader::new(reader).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if line.trim().is_empty() {
                    continue;
                }
                // 每条请求独立处理,互不阻塞 —— 一个慢请求不该挡住后面的。
                // 结果通过 out_tx 汇聚回 writer,顺序由完成时刻决定
                // (JSON-RPC 用 id 配对,不依赖到达顺序)。
                let out = out_tx.clone();
                let mgr = std::sync::Arc::clone(&manager);
                let br = std::sync::Arc::clone(&host_bridge);
                tokio::spawn(async move {
                    // 反向应答:有 id 没 method —— 宿主对内核所发请求的回话。
                    // (正向请求带 method;两个方向的 id 空间各自独立。)
                    if let Ok(v) = serde_json::from_str::<Value>(&line)
                        && v.get("method").is_none()
                        && let Some(id) = v.get("id").and_then(Value::as_u64)
                    {
                        let resp = serde_json::from_value::<riot_protocol::hostcall::HostResponse>(
                            v.get("result").cloned().unwrap_or(Value::Null),
                        )
                        .unwrap_or_else(|e| {
                            riot_protocol::hostcall::HostResponse::Error {
                                kind: riot_protocol::hostcall::HostCallErrorKind::Unavailable,
                                message: format!("宿主应答解析失败:{e}"),
                            }
                        });
                        if !br.resolve(id, resp).await {
                            tracing::warn!(id, "收到无人等待的反向应答");
                        }
                        return;
                    }
                    if let Some(resp) = handle_line(&line, &mgr).await {
                        let _ = out.send(resp);
                    }
                });
            }
            // EOF:宿主关闭了 stdin。这是优雅退出信号。
            Ok(None) => break,
            Err(e) => {
                tracing::warn!(error = %e, "读 stdin 失败,结束服务循环");
                break;
            }
        }
    }

    // stdin EOF:宿主要关了。此刻 SessionManager 和后台轮子可能仍持有 out_tx
    // 的 clone(给会话发事件用),等 channel 自然关闭会挂住 —— 而已经没有
    // 消费者了(宿主没了)。正常关闭走 kernel.shutdown,那条路已经中断会话、
    // flush 过 transcript;这里直接停掉 writer 作为最终兜底。
    drop(out_tx);
    writer_task.abort();
    let _ = writer_task.await;
}

/// 处理一行请求,返回要写回的一行响应(JSON-RPC 信封)。
///
/// `None` 表示无从应答(连 id 都解析不出来),只记日志。有 id 的一律回一条 ——
/// 少回一条会让宿主那边的等待挂到超时。
async fn handle_line(line: &str, manager: &manager::SessionManager) -> Option<String> {
    let value: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            // 没有 id 就没法配对应答,只能丢。宿主发的一定是合法 JSON,
            // 走到这里说明传输被截断或有人手工塞了脏数据。
            //
            // `[约束]` 不能把原文打出来。`turn.submit` 的 params 里是
            // `TurnConfig`，它带着明文 `ModelEndpoint.api_key` —— 而最容易
            // 走到这一支的恰恰是超长请求被截断，超长请求又恰恰是带图片的
            // `turn.submit`。内核日志走 stderr，宿主会接进自己的 tracing
            // 并可能落盘。只记长度和开头，够定位"是不是被截断了"。
            tracing::warn!(
                error = %e,
                bytes = line.len(),
                head = %line.chars().take(48).collect::<String>(),
                "收到非法 JSON,已忽略"
            );
            return None;
        }
    };

    let id = value.get("id").and_then(Value::as_u64);

    let response = match parse_request(&value) {
        Ok(request) => dispatch(request, manager).await,
        Err(e) => RpcResponse::Error {
            error: RpcError {
                code: RpcErrorCode::InvalidParams,
                message: format!("请求解析失败:{e}"),
            },
        },
    };

    // JSON-RPC 信封:id 原样回传供宿主配对,result 里放 RpcResponse。
    let envelope = json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": response,
    });
    serde_json::to_string(&envelope).ok()
}

/// 从 JSON-RPC 信封里抽出 `method` + `params`,还原成 [`RpcRequest`]。
///
/// 宿主发的报文形状是 `{"jsonrpc","id","method","params"}`(见 supervisor
/// 的 `Kernel::request`),而 [`RpcRequest`] 是 adjacently-tagged
/// (`tag="method", content="params"`)。两者只差 `jsonrpc`/`id` 两个信封
/// 字段,把 method+params 摘出来重组即可交给 serde 定型。
///
/// `params` 为 null / 缺失时**不带** content 字段 —— 无参变体
/// (如 `kernel.ping`)在 adjacently-tagged 下期望的就是只有 tag 的对象。
fn parse_request(value: &Value) -> Result<RpcRequest, serde_json::Error> {
    let method = value.get("method").cloned().unwrap_or(Value::Null);
    let reconstructed = match value.get("params") {
        Some(params) if !params.is_null() => json!({ "method": method, "params": params }),
        _ => json!({ "method": method }),
    };
    serde_json::from_value(reconstructed)
}

/// 把一个请求派发到处理逻辑。
///
/// 阶段 B 施工中:目前只有 `kernel.ping` 落地(它验证进程活着、协议通)。
/// 会话相关的方法(session.*/turn.*/permission.*/config.*/tools.*)会随
/// M-B2 把会话装配搬进内核后逐个接上;在那之前它们回一条明确的"尚未实现",
/// 而不是静默无应答 —— 后者会让宿主等到超时,查起来没有任何线索。
async fn dispatch(request: RpcRequest, manager: &manager::SessionManager) -> RpcResponse {
    use riot_protocol::rpc::RpcRequest as Req;

    // 未实现分支要能报出方法名 —— 先算好(下面的 match 会消费 request)。
    let method = serde_json::to_value(&request)
        .ok()
        .and_then(|v| v.get("method").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned());

    match request {
        Req::KernelPing => RpcResponse::Pong {
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        Req::KernelShutdown => {
            manager.shutdown().await;
            RpcResponse::Ok
        }
        Req::SessionCreate { cwd, .. } => RpcResponse::SessionCreated {
            session_id: manager.create(cwd).await,
        },
        Req::SessionResume { session_id, cwd } => {
            let snap = manager.resume(session_id.as_str(), cwd).await;
            RpcResponse::SessionResumed {
                messages: snap.messages,
                archived: snap.archived,
                busy: snap.busy,
                compacting: snap.compacting,
                pending_asks: snap.pending_asks,
                live_text: snap.live_text,
                live_thinking: snap.live_thinking,
                tasks: snap.tasks,
            }
        }
        Req::SessionDelete { session_id } => {
            manager.delete(session_id.as_str()).await;
            RpcResponse::Ok
        }
        Req::TurnSubmit {
            session_id,
            input,
            config,
        } => match manager.submit(session_id.as_str(), input, *config).await {
            Ok(queued_id) => RpcResponse::TurnSubmitted { queued_id },
            Err(e) => RpcResponse::Error {
                error: RpcError {
                    code: RpcErrorCode::Internal,
                    message: e,
                },
            },
        },
        Req::TurnRegenerate {
            session_id,
            message_id,
            config,
        } => match manager
            .regenerate(session_id.as_str(), &message_id, *config)
            .await
        {
            Ok(()) => RpcResponse::Ok,
            Err(e) => RpcResponse::Error {
                error: RpcError {
                    code: if e.contains("正在跑") {
                        RpcErrorCode::TurnInProgress
                    } else {
                        RpcErrorCode::Internal
                    },
                    message: e,
                },
            },
        },
        Req::TurnInterrupt { session_id, .. } => {
            manager.interrupt(session_id.as_str()).await;
            RpcResponse::Ok
        }
        Req::PermissionRespond {
            request_id,
            response,
        } => {
            manager
                .respond_permission(request_id.as_str(), response)
                .await;
            RpcResponse::Ok
        }
        Req::QueueList { session_id } => RpcResponse::QueueList {
            entries: manager.queue_list(session_id.as_str()).await,
        },
        Req::QueueRemove {
            session_id,
            entry_id,
        } => RpcResponse::Removed {
            removed: manager.queue_remove(session_id.as_str(), &entry_id).await,
        },
        Req::QueueTake {
            session_id,
            entry_id,
        } => RpcResponse::QueueTaken {
            input: manager.queue_take(session_id.as_str(), &entry_id).await,
        },
        Req::TaskCancel {
            session_id,
            agent_id,
        } => RpcResponse::Removed {
            removed: manager.cancel_task(session_id.as_str(), &agent_id).await,
        },
        Req::TaskHistory {
            session_id,
            agent_id,
        } => match manager.task_history(session_id.as_str(), &agent_id).await {
            Some((task, messages)) => RpcResponse::TaskHistory {
                task: Some(task),
                messages,
            },
            None => RpcResponse::TaskHistory {
                task: None,
                messages: Vec::new(),
            },
        },
        Req::SessionCompact { session_id, model } => {
            match manager.compact(session_id.as_str(), *model).await {
                Ok(()) => RpcResponse::Ok,
                Err(e) => RpcResponse::Error {
                    error: RpcError {
                        code: RpcErrorCode::Internal,
                        message: e,
                    },
                },
            }
        }
        Req::HistoryEdit {
            session_id,
            message_id,
            text,
        } => match manager
            .edit_message(session_id.as_str(), &message_id, &text)
            .await
        {
            Ok(()) => RpcResponse::Ok,
            Err(e) => RpcResponse::Error {
                error: RpcError {
                    code: if e.contains("正在跑") {
                        RpcErrorCode::TurnInProgress
                    } else {
                        RpcErrorCode::Internal
                    },
                    message: e,
                },
            },
        },
        Req::HistoryDelete {
            session_id,
            message_id,
        } => match manager
            .delete_message(session_id.as_str(), &message_id)
            .await
        {
            Ok(()) => RpcResponse::Ok,
            Err(e) => RpcResponse::Error {
                error: RpcError {
                    code: if e.contains("正在跑") {
                        RpcErrorCode::TurnInProgress
                    } else {
                        RpcErrorCode::Internal
                    },
                    message: e,
                },
            },
        },
        Req::SessionChanges { session_id } => RpcResponse::Changes {
            changes: manager.changes(session_id.as_str()).await,
        },
        Req::SessionGitChanges { session_id, base } => RpcResponse::GitChanges {
            git: manager
                .git_changes(session_id.as_str(), base.as_deref())
                .await,
        },
        Req::SessionSetTitle { session_id, title } => {
            manager.set_title(session_id.as_str(), title).await;
            RpcResponse::Ok
        }
        Req::ConfigSetMode { session_id, mode } => {
            manager.set_mode(session_id.as_str(), mode).await;
            RpcResponse::Ok
        }
        Req::ScopeList { session_id } => RpcResponse::ScopeHosts {
            hosts: manager.scope_hosts(session_id.as_str()).await,
        },
        Req::ScopeRevoke { session_id, host } => {
            manager.revoke_scope(session_id.as_str(), &host).await;
            RpcResponse::Ok
        }
        Req::McpReconcile { servers } => {
            manager.mcp_reconcile(servers).await;
            RpcResponse::Ok
        }
        Req::McpStatus => RpcResponse::McpStatuses {
            servers: manager.mcp_statuses().await,
        },
        Req::McpRestart { id } => {
            if manager.mcp_restart(&id).await {
                RpcResponse::Ok
            } else {
                RpcResponse::Error {
                    error: RpcError {
                        code: RpcErrorCode::InvalidParams,
                        message: format!("没有叫「{id}」的 MCP 服务器在运行。先在设置里启用它。"),
                    },
                }
            }
        }
        // 其余方法(session.list、tools.list)随宿主翻转按需接上;
        // 在那之前明确报未实现。
        _ => RpcResponse::Error {
            error: RpcError {
                code: RpcErrorCode::Internal,
                message: format!("方法「{method}」尚未在内核实现(阶段 B 施工中)"),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_manager() -> manager::SessionManager {
        let (out_tx, _rx) = mpsc::unbounded_channel();
        manager::SessionManager::new(
            out_tx.clone(),
            std::env::temp_dir().join("riot-kernel-test-sessions"),
            bridge::HostBridge::new(out_tx),
        )
    }

    #[tokio::test]
    async fn ping_returns_pong_with_version() {
        let mgr = test_manager();
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"kernel.ping"}"#;
        let out = handle_line(line, &mgr).await.expect("ping 要有应答");
        let v: Value = serde_json::from_str(&out).expect("应答是合法 JSON");
        assert_eq!(v["id"], 1, "id 要原样回传供配对");
        assert_eq!(v["result"]["result"], "pong");
        assert!(
            v["result"]["data"]["version"].as_str().is_some(),
            "pong 要带内核版本:{out}"
        );
    }

    #[tokio::test]
    async fn ping_with_null_params_still_parses() {
        // supervisor 的 request(method, params) 对无参方法传的是 params:null。
        // adjacently-tagged 的无参变体不认多余的 content,得在重组时剥掉。
        let mgr = test_manager();
        let line = r#"{"jsonrpc":"2.0","id":7,"method":"kernel.ping","params":null}"#;
        let out = handle_line(line, &mgr)
            .await
            .expect("带 null params 也要能应答");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["result"]["result"], "pong");
    }

    #[tokio::test]
    async fn unknown_method_reports_error_not_silence() {
        // 尚未接上的方法要回一条明确错误 —— 静默无应答会让宿主等到超时。
        // (随方法逐个落地,这里要挑一个仍未接的;目前是 tools.list。)
        let mgr = test_manager();
        let line = r#"{"jsonrpc":"2.0","id":2,"method":"tools.list","params":{"session_id":"s1"}}"#;
        let out = handle_line(line, &mgr).await.expect("未实现的方法也要应答");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["id"], 2);
        assert_eq!(v["result"]["result"], "error");
        // RpcResponse::Error { error } → data.error.message(变体字段再套一层)。
        let msg = v["result"]["data"]["error"]["message"]
            .as_str()
            .unwrap_or_default();
        assert!(msg.contains("tools.list"), "错误要点名是哪个方法:{out}");
    }

    #[tokio::test]
    async fn malformed_json_is_dropped_without_panic() {
        assert!(
            handle_line("{ 这不是 json", &test_manager())
                .await
                .is_none(),
            "非法 JSON 无从配对应答,丢弃而不是崩"
        );
    }
}
