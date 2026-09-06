//! 一条 WebSocket 连接的生命周期：握手 → 分发 → 收尾。
//!
//! 每条连接在宿主里是一个独立的**观看者**（`remote:<n>`）。它订阅的会话
//! 事件、挂上的终端、打开的浏览器面板都记在这个 id 下；连接一断，
//! [`AppState::detach_viewer`] 一次收干净 —— 不然断掉的手机会让浏览器面板
//! 一直对着空处编码。
//!
//! # 出口队列
//!
//! 命令应答、通道消息、全局事件都汇进一条无界 mpsc，由写任务按序发出去。
//! 无界是因为发送方是同步闭包（`Channel::send`），阻塞不了；而不能丢的东西
//! （`Done` 事件）又一条都不能少。两道护栏防它涨到天上去：
//!
//! 1. **原始字节的通道消息最多积压两条**（每通道计）。这类消息只有浏览器
//!    面板的画面帧，每帧自成一体，网速跟不上时丢掉旧帧比排队播放旧帧好。
//! 2. **总积压超过上限就断连接**。这时候客户端已经慢到没法用了，断开让它
//!    重连、按快照对账（前端 `ensureLive`），比拖着一条越来越滞后的连接强。

// 宿主层：真实 socket、真实时钟。见 clippy.toml。
#![allow(clippy::disallowed_methods)]

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message, Utf8Bytes, WebSocket};
use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Listener, Manager};
use tokio::sync::{mpsc, watch};

use super::protocol::{BIN_CALL_RESULT, BIN_CHANNEL, ClientFrame, ServerFrame, binary_frame};
use super::{Shared, dispatch};
use crate::state::AppState;

/// 鉴权帧最多等多久。连上不说话的连接占着句柄没有意义。
const AUTH_DEADLINE: Duration = Duration::from_secs(10);
/// 服务端心跳间隔。浏览器会自动回 pong，不用前端配合。
const PING_EVERY: Duration = Duration::from_secs(20);
/// 这么久没收到任何帧（含 pong）就当对端死了。手机锁屏、切 Wi-Fi 时 TCP
/// 不一定会给 FIN，不主动断的话这条连接会一直挂着。
const IDLE_LIMIT: Duration = Duration::from_secs(65);
/// 同一通道最多积压几条原始字节消息（画面帧）。
const RAW_BACKLOG_PER_CHANNEL: usize = 2;
/// 总积压上限（条数）。到这里说明客户端已经几十秒没消化东西了。
const HARD_BACKLOG: usize = 20_000;

/// 前端要听的全局事件（对应 `app.emit`）。加一个就在这里补一行 ——
/// 桌面那边是 `listen(name)`，这边是 `listen_any(name)` 再转成 Event 帧。
const FORWARDED_EVENTS: &[&str] = &["schedule_run", "schedule_changed", "sessions_changed"];
// schedule_*：定时任务面板；sessions_changed：侧栏会话/项目列表。
// 加事件时这里补一行，桌面那边是前端 listen(name)，这边是 listen_any 转 Event 帧。

enum Out {
    Msg(Message),
    /// 通道上的原始字节。带通道号是为了写完之后把该通道的积压计数减回去。
    Raw { ch: u32, msg: Message },
}

/// 连接的出口。`Send + Sync`，命令处理任务和各条 Channel 都拿着它。
pub struct Outbound {
    tx: mpsc::UnboundedSender<Out>,
    pending: AtomicUsize,
    raw_pending: Mutex<HashMap<u32, usize>>,
    closed: AtomicBool,
}

impl Outbound {
    fn enqueue(&self, out: Out) -> Result<(), ()> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(());
        }
        let n = self.pending.fetch_add(1, Ordering::Relaxed) + 1;
        if n > HARD_BACKLOG {
            // 客户端消化不动了。这一条不发，把它的计数退回去，然后关连接。
            self.pending.fetch_sub(1, Ordering::Relaxed);
            tracing::warn!(pending = n - 1, "远程连接积压过多，断开让它重连");
            self.close();
            return Err(());
        }
        self.tx.send(out).map_err(|_| ())
    }

    /// 标记关闭并排一条 Close 帧。写任务发完它就收尾。
    ///
    /// `[约束]` Close 也要计进 `pending`：写任务对每条发出去的消息都减一，
    /// 这里不加的话计数会下溢 —— 现在没人在关闭之后读它，但一个错的
    /// 计数迟早会让人在排查积压问题时走弯路。幂等，第二次调什么都不做。
    fn close(&self) {
        if self.closed.swap(true, Ordering::Relaxed) {
            return;
        }
        self.pending.fetch_add(1, Ordering::Relaxed);
        let _ = self.tx.send(Out::Msg(Message::Close(None)));
    }

    fn send_text(&self, s: String) -> Result<(), ()> {
        self.enqueue(Out::Msg(Message::Text(Utf8Bytes::from(s))))
    }

    fn send_frame(&self, f: &ServerFrame<'_>) -> Result<(), ()> {
        match serde_json::to_string(f) {
            Ok(s) => self.send_text(s),
            Err(e) => {
                tracing::warn!(error = %e, "服务端帧序列化失败");
                Err(())
            }
        }
    }

    /// 通道上的一条 JSON 消息。手拼而不是走 [`ServerFrame`]：`data` 已经是
    /// 序列化好的 JSON（`Channel::send` 给的），再 parse 成 `Value` 只为重新
    /// 序列化一遍，在 token 流上是纯浪费。
    fn send_channel_json(&self, ch: u32, json: &str) -> Result<(), ()> {
        self.send_text(format!(r#"{{"t":"channel","ch":{ch},"data":{json}}}"#))
    }

    fn send_channel_raw(&self, ch: u32, bytes: &[u8]) -> Result<(), ()> {
        {
            let mut g = self.raw_pending.lock().expect("积压表锁");
            let n = g.entry(ch).or_insert(0);
            if *n >= RAW_BACKLOG_PER_CHANNEL {
                // 丢这一帧。不算失败 —— 通道还活着，只是这一帧没人等得及看。
                return Ok(());
            }
            *n += 1;
        }
        let msg = Message::Binary(binary_frame(BIN_CHANNEL, ch, bytes).into());
        self.enqueue(Out::Raw { ch, msg })
    }

    fn send_call_raw(&self, id: u64, bytes: &[u8]) -> Result<(), ()> {
        // 请求号是 u64，二进制帧头只给 u32：前端的计数器从 1 起，一条连接
        // 发 40 亿条请求之前早就重连过了。溢出就报错而不是静默截断。
        let Ok(id32) = u32::try_from(id) else {
            return self.send_frame(&ServerFrame::Err {
                id,
                error: "请求号溢出，请刷新页面".to_owned(),
            });
        };
        self.enqueue(Out::Msg(Message::Binary(
            binary_frame(BIN_CALL_RESULT, id32, bytes).into(),
        )))
    }
}

/// 命令分发时用到的连接上下文。
pub struct ConnCtx {
    pub viewer: String,
    pub out: Arc<Outbound>,
}

impl ConnCtx {
    /// 把 args 里的一个通道号变成一条真正的 [`Channel`]：宿主那头照常
    /// `send`，消息落到这条连接的出口队列。连接断了 `send` 报错，持有它的
    /// 一方（终端读线程、浏览器扇出）据此把出口摘掉。
    pub fn channel<T>(&self, id: u32) -> Channel<T> {
        let out = Arc::clone(&self.out);
        Channel::new(move |body| {
            let sent = match body {
                InvokeResponseBody::Json(s) => out.send_channel_json(id, &s),
                InvokeResponseBody::Raw(b) => out.send_channel_raw(id, &b),
            };
            sent.map_err(|()| {
                tauri::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "远程连接已断开",
                ))
            })
        })
    }
}

/// 跑完一条连接的整个生命周期。返回即连接结束。
///
/// `kick` 翻成 true(或发送端被丢掉)= 服务在停,这条连接要主动退出。
pub async fn serve(
    socket: WebSocket,
    app: AppHandle,
    shared: Arc<Shared>,
    peer: IpAddr,
    mut kick: watch::Receiver<bool>,
) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // ── 1. 鉴权 ──────────────────────────────────────────
    let first = tokio::time::timeout(AUTH_DEADLINE, ws_rx.next()).await;
    let token = match first {
        Ok(Some(Ok(Message::Text(t)))) => match serde_json::from_str::<ClientFrame>(&t) {
            Ok(ClientFrame::Auth { token }) => Some(token),
            _ => None,
        },
        _ => None,
    };
    let Some(token) = token else {
        let _ = ws_tx
            .send(Message::Text(
                serde_json::to_string(&ServerFrame::Denied {
                    reason: "第一帧必须是鉴权",
                })
                .unwrap_or_default()
                .into(),
            ))
            .await;
        let _ = ws_tx.close().await;
        return;
    };
    if !shared.throttle.allows(peer) {
        deny(&mut ws_tx, "尝试太频繁，请一分钟后再试").await;
        return;
    }
    let expected = shared.token.lock().expect("令牌锁").clone();
    let ok = expected
        .as_deref()
        .is_some_and(|exp| super::auth::token_matches(exp, &token));
    if !ok {
        shared.throttle.record_failure(peer);
        tracing::warn!(%peer, "远程连接鉴权失败");
        deny(&mut ws_tx, "令牌不对").await;
        return;
    }
    shared.throttle.record_success(peer);

    // ── 2. 建上下文 ──────────────────────────────────────
    let n = shared.next_conn.fetch_add(1, Ordering::Relaxed);
    let viewer = format!("remote:{n}");
    let (tx, mut rx) = mpsc::unbounded_channel::<Out>();
    let out = Arc::new(Outbound {
        tx,
        pending: AtomicUsize::new(0),
        raw_pending: Mutex::default(),
        closed: AtomicBool::new(false),
    });
    let ctx = Arc::new(ConnCtx {
        viewer: viewer.clone(),
        out: Arc::clone(&out),
    });
    let version = app.package_info().version.to_string();
    if out
        .send_frame(&ServerFrame::Ready {
            viewer: &viewer,
            boot: &shared.boot_id,
            version: &version,
        })
        .is_err()
    {
        return;
    }
    shared.connections.fetch_add(1, Ordering::Relaxed);
    tracing::info!(%peer, viewer, "远程连接已建立");

    // 全局事件转发。桌面那边是前端 listen；这里由宿主替它听，转成 Event 帧。
    let listeners: Vec<tauri::EventId> = FORWARDED_EVENTS
        .iter()
        .map(|name| {
            let out = Arc::clone(&out);
            let name_owned = (*name).to_owned();
            app.listen_any(*name, move |ev| {
                let payload: Value = serde_json::from_str(ev.payload()).unwrap_or(Value::Null);
                let _ = out.send_frame(&ServerFrame::Event {
                    name: &name_owned,
                    payload,
                });
            })
        })
        .collect();

    // ── 3. 写任务 ────────────────────────────────────────
    let writer_out = Arc::clone(&out);
    let writer = tokio::spawn(async move {
        while let Some(item) = rx.recv().await {
            let msg = match item {
                Out::Msg(m) => m,
                Out::Raw { ch, msg } => {
                    if let Some(n) = writer_out.raw_pending.lock().expect("积压表锁").get_mut(&ch) {
                        *n = n.saturating_sub(1);
                    }
                    msg
                }
            };
            writer_out.pending.fetch_sub(1, Ordering::Relaxed);
            let closing = matches!(msg, Message::Close(_));
            if ws_tx.send(msg).await.is_err() || closing {
                break;
            }
        }
        let _ = ws_tx.close().await;
    });

    // ── 4. 读循环 ────────────────────────────────────────
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_heard = tokio::time::Instant::now();
    loop {
        tokio::select! {
            frame = ws_rx.next() => {
                let Some(Ok(frame)) = frame else { break };
                last_heard = tokio::time::Instant::now();
                match frame {
                    Message::Text(t) => {
                        match serde_json::from_str::<ClientFrame>(&t) {
                            Ok(ClientFrame::Call { id, cmd, args }) => {
                                let app = app.clone();
                                let ctx = Arc::clone(&ctx);
                                // 每条命令一个任务：浏览器跳转合法地要等几十秒，
                                // 不能堵住后面的停止键。
                                tokio::spawn(async move {
                                    let result = dispatch::dispatch(&app, &ctx, &cmd, args).await;
                                    let _ = match result {
                                        Ok(dispatch::Outcome::Json(v)) => {
                                            ctx.out.send_frame(&ServerFrame::Ok { id, result: v })
                                        }
                                        Ok(dispatch::Outcome::Raw(bytes)) => ctx.out.send_call_raw(id, &bytes),
                                        Err(error) => ctx.out.send_frame(&ServerFrame::Err { id, error }),
                                    };
                                });
                            }
                            Ok(ClientFrame::Ping) => {
                                let _ = out.send_frame(&ServerFrame::Pong);
                            }
                            Ok(ClientFrame::Auth { .. }) => {
                                // 已经过了鉴权还发，多半是前端 bug。忽略。
                            }
                            Err(e) => {
                                tracing::debug!(error = %e, "远程帧解析失败");
                            }
                        }
                    }
                    Message::Close(_) => break,
                    // Ping/Pong/Binary：浏览器不会发二进制上来；pong 只为刷 last_heard。
                    _ => {}
                }
            }
            _ = ping.tick() => {
                if last_heard.elapsed() > IDLE_LIMIT {
                    tracing::info!(viewer, "远程连接超时无响应，断开");
                    break;
                }
                if out.enqueue(Out::Msg(Message::Ping(Vec::new().into()))).is_err() {
                    break;
                }
            }
            // 服务在停（关开关、换端口 / 绑定）。changed() 出 Err 是发送端
            // 没了，同样算停。前端会按退避重连；服务真关了它就一直重连不上，
            // 换了端口它连的还是旧地址 —— 两种都是"用户在桌面上做了决定"的
            // 直接后果，这里不替它兜。
            r = kick.changed() => {
                if r.is_err() || *kick.borrow() {
                    tracing::info!(viewer, "远程服务停止，断开连接");
                    break;
                }
            }
        }
    }

    // ── 5. 收尾 ──────────────────────────────────────────
    // 先标记关闭（之后的 send 全部失败在入口），再摘出口，最后等写任务把
    // Close 帧发出去。摘出口的过程里可能还有事件想往这条线上发，让它们
    // 失败在 send 上而不是写到一个半关闭的 socket 里。close() 幂等：积压
    // 断连那条路已经调过一次的话这里什么都不做。
    out.close();
    for id in listeners {
        app.unlisten(id);
    }
    app.state::<AppState>().inner().detach_viewer(&viewer).await;
    let _ = writer.await;
    shared.connections.fetch_sub(1, Ordering::Relaxed);
    tracing::info!(viewer, "远程连接已关闭");
}

async fn deny(
    ws_tx: &mut futures::stream::SplitSink<WebSocket, Message>,
    reason: &str,
) {
    let _ = ws_tx
        .send(Message::Text(
            serde_json::to_string(&ServerFrame::Denied { reason })
                .unwrap_or_default()
                .into(),
        ))
        .await;
    let _ = ws_tx.close().await;
}
