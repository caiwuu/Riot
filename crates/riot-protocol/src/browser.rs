//! 主应用 ↔ 浏览器进程（`riot-browser`）的线格式。
//!
//! stdin/stdout 上跑 NDJSON:一行一条消息。选它而不是长度前缀的二进制，
//! 理由是可读 —— 出问题时能直接把 stdout 重定向到文件读。
//!
//! `[约束]` 这套类型放在协议层，两个进程各自 depend 同一份。
//!
//! 它们跨进程、分别编译，没有任何编译期检查能兜住不一致 —— 改了字段名而
//! 只更新一边，表现是"命令发过去没反应"，不报错也不崩。共享一份定义是
//! 唯一能让编译器管这件事的办法。
//!
//! `[约束]` 浏览器进程的 stdout **只能**用来传这些消息。CEF 和 Chromium
//! 自己会往 stderr 写大量日志，任何一行漏进 stdout 都会把 NDJSON 流冲坏，
//! 而表现是主应用这边"某条消息解析失败"，完全指不回真正的源头。
//!
//! # 帧不走这条通道
//!
//! 1280×800 的 BGRA 一帧是 4MB，按 base64 塞进 JSON 是 5.5MB，30fps 就是
//! 165MB/s —— 光是序列化就吃满一个核。帧走单独的共享内存，这里只传
//! "第几帧、多大"。见 [`Event::Frame`]。

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// 工具层能对浏览器做的事。
///
/// 和 [`crate::web::WebAccess`] 同一个路子:工具层只描述**要什么**，真正
/// 起进程、说 CDP、管生命周期都在宿主。这样 `riot-tools` 不用知道 CEF
/// 的存在，也就不用把那 355MB 的依赖拖进自己的构建。
///
/// 返回的字符串已经是给模型看的形状 —— 截断、过滤、整形都在实现里做完。
/// 工具层再加工一遍只会让"到底谁负责控制体积"这件事说不清。
#[async_trait]
pub trait BrowserAccess: Send + Sync {
    /// 导航到一个地址并等加载完成。
    async fn navigate(&self, url: &str) -> Result<(), BrowserUnavailable>;

    /// 整页截图，返回 **base64 编码**的 JPEG（见 [`SHOT_MEDIA_TYPE`]）。
    ///
    /// 不解码成字节是刻意的:CDP 给的就是 base64，而工具要塞进内容块的
    /// 也是 base64。中间解一次再编一次纯属白做，对一张几百 KB 的图还要
    /// 多两次全量拷贝。
    async fn screenshot(&self) -> Result<String, BrowserUnavailable>;

    /// 页面的可访问性快照，已经整形成文本。
    ///
    /// 行首的 `[n]` 是元素编号，交互方法（[`Self::click`] 等）用它指名
    /// 目标。编号只对**这一次快照**有效 —— 页面一变（导航、脚本改 DOM），
    /// 旧编号就指不到东西了。
    async fn snapshot(&self) -> Result<String, BrowserUnavailable>;

    /// 页面 console 里累积的消息。
    async fn console(&self) -> Result<Vec<String>, BrowserUnavailable>;

    /// 当前地址。没有页面时返回空串。
    async fn current_url(&self) -> String;

    /// 点击 [`Target`] 指定的元素。
    ///
    /// 目标在视口外时先滚进来再点。返回给模型看的结果描述 ——
    /// 点了什么、页面有没有因此跳转。
    async fn click(&self, target: Target) -> Result<String, InteractError>;

    /// 往 [`Target`] 指定的输入框里填文本：点击聚焦、清掉原值、输入。
    /// `submit` 为真时输入完再按一次回车。
    async fn type_text(
        &self,
        target: Target,
        text: &str,
        submit: bool,
    ) -> Result<String, InteractError>;

    /// 对当前聚焦的元素按一个功能键（Enter、Escape、Tab、方向键等）。
    async fn press_key(&self, key: &str) -> Result<String, InteractError>;

    /// 垂直滚动页面，正数向下（CSS 像素）。返回滚动后的位置描述。
    async fn scroll(&self, delta_y: f64) -> Result<String, InteractError>;

    /// 等某个条件成立，最多等 `timeout_ms` 毫秒。
    ///
    /// 自动化里最容易踩的坑是时序:点完立刻找下一个元素，而它还没渲染出来。
    /// 与其让模型 sleep 猜时间，不如显式等一个可验证的条件。返回等到了
    /// 什么（或超时）。
    async fn wait_for(
        &self,
        cond: WaitCondition,
        timeout_ms: u64,
    ) -> Result<String, InteractError>;

    /// 元素级动作:悬停、双击、右键、下拉选择、拖拽、组合键。见 [`Action`]。
    ///
    /// `[取舍]` 收成一个方法而不是每种动作一个 trait 方法。它们的形状一样
    /// （作用在一个元素上、返回一句结果、可能 `InteractError`），摊成七八个
    /// 方法只会让 `NoBrowser`/测试替身各抄七八遍。
    async fn act(&self, action: Action) -> Result<String, InteractError>;

    /// 页面级操作:前进后退刷新、标签页的列/开/切/关。见 [`Nav`]。
    async fn browse(&self, nav: Nav) -> Result<String, InteractError>;

    /// 在页面里执行一段 JS 并返回结果（已整形成文本）。
    ///
    /// 自动化的瑞士军刀:读 DOM/localStorage、算个值、调页面自己的函数。
    /// 支持 `await`。脚本抛异常时返回带异常信息的 `Target` 错误。
    async fn evaluate(&self, expr: &str) -> Result<String, InteractError>;

    /// 给一个文件输入框（`<input type=file>`）设置要上传的本地文件。
    ///
    /// `[约束]` 走 CDP 的 `DOM.setFileInputFiles` 而不是模拟点击 + 系统文件
    /// 选择框:后者会弹出一个原生对话框，离屏渲染下没人能操作它，自动化
    /// 当场卡死。前者直接把文件塞进 input，页面收到正常的 change 事件。
    async fn upload(&self, target: Target, paths: Vec<String>) -> Result<String, InteractError>;

    /// 当前页面的 Cookie，含安全属性（HttpOnly/Secure/SameSite）。
    ///
    /// `[约束]` 走 CDP 而不是 `document.cookie`:后者读不到 HttpOnly 的
    /// cookie，而会话令牌恰恰几乎都是 HttpOnly —— 少了它，登录态分析看到的
    /// 是残缺的一半。
    async fn cookies(&self) -> Result<String, InteractError>;

    /// 观察当前页面的网络流量。见 [`NetQuery`]。
    ///
    /// 被动观察:开 `Network.enable` 后累积请求/响应，供读回。这是渗透的
    /// 眼睛，也是"接口发现"的来源。第一次调用之后才开始累积 —— 要抓完整的
    /// 加载流量，先调一次 List 开着，再刷新页面。
    async fn network(&self, query: NetQuery) -> Result<String, InteractError>;

    /// 重放一个请求（Repeater）:改参数重发、看响应差异。
    ///
    /// 在**页面上下文**里 fetch，自动带上当前会话的 cookie（`credentials:
    /// include`）—— 这正是渗透要的:用已登录的身份重放、篡改。返回状态、
    /// 响应头、响应体（截断）。侵入性动作，受 scope 约束（在工具层把关）。
    async fn replay(
        &self,
        url: &str,
        method: &str,
        headers: serde_json::Value,
        body: Option<String>,
    ) -> Result<String, InteractError>;

    /// 拦截/改包。见 [`InterceptOp`]。侵入性动作，受 scope 约束。
    ///
    /// `[约束]` 只在有规则时才开 `Fetch.enable`。Fetch 一开会**暂停每一个
    /// 请求**等我们放行 —— 有 bug 漏放一个，页面就卡死。所以没规则时保持
    /// 关闭（零风险），事件循环里对不匹配的请求一律 continue。
    async fn intercept(&self, op: InterceptOp) -> Result<String, InteractError>;
}

/// 拦截规则的操作。
#[derive(Debug, Clone)]
pub enum InterceptOp {
    /// 拦截 URL 含某子串的请求，直接失败（BlockedByClient）。
    Block { url_pattern: String },
    /// 拦截 URL 含某子串的请求，用给定状态码和响应体伪造响应
    /// （测前端如何处理错误/异常数据）。
    Fulfill {
        url_pattern: String,
        status: u32,
        body: String,
    },
    /// 列出当前生效的拦截规则。
    List,
    /// 清空所有规则并关闭拦截。
    Clear,
}

/// 网络观察的三种查询。
#[derive(Debug, Clone)]
pub enum NetQuery {
    /// 列出抓到的请求（方法、URL、状态、类型、大小），可按 URL 子串过滤。
    List { filter: Option<String> },
    /// 看某一条请求的细节:请求/响应头 + 响应体（截断）。id 来自 List。
    Detail { request_id: String },
    /// 安全审计:检查主文档响应头（CSP/HSTS/X-Frame-Options/CORS 等）的
    /// 缺失与弱配置。
    Audit,
}

/// 元素级动作。定位统一走 [`Target`]。
#[derive(Debug, Clone)]
pub enum Action {
    /// 把鼠标移上去（不点）—— 触发悬停菜单、tooltip。
    Hover(Target),
    /// 双击 —— 选词、展开这类默认行为。
    DoubleClick(Target),
    /// 右键 —— 触发页面自定义的上下文菜单。
    RightClick(Target),
    /// 给下拉框/输入框设值并派发 change —— `<select>`、受控组件。
    SelectOption { target: Target, value: String },
    /// 从一个元素拖到另一个 —— 看板、排序列表、滑块。
    Drag { from: Target, to: Target },
    /// 组合键，如 `Control+a`、`Meta+c`。作用在当前焦点上。
    KeyChord(String),
}

/// 页面级操作。
#[derive(Debug, Clone)]
pub enum Nav {
    /// 后退一步。
    Back,
    /// 前进一步。
    Forward,
    /// 重新加载当前页。
    Reload,
    /// 列出所有标签页（号、标题、地址、哪个是活动的）。
    ListTabs,
    /// 新开一个空白标签页。要打开某地址用 `BrowserNavigate`（那条会走
    /// 域名同意）—— 这里不带 URL，免得开标签成了绕过同意的旁路。
    NewTab,
    /// 切到某个标签页（号来自 `ListTabs`）。
    SelectTab(u32),
    /// 关掉某个标签页。
    CloseTab(u32),
}

/// 怎么定位一个元素。
///
/// `[取舍]` 三种方式并存，不是只留编号。编号 `[n]` 来自快照、最省 token，
/// 但页面一变就失效；CSS 选择器和文本跨快照稳定，适合"点那个叫登录的按钮"
/// 这种意图明确、但不想先拍快照的场景。让模型按情况挑。
#[derive(Debug, Clone)]
pub enum Target {
    /// 最近一次 [`BrowserAccess::snapshot`] 输出里行首的编号 `[n]`。
    Ref(u32),
    /// CSS 选择器。多个匹配取第一个。
    Selector(String),
    /// 可见文本：找**包含**这段文字、且最靠近叶子的可点击元素。
    Text(String),
}

impl Target {
    /// 给模型看的一句话，进结果消息和错误里。
    pub fn describe(&self) -> String {
        match self {
            Target::Ref(n) => format!("元素 [{n}]"),
            Target::Selector(s) => format!("选择器 `{s}`"),
            Target::Text(t) => format!("文本 “{t}”"),
        }
    }
}

/// [`BrowserAccess::wait_for`] 等的条件。
#[derive(Debug, Clone)]
pub enum WaitCondition {
    /// 某 CSS 选择器匹配到元素（出现）。
    Selector(String),
    /// 某 CSS 选择器不再匹配（消失）—— 等加载动画转完、遮罩关掉。
    SelectorGone(String),
    /// 页面里出现某段可见文本。
    Text(String),
    /// 当前 URL 包含某子串 —— 等跳转到目标页。
    UrlContains(String),
    /// 网络空闲:一小段时间内没有在途请求。等 SPA 的数据加载完。
    NetworkIdle,
}

/// 交互（点击、输入）失败的两种形态。
///
/// `[约束]` 必须和 [`BrowserUnavailable`] 分开。工具层对这两种失败给模型的
/// 指引截然相反：浏览器不可用 → 别重试，改用 WebFetch；目标失效 → 重新
/// BrowserSnapshot 拿新编号再来。并成一种的话提示只能二选一，选哪个都会
/// 在另一半场景里把模型引进死胡同 —— 要么对着挂掉的浏览器反复快照，
/// 要么放着好好的页面去抓源码。
#[derive(Debug, Clone, thiserror::Error)]
pub enum InteractError {
    /// 浏览器整个用不了。
    #[error("{0}")]
    Unavailable(#[from] BrowserUnavailable),
    /// 目标指不到：编号不在最近一次快照里、元素已经从页面上消失、
    /// 或者它不适合这个动作（往按钮里打字）。消息已经是给模型看的形状，
    /// 包含下一步该怎么办。
    #[error("{0}")]
    Target(String),
}

/// 浏览器用不了。
///
/// 只有一个变体是刻意的:工具层对失败原因**不做分支**，它唯一能做的就是
/// 把话原样转给模型。分成"没打包 / 起不来 / 崩了"几种，只会诱导工具层去
/// 写一堆各自处理的分支，而那些分支的行为其实完全一样。
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct BrowserUnavailable(pub String);

/// 没有浏览器的占位实现。
///
/// `[约束]` 默认必须是它，而不是某个"尽力而为"的兜底。宿主忘了装配的
/// 表现应该是工具明确说"浏览器没起来"，而不是悄悄降级成别的行为。
pub struct NoBrowser;

#[async_trait]
impl BrowserAccess for NoBrowser {
    async fn navigate(&self, _url: &str) -> Result<(), BrowserUnavailable> {
        Err(unavailable())
    }
    async fn screenshot(&self) -> Result<String, BrowserUnavailable> {
        Err(unavailable())
    }
    async fn snapshot(&self) -> Result<String, BrowserUnavailable> {
        Err(unavailable())
    }
    async fn console(&self) -> Result<Vec<String>, BrowserUnavailable> {
        Err(unavailable())
    }
    async fn current_url(&self) -> String {
        String::new()
    }
    async fn click(&self, _target: Target) -> Result<String, InteractError> {
        Err(unavailable().into())
    }
    async fn type_text(
        &self,
        _target: Target,
        _text: &str,
        _submit: bool,
    ) -> Result<String, InteractError> {
        Err(unavailable().into())
    }
    async fn press_key(&self, _key: &str) -> Result<String, InteractError> {
        Err(unavailable().into())
    }
    async fn scroll(&self, _delta_y: f64) -> Result<String, InteractError> {
        Err(unavailable().into())
    }
    async fn wait_for(
        &self,
        _cond: WaitCondition,
        _timeout_ms: u64,
    ) -> Result<String, InteractError> {
        Err(unavailable().into())
    }
    async fn act(&self, _action: Action) -> Result<String, InteractError> {
        Err(unavailable().into())
    }
    async fn browse(&self, _nav: Nav) -> Result<String, InteractError> {
        Err(unavailable().into())
    }
    async fn evaluate(&self, _expr: &str) -> Result<String, InteractError> {
        Err(unavailable().into())
    }
    async fn upload(
        &self,
        _target: Target,
        _paths: Vec<String>,
    ) -> Result<String, InteractError> {
        Err(unavailable().into())
    }
    async fn cookies(&self) -> Result<String, InteractError> {
        Err(unavailable().into())
    }
    async fn network(&self, _query: NetQuery) -> Result<String, InteractError> {
        Err(unavailable().into())
    }
    async fn replay(
        &self,
        _url: &str,
        _method: &str,
        _headers: serde_json::Value,
        _body: Option<String>,
    ) -> Result<String, InteractError> {
        Err(unavailable().into())
    }
    async fn intercept(&self, _op: InterceptOp) -> Result<String, InteractError> {
        Err(unavailable().into())
    }
}

fn unavailable() -> BrowserUnavailable {
    BrowserUnavailable("这个版本没有内置浏览器，或者它没能启动。".into())
}

/// [`BrowserAccess::screenshot`] 出图的类型。
///
/// `[约束]` 实现和工具必须用同一个常量。各写一份字面量的后果是模型收到一张
/// 标着 PNG 的 JPEG —— 服务方要么直接拒收，要么解出一张坏图，而两种失败都
/// 不会指向"类型写错了"。
pub const SHOT_MEDIA_TYPE: &str = "image/jpeg";

/// 一个标签页的编号。
///
/// `[约束]` 由**主应用**分配，不是浏览器进程。浏览器那边创建一个 browser
/// 是异步的（`on_after_created` 稍后才来），要是等它回一个号，主应用就得把
/// "刚开的那个还没有号"这个中间态铺进所有代码路径。主应用自己发号则从一开始
/// 就有确定的身份，命令排在 `TabOpened` 之后发即可 —— 和等 [`Event::Ready`]
/// 是同一个模式。
pub type TabId = u32;

/// 主应用发给浏览器进程的命令。
///
/// 除了 `Shutdown`，每一条都指名一个标签页。浏览器进程同时持有多个 CEF
/// browser，"当前那个"这种隐式状态会让两边对不齐 —— 主应用切了标签、
/// 而某条早发的命令落在了新标签上。
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// 开一个标签页，停在 [`BLANK_PAGE`]。
    ///
    /// 号已经在这条命令里，但要等 [`Event::TabOpened`] 才能对它发别的命令
    /// —— 在那之前浏览器还不存在，命令会以"没有这个标签页"失败。
    OpenTab { tab: TabId },
    /// 关一个标签页。它的进程资源在 [`Event::TabClosed`] 之后才真的释放。
    CloseTab { tab: TabId },
    /// 导航到一个地址。
    Navigate { tab: TabId, url: String },
    /// 改视口尺寸。面板被拖动时发。
    ///
    /// `width`/`height` 是 CSS 像素，`scale` 是面板所在屏幕的像素密度
    /// （Retina 上是 2）。密度不改变页面的排版尺寸，只决定同一块地方用多少
    /// 物理像素去画。
    ///
    /// `[约束]` `scale` 要有默认值。这个结构跨进程，而浏览器的 `.app` 是
    /// 单独打包的，很容易比主应用旧。缺了默认值，旧 `.app` 收到带 `scale`
    /// 的命令会整条解析失败 —— 现象是"拖动面板之后画面再也不动了"，
    /// 而且两边都不报错。
    Resize {
        tab: TabId,
        width: i32,
        height: i32,
        #[serde(default = "one")]
        scale: f32,
    },
    /// 原始 CDP 消息，直接转给 `send_dev_tools_message`。
    ///
    /// 不在这里定义 CDP 的方法枚举:CDP 的域和参数由 Chromium 定义且随版本
    /// 变化，抄一份到 Rust 里只会多一个必须同步维护的副本。上层想调什么就
    /// 拼什么 JSON，这里只负责搬。
    ///
    /// `[约束]` CDP 的 `id` 由主应用在整个进程范围内分配，不是每个标签页
    /// 一套。响应从哪个标签页回来都靠 `id` 认领 —— 各标签页各发号的话，
    /// 两个标签页的第 1 号响应会撞在一起，而这种错乱只在多标签并发时出现。
    Cdp { tab: TabId, payload: serde_json::Value },
    /// 关掉浏览器，进程退出。
    Shutdown,
}

/// 老版本的浏览器进程不知道像素密度这回事，按一倍算。
fn one() -> f32 {
    1.0
}

/// 浏览器起来之后停在哪儿。
///
/// 进程一起来就联网是不对的:用户可能只是打开了面板，还没决定看什么。
///
/// `[约束]` 用 `data:`，**不要用 `about:blank`**。从 `about:blank` 导航到
/// https 会让 renderer 进程直接消失，页面报 `ERR_ABORTED`，紧接着 CDP 收到
/// `Inspector.detached / Render process gone`；而同一个导航从 `data:` 空页或
/// 任何真实页面出发都完全正常 —— 实测对比过三种起点。看现象很容易误判成
/// "创建后不能导航"或者"Chromium 崩了"。
///
/// `[约束]` 里面不能有需要百分号转义的字符（`<` `>` 空格之类）。Chromium 报
/// 回来的地址是转义后的形式，而主应用要拿这个常量去比 —— 比不上的话，
/// 用户开面板第一眼看到的就是地址栏里一串 `data:text/html,%3Chtml%3E...`。
///
/// 放在协议里而不是两边各写一份，就是为了那次比较能成立。
pub const BLANK_PAGE: &str = "data:text/html,";

/// 浏览器进程发给主应用的事件。
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// CEF 就绪，可以接命令了。
    ///
    /// 主应用必须等到这条再发命令。CEF 的初始化是异步的，早发的命令会
    /// 落在还不存在的浏览器上。
    ///
    /// 这一刻还**没有任何标签页** —— 开哪些页由主应用决定，进程自己开一个
    /// 就等于替它做主，而它可能正要恢复上次的几个页面。
    Ready,
    /// 标签页创建完成，可以对它发命令了。
    TabOpened { tab: TabId },
    /// 标签页真的关掉了。CEF 的关闭是异步的，这条之后号才能重用。
    ///
    /// `[约束]` 主应用必须处理这一条，不能只认自己发出的 `CloseTab`。页面
    /// 自己 `window.close()`、渲染进程崩掉都会走到这里 —— 不处理的话，
    /// 主应用的清单里留着一个已经不存在的号，而它还是"当前页"：之后每条
    /// 命令都以"没有这个标签页"被丢掉，每次 CDP 调用都要等满超时。表现是
    /// 面板彻底卡住，且没有任何一条报错指向"那一页已经没了"。
    TabClosed { tab: TabId },
    /// 页面想开一个新的浏览上下文：`target="_blank"`、`window.open()`、
    /// 按住 cmd 点链接。
    ///
    /// `[约束]` 浏览器进程**不会**自己开这一页，它只报告。标签页号由主应用
    /// 分配（见 [`TabId`]），进程自己造一个号必然和主应用发的号撞上。真的
    /// 开页是主应用发 [`Command::OpenTab`] + [`Command::Navigate`]。
    ///
    /// `[约束]` 这条事件是**必须**的，不是锦上添花。离屏渲染下让 CEF 按默认
    /// 行为创建弹窗，得到的是一个独立的原生窗口 —— 它在面板外面，不受标签栏
    /// 管理，而且和母页面共用同一个 client、也就是同一个标签页号。用户关掉
    /// 那个窗口时，`on_before_close` 报的是母页面的号，母页面于是被从表里
    /// 抹掉，面板随之卡死。
    PopupRequested {
        /// 发起的那一页。新页排在它右边 —— 和常见浏览器一致。
        source: TabId,
        /// 要打开的地址。空表示 `window.open()` 没给地址，那就是一张空白页。
        url: String,
        /// 该不该开在后台（cmd 点击、中键点击）。
        background: bool,
    },
    /// 新的一帧可用。像素在共享内存里，这里只给元数据。
    Frame {
        tab: TabId,
        seq: u64,
        width: i32,
        height: i32,
    },
    /// 页面加载结束。
    LoadEnd { tab: TabId, status: i32, url: String },
    /// 页面加载失败。
    LoadError {
        tab: TabId,
        code: i32,
        text: String,
        url: String,
    },
    /// CDP 的响应或事件，原样回传。
    Cdp {
        tab: TabId,
        payload: serde_json::Value,
    },
    /// 进程内部出错。不致命的也报 —— 静默降级比崩溃难查。
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 命令的线格式是稳定的() {
        // 这个格式跨进程，两边各自编译。改了 tag 名或字段名而没同步改
        // 主应用，表现是"命令发过去没反应"，不会有任何编译错误。
        let json = r#"{"cmd":"navigate","tab":2,"url":"https://example.com/"}"#;
        let cmd: Command = serde_json::from_str(json).expect("解析");
        assert!(matches!(
            cmd,
            Command::Navigate { tab: 2, url } if url == "https://example.com/"
        ));
    }

    /// 每条命令都得指名标签页。
    ///
    /// 漏掉 `tab` 的那条命令会在浏览器进程里解析失败 —— 而失败的样子是
    /// "这一条静静地没了"，别的命令照常工作。所以这里把每一条都过一遍。
    #[test]
    fn 除了关机每条命令都带标签页号() {
        let cases = [
            r#"{"cmd":"open_tab","tab":1}"#,
            r#"{"cmd":"close_tab","tab":1}"#,
            r#"{"cmd":"navigate","tab":1,"url":"https://x.test/"}"#,
            r#"{"cmd":"resize","tab":1,"width":700,"height":900,"scale":2.0}"#,
            r#"{"cmd":"cdp","tab":1,"payload":{"id":1,"method":"Page.enable"}}"#,
        ];
        for json in cases {
            let cmd: Command = serde_json::from_str(json)
                .unwrap_or_else(|e| panic!("{json} 解析失败：{e}"));
            assert!(!matches!(cmd, Command::Shutdown));
        }
        // 关机不针对某个标签页 —— 它关的是整个进程。
        assert!(matches!(
            serde_json::from_str::<Command>(r#"{"cmd":"shutdown"}"#).expect("解析"),
            Command::Shutdown
        ));
    }

    /// 旧版本发来的 resize 还得能解析。
    ///
    /// 浏览器的 `.app` 单独打包，装机上的那一份和主应用不保证同版本。
    /// 少了默认值的话，缺 `scale` 的那条命令整条失败 —— 拖动面板之后
    /// 画面就再也不动了，而两边都不会报错。
    #[test]
    fn 不带像素密度的_resize_按一倍算() {
        let json = r#"{"cmd":"resize","tab":1,"width":700,"height":900}"#;
        let cmd: Command = serde_json::from_str(json).expect("解析");
        let Command::Resize { width, height, scale, .. } = cmd else {
            panic!("应该是 Resize");
        };
        assert_eq!((width, height), (700, 900));
        assert_eq!(scale, 1.0, "缺省密度必须是 1，0 会让 Chromium 除以零");
    }

    #[test]
    fn cdp_载荷不做任何解释() {
        // 上层拼什么就传什么。这里加一层枚举等于把 Chromium 的协议抄一遍，
        // 而那份东西每个版本都在动。
        let json = r#"{"cmd":"cdp","tab":1,"payload":{"id":1,"method":"Page.captureScreenshot"}}"#;
        let Command::Cdp { payload, .. } = serde_json::from_str(json).expect("解析") else {
            panic!("应该是 Cdp");
        };
        assert_eq!(payload["method"], "Page.captureScreenshot");
    }

    #[test]
    fn 事件序列化成单行() {
        // NDJSON 的前提是一条消息一行。多行会把流切错位。
        let line = serde_json::to_string(&Event::Frame {
            tab: 1,
            seq: 7,
            width: 1280,
            height: 800,
        })
        .expect("序列化");
        assert!(!line.contains('\n'), "事件不能跨行: {line}");
        assert!(line.contains("\"event\":\"frame\""));
    }

    /// 事件也要带标签页号。
    ///
    /// 不带的话，主应用只能猜"这一帧是哪个页面的" —— 切标签的瞬间会把
    /// 旧页面的最后几帧画在新标签上，看起来像切换失灵。
    #[test]
    fn 页面事件带标签页号() {
        for ev in [
            Event::TabOpened { tab: 3 },
            Event::TabClosed { tab: 3 },
            Event::LoadEnd { tab: 3, status: 200, url: "https://x.test/".into() },
            Event::Cdp { tab: 3, payload: serde_json::json!({ "id": 1 }) },
        ] {
            let line = serde_json::to_string(&ev).expect("序列化");
            assert!(line.contains("\"tab\":3"), "{line} 里应当有标签页号");
        }
    }
}
