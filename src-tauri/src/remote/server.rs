//! HTTP 层：静态资源 + WebSocket 升级。
//!
//! 网页版加载的是**和桌面窗口同一份**前端产物：正式包里它嵌在可执行文件里
//! （`tauri::generate_context!`），开发模式下 Tauri 的 asset resolver 退回读
//! `dist/` 目录。所以没有第二套构建、没有第二份 HTML —— 桌面能用的界面
//! 网页就能用。开发时 `dist/` 还没建出来的话，`/` 跳到 Vite 的 devUrl，
//! 那边的 `/ws` 由 `vite.config.ts` 反代回这里。

// 宿主层：真实 socket。见 clippy.toml。
#![allow(clippy::disallowed_methods)]

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use tauri::AppHandle;
use tokio::sync::watch;

use super::{Shared, conn};

#[derive(Clone)]
struct Ctx {
    app: AppHandle,
    shared: Arc<Shared>,
    /// 服务要停了(关开关、换端口):翻成 true,每条连接看到就自己退出。
    kick: watch::Receiver<bool>,
}

/// 网页版自己的 CSP。
///
/// 不复用 `tauri.conf.json` 里的那条：它放行的是 `ipc:`、`http://ipc.localhost`
/// 这些只在 webview 里存在的源；这里要放行的是回自己的 WebSocket。
/// `'self'` 在 CSP3 里覆盖同源 ws/wss，但 Safari 直到近两年才跟上，显式写
/// `ws: wss:` 免得旧 iOS 上连不上却一个字都不报。
const CSP: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; \
    worker-src 'self' blob:; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; \
    font-src 'self' data:; media-src 'self' blob: data:; \
    connect-src 'self' ws: wss: data: blob:; frame-ancestors 'none'";

pub fn router(app: AppHandle, shared: Arc<Shared>, kick: watch::Receiver<bool>) -> Router {
    Router::new()
        .route("/ws", get(ws_upgrade))
        .route("/healthz", get(|| async { "ok" }))
        .fallback(get(static_asset))
        .with_state(Ctx { app, shared, kick })
}

/// WebSocket 握手。这里只做**来源**校验；令牌在连接建立后的第一帧里验
/// （浏览器的 WebSocket API 设不了请求头，令牌放 URL 又会进日志）。
async fn ws_upgrade(
    State(ctx): State<Ctx>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    if !super::auth::origin_allowed(origin, host, &ctx.shared.allowed_origins) {
        tracing::warn!(?origin, ?host, %peer, "远程连接来源不匹配，拒绝握手");
        return (StatusCode::FORBIDDEN, "来源不匹配").into_response();
    }
    // 限速按谁记账：配了放行 origin = 用户明确处于反向代理后，看 X-Forwarded-For；
    // 否则就是 TCP 对端（见 auth::client_ip）。
    let client = super::auth::client_ip(
        peer.ip(),
        headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()),
        !ctx.shared.allowed_origins.is_empty(),
    );
    let app = ctx.app.clone();
    let shared = Arc::clone(&ctx.shared);
    let kick = ctx.kick.clone();
    ws.on_upgrade(move |socket| conn::serve(socket, app, shared, client, kick))
}

/// 静态资源：从 Tauri 的 asset resolver 取，找不到的路径回 index.html
/// （单页应用的路由都在前端）。
async fn static_asset(State(ctx): State<Ctx>, req: Request) -> Response {
    let uri: &Uri = req.uri();
    let path = uri.path().to_owned();

    // 开发模式（`tauri dev`，tauri-build 打的 cfg）：前端在 Vite 那边热更新，
    // 磁盘上的 dist/ 多半是上次 build 留下的陈货，端出去只会让人对着旧界面
    // 排查新代码。一律跳去 devUrl，那边的 /ws 反代回这里（见 vite.config.ts）。
    #[cfg(dev)]
    if let Some(dev) = ctx.app.config().build.dev_url.as_ref() {
        let mut target = dev.clone();
        target.set_path(uri.path());
        target.set_query(uri.query());
        return Redirect::temporary(target.as_str()).into_response();
    }

    let resolver = ctx.app.asset_resolver();
    // 目录请求（`/`）和没有扩展名的路径（单页应用的前端路由）都落到
    // index.html。正式包里 resolver 自己会这么兜；开发模式它读的是磁盘上的
    // dist/，对 `/` 会去 read 一个目录然后失败，所以这里先替它换成入口页。
    let is_route = path.ends_with('/') || !path.rsplit('/').next().unwrap_or("").contains('.');
    let asset = if is_route {
        resolver.get("/index.html".to_owned())
    } else {
        resolver
            .get(path.clone())
            .or_else(|| resolver.get("/index.html".to_owned()))
    };
    let Some(asset) = asset else {
        // 开发模式、dist 没建：跳去 Vite。正式包里 index.html 一定在，
        // 走不到这里。
        if let Some(dev) = ctx.app.config().build.dev_url.as_ref() {
            return Redirect::temporary(dev.as_str()).into_response();
        }
        return (StatusCode::NOT_FOUND, "没有这个资源").into_response();
    };

    let is_html = asset.mime_type.starts_with("text/html");
    let mut resp = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, asset.mime_type.as_str())
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    if is_html {
        resp = resp
            .header(header::CONTENT_SECURITY_POLICY, CSP)
            // 入口页每次都要新的 —— 它引用的 chunk 名带哈希，升级后旧入口
            // 会指向已经不存在的文件。
            .header(header::CACHE_CONTROL, "no-cache")
            .header("Referrer-Policy", "no-referrer");
    } else if path.starts_with("/assets/") {
        // Vite 产物带内容哈希，可以放心长缓存。
        resp = resp.header(header::CACHE_CONTROL, "public, max-age=31536000, immutable");
    } else {
        resp = resp.header(header::CACHE_CONTROL, "no-cache");
    }
    resp.body(Body::from(asset.bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
