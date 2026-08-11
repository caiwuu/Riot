//! 把浏览器子进程接到工具层的 [`BrowserAccess`]。
//!
//! # 惰性启动
//!
//! `[取舍]` 第一次真的用到才起进程。
//!
//! CEF 起来是六个进程、几百 MB 常驻。大多数会话根本不碰浏览器（改个后端、
//! 看个日志），为它们付这个代价不合理。代价是首次调用要多等一两秒 ——
//! 而那一次本来就要等页面加载，用户感知不到差别。
//!
//! # 谁负责关
//!
//! 进程活到会话结束。每次调用后关掉的话，下一次又要付启动成本，而且
//! 页面状态（登录、滚动位置、SPA 的路由）全丢 —— 模型改完一次样式再截图
//! 会发现自己回到了首页。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use riot_protocol::browser::{BrowserAccess, BrowserUnavailable, Event};
use tokio::sync::{Mutex, mpsc};

use super::{Browser, ops};

/// 一帧画面。
#[derive(Debug, Clone)]
pub struct Frame {
    /// base64 的 JPEG。直接能塞进 `<img src="data:image/jpeg;base64,...">`。
    pub data: String,
    pub width: u32,
    pub height: u32,
}

pub struct HostBrowser {
    /// `.app` 的位置。
    app: PathBuf,
    /// 数据目录。每个会话一份 —— 同一个目录不能有两个 Chromium 实例。
    profile: PathBuf,
    /// 起好的进程。第一次用到时填上。
    inner: Mutex<Option<Arc<Browser>>>,
    /// 画面出口。面板打开时装上，关闭时摘掉。
    frames: Arc<Mutex<Option<mpsc::UnboundedSender<Frame>>>>,
}

impl HostBrowser {
    pub fn new(app: PathBuf, profile: PathBuf) -> Self {
        Self {
            app,
            profile,
            inner: Mutex::new(None),
            frames: Arc::default(),
        }
    }

    /// 开始把画面推到 `sink`。
    ///
    /// `[取舍]` 用 CDP 的 screencast 而不是自己搬 OSR 的像素。
    ///
    /// OSR 给的是 1280×800 的 BGRA，一帧 4MB；screencast 给的是 JPEG，
    /// 同样内容一帧一两百 KB —— 小二十倍，而且编码由 Chromium 做，
    /// 我们连共享内存都不用碰。代价是有损压缩，但面板是给人看的，
    /// 模型要精确像素时走 BrowserScreenshot（那条是 PNG）。
    pub async fn start_screencast(
        &self,
        sink: mpsc::UnboundedSender<Frame>,
    ) -> Result<(), BrowserUnavailable> {
        let b = self.get().await?;
        *self.frames.lock().await = Some(sink);
        b.cdp(
            "Page.startScreencast",
            serde_json::json!({
                "format": "jpeg",
                // 60 在文字页面上已经看不出压缩痕迹，再高只是白涨体积。
                "quality": 60,
                "maxWidth": 1600,
                "maxHeight": 1000,
            }),
        )
        .await
        .map_err(|e| BrowserUnavailable(e.to_string()))?;
        Ok(())
    }

    /// 停止推送。面板关掉时调 —— 没人看的时候继续编码 JPEG 是白烧 CPU。
    pub async fn stop_screencast(&self) {
        *self.frames.lock().await = None;
        let Some(b) = self.inner.lock().await.clone() else {
            return;
        };
        let _ = b.cdp("Page.stopScreencast", serde_json::json!({})).await;
    }

    /// 拿到浏览器，没起来就起。
    ///
    /// `[约束]` 整个过程持锁。并发的两次工具调用都发现"还没起"的话，会
    /// 各起一个进程 —— 而它们指向同一个 profile 目录，第二个拿不到锁直接
    /// 退出，表现为"偶尔有个工具报浏览器不可用"。
    async fn get(&self) -> Result<Arc<Browser>, BrowserUnavailable> {
        let mut slot = self.inner.lock().await;
        if let Some(b) = slot.as_ref() {
            return Ok(Arc::clone(b));
        }

        let (tx, mut rx) = mpsc::unbounded_channel();
        let browser = Browser::spawn(self.app.clone(), Some(self.profile.clone()), tx)
            .await
            .map_err(|e| BrowserUnavailable(e.to_string()))?;

        let browser = Arc::new(browser);

        // 事件流必须一直有人排空 —— 通道是无界的，事件会持续来。
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let acker = Arc::clone(&browser);
        let frames = Arc::clone(&self.frames);
        tokio::spawn(async move {
            let mut ready = Some(ready_tx);
            while let Some(ev) = rx.recv().await {
                match ev {
                    Event::Ready => {
                        if let Some(t) = ready.take() {
                            let _ = t.send(());
                        }
                    }
                    Event::Error { message } => {
                        tracing::warn!(message, "浏览器报错");
                    }
                    Event::Cdp { payload } => {
                        handle_cdp_event(&acker, &frames, &payload).await;
                    }
                    // OSR 的帧元数据现在没人用 —— 画面走 screencast。
                    // 留着不删是因为它是"渲染还活着"的独立信号，
                    // screencast 卡住时能用来分清是编码还是渲染的问题。
                    _ => {}
                }
            }
        });

        // 等 CEF 就绪。没等到就发命令的话，命令会落在还不存在的浏览器上，
        // 全部以"还没有浏览器"失败。
        tokio::time::timeout(std::time::Duration::from_secs(30), ready_rx)
            .await
            .map_err(|_| BrowserUnavailable("浏览器 30 秒内没有就绪".into()))?
            .map_err(|_| BrowserUnavailable("浏览器启动过程中退出了".into()))?;

        // console 钩子要在任何导航之前装 —— 页面加载期间的报错最有价值，
        // 那时候没装就永远抓不到。
        if let Err(e) = ops::install_console_hook(&browser).await {
            tracing::warn!(error = %e, "装 console 钩子失败，console 工具会返回空");
        }
        *slot = Some(Arc::clone(&browser));
        Ok(browser)
    }
}

#[async_trait]
impl BrowserAccess for HostBrowser {
    async fn navigate(&self, url: &str) -> Result<(), BrowserUnavailable> {
        let b = self.get().await?;
        ops::navigate(&b, url)
            .await
            .map_err(|e| BrowserUnavailable(e.to_string()))
    }

    async fn screenshot(&self) -> Result<String, BrowserUnavailable> {
        let b = self.get().await?;
        ops::screenshot(&b)
            .await
            .map_err(|e| BrowserUnavailable(e.to_string()))
    }

    async fn snapshot(&self) -> Result<String, BrowserUnavailable> {
        let b = self.get().await?;
        ops::snapshot(&b)
            .await
            .map_err(|e| BrowserUnavailable(e.to_string()))
    }

    async fn console(&self) -> Result<Vec<String>, BrowserUnavailable> {
        let b = self.get().await?;
        ops::console(&b)
            .await
            .map_err(|e| BrowserUnavailable(e.to_string()))
    }

    async fn current_url(&self) -> String {
        // 没起来就不要为了这个把它起起来 —— 这是个信息性查询。
        let Some(b) = self.inner.lock().await.clone() else {
            return String::new();
        };
        b.cdp("Runtime.evaluate", serde_json::json!({
            "expression": "location.href",
            "returnByValue": true,
        }))
        .await
        .ok()
        .and_then(|v| v["result"]["value"].as_str().map(ToOwned::to_owned))
        .unwrap_or_default()
    }
}

/// 处理不带 id 的 CDP 事件。目前只有 screencast 的帧。
async fn handle_cdp_event(
    browser: &Arc<Browser>,
    frames: &Arc<Mutex<Option<mpsc::UnboundedSender<Frame>>>>,
    payload: &serde_json::Value,
) {
    if payload.get("method").and_then(|m| m.as_str()) != Some("Page.screencastFrame") {
        return;
    }
    let params = &payload["params"];

    // `[约束]` 必须 ack，而且要无条件 ack。
    //
    // Chromium 只在上一帧被确认后才发下一帧。漏一次 ack，画面就永久停在
    // 那一帧 —— 而且不报错，看起来像页面卡住了。所以哪怕下面的转发失败
    // 也要先把这条发出去。
    if let Some(sid) = params.get("sessionId") {
        let _ = browser.cdp_no_wait(
            "Page.screencastFrameAck",
            serde_json::json!({ "sessionId": sid }),
        );
    }

    let Some(sink) = frames.lock().await.clone() else {
        return; // 面板没开，帧丢掉
    };
    let Some(data) = params["data"].as_str() else {
        return;
    };
    let meta = &params["metadata"];
    let frame = Frame {
        data: data.to_owned(),
        width: meta["deviceWidth"].as_f64().unwrap_or_default() as u32,
        height: meta["deviceHeight"].as_f64().unwrap_or_default() as u32,
    };
    // 发失败说明面板那头没了，摘掉出口顺便停推送。
    if sink.send(frame).is_err() {
        *frames.lock().await = None;
        let _ = browser.cdp_no_wait("Page.stopScreencast", serde_json::json!({}));
    }
}

/// 打包好的浏览器在哪儿。
///
/// 开发时在 crate 的 target 下（`scripts/build-browser.sh` 的产物），
/// 发版后在主 app 的 Resources 里。找不到时返回 `None` —— 调用方据此
/// 装 `NoBrowser`，工具会明确说用不了。
pub fn locate_app() -> Option<PathBuf> {
    // 发版布局:Riot.app/Contents/Resources/riot-browser.app
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let bundled = dir.join("../Resources/riot-browser.app");
        if bundled.is_dir() {
            return bundled.canonicalize().ok();
        }
    }
    // 开发布局
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../crates/riot-browser/target/bundle/riot-browser.app");
    dev.is_dir().then(|| dev.canonicalize().unwrap_or(dev))
}
