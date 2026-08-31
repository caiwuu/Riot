//! 浏览器的高层操作。工具层调这里，不直接拼 CDP。
//!
//! 每个函数就是一两条 CDP 命令加一点结果整形。放在这一层而不是工具里，
//! 是因为"用哪个 CDP 方法、参数怎么填"属于浏览器知识，而工具层该只关心
//! "给模型什么"。
//!
//! # 给模型的东西要克制
//!
//! 页面的原始 DOM 动辄几十万字符，截图动辄几 MB。直接塞给模型的结果是
//! 一次调用吃掉整个上下文，后面几轮全靠压缩苟活。所以这里的每个函数都
//! 自带上限，而且宁可截断也不放行。

use std::collections::HashMap;
use std::time::Duration;

use riot_protocol::browser::Command;
use serde_json::{Value, json};

use super::{BrowserError, Tab};

/// 可访问性树的节点数上限。
///
/// 一个中等复杂的页面能有几千个节点。全给模型意味着几万 token，而它真正
/// 需要的通常是"页面上有哪些可交互的东西"。超出就截断并说明。
const MAX_A11Y_NODES: usize = 400;

/// console 一次最多回多少条。
const MAX_CONSOLE: usize = 100;

/// 导航并等页面加载完。
///
/// `[约束]` 必须等页面真的就绪，不能发完 `Page.navigate` 就返回。
///
/// 不等的后果有两层:立刻截图会拍到白屏，而模型会认真地分析那张白屏；
/// 跨文档导航期间 DevTools agent 会短暂脱离，紧接着发的 CDP 命令会以
/// `Not attached to an active page` 失败 —— 那个报错完全不像"页面还没好"。
///
/// 用轮询 `document.readyState` 而不是订阅 `Page.loadEventFired`:事件走的是
/// 另一条通道（不带 id 的 CDP 事件），在这一层拿不到；而轮询只需要现成的
/// 请求/响应，代价是最多多等一个间隔。
pub async fn navigate(tab: Tab<'_>, url: &str) -> Result<(), BrowserError> {
    tab.cdp("Page.enable", json!({})).await?;
    tab.cdp("Page.navigate", json!({ "url": url })).await?;
    wait_until_ready(tab).await
}

/// 页面加载的等待上限。超过就照常返回 —— 有些页面（长轮询、埋点）
/// 永远到不了 complete，为此把整个调用挂死不值得。
const LOAD_TIMEOUT: Duration = Duration::from_secs(20);
const POLL_EVERY: Duration = Duration::from_millis(120);

async fn wait_until_ready(tab: Tab<'_>) -> Result<(), BrowserError> {
    let deadline = tokio::time::Instant::now() + LOAD_TIMEOUT;
    loop {
        // 导航切换文档的瞬间这条会失败（agent 正在换）。那不是错误，
        // 是"还没好"，继续等。
        if let Ok(r) = tab
            .cdp(
                "Runtime.evaluate",
                json!({ "expression": "document.readyState", "returnByValue": true }),
            )
            .await
            && r["result"]["value"].as_str() == Some("complete")
        {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            // 不当失败。页面可能只是有个长连接，内容早就渲染好了。
            tracing::debug!("页面 {LOAD_TIMEOUT:?} 内没到 complete，按已就绪继续");
            return Ok(());
        }
        tokio::time::sleep(POLL_EVERY).await;
    }
}

/// 整页截图的高度上限（CSS 像素）。
///
/// 整页截图靠把离屏表面临时拉到整页高实现（见 [`screenshot`]），高度不设限
/// 的话，一个无限滚动的页面能把渲染进程的显存吃穿。8000 已经是十几屏 ——
/// 比这更长的页面该用 BrowserSnapshot 找目标，而不是靠一张巨图。
const MAX_SHOT_HEIGHT: f64 = 8000.0;

/// 给模型的整页截图，返回 JPEG 的 base64。
///
/// `[取舍]` 拍完整页面而不是只拍视口。模型看视口截图会漏掉折叠下方的
/// 内容，然后自信地说"页面上没有那个按钮"。
///
/// `[约束]` 长页面不能靠 `captureBeyondViewport`，也不能靠 `Emulation`
/// 仿真视口。离屏渲染（OSR）下前者不为视口外的区域真正排版，拿当前视口
/// 的帧重复平铺去填 —— 用户拿到的截图是同一屏内容摞了十几遍（真实发生
/// 过）；后者只改布局视口（`innerHeight` 会变），合成器的表面不跟着走，
/// 出的图同样是坏的。三种做法都被"探针"实测过，唯一出对图的是把**离屏
/// 表面本身**临时拉到整页高（走面板同款的 Resize 命令），截完立刻复原。
/// 代价一:拉高期间面板会闪过几帧被缩小的画面。代价二:`100vh` 定高的
/// 段落会跟着视口变高（这是"视口变大"的正确语义），比例失真好过内容重复。
///
/// `[约束]` 出图恒按 CSS 像素:短页面用 `clip.scale = 1/密度` 抵消面板的
/// Retina 渲染，长页面把表面按 `scale: 1` 拉。跟着面板密度走的话，同一个
/// 页面的截图大小差四倍，而工具那边有体积上限 —— "能不能截图"就变成了
/// 取决于用户把窗口拖到了哪块屏幕上（实测 518 KB 对 1283 KB，肉眼看不出
/// 差别）。
///
/// `[取舍]` JPEG 而不是 PNG。整页 PNG 动辄两三 MB，一张图就撞上限、
/// 或者吃掉小半个上下文窗口。q80 在 1× 下文字仍然清楚，而模型要判断的是
/// 布局、间距、颜色有没有错位，不是发丝级的锐度。
///
/// `deterministic` 为真时，截图前把视觉动态量冻住（见 [`freeze_visuals`]），
/// 截完复原 —— 让两次截图能做像素 diff。默认按原样拍当前一刻。
pub async fn screenshot(tab: Tab<'_>, deterministic: bool) -> Result<String, BrowserError> {
    if !deterministic {
        return screenshot_inner(tab).await;
    }
    // `[约束]` 冻结样式注在页面 DOM 里，截完**必须**移除 —— 不移除的话，
    // 用户面板里的页面从此没有动画、光标不闪，而且没有任何线索指向截图。
    // 长页面路径中途会 Resize 渲染表面，但那不重载文档，注入的样式照旧生效。
    freeze_visuals(tab).await;
    let shot = screenshot_inner(tab).await;
    unfreeze_visuals(tab).await;
    shot
}

/// 冻结页面里会让每帧都不同的东西:CSS 动画/过渡按 0 时长收尾、光标透明、
/// 平滑滚动关掉，再把正在跑的 Web Animations 暂停。
///
/// 全程 best-effort:冻结失败最多是图里留了个转圈，不该让截图本身失败。
/// 只暂停"此刻在跑"的动画并记在 `window.__riotFrozen__` 上，复原时只恢复
/// 这一批 —— 否则会把页面本来就暂停着的动画错误地放出来。
async fn freeze_visuals(tab: Tab<'_>) {
    const JS: &str = r#"(() => {
        let s = document.getElementById('__riot_freeze__');
        if (!s) {
            s = document.createElement('style');
            s.id = '__riot_freeze__';
            s.textContent = '*,*::before,*::after{animation-duration:0s !important;animation-delay:0s !important;animation-iteration-count:1 !important;transition-duration:0s !important;transition-delay:0s !important;caret-color:transparent !important;scroll-behavior:auto !important;}';
            (document.head || document.documentElement).appendChild(s);
        }
        window.__riotFrozen__ = [];
        try {
            for (const a of document.getAnimations()) {
                if (a.playState === 'running') { try { a.pause(); window.__riotFrozen__.push(a); } catch (e) {} }
            }
        } catch (e) {}
        return true;
    })()"#;
    let _ = tab
        .cdp("Runtime.evaluate", json!({ "expression": JS }))
        .await;
    // 给样式生效、动画停稳一点时间再拍。
    tokio::time::sleep(Duration::from_millis(120)).await;
}

/// 撤销 [`freeze_visuals`]:移掉注入的样式，把我们暂停过的动画放回去。
async fn unfreeze_visuals(tab: Tab<'_>) {
    const JS: &str = r#"(() => {
        const s = document.getElementById('__riot_freeze__');
        if (s) s.remove();
        try { (window.__riotFrozen__ || []).forEach(a => { try { a.play(); } catch (e) {} }); } catch (e) {}
        window.__riotFrozen__ = [];
        return true;
    })()"#;
    let _ = tab
        .cdp("Runtime.evaluate", json!({ "expression": JS }))
        .await;
}

async fn screenshot_inner(tab: Tab<'_>) -> Result<String, BrowserError> {
    // 问不出页面尺寸就只拍视口。有图比没图好 —— 而且真到这一步说明页面
    // 本身有问题。
    let Some(content_h) = content_height(tab).await else {
        let r = tab
            .cdp(
                "Page.captureScreenshot",
                json!({ "format": "jpeg", "quality": 80 }),
            )
            .await?;
        return Ok(r["data"].as_str().unwrap_or_default().to_owned());
    };
    let vp = viewport(tab).await?;

    // 一屏装得下:不动渲染表面（面板一帧都不闪），clip 里抵消密度即可。
    if content_h <= f64::from(vp.height) {
        let r = tab
            .cdp(
                "Page.captureScreenshot",
                json!({
                    "format": "jpeg",
                    "quality": 80,
                    "clip": {
                        "x": 0, "y": 0,
                        "width": f64::from(vp.width), "height": content_h,
                        "scale": 1.0 / f64::from(vp.scale),
                    },
                }),
            )
            .await?;
        return Ok(r["data"].as_str().unwrap_or_default().to_owned());
    }

    // 长页面:表面拉到整页高（按 1× 出图），拍完复原。
    let target_h = content_h.min(MAX_SHOT_HEIGHT).round() as i32;
    tab.browser.send(&Command::Resize {
        tab: tab.id,
        width: vp.width,
        height: target_h,
        scale: 1.0,
    })?;

    // 等布局跟上（innerHeight 到位），再给合成器一点提交时间。轮询而不是
    // 订阅帧事件 —— 这一层拿不到事件流，和 wait_until_ready 同一个处境。
    // 超时就按当前状态拍:有图比没图好。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(r) = tab
            .cdp(
                "Runtime.evaluate",
                json!({ "expression": "innerHeight", "returnByValue": true }),
            )
            .await
            && r["result"]["value"].as_i64() == Some(i64::from(target_h))
        {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    tokio::time::sleep(Duration::from_millis(250)).await;

    let shot = tab
        .cdp(
            "Page.captureScreenshot",
            json!({ "format": "jpeg", "quality": 80 }),
        )
        .await;

    // 无论截没截成，表面必须复原 —— 不复原的话面板从此显示一个被拉长的
    // 页面，滚动和点击的坐标全对不上。期间用户若刚好拖了面板，下一次拖动
    // 会再同步一次尺寸，不会永久错位。
    let _ = tab.browser.send(&Command::Resize {
        tab: tab.id,
        width: vp.width,
        height: vp.height,
        scale: vp.scale,
    });

    Ok(shot?["data"].as_str().unwrap_or_default().to_owned())
}

/// 当前渲染表面的尺寸（CSS 像素）和密度。截完图要按它复原。
struct Viewport {
    width: i32,
    height: i32,
    scale: f32,
}

async fn viewport(tab: Tab<'_>) -> Result<Viewport, BrowserError> {
    let r = tab
        .cdp(
            "Runtime.evaluate",
            json!({ "expression": "[innerWidth, innerHeight, devicePixelRatio]", "returnByValue": true }),
        )
        .await?;
    let v = &r["result"]["value"];
    Ok(Viewport {
        width: v[0].as_f64().unwrap_or(1280.0).round() as i32,
        height: v[1].as_f64().unwrap_or(800.0).round() as i32,
        scale: v[2].as_f64().unwrap_or(1.0) as f32,
    })
}

/// 页面内容高度（CSS 像素）。
async fn content_height(tab: Tab<'_>) -> Option<f64> {
    let m = tab.cdp("Page.getLayoutMetrics", json!({})).await.ok()?;
    let h = m["cssContentSize"]["height"].as_f64()?;
    (h >= 1.0).then_some(h)
}

/// 一个元素在视口里的矩形，CSS 像素，左上角是原点。
///
/// `[约束]` 是**视口坐标**，不是文档坐标 —— 已经减掉了滚动偏移（见
/// [`element_rects`]）。点击注入（`Input.dispatchMouseEvent`）和面板叠框
/// 用的都是视口坐标，存文档坐标的话，一滚动全错位。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    /// 和 `vw×vh` 的视口有没有交集。整个落在视口外（上方/下方/两侧）
    /// 就是假 —— 那种元素叠框叠不上、直接点也点不中，得先滚进来。
    pub(crate) fn intersects_viewport(&self, vw: f64, vh: f64) -> bool {
        self.w > 0.0
            && self.h > 0.0
            && self.x < vw
            && self.y < vh
            && self.x + self.w > 0.0
            && self.y + self.h > 0.0
    }
}

/// 快照里一个可指名的元素。
///
/// 编号 → 它的映射由快照产出、交互消费。`label` 跟快照里那一行长得一样
/// （`button "提交"`），交互的结果消息用它 —— 模型（和用户）看到的是
/// "点了什么"，而不是一个裸编号。
#[derive(Debug, Clone, PartialEq)]
pub struct SnapRef {
    /// CDP 的 backendDOMNodeId。交互时拿它换坐标。
    pub backend_id: i64,
    /// 快照行的原文，如 `button "提交"`。
    pub label: String,
    /// 元素在视口里的位置，CSS 像素。`None` = 拿不到几何:虚拟节点、
    /// iframe 内的元素（几何只取主文档），或者 `display:none`。给模型叠
    /// 编号框、以及"要不要先滚动"都看它。
    pub rect: Option<Rect>,
}

/// 页面的可访问性快照。返回给模型的文本和「编号 → 元素」映射。
///
/// `[取舍]` 用 a11y 树而不是 DOM。理由是**信噪比**:a11y 树给的是角色、
/// 名字、状态（button "提交"、textbox "邮箱" required），正好是模型要
/// 决策"点哪儿"需要的信息；而 DOM 里九成是 class 名和布局容器。同样的
/// token 预算下，a11y 树能表达的页面结构多一个数量级。
pub async fn snapshot(tab: Tab<'_>) -> Result<(String, HashMap<u32, SnapRef>), BrowserError> {
    tab.cdp("Accessibility.enable", json!({})).await?;
    let r = tab.cdp("Accessibility.getFullAXTree", json!({})).await?;
    let nodes = r["nodes"].as_array().map_or(&[][..], Vec::as_slice);
    // 几何是**增强**，不是快照的前提:拿不到就退化成"只有文本、没有位置"
    // 的老形态，而不是让整条快照失败。所以这里吞掉错误给空表。
    let rects = element_rects(tab).await.unwrap_or_default();
    Ok(render_nodes(nodes, &rects))
}

/// 主文档里每个元素的视口矩形，key 是 `backendNodeId`。
///
/// `[取舍]` 一次 `DOMSnapshot.captureSnapshot` 拿全，而不是对每个元素
/// `DOM.getBoxModel`。后者是几百次往返（一个中等页面就有几百个可交互
/// 元素），前者一次调用给整棵树的布局盒。
///
/// `[约束]` `bounds` 是**文档坐标、忽略滚动**（CDP 的定义，实测过），
/// 这里减掉 `scrollOffset` 换成视口坐标 —— 和点击注入、面板叠框同一个
/// 坐标系。不减的话，页面一滚动，框和元素就整体错位。
///
/// `[取舍]` 只取 `documents[0]`（主文档）。iframe 各有自己的滚动和相对
/// 父页的偏移，跨源的（Cloudflare、Stripe 那些）a11y 树本来也拿不到 ——
/// 它们的元素在这里没有几何，[`SnapRef::rect`] 是 `None`，叠不了框但也
/// 不会错位。
async fn element_rects(tab: Tab<'_>) -> Result<HashMap<i64, Rect>, BrowserError> {
    // `[约束]` bounds 是**物理像素**（CSS×dpr），要除以 dpr 换成 CSS 像素。
    // 实测:Retina 面板上一个 CSS 1069 宽的视口，根节点 bounds 宽 2138。
    // 不除的话，点击坐标和叠框在 Retina 上会整体偏到两倍远 —— 而在
    // 非 Retina（dpr=1）上一切正常，是最容易漏测的那类坑（Codex 的截图
    // 错位 bug 就是同一族的 CSS/DIP 混淆）。
    let dpr = tab
        .cdp(
            "Runtime.evaluate",
            json!({ "expression": "devicePixelRatio", "returnByValue": true }),
        )
        .await
        .ok()
        .and_then(|r| r["result"]["value"].as_f64())
        .filter(|d| *d > 0.0)
        .unwrap_or(1.0);
    // computedStyles 要空数组:样式表很大，而这里只要布局盒。
    let r = tab
        .cdp(
            "DOMSnapshot.captureSnapshot",
            json!({ "computedStyles": [] }),
        )
        .await?;
    let doc = &r["documents"][0];
    let sox = doc["scrollOffsetX"].as_f64().unwrap_or(0.0);
    let soy = doc["scrollOffsetY"].as_f64().unwrap_or(0.0);
    let backend = doc["nodes"]["backendNodeId"].as_array();
    let node_index = doc["layout"]["nodeIndex"].as_array();
    let bounds = doc["layout"]["bounds"].as_array();
    let (Some(backend), Some(node_index), Some(bounds)) = (backend, node_index, bounds) else {
        return Ok(HashMap::new());
    };

    let mut map = HashMap::with_capacity(node_index.len());
    for (i, ni) in node_index.iter().enumerate() {
        // layout 的第 i 项对应 nodes 里第 nodeIndex[i] 个节点。
        let Some(ni) = ni.as_u64().map(|v| v as usize) else {
            continue;
        };
        let Some(bid) = backend.get(ni).and_then(Value::as_i64) else {
            continue;
        };
        let b = &bounds[i];
        let (Some(x), Some(y), Some(w), Some(h)) =
            (b[0].as_f64(), b[1].as_f64(), b[2].as_f64(), b[3].as_f64())
        else {
            continue;
        };
        map.insert(
            bid,
            Rect {
                x: (x - sox) / dpr,
                y: (y - soy) / dpr,
                w: w / dpr,
                h: h / dpr,
            },
        );
    }
    Ok(map)
}

/// 把 a11y 节点整形成给模型的文本，顺手给每个能定位的元素发号。
///
/// 编号按**输出行**连续递增，不是按节点下标 —— 模型看到 `[7]` 就该能用 7，
/// 中间有洞的话它会试着点那些不存在的号。没有 `backendDOMNodeId` 的节点
/// （虚拟节点）不发号:发了也点不了，还占着一个"看起来能点"的位置。
fn render_nodes(nodes: &[Value], rects: &HashMap<i64, Rect>) -> (String, HashMap<u32, SnapRef>) {
    let total = nodes.len();
    let mut out = String::new();
    let mut refs = HashMap::new();
    let mut next: u32 = 1;

    for node in nodes.iter().take(MAX_A11Y_NODES) {
        let Some(line) = describe_node(node) else {
            continue;
        };
        match node["backendDOMNodeId"].as_i64() {
            Some(backend_id) => {
                out.push_str(&format!("[{next}] {line}\n"));
                let rect = rects.get(&backend_id).copied();
                refs.insert(
                    next,
                    SnapRef {
                        backend_id,
                        label: line,
                        rect,
                    },
                );
                next += 1;
            }
            None => {
                out.push_str(&line);
                out.push('\n');
            }
        }
    }

    if total > MAX_A11Y_NODES {
        out.push_str(&format!(
            "\n（页面共 {total} 个节点，只显示前 {MAX_A11Y_NODES} 个。）\n"
        ));
    }
    (out, refs)
}

/// 一个 a11y 节点的单行描述。忽略没有信息量的节点。
fn describe_node(node: &Value) -> Option<String> {
    if node["ignored"].as_bool().unwrap_or(false) {
        return None;
    }
    let role = node["role"]["value"].as_str().unwrap_or_default();
    // 纯结构性的容器对"点哪儿"没有帮助，滤掉能省下大量篇幅。
    if matches!(
        role,
        "" | "none" | "generic" | "InlineTextBox" | "StaticText"
    ) {
        return None;
    }
    let name = node["name"]["value"].as_str().unwrap_or_default().trim();
    if name.is_empty() {
        return Some(role.to_owned());
    }
    // 名字可能很长（比如一整段说明文字），截断。
    let name: String = name.chars().take(80).collect();
    Some(format!("{role} \"{name}\""))
}

// ── Set-of-Marks:给可交互元素叠编号框 ──────────────────

/// 只给这些"叶子控件"叠框。
///
/// main / navigation / region 这类容器也有 a11y 角色，但框住的是一整块
/// 区域 —— 对"点哪个"没帮助，叠上去只会糊满画面、还和真正的控件框重叠。
const MARKABLE_ROLES: &[&str] = &[
    "button",
    "link",
    "textbox",
    "searchbox",
    "checkbox",
    "radio",
    "combobox",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "tab",
    "switch",
    "slider",
    "option",
    "spinbutton",
    "listbox",
    "disclosuretriangle",
];

/// 这个快照标签是不是可叠框的可交互控件。
///
/// label 形如 `button "提交"` 或 `button`，取空格/引号前的首词当角色。
pub fn is_markable(label: &str) -> bool {
    let role = label.split([' ', '"']).next().unwrap_or("");
    MARKABLE_ROLES.contains(&role)
}

/// 注入 overlay 的脚本。`__MARKS__` 换成 `[[n,x,y,w,h],...]`。
///
/// `[约束]` 不用 format! 拼:这段 JS 里全是 `{}`，会和 format! 的占位打架。
/// 字面量 + 替换一个哨兵，井水不犯河水。
///
/// overlay 是一个 position:fixed 容器:z-index 顶到天、pointer-events 关掉
/// （不挡后续点击）、aria-hidden（不进 a11y 树）。截完由 [`REMOVE_MARKS`]
/// 撤掉，不留痕。
const INJECT_MARKS: &str = r#"(() => {
  const marks = __MARKS__;
  const ID = '__riot_marks__';
  document.getElementById(ID)?.remove();
  const box = document.createElement('div');
  box.id = ID; box.setAttribute('aria-hidden', 'true');
  box.style.cssText = 'position:fixed;left:0;top:0;width:0;height:0;z-index:2147483647;pointer-events:none';
  for (const [n, x, y, w, h] of marks) {
    const b = document.createElement('div');
    b.style.cssText = `position:fixed;left:${x}px;top:${y}px;width:${w}px;height:${h}px;border:2px solid #ff2d55;box-sizing:border-box;pointer-events:none`;
    const t = document.createElement('div');
    t.textContent = n;
    t.style.cssText = `position:fixed;left:${x}px;top:${Math.max(0, y - 15)}px;background:#ff2d55;color:#fff;font:bold 11px/14px monospace;padding:0 3px;pointer-events:none;white-space:nowrap`;
    box.appendChild(b); box.appendChild(t);
  }
  document.body.appendChild(box);
})()"#;

/// 撤掉 [`INJECT_MARKS`] 注入的 overlay。
const REMOVE_MARKS: &str = "document.getElementById('__riot_marks__')?.remove()";

/// 给一批元素叠编号框、截当前视口、再撤掉框。
///
/// `marks` 是 (编号, 视口矩形 CSS 像素)。截的是**视口**而不是整页 ——
/// Set-of-Marks 对应"现在看到的这屏"，编号框也只在视口内才有意义。
///
/// `[约束]` 无论截图成没成，overlay 都要撤掉。留一份红框在页面上，之后
/// 普通截图、甚至用户看面板都会看到它，而且它还会进后续的 DOM 快照。
pub async fn screenshot_marked(
    tab: Tab<'_>,
    marks: &[(u32, Rect)],
) -> Result<String, BrowserError> {
    let payload: Vec<(u32, f64, f64, f64, f64)> = marks
        .iter()
        .map(|(n, r)| (*n, r.x, r.y, r.w, r.h))
        .collect();
    let json = serde_json::to_string(&payload).unwrap_or_else(|_| "[]".to_owned());
    let inject = INJECT_MARKS.replace("__MARKS__", &json);

    tab.cdp("Runtime.evaluate", json!({ "expression": inject }))
        .await?;
    let shot = tab
        .cdp(
            "Page.captureScreenshot",
            json!({ "format": "jpeg", "quality": 80 }),
        )
        .await;
    let _ = tab
        .cdp("Runtime.evaluate", json!({ "expression": REMOVE_MARKS }))
        .await;

    Ok(shot?["data"].as_str().unwrap_or_default().to_owned())
}

// ── 交互原语 ──────────────────────────────────────────
//
// 每个函数一两条 CDP。组合成"点击并报告结果"这种完整动作的是
// access.rs —— 它有编号映射和标签，消息在那边拼。
//
// `[约束]` 这里的输入事件都走**等响应**的 `cdp`，不走面板输入那条
// no_wait 路。面板要的是打字不卡手；这里要的是"做完之后马上查页面
// 反应" —— 不等响应的话，紧跟着的查询会跑在事件前面，看到的是
// 交互之前的页面。

/// 把一个元素换成可点击的视口坐标（CSS 像素）。
///
/// 元素可能在视口外 —— 先滚进来再取几何。顺序不能反:
/// `getContentQuads` 给的是视口坐标，滚动之后旧坐标就作废了。
///
/// `[取舍]` 收敛到 objectId 而不是 backendNodeId。三种定位方式（快照编号、
/// CSS 选择器、文本）本来落在不同 CDP 域上，统一成 objectId 之后，滚动和
/// 取几何都只有一条路 —— 见 [`resolve_backend`]/[`resolve_selector`]/
/// [`resolve_text`]，它们各自把入口方式换成同一种 objectId。
pub async fn locate(tab: Tab<'_>, object_id: &str) -> Result<(f64, f64), BrowserError> {
    // 已经在视口里就不动。每次点击都把目标滚到顶上的话，用户看着
    // 画面会觉得页面在乱跳。
    tab.cdp(
        "DOM.scrollIntoViewIfNeeded",
        json!({ "objectId": object_id }),
    )
    .await?;
    let r = tab
        .cdp("DOM.getContentQuads", json!({ "objectId": object_id }))
        .await?;

    // quad 是四个角的 [x1,y1,...,x4,y4]，中心取平均。取第一个 quad ——
    // 跨行的内联元素会有多个，第一个是首行，点哪个都算点中。
    let quad = r["quads"][0].as_array().cloned().unwrap_or_default();
    if quad.len() != 8 {
        return Err(BrowserError::Cdp {
            method: "DOM.getContentQuads".into(),
            message: "元素没有可见的几何区域（可能被隐藏或折叠）".into(),
        });
    }
    let nums: Vec<f64> = quad.iter().filter_map(Value::as_f64).collect();
    let x = (nums[0] + nums[2] + nums[4] + nums[6]) / 4.0;
    let y = (nums[1] + nums[3] + nums[5] + nums[7]) / 4.0;
    Ok((x, y))
}

/// 把快照编号背后的 backendNodeId 换成 objectId。
///
/// 快照走 a11y 树，给的是 backendNodeId；而交互统一用 objectId（见
/// [`locate`]）。`DOM.resolveNode` 是这两者之间的桥。
pub async fn resolve_backend(tab: Tab<'_>, backend_id: i64) -> Result<String, BrowserError> {
    let r = tab
        .cdp("DOM.resolveNode", json!({ "backendNodeId": backend_id }))
        .await?;
    object_id_of(&r).ok_or_else(|| BrowserError::Cdp {
        method: "DOM.resolveNode".into(),
        message: "元素已不在页面上".into(),
    })
}

/// 用 CSS 选择器找元素，返回它的 objectId。匹配不到是 `Ok(None)`——
/// 那不是错误，是"页面上没有这个东西"，调用方要据此给模型不同的话。
pub async fn resolve_selector(
    tab: Tab<'_>,
    selector: &str,
) -> Result<Option<String>, BrowserError> {
    // 选择器当 JS 字符串字面量嵌进去（JSON 编码防注入和引号问题）。
    // 非法选择器会让 querySelector 抛异常，try/catch 兜成 null —— 当成
    // "没匹配到"，而不是让整条 evaluate 失败。
    let sel = serde_json::to_string(selector).unwrap_or_else(|_| "\"\"".into());
    let expr = format!(
        "(() => {{ try {{ return document.querySelector({sel}); }} catch (e) {{ return null; }} }})()"
    );
    let r = tab
        .cdp("Runtime.evaluate", json!({ "expression": expr }))
        .await?;
    Ok(object_id_of(&r))
}

/// 按可见文本找最合适的可点击元素，返回它的 objectId。
///
/// "最合适" = 包含这段文字、且**没有子元素也包含它**（叶子最优先），
/// 可见的排在前。这样点"登录"命中的是那个按钮，不是包着它的整个 header。
pub async fn resolve_text(tab: Tab<'_>, text: &str) -> Result<Option<String>, BrowserError> {
    let needle = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
    let expr = format!(
        "(() => {{ \
            const t = {needle}; \
            const all = Array.from(document.querySelectorAll('*')); \
            const hit = all.filter(el => el.textContent && el.textContent.includes(t) \
                && !Array.from(el.children).some(c => c.textContent && c.textContent.includes(t))); \
            const vis = hit.filter(el => {{ const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0; }}); \
            return vis[0] || hit[0] || null; \
        }})()"
    );
    let r = tab
        .cdp("Runtime.evaluate", json!({ "expression": expr }))
        .await?;
    Ok(object_id_of(&r))
}

/// 从 `Runtime.evaluate` / `DOM.resolveNode` 的结果里抠出 objectId。
///
/// 元素在则 `result.objectId`（resolveNode 是 `object.objectId`）；返回 null
/// 时没有 objectId —— 那正是"没匹配到"的信号。
fn object_id_of(r: &Value) -> Option<String> {
    r["result"]["objectId"]
        .as_str()
        .or_else(|| r["object"]["objectId"].as_str())
        .map(ToOwned::to_owned)
}

/// 轮询一个返回布尔的 JS 表达式，直到它为真或超时。返回是否等到了。
///
/// 自动化的等待都落到这里:等元素出现就是 `!!document.querySelector(..)`，
/// 等消失就取反。轮询而不是订阅 DOM 变更 —— 这一层拿不到事件流
/// （和 [`wait_until_ready`] 同一处境），而一个短间隔的 evaluate 很便宜。
pub async fn wait_predicate(
    tab: Tab<'_>,
    expr: &str,
    timeout_ms: u64,
) -> Result<bool, BrowserError> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        // 换文档的瞬间 evaluate 会失败（agent 正在换）——那不是"条件不成立"，
        // 是"这会儿问不到"，继续等。
        if let Ok(r) = tab
            .cdp(
                "Runtime.evaluate",
                json!({ "expression": expr, "returnByValue": true }),
            )
            .await
            && r["result"]["value"].as_bool() == Some(true)
        {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// 在视口坐标上合成一次完整的左键单击。
///
/// 先 move 再按 —— 不少页面靠 hover 态决定点击行为（下拉菜单、
/// 延迟加载的按钮），跳过 move 的点击在那些页面上会落空。
pub async fn click_at(tab: Tab<'_>, x: f64, y: f64) -> Result<(), BrowserError> {
    tab.cdp(
        "Input.dispatchMouseEvent",
        json!({ "type": "mouseMoved", "x": x, "y": y }),
    )
    .await?;
    tab.cdp(
        "Input.dispatchMouseEvent",
        json!({ "type": "mousePressed", "x": x, "y": y, "button": "left", "clickCount": 1 }),
    )
    .await?;
    tab.cdp(
        "Input.dispatchMouseEvent",
        json!({ "type": "mouseReleased", "x": x, "y": y, "button": "left", "clickCount": 1 }),
    )
    .await?;
    Ok(())
}

/// 把鼠标移到某个坐标（不按键）。触发 hover 态:下拉菜单、tooltip、
/// 悬停才出现的按钮都靠它。
pub async fn hover_at(tab: Tab<'_>, x: f64, y: f64) -> Result<(), BrowserError> {
    tab.cdp(
        "Input.dispatchMouseEvent",
        json!({ "type": "mouseMoved", "x": x, "y": y }),
    )
    .await
    .map(|_| ())
}

/// 双击。clickCount=2 —— 少了它页面收到的是两次独立单击，`dblclick`
/// 事件不会触发（选词、展开这类默认行为就不发生）。
pub async fn double_click_at(tab: Tab<'_>, x: f64, y: f64) -> Result<(), BrowserError> {
    hover_at(tab, x, y).await?;
    for kind in ["mousePressed", "mouseReleased"] {
        tab.cdp(
            "Input.dispatchMouseEvent",
            json!({ "type": kind, "x": x, "y": y, "button": "left", "clickCount": 2 }),
        )
        .await?;
    }
    Ok(())
}

/// 右键。用来触发上下文菜单（页面自定义的那种）。
pub async fn right_click_at(tab: Tab<'_>, x: f64, y: f64) -> Result<(), BrowserError> {
    hover_at(tab, x, y).await?;
    for kind in ["mousePressed", "mouseReleased"] {
        tab.cdp(
            "Input.dispatchMouseEvent",
            json!({ "type": kind, "x": x, "y": y, "button": "right", "clickCount": 1 }),
        )
        .await?;
    }
    Ok(())
}

/// 从一个坐标拖到另一个坐标:按下、移动几步、松开。
///
/// 中间插几步 mouseMoved 而不是一步到位 —— 靠 HTML5 拖拽的组件（看板、
/// 排序列表）要有连续的移动事件才认，瞬移过去它们当没发生。
pub async fn drag_between(
    tab: Tab<'_>,
    from: (f64, f64),
    to: (f64, f64),
) -> Result<(), BrowserError> {
    let (x1, y1) = from;
    let (x2, y2) = to;
    hover_at(tab, x1, y1).await?;
    tab.cdp(
        "Input.dispatchMouseEvent",
        json!({ "type": "mousePressed", "x": x1, "y": y1, "button": "left", "clickCount": 1 }),
    )
    .await?;
    // 五步线性插值。步数不必多，够触发 dragover 序列即可。
    for i in 1..=5 {
        let t = f64::from(i) / 5.0;
        tab.cdp(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseMoved",
                "x": x1 + (x2 - x1) * t,
                "y": y1 + (y2 - y1) * t,
                "button": "left",
            }),
        )
        .await?;
    }
    tab.cdp(
        "Input.dispatchMouseEvent",
        json!({ "type": "mouseReleased", "x": x2, "y": y2, "button": "left", "clickCount": 1 }),
    )
    .await?;
    Ok(())
}

/// 给一个元素设值并派发 input/change 事件。
///
/// `<select>` 的选择、以及 React 那种"必须走事件才认"的受控输入都靠它。
/// 走 [`Runtime.callFunctionOn`] 直接作用在 objectId 上，不必先聚焦。
pub async fn set_value(tab: Tab<'_>, object_id: &str, value: &str) -> Result<(), BrowserError> {
    let func = "function(v){ \
        this.value = v; \
        this.dispatchEvent(new Event('input', { bubbles: true })); \
        this.dispatchEvent(new Event('change', { bubbles: true })); \
    }";
    tab.cdp(
        "Runtime.callFunctionOn",
        json!({
            "objectId": object_id,
            "functionDeclaration": func,
            "arguments": [{ "value": value }],
        }),
    )
    .await
    .map(|_| ())
}

/// 按一个组合键，如 `Control+a`、`Meta+c`、`Control+Shift+k`。
///
/// `[约束]` 修饰键要走 `modifiers` 位掩码，而不是逐个按下再按主键。
/// Chromium 的快捷键匹配看的是**主键事件上带的修饰位**:全选是"按下 a 时
/// Ctrl 正按着"，拆成四个独立事件它一个也匹配不上 —— 现象是组合键全部
/// 静默失效。位:Alt=1、Ctrl=2、Meta=4、Shift=8。
pub async fn key_chord(tab: Tab<'_>, chord: &str) -> Result<(), BrowserError> {
    let mut modifiers = 0;
    let mut key = "";
    for part in chord.split('+') {
        match part.trim() {
            "Alt" | "Option" => modifiers |= 1,
            "Control" | "Ctrl" => modifiers |= 2,
            "Meta" | "Cmd" | "Command" => modifiers |= 4,
            "Shift" => modifiers |= 8,
            k => key = k,
        }
    }
    if key.is_empty() {
        return Err(BrowserError::Cdp {
            method: "Input.dispatchKeyEvent".into(),
            message: format!("组合键 `{chord}` 里没有主键"),
        });
    }
    let vk = vk_for(key);
    let down = json!({
        "type": "keyDown", "key": key, "modifiers": modifiers,
        "windowsVirtualKeyCode": vk,
    });
    let up = json!({
        "type": "keyUp", "key": key, "modifiers": modifiers,
        "windowsVirtualKeyCode": vk,
    });
    tab.cdp("Input.dispatchKeyEvent", down).await?;
    tab.cdp("Input.dispatchKeyEvent", up).await?;
    Ok(())
}

/// 组合键里主键的 Windows 虚拟键码。
///
/// 命名键复用 [`key_code`]；单个字母/数字按 ASCII 大写（`a` → 65），这正是
/// Chromium 给字母键的 vk。给不出就 0 —— 那对纯修饰组合是对的。
fn vk_for(key: &str) -> u32 {
    let named = key_code(key);
    if named != 0 {
        return named;
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphanumeric() => u32::from(c.to_ascii_uppercase() as u8),
        _ => 0,
    }
}

/// 当前聚焦的元素能不能接受文本输入。
///
/// 往不可编辑的元素里 insertText 不报错、也不产生任何效果 —— 静默
/// 什么都没发生是最难排查的失败，所以在输入前把它变成一个明确的回答。
pub async fn focused_editable(tab: Tab<'_>) -> Result<bool, BrowserError> {
    let r = tab
        .cdp(
            "Runtime.evaluate",
            json!({
                "expression": "(() => { \
                    const e = document.activeElement; \
                    if (!e) return false; \
                    if (e.isContentEditable) return true; \
                    return e.tagName === 'INPUT' || e.tagName === 'TEXTAREA'; \
                })()",
                "returnByValue": true,
            }),
        )
        .await?;
    Ok(r["result"]["value"].as_bool().unwrap_or(false))
}

/// 全选聚焦元素里的内容。配合 insertText 实现"替换原值"——
/// insertText 走的是 IME 提交路径，会把选区整个换成新文本。
pub async fn select_all(tab: Tab<'_>) -> Result<(), BrowserError> {
    tab.cdp(
        "Runtime.evaluate",
        json!({ "expression": "document.execCommand('selectAll')" }),
    )
    .await
    .map(|_| ())
}

/// 往聚焦元素里插入文本。
///
/// 走 insertText 而不是逐字符 keyDown —— 中文、emoji 没有对应键码，
/// 逐字符发根本发不出来。和面板输入的 [`super::access::Input::Text`]
/// 是同一条 CDP，语义一致。
pub async fn insert_text(tab: Tab<'_>, text: &str) -> Result<(), BrowserError> {
    tab.cdp("Input.insertText", json!({ "text": text }))
        .await
        .map(|_| ())
}

/// 按一个功能键（keyDown + keyUp）。
pub async fn press(tab: Tab<'_>, key: &str) -> Result<(), BrowserError> {
    let code = key_code(key);
    let mut down = json!({ "type": "keyDown", "key": key, "windowsVirtualKeyCode": code });
    // `[约束]` 回车必须带 text。"提交表单 / 换行"这些默认行为挂在 char
    // 事件上，而 CDP 只在 keyDown 带 text 时才生成 char —— 只发键码的
    // 回车，页面收得到 keydown，表单却纹丝不动（实测）。别的功能键
    // （退格、方向键）的默认行为走 keydown，不需要。
    if key == "Enter" {
        down["text"] = json!("\r");
    }
    tab.cdp("Input.dispatchKeyEvent", down).await?;
    tab.cdp(
        "Input.dispatchKeyEvent",
        json!({ "type": "keyUp", "key": key, "windowsVirtualKeyCode": code }),
    )
    .await?;
    Ok(())
}

/// 功能键的 Windows 虚拟键码。
///
/// `[约束]` 这几个键必须带键码。只发 `key` 字符串的话，Chromium 收得到
/// 事件但不会执行默认行为 —— 回车不提交表单、退格不删字符。看起来像
/// "按了没反应"，而事件其实是送到了的。
///
/// 只列常用的。列表外的键当普通文本处理，那对单字符键是对的。
pub fn key_code(key: &str) -> u32 {
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

/// 在视口中心合成一次滚轮，返回滚完后的（位置，可滚上限），CSS 像素。
///
/// 用滚轮而不是 `window.scrollBy`:滚轮落在中心点下方的元素上，页面里
/// 的内嵌滚动区（代码块、聊天列表）会正确接住它 —— JS 滚动只动最外层。
/// 位置回读等一小拍:滚轮经过合成器，落地不是同步的。
pub async fn scroll_by(tab: Tab<'_>, delta_y: f64) -> Result<(f64, f64), BrowserError> {
    let vp = viewport(tab).await?;
    tab.cdp(
        "Input.dispatchMouseEvent",
        json!({
            "type": "mouseWheel",
            "x": f64::from(vp.width) / 2.0,
            "y": f64::from(vp.height) / 2.0,
            "deltaX": 0.0,
            "deltaY": delta_y,
        }),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let r = tab
        .cdp(
            "Runtime.evaluate",
            json!({
                "expression": "[scrollY, Math.max(0, document.documentElement.scrollHeight - innerHeight)]",
                "returnByValue": true,
            }),
        )
        .await?;
    let v = &r["result"]["value"];
    Ok((
        v[0].as_f64().unwrap_or_default(),
        v[1].as_f64().unwrap_or_default(),
    ))
}

/// 交互之后等页面稳一稳。
///
/// 点击可能触发导航。先给同步跳转（`location.href = ...`、表单提交）
/// 一小拍时间把 readyState 打回 loading；真在加载就按导航的等法等到
/// 加载完 —— 不等的话，紧接着的截图拍到的是半张白屏。
pub async fn settle(tab: Tab<'_>) {
    tokio::time::sleep(Duration::from_millis(150)).await;
    let ready = tab
        .cdp(
            "Runtime.evaluate",
            json!({ "expression": "document.readyState", "returnByValue": true }),
        )
        .await;
    match ready {
        Ok(r) if r["result"]["value"].as_str() == Some("complete") => {}
        // loading，或者 agent 正在换文档（evaluate 会失败）—— 按导航等。
        _ => {
            let _ = wait_until_ready(tab).await;
        }
    }
}

/// 当前地址。问不到（还没有文档、正在换文档）就是空串。
pub async fn url_of(tab: Tab<'_>) -> String {
    tab.cdp(
        "Runtime.evaluate",
        json!({ "expression": "location.href", "returnByValue": true }),
    )
    .await
    .ok()
    .and_then(|v| v["result"]["value"].as_str().map(ToOwned::to_owned))
    .unwrap_or_default()
}

/// 命中测试用的函数:给视口坐标，返回那儿元素的 CSS 选择器 + 一句描述。
///
/// `document.elementFromPoint` 收的是视口 CSS 坐标 —— 面板转发的输入坐标
/// 正是这个（相对视口左上角）。选择器优先用 `#id`，否则往上拼 `tag:nth-of-type`
/// 链（最多 5 层，够定位又不至于长得没法用）。
const PICK_FN: &str = r#"(px, py) => {
    const el = document.elementFromPoint(px, py);
    if (!el || el.nodeType !== 1) return null;
    const esc = (s) => (window.CSS && CSS.escape) ? CSS.escape(String(s)) : String(s);
    const parts = [];
    let e = el;
    while (e && e.nodeType === 1 && parts.length < 5) {
        if (e.id) { parts.unshift('#' + esc(e.id)); break; }
        let part = e.tagName.toLowerCase();
        const p = e.parentElement;
        if (p) {
            const same = Array.prototype.filter.call(p.children, c => c.tagName === e.tagName);
            if (same.length > 1) part += ':nth-of-type(' + (same.indexOf(e) + 1) + ')';
        }
        parts.unshift(part);
        e = e.parentElement;
    }
    const selector = parts.join(' > ');
    const tag = el.tagName.toLowerCase();
    const idPart = el.id ? ('#' + el.id) : '';
    let clsPart = '';
    if (typeof el.className === 'string' && el.className.trim()) clsPart = '.' + el.className.trim().split(/\s+/).join('.');
    const text = (el.textContent || '').trim().replace(/\s+/g, ' ').slice(0, 60);
    const description = tag + idPart + clsPart + (text ? ' — "' + text + '"' : '');
    return { selector, description };
}"#;

/// 取件模式的悬停高亮:在页面里盖一个半透明蓝框到光标下的元素上（像
/// DevTools 的元素审查）。框走 `position:fixed` + `pointer-events:none`——
/// 不占布局、不参与命中测试（否则会把自己盖住的元素挡掉），随下一帧
/// screencast 一起画出来，前端不用做任何坐标换算。
///
/// 用一个常驻的 overlay div 反复挪位置，不是每次新建 —— 移动是高频操作。
const PICK_HOVER_FN: &str = r#"(px, py) => {
    let ov = document.getElementById('__riot_pick_ov__');
    if (!ov) {
        ov = document.createElement('div');
        ov.id = '__riot_pick_ov__';
        ov.style.cssText = 'position:fixed;pointer-events:none;z-index:2147483647;box-sizing:border-box;background:rgba(90,141,214,0.25);border:1px solid rgba(90,141,214,0.9);border-radius:2px;';
        document.documentElement.appendChild(ov);
    }
    const el = document.elementFromPoint(px, py);
    if (!el || el.id === '__riot_pick_ov__') { ov.style.display = 'none'; return; }
    const r = el.getBoundingClientRect();
    ov.style.display = 'block';
    ov.style.left = r.left + 'px';
    ov.style.top = r.top + 'px';
    ov.style.width = r.width + 'px';
    ov.style.height = r.height + 'px';
}"#;

/// 把高亮框移到 (x,y) 下的元素上。best-effort。
pub async fn pick_hover(tab: Tab<'_>, x: f64, y: f64) {
    let expr = format!("({PICK_HOVER_FN})({x}, {y})");
    let _ = tab
        .cdp("Runtime.evaluate", json!({ "expression": expr }))
        .await;
}

/// 撤掉悬停高亮框。退出取件模式、鼠标离开画面、或点选完成时调。
pub async fn pick_clear(tab: Tab<'_>) {
    let _ = tab
        .cdp(
            "Runtime.evaluate",
            json!({ "expression": "document.getElementById('__riot_pick_ov__')?.remove()" }),
        )
        .await;
}

/// 命中测试:视口坐标 → (选择器, 描述)。点不到元素返回 `None`。
///
/// `[取舍]` 走 `elementFromPoint` 一次 evaluate，不走 `DOM.getNodeForLocation`
/// 配 resolveNode 那套:前者一条命令拿到元素并当场算好选择器，后者要两三次
/// 往返、还得把节点搬进 Runtime 才能读属性。命中测试是用户点一下就要有反馈
/// 的交互，越短越好。
///
/// `format!` 里 `{PICK_FN}` 是把常量的**值**填进去（值里的 `{}` 不会被再解析），
/// 只有 `{x}`/`{y}` 是真正的格式参数。
pub async fn pick_at(tab: Tab<'_>, x: f64, y: f64) -> Option<(String, String)> {
    let expr = format!("({PICK_FN})({x}, {y})");
    let r = tab
        .cdp(
            "Runtime.evaluate",
            json!({ "expression": expr, "returnByValue": true }),
        )
        .await
        .ok()?;
    let v = &r["result"]["value"];
    if v.is_null() {
        return None;
    }
    let selector = v["selector"].as_str()?.to_owned();
    let description = v["description"].as_str().unwrap_or(&selector).to_owned();
    Some((selector, description))
}

/// 在即将操作的元素上闪一下红色高亮框，让盯着面板的用户看清模型接下来
/// 要动哪个元素（意图预览）。
///
/// `[取舍]` 直接 `callFunctionOn` 作用在这个 objectId 上，不靠选择器 ——
/// 对象就是马上要点的那个，不会框错。改的是内联 `outline`（不占布局、
/// 不引起重排），把原值存在 dataset 里、结束再还原。全程 best-effort:
/// 闪失败最多是少个提示，绝不能挡住真正的操作。
///
/// `[约束]` 只由 [`HostBrowser`] 在**面板有人看**时调用。没人看还闪，等于
/// 给 headless 自动化平白加一次 sleep —— 而那正是这个项目引以为傲的速度。
pub async fn flash_target(tab: Tab<'_>, object_id: &str) {
    const ADD: &str = "function(){\
        this.dataset.riotFlashOutline = this.style.outline || '';\
        this.dataset.riotFlashOffset = this.style.outlineOffset || '';\
        this.style.outline = '3px solid #ff3b30';\
        this.style.outlineOffset = '2px';\
    }";
    if tab
        .cdp(
            "Runtime.callFunctionOn",
            json!({ "objectId": object_id, "functionDeclaration": ADD }),
        )
        .await
        .is_err()
    {
        return; // 加不上就别等、也别费第二次调用去移除。
    }
    // 让高亮停留一瞬，肉眼能看见"要点这里"。这点延迟只在面板开着时付。
    tokio::time::sleep(Duration::from_millis(140)).await;
    const REMOVE: &str = "function(){\
        this.style.outline = this.dataset.riotFlashOutline || '';\
        this.style.outlineOffset = this.dataset.riotFlashOffset || '';\
        delete this.dataset.riotFlashOutline;\
        delete this.dataset.riotFlashOffset;\
    }";
    let _ = tab
        .cdp(
            "Runtime.callFunctionOn",
            json!({ "objectId": object_id, "functionDeclaration": REMOVE }),
        )
        .await;
}

/// 读一个元素对应的源码位置（文件:行）和组件名。作用在 objectId 上。
///
/// `[取舍]` 依赖开发构建里保留的调试信息，多框架各试一遍:
/// - React:DOM 节点上的 `__reactFiber$*`，沿 fiber 往上找带 `_debugSource`
///   的那层（`@babel/plugin-transform-react-jsx-source` 在 dev 下自动注入）；
///   找不到 source 就退而给组件名。
/// - Vue 3:`__vueParentComponent.type.__file`；Vue 2:`__vue__.$options.__file`。
///
/// 生产构建把这些都抹了 —— 那时返回 null，上层照实说"这个构建没有源码信息"，
/// 不猜。返回的是一个只含基本类型的小对象，`returnByValue` 安全（不会把整个
/// fiber 拖回来）。
const SOURCE_FN: &str = r#"function(){
    const el = this;
    const rk = Object.keys(el).find(k => k.startsWith('__reactFiber$') || k.startsWith('__reactInternalInstance$'));
    if (rk) {
        let f = el[rk];
        for (let i = 0; i < 12 && f; i++) {
            const s = f._debugSource;
            if (s && s.fileName) {
                let comp = null;
                const o = f._debugOwner;
                if (o && o.type) comp = o.type.displayName || o.type.name || null;
                if (!comp && typeof f.type === 'function') comp = f.type.displayName || f.type.name || null;
                return { framework: 'react', file: s.fileName, line: s.lineNumber || null, column: s.columnNumber || null, component: comp };
            }
            f = f.return;
        }
        let g = el[rk];
        for (let i = 0; i < 12 && g; i++) {
            if (typeof g.type === 'function') return { framework: 'react', file: null, line: null, column: null, component: g.type.displayName || g.type.name || '(匿名组件)' };
            g = g.return;
        }
    }
    if (el.__vueParentComponent) {
        const t = el.__vueParentComponent.type || {};
        return { framework: 'vue', file: t.__file || null, line: null, column: null, component: t.name || t.__name || null };
    }
    if (el.__vue__) {
        const o = (el.__vue__.$options) || {};
        return { framework: 'vue2', file: o.__file || null, line: null, column: null, component: o.name || null };
    }
    return null;
}"#;

pub async fn source_of(tab: Tab<'_>, object_id: &str) -> Result<String, BrowserError> {
    let r = tab
        .cdp(
            "Runtime.callFunctionOn",
            json!({
                "objectId": object_id,
                "functionDeclaration": SOURCE_FN,
                "returnByValue": true,
            }),
        )
        .await?;
    let v = &r["result"]["value"];
    if v.is_null() {
        return Ok("没找到源码信息。这个页面多半是生产构建（去掉了组件调试数据），\
                   或者不是 React / Vue —— 源码映射只在开发构建里有。"
            .to_owned());
    }
    let fw = v["framework"].as_str().unwrap_or("?");
    let comp = v["component"].as_str();
    let file = v["file"].as_str();
    let line = v["line"].as_i64();
    let col = v["column"].as_i64();

    let mut out = String::new();
    if let Some(c) = comp {
        out.push_str(&format!("组件：{c}\n"));
    }
    match (file, line) {
        (Some(f), Some(l)) => {
            let c = col.map(|c| format!(":{c}")).unwrap_or_default();
            out.push_str(&format!("源码：{f}:{l}{c}"));
        }
        (Some(f), None) => out.push_str(&format!("源码文件：{f}")),
        (None, _) => out.push_str(&format!(
            "框架：{fw}（这个构建没保留文件位置，只能给到组件名）"
        )),
    }
    Ok(out)
}

/// 文档身份令牌:`performance.timeOrigin` 在同一篇文档里恒定，跨导航/重载
/// 必变（新文档 = 新 origin）。
///
/// `[取舍]` 用它、不数 `LoadEnd` 事件来判断"页面在快照之后重载没有"。
/// 事件走异步事件流（子进程 → stdout → 宿主事件循环），可能晚于紧跟着
/// 快照来的那次交互到达 —— 那样会误报"页面重载过"。这个令牌是在**交互
/// 同一条 CDP 通道**上现读的，和快照记的那次同源，不存在这种竞态。
/// 问不到就返回 `None`（没有文档、正在换文档），调用方按"未知"处理。
pub async fn document_token(tab: Tab<'_>) -> Option<f64> {
    tab.cdp(
        "Runtime.evaluate",
        json!({ "expression": "performance.timeOrigin", "returnByValue": true }),
    )
    .await
    .ok()
    .and_then(|v| v["result"]["value"].as_f64())
}

/// evaluate 结果的文本上限。
///
/// 模型经常一不小心 `evaluate("document.body.innerHTML")` 把整页 DOM 拉回来。
/// 给个上限截断，比让它吃掉半个上下文强。
const MAX_EVAL_LEN: usize = 20_000;

/// 在页面里跑一段 JS，把结果整形成文本。
///
/// `[取舍]` `returnByValue` + `awaitPromise`:模型写 `await fetch(...)` 这种
/// 也能拿到值。结果按 JSON 回来，序列化成紧凑文本；非 JSON 值（函数、
/// DOM 节点）退回它的 `description`。脚本抛异常时把异常信息当错误抛出 ——
/// 那是模型的脚本错了，得让它看见。
pub async fn evaluate(tab: Tab<'_>, expr: &str) -> Result<String, BrowserError> {
    let r = tab
        .cdp(
            "Runtime.evaluate",
            json!({
                "expression": expr,
                "returnByValue": true,
                "awaitPromise": true,
                // 页面自己的死循环不该把工具永久挂住（外层还有 CDP_TIMEOUT 兜底）。
                "timeout": 5000,
            }),
        )
        .await?;

    if let Some(exc) = r.get("exceptionDetails") {
        let msg = exc["exception"]["description"]
            .as_str()
            .or_else(|| exc["exception"]["value"].as_str())
            .or_else(|| exc["text"].as_str())
            .unwrap_or("脚本抛出异常");
        return Err(BrowserError::Cdp {
            method: "Runtime.evaluate".into(),
            message: msg.to_owned(),
        });
    }

    let result = &r["result"];
    let text = if result.get("value").is_some() {
        // 字符串直接给（不加引号，模型要的是内容）；其它 JSON 紧凑序列化。
        match &result["value"] {
            Value::String(s) => s.clone(),
            v => serde_json::to_string(v).unwrap_or_else(|_| v.to_string()),
        }
    } else {
        // 没有 value:undefined、函数、DOM 节点这类。给个描述聊胜于无。
        result["description"]
            .as_str()
            .unwrap_or(result["type"].as_str().unwrap_or("undefined"))
            .to_owned()
    };

    Ok(truncate_chars(&text, MAX_EVAL_LEN))
}

/// 当前页面的 Cookie，整形成一行一条、带安全属性的文本。
///
/// 走 `Network.getCookies` 拿完整属性（含 HttpOnly，那是 `document.cookie`
/// 读不到的）。值可能是长长的 JWT，截断显示 —— 需要完整值时模型能再针对
/// 具体 cookie 追问。
pub async fn cookies(tab: Tab<'_>) -> Result<String, BrowserError> {
    let url = url_of(tab).await;
    let r = tab
        .cdp("Network.getCookies", json!({ "urls": [url] }))
        .await?;
    let list = r["cookies"].as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        return Ok("当前页面没有 Cookie。".to_owned());
    }
    let mut out = String::new();
    for c in &list {
        let name = c["name"].as_str().unwrap_or("");
        let value = truncate_chars(c["value"].as_str().unwrap_or(""), 60);
        let mut attrs = Vec::new();
        if c["httpOnly"].as_bool() == Some(true) {
            attrs.push("HttpOnly".to_owned());
        }
        if c["secure"].as_bool() == Some(true) {
            attrs.push("Secure".to_owned());
        }
        if let Some(ss) = c["sameSite"].as_str() {
            attrs.push(format!("SameSite={ss}"));
        }
        if let Some(d) = c["domain"].as_str() {
            attrs.push(format!("domain={d}"));
        }
        out.push_str(&format!("{name} = {value}"));
        if !attrs.is_empty() {
            out.push_str(&format!("  [{}]", attrs.join(", ")));
        }
        out.push('\n');
    }
    Ok(out)
}

/// 在页面上下文里重放一个请求，返回整形后的结果文本。
///
/// `[取舍]` 走页面的 `fetch` 而不是宿主的 HTTP 客户端:页面里发请求天然带
/// 当前会话的 cookie（`credentials: include`），这正是渗透要的"用已登录
/// 身份重放"。代价是受同源策略约束 —— 跨源重放会撞 CORS，那时结果里的
/// error 会说清楚。
pub async fn replay(
    tab: Tab<'_>,
    url: &str,
    method: &str,
    headers: &Value,
    body: Option<&str>,
) -> Result<String, BrowserError> {
    // init 对象整个用 JSON 拼，避免手工拼 JS 出引号/转义问题。
    let mut init = serde_json::Map::new();
    init.insert("method".into(), Value::String(method.to_owned()));
    init.insert("credentials".into(), Value::String("include".into()));
    if headers.is_object() {
        init.insert("headers".into(), headers.clone());
    }
    if let Some(b) = body {
        init.insert("body".into(), Value::String(b.to_owned()));
    }
    let init_json = Value::Object(init);

    let expr = format!(
        "(async () => {{ \
            try {{ \
                const r = await fetch({}, {}); \
                const text = await r.text(); \
                const h = {{}}; r.headers.forEach((v, k) => h[k] = v); \
                return JSON.stringify({{ status: r.status, headers: h, body: text.slice(0, {}) }}); \
            }} catch (e) {{ return JSON.stringify({{ error: String(e) }}); }} \
        }})()",
        serde_json::to_string(url).unwrap_or_else(|_| "\"\"".into()),
        init_json,
        MAX_EVAL_LEN,
    );

    let r = tab
        .cdp(
            "Runtime.evaluate",
            json!({ "expression": expr, "returnByValue": true, "awaitPromise": true }),
        )
        .await?;
    let raw = r["result"]["value"].as_str().unwrap_or("{}");
    let parsed: Value = serde_json::from_str(raw).unwrap_or(Value::Null);

    if let Some(err) = parsed.get("error").and_then(Value::as_str) {
        return Ok(format!(
            "重放失败:{err}\n（跨源请求会撞 CORS —— 同源接口才能带会话重放。）"
        ));
    }
    let status = parsed["status"].as_i64().unwrap_or(0);
    let headers = parsed["headers"].as_object();
    let body = parsed["body"].as_str().unwrap_or_default();

    let mut out = format!("状态:{status}\n响应头:\n");
    if let Some(h) = headers {
        let mut lines: Vec<String> = h
            .iter()
            .map(|(k, v)| format!("  {k}: {}", v.as_str().unwrap_or_default()))
            .collect();
        lines.sort();
        out.push_str(&lines.join("\n"));
    }
    out.push_str(&format!(
        "\n响应体:\n{}",
        truncate_chars(body, MAX_EVAL_LEN)
    ));
    Ok(out)
}

/// 按**字符**（不是字节）截断，避免把多字节字符切成半个。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}…（已截断，共 {} 字）", s.chars().count())
}

/// 取 console 里累积的消息。
///
/// `[前提]` 依赖 `Log.enable` 已经开着 —— 它只回放**开启之后**的记录。
/// 页面加载期间的报错要在导航前就 enable 才抓得到，那由调用方保证。
pub async fn console(tab: Tab<'_>) -> Result<Vec<String>, BrowserError> {
    let r = tab.cdp("Runtime.evaluate", json!({
        // CDP 没有"把历史 console 拿回来"的方法，Log.entryAdded 是推送式的。
        // 这里读的是注入脚本攒下的缓冲区，没注入过就返回空数组。
        "expression": "globalThis.__riotConsole ? JSON.stringify(globalThis.__riotConsole) : '[]'",
        "returnByValue": true,
    })).await?;

    let raw = r["result"]["value"].as_str().unwrap_or("[]");
    let list: Vec<String> = serde_json::from_str(raw).unwrap_or_default();
    Ok(list.into_iter().take(MAX_CONSOLE).collect())
}

/// 禁掉页面滚到边缘时的橡皮筋回弹。
///
/// `[约束]` 只能靠往每个文档注 CSS，别的路都不通:macOS 上 Chromium 的
/// 回弹跟随系统偏好、代码里对 Apple 平台**写死启用**（见 Chromium
/// `ui_base_switches_util.cc` 的 `IsElasticOverscrollEnabled`），
/// `--disable-features=ElasticOverscroll` 只对 Windows/Android 生效。
///
/// 为什么要禁:面板的画面是远程帧，回弹发生在另一个进程的合成器里 ——
/// 用户这边没有触控板的手感配合，看到的只是"整页在窗口里晃了一下再弹
/// 回去"，像画面故障。更糟的是回弹瞬间会把布局视口外的内容拽进画面，
/// 排查显示问题时它制造过一整条假线索（"平时看不到的输入框闪出来了"）。
///
/// `[约束]` `html` 和 `body` 都要钉。viewport 的 overscroll-behavior 传播
/// 历史上从 body 读过（和 overflow 的兼容行为一族），只钉 html 等于赌
/// Chromium 版本行为。`!important` 是防页面自己的样式把它翻回去。
///
/// `[约束]` 要处理 `documentElement` 还是 null 的情况。
/// `addScriptToEvaluateOnNewDocument` 跑在新文档的 DOM 建立**之前** ——
/// 那一刻 appendChild 直接抛异常，样式一个字都没进去，而且没有任何报错
/// 冒到宿主（console 钩子不碰 DOM，所以同一条注入路上它毫发无损，
/// 很容易误以为这条路对 DOM 也是安全的）。等 DOMContentLoaded 再钉。
const NO_BOUNCE: &str = r#"(() => {
  const ID = '__riot_no_bounce__';
  const pin = () => {
    if (!document.documentElement || document.getElementById(ID)) return;
    const s = document.createElement('style');
    s.id = ID;
    s.textContent = 'html, body { overscroll-behavior: none !important; }';
    document.documentElement.appendChild(s);
  };
  if (document.documentElement) pin();
  else addEventListener('DOMContentLoaded', pin);
})()"#;

/// 在页面里装一个 console 缓冲区。
///
/// `[约束]` 必须用 `Page.addScriptToEvaluateOnNewDocument` 而不是直接
/// `Runtime.evaluate`:后者注入的东西活不过下一次导航，而 console 报错
/// 恰恰最常出现在页面加载阶段 —— 等模型想起来看的时候早就没了。
pub async fn install_console_hook(tab: Tab<'_>) -> Result<(), BrowserError> {
    const HOOK: &str = r"
        (() => {
          if (globalThis.__riotConsole) return;
          const buf = globalThis.__riotConsole = [];
          for (const level of ['log', 'info', 'warn', 'error']) {
            const orig = console[level].bind(console);
            console[level] = (...args) => {
              if (buf.length < 500) {
                buf.push(level + ': ' + args.map(a => {
                  try { return typeof a === 'string' ? a : JSON.stringify(a); }
                  catch { return String(a); }
                }).join(' '));
              }
              orig(...args);
            };
          }
          addEventListener('error', e => { buf.push('error: ' + e.message); });
          addEventListener('unhandledrejection', e => {
            buf.push('error: unhandled rejection: ' + e.reason);
          });
        })()
    ";
    tab.cdp(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({ "source": HOOK }),
    )
    .await?;
    // 当前这个文档是在注入之前加载的，补一次。
    tab.cdp("Runtime.evaluate", json!({ "expression": HOOK }))
        .await?;

    // 禁回弹和 console 钩子同款:每个新文档注一遍 + 当前文档补一次。
    // 这个函数本就是"每个标签页一次的初始化"，搭同一趟车。
    tab.cdp(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({ "source": NO_BOUNCE }),
    )
    .await?;
    tab.cdp("Runtime.evaluate", json!({ "expression": NO_BOUNCE }))
        .await?;

    // 顺手开 Page 域。JS 对话框（alert/confirm/beforeunload）的事件只在
    // Page.enable 之后才推送，而事件循环要靠它来自动放行对话框 ——
    // 不开的话，页面弹一个 confirm 就把自动化永久卡住（见 access 的事件
    // 循环）。装 console 钩子本就是每个标签页一次的初始化，搭这一趟车。
    tab.cdp("Page.enable", json!({})).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 结构性节点被滤掉() {
        // 一个页面里 generic / StaticText 能占到八成节点，全留下来的话
        // a11y 快照会比 DOM 还长，那这个取舍就白做了。
        for role in ["generic", "none", "InlineTextBox", "StaticText"] {
            let n = json!({ "role": { "value": role }, "name": { "value": "x" } });
            assert!(describe_node(&n).is_none(), "{role} 应当被滤掉");
        }
    }

    #[test]
    fn 可交互节点带上名字() {
        let n = json!({
            "role": { "value": "button" },
            "name": { "value": "提交" },
        });
        assert_eq!(describe_node(&n).as_deref(), Some(r#"button "提交""#));
    }

    #[test]
    fn 超长的名字会被截断() {
        // 一整段说明文字挂在某个节点上是常见的，不截断的话单个节点
        // 就能吃掉几百 token。
        let long = "很".repeat(300);
        let n = json!({ "role": { "value": "link" }, "name": { "value": long } });
        let out = describe_node(&n).expect("有描述");
        assert!(
            out.chars().count() < 100,
            "应当截断，实际 {} 字",
            out.chars().count()
        );
    }

    #[test]
    fn 被忽略的节点不出现() {
        let n = json!({
            "ignored": true,
            "role": { "value": "button" },
            "name": { "value": "隐藏的" },
        });
        assert!(describe_node(&n).is_none());
    }

    /// 编号必须连续，而且和映射一一对应。
    ///
    /// 编号是模型的操作句柄:文本里写 `[2]` 而映射里没有 2，模型的点击
    /// 会得到"编号无效"——它没有任何办法知道是自己错了还是快照错了。
    #[test]
    fn 编号连续且和映射对得上() {
        let nodes = vec![
            json!({ "role": { "value": "button" }, "name": { "value": "提交" },
                    "backendDOMNodeId": 11 }),
            // 被滤掉的节点不占号
            json!({ "role": { "value": "generic" }, "name": { "value": "x" },
                    "backendDOMNodeId": 12 }),
            json!({ "role": { "value": "textbox" }, "name": { "value": "邮箱" },
                    "backendDOMNodeId": 13 }),
        ];
        let (text, refs) = render_nodes(&nodes, &HashMap::new());

        assert!(text.contains("[1] button \"提交\""), "{text}");
        assert!(text.contains("[2] textbox \"邮箱\""), "{text}");
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[&1].backend_id, 11);
        assert_eq!(refs[&2].backend_id, 13);
        assert_eq!(refs[&2].label, "textbox \"邮箱\"");
    }

    /// 没有 backendDOMNodeId 的节点不发号。
    ///
    /// 发了也点不了 —— 模型会拿着一个"看起来能点"的编号反复撞墙。
    #[test]
    fn 定位不了的节点不发号() {
        let nodes = vec![
            json!({ "role": { "value": "heading" }, "name": { "value": "标题" } }),
            json!({ "role": { "value": "button" }, "name": { "value": "好" },
                    "backendDOMNodeId": 7 }),
        ];
        let (text, refs) = render_nodes(&nodes, &HashMap::new());

        assert!(
            text.contains("heading \"标题\"\n"),
            "无号节点原样输出：{text}"
        );
        assert!(!text.contains("[1] heading"), "不能给它发号：{text}");
        assert!(
            text.contains("[1] button \"好\""),
            "号从能点的第一个开始：{text}"
        );
        assert_eq!(refs[&1].backend_id, 7);
    }

    /// 有几何的元素带上视口矩形，拿不到的留空而不是崩。
    ///
    /// `rect` 是给模型叠编号框、判断"要不要先滚动"用的。按 `backendDOMNodeId`
    /// 关联 —— iframe 内、`display:none` 的元素在几何表里没有条目，那时
    /// `rect` 必须是 `None`，而不是一个零矩形（零矩形会让框叠在左上角）。
    #[test]
    fn 编号元素带上几何() {
        let nodes = vec![
            json!({ "role": { "value": "button" }, "name": { "value": "提交" },
                    "backendDOMNodeId": 11 }),
            json!({ "role": { "value": "link" }, "name": { "value": "关于" },
                    "backendDOMNodeId": 99 }),
        ];
        let mut rects = HashMap::new();
        rects.insert(
            11_i64,
            Rect {
                x: 10.0,
                y: 20.0,
                w: 80.0,
                h: 30.0,
            },
        );
        // 99 号故意不给几何 —— iframe 内 / display:none 的形态。

        let (_text, refs) = render_nodes(&nodes, &rects);

        assert_eq!(
            refs[&1].rect,
            Some(Rect {
                x: 10.0,
                y: 20.0,
                w: 80.0,
                h: 30.0
            })
        );
        assert_eq!(refs[&2].rect, None, "拿不到几何的元素 rect 该是 None");
    }

    /// Set-of-Marks 只框可交互控件，容器角色不框。
    ///
    /// 框住 main / navigation 这类整块容器对"点哪个"没帮助，还会糊满画面、
    /// 和真正的控件框叠在一起。label 形如 `button "提交"`，取首词当角色。
    #[test]
    fn 可交互角色才叠框() {
        assert!(is_markable("button \"提交\""));
        assert!(is_markable("link \"帮助\""));
        assert!(is_markable("textbox \"邮箱\""));
        assert!(is_markable("checkbox"), "无名字的控件也要框");
        assert!(!is_markable("main"), "容器不叠框");
        assert!(!is_markable("navigation \"侧栏\""), "容器不叠框");
        assert!(!is_markable("heading \"标题\""), "标题不是可交互控件");
    }

    #[test]
    fn 功能键都有键码() {
        // 键码是 0 的功能键会"按了没反应"——事件送到了，默认行为不执行。
        for key in [
            "Enter",
            "Backspace",
            "Tab",
            "Escape",
            "ArrowLeft",
            "ArrowUp",
            "ArrowRight",
            "ArrowDown",
            "Delete",
            "Home",
            "End",
            "PageUp",
            "PageDown",
        ] {
            assert_ne!(key_code(key), 0, "{key} 缺键码");
        }
        assert_eq!(key_code("F13"), 0, "列表外的键不硬编");
    }
}
