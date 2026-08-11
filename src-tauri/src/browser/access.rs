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
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    /// base64 的 JPEG。直接能塞进 `<img src="data:image/jpeg;base64,...">`。
    pub data: String,
    pub width: u32,
    pub height: u32,
}

/// 面板转发过来的一次输入。
///
/// 坐标是**页面坐标**（相对视口左上角，CSS 像素）。面板负责把自己的
/// DOM 坐标换算过来 —— 它知道自己的缩放比例，这一层不知道。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Input {
    /// 按下并抬起。合成一次完整点击，而不是让前端发两条 —— 中间要是
    /// 丢了一条，页面会停在"按住"状态，后续所有交互都不对。
    Click { x: f64, y: f64, button: String },
    Move { x: f64, y: f64 },
    Scroll { x: f64, y: f64, delta_y: f64 },
    /// 输入文本。走 insertText 而不是逐字符 keyDown ——
    /// 中文、emoji 这些没有对应键码，逐字符发根本发不出来。
    Text { text: String },
    /// 功能键（Enter、Backspace、方向键之类）。
    Key { key: String },
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

    /// 把面板上的一次输入打到页面里。
    ///
    /// `[取舍]` 走 CDP 的 `Input.*` 而不是在页面里合成 DOM 事件。
    ///
    /// 合成事件（`element.dispatchEvent(new MouseEvent(...))`）拿不到
    /// `isTrusted`，很多库会忽略它；也走不通原生控件（`<select>` 的下拉、
    /// 文件选择、拖拽）。`Input.*` 是从浏览器输入栈的顶端进去的，页面
    /// 分辨不出和真人操作的区别。
    pub async fn send_input(&self, input: Input) -> Result<(), BrowserUnavailable> {
        let b = self.get().await?;
        let calls: Vec<(&str, serde_json::Value)> = match input {
            Input::Click { x, y, button } => vec![
                ("Input.dispatchMouseEvent", serde_json::json!({
                    "type": "mousePressed", "x": x, "y": y,
                    "button": button, "clickCount": 1,
                })),
                ("Input.dispatchMouseEvent", serde_json::json!({
                    "type": "mouseReleased", "x": x, "y": y,
                    "button": button, "clickCount": 1,
                })),
            ],
            Input::Move { x, y } => vec![(
                "Input.dispatchMouseEvent",
                serde_json::json!({ "type": "mouseMoved", "x": x, "y": y }),
            )],
            Input::Scroll { x, y, delta_y } => vec![(
                "Input.dispatchMouseEvent",
                serde_json::json!({
                    "type": "mouseWheel", "x": x, "y": y,
                    "deltaX": 0, "deltaY": delta_y,
                }),
            )],
            Input::Text { text } => vec![(
                "Input.insertText",
                serde_json::json!({ "text": text }),
            )],
            Input::Key { key } => {
                let code = key_code(&key);
                vec![
                    ("Input.dispatchKeyEvent", serde_json::json!({
                        "type": "keyDown", "key": key, "windowsVirtualKeyCode": code,
                    })),
                    ("Input.dispatchKeyEvent", serde_json::json!({
                        "type": "keyUp", "key": key, "windowsVirtualKeyCode": code,
                    })),
                ]
            }
        };

        for (method, params) in calls {
            // 不等响应。输入事件是连续流，逐个等往返会让打字有明显延迟，
            // 而它们的响应本来就是空的。
            b.cdp_no_wait(method, params)
                .map_err(|e| BrowserUnavailable(e.to_string()))?;
        }
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

/// 功能键的 Windows 虚拟键码。
///
/// `[约束]` 这几个键必须带键码。只发 `key` 字符串的话，Chromium 收得到
/// 事件但不会执行默认行为 —— 回车不提交表单、退格不删字符。看起来像
/// "按了没反应"，而事件其实是送到了的。
///
/// 只列常用的。列表外的键当普通文本处理，那对单字符键是对的。
fn key_code(key: &str) -> u32 {
    match key {
        "Enter" => 13,
        "Backspace" => 8,
        "Tab" => 9,
        "Escape" => 27,
        "ArrowLeft" => 37,
        "ArrowUp" => 38,
        "ArrowRight" => 39,
        "ArrowDown" => 40,
        "Delete" => 46,
        "Home" => 36,
        "End" => 35,
        "PageUp" => 33,
        "PageDown" => 34,
        _ => 0,
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
