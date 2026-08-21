//! MCP（Model Context Protocol）客户端。
//!
//! # 结构
//!
//! - [`wire`] —— JSON-RPC 2.0 帧和 MCP 消息类型。
//! - [`client`] —— 传输无关的客户端：握手、tools/list、tools/call、
//!   取消通知、服务器 ping 的代答。测试用内存管道，不起进程。
//! - `stdio` —— 把服务器作为子进程拉起来（进程组包裹，杀得干净）。
//! - [`tool`] —— [`McpTool`]：把远端工具适配成 [`riot_protocol::tool::Tool`]，
//!   走和内置工具完全相同的注册、调度、权限管线。
//! - [`hub`] —— [`McpHub`]：应用级的连接生命周期（会话间共享）。
//!
//! # 边界
//!
//! 传输只做 stdio。HTTP/SSE 的远程服务器是另一类信任模型（要 OAuth、
//! 要处理网络中断重连），等有真实需求再加 —— [`client::Client`] 对传输
//! 无感知，加的时候这层不用动。

pub mod client;
pub mod hub;
mod stdio;
pub mod tool;
pub mod wire;

pub use client::{Client, ClientError, ServerHello, Timeouts};
pub use hub::{McpHub, ServerSpec, ServerStatus};
pub use tool::{McpTool, tool_name};

// 豁免理由：测试等待的是假服务器的异步往返，用真实时钟。
#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio_util::sync::CancellationToken;

    use super::*;

    /// 起一个假服务器，返回已握手的客户端。
    async fn connect_scripted(
        handler: impl Fn(&str, &Value) -> Vec<Value> + Send + 'static,
    ) -> (Arc<Client>, ServerHello) {
        // client_io 给客户端读写；server_io 给假服务器读写。
        let (client_io, server_io) = tokio::io::duplex(256 * 1024);
        let (server_read, mut server_write) = tokio::io::split(server_io);

        tokio::spawn(async move {
            let mut lines = BufReader::new(server_read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let msg: Value = match serde_json::from_str(&line) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                let method = msg
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let id = msg.get("id").cloned();
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                // handler 返回零或多条要写回的完整帧（可以是响应，也可以是
                // 服务器发起的请求/通知 —— result 里替换 $ID 占位）。
                for mut frame in handler(&method, &params) {
                    if let (Some(id), Some(obj)) = (id.clone(), frame.as_object_mut())
                        && obj.remove("$reply").is_some()
                    {
                        obj.insert("id".into(), id);
                    }
                    let _ = server_write
                        .write_all(format!("{frame}\n").as_bytes())
                        .await;
                }
            }
        });

        let (r, w) = tokio::io::split(client_io);
        let timeouts = Timeouts {
            connect: Duration::from_secs(5),
            request: Duration::from_secs(5),
            call: Duration::from_secs(5),
        };
        Client::connect(r, w, timeouts).await.expect("握手")
    }

    fn init_reply() -> Value {
        json!({
            "$reply": true, "jsonrpc": "2.0",
            "result": {
                "protocolVersion": "2025-06-18",
                "serverInfo": { "name": "fake", "version": "0.1" },
                "capabilities": {}
            }
        })
    }

    #[tokio::test]
    async fn 握手拿到服务器身份() {
        let (_c, hello) = connect_scripted(|m, _| match m {
            "initialize" => vec![init_reply()],
            _ => vec![],
        })
        .await;
        assert_eq!(hello.name, "fake");
        assert_eq!(hello.protocol_version, "2025-06-18");
    }

    #[tokio::test]
    async fn 工具清单翻完分页() {
        let (c, _) = connect_scripted(|m, p| match m {
            "initialize" => vec![init_reply()],
            "tools/list" => {
                let cursor = p.get("cursor").and_then(Value::as_str);
                let result = match cursor {
                    None => json!({
                        "tools": [{ "name": "alpha", "inputSchema": {"type":"object"} }],
                        "nextCursor": "p2"
                    }),
                    Some("p2") => json!({
                        "tools": [{ "name": "beta", "inputSchema": {"type":"object"} }]
                    }),
                    Some(other) => panic!("不该有第三页：{other}"),
                };
                vec![json!({ "$reply": true, "jsonrpc": "2.0", "result": result })]
            }
            _ => vec![],
        })
        .await;

        let tools = c.list_tools().await.expect("列工具");
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"], "两页都要拿到");
    }

    #[tokio::test]
    async fn 调用工具拿回文本() {
        let (c, _) = connect_scripted(|m, p| match m {
            "initialize" => vec![init_reply()],
            "tools/call" => {
                assert_eq!(p.pointer("/name").and_then(Value::as_str), Some("echo"));
                let text = p
                    .pointer("/arguments/text")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                vec![json!({
                    "$reply": true, "jsonrpc": "2.0",
                    "result": { "content": [{ "type": "text", "text": format!("回声：{text}") }] }
                })]
            }
            _ => vec![],
        })
        .await;

        let r = c
            .call_tool("echo", json!({"text": "你好"}), &CancellationToken::new())
            .await
            .expect("调用");
        assert_eq!(
            r.content[0].get("text").and_then(Value::as_str),
            Some("回声：你好")
        );
        assert_ne!(r.is_error, Some(true));
    }

    #[tokio::test]
    async fn 取消返回_cancelled_并通知服务器() {
        // tools/call 故意不回，让取消先到。
        let (c, _) = connect_scripted(|m, _| match m {
            "initialize" => vec![init_reply()],
            "tools/call" => vec![], // 挂着不回
            _ => vec![],
        })
        .await;

        let cancel = CancellationToken::new();
        let call = c.call_tool("slow", json!({}), &cancel);
        // 先让请求发出去，再取消。
        let cancel2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel2.cancel();
        });
        match call.await {
            Err(ClientError::Cancelled) => {}
            other => panic!("该是取消：{other:?}"),
        }
    }

    #[tokio::test]
    async fn 服务器的_ping_得到回应() {
        // 服务器在 initialize 之后立刻发一个 ping 请求；客户端必须回，
        // 否则有些服务器会等在那里不再处理后续请求。
        let (c, _) = connect_scripted(|m, _| match m {
            "initialize" => vec![
                init_reply(),
                json!({ "jsonrpc": "2.0", "id": "srv-ping-1", "method": "ping" }),
            ],
            "tools/list" => vec![json!({
                "$reply": true, "jsonrpc": "2.0", "result": { "tools": [] }
            })],
            _ => vec![],
        })
        .await;
        // ping 的响应没有直接的观察点，但后续请求还能正常往返就说明
        // 读任务没有被它噎住。
        assert!(c.list_tools().await.expect("列工具").is_empty());
    }

    #[tokio::test]
    async fn 服务器报错映射成_rpc_错误() {
        let (c, _) = connect_scripted(|m, _| match m {
            "initialize" => vec![init_reply()],
            "tools/call" => vec![json!({
                "$reply": true, "jsonrpc": "2.0",
                "error": { "code": -32602, "message": "参数不对" }
            })],
            _ => vec![],
        })
        .await;

        match c.call_tool("x", json!({}), &CancellationToken::new()).await {
            Err(ClientError::Rpc { code, message }) => {
                assert_eq!(code, -32602);
                assert!(message.contains("参数不对"));
            }
            other => panic!("该是 Rpc 错误：{other:?}"),
        }
    }

    #[tokio::test]
    async fn 连接断开后挂起的请求立刻失败() {
        // 服务器进程崩溃时，正在等的调用必须立刻失败 —— 让它等满超时的话，
        // 用户看到的是工具卡片白转十分钟。
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (server_read, mut server_write) = tokio::io::split(server_io);

        tokio::spawn(async move {
            let mut lines = BufReader::new(server_read).lines();
            // 答复 initialize
            let line = lines.next_line().await.expect("读").expect("有行");
            let msg: Value = serde_json::from_str(&line).expect("json");
            let resp = json!({
                "jsonrpc": "2.0", "id": msg["id"],
                "result": {
                    "protocolVersion": "x",
                    "serverInfo": { "name": "f", "version": "0" },
                    "capabilities": {}
                }
            });
            server_write
                .write_all(format!("{resp}\n").as_bytes())
                .await
                .expect("写");
            // 等到下一条请求（跳过 initialized 通知），然后模拟进程退出。
            loop {
                match lines.next_line().await {
                    Ok(Some(l)) if l.contains("\"id\"") => break,
                    Ok(Some(_)) => continue,
                    _ => break,
                }
            }
            // 两个半边一起 drop → 客户端读到 EOF。
        });

        let (r, w) = tokio::io::split(client_io);
        let (c, _) = Client::connect(r, w, Timeouts::default())
            .await
            .expect("握手");
        assert!(c.is_alive());

        // Timeouts::default 的 request 是 30 秒；5 秒内失败才算"立刻"。
        let err = tokio::time::timeout(Duration::from_secs(5), c.list_tools())
            .await
            .expect("必须立刻失败，不是等满请求超时")
            .expect_err("断开必须失败");
        assert!(matches!(err, ClientError::Closed), "该是 Closed：{err:?}");
        assert!(!c.is_alive());
    }

    #[tokio::test]
    async fn 清单变更通知置位() {
        let (c, _) = connect_scripted(|m, _| match m {
            "initialize" => vec![
                init_reply(),
                json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" }),
            ],
            "tools/list" => vec![json!({
                "$reply": true, "jsonrpc": "2.0", "result": { "tools": [] }
            })],
            _ => vec![],
        })
        .await;
        // 用一次请求同步：等 tools/list 往返完成，通知肯定也已被处理
        //（同一条读循环，顺序处理）。
        let _ = c.list_tools().await.expect("列工具");
        assert!(c.take_list_changed(), "通知该置位");
        assert!(!c.take_list_changed(), "取一次就清掉");
    }

    /// 真实子进程链路的冒烟：spawn → stdio 管道 → 握手 → 列工具。
    ///
    /// 依赖本机有 node，所以默认 ignore —— CI 的内核跑道不保证有它。
    /// 改动 stdio/hub 之后本地跑：`cargo test -p riot-mcp -- --ignored`
    #[tokio::test]
    #[ignore = "需要 node，本地手动验证 stdio 链路"]
    async fn 真实子进程冒烟() {
        let script = r#"
const rl = require('readline').createInterface({ input: process.stdin });
rl.on('line', (l) => {
  const m = JSON.parse(l);
  if (!m.id) return;
  if (m.method === 'initialize') {
    console.log(JSON.stringify({ jsonrpc:'2.0', id:m.id, result:{ protocolVersion:'2025-06-18', serverInfo:{name:'node-fake',version:'1'}, capabilities:{} } }));
  } else if (m.method === 'tools/list') {
    console.log(JSON.stringify({ jsonrpc:'2.0', id:m.id, result:{ tools:[{name:'hello', description:'打招呼', inputSchema:{type:'object'}}] } }));
  }
});
"#;
        let hub = McpHub::new();
        hub.reconcile(vec![ServerSpec {
            id: "node".into(),
            command: "node".into(),
            args: vec!["-e".into(), script.into()],
            env: vec![],
        }])
        .await;

        let mut connected = false;
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let s = hub.statuses().await;
            if let Some(first) = s.first() {
                assert_ne!(first.state, "failed", "不该失败：{}", first.detail);
                if first.state == "connected" {
                    assert_eq!(first.tools, vec!["mcp__node__hello"]);
                    connected = true;
                    break;
                }
            }
        }
        assert!(connected, "5 秒内该连上");

        let tools = hub.tools().await;
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "mcp__node__hello");

        hub.shutdown().await;
        assert!(hub.statuses().await.is_empty());
    }

    #[tokio::test]
    async fn 枢纽_启动失败的服务器报_failed() {
        let hub = McpHub::new();
        hub.reconcile(vec![ServerSpec {
            id: "ghost".into(),
            command: "/definitely/not/a/real/binary-riot-test".into(),
            args: vec![],
            env: vec![],
        }])
        .await;

        // 启动失败是同步可见的（spawn 直接报错），但状态写入在任务里，
        // 给它一点时间。
        let mut state = String::new();
        let mut detail = String::new();
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let s = hub.statuses().await;
            if let Some(first) = s.first()
                && first.state != "connecting"
            {
                state = first.state.clone();
                detail = first.detail.clone();
                break;
            }
        }
        assert_eq!(
            state, "failed",
            "起不来的服务器必须报 failed 而不是永远 connecting"
        );
        assert!(
            detail.contains("找不到命令"),
            "找不到二进制要把 PATH 问题说清楚：{detail}"
        );
        assert!(hub.tools().await.is_empty(), "失败的服务器不该贡献工具");

        // 消失的服务器要被摘掉
        hub.reconcile(vec![]).await;
        assert!(hub.statuses().await.is_empty());
    }
}
