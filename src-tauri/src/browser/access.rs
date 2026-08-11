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

pub struct HostBrowser {
    /// `.app` 的位置。
    app: PathBuf,
    /// 数据目录。每个会话一份 —— 同一个目录不能有两个 Chromium 实例。
    profile: PathBuf,
    /// 起好的进程。第一次用到时填上。
    inner: Mutex<Option<Arc<Browser>>>,
}

impl HostBrowser {
    pub fn new(app: PathBuf, profile: PathBuf) -> Self {
        Self {
            app,
            profile,
            inner: Mutex::new(None),
        }
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

        // 事件流必须一直有人排空 —— 通道是无界的，帧事件会持续来。
        // 目前只用来等 Ready，之后面板会接到这里。
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
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
                    // 帧和加载事件目前没人要，读掉即可。面板做好之后
                    // 这里转发出去。
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

        let browser = Arc::new(browser);
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
