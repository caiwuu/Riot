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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use riot_protocol::browser::{
    Action, BLANK_PAGE, BrowserAccess, BrowserUnavailable, Command, Event, InteractError,
    InterceptOp, MarkedView, Nav, NetQuery, TabId, Target, WaitCondition,
};
use tokio::sync::{Mutex, mpsc, oneshot};

use super::{Browser, Tab, ops};

/// 开一个标签页的等待上限。
///
/// 比进程启动那 30 秒短得多:进程已经在跑了，这里等的只是 CEF 建一个
/// browser —— 那是毫秒级的事。等太久的话，一次偶发失败会让"新建标签页"
/// 这个按钮僵住半分钟。
const TAB_OPEN_TIMEOUT: Duration = Duration::from_secs(10);

/// 页面自己要求开页时，标签页总数的上限。见 [`HostBrowser::open_popup`]。
///
/// 只管这条路。用户点"新建标签页"和模型开页是明确的意图，不该被拦。
const POPUP_TAB_LIMIT: usize = 24;

/// 一帧画面。
///
/// `data` 是 JPEG 的**原始字节**。CDP 给的是 base64，在这一层就解掉 ——
/// Rust 里解一帧是微秒级；留给前端的话，几百 KB 的字符串要以 JSON 穿过
/// IPC、在 JS 主线程上 parse、再交给 `data:` URL 同步解码，每一步都压在
/// 那条同时要处理输入事件的线程上，帧率一高面板就卡。二进制的打包格式
/// 见 `browser_open`。
#[derive(Debug, Clone)]
pub struct Frame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// 工具栏要显示的东西：当前地址，以及前进后退能不能按。
///
/// 三样一起回而不是分三个查询:它们来自同一次 `Page.getNavigationHistory`，
/// 拆开问不但多两次往返，还会撞上"地址已经变了但按钮还是上一页的状态"
/// 这种自己和自己打架的中间态。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NavState {
    /// 页面真正所在的地址。空表示浏览器还没起来。
    pub url: String,
    pub can_back: bool,
    pub can_forward: bool,
}

/// 面板转发过来的一次输入。
///
/// 坐标是**页面坐标**（相对视口左上角，CSS 像素）。面板负责把自己的
/// DOM 坐标换算过来 —— 它知道自己的缩放比例，这一层不知道。
///
/// `[约束]` `rename_all_fields` 不能省。容器上的 `rename_all` 只改**变体名**，
/// 变体里的字段照旧是 snake_case —— 于是前端发的 `deltaY` 对不上 `delta_y`，
/// 整条命令在 Tauri 解析参数的阶段就失败了。失败的现象是"滚轮转了没反应"：
/// 前端拿到的是一个 reject 的 Promise，宿主侧连日志都没有。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Input {
    /// 按下并抬起。合成一次完整点击，而不是让前端发两条 —— 中间要是
    /// 丢了一条，页面会停在"按住"状态，后续所有交互都不对。
    Click {
        x: f64,
        y: f64,
        button: String,
    },
    /// 单独的按下 / 抬起。面板转发原生 mousedown/mouseup（而不只是合成的
    /// click），页面里才拖得动滑块、选得中文字。`click_count` 让双击选词、
    /// 三击选段成立。丢一条会停在按住态，但那是真实鼠标本来就有的风险，
    /// 换来的是完整的指针语义。
    Down {
        x: f64,
        y: f64,
        button: String,
        click_count: i64,
    },
    Up {
        x: f64,
        y: f64,
        button: String,
        click_count: i64,
    },
    Move {
        x: f64,
        y: f64,
    },
    /// 滚轮。两个轴都要带。
    ///
    /// `[约束]` `delta_x` 不能丢。页面比视口宽是常态（面板通常只有半个
    /// 窗口宽，而多数站点的布局有个上千像素的最小宽度），横向滚不动的话
    /// 右边那一截内容永远看不到 —— 而且看起来像是页面被裁掉了，不像是
    /// 输入没送到。
    Scroll {
        x: f64,
        y: f64,
        delta_x: f64,
        delta_y: f64,
    },
    /// 输入文本。走 insertText 而不是逐字符 keyDown ——
    /// 中文、emoji 这些没有对应键码，逐字符发根本发不出来。
    Text {
        text: String,
    },
    /// 输入法正在组字：`text` 是还没上屏的临时内容，空串表示取消。
    ///
    /// 只发最终结果也能用，但页面在整个打字过程中一个字都不显示 ——
    /// 带自动补全的搜索框会一直是空的，直到你按下回车。
    Compose {
        text: String,
    },
    /// 功能键（Enter、Backspace、方向键之类）。
    Key {
        key: String,
    },
}

/// 标签栏上的一页。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabInfo {
    pub id: TabId,
    /// 页面地址。空 = 停在空白页，也就是"新标签页"。
    pub url: String,
    /// 页面标题。加载完之前是空的 —— 那时候前端显示"新标签页"。
    pub title: String,
    pub can_back: bool,
    pub can_forward: bool,
}

/// 面板要显示的全部状态。
///
/// 标签栏和工具栏一起回:它们描述的是同一个时刻。分两条查询的话，切标签的
/// 瞬间会出现"标签栏已经高亮了新页、地址栏还是旧页"这种自己和自己打架的
/// 中间态。
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelState {
    pub tabs: Vec<TabInfo>,
    /// 当前显示的那一页。没有标签页时是 0（不是合法的号）。
    pub active: TabId,
}

impl PanelState {
    /// 当前显示的那一页。
    ///
    /// 工具栏（地址、前进后退）读的就是这一条。让调用方各自在列表里按号找的
    /// 话，那段查找会在每个用到工具栏的地方重复一遍，而"没找到"的分支各写
    /// 各的 —— 这里统一回一个空的占位:什么都没打开时工具栏本该是空的。
    pub fn active_tab(&self) -> TabInfo {
        self.tabs
            .iter()
            .find(|t| t.id == self.active)
            .cloned()
            .unwrap_or_default()
    }
}

/// 标签页清单。
struct Tabs {
    /// 下一个要发的号。只增不减 —— 复用号会让"关掉的那页的延迟事件"
    /// 落到新页上。
    next: TabId,
    /// 标签栏上的顺序。
    order: Vec<TabId>,
    active: TabId,
}

impl Default for Tabs {
    fn default() -> Self {
        Self {
            next: 1,
            order: Vec::new(),
            active: 0,
        }
    }
}

pub struct HostBrowser {
    /// 指回自己。
    ///
    /// `[约束]` 事件循环那个 spawn 的任务需要它。浏览器进程会主动报事情
    /// （某一页关掉了、某个页面要开新标签页），处理这些要用到几乎整个
    /// `HostBrowser`（分号、清单、视口、画面出口），一个个 `Arc` 字段传
    /// 进去等于把这个结构拆散。用 `Weak` 而不是 `Arc`:任务持有强引用会让
    /// 会话结束后这个结构永远不释放，连着 CEF 那六个进程一起留下。
    me: Weak<Self>,
    /// `.app` 的位置。
    app: PathBuf,
    /// 数据目录。每个会话一份 —— 同一个目录不能有两个 Chromium 实例。
    profile: PathBuf,
    /// 起好的进程。第一次用到时填上。
    inner: Mutex<Option<Arc<Browser>>>,
    /// 画面出口。面板打开时装上，关闭时摘掉。
    frames: Arc<Mutex<Option<mpsc::UnboundedSender<Frame>>>>,
    tabs: Mutex<Tabs>,
    /// 在等 `TabOpened` 的人。事件流收到就唤醒。
    opening: Arc<Mutex<HashMap<TabId, oneshot::Sender<()>>>>,
    /// 正在推画面的那一页。
    ///
    /// `[约束]` 帧必须按它过滤。切标签是"停旧的、开新的"两条 CDP 命令，
    /// 中间旧页还会再来几帧 —— 不过滤的话，那几帧会画在新标签上，看起来
    /// 像切换失灵或者切错了页。
    streaming: Arc<Mutex<Option<TabId>>>,
    /// 面板最近一次报的视口。新开的标签页直接按它来。
    ///
    /// `[约束]` 必须记住。前端只在尺寸**变化**时才发 resize，而开新标签页
    /// 时尺寸没变 —— 新页于是停在子进程那个 1280×800 的初值上，切过去
    /// 第一眼是带黑边、缩小了的页面，还要等下一次拖动窗口才会纠正。
    view: Mutex<Option<(i32, i32, f32)>>,
    /// screencast JPEG 的物理像素上限 = 面板画面区 × 密度。
    ///
    /// `[约束]` 不能跟 `view` 绑死。Web 模式视口钉在 1280，面板可能只有
    /// 四百宽：按视口出的是 2560 宽的 JPEG，再缩进面板。编码晚一截，
    /// 滚动就有肉眼可见的延迟。模型截图走另一条（整页 PNG/JPEG），
    /// 不受这个上限影响。`None` = 还没收到面板尺寸，用兜底值。
    cast: Mutex<Option<(u32, u32)>>,
    /// 「没有页就开一页」这件事的互斥锁。
    ///
    /// `[约束]` 检查和创建之间不能有别人插进来。两个并发调用都看到"一页都
    /// 没有"的话，会各开一个 —— 现象是打开面板出现两个空标签页。
    ///
    /// 这在 dev 下是必然发生的:React 的 StrictMode 把 effect 跑两遍，
    /// `browser_open` 连着发两次。生产下少了那一层，但模型的工具和面板同时
    /// 启动是同一个形状的竞态。
    ///
    /// 不能拿 `tabs` 那把锁来兜:开页的过程本身要多次借它（分号、登记），
    /// 全程持有会自锁。
    ensuring: Mutex<()>,
    /// 最近一次快照发出去的元素编号。交互（点击、输入）拿编号来换元素。
    ///
    /// 记着"是在哪个标签页拍的":用户中途切了标签，旧编号指的是另一页
    /// 上的东西 —— 拿去点会点到坐标恰好重合的无关元素。`None` = 还没
    /// 拍过快照。
    ///
    /// 页面自己变了（脚本改 DOM、页内跳转）不主动清:backendDOMNodeId
    /// 跟着节点走，节点还在就照常能点；节点没了 CDP 会报错，那条错误
    /// 会被整形成"重新快照"的提示。
    snap_refs: Mutex<Option<(TabId, HashMap<u32, ops::SnapRef>)>>,
    /// 每个标签页累积的 CDP 事件（抓包、对话框、日志）。见 [`super::taps`]。
    ///
    /// `[约束]` 用 `Arc`:事件循环那个 spawn 的任务要长期持有它往里 `ingest`，
    /// 而工具走 `HostBrowser` 的方法读它 —— 两条路访问同一份状态，只能共享
    /// 所有权。和 `frames`/`streaming` 同一个道理。
    taps: Arc<Mutex<HashMap<TabId, super::taps::EventTaps>>>,
    /// 每个标签页的请求拦截规则。事件循环按它处理 `Fetch.requestPaused`。
    ///
    /// `[约束]` 同样 `Arc`:事件循环要读它来决定放行/拦截，工具要写它来加规则。
    /// 空表 = 没开拦截（那时 `Fetch` 是关的，根本不会有 requestPaused）。
    intercept: Arc<Mutex<HashMap<TabId, Vec<InterceptRule>>>>,
    /// 「页面要求开页」这件事的互斥锁。见 [`HostBrowser::open_popup`]。
    ///
    /// `[约束]` 数页数和开页之间不能有别人插进来。一个页面在 onload 里连开
    /// 几十个的时候，那几十个请求会同时到 —— 各自数一遍都看到"还没到上限"，
    /// 于是上限一条也没拦住。持锁串起来之后，第 25 个数到的是真实的 24。
    popping: Mutex<()>,
    /// 标签清单的变更通知出口。前端的工作台标签栏开着浏览器组时装上。
    ///
    /// `[约束]` 开页 / 关页 / 切页的**瞬间**要 ping 一声。页面标签渲染在
    /// 前端的统一标签栏上，只靠一秒一次的轮询对齐的话，点开新页要等下一拍
    /// 才在标签栏上冒出来 —— 而画面（帧流）是即时的，肉眼看得出谁先谁后。
    /// ping 不带内容:前端收到就重查一次 [`Self::state`]，这边不再算一份
    /// 状态塞进通知里，免得和轮询的回包互相乱序。
    tabs_watch: Mutex<Option<mpsc::UnboundedSender<()>>>,
}

/// 一条请求拦截规则:URL 含 `needle` 就执行 `action`。
#[derive(Clone)]
struct InterceptRule {
    needle: String,
    action: InterceptAction,
}

#[derive(Clone)]
enum InterceptAction {
    /// 直接失败（BlockedByClient）。
    Block,
    /// 伪造响应:给定状态码和响应体。
    Fulfill { status: u32, body: String },
}

impl HostBrowser {
    /// 直接回 `Arc`:事件循环要一个指回来的弱引用（见 [`Self::me`]），
    /// 而那个引用只能在 `Arc` 建好的同时拿到。
    pub fn new(app: PathBuf, profile: PathBuf) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            me: me.clone(),
            app,
            profile,
            inner: Mutex::new(None),
            frames: Arc::default(),
            tabs: Mutex::default(),
            opening: Arc::default(),
            streaming: Arc::default(),
            view: Mutex::default(),
            cast: Mutex::default(),
            ensuring: Mutex::default(),
            snap_refs: Mutex::default(),
            taps: Arc::default(),
            intercept: Arc::default(),
            popping: Mutex::default(),
            tabs_watch: Mutex::default(),
        })
    }

    /// 装上标签清单的变更通知出口。已有的直接替换 —— 订阅方只会有一个
    /// （工作台标签栏），旧通道属于它的上一次挂载。
    pub async fn watch_tabs(&self, tx: mpsc::UnboundedSender<()>) {
        *self.tabs_watch.lock().await = Some(tx);
    }

    /// 标签清单变了（开 / 关 / 切页）。发不出去说明前端那头已经没了，
    /// 顺手摘掉，别在每次变更时都对着空处扔。
    async fn ping_tabs(&self) {
        let mut slot = self.tabs_watch.lock().await;
        if let Some(tx) = slot.as_ref()
            && tx.send(()).is_err()
        {
            *slot = None;
        }
    }

    /// 当前活动的标签页；一个都没有就开一个。
    ///
    /// 面板上的每个动作都从这里拿页面。返回进程句柄和号，调用方自己拼
    /// [`Tab`] —— 借用关系让编译器盯着"句柄别比进程活得久"。
    async fn active(&self) -> Result<(Arc<Browser>, TabId), BrowserUnavailable> {
        let b = self.get().await?;
        // 先无锁看一眼。绝大多数调用都走这条 —— 已经有页了，不必去排队。
        if let Some(id) = self.current().await {
            return Ok((b, id));
        }
        // 真的要开页了才排队。见 `ensuring` 的说明:检查和创建之间放进别人，
        // 结果是两个空标签页。
        let _guard = self.ensuring.lock().await;
        if let Some(id) = self.current().await {
            return Ok((b, id));
        }
        let id = self.spawn_tab(&b, None, true).await?;
        Ok((b, id))
    }

    /// 当前活动页，一页都没有时是 `None`。
    async fn current(&self) -> Option<TabId> {
        let tabs = self.tabs.lock().await;
        (!tabs.order.is_empty()).then_some(tabs.active)
    }

    /// 开一个标签页并切过去。
    pub async fn open_tab(&self) -> Result<PanelState, BrowserUnavailable> {
        let b = self.get().await?;
        self.spawn_tab(&b, None, true).await?;
        self.state().await
    }

    /// 关一个标签页。
    ///
    /// `[取舍]` 不等 `TabClosed`。CEF 的销毁要走一圈（渲染进程收尾、
    /// `on_before_close`），等在这里的话点关闭之后标签要过一两百毫秒才消失。
    /// 而这一层的清单是给界面看的，先摘掉就够。
    ///
    /// 关掉最后一页就是空清单，**不补新页**。面板据此把自己关掉 —— 和浏览器
    /// 关掉最后一个标签页等于关窗口是一个道理。补一页的话，那个关闭按钮
    /// 在只剩一页时会变成"清空当前页"，按下去什么都没发生。
    pub async fn close_tab(&self, tab: TabId) -> Result<PanelState, BrowserUnavailable> {
        let b = self.get().await?;
        // 先从清单里摘掉，再发关闭命令。反过来的话，快速连点两次都会看到它
        // 还在清单里，于是各发一条 CloseTab，第二条在子进程那边以
        // "标签页不存在"报错 —— 一条纯噪音的报错，而且指向的是用户刚刚
        // 正常关掉的那一页。摘掉这一步是持锁做的，第二次进来就什么都不做了。
        if self.forget_tab(tab).await {
            b.send(&Command::CloseTab { tab })
                .map_err(|e| BrowserUnavailable(e.to_string()))?;
        }
        self.state().await
    }

    /// 某一页没了 —— 把这一层记着的它全丢掉，画面挪到还活着的页上。
    ///
    /// `[约束]` 这条路必须能被浏览器进程触发，不能只在用户点关闭时走。页面
    /// 自己 `window.close()`、渲染进程崩掉、脚本开的那一页被关掉，都会让
    /// 一个号在子进程那边消失而这一层不知情 —— 而那个号很可能正是"当前页"。
    /// 之后每条命令都以"标签页不存在"被丢掉，每次 CDP 调用都要等满 30 秒:
    /// 面板卡死，地址栏空着，没有任何一条报错说得出"那一页已经没了"。
    /// 事件循环收到 [`Event::TabClosed`] 就调这里。
    ///
    /// 回"清单里本来有它没有"。不认识的号什么都不做 —— 同一页会被清两次
    /// （用户点关闭，然后子进程报 `TabClosed`），[`Self::close_tab`] 还靠
    /// 这个返回值判断该不该真的发关闭命令。
    async fn forget_tab(&self, tab: TabId) -> bool {
        // 不走 `get()`:这条路是被事件驱动的，进程要是已经没了，为它再起一个
        // 完全说不通 —— 整份清单会由 [`Self::forget_crashed`] 一起清掉。
        let Some(b) = self.live().await else {
            return false;
        };

        let left = {
            let mut tabs = self.tabs.lock().await;
            let Some(at) = tabs.order.iter().position(|&t| t == tab) else {
                return false;
            };
            tabs.order.remove(at);
            if tabs.active == tab {
                // 顶掉右边那个，没有右边就取左边 —— 和常见浏览器一致。
                tabs.active = tabs
                    .order
                    .get(at)
                    .or_else(|| tabs.order.last())
                    .copied()
                    .unwrap_or(0);
            }
            tabs.active
        };

        // 这些表都按标签页号存。留着不清是两件事:一是白占内存（抓包能攒到
        // 几 MB），二是号虽然不复用、但快照编号指向的节点已经跟着页面走了 ——
        // 拿旧编号去点会落在一个不存在的元素上。
        self.taps.lock().await.remove(&tab);
        self.intercept.lock().await.remove(&tab);
        {
            let mut refs = self.snap_refs.lock().await;
            if refs.as_ref().is_some_and(|(t, _)| *t == tab) {
                *refs = None;
            }
        }

        if left == 0 {
            // 没页了，画面也就没有出处。不摘掉的话，下次开页时 stream()
            // 会以为旧页还在推，先对一个已经关掉的页面发 stopScreencast。
            *self.streaming.lock().await = None;
        } else {
            self.stream(&b, left).await;
        }
        self.ping_tabs().await;
        true
    }

    /// 进程崩了 —— 把这一层记着的、属于它的东西全丢掉。
    ///
    /// [`Self::forget_tab`] 的整份版本。由 [`Self::get`] 在重开之前调用。
    ///
    /// `[约束]` 标签页清单必须清空。那些号在新进程里一个都不存在，留着的话
    /// 面板会显示一排幻影标签，而发给它们的每条命令都在子进程那边以
    /// "标签页不存在"被丢掉 —— 和 [`Self::forget_tab`] 文档里说的是同一种
    /// 卡死，只是这次整份清单都是死的。
    ///
    /// `[约束]` 不能碰 `view`。它记的是**面板**的尺寸，和进程没关系 ——
    /// 清掉的话，重开后的第一页会停在子进程那个 1280×800 的初值上（前端只在
    /// 尺寸变化时才发 resize，而崩溃前后面板尺寸没变），画面带黑边、字被缩小，
    /// 一直到用户拖一下窗口才纠正。见 [`Self::view`]。
    ///
    /// 同理不碰 `frames`:面板还开着，画面出口照旧有效。新页开出来时
    /// [`Self::stream`] 会照它把 screencast 接上，画面自己就回来了。
    async fn forget_crashed(&self) {
        {
            let mut tabs = self.tabs.lock().await;
            tabs.order.clear();
            tabs.active = 0;
            // `next` 刻意不重置。号只增不减 —— 从 1 重新发的话，新进程的
            // 1 号和刚消失的 1 号同号，而那些按号索引的表（等待者、抓包、
            // 拦截规则）分不出两者。
        }
        // 等 `TabOpened` 的人永远等不到了 —— 那条事件本该由旧进程的事件流
        // 送来。drop 掉唤醒端让它们立刻拿到"浏览器进程退出了"，而不是各自
        // 等满 10 秒（见 [`Self::spawn_tab`] 里那个 `Ok(Err(_))` 分支）。
        self.opening.lock().await.clear();
        // 画面没有出处了。不清的话，重开后第一次 [`Self::stream`] 会先对
        // 一个属于死进程的号发 stopScreencast。
        *self.streaming.lock().await = None;
        self.taps.lock().await.clear();
        self.intercept.lock().await.clear();
        *self.snap_refs.lock().await = None;
        self.ping_tabs().await;
    }

    /// 页面自己要求开一页（`target="_blank"`、`window.open()`）。
    ///
    /// 浏览器进程只报告，开页在这里做 —— 标签页号由这一层分配，见
    /// [`Event::PopupRequested`]。
    async fn open_popup(&self, source: TabId, url: &str, background: bool) {
        let Some(b) = self.live().await else {
            return;
        };

        // `[约束]` 要有上限，而且数页数和开页要在同一次持锁里做完 ——
        // 见 [`Self::popping`]。开一页就是一个完整的 CEF browser（几十 MB
        // 常驻），而这一整条路是页面上的脚本触发的 —— 广告页在 onload 里
        // 连开几十个不是假想。上限之外只记一条日志:这正是浏览器拦弹窗的
        // 做法，而用户看得见标签栏，知道自己开了多少页。
        let _guard = self.popping.lock().await;
        let full = self.tabs.lock().await.order.len() >= POPUP_TAB_LIMIT;
        if full {
            tracing::warn!(source, url, "标签页已达 {POPUP_TAB_LIMIT} 上限，这一页不开");
            return;
        }

        let id = match self.spawn_tab(&b, Some(source), !background).await {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(error = %e, source, url, "页面要求开的标签页没开成");
                return;
            }
        };

        // 空地址就是 `window.open()` 没给地址，新页停在空白页上正是对的。
        if url.is_empty() {
            return;
        }
        // 走 `Command::Navigate` 而不是 `ops::navigate`:后者要等
        // `readyState` 到 complete（最多 20 秒）。这里没人在等结果 ——
        // 用户要的是"标签页立刻出现、然后自己转起来"，和真实浏览器一样。
        if let Err(e) = b.send(&Command::Navigate {
            tab: id,
            url: url.to_owned(),
        }) {
            tracing::warn!(error = %e, tab = id, url, "新标签页导航失败");
        }
    }

    /// 切到某个标签页。
    pub async fn select_tab(&self, tab: TabId) -> Result<PanelState, BrowserUnavailable> {
        let b = self.get().await?;
        {
            let mut tabs = self.tabs.lock().await;
            if !tabs.order.contains(&tab) {
                return Err(BrowserUnavailable(format!("标签页 {tab} 不存在")));
            }
            tabs.active = tab;
        }
        self.stream(&b, tab).await;
        // 模型切页时（Nav::SelectTab）前端不知情 —— ping 让标签栏的高亮
        // 立刻跟上。面板自己切页走的也是这条，重查一次无妨。
        self.ping_tabs().await;
        self.state().await
    }

    /// 标签栏 + 工具栏要显示的东西。
    ///
    /// `[约束]` 不能为了回答这个把浏览器起起来。面板每秒问一次，而这条命令
    /// 在浏览器还没起来的时候也会被问到（面板刚挂上、screencast 还在启动）
    /// —— 那时候起进程等于把「惰性启动」这个取舍作废掉。崩掉之后同理:重开
    /// 由真实动作驱动（用户点一下面板、模型调一次工具），不由这条每秒一次的
    /// 轮询驱动 —— 否则一个起不来的浏览器会变成每秒一次的 spawn 风暴。
    ///
    /// 于是崩掉之后这里回的是空清单，和"还没起来"同一个形状 —— 面板据此
    /// 显示那个「正在启动」的占位标签，而不是一排点不动的幻影标签页。
    pub async fn state(&self) -> Result<PanelState, BrowserUnavailable> {
        let Some(b) = self.live().await else {
            return Ok(PanelState::default());
        };
        let (order, active) = {
            let tabs = self.tabs.lock().await;
            (tabs.order.clone(), tabs.active)
        };

        // 一页一次往返。看着浪费，但一次是几十字节、几毫秒，而面板同时
        // 在收十几帧 JPEG —— 在那个背景噪音里量不出来。标签页也就几个。
        let mut infos = Vec::with_capacity(order.len());
        for id in order {
            let tab = Tab { browser: &b, id };
            let info = match history(tab).await {
                Ok((entries, index)) => info_at(id, &entries, index),
                // 问不到就给个只有号的占位。页面正在换文档时会这样，
                // 而让整条查询失败会连带把标签栏清空。
                Err(_) => TabInfo {
                    id,
                    ..TabInfo::default()
                },
            };
            infos.push(info);
        }
        Ok(PanelState {
            tabs: infos,
            active,
        })
    }

    /// 真的开一个标签页:发命令、等它就绪、装 console 钩子、切过去。
    ///
    /// `after` 是把新页插在谁的右边，`None` 是排到最后。`focus` 决定要不要
    /// 切过去 —— 一页都没有时它不起作用，那一页必然是当前页。
    async fn spawn_tab(
        &self,
        b: &Arc<Browser>,
        after: Option<TabId>,
        focus: bool,
    ) -> Result<TabId, BrowserUnavailable> {
        let id = {
            let mut tabs = self.tabs.lock().await;
            let id = tabs.next;
            tabs.next += 1;
            id
        };

        // 登记等待者要在发命令**之前**。反过来的话，TabOpened 可能在登记
        // 之前就到了，于是这里永远等不到 —— 表现是"新建标签页转半天然后失败"。
        let (tx, rx) = oneshot::channel();
        self.opening.lock().await.insert(id, tx);
        if let Err(e) = b.send(&Command::OpenTab { tab: id }) {
            self.opening.lock().await.remove(&id);
            return Err(BrowserUnavailable(e.to_string()));
        }
        let opened = tokio::time::timeout(TAB_OPEN_TIMEOUT, rx).await;
        self.opening.lock().await.remove(&id);
        match opened {
            Ok(Ok(())) => {}
            Ok(Err(_)) => return Err(BrowserUnavailable("浏览器进程退出了".into())),
            Err(_) => {
                return Err(BrowserUnavailable(format!(
                    "标签页 {TAB_OPEN_TIMEOUT:?} 内没有就绪"
                )));
            }
        }

        let tab = Tab { browser: b, id };

        // console 钩子是**每个页面**一份 —— 它注入的是这个页面的 document。
        // 只在进程启动时装一次的话，后开的标签页 console 工具永远返回空。
        if let Err(e) = ops::install_console_hook(tab).await {
            tracing::warn!(error = %e, tab = id, "装 console 钩子失败，console 工具会返回空");
        }

        let show = {
            // `[约束]` 入列和补视口要在同一次持锁里做完，而且锁的顺序是
            // tabs → view（[`Self::resize`] 也是这个顺序，反过来就会死锁）。
            //
            // 拆开的话中间会漏进一个 resize:它那时候看不到这一页（还没入列），
            // 而这里也还没读到它写下的尺寸 —— 两边都以为对方会管，于是首个
            // 标签页停在子进程那个 1280×800 的初值上，画面带黑边、字被缩小，
            // 一直到用户拖一下窗口才纠正。
            let mut tabs = self.tabs.lock().await;
            let at = after
                .and_then(|src| tabs.order.iter().position(|&t| t == src))
                .map_or(tabs.order.len(), |i| i + 1);
            tabs.order.insert(at, id);
            // 一页都没有时必须切过去:`active` 那时是 0，那不是个合法的号，
            // 留着它面板会以为"有一页但问不出它的地址"。
            let show = focus || tabs.order.len() == 1;
            if show {
                tabs.active = id;
            }
            if let Some((width, height, scale)) = *self.view.lock().await {
                let _ = b.send(&Command::Resize {
                    tab: id,
                    width,
                    height,
                    scale,
                });
            }
            show
        };
        // 后台页不抢画面。抢了的话 cmd 点链接会让面板跳到新页上，
        // 而"在后台开"这个动作的全部意义就是不打断当前这一页。
        if show {
            self.stream(b, id).await;
        }
        self.ping_tabs().await;
        Ok(id)
    }

    /// 把画面切到某一页。面板没开着就什么都不做。
    async fn stream(&self, b: &Arc<Browser>, tab: TabId) {
        if self.frames.lock().await.is_none() {
            // 没人看。记下该推哪页，等面板打开时再真的开 —— 那时候
            // start_screencast 会照这个值开。
            *self.streaming.lock().await = Some(tab);
            return;
        }

        let mut cur = self.streaming.lock().await;
        if let Some(old) = *cur
            && old != tab
        {
            // 停不掉也继续。旧页可能已经关了，而那时候"停"本来就没意义。
            let _ = b
                .cdp(old, "Page.stopScreencast", serde_json::json!({}))
                .await;
        }
        *cur = Some(tab);
        let params = screencast_params(self.cast_max().await);
        if let Err(e) = b.cdp(tab, "Page.startScreencast", params).await {
            tracing::warn!(error = %e, tab, "开 screencast 失败");
        }
    }

    /// 开始把画面推到 `sink`。
    ///
    /// `[取舍]` 用 CDP 的 screencast 而不是自己搬 OSR 的像素。
    ///
    /// OSR 给的是 1280×800 的 BGRA，一帧 4MB；screencast 给的是 JPEG，
    /// 同样内容一帧一两百 KB —— 小二十倍，而且编码由 Chromium 做，
    /// 我们连共享内存都不用碰。代价是有损压缩，但面板是给人看的，
    /// 模型要看完整页面时走 BrowserScreenshot（那条是整页、按 CSS 像素出图）。
    pub async fn start_screencast(
        &self,
        sink: mpsc::UnboundedSender<Frame>,
    ) -> Result<(), BrowserUnavailable> {
        let (b, tab) = self.active().await?;
        *self.frames.lock().await = Some(sink);
        *self.streaming.lock().await = Some(tab);
        b.cdp(
            tab,
            "Page.startScreencast",
            screencast_params(self.cast_max().await),
        )
        .await
        .map_err(|e| BrowserUnavailable(e.to_string()))?;
        Ok(())
    }

    /// 视口跟着面板的尺寸走。
    ///
    /// `[约束]` 视口不能钉死在子进程那个 1280×800 的初值。画面是按帧的原始
    /// 比例等比缩放后铺进面板的，比例对不上时短的那一边两侧会空出来 ——
    /// 面板越窄空得越多，1280×800 的帧塞进一块竖着的面板，上下各能留出
    /// 两百多像素的黑边。而且缩小之后页面里 16px 的字只剩九个像素高。
    ///
    /// 走 CEF 的 `was_resized` 而不是 CDP 的 `Emulation.setDeviceMetricsOverride`：
    /// 离屏渲染的画布尺寸本来就由宿主给的 `view_rect` 决定，改视口就是改
    /// 那个矩形。用 emulation 的话是让渲染进程在一块尺寸不符的画布上按另一个
    /// 尺寸排版，两套尺寸对不齐时点击坐标会整体偏。
    ///
    /// `[取舍]` 面板关掉之后不还原尺寸。还原听起来更干净 —— 没人看的时候
    /// 回到标准桌面视口，模型的截图不受用户窗口宽度影响。但关闭和重新打开
    /// 是两条独立的异步命令，快速开关时"还原"可能压在"新尺寸"后面落地，
    /// 于是面板一打开就是带黑边的，而且不会再有第二次尺寸变化来纠正它。
    /// 拿这个换一个模型基本感知不到的差别不值得:它要精确内容时走的是
    /// `captureBeyondViewport` 的整页截图，本来就不受视口高度限制。
    /// `scale` 是面板所在屏幕的像素密度（Retina 上是 2）。
    ///
    /// `[约束]` 它必须由面板给，不能在这一层猜。同一台机器上外接屏和内置屏
    /// 的密度常常不一样，而窗口在哪块屏上只有前端知道 —— 猜错的方向要么是
    /// 糊（按 1 出、按 2 铺），要么是白烧四倍的编码和带宽。
    /// `[取舍]` 尺寸发给**所有**标签页，不只当前那个。
    ///
    /// 后台页也重排一遍是白做的功（几毫秒，而且拖窗口有防抖），但省下这一步
    /// 的代价是切过去的第一眼:那一页还按旧尺寸渲染，画面带黑边、字被缩小，
    /// 要等下一次拖动窗口才纠正。
    pub async fn resize(
        &self,
        width: i32,
        height: i32,
        scale: f32,
    ) -> Result<(), BrowserUnavailable> {
        // 测试和旧调用没画面区尺寸：按视口出，行为与改之前一致。
        self.resize_view(width, height, scale, width, height).await
    }

    /// `view_w` / `view_h` 是面板画面区的 CSS 像素。screencast 按它 × 密度
    /// 封顶，页面视口仍是 `width` × `height`（Web 模式的 1280）。
    pub async fn resize_view(
        &self,
        width: i32,
        height: i32,
        scale: f32,
        view_w: i32,
        view_h: i32,
    ) -> Result<(), BrowserUnavailable> {
        let b = self.get().await?;
        let cast = cast_size(view_w, view_h, scale);
        let changed = {
            // 锁的顺序必须是 tabs → view，和 spawn_tab 一致 —— 反过来会死锁。
            // 两者都持着 tabs 做完"记下尺寸"和"发给已有的页"，正在创建的那一页
            // 才不会两头落空，见 spawn_tab 末尾的说明。
            let tabs = self.tabs.lock().await;
            *self.view.lock().await = Some((width, height, scale));
            let old = *self.cast.lock().await;
            *self.cast.lock().await = Some(cast);
            for &tab in &tabs.order {
                b.send(&Command::Resize {
                    tab,
                    width,
                    height,
                    scale,
                })
                .map_err(|e| BrowserUnavailable(e.to_string()))?;
            }
            old != Some(cast)
        };
        let recast = changed && self.frames.lock().await.is_some();
        // 推流上限变了要重开 screencast，否则 Web 模式一直按 2560 宽出
        // JPEG。CDP 没有"改参数"，再发一次 start 会换掉当前会话。
        if recast && let Some(tab) = *self.streaming.lock().await {
            let _ = b
                .cdp(tab, "Page.startScreencast", screencast_params(cast))
                .await;
        }
        Ok(())
    }

    async fn cast_max(&self) -> (u32, u32) {
        self.cast.lock().await.unwrap_or(CAST_FALLBACK)
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
        let (b, tab) = self.active().await?;
        let calls: Vec<(&str, serde_json::Value)> = match input {
            Input::Click { x, y, button } => vec![
                (
                    "Input.dispatchMouseEvent",
                    serde_json::json!({
                        "type": "mousePressed", "x": x, "y": y,
                        "button": button, "clickCount": 1,
                    }),
                ),
                (
                    "Input.dispatchMouseEvent",
                    serde_json::json!({
                        "type": "mouseReleased", "x": x, "y": y,
                        "button": button, "clickCount": 1,
                    }),
                ),
            ],
            Input::Down {
                x,
                y,
                button,
                click_count,
            } => vec![(
                "Input.dispatchMouseEvent",
                serde_json::json!({
                    "type": "mousePressed", "x": x, "y": y,
                    "button": button, "clickCount": click_count,
                }),
            )],
            Input::Up {
                x,
                y,
                button,
                click_count,
            } => vec![(
                "Input.dispatchMouseEvent",
                serde_json::json!({
                    "type": "mouseReleased", "x": x, "y": y,
                    "button": button, "clickCount": click_count,
                }),
            )],
            Input::Move { x, y } => vec![(
                "Input.dispatchMouseEvent",
                serde_json::json!({ "type": "mouseMoved", "x": x, "y": y }),
            )],
            Input::Scroll {
                x,
                y,
                delta_x,
                delta_y,
            } => vec![(
                "Input.dispatchMouseEvent",
                serde_json::json!({
                    "type": "mouseWheel", "x": x, "y": y,
                    "deltaX": delta_x, "deltaY": delta_y,
                }),
            )],
            // insertText 走的是 ImeCommitText，所以它同时也是"确认候选"——
            // 组字进行中发这条，临时内容会被最终结果替掉，不会两份都留下。
            Input::Text { text } => vec![("Input.insertText", serde_json::json!({ "text": text }))],
            Input::Compose { text } => {
                // `[约束]` 取消组字要把选区传 -1。传 0 的话 Chromium 认为
                // 组字还在继续，页面里会留下一段带下划线、删不掉的空文本，
                // 后面所有输入都跟在它后面。
                let caret = if text.is_empty() {
                    -1
                } else {
                    // 偏移按 UTF-16 算 —— Chromium 内部的字符串就是这个编码，
                    // 按字符数算的话，组字里一旦出现代理对，光标会落错位置。
                    i32::try_from(text.encode_utf16().count()).unwrap_or(0)
                };
                vec![(
                    "Input.imeSetComposition",
                    serde_json::json!({
                        "text": text,
                        "selectionStart": caret,
                        "selectionEnd": caret,
                    }),
                )]
            }
            Input::Key { key } => {
                let code = ops::key_code(&key);
                // 回车带 text 的理由见 ops::press —— 表单提交挂在 char
                // 事件上，不带的话面板里按回车搜索框没反应。
                let mut down = serde_json::json!({
                    "type": "keyDown", "key": key, "windowsVirtualKeyCode": code,
                });
                if key == "Enter" {
                    down["text"] = serde_json::json!("\r");
                }
                vec![
                    ("Input.dispatchKeyEvent", down),
                    (
                        "Input.dispatchKeyEvent",
                        serde_json::json!({
                            "type": "keyUp", "key": key, "windowsVirtualKeyCode": code,
                        }),
                    ),
                ]
            }
        };

        for (method, params) in calls {
            // 不等响应。输入事件是连续流，逐个等往返会让打字有明显延迟，
            // 而它们的响应本来就是空的。
            b.cdp_no_wait(tab, method, params)
                .map_err(|e| BrowserUnavailable(e.to_string()))?;
        }
        Ok(())
    }

    /// 在历史里走一步。`delta` 为 -1 是后退，+1 是前进。
    ///
    /// `[约束]` CDP 没有 goBack/goForward，只有「跳到某个历史条目」，而条目
    /// 的 `id` 是 Chromium 自己发的号，和下标不是一回事。拿下标当 id 传的
    /// 话，navigateToHistoryEntry 要么报找不到，要么撞上另一个条目的号 ——
    /// 后一种会跳到一个毫不相干的页面，而且没有任何报错。
    ///
    /// `[取舍]` 不等页面加载完就返回。面板看的是实时画面，加载过程本来就
    /// 在眼前；等在这里只会让按钮按下去之后僵住一两秒。回给前端的状态按
    /// 目标条目算，所以地址栏是立刻对的。
    pub async fn go(&self, delta: i32) -> Result<TabInfo, BrowserUnavailable> {
        let (b, id) = self.active().await?;
        let tab = Tab { browser: &b, id };
        let (entries, index) = history(tab).await?;
        let target = index + i64::from(delta);

        // 越界不当错误。按钮该是灰的，走到这儿说明状态还没同步过来
        // （比如页面自己刚跳了一次），那就什么也不做。
        let Some(entry_id) = usize::try_from(target)
            .ok()
            .and_then(|i| entries.get(i))
            .and_then(|e| e.get("id"))
            .cloned()
        else {
            return Ok(info_at(id, &entries, index));
        };

        tab.cdp(
            "Page.navigateToHistoryEntry",
            serde_json::json!({ "entryId": entry_id }),
        )
        .await
        .map_err(|e| BrowserUnavailable(e.to_string()))?;
        Ok(info_at(id, &entries, target))
    }

    /// 重新加载当前页面。走缓存 —— 用户按的是刷新，不是「清缓存重来」。
    pub async fn reload(&self) -> Result<(), BrowserUnavailable> {
        let (b, tab) = self.active().await?;
        b.cdp(tab, "Page.reload", serde_json::json!({}))
            .await
            .map(|_| ())
            .map_err(|e| BrowserUnavailable(e.to_string()))
    }

    /// 停止推送。面板关掉时调 —— 没人看的时候继续编码 JPEG 是白烧 CPU。
    pub async fn stop_screencast(&self) {
        *self.frames.lock().await = None;
        let Some(b) = self.live().await else {
            return;
        };
        // 停的是"正在推的那一页"，不是"当前活动页"。切标签失败或者正好
        // 卡在中间时这两者会不一样，按后者停等于让前者永远推下去。
        let Some(tab) = self.streaming.lock().await.take() else {
            return;
        };
        let _ = b
            .cdp(tab, "Page.stopScreencast", serde_json::json!({}))
            .await;
    }

    /// 活着的浏览器，`None` = 没起来，或者起过但已经不在了。
    ///
    /// 那些"不为它起进程"的路径都走这里 —— 信息性查询（[`Self::state`]、
    /// `current_url`）和事件驱动的清理（[`Self::forget_tab`]）。直接读
    /// `inner` 的话，进程崩掉之后拿到的是个死句柄:面板会照旧显示一排
    /// 幻影标签页，而点它们的每条命令都静默失败。
    async fn live(&self) -> Option<Arc<Browser>> {
        self.inner.lock().await.clone().filter(|b| b.alive())
    }

    /// 拿到浏览器，没起来就起，崩了就重开。
    ///
    /// `[约束]` 整个过程持锁。并发的两次工具调用都发现"还没起"的话，会
    /// 各起一个进程 —— 而它们指向同一个 profile 目录，第二个拿不到锁直接
    /// 退出，表现为"偶尔有个工具报浏览器不可用"。
    ///
    /// `[约束]` 崩掉的句柄必须换掉，不能只是照旧交出去。CEF 会崩（渲染
    /// 一个恶意页面、显存耗尽、被系统的内存压力杀掉），而这个槽位一旦填上
    /// 就没有别的地方会清它 —— 交出死句柄的结果是这个会话的浏览器永久
    /// 不可用（面板、模型的每个 Browser* 工具全部报"浏览器进程未运行"），
    /// 而用户唯一的出路是新建会话或者重启应用。
    ///
    /// `[取舍]` 重开是惰性的:等下一次真的用到才做，而不是收到"进程没了"
    /// 就立刻拉起来。崩溃常常发生在没人看的时候（面板关着、模型早就转去
    /// 改代码了），那时候拉起六个进程几百 MB 纯属白付 —— 和这一层
    /// 「第一次用到才起」是同一条取舍。也因此这里不需要退避:重开由真实
    /// 调用驱动，起来就崩的循环最多跟着调用频率转，不会自己打满 CPU。
    async fn get(&self) -> Result<Arc<Browser>, BrowserUnavailable> {
        let mut slot = self.inner.lock().await;
        if let Some(b) = slot.as_ref() {
            if b.alive() {
                return Ok(Arc::clone(b));
            }
            // 丢掉死句柄，然后照常往下走 —— 下面那段起进程的代码不必知道
            // 这是首次启动还是崩溃之后的重开。
            tracing::warn!("浏览器进程已经不在了，重开一个");
            *slot = None;
            self.forget_crashed().await;
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
        let streaming = Arc::clone(&self.streaming);
        let opening = Arc::clone(&self.opening);
        let taps = Arc::clone(&self.taps);
        let intercept = Arc::clone(&self.intercept);
        let host = self.me.clone();
        tokio::spawn(async move {
            let mut ready = Some(ready_tx);
            while let Some(ev) = rx.recv().await {
                match ev {
                    Event::Ready => {
                        if let Some(t) = ready.take() {
                            let _ = t.send(());
                        }
                    }
                    Event::TabOpened { tab } => {
                        // 没人等也正常:超时之后那个等待者已经撤了。
                        if let Some(w) = opening.lock().await.remove(&tab) {
                            let _ = w.send(());
                        }
                    }
                    // 下面两条都另起一个任务做，不在这个循环里 await。
                    //
                    // `[约束]` 开一页要等 `TabOpened`，而那条事件正是这个循环
                    // 派发的 —— 在循环里等它就是等自己，只能等到 10 秒超时，
                    // 表现是"页面要求开的标签页永远开不出来"。清理那条不等
                    // 事件，但它会发 CDP 命令，慢起来会挡住后面的帧。
                    //
                    // 弱引用升不上来 = 会话已经结束，这些事也就没意义了。

                    // 用户点关闭时这一层已经清过一遍（[`Self::forget_tab`]
                    // 是幂等的），这一条真正要接的是"我们没让它关、但它关了"。
                    Event::TabClosed { tab } => {
                        if let Some(h) = host.upgrade() {
                            tokio::spawn(async move { h.forget_tab(tab).await });
                        }
                    }
                    // 子进程已经把 CEF 的弹窗拦下来了，这里把它变成一个真的
                    // 标签页 —— 见 [`Event::PopupRequested`]。
                    Event::PopupRequested {
                        source,
                        url,
                        background,
                    } => {
                        if let Some(h) = host.upgrade() {
                            tokio::spawn(async move {
                                h.open_popup(source, &url, background).await;
                            });
                        }
                    }
                    Event::Error { message } => {
                        tracing::warn!(message, "浏览器报错");
                    }
                    Event::Cdp { tab, payload } => {
                        handle_cdp_event(
                            &acker, &frames, &streaming, &taps, &intercept, tab, &payload,
                        )
                        .await;
                    }
                    // OSR 的帧元数据现在没人用 —— 画面走 screencast。
                    // 留着不删是因为它是"渲染还活着"的独立信号，
                    // screencast 卡住时能用来分清是编码还是渲染的问题。
                    Event::Frame { .. } | Event::LoadEnd { .. } | Event::LoadError { .. } => {}
                }
            }
        });

        // 等 CEF 就绪。没等到就发命令的话，命令会落在一个还没有消息循环的
        // 进程上，全部静默丢掉。
        //
        // 这一刻还没有任何标签页 —— 开页由 active() / open_tab() 负责，
        // console 钩子也是每页一份，跟着开页一起装。
        tokio::time::timeout(std::time::Duration::from_secs(30), ready_rx)
            .await
            .map_err(|_| BrowserUnavailable("浏览器 30 秒内没有就绪".into()))?
            .map_err(|_| BrowserUnavailable("浏览器启动过程中退出了".into()))?;

        *slot = Some(Arc::clone(&browser));
        Ok(browser)
    }
}

/// 模型的浏览器工具。
///
/// `[约束]` 一律作用在**当前活动的标签页**上，也就是用户此刻看着的那一页。
///
/// 这条是整个面板存在的前提:"你和模型看同一个东西"。给模型一个专属的隐藏
/// 标签页听起来更安全（用户切页不会打断模型），但那样两边就各看一个页面 ——
/// 用户说"把这个按钮改红"，模型截图截到的是另一页，而它没有任何办法察觉。
///
/// 代价是用户在模型干活的中途切标签，模型的下一步会落到新页上。真实使用里
/// 那正是用户想要的（"看这个"），而误切的后果也看得见 —— 画面就在眼前。
#[async_trait]
impl BrowserAccess for HostBrowser {
    async fn navigate(&self, url: &str) -> Result<(), BrowserUnavailable> {
        let (b, id) = self.active().await?;
        ops::navigate(Tab { browser: &b, id }, url)
            .await
            .map_err(|e| BrowserUnavailable(e.to_string()))
    }

    async fn screenshot(&self) -> Result<String, BrowserUnavailable> {
        let (b, id) = self.active().await?;
        ops::screenshot(Tab { browser: &b, id })
            .await
            .map_err(|e| BrowserUnavailable(e.to_string()))
    }

    async fn snapshot(&self) -> Result<String, BrowserUnavailable> {
        let (b, id) = self.active().await?;
        let (text, refs) = ops::snapshot(Tab { browser: &b, id })
            .await
            .map_err(|e| BrowserUnavailable(e.to_string()))?;
        // 编号跟着最新一次快照走。旧的整份换掉而不是合并 —— 两份快照的
        // 同一个号指向不同元素，合并会让"[3] 到底是谁"没有答案。
        *self.snap_refs.lock().await = Some((id, refs));
        Ok(text)
    }

    async fn snapshot_marked(&self) -> Result<MarkedView, BrowserUnavailable> {
        let (b, id) = self.active().await?;
        let tab = Tab { browser: &b, id };
        let (listing, refs) = ops::snapshot(tab)
            .await
            .map_err(|e| BrowserUnavailable(e.to_string()))?;

        // 视口尺寸（CSS 像素）。没 resize 过就不做视口过滤 —— 宁可多画几个框，
        // 也不因为不知道视口大小而一个都不画。
        let (vw, vh) = (*self.view.lock().await).map_or((f64::MAX, f64::MAX), |(w, h, _)| {
            (f64::from(w), f64::from(h))
        });

        // 只框可交互、有几何、在视口内的；按编号排序，让框的出现顺序和清单一致。
        let mut marks: Vec<(u32, ops::Rect)> = refs
            .iter()
            .filter(|(_, r)| ops::is_markable(&r.label))
            .filter_map(|(n, r)| r.rect.map(|rc| (*n, rc)))
            .filter(|(_, rc)| rc.intersects_viewport(vw, vh))
            .collect();
        marks.sort_by_key(|(n, _)| *n);

        // 编号跟最新快照走（和 snapshot 一致）——交互方法用的就是这套号。
        *self.snap_refs.lock().await = Some((id, refs));

        let screenshot = ops::screenshot_marked(tab, &marks)
            .await
            .map_err(|e| BrowserUnavailable(e.to_string()))?;
        Ok(MarkedView {
            listing,
            screenshot,
        })
    }

    async fn console(&self) -> Result<Vec<String>, BrowserUnavailable> {
        let (b, id) = self.active().await?;
        ops::console(Tab { browser: &b, id })
            .await
            .map_err(|e| BrowserUnavailable(e.to_string()))
    }

    async fn current_url(&self) -> String {
        // 没起来（或者崩了）就不要为了这个把它起起来 —— 这是个信息性查询。
        let Some(b) = self.live().await else {
            return String::new();
        };
        let id = self.tabs.lock().await.active;
        ops::url_of(Tab { browser: &b, id }).await
    }

    async fn click(&self, target: Target) -> Result<String, InteractError> {
        let (b, id, oid, label) = self.resolve(target).await?;
        let tab = Tab { browser: &b, id };

        let before = ops::url_of(tab).await;
        let (x, y) = ops::locate(tab, &oid).await.map_err(stale_target)?;
        ops::click_at(tab, x, y).await.map_err(stale_target)?;
        ops::settle(tab).await;

        Ok(after_action(
            format!("已点击 {label}"),
            &before,
            &ops::url_of(tab).await,
        ))
    }

    async fn type_text(
        &self,
        target: Target,
        text: &str,
        submit: bool,
    ) -> Result<String, InteractError> {
        let (b, id, oid, label) = self.resolve(target).await?;
        let tab = Tab { browser: &b, id };

        // 点一下拿焦点。比 DOM.focus 多一步，但和真人操作一致 ——
        // 不少输入框在 click 上才初始化（日期选择器、代码编辑器）。
        let (x, y) = ops::locate(tab, &oid).await.map_err(stale_target)?;
        ops::click_at(tab, x, y).await.map_err(stale_target)?;

        if !ops::focused_editable(tab).await.map_err(stale_target)? {
            return Err(InteractError::Target(format!(
                "{label} 不是文本输入框（点击后焦点不在可编辑元素上）。\
                 只是想点它的话用 BrowserClick。"
            )));
        }

        // 全选 + 插入 = 替换原值。insertText 走 IME 提交路径，会把选区
        // 整个换掉；框本来是空的时全选就是空操作。
        ops::select_all(tab).await.map_err(stale_target)?;
        ops::insert_text(tab, text).await.map_err(stale_target)?;

        let mut msg = format!("已在 {label} 输入 {text:?}");
        if submit {
            let before = ops::url_of(tab).await;
            ops::press(tab, "Enter").await.map_err(stale_target)?;
            ops::settle(tab).await;
            msg = after_action(
                format!("{msg} 并按了回车"),
                &before,
                &ops::url_of(tab).await,
            );
        }
        Ok(msg)
    }

    async fn press_key(&self, key: &str) -> Result<String, InteractError> {
        // 键名在这儿把关，而不是发一个没键码的事件出去 —— 那种事件页面
        // 收得到但什么都不做，模型会以为"按了没用"然后换别的乱试。
        if ops::key_code(key) == 0 {
            return Err(InteractError::Target(format!(
                "不认识 {key} 这个键。支持:Enter、Tab、Escape、Backspace、Delete、\
                 ArrowUp/Down/Left/Right、Home、End、PageUp、PageDown。\
                 要输入文字用 BrowserType。"
            )));
        }
        let (b, id) = self.active().await.map_err(InteractError::Unavailable)?;
        let tab = Tab { browser: &b, id };

        let before = ops::url_of(tab).await;
        ops::press(tab, key).await.map_err(stale_target)?;
        ops::settle(tab).await;
        Ok(after_action(
            format!("已按 {key}"),
            &before,
            &ops::url_of(tab).await,
        ))
    }

    async fn scroll(&self, delta_y: f64) -> Result<String, InteractError> {
        let (b, id) = self.active().await.map_err(InteractError::Unavailable)?;
        let tab = Tab { browser: &b, id };

        let (pos, max) = ops::scroll_by(tab, delta_y).await.map_err(stale_target)?;
        Ok(if max <= 0.0 {
            "页面没有可滚动的空间。".to_owned()
        } else if pos >= max - 1.0 {
            format!("已滚动到页面底部（{pos:.0}px）。")
        } else if pos <= 0.0 {
            "已滚动到页面顶部。".to_owned()
        } else {
            format!("已滚动到 {pos:.0}px / 共 {max:.0}px。")
        })
    }

    async fn wait_for(
        &self,
        cond: WaitCondition,
        timeout_ms: u64,
    ) -> Result<String, InteractError> {
        let (b, id) = self.active().await.map_err(InteractError::Unavailable)?;
        let tab = Tab { browser: &b, id };

        // 网络空闲要数在途请求，走累积器，和"轮询一个 JS 谓词"是两条路。
        if let WaitCondition::NetworkIdle = cond {
            return self.wait_network_idle(tab, timeout_ms).await;
        }

        let (expr, desc) = match &cond {
            WaitCondition::Selector(s) => (
                format!("!!document.querySelector({})", js_str(s)),
                format!("元素 `{s}` 出现"),
            ),
            WaitCondition::SelectorGone(s) => (
                format!("!document.querySelector({})", js_str(s)),
                format!("元素 `{s}` 消失"),
            ),
            WaitCondition::Text(t) => (
                format!(
                    "!!(document.body && document.body.innerText.includes({}))",
                    js_str(t)
                ),
                format!("文本 “{t}” 出现"),
            ),
            WaitCondition::UrlContains(u) => (
                format!("location.href.includes({})", js_str(u)),
                format!("地址包含 “{u}”"),
            ),
            WaitCondition::NetworkIdle => unreachable!("上面提前处理了"),
        };
        let ok = ops::wait_predicate(tab, &expr, timeout_ms)
            .await
            .map_err(stale_target)?;
        if ok {
            Ok(format!("等到了:{desc}。"))
        } else {
            Err(InteractError::Target(format!(
                "等了 {timeout_ms}ms，{desc} 仍未发生。\
                 页面可能没走到那个状态，或者选择器/文本不对 —— \
                 用 BrowserSnapshot 看看当前页面。"
            )))
        }
    }

    async fn act(&self, action: Action) -> Result<String, InteractError> {
        match action {
            Action::Hover(t) => {
                let (b, id, oid, label) = self.resolve(t).await?;
                let tab = Tab { browser: &b, id };
                let (x, y) = ops::locate(tab, &oid).await.map_err(stale_target)?;
                ops::hover_at(tab, x, y).await.map_err(stale_target)?;
                Ok(format!("已悬停在 {label}。"))
            }
            Action::DoubleClick(t) => {
                let (b, id, oid, label) = self.resolve(t).await?;
                let tab = Tab { browser: &b, id };
                let before = ops::url_of(tab).await;
                let (x, y) = ops::locate(tab, &oid).await.map_err(stale_target)?;
                ops::double_click_at(tab, x, y)
                    .await
                    .map_err(stale_target)?;
                ops::settle(tab).await;
                Ok(after_action(
                    format!("已双击 {label}"),
                    &before,
                    &ops::url_of(tab).await,
                ))
            }
            Action::RightClick(t) => {
                let (b, id, oid, label) = self.resolve(t).await?;
                let tab = Tab { browser: &b, id };
                let (x, y) = ops::locate(tab, &oid).await.map_err(stale_target)?;
                ops::right_click_at(tab, x, y).await.map_err(stale_target)?;
                Ok(format!("已右键 {label}。"))
            }
            Action::SelectOption { target, value } => {
                let (b, id, oid, label) = self.resolve(target).await?;
                let tab = Tab { browser: &b, id };
                ops::set_value(tab, &oid, &value)
                    .await
                    .map_err(stale_target)?;
                Ok(format!("已把 {label} 设为 {value:?}。"))
            }
            Action::Drag { from, to } => {
                // 两端分别解析。第二次 resolve 不会让第一次的 objectId 失效
                // （只是查询），但两端必须在同一标签页 —— 都走 active()，天然一致。
                let (b, id, oid_from, l_from) = self.resolve(from).await?;
                let (_, _, oid_to, l_to) = self.resolve(to).await?;
                let tab = Tab { browser: &b, id };
                let p1 = ops::locate(tab, &oid_from).await.map_err(stale_target)?;
                let p2 = ops::locate(tab, &oid_to).await.map_err(stale_target)?;
                ops::drag_between(tab, p1, p2).await.map_err(stale_target)?;
                ops::settle(tab).await;
                Ok(format!("已把 {l_from} 拖到 {l_to}。"))
            }
            Action::KeyChord(chord) => {
                let (b, id) = self.active().await.map_err(InteractError::Unavailable)?;
                let tab = Tab { browser: &b, id };
                let before = ops::url_of(tab).await;
                ops::key_chord(tab, &chord).await.map_err(stale_target)?;
                ops::settle(tab).await;
                Ok(after_action(
                    format!("已按 {chord}"),
                    &before,
                    &ops::url_of(tab).await,
                ))
            }
        }
    }

    async fn browse(&self, nav: Nav) -> Result<String, InteractError> {
        match nav {
            Nav::Back => {
                let info = self.go(-1).await.map_err(InteractError::Unavailable)?;
                Ok(format!("已后退到 {}", tab_line(&info)))
            }
            Nav::Forward => {
                let info = self.go(1).await.map_err(InteractError::Unavailable)?;
                Ok(format!("已前进到 {}", tab_line(&info)))
            }
            Nav::Reload => {
                self.reload().await.map_err(InteractError::Unavailable)?;
                Ok("已重新加载当前页。".to_owned())
            }
            Nav::ListTabs => {
                let st = self.state().await.map_err(InteractError::Unavailable)?;
                if st.tabs.is_empty() {
                    return Ok("当前没有打开的标签页。".to_owned());
                }
                let mut out = String::from("标签页:\n");
                for t in &st.tabs {
                    let active = if t.id == st.active {
                        "（活动）"
                    } else {
                        ""
                    };
                    out.push_str(&format!("[{}] {}{active}\n", t.id, tab_line(t)));
                }
                Ok(out)
            }
            Nav::NewTab => {
                self.open_tab().await.map_err(InteractError::Unavailable)?;
                Ok("已新开一个空白标签页。用 BrowserNavigate 打开地址。".to_owned())
            }
            // 标签号给错是模型的问题，不是浏览器坏了 —— 归 Target，让它重新
            // ListTabs 拿正确的号，而不是当"浏览器不可用"去改用 WebFetch。
            Nav::SelectTab(id) => {
                self.select_tab(id)
                    .await
                    .map_err(|e| InteractError::Target(e.to_string()))?;
                Ok(format!("已切到标签页 [{id}]。"))
            }
            Nav::CloseTab(id) => {
                self.close_tab(id)
                    .await
                    .map_err(|e| InteractError::Target(e.to_string()))?;
                Ok(format!("已关闭标签页 [{id}]。"))
            }
        }
    }

    async fn evaluate(&self, expr: &str) -> Result<String, InteractError> {
        let (b, id) = self.active().await.map_err(InteractError::Unavailable)?;
        ops::evaluate(Tab { browser: &b, id }, expr)
            .await
            .map_err(eval_err)
    }

    async fn upload(&self, target: Target, paths: Vec<String>) -> Result<String, InteractError> {
        let (b, id, oid, label) = self.resolve(target).await?;
        let tab = Tab { browser: &b, id };
        let count = paths.len();
        // setFileInputFiles 直接吃 objectId。文件不存在 / 不是 file input 时
        // CDP 会报错，eval_err 把它整形给模型（不套"重新快照"那句）。
        tab.cdp(
            "DOM.setFileInputFiles",
            serde_json::json!({ "objectId": oid, "files": paths }),
        )
        .await
        .map_err(eval_err)?;
        Ok(format!("已给 {label} 设置 {count} 个待上传文件。"))
    }

    async fn cookies(&self) -> Result<String, InteractError> {
        let (b, id) = self.active().await.map_err(InteractError::Unavailable)?;
        ops::cookies(Tab { browser: &b, id })
            .await
            .map_err(eval_err)
    }

    async fn network(&self, query: NetQuery) -> Result<String, InteractError> {
        // 订阅 Network（幂等）。第一次调用只是"开始累积"，所以列表这次多半
        // 是空的 —— 提示语里会说清"再刷新一次"。
        self.tap_enable("Network")
            .await
            .map_err(InteractError::Unavailable)?;
        let events = self.tap_read("Network").await;
        match query {
            NetQuery::List { filter } => Ok(super::netlog::list(&events, filter.as_deref())),
            NetQuery::Detail { request_id } => {
                let Some(headers) = super::netlog::detail_headers(&events, &request_id) else {
                    return Err(InteractError::Target(format!(
                        "没抓到 #{request_id} 这条请求。先用 list 看有哪些，或刷新页面再抓。"
                    )));
                };
                // 响应体要现取:getResponseBody 只在响应还留着时有效。
                let (b, id) = self.active().await.map_err(InteractError::Unavailable)?;
                let body = self
                    .response_body(Tab { browser: &b, id }, &request_id)
                    .await;
                Ok(match body {
                    Some(text) => format!("{headers}\n\n响应体:\n{text}"),
                    None => format!("{headers}\n\n（响应体取不到:可能已释放，或是二进制/太大。）"),
                })
            }
            NetQuery::Audit => {
                let (b, id) = self.active().await.map_err(InteractError::Unavailable)?;
                let url = ops::url_of(Tab { browser: &b, id }).await;
                Ok(super::netlog::audit(&events, &url))
            }
        }
    }

    async fn replay(
        &self,
        url: &str,
        method: &str,
        headers: serde_json::Value,
        body: Option<String>,
    ) -> Result<String, InteractError> {
        let (b, id) = self.active().await.map_err(InteractError::Unavailable)?;
        ops::replay(
            Tab { browser: &b, id },
            url,
            method,
            &headers,
            body.as_deref(),
        )
        .await
        .map_err(eval_err)
    }

    async fn intercept(&self, op: InterceptOp) -> Result<String, InteractError> {
        let (b, id) = self.active().await.map_err(InteractError::Unavailable)?;
        let tab = Tab { browser: &b, id };
        match op {
            InterceptOp::List => {
                let guard = self.intercept.lock().await;
                let rules = guard.get(&id).map(Vec::as_slice).unwrap_or_default();
                if rules.is_empty() {
                    return Ok("当前没有拦截规则。".to_owned());
                }
                let mut out = String::from("拦截规则:\n");
                for r in rules {
                    let what = match &r.action {
                        InterceptAction::Block => "阻断".to_owned(),
                        InterceptAction::Fulfill { status, .. } => format!("伪造响应 {status}"),
                    };
                    out.push_str(&format!("- URL 含 `{}` → {what}\n", r.needle));
                }
                Ok(out)
            }
            InterceptOp::Clear => {
                let had = { self.intercept.lock().await.remove(&id).is_some() };
                if had {
                    // 关掉 Fetch，被暂停的请求随之恢复。关不掉也不算错 ——
                    // 页面可能已经走了。
                    let _ = tab.cdp("Fetch.disable", serde_json::json!({})).await;
                }
                Ok("已清空拦截规则。".to_owned())
            }
            InterceptOp::Block { url_pattern } => {
                self.add_rule(
                    id,
                    tab,
                    InterceptRule {
                        needle: url_pattern.clone(),
                        action: InterceptAction::Block,
                    },
                )
                .await?;
                Ok(format!("已加规则:阻断 URL 含 `{url_pattern}` 的请求。"))
            }
            InterceptOp::Fulfill {
                url_pattern,
                status,
                body,
            } => {
                self.add_rule(
                    id,
                    tab,
                    InterceptRule {
                        needle: url_pattern.clone(),
                        action: InterceptAction::Fulfill { status, body },
                    },
                )
                .await?;
                Ok(format!(
                    "已加规则:对 URL 含 `{url_pattern}` 的请求伪造 {status} 响应。"
                ))
            }
        }
    }
}

impl HostBrowser {
    /// 把一个 [`Target`] 解析成当前页面上那个元素:返回进程句柄、标签页号、
    /// 元素的 objectId、给模型看的标签。三种定位方式在这里收敛到同一种
    /// objectId（见 [`ops::locate`] 的说明）。
    async fn resolve(
        &self,
        target: Target,
    ) -> Result<(Arc<Browser>, TabId, String, String), InteractError> {
        let (b, id) = self.active().await.map_err(InteractError::Unavailable)?;
        let tab = Tab { browser: &b, id };
        let (object_id, label) = match target {
            Target::Ref(n) => {
                let r = self.ref_of(id, n).await?;
                let oid = ops::resolve_backend(tab, r.backend_id)
                    .await
                    .map_err(stale_target)?;
                (oid, r.label)
            }
            Target::Selector(ref s) => {
                let oid = ops::resolve_selector(tab, s)
                    .await
                    .map_err(stale_target)?
                    .ok_or_else(|| {
                        InteractError::Target(format!(
                            "选择器 `{s}` 在当前页面没匹配到任何元素。\
                             用 BrowserSnapshot 看看页面上有什么。"
                        ))
                    })?;
                (oid, target.describe())
            }
            Target::Text(ref t) => {
                let oid = ops::resolve_text(tab, t)
                    .await
                    .map_err(stale_target)?
                    .ok_or_else(|| {
                        InteractError::Target(format!(
                            "页面上找不到包含文本 “{t}” 的可点击元素。\
                             换个更短的关键词，或用 BrowserSnapshot 看结构。"
                        ))
                    })?;
                (oid, target.describe())
            }
        };
        Ok((Arc::clone(&b), id, object_id, label))
    }

    /// 校验快照编号并取回它对应的元素。三种查无此号的情况各说各的话 ——
    /// "没拍过快照"、"快照是别的标签页的"、"号不在这份快照里"，模型要做的
    /// 下一步都一样（重新快照），但知道差在哪能少走一步弯路。
    async fn ref_of(&self, id: TabId, n: u32) -> Result<ops::SnapRef, InteractError> {
        let guard = self.snap_refs.lock().await;
        let Some((snap_tab, refs)) = guard.as_ref() else {
            return Err(InteractError::Target(
                "还没有拍过页面快照，元素编号无从谈起。\
                 先用 BrowserSnapshot 看页面、拿到 [n] 编号。"
                    .into(),
            ));
        };
        if *snap_tab != id {
            return Err(InteractError::Target(
                "上次快照是在另一个标签页拍的，编号对不上当前页面。\
                 用 BrowserSnapshot 在当前页重新拿编号。"
                    .into(),
            ));
        }
        refs.get(&n).cloned().ok_or_else(|| {
            InteractError::Target(format!(
                "编号 [{n}] 不在最近一次快照里（有效编号是 1 到 {}）。\
                 页面可能变了，用 BrowserSnapshot 重新拿编号。",
                refs.len()
            ))
        })
    }

    /// 等网络空闲:一小段时间内没有在途请求。SPA 点一下常常紧跟一串
    /// 数据请求，`document.readyState` 早就 complete 了，真正的内容还在飞。
    async fn wait_network_idle(
        &self,
        _tab: Tab<'_>,
        timeout_ms: u64,
    ) -> Result<String, InteractError> {
        self.tap_enable("Network")
            .await
            .map_err(InteractError::Unavailable)?;

        // 空闲要"持续"一小拍才算数。刚发完一个请求、下一个还没起的瞬间
        // 在途数也可能短暂归零，不设静默窗的话会误判成空闲。
        let quiet_window = Duration::from_millis(500);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        let mut quiet_since: Option<tokio::time::Instant> = None;
        loop {
            let inflight = self.network_inflight().await;
            if inflight == 0 {
                let since = *quiet_since.get_or_insert_with(tokio::time::Instant::now);
                if since.elapsed() >= quiet_window {
                    return Ok("网络已空闲。".to_owned());
                }
            } else {
                quiet_since = None;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(InteractError::Target(format!(
                    "等了 {timeout_ms}ms 网络仍未空闲（还有 {inflight} 个在途请求）。"
                )));
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    /// 当前累积到的在途请求数 = 已发起 − 已结束（完成或失败）。
    async fn network_inflight(&self) -> usize {
        let events = self.tap_read("Network").await;
        let mut started = std::collections::HashSet::new();
        let mut done = std::collections::HashSet::new();
        for e in &events {
            let rid = e["params"]["requestId"].as_str().unwrap_or_default();
            match e["method"].as_str().unwrap_or_default() {
                "Network.requestWillBeSent" => {
                    started.insert(rid.to_owned());
                }
                "Network.loadingFinished" | "Network.loadingFailed" => {
                    done.insert(rid.to_owned());
                }
                _ => {}
            }
        }
        started.difference(&done).count()
    }

    // ── CDP 事件累积器的内部 API ──────────────────────
    //
    // 抓包（Network）、对话框（Page）、日志（Log）这些"要留住过去一段时间
    // 发生了什么"的能力都走这里。阶段 0 先把地基铺好，工具在阶段 2 起接上。
    //
    // `[约束]` 订阅、读、清都作用在**当前活动标签页**上，和别的浏览器工具
    // 一致（见 BrowserAccess 的实现说明）—— 事件累积的语义必须和用户看着的
    // 那一页对齐，否则模型抓到的是另一页的流量。

    /// 订阅某 domain 的 CDP 事件并开始累积。幂等。
    ///
    /// 第一次订阅时发一条无参 `<Domain>.enable`（Network / Log / Runtime /
    /// Page 都是这个形状）。`Fetch.enable` 带 patterns 参数、语义也不同
    /// （要逐条放行），不走这里，由拦截那条路自己发。
    async fn tap_enable(&self, domain: &str) -> Result<(), BrowserUnavailable> {
        let (b, id) = self.active().await?;
        let fresh = {
            let mut taps = self.taps.lock().await;
            taps.entry(id).or_default().subscribe(domain)
        };
        if fresh {
            Tab { browser: &b, id }
                .cdp(&format!("{domain}.enable"), serde_json::json!({}))
                .await
                .map_err(|e| BrowserUnavailable(e.to_string()))?;
        }
        Ok(())
    }

    /// 读回当前标签页某 domain 累积的事件。没订阅过就是空。
    async fn tap_read(&self, domain: &str) -> Vec<serde_json::Value> {
        let Some(id) = self.current().await else {
            return Vec::new();
        };
        self.taps
            .lock()
            .await
            .get(&id)
            .map(|t| t.read(domain))
            .unwrap_or_default()
    }

    /// 加一条拦截规则;第一条规则时才开 `Fetch.enable`。
    ///
    /// `[约束]` Fetch 一开会暂停每个请求（要靠事件循环逐一放行）。所以只在
    /// 真的有规则时开 —— 没规则时保持关闭，零风险。
    async fn add_rule(
        &self,
        id: TabId,
        tab: Tab<'_>,
        rule: InterceptRule,
    ) -> Result<(), InteractError> {
        let first = {
            let mut guard = self.intercept.lock().await;
            let rules = guard.entry(id).or_default();
            let was_empty = rules.is_empty();
            rules.push(rule);
            was_empty
        };
        if first {
            // 空 patterns = 拦截所有请求。事件循环对不匹配规则的一律
            // continue，所以"拦所有再逐条放行"是安全的，也是唯一能覆盖
            // 任意 URL 的开法。
            tab.cdp(
                "Fetch.enable",
                serde_json::json!({ "patterns": [{ "urlPattern": "*" }] }),
            )
            .await
            .map_err(|e| InteractError::Unavailable(BrowserUnavailable(e.to_string())))?;
        }
        Ok(())
    }

    /// 取某条请求的响应体，截断成文本。取不到（已释放、二进制、太大）
    /// 返回 `None` —— 调用方据此给一句说明，而不是把错误抛给模型。
    async fn response_body(&self, tab: Tab<'_>, request_id: &str) -> Option<String> {
        let r = tab
            .cdp(
                "Network.getResponseBody",
                serde_json::json!({ "requestId": request_id }),
            )
            .await
            .ok()?;
        // base64 编码的多半是二进制，跳过 —— 塞给模型一堆 base64 没意义。
        if r["base64Encoded"].as_bool() == Some(true) {
            return None;
        }
        let body = r["body"].as_str()?;
        let cut: String = body.chars().take(super::netlog::MAX_BODY).collect();
        Some(if body.chars().count() > super::netlog::MAX_BODY {
            format!("{cut}…（已截断）")
        } else {
            cut
        })
    }

    /// 清空当前标签页某 domain 的累积历史，但保留订阅。
    #[allow(dead_code)] // 阶段 2 起有调用者
    async fn tap_clear(&self, domain: &str) {
        let Some(id) = self.current().await else {
            return;
        };
        if let Some(t) = self.taps.lock().await.get_mut(&id) {
            t.clear(domain);
        }
    }
}

/// 一个标签页在结果里显示成什么:标题优先，退到地址，都空就写"空白页"。
fn tab_line(info: &TabInfo) -> String {
    if !info.title.is_empty() {
        format!("{} — {}", info.title, info.url)
    } else if !info.url.is_empty() {
        info.url.clone()
    } else {
        "空白页".to_owned()
    }
}

/// 把一段文本变成能安全嵌进 JS 的字符串字面量（含引号）。
///
/// 等待条件里的选择器/文本要拼进 `document.querySelector(...)` 这种表达式，
/// 直接插会被引号和特殊字符打断。JSON 编码正好是合法的 JS 字符串字面量。
fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// 动作完成后的结果消息:页面因此跳转了就带上新地址。
///
/// 跳转是点击最重要的副作用 —— 不说的话，模型的下一步还以为自己在旧页上。
fn after_action(done: String, before: &str, after: &str) -> String {
    if after != before && !after.is_empty() {
        format!("{done}，页面跳到了 {after}")
    } else {
        format!("{done}。")
    }
}

/// evaluate / cookies 的错误整形。
///
/// 和 [`stale_target`] 分开:脚本抛异常是"模型的脚本错了"，指引它改脚本，
/// 而不是"重新快照"（那条对 evaluate 毫无意义，只会把模型带偏）。
fn eval_err(e: super::BrowserError) -> InteractError {
    match e {
        super::BrowserError::Cdp { message, .. } => InteractError::Target(message),
        other => InteractError::Unavailable(BrowserUnavailable(other.to_string())),
    }
}

/// 把交互路上的 CDP 错误整形给模型。
///
/// CDP 报错（找不到节点、没有几何）几乎都指向同一件事:快照过期了。
/// 进程级失败（没起来、超时）才是"浏览器不可用"。分错类的代价见
/// [`InteractError`] 的说明。
fn stale_target(e: super::BrowserError) -> InteractError {
    match e {
        super::BrowserError::Cdp { .. } => InteractError::Target(format!(
            "{e}\n元素可能已经不在页面上（页面变了、或被移除）。\
             用 BrowserSnapshot 重新拿编号再试。"
        )),
        other => InteractError::Unavailable(BrowserUnavailable(other.to_string())),
    }
}

/// 还没收到面板尺寸时的 screencast 上限。
const CAST_FALLBACK: (u32, u32) = (4000, 3000);

/// 面板画面区（CSS）× 密度 → JPEG 物理像素上限。
fn cast_size(view_w: i32, view_h: i32, scale: f32) -> (u32, u32) {
    let scale = if scale.is_finite() { scale.clamp(1.0, 3.0) } else { 1.0 };
    let w = ((view_w.max(1) as f32) * scale).round() as u32;
    let h = ((view_h.max(1) as f32) * scale).round() as u32;
    (w.clamp(80, CAST_FALLBACK.0), h.clamp(80, CAST_FALLBACK.1))
}

/// screencast 的参数。开页、切页、改推流上限都用这一套。
fn screencast_params((max_width, max_height): (u32, u32)) -> serde_json::Value {
    serde_json::json!({
        "format": "jpeg",
        // 60 在文字页面上已经看不出压缩痕迹，再高只是白涨体积。
        "quality": 60,
        // `[约束]` 这两个数是**物理像素**。按面板画面区给，而不是页面
        // 视口：Web 模式视口 1280、面板 400 宽时，按视口出 2560 的帧
        // 再缩小，编码本身就比滚动慢。模型截图不走这条。
        "maxWidth": max_width,
        "maxHeight": max_height,
    })
}

/// 取一次导航历史，返回（条目表，当前下标）。
///
/// 页面正在换文档时这条会失败。那不是"没有历史"，是"这会儿问不到" ——
/// 所以往上抛错误而不是回一个空状态，让面板保持上一次的显示。空状态会
/// 让地址栏和按钮在每次导航中间闪一下。
async fn history(tab: Tab<'_>) -> Result<(Vec<serde_json::Value>, i64), BrowserUnavailable> {
    let h = tab
        .cdp("Page.getNavigationHistory", serde_json::json!({}))
        .await
        .map_err(|e| BrowserUnavailable(e.to_string()))?;
    let index = h["currentIndex"].as_i64().unwrap_or(-1);
    let entries = h["entries"].as_array().cloned().unwrap_or_default();
    Ok((entries, index))
}

/// 站在历史的第 `index` 条上时，这一页在界面上是什么样。
fn info_at(id: TabId, entries: &[serde_json::Value], index: i64) -> TabInfo {
    let at = |i: i64| usize::try_from(i).ok().and_then(|i| entries.get(i));
    // 下标落在表外说明历史还没建立起来。这时候除了号什么都不给，
    // 而不是让 can_forward 去看第 0 条 —— 那会让一个空页面的前进键亮着。
    let Some(current) = at(index) else {
        return TabInfo {
            id,
            ..TabInfo::default()
        };
    };
    let url = displayable(current["url"].as_str().unwrap_or_default());
    TabInfo {
        id,
        url: url.to_owned(),
        // 空白页的标题是那串 data URL 本身，不能给用户看 —— 地址已经抹成
        // 空了，标题再漏出来是同一个问题换个地方冒出来。
        title: if url.is_empty() {
            String::new()
        } else {
            current["title"].as_str().unwrap_or_default().to_owned()
        },
        can_back: at(index - 1).is_some(),
        can_forward: at(index + 1).is_some(),
    }
}

/// 能摆进地址栏的地址。空白页返回空串。
///
/// `[约束]` 空白页的地址不能给到界面上。它是实现细节（一个 `data:` URL，
/// 见 [`BLANK_PAGE`]），显示出来的效果是"打开面板第一眼，地址栏里一串
/// 看不懂的东西"，而且看着像个能访问的地方。
///
/// 在这一层抹掉而不是让前端认:这个常量是协议里的，前端拿不到，抄一份过去
/// 就会变成改了一边忘了另一边。
fn displayable(url: &str) -> &str {
    // about:blank 也算 —— 页面自己可能跳到那儿（比如 target=_blank 的中间态）。
    if url == BLANK_PAGE || url == "about:blank" {
        return "";
    }
    url
}

/// 处理不带 id 的 CDP 事件:screencast 帧走画面出口，其余按订阅累积。
///
/// 走到这里的都是**事件**（不带 id）—— 带 id 的响应在 [`super::mod`] 的
/// `route_cdp_response` 就被认领走了。所以这里不必再分辨响应和事件。
#[allow(clippy::too_many_arguments)] // 事件循环要摸的共享状态就这么多，拆开更糊涂
async fn handle_cdp_event(
    browser: &Arc<Browser>,
    frames: &Arc<Mutex<Option<mpsc::UnboundedSender<Frame>>>>,
    streaming: &Arc<Mutex<Option<TabId>>>,
    taps: &Arc<Mutex<HashMap<TabId, super::taps::EventTaps>>>,
    intercept: &Arc<Mutex<HashMap<TabId, Vec<InterceptRule>>>>,
    tab: TabId,
    payload: &serde_json::Value,
) {
    let method = payload
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or_default();

    // `[约束]` JS 对话框必须无条件、立即放行。alert/confirm/beforeunload 会
    // **阻塞页面**直到有人应答 —— 没人应答的话，紧跟着的每一条 CDP 都超时，
    // 表现是"点了个按钮之后浏览器整个僵住"。所以这里不问模型、不看订阅，
    // 收到就 accept。prompt 给空文本（accept 但不填），beforeunload accept
    // 等于允许离开，都是自动化里想要的默认。
    if method == "Page.javascriptDialogOpening" {
        let _ = browser.cdp_no_wait(
            tab,
            "Page.handleJavaScriptDialog",
            serde_json::json!({ "accept": true }),
        );
        return;
    }

    // `[约束]` Fetch 暂停的请求必须**逐一放行**，否则页面卡死（和对话框
    // 同一个道理）。匹配到规则就拦/伪造，否则一律 continue —— 绝不把一个
    // paused 请求漏在那里。
    if method == "Fetch.requestPaused" {
        handle_request_paused(browser, intercept, tab, payload).await;
        return;
    }

    // `[约束]` screencast 帧必须在累积之前拦掉。它的 domain 也是 `Page`，
    // 订阅了 Page 的页面会把每秒几十帧的图塞进桶里 —— 内存瞬间爆掉，
    // 而且把真正要看的事件淹了。帧有自己的专门通道，不进桶。
    if method != "Page.screencastFrame" {
        // 只有订阅过 domain 的标签页才有条目;没订阅的这里查不到，直接丢，
        // 不为它建空条目（见 taps 模块"只累积订阅过的"）。
        if let Some(t) = taps.lock().await.get_mut(&tab) {
            t.ingest(payload);
        }
        return;
    }
    let params = &payload["params"];

    // `[约束]` 必须 ack，而且要无条件 ack。
    //
    // Chromium 只在上一帧被确认后才发下一帧。漏一次 ack，那一页的画面就
    // 永久停在那一帧 —— 而且不报错，看起来像页面卡住了。所以哪怕这一帧
    // 属于已经切走的标签页，也要先把 ack 发出去:那一页之后可能被切回来，
    // 而它那时候还欠着一次确认。
    if let Some(sid) = params.get("sessionId") {
        let _ = browser.cdp_no_wait(
            tab,
            "Page.screencastFrameAck",
            serde_json::json!({ "sessionId": sid }),
        );
    }

    // 不是正在显示的那一页就丢掉。切标签是"停旧的、开新的"两条命令，
    // 中间旧页还会来几帧 —— 画上去的话，新标签会先闪一下旧页面的内容。
    if *streaming.lock().await != Some(tab) {
        return;
    }

    let Some(sink) = frames.lock().await.clone() else {
        return; // 面板没开，帧丢掉
    };
    let Some(data) = params["data"].as_str() else {
        return;
    };
    // base64 在这儿解成字节，见 [`Frame`] 的说明。解不开就丢这一帧 ——
    // 坏一帧的代价是画面晚 30ms 更新，报错反而没人能处理。
    use base64::Engine as _;
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else {
        return;
    };
    let meta = &params["metadata"];
    let frame = Frame {
        data: bytes,
        width: meta["deviceWidth"].as_f64().unwrap_or_default() as u32,
        height: meta["deviceHeight"].as_f64().unwrap_or_default() as u32,
    };
    // 发失败说明面板那头没了，摘掉出口顺便停推送。
    if sink.send(frame).is_err() {
        *frames.lock().await = None;
        let _ = browser.cdp_no_wait(tab, "Page.stopScreencast", serde_json::json!({}));
    }
}

/// 处理一个被 Fetch 暂停的请求:匹配规则就拦/伪造，否则放行。
///
/// `[约束]` 每条路径最后都要给 CDP 一个确定的答复（fail / fulfill /
/// continue）。任何一条漏掉，那个请求就永远挂着，页面随之卡死。用
/// `cdp_no_wait` 是因为这些答复没有有用的返回值，而且要快。
async fn handle_request_paused(
    browser: &Arc<Browser>,
    intercept: &Arc<Mutex<HashMap<TabId, Vec<InterceptRule>>>>,
    tab: TabId,
    payload: &serde_json::Value,
) {
    let params = &payload["params"];
    let Some(request_id) = params["requestId"].as_str() else {
        return;
    };
    let url = params["request"]["url"].as_str().unwrap_or_default();

    let matched = {
        let guard = intercept.lock().await;
        guard
            .get(&tab)
            .and_then(|rules| rules.iter().find(|r| url.contains(&r.needle)).cloned())
            .map(|r| r.action)
    };

    match matched {
        Some(InterceptAction::Block) => {
            let _ = browser.cdp_no_wait(
                tab,
                "Fetch.failRequest",
                serde_json::json!({ "requestId": request_id, "errorReason": "BlockedByClient" }),
            );
        }
        Some(InterceptAction::Fulfill { status, body }) => {
            use base64::Engine as _;
            let b64 = base64::engine::general_purpose::STANDARD.encode(body.as_bytes());
            let _ = browser.cdp_no_wait(
                tab,
                "Fetch.fulfillRequest",
                serde_json::json!({
                    "requestId": request_id,
                    "responseCode": status,
                    "body": b64,
                }),
            );
        }
        // 不匹配任何规则:原样放行。绝不把它漏在 paused。
        None => {
            let _ = browser.cdp_no_wait(
                tab,
                "Fetch.continueRequest",
                serde_json::json!({ "requestId": request_id }),
            );
        }
    }
}

fn is_browser_bundle(path: &std::path::Path) -> bool {
    // 判据 = 里面的可执行文件存在，与 Browser::spawn 同源（executable_in）。
    // 只查目录存在的话，CI 为了过 tauri-build 资源检查造的空占位目录
    // 会被当成"有浏览器"，会话装上一个永远起不来的 HostBrowser。
    path.is_dir() && super::executable_in(path).is_file()
}

/// 打包好的浏览器在哪儿。
///
/// 开发时在 crate 的 target 下（`scripts/build-browser.sh` /
/// `scripts/build-browser.ps1` 的产物），发版后跟着主程序走。找不到时
/// 返回 `None` —— 调用方据此装 `NoBrowser`，工具会明确说用不了。
pub fn locate_app() -> Option<PathBuf> {
    // 发版布局:macOS 在 Riot.app/Contents/Resources/riot-browser.app;
    // Windows 没有 bundle，整个目录铺在主 exe 旁边的 riot-browser\ 里。
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        #[cfg(windows)]
        let bundled = dir.join("riot-browser");
        #[cfg(not(windows))]
        let bundled = dir.join("../Resources/riot-browser.app");
        if is_browser_bundle(&bundled) {
            return bundled.canonicalize().ok();
        }
    }
    // 开发布局（打包脚本的默认输出位置）
    #[cfg(windows)]
    const DEV_BUNDLE: &str = "../crates/riot-browser/target/bundle/riot-browser";
    #[cfg(not(windows))]
    const DEV_BUNDLE: &str = "../crates/riot-browser/target/bundle/riot-browser.app";
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DEV_BUNDLE);
    is_browser_bundle(&dev).then(|| dev.canonicalize().unwrap_or(dev))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// 面板发的每一种输入都要解析得了。
    ///
    /// 这几条 JSON 是照着 `src/bridge/index.ts` 里的 `BrowserInput` 抄的，
    /// 改字段名的话两边要一起改。盯着的是同一类失败：类型对不上时**编译不会
    /// 报错**，运行时也只是命令悄悄 reject —— 上一次的现象是滚轮完全不动，
    /// 而点击和打字都正常，看起来像 CDP 不支持滚动。
    #[test]
    fn 面板输入按前端的字段名解析() {
        let cases = json!([
            { "kind": "click", "x": 10.0, "y": 20.0, "button": "left" },
            { "kind": "move", "x": 10.0, "y": 20.0 },
            { "kind": "scroll", "x": 10.0, "y": 20.0, "deltaX": 0.0, "deltaY": -120.0 },
            { "kind": "text", "text": "你好" },
            { "kind": "compose", "text": "ni" },
            { "kind": "compose", "text": "" },
            { "kind": "key", "key": "Enter" },
        ]);
        for case in cases.as_array().expect("是数组") {
            serde_json::from_value::<Input>(case.clone())
                .unwrap_or_else(|e| panic!("{case} 解析失败：{e}"));
        }
    }

    #[test]
    fn 两个轴的滚动距离都原样传下去() {
        // 方向也要对。取反的话页面会往相反方向滚，而那种错很容易被
        // 当成"触控板设置反了"放过去。
        //
        // 横轴是后补的。补之前这一层收都不收 deltaX，宿主往 CDP 里写死
        // 一个 0 —— 现象是竖着能滚、横着纹丝不动，看起来像页面本身
        // 没有横向滚动条。
        let input = serde_json::from_value::<Input>(json!({
            "kind": "scroll", "x": 1.0, "y": 2.0, "deltaX": 40.0, "deltaY": -120.0,
        }))
        .expect("解析");
        let Input::Scroll {
            x,
            y,
            delta_x,
            delta_y,
        } = input
        else {
            panic!("应当是 Scroll");
        };
        assert_eq!((x, y, delta_x, delta_y), (1.0, 2.0, 40.0, -120.0));
    }

    /// 摆一个"用过一阵子"的 `HostBrowser`，但不真的起进程。
    ///
    /// `new` 是惰性的（第一次用到才 spawn），所以路径不存在无所谓 ——
    /// 这几个用例测的是崩溃之后这一层怎么收拾自己记着的状态。
    fn 用过的() -> Arc<HostBrowser> {
        let host = HostBrowser::new(
            PathBuf::from("/nonexistent/riot-browser.app"),
            PathBuf::from("/nonexistent/profile"),
        );
        {
            let mut tabs = host.tabs.try_lock().expect("刚建好，没人抢");
            tabs.order = vec![1, 2, 3];
            tabs.active = 2;
            tabs.next = 4;
        }
        host
    }

    /// 崩溃之后，按标签页号存的东西必须一个不留。
    ///
    /// 盯着的是"面板上有标签、点了全都没反应"这一类:那些号在新进程里
    /// 一个都不存在，发过去的每条命令都在子进程那边被丢掉，而没有任何一条
    /// 报错说得出"那些页早就没了"。
    #[tokio::test]
    async fn 崩溃清理丢掉所有标签页号() {
        let host = 用过的();
        *host.streaming.lock().await = Some(2);
        host.taps
            .lock()
            .await
            .insert(1, crate::browser::taps::EventTaps::default());
        host.intercept.lock().await.insert(1, Vec::new());
        *host.snap_refs.lock().await = Some((1, HashMap::new()));

        host.forget_crashed().await;

        {
            let tabs = host.tabs.lock().await;
            assert!(
                tabs.order.is_empty(),
                "幻影标签页会让面板上每一下点击都静默失败"
            );
            assert_eq!(tabs.active, 0, "0 不是合法的号，正是「什么都没打开」");
        }
        assert!(host.streaming.lock().await.is_none());
        assert!(host.taps.lock().await.is_empty());
        assert!(host.intercept.lock().await.is_empty());
        assert!(host.snap_refs.lock().await.is_none());
    }

    /// 标签页号不能从头再发。
    ///
    /// 重发的话，新进程的 1 号和刚刚消失的 1 号同号 —— 那些按号索引的表
    /// （等待者、抓包、拦截规则）分不出两者，旧页的延迟事件会落到新页上。
    #[tokio::test]
    async fn 崩溃清理之后号接着往下发() {
        let host = 用过的();
        host.forget_crashed().await;
        assert_eq!(host.tabs.lock().await.next, 4);
    }

    /// 面板尺寸要留着。
    ///
    /// 清掉的话，重开后的第一页会停在子进程那个 1280×800 的初值上 ——
    /// 前端只在尺寸**变化**时才发 resize，而崩溃前后面板尺寸没变，于是
    /// 没有任何一次 resize 来纠正它:画面带黑边、页面里的字被缩小，
    /// 一直到用户拖一下窗口。
    #[tokio::test]
    async fn 崩溃清理不碰面板尺寸() {
        let host = 用过的();
        *host.view.lock().await = Some((800, 600, 2.0));
        host.forget_crashed().await;
        assert_eq!(*host.view.lock().await, Some((800, 600, 2.0)));
    }

    /// 等开页的人要立刻收到坏消息。
    ///
    /// 那条 `TabOpened` 本该由已经没了的那个进程送来，永远不会到。不叫醒
    /// 的话它们各自等满 10 秒，而那十秒里"新建标签页"这个按钮只是僵着。
    #[tokio::test]
    async fn 崩溃清理叫醒等开页的人() {
        let host = 用过的();
        let (tx, rx) = oneshot::channel();
        host.opening.lock().await.insert(9, tx);

        host.forget_crashed().await;

        assert!(rx.await.is_err(), "唤醒端要被丢掉，等待者才会立刻失败");
        assert!(host.opening.lock().await.is_empty());
    }

    /// 历史两头的按钮要是灰的。
    ///
    /// 不灰的后果不是"按了没反应"这么轻:后退键在第一条上按下去，如果
    /// 越界没拦住而是按下标去取 id，取到的会是别的条目 —— 页面跳到一个
    /// 用户没去过的地方。
    #[test]
    fn 历史两头的前进后退是灰的() {
        let entries = vec![
            json!({ "id": 1, "url": "https://a.example/" }),
            json!({ "id": 2, "url": "https://b.example/" }),
            json!({ "id": 3, "url": "https://c.example/" }),
        ];

        let first = info_at(1, &entries, 0);
        assert_eq!(first.url, "https://a.example/");
        assert!(!first.can_back, "第一条不该能后退");
        assert!(first.can_forward);

        let middle = info_at(1, &entries, 1);
        assert!(middle.can_back && middle.can_forward, "中间两头都能走");

        let last = info_at(1, &entries, 2);
        assert!(last.can_back);
        assert!(!last.can_forward, "最后一条不该能前进");
    }

    /// 起始空白页在地址栏里是空的。
    ///
    /// 漏掉这一层的现象:打开面板第一眼，地址栏里躺着一串 `data:text/html,`
    /// —— 用户会以为那是刚才自己输错的东西，或者以为面板坏了。
    #[test]
    fn 空白页不出现在地址栏里() {
        for blank in [BLANK_PAGE, "about:blank"] {
            let entries = vec![json!({ "id": 1, "url": blank })];
            assert_eq!(info_at(1, &entries, 0).url, "", "{blank} 该显示成空");
        }
        // 真实地址一个字都不能动 —— 包括那些恰好也是 data: 的。
        let real = "data:text/html,<h1>hi</h1>";
        let entries = vec![json!({ "id": 1, "url": real })];
        assert_eq!(info_at(1, &entries, 0).url, real);
    }

    #[test]
    fn 没有历史时工具栏整个是空的() {
        // currentIndex 在浏览器刚起来、还没有任何文档时会是 -1。
        // 那时候拿它去索引会 panic，或者（更隐蔽）折回表尾拿到最后一条。
        assert_eq!(
            info_at(1, &[], -1),
            TabInfo {
                id: 1,
                ..TabInfo::default()
            }
        );
        assert_eq!(
            info_at(1, &[json!({ "url": "x" })], -1),
            TabInfo {
                id: 1,
                ..TabInfo::default()
            }
        );
    }
}
