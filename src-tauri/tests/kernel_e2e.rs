//! 宿主 ↔ 真内核的端到端链路。
//!
//! 这是"双进程真的能跑"的证据:AppState(RPC 客户端)→ 真 riot-kernel
//! 进程 → 会话轮次 → 事件经 stdout / Coalescer 回流前端 channel。
//! 内核自己的 stdio_smoke 验证的是内核单侧;这里验证的是宿主这一半
//! (KernelClient、事件分发、崩溃后自动重启)接上之后的整体。
//!
//! 模型端点指向本进程里的假 401 服务器:认证失败是不可恢复错误,
//! 轮子立刻以 Done{Error} 结束 —— 不打真网络、不吃重试退避,
//! 但整条 HTTP → provider → 主循环 → 事件流的路都走过了。

#![cfg(unix)]
#![allow(clippy::disallowed_methods)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use riot_host_lib::config::AppConfig;
use riot_host_lib::state::AppState;
use tauri::ipc::{Channel, InvokeResponseBody};

/// 确保内核二进制已构建(cargo test -p riot-host 不会连带 build 它)。
/// 运行期的嵌套 cargo build 没有锁冲突 —— test 运行时构建锁已释放。
fn ensure_kernel_built() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace 根")
        .to_path_buf();
    let bin = root.join("target/debug/riot-kernel");
    if bin.exists() {
        return;
    }
    let status = std::process::Command::new("cargo")
        .args(["build", "-p", "riot-kernel"])
        .current_dir(&root)
        .status()
        .expect("跑得动 cargo");
    assert!(status.success(), "内核二进制构建失败");
}

/// 一个只会回 401 的假模型端点。
///
/// 401 是不可恢复错误(认证失败不重试),轮子拿到它立刻收尾 ——
/// 测试快且确定,同时整条真实的 HTTP 栈都走过。
async fn fake_401_endpoint() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑本地端口");
    let addr = listener.local_addr().expect("有地址");
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 8192];
                let _ = sock.read(&mut buf).await;
                let body = r#"{"error":{"message":"e2e 假端点:key 无效","type":"invalid_request_error"}}"#;
                let resp = format!(
                    "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len(),
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}")
}

fn test_config(base_url: &str) -> AppConfig {
    // 走 JSON 构造(和 config.json 同一形状),避免逐字段追 ProviderConfig
    // 的非公开细节。key 从环境变量来 —— 内容随便,能通过"非空"检查即可,
    // 假端点反正回 401。
    serde_json::from_str(&format!(
        r#"{{
            "providers": [{{
                "id": "e2e", "name": "e2e", "protocol": "openai",
                "baseUrl": "{base_url}",
                "models": ["fake-model"],
                "apiKeyEnv": "RIOT_E2E_FAKE_KEY"
            }}],
            "activeProvider": "e2e",
            "activeModel": "fake-model"
        }}"#
    ))
    .expect("测试配置该能解析")
}

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("riot-e2e-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).expect("建临时目录");
    d
}

/// 收集事件的前端 channel 替身。Channel 的回调拿到的是序列化后的 body,
/// 判断 Done 靠找 `"type":"done"`(AgentEvent 的 serde tag)。
fn done_probe() -> (Channel<riot_protocol::event::AgentEvent>, Arc<AtomicBool>) {
    let saw_done = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&saw_done);
    let ch = Channel::new(move |body: InvokeResponseBody| {
        if let InvokeResponseBody::Json(s) = &body
            && s.contains(r#""type":"done""#)
        {
            flag.store(true, Ordering::SeqCst);
        }
        Ok(())
    });
    (ch, saw_done)
}

async fn eventually(limit: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + limit;
    while std::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    cond()
}

#[tokio::test]
async fn 完整链路_建会话_跑轮_done_事件回流() {
    ensure_kernel_built();
    // 豁免理由:测试进程自己的环境,值是假的。
    unsafe { std::env::set_var("RIOT_E2E_FAKE_KEY", "fake-key-for-e2e") };

    let base = fake_401_endpoint().await;
    let cfg_dir = temp_dir("link");
    let ws = temp_dir("link-ws");

    let state = AppState::restore_at(cfg_dir.join("config.json"));
    state.spawn_host_bridge();
    state.set_config(test_config(&base)).await;

    let info = state
        .create_session(ws.to_str().expect("utf8"))
        .await
        .expect("建会话(纯宿主,不需要内核)");

    let (ch, saw_done) = done_probe();
    assert!(state.attach_sink(info.id.clone(), 1, ch).await);

    // 这一步会:惰性水合(session.resume)→ 打包 TurnConfig → turn.submit
    // → 内核跑轮 → 401 → Done{Error} 经 event.agent 回流。
    let queued = state
        .send_turn(&info.id, "你好", vec![], vec![])
        .await
        .expect("提交该被内核接受");
    assert!(queued.is_none(), "空闲会话该直接开轮,不是排队");

    assert!(
        eventually(Duration::from_secs(20), || saw_done.load(Ordering::SeqCst)).await,
        "Done 事件该穿过 内核 stdout → KernelClient → Coalescer → channel 整条链路回来"
    );

    state.shutdown().await;
}

#[tokio::test]
async fn 内核关停后下一次调用自动重启() {
    ensure_kernel_built();
    unsafe { std::env::set_var("RIOT_E2E_FAKE_KEY", "fake-key-for-e2e") };

    let base = fake_401_endpoint().await;
    let cfg_dir = temp_dir("restart");
    let ws = temp_dir("restart-ws");

    let state = AppState::restore_at(cfg_dir.join("config.json"));
    state.spawn_host_bridge();
    state.set_config(test_config(&base)).await;
    let info = state.create_session(ws.to_str().expect("utf8")).await.expect("会话");

    // 第一次触达:拉起内核(history 走 resume,不需要模型)。
    let h = state.history(&info.id).await.expect("第一次水合");
    assert!(h.messages.is_empty());

    // 内核进程退出(四步关闭)。宿主的死亡检测会把 hydrated 清掉。
    state.shutdown().await;

    // 下一次调用要能自动重启内核并重新水合 —— 用户视角:内核崩了,
    // 下一条操作照常工作,最多多等一个退避间隔。
    let h = state.history(&info.id).await.expect("重启后照常工作");
    assert!(h.messages.is_empty());

    state.shutdown().await;
}
