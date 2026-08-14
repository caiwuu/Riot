//! 连接枢纽：按配置管理一组 MCP 服务器的生命周期。
//!
//! `[约束]` 连接是**应用级**的，会话之间共享（ARCHITECTURE.md §2.4）——
//! 每个会话各起一份的话，三个会话配三个服务器就是九个常驻子进程。
//! 每轮开始时用 [`McpHub::tools`] 拿快照：配置中途改了，下一轮生效，
//! 和联网/视觉能力同一条规矩。
//!
//! 豁免理由：终止进程要等真实的宽限期，用真实时钟。
#![allow(clippy::disallowed_methods)]

use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use riot_protocol::tool::Tool;

use crate::client::{Client, Timeouts};
use crate::stdio;
use crate::tool::McpTool;
use crate::wire::ToolDef;

/// 一个服务器怎么启动。由宿主从配置映射过来。
///
/// `PartialEq` 是 reconcile 的判据：spec 没变就不动它 —— 改一个无关
/// 设置就把所有服务器全部重启，用户会看到工具清单闪断。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSpec {
    /// 稳定标识，进工具名（`mcp__<id>__…`），也是权限规则的一部分。
    pub id: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// 给设置页看的状态快照。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub id: String,
    /// `connecting` / `connected` / `failed`
    pub state: String,
    /// connected 时是服务器自报的名字和版本；failed 时是错误原因。
    pub detail: String,
    /// 对外的完整工具名（`mcp__…`）。
    pub tools: Vec<String>,
}

enum ServerState {
    Connecting,
    Ready {
        client: Arc<Client>,
        /// 进程句柄留在状态里：移除服务器时要杀整组。
        child: Box<dyn process_wrap::tokio::ChildWrapper>,
        server_name: String,
        tools: Vec<ToolDef>,
    },
    Failed {
        error: String,
    },
}

struct Handle {
    spec: ServerSpec,
    state: Arc<Mutex<ServerState>>,
    /// 移除时通知还在连接中的任务收尾（杀进程、别再写状态）。
    cancel: CancellationToken,
}

#[derive(Default)]
pub struct McpHub {
    servers: Mutex<HashMap<String, Handle>>,
}

impl McpHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// 让运行中的服务器集合对齐 `specs`：新增的启动、消失的停掉、
    /// 变了的重启、没变的不动（包括 Failed 的 —— 重试走 [`Self::restart`]，
    /// 每次 reconcile 都自动重试的话，一个配错的服务器会无限重启风暴）。
    pub async fn reconcile(&self, specs: Vec<ServerSpec>) {
        let mut map = self.servers.lock().await;

        let wanted: Vec<&str> = specs.iter().map(|s| s.id.as_str()).collect();
        let doomed: Vec<String> = map
            .keys()
            .filter(|id| !wanted.contains(&id.as_str()))
            .cloned()
            .collect();
        for id in doomed {
            if let Some(h) = map.remove(&id) {
                shutdown_handle(h).await;
            }
        }

        for spec in specs {
            if let Some(existing) = map.get(&spec.id)
                && existing.spec == spec
            {
                continue;
            }
            if let Some(old) = map.remove(&spec.id) {
                shutdown_handle(old).await;
            }
            map.insert(spec.id.clone(), start(spec));
        }
    }

    /// 手动重启一个服务器（设置页的「重连」按钮）。不存在返回 false。
    pub async fn restart(&self, id: &str) -> bool {
        let mut map = self.servers.lock().await;
        let Some(old) = map.remove(id) else {
            return false;
        };
        let spec = old.spec.clone();
        shutdown_handle(old).await;
        map.insert(spec.id.clone(), start(spec));
        true
    }

    pub async fn statuses(&self) -> Vec<ServerStatus> {
        let map = self.servers.lock().await;
        let mut out = Vec::with_capacity(map.len());
        for (id, h) in map.iter() {
            let st = h.state.lock().await;
            out.push(match &*st {
                ServerState::Connecting => ServerStatus {
                    id: id.clone(),
                    state: "connecting".into(),
                    detail: String::new(),
                    tools: Vec::new(),
                },
                ServerState::Ready { client, server_name, tools, .. } => {
                    if client.is_alive() {
                        ServerStatus {
                            id: id.clone(),
                            state: "connected".into(),
                            detail: server_name.clone(),
                            tools: tools
                                .iter()
                                .map(|t| crate::tool::tool_name(id, &t.name))
                                .collect(),
                        }
                    } else {
                        ServerStatus {
                            id: id.clone(),
                            state: "failed".into(),
                            detail: "进程退出或连接断开。点「重连」再试。".into(),
                            tools: Vec::new(),
                        }
                    }
                }
                ServerState::Failed { error } => ServerStatus {
                    id: id.clone(),
                    state: "failed".into(),
                    detail: error.clone(),
                    tools: Vec::new(),
                },
            });
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// 当前可用工具的快照（本轮用）。断开的服务器自然不在里面。
    pub async fn tools(&self) -> Vec<Arc<dyn Tool>> {
        let map = self.servers.lock().await;
        let mut out: Vec<Arc<dyn Tool>> = Vec::new();
        for (id, h) in map.iter() {
            let mut st = h.state.lock().await;
            let ServerState::Ready { client, tools, .. } = &mut *st else {
                continue;
            };
            if !client.is_alive() {
                continue;
            }
            // 服务器说清单变了就重拉一次。失败不致命 —— 用旧清单，
            // 调到已下线的工具时服务器会报错，模型能看懂并换路。
            if client.take_list_changed() {
                match client.list_tools().await {
                    Ok(fresh) => *tools = fresh,
                    Err(e) => tracing::warn!(server = %id, error = %e, "工具清单刷新失败，沿用旧清单"),
                }
            }
            for def in tools.iter() {
                out.push(Arc::new(McpTool::new(id, def, Arc::clone(client))));
            }
        }
        out
    }

    /// 停掉全部服务器。退出钩子用。
    pub async fn shutdown(&self) {
        let mut map = self.servers.lock().await;
        for (_, h) in map.drain() {
            shutdown_handle(h).await;
        }
    }
}

fn start(spec: ServerSpec) -> Handle {
    let state = Arc::new(Mutex::new(ServerState::Connecting));
    let cancel = CancellationToken::new();
    tokio::spawn(connect_task(spec.clone(), Arc::clone(&state), cancel.clone()));
    Handle { spec, state, cancel }
}

async fn connect_task(spec: ServerSpec, state: Arc<Mutex<ServerState>>, cancel: CancellationToken) {
    // 进程先起起来、句柄拿在手上，之后的每条失败路径都要负责杀掉它 ——
    // 把 spawn 塞进可取消的 future 里的话，取消时机不巧就会漏一个进程组。
    let spawned = match stdio::spawn_server(&spec) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(server = %spec.id, error = %e, "MCP 服务器启动失败");
            *state.lock().await = ServerState::Failed {
                error: format!("启动失败：{e}。检查命令路径和参数。"),
            };
            return;
        }
    };
    let child = spawned.child;

    let connect = async {
        let (client, hello) =
            Client::connect(spawned.stdout, spawned.stdin, Timeouts::default()).await?;
        let tools = client.list_tools().await?;
        Ok::<_, crate::client::ClientError>((client, hello, tools))
    };

    tokio::select! {
        r = connect => match r {
            Ok((client, hello, tools)) => {
                tracing::info!(
                    server = %spec.id,
                    name = %hello.name,
                    tools = tools.len(),
                    "MCP 服务器已连接"
                );
                *state.lock().await = ServerState::Ready {
                    client,
                    child,
                    server_name: format!("{} {}", hello.name, hello.version),
                    tools,
                };
            }
            Err(e) => {
                tracing::warn!(server = %spec.id, error = %e, "MCP 握手失败");
                stdio::terminate(child).await;
                *state.lock().await = ServerState::Failed { error: e.to_string() };
            }
        },
        _ = cancel.cancelled() => {
            stdio::terminate(child).await;
        }
    }
}

async fn shutdown_handle(h: Handle) {
    // 还在连接中的任务看到取消会自己杀进程；已就绪的从状态里取出句柄杀。
    h.cancel.cancel();
    let mut st = h.state.lock().await;
    if let ServerState::Ready { child, .. } =
        std::mem::replace(&mut *st, ServerState::Failed { error: "已停止".into() })
    {
        stdio::terminate(child).await;
    }
}
