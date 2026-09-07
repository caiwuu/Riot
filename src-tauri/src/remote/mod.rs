//! 远程访问（网页版）。
//!
//! 宿主进程里起一个 HTTP + WebSocket 服务，浏览器加载**同一份**前端产物，
//! bridge 层把命令走 WebSocket 而不是 Tauri IPC（见 `src/bridge/transport/`）。
//! 于是桌面窗口和手机上的网页是同一个 Riot 的两个观看者：看同一批会话、
//! 同一个终端面板、同一个浏览器面板。
//!
//! # 分层
//!
//! ```text
//! protocol.rs   线协议：帧类型、通道占位、二进制帧格式
//! auth.rs       令牌、来源校验、失败限速
//! server.rs     axum 路由：静态资源、/ws 握手
//! conn.rs       一条连接：鉴权 → 读写循环 → 收尾（detach_viewer）
//! dispatch.rs   命令名 → lib.rs 里同一个处理函数
//! mod.rs        生命周期：按配置起停、令牌落盘、状态与二维码
//! ```
//!
//! # 为什么放在宿主里而不是内核
//!
//! 网页版要操作的不只是会话：项目列表、终端、浏览器面板、设置、定时任务，
//! 这些的权威都在宿主（`AppState`），内核只有会话运行时。放内核的话等于
//! 把宿主重写一遍。放宿主里则**零业务代码**：分发表调的是桌面命令同一份
//! 函数，`AppState` 一行没改语义 —— 只把"事件出口"从一份变成按观看者一份。
//!
//! # 安全模型（一句话）
//!
//! 令牌 = 主人身份。拿到令牌的人能做桌面前的人能做的一切。所以：默认关；
//! 令牌只在本机 `auth.json`（0600）和主人扫的二维码里出现；来源校验挡住
//! 浏览器里的第三方页面；失败限速让误配不刷屏。详见 auth.rs 模块说明。

// 宿主层：真实 socket、真实随机源。见 clippy.toml。
#![allow(clippy::disallowed_methods)]

pub mod auth;
mod conn;
mod dispatch;
mod protocol;
mod server;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::{Arc, Mutex};

use riot_protocol::{UiError, ui_error};
use serde::Serialize;
use tauri::AppHandle;
use tokio::sync::{oneshot, watch};

use crate::config::{REMOTE_TOKEN_KEY, RemoteBind, RemoteConfig};

/// 所有连接共享的状态。
pub struct Shared {
    /// 当前有效令牌。`None` = 还没生成（服务不该在这种状态下接受连接）。
    pub token: Mutex<Option<String>>,
    pub throttle: auth::Throttle,
    /// 本次进程启动的随机标识，见 [`protocol::ServerFrame::Ready`]。
    pub boot_id: String,
    pub next_conn: AtomicU64,
    pub connections: AtomicUsize,
    pub allowed_origins: Vec<String>,
}

/// 一个正在监听的服务。
struct Running {
    bind: RemoteBind,
    port: u16,
    addr: SocketAddr,
    /// 让 accept 循环停下(axum 的 graceful shutdown)。
    stop: Option<oneshot::Sender<()>>,
    /// 让已经升级成 WebSocket 的连接也退出。graceful shutdown 管不到它们:
    /// 升级之后 hyper 的连接 future 就结束了,WebSocket 活在 axum 另起的
    /// 任务里。不发这个,"关掉开关"只是不再收新连接,手机上那页照常能跑命令。
    kick: watch::Sender<bool>,
    /// serve 任务本身。停的时候等它,listener 才算真的释放了。
    task: tokio::task::JoinHandle<()>,
}

/// 远程服务的总开关。Tauri `manage` 一份，`set_config` 后调 [`Remote::apply`]。
pub struct Remote {
    shared: Arc<Shared>,
    running: tokio::sync::Mutex<Option<Running>>,
    /// 上次启动失败的原因（端口被占之类）。设置页要能看到。
    last_error: Mutex<Option<UiError>>,
}

impl Default for Remote {
    fn default() -> Self {
        Self {
            shared: Arc::new(Shared {
                token: Mutex::new(None),
                throttle: auth::Throttle::default(),
                boot_id: auth::generate_token(),
                next_conn: AtomicU64::new(1),
                connections: AtomicUsize::new(0),
                allowed_origins: auth::extra_allowed_origins(),
            }),
            running: tokio::sync::Mutex::new(None),
            last_error: Mutex::new(None),
        }
    }
}

impl Remote {
    /// 让服务状态对齐配置：该开的开、该关的关、端口/绑定变了就重起。
    /// 幂等，启动和每次保存设置后都调。
    pub async fn apply(&self, app: &AppHandle, cfg: &RemoteConfig) {
        let mut g = self.running.lock().await;
        if !cfg.enabled {
            if let Some(r) = g.take() {
                stop(r).await;
                tracing::info!("远程访问已关闭");
            }
            *self.last_error.lock().expect("错误锁") = None;
            return;
        }
        if let Some(r) = g.as_ref()
            && r.bind == cfg.bind
            && r.port == cfg.port
        {
            return;
        }
        if let Some(r) = g.take() {
            // 等旧的真正退出再绑新地址。只发信号不等的话,端口不变、只把
            // 「仅本机」改成「局域网」时新旧 socket 同端口,会撞上 EADDRINUSE。
            stop(r).await;
        }

        // 令牌：有就用，没有就生成一枚落进 auth.json。
        let token = match crate::config::load_secret(REMOTE_TOKEN_KEY) {
            Some(t) => t,
            None => {
                let t = auth::generate_token();
                if let Err(e) = crate::config::save_key(REMOTE_TOKEN_KEY, &t) {
                    tracing::error!(error = %e, "remote token could not be written to auth.json");
                    *self.last_error.lock().expect("错误锁") =
                        Some(ui_error!("host.remote.tokenSaveFailed"; e));
                    return;
                }
                t
            }
        };
        *self.shared.token.lock().expect("令牌锁") = Some(token);

        let ip = match cfg.bind {
            RemoteBind::Loopback => IpAddr::V4(Ipv4Addr::LOCALHOST),
            RemoteBind::Lan => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        };
        let addr = SocketAddr::new(ip, cfg.port);
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!(%addr, error = %e, "remote access failed to bind");
                *self.last_error.lock().expect("错误锁") =
                    Some(ui_error!("host.remote.bindFailed", addr = addr; e));
                return;
            }
        };
        let addr = listener.local_addr().unwrap_or(addr);
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let (kick_tx, kick_rx) = watch::channel(false);
        let router = server::router(app.clone(), Arc::clone(&self.shared), kick_rx);
        let task = tokio::spawn(async move {
            let serve = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = stop_rx.await;
            });
            if let Err(e) = serve.await {
                tracing::error!(error = %e, "远程服务异常退出");
            }
        });
        *self.last_error.lock().expect("错误锁") = None;
        *g = Some(Running {
            bind: cfg.bind,
            port: cfg.port,
            addr,
            stop: Some(stop_tx),
            kick: kick_tx,
            task,
        });
        tracing::info!(%addr, "远程访问已开启");
    }

    /// 换一枚新令牌。已连上的连接不断（它们已经过了门），之后的连接要用新的。
    pub fn rotate_token(&self) -> Result<String, crate::config::ConfigError> {
        let t = auth::generate_token();
        crate::config::save_key(REMOTE_TOKEN_KEY, &t)?;
        *self.shared.token.lock().expect("令牌锁") = Some(t.clone());
        Ok(t)
    }

    /// 设置页要看的全部状态。
    pub async fn status(&self, cfg: &RemoteConfig) -> RemoteStatus {
        let g = self.running.lock().await;
        let running = g.as_ref();
        let token = self
            .shared
            .token
            .lock()
            .expect("令牌锁")
            .clone()
            .or_else(|| crate::config::load_secret(REMOTE_TOKEN_KEY));
        let port = running.map(|r| r.port).unwrap_or(cfg.port);
        let bind = running.map(|r| r.bind).unwrap_or(cfg.bind);
        let urls = if running.is_some() {
            access_urls(bind, port)
        } else {
            Vec::new()
        };
        // 登录链接把令牌放在 `#` 后面：片段不进 HTTP 请求，服务端日志和
        // 中间的代理都看不到它。前端首次加载时把它收进 localStorage 并
        // 从地址栏抹掉。
        let login_url = match (&token, urls.first()) {
            (Some(t), Some(u)) => Some(format!("{u}#token={t}")),
            _ => None,
        };
        let qr_svg = login_url.as_deref().and_then(qr_svg);
        RemoteStatus {
            enabled: cfg.enabled,
            running: running.is_some(),
            bind,
            port,
            listen_addr: running.map(|r| r.addr.to_string()),
            urls,
            login_url,
            qr_svg,
            token,
            connections: self
                .shared
                .connections
                .load(std::sync::atomic::Ordering::Relaxed),
            error: self.last_error.lock().expect("错误锁").clone(),
        }
    }
}

/// 停一个服务:先踢 WebSocket 连接,再停 accept 循环,然后等 serve 任务退出。
///
/// 等是有上限的:graceful shutdown 会等还没完成的 HTTP 请求(静态资源,
/// 毫秒级),WebSocket 已经被踢了不在它的账上;万一有条连接卡着,也不能
/// 让"保存设置"一直转 —— 到点就放手,listener 迟早会随任务结束释放。
async fn stop(mut r: Running) {
    let _ = r.kick.send_replace(true);
    if let Some(tx) = r.stop.take() {
        let _ = tx.send(());
    }
    if tokio::time::timeout(STOP_DEADLINE, &mut r.task)
        .await
        .is_err()
    {
        tracing::warn!("远程服务没在期限内退出,不再等它");
        r.task.abort();
    }
}

/// 停服务最多等多久。
const STOP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(3);

/// 设置页看到的远程访问状态。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    pub enabled: bool,
    pub running: bool,
    pub bind: RemoteBind,
    pub port: u16,
    pub listen_addr: Option<String>,
    /// 可以打开的地址（不含令牌）。
    pub urls: Vec<String>,
    /// 带令牌的一键登录链接（二维码编的就是它）。
    pub login_url: Option<String>,
    pub qr_svg: Option<String>,
    /// 当前令牌。只给设置页显示 —— 拿得到这条命令的人已经是主人。
    pub token: Option<String>,
    pub connections: usize,
    /// 上次起服务失败的原因（端口被占、令牌写不进 auth.json）。
    pub error: Option<UiError>,
}

/// 能打开网页版的地址。
///
/// 回环只有一个。局域网列出本机对外那块网卡的 IPv4 —— 用 UDP "connect"
/// 探路由，不真发包，不依赖网卡枚举库；多网卡机器只给出默认路由那一块，
/// 那也正是手机能连到的那一块。
fn access_urls(bind: RemoteBind, port: u16) -> Vec<String> {
    match bind {
        RemoteBind::Loopback => vec![format!("http://127.0.0.1:{port}/")],
        RemoteBind::Lan => {
            let mut out = Vec::new();
            if let Some(ip) = primary_lan_ip() {
                out.push(format!("http://{ip}:{port}/"));
            }
            out.push(format!("http://127.0.0.1:{port}/"));
            out
        }
    }
}

fn primary_lan_ip() -> Option<Ipv4Addr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?; // TEST-NET，不会真有对端；只为让内核选路
    match s.local_addr().ok()?.ip() {
        IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_unspecified() => Some(v4),
        _ => None,
    }
}

/// 把登录链接画成二维码 SVG。链接太长编不进去（不会发生：URL + 43 位令牌
/// 远在容量之内）就没有二维码，界面上显示链接本身。
///
/// 只出形状不出颜色：模块填 `currentColor`、底不填。配色归页面管（深色是
/// 浅模块深底，浅色反过来），这里写死任何一种都会在另一套主题里变成一块
/// 反色的方块。`[约束]` 所以前端必须把它**内联**进 DOM，不能当 `<img src>`
/// 用 —— 图片里的 SVG 拿不到页面的 `color`，`currentColor` 会落回黑色。
/// 同一个理由，去掉开头的 XML 声明：它是独立文件才需要的东西，塞进 HTML
/// 会被解析成一段无意义的注释。
fn qr_svg(text: &str) -> Option<String> {
    use qrcode::render::svg;
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let doc = code
        .render::<svg::Color<'_>>()
        .min_dimensions(180, 180)
        .quiet_zone(true)
        .dark_color(svg::Color("currentColor"))
        .light_color(svg::Color("none"))
        .build();
    let start = doc.find("<svg")?;
    Some(doc[start..].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 回环只给一个地址_局域网至少给回环() {
        assert_eq!(
            access_urls(RemoteBind::Loopback, 7823),
            vec!["http://127.0.0.1:7823/"]
        );
        let lan = access_urls(RemoteBind::Lan, 7823);
        assert!(lan.iter().any(|u| u == "http://127.0.0.1:7823/"));
    }

    #[test]
    fn 二维码能编下登录链接() {
        let url = format!(
            "http://192.168.1.100:7823/#token={}",
            auth::generate_token()
        );
        let svg = qr_svg(&url).expect("能编");
        assert!(svg.starts_with("<svg"), "内联用，不带 XML 声明：{}", &svg[..60]);
        assert!(
            svg.contains(r#"fill="currentColor""#) && svg.contains(r#"fill="none""#),
            "颜色留给页面定"
        );
        assert!(!svg.contains('#'), "不该写死任何颜色");
    }
}
