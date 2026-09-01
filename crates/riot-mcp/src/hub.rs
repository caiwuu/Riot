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
                ServerState::Ready {
                    client,
                    server_name,
                    tools,
                    ..
                } => {
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
    ///
    /// `[约束]` 刷新清单要走网络（`tools/list`，30 秒超时），这段
    /// **不能持锁**——无论是 `servers` 还是某个服务器的 `state`。
    /// 早先是整段持 `servers` 锁的：一个挂起的服务器就把整个 hub 按住
    /// 半分钟，`statuses()` / `reconcile()` 全排队，设置页表现为整体卡死，
    /// 而唯一的线索只指向某一个服务器。
    pub async fn tools(&self) -> Vec<Arc<dyn Tool>> {
        // 1. 锁内只取句柄快照。
        let handles: Vec<(String, Arc<Mutex<ServerState>>)> = {
            let map = self.servers.lock().await;
            map.iter()
                .map(|(id, h)| (id.clone(), Arc::clone(&h.state)))
                .collect()
        };

        let mut out: Vec<Arc<dyn Tool>> = Vec::new();
        let mut names: HashMap<String, String> = HashMap::new();

        for (id, state) in handles {
            // 2. 锁内只读出客户端和当前清单，立刻放锁。
            let Some((client, mut tools, stale)) = ({
                let st = state.lock().await;
                match &*st {
                    ServerState::Ready { client, tools, .. } if client.is_alive() => Some((
                        Arc::clone(client),
                        tools.clone(),
                        client.take_list_changed(),
                    )),
                    _ => None,
                }
            }) else {
                continue;
            };

            // 3. 锁外刷新。失败不致命 —— 用旧清单，调到已下线的工具时
            //    服务器会报错，模型能看懂并换路。
            if stale {
                match client.list_tools().await {
                    Ok(fresh) => {
                        tools = fresh;
                        // 4. 写回去。这期间服务器可能已经停了，所以要
                        //    重新确认它还是 Ready 才写。被 reconcile 整个
                        //    换掉的情况天然安全：那会换上一个新的状态对象，
                        //    我们手上这个已经没人看了。
                        if let ServerState::Ready { tools: slot, .. } = &mut *state.lock().await {
                            *slot = tools.clone();
                        }
                    }
                    Err(e) => {
                        tracing::warn!(server = %id, error = %e, "工具清单刷新失败，沿用旧清单")
                    }
                }
            }

            for def in &tools {
                let name = crate::tool::tool_name(&id, &def.name);
                // 重名的不注册。名字是权限规则的匹配键，两个工具共用一个
                // 名字意味着用户对着其中一个点的"总是允许"顺带放行了另一个。
                if let Some(prev) = names.get(&name) {
                    tracing::warn!(
                        tool = %name,
                        server = %id,
                        remote = %def.name,
                        first = %prev,
                        "对外工具名重复，后来的这个不注册"
                    );
                    continue;
                }
                names.insert(name, format!("{id}/{}", def.name));
                out.push(Arc::new(McpTool::new(&id, def, Arc::clone(&client))));
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
    tokio::spawn(connect_task(
        spec.clone(),
        Arc::clone(&state),
        cancel.clone(),
    ));
    Handle {
        spec,
        state,
        cancel,
    }
}

async fn connect_task(spec: ServerSpec, state: Arc<Mutex<ServerState>>, cancel: CancellationToken) {
    // 进程先起起来、句柄拿在手上，之后的每条失败路径都要负责杀掉它 ——
    // 把 spawn 塞进可取消的 future 里的话，取消时机不巧就会漏一个进程组。
    let spawned = match stdio::spawn_server(&spec) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(server = %spec.id, error = %e, "MCP 服务器启动失败");
            *state.lock().await = ServerState::Failed {
                error: spawn_error(&spec.command, e),
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

fn spawn_error(command: &str, err: std::io::Error) -> String {
    if err.kind() == std::io::ErrorKind::NotFound {
        format!(
            "启动失败：找不到命令「{command}」。\
             从访达或 Dock 打开时没有终端里的 PATH，\
             把命令改成 `which {command}` 给出的绝对路径，或确认 npx / uvx / node 已安装。"
        )
    } else {
        format!("启动失败：{err}。检查命令路径和参数。")
    }
}

async fn shutdown_handle(h: Handle) {
    // 还在连接中的任务看到取消会自己杀进程；已就绪的从状态里取出句柄杀。
    h.cancel.cancel();
    let mut st = h.state.lock().await;
    if let ServerState::Ready { child, .. } = std::mem::replace(
        &mut *st,
        ServerState::Failed {
            error: "已停止".into(),
        },
    ) {
        stdio::terminate(child).await;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    use super::*;

    fn tool_def(name: &str) -> ToolDef {
        ToolDef {
            name: name.into(),
            description: None,
            input_schema: json!({ "type": "object" }),
            annotations: None,
        }
    }

    /// 一个占位的进程句柄。
    ///
    /// `ServerState::Ready` 要拿着真实的子进程句柄（移除服务器时要杀整组），
    /// 所以枢纽这一层的测试绕不开起一个进程。用立刻退出的 `echo`：
    /// 句柄有效就够了，测的是锁，不是进程。
    fn dummy_child() -> Box<dyn process_wrap::tokio::ChildWrapper> {
        let mut cmd = tokio::process::Command::new("echo");
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        process_wrap::tokio::CommandWrap::from(cmd)
            .spawn()
            .expect("echo 在测试机器上总有")
    }

    /// 直接塞一个已就绪的服务器进枢纽，跳过 spawn + 握手。
    async fn insert_ready(hub: &McpHub, id: &str, client: Arc<Client>, tools: Vec<ToolDef>) {
        let spec = ServerSpec {
            id: id.to_owned(),
            command: "echo".into(),
            args: vec![],
            env: vec![],
        };
        hub.servers.lock().await.insert(
            id.to_owned(),
            Handle {
                spec,
                state: Arc::new(Mutex::new(ServerState::Ready {
                    client,
                    child: dummy_child(),
                    server_name: "fake 0".into(),
                    tools,
                })),
                cancel: CancellationToken::new(),
            },
        );
    }

    /// 假服务器：答 initialize 和**第一次** tools/list（回应之前先发一条
    /// "清单变了"通知），之后的 tools/list 挂着不回，并把"收到了"从
    /// `stalled` 通道说出来。
    ///
    /// 通知排在响应**前面**是为了不靠 sleep 同步：读循环顺序处理，
    /// 第一次 `list_tools()` 返回时通知一定已经生效。
    async fn hanging_client(stalled: tokio::sync::mpsc::UnboundedSender<()>) -> Arc<Client> {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (server_read, mut server_write) = tokio::io::split(server_io);

        tokio::spawn(async move {
            let mut lines = BufReader::new(server_read).lines();
            let mut listed = false;
            while let Ok(Some(line)) = lines.next_line().await {
                let msg: Value = match serde_json::from_str(&line) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                let Some(id) = msg.get("id").cloned() else {
                    continue;
                };
                let reply = match msg.get("method").and_then(Value::as_str) {
                    Some("initialize") => json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {
                            "protocolVersion": "2025-06-18",
                            "serverInfo": { "name": "fake", "version": "0" },
                            "capabilities": {}
                        }
                    }),
                    Some("tools/list") if !listed => {
                        listed = true;
                        let notice = json!({
                            "jsonrpc": "2.0", "method": "notifications/tools/list_changed"
                        });
                        let _ = server_write
                            .write_all(format!("{notice}\n").as_bytes())
                            .await;
                        json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": [] } })
                    }
                    // 第二次开始装死 —— 真实世界里就是服务器卡住了。
                    Some("tools/list") => {
                        let _ = stalled.send(());
                        continue;
                    }
                    _ => continue,
                };
                let _ = server_write
                    .write_all(format!("{reply}\n").as_bytes())
                    .await;
            }
        });

        let (r, w) = tokio::io::split(client_io);
        let (c, _) = Client::connect(r, w, Timeouts::default())
            .await
            .expect("握手");
        c
    }

    #[tokio::test]
    async fn 卡住的服务器不拖住整个枢纽() {
        // 早先 tools() 全程持着 servers 锁，刷新清单又是一次网络往返
        // （30 秒超时）。一个挂起的服务器把锁按住半分钟，statuses() 和
        // reconcile() 全排队 —— 设置页表现为整体卡死，而线索只指向
        // 某一个服务器。
        let (tx, mut stalled) = tokio::sync::mpsc::unbounded_channel();
        let client = hanging_client(tx).await;

        // 走一次正常的 list_tools：假服务器在回应之前先发了"清单变了"，
        // 于是接下来的 tools() 必定走刷新那条路 —— 也就是网络往返那条路。
        client.list_tools().await.expect("第一次照常");

        let hub = Arc::new(McpHub::new());
        insert_ready(&hub, "slow", client, vec![tool_def("t")]).await;

        let hub2 = Arc::clone(&hub);
        let refreshing = tokio::spawn(async move { hub2.tools().await });

        // 等到假服务器确认"刷新请求已经到了、我不回"，此刻 tools()
        // 正卡在网络等待上。
        stalled.recv().await.expect("刷新请求应该发出去了");

        tokio::time::timeout(Duration::from_secs(2), hub.statuses())
            .await
            .expect("statuses 被卡住的服务器按住了 —— 设置页会整体转圈");
        tokio::time::timeout(Duration::from_secs(2), hub.reconcile(vec![]))
            .await
            .expect("reconcile 被卡住的服务器按住了");

        refreshing.abort();
    }

    #[tokio::test]
    async fn 重名的工具只注册一个() {
        // 名字是权限规则的匹配键。两个工具共用一个名字意味着用户对着
        // 其中一个点的"总是允许"顺带放行了另一个，而弹窗里没提过它。
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let client = hanging_client(tx).await;

        let hub = McpHub::new();
        insert_ready(
            &hub,
            "srv",
            client,
            vec![tool_def("dup"), tool_def("dup"), tool_def("other")],
        )
        .await;

        let names: Vec<String> = hub.tools().await.iter().map(|t| t.name().into()).collect();
        assert_eq!(names, vec!["mcp__srv__dup", "mcp__srv__other"]);
    }

    #[test]
    fn 找不到命令时把_path_问题说清楚() {
        let e = std::io::Error::new(std::io::ErrorKind::NotFound, "os error 2");
        let s = spawn_error("npx", e);
        assert!(s.contains("找不到命令「npx」"), "{s}");
        assert!(s.contains("PATH"), "{s}");
    }

    #[test]
    fn 别的启动错误仍指向命令和参数() {
        let e = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied");
        let s = spawn_error("npx", e);
        assert!(s.contains("检查命令路径和参数"), "{s}");
        assert!(!s.contains("PATH"), "{s}");
    }
}
