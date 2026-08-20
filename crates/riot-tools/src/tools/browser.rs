//! 内置浏览器的工具。
//!
//! 让模型自己看见页面:改完前端 → 打开 → 截图 → 读 console 报错 → 再改。
//! 这个闭环是纯文本 agent 做不到的 —— 它只能靠用户描述"看起来不对"。
//!
//! 真正干活的在宿主（[`riot_protocol::browser::BrowserAccess`]），这一层
//! 只做三件事:参数校验、权限判定、把结果整形成给模型的样子。
//!
//! # 权限:和 WebFetch 共用同一份域名同意
//!
//! `[约束]` 导航用的内容键 http(s) 是 `domain:<host>`，**和 WebFetch 完全一致**。
//! 本地文件是 `file:<目录>`：同一页的静态资源都在旁边，问一次覆盖整个目录。
//! URL 准入不共用：抓取拒绝内网，浏览器允许 localhost / 内网 / 本地 file://
//! （仍要过同意弹窗）。
//!
//! 用户点"总是允许 example.com"表达的是"我信任这个站"，那个判断和用哪个
//! 工具去访问无关。各用各的键会让同一个域名被问两遍，而且「全部放行」
//! 只对其中一边生效 —— 那种不一致用户根本没法理解。

use std::path::PathBuf;
use std::sync::Arc;

use riot_protocol::browser::{
    Action, BrowserUnavailable, InteractError, InterceptOp, Nav, NetQuery, Target, WaitCondition,
};
use riot_protocol::message::ToolResultContent;
use riot_permissions::{MatchMode, RuleSet};
use riot_protocol::permission::{
    DecisionReason, PermissionContext, PermissionMode, PermissionResult, PermissionUpdate,
    RuleDecision, SafetyKind, UpdateScope,
};
use riot_protocol::tool::{
    InterruptBehavior, PromptContext, ResultBudget, Tool, ToolContext, ToolOutcome,
    ValidationError,
};

use super::web::url as weburl;

/// 导航的入参。
///
/// 字段只用来生成 schema —— 真正读参数走 `input.get("url")`，因为
/// check_permissions 拿到的是原始 JSON，两处用同一条路径更不容易漂。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct NavigateInput {
    /// 要打开的地址，必须含协议（`http://`、`https://` 或本地 `file://`）。
    /// 本地开发服务器用 `http://localhost:...`，不要改成 https。
    /// 本地 HTML 用 `file:///绝对路径`，也可以直接给本机绝对路径。
    url: String,
}

/// 不需要参数的工具共用。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct NoInput {}

/// 截图的上限，按 base64 后的长度算。
///
/// 超过就说明页面很长或者很复杂。与其把几 MB 的 base64 塞进上下文（一张图
/// 能吃掉小半个窗口，后面几轮全靠压缩苟活），不如让模型换个更窄的问法。
const MAX_SHOT_B64: usize = 2_000_000;

// ── 导航 ──────────────────────────────────────────────

pub struct BrowserNavigate;

#[async_trait::async_trait]
impl Tool for BrowserNavigate {
    fn name(&self) -> &'static str {
        "BrowserNavigate"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "在内置浏览器里打开一个地址并等页面加载完。\
         之后可以用 BrowserSnapshot 看结构、BrowserScreenshot 看渲染、\
         BrowserConsole 看报错，用 BrowserClick / BrowserType 操作页面。\
         适合验证自己刚改完的前端。\
         `url` 必须带协议；本地开发服务器用 `http://localhost:端口/...`，不要改成 https。\
         本地 HTML / 静态页用 `file:///绝对路径`，也可以直接给本机绝对路径。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(NavigateInput)
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let url = input.get("url").and_then(|v| v.as_str()).unwrap_or("...");
        format!("在浏览器里打开 {url}")
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        // 导航会让页面执行脚本、发请求、写它自己的 storage。叫它只读会让
        // 调度器把它和别的工具并发跑，而两次导航打架的结果无法预测。
        false
    }

    fn interrupt_behavior(&self) -> InterruptBehavior {
        InterruptBehavior::Cancel
    }

    /// 本地文件走路径安全检查（凭证、`.ssh` 等）。http(s) 没有单一文件目标。
    fn target_path(&self, input: &serde_json::Value) -> Option<PathBuf> {
        let raw = input.get("url").and_then(|v| v.as_str())?;
        weburl::local_file_path(&weburl::normalize_for_browser(raw).ok()?)
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        let raw = input.get("url").and_then(|v| v.as_str()).unwrap_or_default();
        weburl::normalize_for_browser(raw).map(|_| ()).map_err(|e| {
            ValidationError::rejected(format!("{e}"))
        })
    }

    fn check_permissions(
        &self,
        input: &serde_json::Value,
        ctx: &PermissionContext,
    ) -> PermissionResult {
        let raw = input.get("url").and_then(|v| v.as_str()).unwrap_or_default();
        // 解析不了的交给 validate_input 报错。这里说 Deny 的话，模型收到的
        // 是"没权限"，它会去要权限而不是修 URL。
        let Ok(u) = weburl::normalize_for_browser(raw) else {
            return PermissionResult::Passthrough;
        };
        super::web::consent::decide_for_domain(self.name(), &u, ctx)
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let raw = input.get("url").and_then(|v| v.as_str()).unwrap_or_default();
        let u = match weburl::normalize_for_browser(raw) {
            Ok(u) => u,
            Err(e) => return ToolOutcome::failed(format!("无法打开这个地址：{e}")),
        };
        match ctx.browser.navigate(u.as_str()).await {
            Ok(()) => ToolOutcome::ok_text(format!("已打开 {u}")),
            Err(e) => ToolOutcome::failed(unavailable_hint(&e)),
        }
    }
}

// ── 快照 ──────────────────────────────────────────────

pub struct BrowserSnapshot;

#[async_trait::async_trait]
impl Tool for BrowserSnapshot {
    fn name(&self) -> &'static str {
        "BrowserSnapshot"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "读当前页面的可访问性结构：有哪些按钮、链接、输入框，各自叫什么。\
         行首的 [n] 是元素编号，BrowserClick / BrowserType 用它指定目标；\
         页面变过之后编号会失效，重新快照拿新的。\
         比截图省得多，判断页面上有什么，优先用它。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(NoInput)
    }

    fn describe(&self, _input: &serde_json::Value) -> String {
        "读当前页面的结构".to_owned()
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        read_current_page()
    }

    async fn call(&self, _input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        match ctx.browser.snapshot().await {
            Ok(s) if s.trim().is_empty() => {
                ToolOutcome::ok_text("页面上没有可识别的结构。可能还没导航，或者页面是空的。")
            }
            Ok(s) => ToolOutcome::ok_text(s),
            Err(e) => ToolOutcome::failed(unavailable_hint(&e)),
        }
    }
}

// ── 截图 ──────────────────────────────────────────────

pub struct BrowserScreenshot;

#[async_trait::async_trait]
impl Tool for BrowserScreenshot {
    fn name(&self) -> &'static str {
        "BrowserScreenshot"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "给当前页面截整页的图。判断视觉效果（布局、间距、颜色、有没有错位）\
         用它；只想知道页面上有什么元素的话，BrowserSnapshot 更省。\
         截图会直接显示在界面的工具结果卡片里，用户自己看得到 —— 不要试图\
         在回复里贴图，也不要说自己无法展示图片。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(NoInput)
    }

    fn describe(&self, _input: &serde_json::Value) -> String {
        "给当前页面截图".to_owned()
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn result_budget(&self) -> ResultBudget {
        // 图片按自己的上限管，不走文本预算 —— 用文本长度衡量 base64
        // 会把一张正常的图判成"结果过大"。
        ResultBudget::Unlimited
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        read_current_page()
    }

    async fn call(&self, _input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let full = match ctx.browser.screenshot().await {
            Ok(d) => d,
            Err(e) => return ToolOutcome::failed(unavailable_hint(&e)),
        };

        // 原图落盘，界面按路径显示。消息里只进压缩图 —— 一张整页截图的
        // base64 有几 MB，塞进会话历史的话，切一次会话就要整个搬一遍。
        // decode 不动原串:`full` 后面可能原样进消息（压不了的兜底路径）。
        let raw = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &full).ok();
        let path = match &raw {
            Some(bytes) => stash_original(&ctx, bytes).await,
            None => None,
        };

        // 给模型的图压到视觉模型的甜点尺寸（见 shrink 模块）。压不了就
        // 原样用 —— 压缩是优化不是闸门。
        let (data, media_type) = match raw.as_deref().and_then(super::shrink::for_model) {
            Some(s) => (s.data, s.media_type),
            None => (full, riot_protocol::browser::SHOT_MEDIA_TYPE),
        };

        // 压缩后还超上限说明图不正常（压缩失败的超大原图、异常格式）。
        // 这个闸拦的是"几 MB 的 base64 进上下文"，不再是常规长页面 ——
        // 长页面压完一般只有一两百 KB。
        if data.len() > MAX_SHOT_B64 {
            return ToolOutcome::failed(format!(
                "截图有 {} KB，超过上限。这个页面太长或太复杂 —— \
                 先用 BrowserSnapshot 定位到关心的区域，或者把窗口调窄再截。",
                data.len() / 1024
            ));
        }

        // 模型自己能看图就直接给它压缩图。
        if ctx.vision.accepts_images() {
            return ToolOutcome::Ok {
                model_content: ToolResultContent::Image {
                    media_type: media_type.into(),
                    data,
                    path,
                },
                ui_payload: None,
                side_messages: Vec::new(),
            };
        }

        // 看不了图的模型走视觉兼容:让一个能看图的辅助模型把它转成文字。
        // 喂的也是压缩图 —— 描述布局用不着原始分辨率，图小了转述模型
        // 更快、更便宜，也更不容易把输出预算写爆。
        //
        // `[约束]` 不能把图片照发。纯文本模型那边图片会在 provider 层被丢掉，
        // 模型只收到一句"有张图"，然后它会自己想办法 —— 去 shell 里
        // screencapture 截整个屏幕，再拿着一张截错的图言之凿凿。
        //
        // 转述给模型，图片本体留给界面（DescribedImage）—— 用户在工具卡片
        // 里看到的应该是截图本身，而不是一段写给模型的转述文字。
        match ctx
            .vision
            .describe(riot_protocol::vision::DescribeRequest {
                media_type: media_type.into(),
                data: data.clone(),
                focus: "页面的整体布局、可见的文字和控件、有没有明显的错位或\
                        空白，以及任何看起来是报错的内容"
                    .into(),
            })
            .await
        {
            Ok(text) => ToolOutcome::Ok {
                model_content: ToolResultContent::DescribedImage {
                    media_type: media_type.into(),
                    data,
                    path,
                    text,
                },
                ui_payload: None,
                side_messages: Vec::new(),
            },
            Err(e) => ToolOutcome::failed(format!(
                "{e}\n在此之前可以用 BrowserSnapshot 看页面结构 —— \
                 判断页面上有什么它就够了。不要用 shell 截屏，\
                 那截的是整个屏幕而不是页面。"
            )),
        }
    }
}

// ── 带编号框的视口快照（Set-of-Marks）────────────────────

pub struct BrowserView;

#[async_trait::async_trait]
impl Tool for BrowserView {
    fn name(&self) -> &'static str {
        "BrowserView"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "看当前视口:一张给每个可交互元素叠了编号框的截图，外加「编号 → 元素」\
         清单。图上第 [n] 个框就是清单里的 [n]，BrowserClick / BrowserType 用这个\
         编号指目标。纯文本快照（BrowserSnapshot）说不清「是哪一个」时（多个同名\
         按钮、密集布局）用它 —— 看图一眼就能分辨。编号只对这次调用有效，\
         页面一变（导航、脚本改 DOM）就要重新看。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(NoInput)
    }

    fn describe(&self, _input: &serde_json::Value) -> String {
        "看当前视口（带编号框）".to_owned()
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn result_budget(&self) -> ResultBudget {
        // 图按自己的上限管，不走文本预算（同 BrowserScreenshot）。
        ResultBudget::Unlimited
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        read_current_page()
    }

    async fn call(&self, _input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let view = match ctx.browser.snapshot_marked().await {
            Ok(v) => v,
            Err(e) => return ToolOutcome::failed(unavailable_hint(&e)),
        };
        let listing = if view.listing.trim().is_empty() {
            "页面上没有可识别的结构。可能还没导航，或者页面是空的。".to_owned()
        } else {
            view.listing
        };

        // 纯文本模型看不了图，带框截图对它没意义 —— 退回编号清单文本
        // （等同 BrowserSnapshot）。
        if !ctx.vision.accepts_images() {
            return ToolOutcome::ok_text(listing);
        }

        // 落盘原图给界面，压缩图给模型（同 BrowserScreenshot 那套）。
        let raw =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &view.screenshot)
                .ok();
        let path = match &raw {
            Some(bytes) => stash_original(&ctx, bytes).await,
            None => None,
        };
        let (data, media_type) = match raw.as_deref().and_then(super::shrink::for_model) {
            Some(s) => (s.data, s.media_type),
            None => (view.screenshot, riot_protocol::browser::SHOT_MEDIA_TYPE),
        };
        // 压完还超上限说明图不正常 —— 退回纯清单，编号本身已经够指目标。
        if data.len() > MAX_SHOT_B64 {
            return ToolOutcome::ok_text(listing);
        }

        ToolOutcome::Ok {
            model_content: ToolResultContent::MarkedImage {
                media_type: media_type.into(),
                data,
                path,
                text: listing,
            },
            ui_payload: None,
            side_messages: Vec::new(),
        }
    }
}

/// 截图原图落盘（工件目录，会话专属），返回写成的路径。
///
/// 写不进返回 `None`，工具照常出结果 —— 界面拿压缩图兜底显示。落盘是
/// 给界面和用户留档的优化，不能成为截图链路上的新故障点。
async fn stash_original(ctx: &ToolContext, bytes: &[u8]) -> Option<std::path::PathBuf> {
    // tool_use_id 全局唯一，天然不撞名；扩展名跟 SHOT_MEDIA_TYPE（jpeg）。
    let path = ctx
        .artifacts_dir
        .join(format!("{}.jpg", ctx.tool_use_id.as_str()));
    ctx.fs.write(&path, bytes).await.ok()?;
    Some(path)
}

// ── console ───────────────────────────────────────────

pub struct BrowserConsole;

#[async_trait::async_trait]
impl Tool for BrowserConsole {
    fn name(&self) -> &'static str {
        "BrowserConsole"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "读当前页面 console 里的消息，含加载期间的报错和未捕获异常。\
         页面看起来不对但结构正常时，先看这里。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(NoInput)
    }

    fn describe(&self, _input: &serde_json::Value) -> String {
        "读当前页面的 console".to_owned()
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        read_current_page()
    }

    async fn call(&self, _input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        match ctx.browser.console().await {
            Ok(logs) if logs.is_empty() => ToolOutcome::ok_text("console 是空的，没有报错。"),
            Ok(logs) => ToolOutcome::ok_text(logs.join("\n")),
            Err(e) => ToolOutcome::failed(unavailable_hint(&e)),
        }
    }
}

// ── 点击 ──────────────────────────────────────────────

/// 点击的入参。三种定位方式给一个即可（优先级 ref > selector > text）。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct ClickInput {
    /// 元素编号，即 BrowserSnapshot 输出里行首的 [n]。最省 token。
    #[serde(default)]
    r#ref: Option<u32>,
    /// CSS 选择器，如 `#login` 或 `button.primary`。跨快照稳定。
    #[serde(default)]
    selector: Option<String>,
    /// 可见文本，如 `登录`。找包含这段文字的最贴切的可点击元素。
    #[serde(default)]
    text: Option<String>,
    /// 双击（选词、展开这类默认行为）。默认单击。
    #[serde(default)]
    double: bool,
    /// 右键（触发页面的上下文菜单）。默认左键。
    #[serde(default)]
    right: bool,
}

pub struct BrowserClick;

#[async_trait::async_trait]
impl Tool for BrowserClick {
    fn name(&self) -> &'static str {
        "BrowserClick"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "点击页面上的一个元素。三种指定方式给一个即可:ref（BrowserSnapshot \
         行首的 [n]，最省）、selector（CSS 选择器）、text（可见文字）。\
         double=true 双击、right=true 右键。目标在视口外会自动滚进视野；\
         点完会报告页面有没有因此跳转。用 ref 报错说编号失效时，\
         重新 BrowserSnapshot，或改用 selector/text。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(ClickInput)
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let verb = if input.get("double").and_then(serde_json::Value::as_bool) == Some(true) {
            "双击"
        } else if input.get("right").and_then(serde_json::Value::as_bool) == Some(true) {
            "右键"
        } else {
            "点击"
        };
        match target_from_input(input) {
            Some(t) => format!("{verb}{}", t.describe()),
            None => format!("{verb}页面元素"),
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        // 点击会触发页面脚本、提交表单、发请求。和导航同理，
        // 并发跑两次点击的结果无法预测。
        false
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        require_target(input)
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        ctx: &PermissionContext,
    ) -> PermissionResult {
        interact_consent(ctx)
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let Some(t) = target_from_input(&input) else {
            return ToolOutcome::failed("要点哪个:给 ref、selector 或 text 之一。");
        };
        let double = input.get("double").and_then(serde_json::Value::as_bool) == Some(true);
        let right = input.get("right").and_then(serde_json::Value::as_bool) == Some(true);
        let result = if double {
            ctx.browser.act(Action::DoubleClick(t)).await
        } else if right {
            ctx.browser.act(Action::RightClick(t)).await
        } else {
            ctx.browser.click(t).await
        };
        match result {
            Ok(msg) => ToolOutcome::ok_text(msg),
            Err(e) => ToolOutcome::failed(interact_hint(e)),
        }
    }
}

// ── 输入文本 ──────────────────────────────────────────

/// 填文本的入参。定位方式给一个（ref/selector/text），外加要填的内容。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct TypeInput {
    /// 目标输入框编号，来自 BrowserSnapshot 行首的 [n]。
    #[serde(default)]
    r#ref: Option<u32>,
    /// 目标输入框的 CSS 选择器。
    #[serde(default)]
    selector: Option<String>,
    /// 目标输入框附近的可见文本（如占位符、标签文字）。
    #[serde(default, rename = "target_text")]
    target_text: Option<String>,
    /// 要输入的文本。会替换框里原有的内容。
    text: String,
    /// 输入完是否按回车（提交表单、触发搜索）。默认不按。
    #[serde(default)]
    submit: bool,
}

pub struct BrowserType;

#[async_trait::async_trait]
impl Tool for BrowserType {
    fn name(&self) -> &'static str {
        "BrowserType"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "往输入框里填文本：点击聚焦、替换原有内容。定位方式给一个:ref \
         （BrowserSnapshot 行首的 [n]）、selector（CSS 选择器）、target_text \
         （输入框附近的可见文字）。submit 为 true 时填完按一次回车 —— \
         搜索框、单行表单用它能省一次调用。目标不是输入框时会明确报错。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(TypeInput)
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let text = input.get("text").and_then(|v| v.as_str()).unwrap_or("...");
        let short: String = text.chars().take(30).collect();
        let ellipsis = if text.chars().count() > 30 { "…" } else { "" };
        match type_target(input) {
            Some(t) => format!("在{}输入 \"{short}{ellipsis}\"", t.describe()),
            None => format!("在页面里输入 \"{short}{ellipsis}\""),
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        false
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        if type_target(input).is_none() {
            return Err(ValidationError::rejected(
                "要填哪个输入框:给 ref、selector 或 target_text 之一。",
            ));
        }
        if input.get("text").and_then(|v| v.as_str()).is_none() {
            return Err(ValidationError::rejected("缺少要输入的 text。"));
        }
        Ok(())
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        ctx: &PermissionContext,
    ) -> PermissionResult {
        interact_consent(ctx)
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let Some(t) = type_target(&input) else {
            return ToolOutcome::failed("要填哪个输入框:给 ref、selector 或 target_text 之一。");
        };
        let text = input.get("text").and_then(|v| v.as_str()).unwrap_or_default();
        let submit = input.get("submit").and_then(|v| v.as_bool()).unwrap_or(false);
        match ctx.browser.type_text(t, text, submit).await {
            Ok(msg) => ToolOutcome::ok_text(msg),
            Err(e) => ToolOutcome::failed(interact_hint(e)),
        }
    }
}

/// BrowserType 的定位:和点击同一套三选一，只是文本字段叫 `target_text`
/// （`text` 已经用来装"要输入的内容"了，不能撞名）。
fn type_target(input: &serde_json::Value) -> Option<Target> {
    if let Some(n) = input.get("ref").and_then(serde_json::Value::as_u64).and_then(|v| u32::try_from(v).ok()) {
        return Some(Target::Ref(n));
    }
    if let Some(s) = input.get("selector").and_then(serde_json::Value::as_str).filter(|s| !s.is_empty()) {
        return Some(Target::Selector(s.to_owned()));
    }
    if let Some(t) = input.get("target_text").and_then(serde_json::Value::as_str).filter(|t| !t.is_empty()) {
        return Some(Target::Text(t.to_owned()));
    }
    None
}

// ── 按键 ──────────────────────────────────────────────

/// 按键的入参。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct KeyInput {
    /// 单个功能键（Enter、Tab、Escape、方向键…），或组合键如 `Control+a`、
    /// `Meta+c`、`Control+Shift+k`。修饰键名:Control/Ctrl、Meta/Cmd、Shift、
    /// Alt/Option。
    key: String,
}

pub struct BrowserKey;

/// key 里带 `+` 且不止一段就是组合键（把单独一个 `+` 排除掉）。
fn is_chord(key: &str) -> bool {
    key.contains('+') && key.split('+').filter(|p| !p.trim().is_empty()).count() >= 2
}

#[async_trait::async_trait]
impl Tool for BrowserKey {
    fn name(&self) -> &'static str {
        "BrowserKey"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "对当前聚焦的元素按键:单个功能键（Enter、Escape、Tab、方向键等），\
         或组合键（Control+a 全选、Meta+c 复制、Control+Shift+k…）。\
         作用在焦点上 —— 通常先 BrowserClick 或 BrowserType 把焦点放对。\
         要输入整段文字用 BrowserType，不要逐键拼。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(KeyInput)
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let key = input.get("key").and_then(|v| v.as_str()).unwrap_or("...");
        format!("按 {key}")
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        false
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        match input.get("key").and_then(|v| v.as_str()) {
            Some(k) if !k.trim().is_empty() => Ok(()),
            _ => Err(ValidationError::rejected("缺少要按的 key。")),
        }
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        ctx: &PermissionContext,
    ) -> PermissionResult {
        interact_consent(ctx)
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let key = input.get("key").and_then(|v| v.as_str()).unwrap_or_default();
        // 组合键走 act(KeyChord)，单键走 press_key —— 后者只认功能键白名单，
        // 组合键（Control+a）过不了那道校验，得分流。
        let result = if is_chord(key) {
            ctx.browser.act(Action::KeyChord(key.to_owned())).await
        } else {
            ctx.browser.press_key(key).await
        };
        match result {
            Ok(msg) => ToolOutcome::ok_text(msg),
            Err(e) => ToolOutcome::failed(interact_hint(e)),
        }
    }
}

// ── 滚动 ──────────────────────────────────────────────

/// 滚动的入参。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct ScrollInput {
    /// 垂直滚动距离（CSS 像素），正数向下、负数向上。一屏大约 700。
    delta_y: f64,
}

pub struct BrowserScroll;

#[async_trait::async_trait]
impl Tool for BrowserScroll {
    fn name(&self) -> &'static str {
        "BrowserScroll"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "垂直滚动页面，正数向下（一屏约 700px）。点击和输入会自动把目标\
         滚进视野，不用先滚再点；这个工具用于浏览长页面、触发懒加载。\
         滚完会报告当前位置。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(ScrollInput)
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let dy = input.get("delta_y").and_then(serde_json::Value::as_f64).unwrap_or(0.0);
        if dy < 0.0 {
            format!("向上滚动页面 {:.0}px", -dy)
        } else {
            format!("向下滚动页面 {dy:.0}px")
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        // 滚动不改页面数据，但会动视口、可能触发懒加载 ——
        // 和别的交互并发跑会互相拆台，按写操作排队。
        false
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        match input.get("delta_y").and_then(serde_json::Value::as_f64) {
            Some(d) if d.is_finite() && d != 0.0 => Ok(()),
            Some(_) => Err(ValidationError::rejected("delta_y 不能是 0。")),
            None => Err(ValidationError::rejected("缺少滚动距离 delta_y。")),
        }
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        // 滚动和截图属于同一信任级别:页面数据不动，动的只是看哪儿。
        // 交互三件套要问，它不问 —— 问它的后果是模型每看一屏弹一次窗。
        PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Preapproved {
                what: "滚动当前页面".into(),
            },
        }
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let dy = input.get("delta_y").and_then(serde_json::Value::as_f64).unwrap_or(0.0);
        match ctx.browser.scroll(dy).await {
            Ok(msg) => ToolOutcome::ok_text(msg),
            Err(e) => ToolOutcome::failed(interact_hint(e)),
        }
    }
}

// ── 等待 ──────────────────────────────────────────────

/// 等待条件的入参。恰好给一个条件字段，外加可选超时。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct WaitInput {
    /// 等这个 CSS 选择器匹配到元素（出现）。
    #[serde(default)]
    selector: Option<String>,
    /// 等这个 CSS 选择器不再匹配（消失）——加载动画、遮罩关掉。
    #[serde(default)]
    selector_gone: Option<String>,
    /// 等页面里出现这段可见文本。
    #[serde(default)]
    text: Option<String>,
    /// 等当前地址包含这个子串——跳转到目标页。
    #[serde(default)]
    url_contains: Option<String>,
    /// 等网络空闲（一小段时间没有在途请求）——SPA 数据加载完。
    #[serde(default)]
    network_idle: bool,
    /// 最多等多少毫秒。默认 10000。
    #[serde(default)]
    timeout_ms: Option<u64>,
}

pub struct BrowserWaitFor;

#[async_trait::async_trait]
impl Tool for BrowserWaitFor {
    fn name(&self) -> &'static str {
        "BrowserWaitFor"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "等一个条件成立再继续，别用 sleep 猜时间。恰好给一个条件:selector\
         （元素出现）、selector_gone（元素消失）、text（文本出现）、\
         url_contains（跳转到目标页）、network_idle（网络空闲）。\
         点击/输入后要等结果渲染出来时，先 wait 再 snapshot 更稳。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(WaitInput)
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        match wait_condition(input) {
            Some(WaitCondition::Selector(s)) => format!("等元素 `{s}` 出现"),
            Some(WaitCondition::SelectorGone(s)) => format!("等元素 `{s}` 消失"),
            Some(WaitCondition::Text(t)) => format!("等文本 “{t}”"),
            Some(WaitCondition::UrlContains(u)) => format!("等地址包含 “{u}”"),
            Some(WaitCondition::NetworkIdle) => "等网络空闲".to_owned(),
            None => "等待页面条件".to_owned(),
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        // 只是观察等待，不改页面。等待过程中放行并发的读也无害。
        true
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        read_current_page()
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        if wait_condition(input).is_none() {
            return Err(ValidationError::rejected(
                "给一个等待条件:selector / selector_gone / text / url_contains / network_idle。",
            ));
        }
        Ok(())
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let Some(cond) = wait_condition(&input) else {
            return ToolOutcome::failed(
                "给一个等待条件:selector / selector_gone / text / url_contains / network_idle。",
            );
        };
        let timeout = input
            .get("timeout_ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(10_000)
            // 兜个上限:一次 wait 挂十分钟只会让整轮卡死。
            .min(120_000);
        match ctx.browser.wait_for(cond, timeout).await {
            Ok(msg) => ToolOutcome::ok_text(msg),
            Err(e) => ToolOutcome::failed(interact_hint(e)),
        }
    }
}

/// 从入参里读出唯一的等待条件。多给了按 selector > gone > text > url >
/// idle 的顺序取第一个 —— 校验层已经保证至少有一个。
fn wait_condition(input: &serde_json::Value) -> Option<WaitCondition> {
    let s = |k: &str| {
        input
            .get(k)
            .and_then(serde_json::Value::as_str)
            .filter(|v| !v.is_empty())
            .map(ToOwned::to_owned)
    };
    if let Some(v) = s("selector") {
        return Some(WaitCondition::Selector(v));
    }
    if let Some(v) = s("selector_gone") {
        return Some(WaitCondition::SelectorGone(v));
    }
    if let Some(v) = s("text") {
        return Some(WaitCondition::Text(v));
    }
    if let Some(v) = s("url_contains") {
        return Some(WaitCondition::UrlContains(v));
    }
    if input.get("network_idle").and_then(serde_json::Value::as_bool) == Some(true) {
        return Some(WaitCondition::NetworkIdle);
    }
    None
}

// ── 悬停 ──────────────────────────────────────────────

pub struct BrowserHover;

#[async_trait::async_trait]
impl Tool for BrowserHover {
    fn name(&self) -> &'static str {
        "BrowserHover"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "把鼠标悬停到一个元素上（不点击）——展开悬停菜单、触发 tooltip、\
         让只在 hover 时出现的按钮显形。定位方式同 BrowserClick:\
         ref / selector / text 给一个。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(ClickInput)
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        match target_from_input(input) {
            Some(t) => format!("悬停到{}", t.describe()),
            None => "悬停到页面元素".to_owned(),
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        false
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        require_target(input)
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        ctx: &PermissionContext,
    ) -> PermissionResult {
        interact_consent(ctx)
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let Some(t) = target_from_input(&input) else {
            return ToolOutcome::failed("要悬停到哪个:给 ref、selector 或 text 之一。");
        };
        match ctx.browser.act(Action::Hover(t)).await {
            Ok(msg) => ToolOutcome::ok_text(msg),
            Err(e) => ToolOutcome::failed(interact_hint(e)),
        }
    }
}

// ── 下拉选择 ──────────────────────────────────────────

/// 下拉选择的入参:定位一个元素（通常是 `<select>`），设成某个值。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct SelectInput {
    #[serde(default)]
    r#ref: Option<u32>,
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    text: Option<String>,
    /// 要选的值（`<option>` 的 value 或文本）。
    value: String,
}

pub struct BrowserSelect;

#[async_trait::async_trait]
impl Tool for BrowserSelect {
    fn name(&self) -> &'static str {
        "BrowserSelect"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "给下拉框 `<select>`（或受控输入）设值并派发 change 事件。\
         定位方式同 BrowserClick（ref/selector/text），value 是要选的值。\
         普通文本框还是用 BrowserType。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(SelectInput)
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let value = input.get("value").and_then(|v| v.as_str()).unwrap_or("...");
        match target_from_input(input) {
            Some(t) => format!("把{}设为 {value:?}", t.describe()),
            None => format!("下拉选择 {value:?}"),
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        false
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        require_target(input)?;
        if input.get("value").and_then(|v| v.as_str()).is_none() {
            return Err(ValidationError::rejected("缺少要选的 value。"));
        }
        Ok(())
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        ctx: &PermissionContext,
    ) -> PermissionResult {
        interact_consent(ctx)
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let Some(t) = target_from_input(&input) else {
            return ToolOutcome::failed("要给哪个下拉框设值:给 ref、selector 或 text 之一。");
        };
        let value = input.get("value").and_then(|v| v.as_str()).unwrap_or_default().to_owned();
        match ctx.browser.act(Action::SelectOption { target: t, value }).await {
            Ok(msg) => ToolOutcome::ok_text(msg),
            Err(e) => ToolOutcome::failed(interact_hint(e)),
        }
    }
}

// ── 拖拽 ──────────────────────────────────────────────

/// 拖拽的入参:from/to 两端各用 `<端>_ref` / `<端>_selector` / `<端>_text`
/// 之一定位。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct DragInput {
    #[serde(default)]
    from_ref: Option<u32>,
    #[serde(default)]
    from_selector: Option<String>,
    #[serde(default)]
    from_text: Option<String>,
    #[serde(default)]
    to_ref: Option<u32>,
    #[serde(default)]
    to_selector: Option<String>,
    #[serde(default)]
    to_text: Option<String>,
}

pub struct BrowserDrag;

/// 从带前缀的字段里解析一端的定位目标。
fn prefixed_target(input: &serde_json::Value, prefix: &str) -> Option<Target> {
    if let Some(n) = input
        .get(format!("{prefix}_ref"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
    {
        return Some(Target::Ref(n));
    }
    if let Some(s) = input
        .get(format!("{prefix}_selector"))
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
    {
        return Some(Target::Selector(s.to_owned()));
    }
    if let Some(t) = input
        .get(format!("{prefix}_text"))
        .and_then(serde_json::Value::as_str)
        .filter(|t| !t.is_empty())
    {
        return Some(Target::Text(t.to_owned()));
    }
    None
}

#[async_trait::async_trait]
impl Tool for BrowserDrag {
    fn name(&self) -> &'static str {
        "BrowserDrag"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "把一个元素拖到另一个元素上 —— 看板卡片、排序列表、滑块。\
         两端各用 from_ref/from_selector/from_text 和 to_* 定位。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(DragInput)
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        match (prefixed_target(input, "from"), prefixed_target(input, "to")) {
            (Some(f), Some(t)) => format!("把{}拖到{}", f.describe(), t.describe()),
            _ => "拖拽元素".to_owned(),
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        false
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        if prefixed_target(input, "from").is_none() {
            return Err(ValidationError::rejected("缺少起点:给 from_ref/from_selector/from_text 之一。"));
        }
        if prefixed_target(input, "to").is_none() {
            return Err(ValidationError::rejected("缺少终点:给 to_ref/to_selector/to_text 之一。"));
        }
        Ok(())
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        ctx: &PermissionContext,
    ) -> PermissionResult {
        interact_consent(ctx)
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let (Some(from), Some(to)) =
            (prefixed_target(&input, "from"), prefixed_target(&input, "to"))
        else {
            return ToolOutcome::failed("拖拽要给起点和终点两端。");
        };
        match ctx.browser.act(Action::Drag { from, to }).await {
            Ok(msg) => ToolOutcome::ok_text(msg),
            Err(e) => ToolOutcome::failed(interact_hint(e)),
        }
    }
}

// ── 前进后退刷新 ──────────────────────────────────────

/// 历史导航的入参:direction 是 back / forward / reload。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct GoInput {
    /// back（后退）、forward（前进）、reload（刷新）。
    direction: String,
}

pub struct BrowserGo;

#[async_trait::async_trait]
impl Tool for BrowserGo {
    fn name(&self) -> &'static str {
        "BrowserGo"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "在浏览历史里走一步:direction 为 back（后退）、forward（前进）或 \
         reload（刷新当前页）。都是回到已经访问过的页面，不会到新站点。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(GoInput)
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        match input.get("direction").and_then(|v| v.as_str()).unwrap_or("") {
            "back" => "后退".to_owned(),
            "forward" => "前进".to_owned(),
            "reload" => "刷新页面".to_owned(),
            _ => "历史导航".to_owned(),
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        false
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        // 前进后退刷新都落在已经访问过的历史条目上，到不了新域名 ——
        // 首次访问时已经过了域名同意，这里不必再问。
        PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Preapproved {
                what: "在已访问的历史里导航".into(),
            },
        }
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        match input.get("direction").and_then(|v| v.as_str()) {
            Some("back" | "forward" | "reload") => Ok(()),
            _ => Err(ValidationError::rejected("direction 要是 back / forward / reload。")),
        }
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let nav = match input.get("direction").and_then(|v| v.as_str()) {
            Some("back") => Nav::Back,
            Some("forward") => Nav::Forward,
            Some("reload") => Nav::Reload,
            _ => return ToolOutcome::failed("direction 要是 back / forward / reload。"),
        };
        match ctx.browser.browse(nav).await {
            Ok(msg) => ToolOutcome::ok_text(msg),
            Err(e) => ToolOutcome::failed(interact_hint(e)),
        }
    }
}

// ── 标签页 ────────────────────────────────────────────

/// 标签页操作的入参。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct TabsInput {
    /// list（列出）、new（新开空白页）、select（切换）、close（关闭）。
    action: String,
    /// select / close 要操作的标签页号（来自 list 的输出）。
    #[serde(default)]
    id: Option<u32>,
}

pub struct BrowserTabs;

#[async_trait::async_trait]
impl Tool for BrowserTabs {
    fn name(&self) -> &'static str {
        "BrowserTabs"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "管理标签页:action 为 list（列出所有标签页和它们的号）、new（新开一个\
         空白页，之后用 BrowserNavigate 打开地址）、select（切到 id 那个）、\
         close（关掉 id 那个）。select/close 的 id 来自 list 的输出。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(TabsInput)
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let id = input.get("id").and_then(serde_json::Value::as_u64).unwrap_or(0);
        match input.get("action").and_then(|v| v.as_str()).unwrap_or("") {
            "list" => "列出标签页".to_owned(),
            "new" => "新开标签页".to_owned(),
            "select" => format!("切到标签页 [{id}]"),
            "close" => format!("关闭标签页 [{id}]"),
            _ => "标签页操作".to_owned(),
        }
    }

    fn is_read_only(&self, input: &serde_json::Value) -> bool {
        // 只有 list 是纯读；new/select/close 会改变浏览器状态。
        input.get("action").and_then(|v| v.as_str()) == Some("list")
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        // 开/切/关标签页不触及新域名（new 是空白页），和历史导航同级免确认。
        PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Preapproved {
                what: "管理浏览器标签页".into(),
            },
        }
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        match input.get("action").and_then(|v| v.as_str()) {
            Some("list" | "new") => Ok(()),
            Some("select" | "close") => {
                if tab_id(input).is_some() {
                    Ok(())
                } else {
                    Err(ValidationError::rejected("select / close 要给标签页号 id。"))
                }
            }
            _ => Err(ValidationError::rejected("action 要是 list / new / select / close。")),
        }
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let nav = match input.get("action").and_then(|v| v.as_str()) {
            Some("list") => Nav::ListTabs,
            Some("new") => Nav::NewTab,
            Some("select") => match tab_id(&input) {
                Some(id) => Nav::SelectTab(id),
                None => return ToolOutcome::failed("select 要给标签页号 id。"),
            },
            Some("close") => match tab_id(&input) {
                Some(id) => Nav::CloseTab(id),
                None => return ToolOutcome::failed("close 要给标签页号 id。"),
            },
            _ => return ToolOutcome::failed("action 要是 list / new / select / close。"),
        };
        match ctx.browser.browse(nav).await {
            Ok(msg) => ToolOutcome::ok_text(msg),
            Err(e) => ToolOutcome::failed(interact_hint(e)),
        }
    }
}

fn tab_id(input: &serde_json::Value) -> Option<u32> {
    input
        .get("id")
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
}

// ── 执行 JS ───────────────────────────────────────────

/// 执行脚本的入参。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct EvaluateInput {
    /// 要在页面里执行的 JS 表达式。支持 await。返回值会整形成文本。
    expression: String,
}

pub struct BrowserEvaluate;

#[async_trait::async_trait]
impl Tool for BrowserEvaluate {
    fn name(&self) -> &'static str {
        "BrowserEvaluate"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "在当前页面执行一段 JS 并拿回结果:读 DOM、读 localStorage、算个值、\
         调页面自己的函数。支持 await。`[NEVER]` 不要用它做导航（改地址栏\
         用 BrowserNavigate），也不要用它整段抓 DOM（用 BrowserSnapshot）——\
         结果超长会被截断。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(EvaluateInput)
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let e = input.get("expression").and_then(|v| v.as_str()).unwrap_or("...");
        let short: String = e.chars().take(50).collect();
        format!("执行 JS: {short}")
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        // JS 能改 DOM、发请求、写 storage —— 绝不是只读。
        false
    }

    fn classifier_input(&self, input: &serde_json::Value) -> Option<String> {
        // 任意 JS 是安全敏感的，喂给分类器过一道。
        input
            .get("expression")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned)
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        match input.get("expression").and_then(|v| v.as_str()) {
            Some(e) if !e.trim().is_empty() => Ok(()),
            _ => Err(ValidationError::rejected("缺少要执行的 expression。")),
        }
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        ctx: &PermissionContext,
    ) -> PermissionResult {
        // 高权限:任意 JS 能读会话、发请求。默认问一次（可"总是允许"），
        // 「全部放行」压得过它——bypass 的语义正是信任 agent 做开发。
        single_consent(ctx, self.name(), "在当前页面执行脚本")
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let expr = input.get("expression").and_then(|v| v.as_str()).unwrap_or_default();
        match ctx.browser.evaluate(expr).await {
            Ok(out) => ToolOutcome::ok_text(out),
            Err(e) => ToolOutcome::failed(interact_hint(e)),
        }
    }
}

// ── 文件上传 ──────────────────────────────────────────

/// 上传的入参:定位一个 `<input type=file>` + 本地文件路径列表。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct UploadInput {
    #[serde(default)]
    r#ref: Option<u32>,
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    text: Option<String>,
    /// 要上传的本地文件绝对路径（可多个）。
    paths: Vec<String>,
}

pub struct BrowserUpload;

#[async_trait::async_trait]
impl Tool for BrowserUpload {
    fn name(&self) -> &'static str {
        "BrowserUpload"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "给文件输入框设置要上传的本地文件（不弹系统选择框，直接塞进 input \
         并触发 change）。定位方式同 BrowserClick（ref/selector/text 指向那个 \
         `<input type=file>`），paths 是本地文件绝对路径列表。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(UploadInput)
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let n = input.get("paths").and_then(|v| v.as_array()).map_or(0, Vec::len);
        match target_from_input(input) {
            Some(t) => format!("给{}上传 {n} 个文件", t.describe()),
            None => format!("上传 {n} 个文件"),
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        false
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        require_target(input)?;
        match input.get("paths").and_then(|v| v.as_array()) {
            Some(a) if !a.is_empty() => Ok(()),
            _ => Err(ValidationError::rejected("缺少要上传的 paths（本地文件路径数组）。")),
        }
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        ctx: &PermissionContext,
    ) -> PermissionResult {
        // 上传把本地文件内容发到网页，比点击敏感 —— 单独问一次（可总是允许）。
        single_consent(ctx, self.name(), "把本地文件上传到当前页面")
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let Some(t) = target_from_input(&input) else {
            return ToolOutcome::failed("要上传到哪个输入框:给 ref、selector 或 text 之一。");
        };
        let paths: Vec<String> = input
            .get("paths")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(ToOwned::to_owned)).collect())
            .unwrap_or_default();
        match ctx.browser.upload(t, paths).await {
            Ok(msg) => ToolOutcome::ok_text(msg),
            Err(e) => ToolOutcome::failed(interact_hint(e)),
        }
    }
}

// ── Cookie ────────────────────────────────────────────

pub struct BrowserCookies;

#[async_trait::async_trait]
impl Tool for BrowserCookies {
    fn name(&self) -> &'static str {
        "BrowserCookies"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "读当前页面的 Cookie，含 HttpOnly / Secure / SameSite 等安全属性。\
         用于登录态分析、会话安全审计。要读 localStorage 用 BrowserEvaluate。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(NoInput)
    }

    fn describe(&self, _input: &serde_json::Value) -> String {
        "读当前页面的 Cookie".to_owned()
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        ctx: &PermissionContext,
    ) -> PermissionResult {
        // Cookie 里常有会话令牌，敏感。问一次（可"总是允许"）。
        single_consent(ctx, self.name(), "读取当前页面的 Cookie")
    }

    async fn call(&self, _input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        match ctx.browser.cookies().await {
            Ok(out) => ToolOutcome::ok_text(out),
            Err(e) => ToolOutcome::failed(interact_hint(e)),
        }
    }
}

// ── 网络观察（被动）───────────────────────────────────

/// 抓包的入参。action 决定看什么。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct NetworkInput {
    /// list（列请求）、detail（看某条细节）、audit（响应头安全审计）。默认 list。
    #[serde(default)]
    action: Option<String>,
    /// list 时按 URL 子串过滤。
    #[serde(default)]
    filter: Option<String>,
    /// detail 时要看的请求号（来自 list 输出行首的 #id）。
    #[serde(default)]
    request_id: Option<String>,
}

pub struct BrowserNetwork;

#[async_trait::async_trait]
impl Tool for BrowserNetwork {
    fn name(&self) -> &'static str {
        "BrowserNetwork"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "观察当前页面的网络流量（被动抓包）。action:list 列出请求（可 filter \
         按 URL 子串筛）、detail 看某条的请求/响应头和响应体（request_id 来自 \
         list 的 #号）、audit 审计主文档响应头的安全配置（CSP/HSTS/CORS 等）。\
         第一次调用只是开始累积 —— 要抓完整加载流量，先 list 一次再刷新页面。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(NetworkInput)
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        match input.get("action").and_then(|v| v.as_str()).unwrap_or("list") {
            "detail" => {
                let id = input.get("request_id").and_then(|v| v.as_str()).unwrap_or("?");
                format!("看请求 #{id} 的细节")
            }
            "audit" => "审计响应头安全配置".to_owned(),
            _ => "列出网络请求".to_owned(),
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        // 被动观察，不改页面也不改数据。
        true
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        // 被动抓包针对的是已经打开的页面 —— 能导航到这页就已经过了域名同意，
        // 看它自己的流量属于观察，不需要 scope（见计划的授权分层）。
        read_current_page()
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        match input.get("action").and_then(|v| v.as_str()) {
            None | Some("list" | "audit") => Ok(()),
            Some("detail") => {
                if input.get("request_id").and_then(|v| v.as_str()).is_some() {
                    Ok(())
                } else {
                    Err(ValidationError::rejected("detail 要给 request_id（来自 list 的 #号）。"))
                }
            }
            Some(_) => Err(ValidationError::rejected("action 要是 list / detail / audit。")),
        }
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let query = match input.get("action").and_then(|v| v.as_str()).unwrap_or("list") {
            "list" => NetQuery::List {
                filter: input.get("filter").and_then(|v| v.as_str()).map(ToOwned::to_owned),
            },
            "audit" => NetQuery::Audit,
            "detail" => match input.get("request_id").and_then(|v| v.as_str()) {
                Some(id) => NetQuery::Detail { request_id: id.to_owned() },
                None => return ToolOutcome::failed("detail 要给 request_id。"),
            },
            other => return ToolOutcome::failed(format!("不认识的 action: {other}")),
        };
        match ctx.browser.network(query).await {
            Ok(out) => ToolOutcome::ok_text(out),
            Err(e) => ToolOutcome::failed(interact_hint(e)),
        }
    }
}

// ── 重放（Repeater，主动，受 scope）───────────────────

/// 重放的入参。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct ReplayInput {
    /// 要重放的完整 URL（含协议）。它的 host 必须在渗透 scope 内。
    url: String,
    /// HTTP 方法。默认 GET。
    #[serde(default)]
    method: Option<String>,
    /// 请求头，键值对对象。
    #[serde(default)]
    headers: Option<serde_json::Value>,
    /// 请求体（POST/PUT 用）。
    #[serde(default)]
    body: Option<String>,
}

pub struct BrowserReplay;

#[async_trait::async_trait]
impl Tool for BrowserReplay {
    fn name(&self) -> &'static str {
        "BrowserReplay"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "重放一个请求并看响应（Repeater）:改参数、改头、改 body 重发，对比\
         差异。在页面上下文里发，自动带当前会话 cookie —— 用已登录身份测试\
         越权、注入、逻辑漏洞。仅限已授权的渗透 scope 内目标。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(ReplayInput)
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let m = input.get("method").and_then(|v| v.as_str()).unwrap_or("GET");
        let u = input.get("url").and_then(|v| v.as_str()).unwrap_or("...");
        format!("重放 {m} {u}")
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        false
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        match input.get("url").and_then(|v| v.as_str()) {
            Some(u) if target_host(u).is_some() => Ok(()),
            _ => Err(ValidationError::rejected("url 要是含协议的完整地址。")),
        }
    }

    fn check_permissions(
        &self,
        input: &serde_json::Value,
        ctx: &PermissionContext,
    ) -> PermissionResult {
        pentest_gate(input.get("url").and_then(|v| v.as_str()), ctx)
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let url = input.get("url").and_then(|v| v.as_str()).unwrap_or_default().to_owned();
        let method = input
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET")
            .to_uppercase();
        let headers = input.get("headers").cloned().unwrap_or(serde_json::Value::Null);
        let body = input.get("body").and_then(|v| v.as_str()).map(ToOwned::to_owned);
        match ctx.browser.replay(&url, &method, headers, body).await {
            Ok(out) => ToolOutcome::ok_text(out),
            Err(e) => ToolOutcome::failed(interact_hint(e)),
        }
    }
}

// ── 拦截/改包（主动，受 scope）────────────────────────

/// 拦截的入参。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct InterceptInput {
    /// block（阻断）、fulfill（伪造响应）、list（列规则）、clear（清空）。
    action: String,
    /// 授权目标的 host（必须在 scope 内）。block/fulfill 必填。
    #[serde(default)]
    host: Option<String>,
    /// 匹配请求 URL 的子串。block/fulfill 必填。
    #[serde(default)]
    url_pattern: Option<String>,
    /// fulfill 时伪造的状态码。默认 200。
    #[serde(default)]
    status: Option<u32>,
    /// fulfill 时伪造的响应体。
    #[serde(default)]
    body: Option<String>,
}

pub struct BrowserIntercept;

#[async_trait::async_trait]
impl Tool for BrowserIntercept {
    fn name(&self) -> &'static str {
        "BrowserIntercept"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "拦截/改包:action 为 block（阻断匹配 url_pattern 的请求）、fulfill\
         （用 status+body 伪造响应，测前端如何处理错误/异常数据）、list、\
         clear。block/fulfill 要给 host（授权目标，须在 scope 内）和 \
         url_pattern。仅限已授权的渗透 scope 内目标。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(InterceptInput)
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let p = input.get("url_pattern").and_then(|v| v.as_str()).unwrap_or("");
        match input.get("action").and_then(|v| v.as_str()).unwrap_or("") {
            "block" => format!("拦截含 `{p}` 的请求"),
            "fulfill" => format!("伪造 `{p}` 的响应"),
            "list" => "列出拦截规则".to_owned(),
            "clear" => "清空拦截规则".to_owned(),
            _ => "拦截设置".to_owned(),
        }
    }

    fn is_read_only(&self, input: &serde_json::Value) -> bool {
        input.get("action").and_then(|v| v.as_str()) == Some("list")
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        match input.get("action").and_then(|v| v.as_str()) {
            Some("list" | "clear") => Ok(()),
            Some("block" | "fulfill") => {
                if input.get("host").and_then(|v| v.as_str()).and_then(target_host).is_none() {
                    return Err(ValidationError::rejected("block/fulfill 要给 host（授权目标）。"));
                }
                if input.get("url_pattern").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).is_none() {
                    return Err(ValidationError::rejected("block/fulfill 要给 url_pattern。"));
                }
                Ok(())
            }
            _ => Err(ValidationError::rejected("action 要是 block / fulfill / list / clear。")),
        }
    }

    fn check_permissions(
        &self,
        input: &serde_json::Value,
        ctx: &PermissionContext,
    ) -> PermissionResult {
        // list/clear 不打目标，免确认；block/fulfill 按 host 走 scope。
        match input.get("action").and_then(|v| v.as_str()) {
            Some("list" | "clear") => read_current_page(),
            _ => pentest_gate(input.get("host").and_then(|v| v.as_str()), ctx),
        }
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or_default();
        let pattern = input.get("url_pattern").and_then(|v| v.as_str()).unwrap_or_default().to_owned();
        let op = match action {
            "list" => InterceptOp::List,
            "clear" => InterceptOp::Clear,
            "block" => InterceptOp::Block { url_pattern: pattern },
            "fulfill" => InterceptOp::Fulfill {
                url_pattern: pattern,
                status: input.get("status").and_then(serde_json::Value::as_u64).unwrap_or(200) as u32,
                body: input.get("body").and_then(|v| v.as_str()).unwrap_or_default().to_owned(),
            },
            other => return ToolOutcome::failed(format!("不认识的 action: {other}")),
        };
        match ctx.browser.intercept(op).await {
            Ok(out) => ToolOutcome::ok_text(out),
            Err(e) => ToolOutcome::failed(interact_hint(e)),
        }
    }
}

// ── 密钥泄露扫描（被动）───────────────────────────────

pub struct BrowserSecrets;

#[async_trait::async_trait]
impl Tool for BrowserSecrets {
    fn name(&self) -> &'static str {
        "BrowserSecrets"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "扫当前页面的 HTML 和内联脚本里有没有泄露的密钥/凭证（AWS/Google/\
         GitHub token、JWT、私钥、api_key 赋值等）。命中的值会打码。\
         被动扫描，只看已经打开的这一页。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(NoInput)
    }

    fn describe(&self, _input: &serde_json::Value) -> String {
        "扫描页面里的密钥泄露".to_owned()
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        read_current_page()
    }

    async fn call(&self, _input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        // 取整页 HTML（含内联脚本）。外链 JS 不在这里 —— 那要另外抓，
        // 先覆盖最常见的"密钥硬编码在页面/内联脚本里"。
        let html = match ctx.browser.evaluate("document.documentElement.outerHTML").await {
            Ok(h) => h,
            Err(e) => return ToolOutcome::failed(interact_hint(e)),
        };
        let found = super::pentest::scan_secrets(&html);
        if found.is_empty() {
            ToolOutcome::ok_text("没扫到明显的密钥泄露。")
        } else {
            ToolOutcome::ok_text(format!("扫到疑似密钥泄露:\n{}", found.join("\n")))
        }
    }
}

// ── 接口/表单发现（被动）──────────────────────────────

pub struct BrowserDiscover;

#[async_trait::async_trait]
impl Tool for BrowserDiscover {
    fn name(&self) -> &'static str {
        "BrowserDiscover"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "枚举当前页面的攻击面:表单（action/method/字段）和页面内的链接。\
         配合 BrowserNetwork 能拼出接口清单，是渗透侦察的起点。被动，只看\
         当前页。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(NoInput)
    }

    fn describe(&self, _input: &serde_json::Value) -> String {
        "枚举页面的表单和链接".to_owned()
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        read_current_page()
    }

    async fn call(&self, _input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        // 页面里一趟取回表单和链接的结构，Rust 侧只做整形。
        let js = "JSON.stringify({ \
            forms: [...document.forms].map(f => ({ \
                action: f.action, method: (f.method||'get').toUpperCase(), \
                fields: [...f.elements].map(e => e.name).filter(Boolean) })), \
            links: [...new Set([...document.querySelectorAll('a[href]')].map(a => a.href))].slice(0, 100) \
        })";
        let raw = match ctx.browser.evaluate(js).await {
            Ok(r) => r,
            Err(e) => return ToolOutcome::failed(interact_hint(e)),
        };
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
        let mut out = String::new();

        let forms = parsed["forms"].as_array().cloned().unwrap_or_default();
        out.push_str(&format!("表单（{}）:\n", forms.len()));
        for f in &forms {
            let fields = f["fields"].as_array().map(|a| {
                a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", ")
            }).unwrap_or_default();
            out.push_str(&format!(
                "- {} {}  字段: [{fields}]\n",
                f["method"].as_str().unwrap_or("GET"),
                f["action"].as_str().unwrap_or("")
            ));
        }

        let links = parsed["links"].as_array().cloned().unwrap_or_default();
        out.push_str(&format!("\n链接（{}，最多 100）:\n", links.len()));
        for l in links.iter().filter_map(|v| v.as_str()) {
            out.push_str(&format!("- {l}\n"));
        }
        ToolOutcome::ok_text(out)
    }
}

// ── 参数 fuzzing（主动，受 scope）─────────────────────

/// fuzz 的入参。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct FuzzInput {
    /// 目标 URL，用字面量 `FUZZ` 标记要注入的位置，如
    /// `https://api.test/search?q=FUZZ`。host 必须在渗透 scope 内。
    url: String,
    /// HTTP 方法。默认 GET。
    #[serde(default)]
    method: Option<String>,
    /// 自定义 payload 列表。省略则用内置的 XSS/SQLi/遍历/重定向探针集。
    #[serde(default)]
    payloads: Option<Vec<String>>,
}

pub struct BrowserFuzz;

/// 把 FUZZ 占位符替换成给定值。
fn fuzz_url(url: &str, payload: &str) -> String {
    url.replace("FUZZ", payload)
}

/// fuzz 目标的 host:把 FUZZ 换成无害串后取 host（用于 scope 判定）。
fn fuzz_host(url: &str) -> Option<String> {
    target_host(&fuzz_url(url, "x"))
}

#[async_trait::async_trait]
impl Tool for BrowserFuzz {
    fn name(&self) -> &'static str {
        "BrowserFuzz"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "对一个参数灌 payload，看响应差异 —— 反射型 XSS、SQL 注入、状态\
         异常都在这里冒头。url 里用 FUZZ 标记注入点（如 \
         `https://api.test/s?q=FUZZ`），payloads 省略则用内置探针集。\
         逐个请求带当前会话发出，分析反射/报错/状态变化。仅限已授权 scope。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(FuzzInput)
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let u = input.get("url").and_then(|v| v.as_str()).unwrap_or("...");
        format!("fuzz {u}")
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        false
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        match input.get("url").and_then(|v| v.as_str()) {
            Some(u) if u.contains("FUZZ") && fuzz_host(u).is_some() => Ok(()),
            Some(u) if fuzz_host(u).is_none() => {
                Err(ValidationError::rejected("url 要是含协议的完整地址。"))
            }
            _ => Err(ValidationError::rejected("url 里要有 FUZZ 占位符标记注入点。")),
        }
    }

    fn check_permissions(
        &self,
        input: &serde_json::Value,
        ctx: &PermissionContext,
    ) -> PermissionResult {
        let host = input.get("url").and_then(|v| v.as_str()).and_then(fuzz_host);
        // fuzz_host 已经把 host 提出来了;pentest_permission 直接判。
        match host {
            Some(h) => pentest_permission(&h, ctx),
            None => PermissionResult::Passthrough,
        }
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let url = input.get("url").and_then(|v| v.as_str()).unwrap_or_default().to_owned();
        let method = input
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET")
            .to_uppercase();

        // 基线:用无害串探一次，作为对比。
        let baseline = match ctx
            .browser
            .replay(&fuzz_url(&url, "riotbaseline"), &method, serde_json::Value::Null, None)
            .await
        {
            Ok(b) => b,
            Err(e) => return ToolOutcome::failed(interact_hint(e)),
        };

        // payload 集:自定义优先，否则内置。
        let custom = input
            .get("payloads")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(ToOwned::to_owned)).collect::<Vec<_>>());
        let jobs: Vec<(String, String)> = match &custom {
            Some(list) => list.iter().map(|p| (p.clone(), "自定义".to_owned())).collect(),
            None => super::pentest::default_payloads()
                .into_iter()
                .map(|p| (p.value.to_owned(), p.intent.to_owned()))
                .collect(),
        };

        let mut report = String::from("fuzz 结果:\n");
        let mut any = false;
        for (payload, intent) in jobs {
            // 取消要及时响应 —— fuzz 是一串请求，用户按停不该还在打。
            if ctx.cancel.is_cancelled() {
                report.push_str("（已取消）\n");
                break;
            }
            let resp = match ctx
                .browser
                .replay(&fuzz_url(&url, &payload), &method, serde_json::Value::Null, None)
                .await
            {
                Ok(r) => r,
                Err(e) => return ToolOutcome::failed(interact_hint(e)),
            };
            let signals = super::pentest::analyze_fuzz(&payload, &baseline, &resp);
            if !signals.is_empty() {
                any = true;
                report.push_str(&format!("\n[{intent}] payload {payload:?}\n"));
                for s in signals {
                    report.push_str(&format!("  ⚠ {s}\n"));
                }
            }
        }
        if !any {
            report.push_str("所有 payload 都没看出异常反应。\n");
        }
        ToolOutcome::ok_text(report)
    }
}

// ── 发现报告 ──────────────────────────────────────────

/// 报告的入参。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct ReportInput {
    /// 目标（域名/URL/系统名），写进报告抬头。
    #[serde(default)]
    target: Option<String>,
    /// 发现列表。每条:title、severity（critical/high/medium/low/info）、
    /// evidence（证据/PoC）、remediation（修复建议）。
    findings: Vec<serde_json::Value>,
}

pub struct BrowserReport;

#[async_trait::async_trait]
impl Tool for BrowserReport {
    fn name(&self) -> &'static str {
        "BrowserReport"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "把渗透过程中攒下的发现整理成结构化 Markdown 报告并存成文件。\
         findings 每条给 title、severity（critical/high/medium/low/info）、\
         evidence、remediation。按严重度排序，返回报告全文和存盘路径。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(ReportInput)
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let n = input.get("findings").and_then(|v| v.as_array()).map_or(0, Vec::len);
        format!("生成渗透报告（{n} 条发现）")
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        // 只往会话工件目录写一个报告文件，不碰工作区、不触网。
        true
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        // 只写会话自己的工件目录（和截图落盘同类），不需要问。
        PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Preapproved {
                what: "把发现写成报告文件".into(),
            },
        }
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        match input.get("findings") {
            Some(serde_json::Value::Array(_)) => Ok(()),
            _ => Err(ValidationError::rejected("findings 要是发现对象的数组。")),
        }
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let target = input.get("target").and_then(|v| v.as_str()).unwrap_or_default();
        let findings = input
            .get("findings")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let report = super::pentest::format_report(target, &findings);

        // 落盘到会话工件目录，给用户一个能直接交付的文件。写不进不算失败 ——
        // 报告全文照样在结果里（和截图落盘的降级同理）。
        let path = ctx.artifacts_dir.join(format!("report-{}.md", ctx.tool_use_id.as_str()));
        let saved = ctx.fs.write(&path, report.as_bytes()).await.is_ok();

        if saved {
            ToolOutcome::ok_text(format!("报告已存到 {}\n\n{report}", path.display()))
        } else {
            ToolOutcome::ok_text(report)
        }
    }
}

// ── 爬虫/站点地图（主动，受 scope）────────────────────

/// 爬虫的入参。
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct CrawlInput {
    /// 起点 URL（含协议）。只爬**同 host** 的页面，host 须在 scope 内。
    url: String,
    /// 最多爬几页。默认 10，上限 30 —— 爬虫会实际逐页导航，不设上限会失控。
    #[serde(default)]
    max_pages: Option<usize>,
}

pub struct BrowserCrawl;

/// 一次爬取最多访问多少页的硬上限。
const CRAWL_CAP: usize = 30;

#[async_trait::async_trait]
impl Tool for BrowserCrawl {
    fn name(&self) -> &'static str {
        "BrowserCrawl"
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "从起点 URL 开始爬同一站点，生成站点地图:每页的标题、表单数、链接数。\
         渗透侦察用 —— 摸清攻击面。会实际逐页导航（默认最多 10 页），只跟同 \
         host 的链接。仅限已授权的渗透 scope 内目标。"
            .to_owned()
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(CrawlInput)
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let u = input.get("url").and_then(|v| v.as_str()).unwrap_or("...");
        format!("爬取 {}", u.chars().take(60).collect::<String>())
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        false
    }

    fn interrupt_behavior(&self) -> InterruptBehavior {
        InterruptBehavior::Cancel
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        match input.get("url").and_then(|v| v.as_str()) {
            Some(u) if target_host(u).is_some() => Ok(()),
            _ => Err(ValidationError::rejected("url 要是含协议的完整地址。")),
        }
    }

    fn check_permissions(
        &self,
        input: &serde_json::Value,
        ctx: &PermissionContext,
    ) -> PermissionResult {
        pentest_gate(input.get("url").and_then(|v| v.as_str()), ctx)
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let start = input.get("url").and_then(|v| v.as_str()).unwrap_or_default().to_owned();
        let Some(host) = target_host(&start) else {
            return ToolOutcome::failed("url 要是含协议的完整地址。");
        };
        let max = input
            .get("max_pages")
            .and_then(serde_json::Value::as_u64)
            .map_or(10, |n| n as usize)
            .clamp(1, CRAWL_CAP);

        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut queue: Vec<String> = vec![start];
        let mut sitemap = String::new();
        let mut pages = 0;

        while let Some(u) = queue.pop() {
            if pages >= max || ctx.cancel.is_cancelled() {
                break;
            }
            if visited.contains(&u) {
                continue;
            }
            visited.insert(u.clone());
            pages += 1;

            if let Err(e) = ctx.browser.navigate(&u).await {
                sitemap.push_str(&format!("- {u}  (打不开:{e})\n"));
                continue;
            }
            // 一趟取回标题、表单数、链接（绝对 URL）。
            let js = "JSON.stringify({ \
                title: document.title, \
                forms: document.forms.length, \
                links: [...new Set([...document.querySelectorAll('a[href]')].map(a => a.href))] })";
            let raw = match ctx.browser.evaluate(js).await {
                Ok(r) => r,
                Err(e) => return ToolOutcome::failed(interact_hint(e)),
            };
            let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
            let title = parsed["title"].as_str().unwrap_or("");
            let forms = parsed["forms"].as_u64().unwrap_or(0);
            let links: Vec<String> = parsed["links"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(ToOwned::to_owned)).collect())
                .unwrap_or_default();
            sitemap.push_str(&format!(
                "- {u}  「{title}」(表单 {forms}, 链接 {})\n",
                links.len()
            ));
            for next in super::pentest::crawl_next(&links, &host, &visited) {
                if !queue.contains(&next) {
                    queue.push(next);
                }
            }
        }

        let head = if ctx.cancel.is_cancelled() {
            format!("爬取被取消，已访问 {pages} 页:\n")
        } else {
            format!("站点地图（{host}，访问了 {pages} 页）:\n")
        };
        ToolOutcome::ok_text(format!("{head}{sitemap}"))
    }
}

// ── 共用 ──────────────────────────────────────────────

/// 会问一次权限的交互工具。滚动不在里面（它免确认），所以"总是允许"
/// 的建议也只覆盖这三个。
const INTERACT_TOOLS: [&str; 3] = ["BrowserClick", "BrowserType", "BrowserKey"];

/// 从入参里解析出定位目标（ref > selector > text）。三个都没有是 `None`。
fn target_from_input(input: &serde_json::Value) -> Option<Target> {
    if let Some(n) = input
        .get("ref")
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
    {
        return Some(Target::Ref(n));
    }
    if let Some(s) = input
        .get("selector")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
    {
        return Some(Target::Selector(s.to_owned()));
    }
    if let Some(t) = input
        .get("text")
        .and_then(serde_json::Value::as_str)
        .filter(|t| !t.is_empty())
    {
        return Some(Target::Text(t.to_owned()));
    }
    None
}

fn require_target(input: &serde_json::Value) -> Result<(), ValidationError> {
    if target_from_input(input).is_some() {
        return Ok(());
    }
    Err(ValidationError::rejected(
        "要指定元素:给 ref（BrowserSnapshot 行首的 [n]）、selector 或 text 之一。",
    ))
}

/// 交互工具的权限:默认问一次，用户点"总是允许"后整个会话不再问。
///
/// `[约束]` 建议要把三个交互工具**一起**记住。分开记的话，点击、输入、
/// 按键各弹一次窗 —— 三连问之后用户学到的是无脑点允许，那比一次授权
/// 三个工具危险得多。前端的"总是允许"会把整组建议一并带回。
///
/// `[约束]` 理由必须是 `Consent`:这是"没有规则命中时的例行询问"，
/// 「全部放行」模式该压得过它。写成别的理由会让放行模式下还在弹窗
/// （见决策链第 3/5 步和 consent.rs 踩过的同一个坑）。
///
/// 规划模式下返回 Passthrough 交给决策链兜底 —— 那边对非只读工具的
/// 判定是**拒绝**。在这儿抢答成 Ask，等于把"不想让它动手"改成了
/// "问问看"，门反而开大了。
fn interact_consent(ctx: &PermissionContext) -> PermissionResult {
    if ctx.mode.get() == PermissionMode::Plan {
        return PermissionResult::Passthrough;
    }
    PermissionResult::Ask {
        message: "是否允许模型操作当前页面（点击、输入、按键）？".into(),
        suggestions: INTERACT_TOOLS
            .iter()
            .map(|t| PermissionUpdate::AddRule {
                tool: (*t).to_owned(),
                pattern: None,
                decision: RuleDecision::Allow,
                scope: UpdateScope::Session,
            })
            .collect(),
        reason: DecisionReason::Consent {
            what: "操作当前页面".into(),
        },
    }
}

/// 单个工具的"问一次、可总是允许"。用于 evaluate、cookies 这类强力但
/// 各自独立的工具（不像点击/输入/按键那样成组）。
///
/// `[约束]` 理由必须是 `Consent`、规划模式 Passthrough —— 和 [`interact_consent`]
/// 同一套道理:放行模式该压得过它，规划模式交给决策链拒绝而不是在这儿抢答。
fn single_consent(ctx: &PermissionContext, tool: &str, what: &str) -> PermissionResult {
    if ctx.mode.get() == PermissionMode::Plan {
        return PermissionResult::Passthrough;
    }
    PermissionResult::Ask {
        message: format!("是否允许{what}？"),
        suggestions: vec![PermissionUpdate::AddRule {
            tool: tool.to_owned(),
            pattern: None,
            decision: RuleDecision::Allow,
            scope: UpdateScope::Session,
        }],
        reason: DecisionReason::Consent { what: what.to_owned() },
    }
}

// ── 授权 scope（渗透安全骨架）─────────────────────────
//
// `[约束]` 主动/侵入性渗透动作（改包、重放、fuzzing、爬虫）只能打到用户
// **显式授权**的目标。授权表达为会话级规则 `Pentest(scope:<host>)=allow`，
// 由用户在弹窗里"总是允许"加入 —— 模型不能自行扩 scope。
//
// scope 判定挂在一个统一的伪工具名下（不是每个渗透工具各一套），这样
// 授权一次 example.com，重放/fuzz/爬虫全都命中同一个 scope。

/// scope 规则挂靠的伪工具名。所有主动渗透工具查同一个，实现"授权一次、
/// 全体命中"。
const SCOPE_TOOL: &str = "Pentest";

/// 从一个 URL 或裸域名里取出 host（小写）。取不出返回 `None`。
fn target_host(url_or_host: &str) -> Option<String> {
    // 走浏览器那条校验：localhost / 内网 IP 也要能取出 host，否则对本地
    // 目标做渗透时会在「URL 无法解析」处误报，而不是走 scope 授权。
    if let Ok(u) = weburl::normalize_for_browser(url_or_host) {
        return u.host_str().map(str::to_ascii_lowercase);
    }
    // 裸域名:normalize 要求带协议，这里兜一下 host[:port] 形式。
    let h = url_or_host.trim();
    let host = h.split('/').next().unwrap_or(h).split(':').next().unwrap_or(h);
    looks_like_host(host).then(|| host.to_ascii_lowercase())
}

fn looks_like_host(host: &str) -> bool {
    !host.is_empty() && (host.contains('.') || host.eq_ignore_ascii_case("localhost"))
}

/// 渗透工具的权限入口:从输入里的 URL/host 取出 host 再判定。
///
/// 解析不出 host（缺字段、格式不对）时返回 Passthrough —— 让 validate_input
/// 去报"参数不对"，而不是在这儿说"没权限"（模型会去要权限而不是修参数）。
fn pentest_gate(url_or_host: Option<&str>, ctx: &PermissionContext) -> PermissionResult {
    let Some(host) = url_or_host.and_then(target_host) else {
        return PermissionResult::Passthrough;
    };
    pentest_permission(&host, ctx)
}

/// 渗透工具的 scope 判定:目标在 scope 内放行，否则要用户先授权。
///
/// `[约束]` 规划模式返回 Passthrough，交给决策链按写操作**拒绝** ——
/// 和交互工具同理，不在这儿抢答成"问问看"。
fn pentest_permission(host: &str, ctx: &PermissionContext) -> PermissionResult {
    if ctx.mode.get() == PermissionMode::Plan {
        return PermissionResult::Passthrough;
    }
    scope_gate(host, ctx)
}

/// scope 判定本体。deny 规则 > 已授权 allow > 未授权（要求授权）。
///
/// `[约束]` 未授权时理由是 `SafetyCheck`（不是 `Consent`）—— 这让它**对
/// bypass 免疫**:「全部放行」信任的是"agent 做常规开发"，不含"对任意目标
/// 发起攻击"。scope 外的动作哪怕开着 bypass 也要用户点头，只有无人值守
/// 模式（明示交出一切）才放行。见 [`SafetyKind::OutOfScope`]。
fn scope_gate(host: &str, ctx: &PermissionContext) -> PermissionResult {
    let content = format!("scope:{host}");
    let rules = RuleSet::new(ctx.rules.clone());
    for want in [RuleDecision::Deny, RuleDecision::Ask, RuleDecision::Allow] {
        let Some(r) = rules.content_rule(SCOPE_TOOL, &content, want, MatchMode::Raw) else {
            continue;
        };
        let reason = DecisionReason::Rule {
            source: r.source,
            pattern: r.pattern.clone().unwrap_or_default(),
        };
        return match want {
            RuleDecision::Deny => PermissionResult::Deny {
                message: format!("{host} 被规则排除在渗透范围外。"),
                reason,
            },
            RuleDecision::Ask => PermissionResult::Ask {
                message: format!("是否对 {host} 执行这次渗透动作？"),
                suggestions: vec![scope_suggestion(&content)],
                reason,
            },
            RuleDecision::Allow => PermissionResult::Allow {
                updated_input: None,
                reason,
            },
        };
    }
    // 不在 scope 内:必须先授权。
    PermissionResult::Ask {
        message: format!(
            "目标 {host} 不在本次会话的渗透授权范围内。\
             只在你有权测试的目标上继续 —— 是否授权对 {host} 进行渗透测试？"
        ),
        suggestions: vec![scope_suggestion(&content)],
        reason: DecisionReason::SafetyCheck {
            safety: SafetyKind::OutOfScope,
        },
    }
}

/// "把这个目标加入本次会话渗透 scope"的规则建议。
fn scope_suggestion(content: &str) -> PermissionUpdate {
    PermissionUpdate::AddRule {
        tool: SCOPE_TOOL.to_owned(),
        pattern: Some(content.to_owned()),
        decision: RuleDecision::Allow,
        scope: UpdateScope::Session,
    }
}

/// 交互失败时给模型的话。两类失败的指引相反，见 [`InteractError`]。
fn interact_hint(e: InteractError) -> String {
    match e {
        InteractError::Unavailable(u) => unavailable_hint(&u),
        // 宿主已经把"下一步怎么办"（重新快照）写进消息里了。
        InteractError::Target(msg) => msg,
    }
}

/// 读当前页面的工具一律放行。
///
/// `[约束]` 这几个工具**不再单独征求同意**，因为进到这个页面时已经问过了。
///
/// 反过来做的代价很具体:模型改一次样式要截三次图，每次都弹窗的话，用户
/// 两分钟内就会去开「全部放行」—— 那比这里放行危险得多。真正的边界在
/// 导航那一步，守住它就够了。
fn read_current_page() -> PermissionResult {
    PermissionResult::Allow {
        updated_input: None,
        reason: DecisionReason::Preapproved {
            what: "读取当前已打开的页面".into(),
        },
    }
}

/// 浏览器用不了时给模型的话。
///
/// 要说清"这不是你的错，也不是重试能解决的" —— 否则模型会把同一个调用
/// 换个参数再来一遍，白烧几轮。
fn unavailable_hint(e: &BrowserUnavailable) -> String {
    format!("{e}\n浏览器不可用时不要重试，改用 WebFetch 读页面源码，或者请用户检查。")
}

/// 注册用。和 `builtin()` 分开是因为浏览器工具依赖宿主装配 —— 没装
/// 浏览器的构建（比如未来的 CLI）不该看到这几个工具。
///
/// 交互工具排在观察工具后面（prompt cache 的前缀顺序，见 builtin()），
/// 也暗合使用顺序:先看清页面，再动手。
pub fn tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(BrowserNavigate),
        Arc::new(BrowserSnapshot),
        Arc::new(BrowserScreenshot),
        Arc::new(BrowserView),
        Arc::new(BrowserConsole),
        Arc::new(BrowserClick),
        Arc::new(BrowserType),
        Arc::new(BrowserKey),
        Arc::new(BrowserScroll),
        Arc::new(BrowserWaitFor),
        Arc::new(BrowserHover),
        Arc::new(BrowserSelect),
        Arc::new(BrowserDrag),
        Arc::new(BrowserGo),
        Arc::new(BrowserTabs),
        Arc::new(BrowserEvaluate),
        Arc::new(BrowserUpload),
        Arc::new(BrowserCookies),
        Arc::new(BrowserNetwork),
        Arc::new(BrowserReplay),
        Arc::new(BrowserIntercept),
        Arc::new(BrowserSecrets),
        Arc::new(BrowserDiscover),
        Arc::new(BrowserFuzz),
        Arc::new(BrowserCrawl),
        Arc::new(BrowserReport),
    ]
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use riot_protocol::id::{SessionId, ToolUseId};
    use riot_protocol::tool::ProgressSink;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::testing::{
        FakeBrowser, FakeVision, FixedClock, NullFileState, NullFs, NullProc,
    };

    fn ctx(shot: &str, vision: FakeVision) -> ToolContext {
        // NullFs 写不进任何东西 —— 顺便钉住"落盘失败只是少了路径，
        // 工具照常出结果"的降级行为。
        ctx_fs(shot, vision, Arc::new(NullFs))
    }

    fn ctx_fs(
        shot: &str,
        vision: FakeVision,
        fs: Arc<dyn riot_protocol::tool::FileSystem>,
    ) -> ToolContext {
        let browser = Arc::new(FakeBrowser {
            shot: shot.into(),
            ..FakeBrowser::default()
        });
        ctx_browser(browser, vision, fs)
    }

    fn ctx_browser(
        browser: Arc<FakeBrowser>,
        vision: FakeVision,
        fs: Arc<dyn riot_protocol::tool::FileSystem>,
    ) -> ToolContext {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let id = ToolUseId::from_raw("t1");
        ToolContext {
            session_id: SessionId::from_raw("s1"),
            tool_use_id: id.clone(),
            cwd: "/work".into(),
            artifacts_dir: "/artifacts".into(),
            cancel: CancellationToken::new(),
            progress: ProgressSink::new(id, tx),
            file_state: Arc::new(NullFileState),
            fs,
            proc: Arc::new(NullProc),
            web: Arc::new(riot_protocol::web::NoWeb),
            browser,
            terminal: Arc::new(riot_protocol::terminal::NoTerminal),
            vision: Arc::new(vision),
            clock: Arc::new(FixedClock::default()),
        }
    }

    /// 模型自己能看图时，图片原样交给它。
    #[tokio::test]
    async fn 能看图的模型直接拿到图片() {
        let out = BrowserScreenshot
            .call(serde_json::json!({}), ctx("SHOT", FakeVision::Direct))
            .await;
        let ToolOutcome::Ok { model_content, .. } = out else {
            panic!("应当成功：{out:?}");
        };
        let ToolResultContent::Image { media_type, data, path } = model_content else {
            panic!("应当是图片内容块");
        };
        assert_eq!(data, "SHOT");
        assert_eq!(media_type, riot_protocol::browser::SHOT_MEDIA_TYPE);
        assert!(path.is_none(), "写不进盘就不带路径，而不是报错：{path:?}");
    }

    /// 看不了图的模型走视觉兼容，拿到的是转述文字；图片本体留给界面。
    ///
    /// `[约束]` 这条路必须**不返回纯图片**。返回了的话图片会在 provider 那层
    /// 被替换成一句话，模型只知道"有张图"，然后它会自己想办法 —— 去 shell 里
    /// 截整个屏幕。真实发生过。
    ///
    /// `[约束]` 图片也不能丢。DescribedImage 的 data 是界面上贴出来的那张图
    /// —— 只给文字的话，用户在工具卡片里看到的是一段写给模型的转述。
    #[tokio::test]
    async fn 看不了图的模型拿到转述文字() {
        let out = BrowserScreenshot
            .call(
                serde_json::json!({}),
                ctx("SHOT", FakeVision::Describe("{\"layout\":\"两栏\"}".into())),
            )
            .await;
        let ToolOutcome::Ok { model_content, .. } = out else {
            panic!("应当成功：{out:?}");
        };
        let ToolResultContent::DescribedImage { media_type, data, text, .. } = model_content
        else {
            panic!("应当是带转述的图片：{model_content:?}");
        };
        assert!(text.contains("两栏"), "要带上转述内容：{text}");
        assert_eq!(data, "SHOT", "图片本体要留给界面显示");
        assert_eq!(media_type, riot_protocol::browser::SHOT_MEDIA_TYPE);
    }

    /// 真实图片:原图落盘给界面，消息里只进压缩图。
    ///
    /// `[约束]` 这是这条链路的经济模型 —— 一张整页截图的 base64 有几 MB，
    /// 原样进消息的话每次切会话都要整个搬一遍，发给模型还按原始分辨率
    /// 计 token。原图必须落盘，消息里的 data 必须是压过的。
    #[tokio::test]
    async fn 原图落盘且给模型的是压缩图() {
        use base64::Engine as _;

        // 1280×4000:整页截图的典型比例，远超给模型的像素上限。
        let img = image::RgbaImage::from_pixel(1280, 4000, image::Rgba([10, 20, 30, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("编码 PNG");
        let raw = png.into_inner();
        let full = base64::engine::general_purpose::STANDARD.encode(&raw);

        let fs = Arc::new(crate::tools::memfs::MemFs::new().with_dir("/artifacts"));
        let out = BrowserScreenshot
            .call(
                serde_json::json!({}),
                ctx_fs(&full, FakeVision::Direct, Arc::clone(&fs) as Arc<_>),
            )
            .await;

        let ToolOutcome::Ok { model_content, .. } = out else {
            panic!("应当成功：{out:?}");
        };
        let ToolResultContent::Image { media_type, data, path } = model_content else {
            panic!("应当是图片内容块");
        };

        let path = path.expect("原图要落盘");
        assert_eq!(path, std::path::PathBuf::from("/artifacts/t1.jpg"));
        let stored = riot_protocol::tool::FileSystem::read(fs.as_ref(), &path)
            .await
            .expect("落盘的文件读得回来");
        assert_eq!(stored, raw, "落盘的必须是原图字节");

        assert_eq!(media_type, "image/jpeg", "压缩产物统一是 JPEG");
        // 体积不是好断言（纯色 PNG 本来就异常小），验像素:进消息的图
        // 必须缩到模型甜点区以内。
        let small = image::load_from_memory(
            &base64::engine::general_purpose::STANDARD
                .decode(&data)
                .expect("合法 base64"),
        )
        .expect("解得开压缩图");
        assert!(
            small.width() * small.height() <= crate::tools::shrink::MAX_MODEL_PIXELS,
            "给模型的该是压缩图，实际还有 {}×{}",
            small.width(),
            small.height()
        );
    }

    /// 既看不了图、也没配兼容模型 —— 要明确说去配，并且指出替代做法。
    #[tokio::test]
    async fn 没配兼容模型时说清怎么办() {
        let out = BrowserScreenshot
            .call(serde_json::json!({}), ctx("SHOT", FakeVision::None))
            .await;
        let ToolOutcome::Failed { error_for_model: text, .. } = out else {
            panic!("应当失败而不是给一张模型看不了的图：{out:?}");
        };
        assert!(text.contains("视觉兼容"), "要说清缺什么：{text}");
        assert!(text.contains("BrowserSnapshot"), "要给替代做法：{text}");
        assert!(!text.contains("SHOT"), "不能把图片数据塞进文字里：{text}");
    }

    /// 本地开发地址必须能进到宿主，且 http 不能被升级成 https。
    ///
    /// `[约束]` 不能复用 WebFetch 的 `normalize`：那条路会拒 localhost，
    /// 还会把地址改成 https —— 本地服务器几乎都没有证书。
    #[tokio::test]
    async fn 本地_http_地址可以导航且不升级协议() {
        let b = Arc::new(FakeBrowser::default());
        let ctx = ctx_browser(Arc::clone(&b), FakeVision::Direct, Arc::new(NullFs));
        let input = serde_json::json!({ "url": "http://localhost:8765/wechat.html" });

        BrowserNavigate
            .validate_input(&input, &ctx)
            .await
            .expect("本地地址应当通过校验");

        let out = BrowserNavigate.call(input.clone(), ctx).await;
        let ToolOutcome::Failed { error_for_model, .. } = out else {
            panic!("替身不导航，应当失败：{out:?}");
        };
        assert!(
            error_for_model.contains("替身不导航") || error_for_model.contains("不可用"),
            "应当到达宿主而不是被解析拦下：{error_for_model}"
        );
        assert_eq!(
            b.calls.lock().expect("calls")[0],
            "navigate http://localhost:8765/wechat.html"
        );

        let ask = BrowserNavigate.check_permissions(&input, &PermissionContext::default());
        assert!(
            matches!(ask, PermissionResult::Ask { .. }),
            "本地地址仍要问一次：{ask:?}"
        );
    }

    /// 本地 HTML 必须能进到宿主。模型预览静态页几乎总是走 `file://`。
    #[tokio::test]
    async fn 本地_file_地址可以导航() {
        // Windows 的 file URL 必须带盘符：`Url::to_file_path` 不接受没有盘符
        // 的路径，这个地址会在 URL 归一化那层就被判畸形。`LOCAL_PATH` 是它
        // 对应的本机路径 —— 弹窗和安全检查用的是这个形态。
        #[cfg(windows)]
        const FILE_URL: &str = "file:///C:/Users/me/proj/wechat.html";
        #[cfg(windows)]
        const LOCAL_PATH: &str = r"C:\Users\me\proj\wechat.html";
        #[cfg(not(windows))]
        const FILE_URL: &str = "file:///Users/me/proj/wechat.html";
        #[cfg(not(windows))]
        const LOCAL_PATH: &str = "/Users/me/proj/wechat.html";

        let b = Arc::new(FakeBrowser::default());
        let ctx = ctx_browser(Arc::clone(&b), FakeVision::Direct, Arc::new(NullFs));
        let input = serde_json::json!({ "url": FILE_URL });

        BrowserNavigate
            .validate_input(&input, &ctx)
            .await
            .expect("本地文件应当通过校验");

        let out = BrowserNavigate.call(input.clone(), ctx).await;
        let ToolOutcome::Failed { error_for_model, .. } = out else {
            panic!("替身不导航，应当失败：{out:?}");
        };
        assert!(
            error_for_model.contains("替身不导航") || error_for_model.contains("不可用"),
            "应当到达宿主而不是被解析拦下：{error_for_model}"
        );
        assert_eq!(
            b.calls.lock().expect("calls")[0],
            format!("navigate {FILE_URL}")
        );

        let ask = BrowserNavigate.check_permissions(&input, &PermissionContext::default());
        let PermissionResult::Ask { message, .. } = ask else {
            panic!("本地文件仍要问一次：{ask:?}");
        };
        assert!(
            message.contains(LOCAL_PATH),
            "弹窗要显示完整路径：{message}"
        );

        assert_eq!(
            BrowserNavigate.target_path(&input),
            Some(std::path::PathBuf::from(LOCAL_PATH)),
            "凭证文件的安全检查靠这条路径"
        );
    }

    // ── 交互工具 ──────────────────────────────────────

    fn interactive(reply: Result<&str, &str>) -> Arc<FakeBrowser> {
        Arc::new(FakeBrowser {
            interact: Some(match reply {
                Ok(m) => Ok(m.to_owned()),
                Err(t) => Err(t.to_owned()),
            }),
            ..FakeBrowser::default()
        })
    }

    /// 参数要原样到达宿主 —— ref/text/submit 少一个都是"点了没反应"。
    #[tokio::test]
    async fn 交互参数原样到达宿主() {
        let b = interactive(Ok("好了"));
        let _ = BrowserClick
            .call(
                serde_json::json!({ "ref": 3 }),
                ctx_browser(Arc::clone(&b), FakeVision::Direct, Arc::new(NullFs)),
            )
            .await;
        let _ = BrowserType
            .call(
                serde_json::json!({ "ref": 5, "text": "你好", "submit": true }),
                ctx_browser(Arc::clone(&b), FakeVision::Direct, Arc::new(NullFs)),
            )
            .await;
        let _ = BrowserKey
            .call(
                serde_json::json!({ "key": "Enter" }),
                ctx_browser(Arc::clone(&b), FakeVision::Direct, Arc::new(NullFs)),
            )
            .await;
        let _ = BrowserScroll
            .call(
                serde_json::json!({ "delta_y": -350.0 }),
                ctx_browser(Arc::clone(&b), FakeVision::Direct, Arc::new(NullFs)),
            )
            .await;
        // 选择器/文本定位也要原样传下去，不是只支持编号。
        let _ = BrowserClick
            .call(
                serde_json::json!({ "selector": "#login" }),
                ctx_browser(Arc::clone(&b), FakeVision::Direct, Arc::new(NullFs)),
            )
            .await;
        let _ = BrowserType
            .call(
                serde_json::json!({ "target_text": "邮箱", "text": "a@b.c" }),
                ctx_browser(Arc::clone(&b), FakeVision::Direct, Arc::new(NullFs)),
            )
            .await;

        let calls = b.calls.lock().expect("calls");
        assert_eq!(
            *calls,
            vec![
                "click ref:3".to_owned(),
                "type ref:5 \"你好\" submit=true".to_owned(),
                "key Enter".to_owned(),
                "scroll -350".to_owned(),
                "click sel:#login".to_owned(),
                "type text:邮箱 \"a@b.c\" submit=false".to_owned(),
            ]
        );
    }

    /// BrowserWaitFor 恰好取一个条件，超时封顶，参数原样到宿主。
    #[tokio::test]
    async fn 等待条件与超时传到宿主() {
        let b = interactive(Ok("等到了"));
        let _ = BrowserWaitFor
            .call(
                serde_json::json!({ "selector": ".ready", "timeout_ms": 3000 }),
                ctx_browser(Arc::clone(&b), FakeVision::Direct, Arc::new(NullFs)),
            )
            .await;
        let _ = BrowserWaitFor
            .call(
                serde_json::json!({ "network_idle": true }),
                ctx_browser(Arc::clone(&b), FakeVision::Direct, Arc::new(NullFs)),
            )
            .await;
        let calls = b.calls.lock().expect("calls");
        assert_eq!(calls[0], "wait Selector(\".ready\") 3000");
        assert_eq!(calls[1], "wait NetworkIdle 10000", "缺省超时是 10s");
    }

    /// 三种定位一个都不给要在校验期就拦下，而不是发一个空目标到宿主。
    #[tokio::test]
    async fn 点击缺目标被校验拦下() {
        let ctx = ctx_browser(interactive(Ok("x")), FakeVision::Direct, Arc::new(NullFs));
        let r = BrowserClick.validate_input(&serde_json::json!({}), &ctx).await;
        assert!(r.is_err(), "没有 ref/selector/text 该被拦");
    }

    /// 双击、右键、组合键、下拉、拖拽、导航、标签都要落到正确的宿主动作上。
    #[tokio::test]
    async fn 扩展交互路由到正确动作() {
        let b = interactive(Ok("好"));
        let run = |tool: &'static str, input: serde_json::Value, b: Arc<FakeBrowser>| async move {
            let ctx = ctx_browser(b, FakeVision::Direct, Arc::new(NullFs));
            match tool {
                "hover" => BrowserHover.call(input, ctx).await,
                "select" => BrowserSelect.call(input, ctx).await,
                "drag" => BrowserDrag.call(input, ctx).await,
                "go" => BrowserGo.call(input, ctx).await,
                "tabs" => BrowserTabs.call(input, ctx).await,
                "key" => BrowserKey.call(input, ctx).await,
                "click" => BrowserClick.call(input, ctx).await,
                _ => unreachable!(),
            }
        };
        run("click", serde_json::json!({ "ref": 1, "double": true }), Arc::clone(&b)).await;
        run("click", serde_json::json!({ "ref": 1, "right": true }), Arc::clone(&b)).await;
        run("key", serde_json::json!({ "key": "Control+a" }), Arc::clone(&b)).await;
        run("hover", serde_json::json!({ "selector": ".menu" }), Arc::clone(&b)).await;
        run("select", serde_json::json!({ "selector": "#s", "value": "cn" }), Arc::clone(&b)).await;
        run(
            "drag",
            serde_json::json!({ "from_ref": 2, "to_selector": ".slot" }),
            Arc::clone(&b),
        )
        .await;
        run("go", serde_json::json!({ "direction": "back" }), Arc::clone(&b)).await;
        run("tabs", serde_json::json!({ "action": "list" }), Arc::clone(&b)).await;
        run("tabs", serde_json::json!({ "action": "select", "id": 3 }), Arc::clone(&b)).await;

        let calls = b.calls.lock().expect("calls");
        assert_eq!(calls[0], "act DoubleClick(Ref(1))");
        assert_eq!(calls[1], "act RightClick(Ref(1))");
        assert_eq!(calls[2], "act KeyChord(\"Control+a\")");
        assert_eq!(calls[3], "act Hover(Selector(\".menu\"))");
        assert_eq!(calls[4], "act SelectOption { target: Selector(\"#s\"), value: \"cn\" }");
        assert_eq!(calls[5], "act Drag { from: Ref(2), to: Selector(\".slot\") }");
        assert_eq!(calls[6], "browse Back");
        assert_eq!(calls[7], "browse ListTabs");
        assert_eq!(calls[8], "browse SelectTab(3)");
    }

    /// 抓包三种 action 路由到正确的查询；被动观察免确认。
    #[tokio::test]
    async fn 抓包路由与免确认() {
        let b = interactive(Ok("流量"));
        let run = |input: serde_json::Value, b: Arc<FakeBrowser>| async move {
            BrowserNetwork
                .call(input, ctx_browser(b, FakeVision::Direct, Arc::new(NullFs)))
                .await
        };
        run(serde_json::json!({ "action": "list", "filter": "api" }), Arc::clone(&b)).await;
        run(serde_json::json!({ "action": "detail", "request_id": "9.3" }), Arc::clone(&b)).await;
        run(serde_json::json!({ "action": "audit" }), Arc::clone(&b)).await;

        let calls = b.calls.lock().expect("calls");
        assert_eq!(calls[0], "network List { filter: Some(\"api\") }");
        assert_eq!(calls[1], "network Detail { request_id: \"9.3\" }");
        assert_eq!(calls[2], "network Audit");

        // 被动抓包免确认。
        let r = BrowserNetwork.check_permissions(&serde_json::json!({}), &PermissionContext::default());
        assert!(matches!(r, PermissionResult::Allow { .. }), "{r:?}");
    }

    /// evaluate 把表达式原样交给宿主；cookies 无参调用。
    #[tokio::test]
    async fn 执行脚本和读cookie路由正确() {
        let b = interactive(Ok("42"));
        let _ = BrowserEvaluate
            .call(
                serde_json::json!({ "expression": "1+1" }),
                ctx_browser(Arc::clone(&b), FakeVision::Direct, Arc::new(NullFs)),
            )
            .await;
        let _ = BrowserCookies
            .call(
                serde_json::json!({}),
                ctx_browser(Arc::clone(&b), FakeVision::Direct, Arc::new(NullFs)),
            )
            .await;
        let calls = b.calls.lock().expect("calls");
        assert_eq!(calls[0], "eval 1+1");
        assert_eq!(calls[1], "cookies");
    }

    /// evaluate 默认问一次、理由是 Consent（放行模式能压过），规划模式不抢答。
    #[test]
    fn 执行脚本权限是可放行的同意() {
        let ask = BrowserEvaluate
            .check_permissions(&serde_json::json!({ "expression": "x" }), &PermissionContext::default());
        assert!(
            matches!(ask, PermissionResult::Ask { reason: DecisionReason::Consent { .. }, .. }),
            "{ask:?}"
        );
        let plan = PermissionContext {
            mode: riot_protocol::permission::PermissionModeState(Some(PermissionMode::Plan)),
            ..PermissionContext::default()
        };
        let r = BrowserEvaluate.check_permissions(&serde_json::json!({ "expression": "x" }), &plan);
        assert!(matches!(r, PermissionResult::Passthrough), "规划模式该交给决策链：{r:?}");
    }

    /// 单个功能键仍走 press_key，不被误判成组合键。
    #[tokio::test]
    async fn 单键不走组合键路径() {
        let b = interactive(Ok("好"));
        let _ = BrowserKey
            .call(
                serde_json::json!({ "key": "Enter" }),
                ctx_browser(Arc::clone(&b), FakeVision::Direct, Arc::new(NullFs)),
            )
            .await;
        let calls = b.calls.lock().expect("calls");
        assert_eq!(calls[0], "key Enter", "单键该走 press_key");
    }

    /// 标签页 new 不接受 URL —— 开新标签不能成为绕过域名同意的旁路。
    #[test]
    fn 新标签页操作免确认且不带地址() {
        let r = BrowserTabs
            .check_permissions(&serde_json::json!({ "action": "new" }), &PermissionContext::default());
        assert!(matches!(r, PermissionResult::Allow { .. }), "{r:?}");
        // schema 里没有 url 字段:开标签只能开空白页。
        let schema = serde_json::to_string(&BrowserTabs.input_schema()).expect("schema");
        assert!(!schema.contains("\"url\""), "开标签不该收 URL：{schema}");
    }

    /// 宿主的成功消息原样给模型 —— 那句话里有"页面跳到了哪儿"。
    #[tokio::test]
    async fn 点击成功时透传宿主的消息() {
        let out = BrowserClick
            .call(
                serde_json::json!({ "ref": 1 }),
                ctx_browser(
                    interactive(Ok("已点击 button \"提交\"，页面跳到了 https://x.test/done")),
                    FakeVision::Direct,
                    Arc::new(NullFs),
                ),
            )
            .await;
        let ToolOutcome::Ok { model_content: ToolResultContent::Text { text }, .. } = out else {
            panic!("应当成功：{out:?}");
        };
        assert!(text.contains("页面跳到了"), "{text}");
    }

    /// 编号失效的指引是"重新快照"，不能劝去 WebFetch。
    ///
    /// `[约束]` 两类失败的指引相反（见 InteractError）。混起来的话，
    /// 模型要么对着挂掉的浏览器反复快照，要么放着好好的页面去抓源码。
    #[tokio::test]
    async fn 编号失效时指引重新快照() {
        let out = BrowserClick
            .call(
                serde_json::json!({ "ref": 9 }),
                ctx_browser(
                    interactive(Err("编号 [9] 不在最近一次快照里。用 BrowserSnapshot 重新拿编号。")),
                    FakeVision::Direct,
                    Arc::new(NullFs),
                ),
            )
            .await;
        let ToolOutcome::Failed { error_for_model: text, .. } = out else {
            panic!("应当失败：{out:?}");
        };
        assert!(text.contains("BrowserSnapshot"), "{text}");
        assert!(!text.contains("WebFetch"), "编号失效不该劝换工具：{text}");
    }

    /// 浏览器整个不可用才劝换 WebFetch。
    #[tokio::test]
    async fn 浏览器不可用时才劝换路() {
        let out = BrowserClick
            .call(
                serde_json::json!({ "ref": 1 }),
                // interact: None = 替身报"不可用"
                ctx_browser(
                    Arc::new(FakeBrowser::default()),
                    FakeVision::Direct,
                    Arc::new(NullFs),
                ),
            )
            .await;
        let ToolOutcome::Failed { error_for_model: text, .. } = out else {
            panic!("应当失败：{out:?}");
        };
        assert!(text.contains("WebFetch"), "不可用要给替代出路：{text}");
    }

    /// 默认模式下交互问一次，且"总是允许"一次覆盖三个交互工具。
    ///
    /// `[约束]` 分开记的话点击、输入、按键各弹一次窗 —— 三连问训练出的
    /// 是无脑点允许。
    #[test]
    fn 交互权限一次询问覆盖三个工具() {
        let ctx = PermissionContext::default();
        let r = BrowserClick.check_permissions(&serde_json::json!({ "ref": 1 }), &ctx);
        let PermissionResult::Ask { suggestions, reason, .. } = r else {
            panic!("默认模式该问：{r:?}");
        };
        assert!(
            matches!(reason, DecisionReason::Consent { .. }),
            "理由必须是 Consent，否则「全部放行」压不过它：{reason:?}"
        );
        let tools: Vec<_> = suggestions
            .iter()
            .map(|s| match s {
                PermissionUpdate::AddRule { tool, pattern, decision, .. } => {
                    assert!(pattern.is_none(), "交互规则是整工具级的");
                    assert_eq!(*decision, RuleDecision::Allow);
                    tool.as_str()
                }
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(tools, INTERACT_TOOLS);
    }

    /// 规划模式下交互工具交给决策链拒绝，不该抢答成"问问看"。
    #[test]
    fn 规划模式下交互不抢答() {
        let ctx = PermissionContext {
            mode: riot_protocol::permission::PermissionModeState(Some(PermissionMode::Plan)),
            ..PermissionContext::default()
        };
        let r = BrowserType.check_permissions(&serde_json::json!({}), &ctx);
        assert!(
            matches!(r, PermissionResult::Passthrough),
            "该交给决策链按写操作拒绝：{r:?}"
        );
    }

    /// 滚动免确认 —— 和截图同一信任级别。
    #[test]
    fn 滚动免确认() {
        let r = BrowserScroll
            .check_permissions(&serde_json::json!({ "delta_y": 700.0 }), &PermissionContext::default());
        assert!(matches!(r, PermissionResult::Allow { .. }), "{r:?}");
    }

    // ── 授权 scope ────────────────────────────────────

    use riot_protocol::permission::{PermissionRule, RuleSource};

    fn scope_rule(host: &str) -> PermissionRule {
        PermissionRule {
            tool: SCOPE_TOOL.to_owned(),
            pattern: Some(format!("scope:{host}")),
            decision: RuleDecision::Allow,
            source: RuleSource::Session,
        }
    }

    #[test]
    fn scope_从_url_和裸域名都能取出_host() {
        assert_eq!(target_host("https://a.example.com/x?y=1").as_deref(), Some("a.example.com"));
        assert_eq!(target_host("example.com").as_deref(), Some("example.com"));
        assert_eq!(target_host("example.com:8443/path").as_deref(), Some("example.com"));
        assert_eq!(target_host("not a url").as_deref(), None);
        assert_eq!(
            target_host("http://localhost:8765/wechat.html").as_deref(),
            Some("localhost")
        );
        assert_eq!(target_host("http://127.0.0.1:3000/").as_deref(), Some("127.0.0.1"));
        assert_eq!(target_host("localhost").as_deref(), Some("localhost"));
    }

    #[test]
    fn 未授权目标要求先加入_scope() {
        let r = scope_gate("target.test", &PermissionContext::default());
        let PermissionResult::Ask { reason, suggestions, .. } = r else {
            panic!("未授权该问：{r:?}");
        };
        // 理由必须是 SafetyCheck::OutOfScope —— 这是它对 bypass 免疫的根据。
        assert!(
            matches!(reason, DecisionReason::SafetyCheck { safety: SafetyKind::OutOfScope }),
            "{reason:?}"
        );
        match &suggestions[0] {
            PermissionUpdate::AddRule { tool, pattern, .. } => {
                assert_eq!(tool, SCOPE_TOOL);
                assert_eq!(pattern.as_deref(), Some("scope:target.test"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn 已授权目标放行() {
        let ctx = PermissionContext {
            rules: vec![scope_rule("target.test")],
            ..PermissionContext::default()
        };
        assert!(matches!(scope_gate("target.test", &ctx), PermissionResult::Allow { .. }));
        // 别的域名不受这条影响。
        assert!(matches!(scope_gate("other.test", &ctx), PermissionResult::Ask { .. }));
    }

    #[test]
    fn 规划模式下渗透交给决策链拒绝() {
        let ctx = PermissionContext {
            mode: riot_protocol::permission::PermissionModeState(Some(PermissionMode::Plan)),
            ..PermissionContext::default()
        };
        assert!(matches!(pentest_permission("target.test", &ctx), PermissionResult::Passthrough));
    }

    /// 重放/拦截受 scope 约束:未授权目标要问，已授权放行；参数原样到宿主。
    #[tokio::test]
    async fn 重放拦截受_scope_约束且路由正确() {
        // 未授权:check_permissions 要 Ask（SafetyCheck::OutOfScope）。
        let ask = BrowserReplay.check_permissions(
            &serde_json::json!({ "url": "https://evil.test/api" }),
            &PermissionContext::default(),
        );
        assert!(
            matches!(ask, PermissionResult::Ask { reason: DecisionReason::SafetyCheck { safety: SafetyKind::OutOfScope }, .. }),
            "{ask:?}"
        );

        // 已授权 host:放行，且调用透传到宿主。
        let ctx_rules = PermissionContext {
            rules: vec![scope_rule("api.test")],
            ..PermissionContext::default()
        };
        let allow = BrowserReplay.check_permissions(
            &serde_json::json!({ "url": "https://api.test/x" }),
            &ctx_rules,
        );
        assert!(matches!(allow, PermissionResult::Allow { .. }), "{allow:?}");

        let b = interactive(Ok("响应"));
        let _ = BrowserReplay
            .call(
                serde_json::json!({ "url": "https://api.test/login", "method": "post", "body": "u=1" }),
                ctx_browser(Arc::clone(&b), FakeVision::Direct, Arc::new(NullFs)),
            )
            .await;
        let _ = BrowserIntercept
            .call(
                serde_json::json!({ "action": "block", "host": "api.test", "url_pattern": "/track" }),
                ctx_browser(Arc::clone(&b), FakeVision::Direct, Arc::new(NullFs)),
            )
            .await;
        let calls = b.calls.lock().expect("calls");
        // method 被大写化。
        assert_eq!(calls[0], "replay POST https://api.test/login body=true");
        assert_eq!(calls[1], "intercept Block { url_pattern: \"/track\" }");
    }

    /// fuzz 受 scope 约束、要有 FUZZ 占位符;密钥/发现是被动免确认。
    #[tokio::test]
    async fn 探针工具的权限与校验() {
        // fuzz 未授权目标 → SafetyCheck::OutOfScope 的 Ask。
        let ask = BrowserFuzz.check_permissions(
            &serde_json::json!({ "url": "https://evil.test/s?q=FUZZ" }),
            &PermissionContext::default(),
        );
        assert!(
            matches!(ask, PermissionResult::Ask { reason: DecisionReason::SafetyCheck { safety: SafetyKind::OutOfScope }, .. }),
            "{ask:?}"
        );
        // 少了 FUZZ 占位符 → 校验拦下。
        let ctx = ctx_browser(interactive(Ok("x")), FakeVision::Direct, Arc::new(NullFs));
        let bad = BrowserFuzz
            .validate_input(&serde_json::json!({ "url": "https://api.test/s?q=1" }), &ctx)
            .await;
        assert!(bad.is_err(), "缺 FUZZ 该被拦");
        // 密钥扫描、接口发现都是被动免确认。
        for r in [
            BrowserSecrets.check_permissions(&serde_json::json!({}), &PermissionContext::default()),
            BrowserDiscover.check_permissions(&serde_json::json!({}), &PermissionContext::default()),
        ] {
            assert!(matches!(r, PermissionResult::Allow { .. }), "{r:?}");
        }
    }

    /// 密钥扫描把页面 HTML 交给纯扫描逻辑;命中打码后报出。
    #[tokio::test]
    async fn 密钥扫描透传页面并打码() {
        // 替身的 evaluate 返回"页面 HTML"（含一个 AWS key）。
        let b = interactive(Ok("<script>k='AKIAIOSFODNN7EXAMPLE'</script>"));
        let out = BrowserSecrets
            .call(
                serde_json::json!({}),
                ctx_browser(Arc::clone(&b), FakeVision::Direct, Arc::new(NullFs)),
            )
            .await;
        let ToolOutcome::Ok { model_content: ToolResultContent::Text { text }, .. } = out else {
            panic!("应当成功：{out:?}");
        };
        assert!(text.contains("AWS Access Key"), "{text}");
        assert!(!text.contains("AKIAIOSFODNN7EXAMPLE"), "要打码：{text}");
    }

    /// 上传:定位透传 + 路径透传;敏感操作问一次。
    #[tokio::test]
    async fn 上传路由与权限() {
        let b = interactive(Ok("已设置"));
        let _ = BrowserUpload
            .call(
                serde_json::json!({ "selector": "#file", "paths": ["/tmp/a.png", "/tmp/b.png"] }),
                ctx_browser(Arc::clone(&b), FakeVision::Direct, Arc::new(NullFs)),
            )
            .await;
        assert_eq!(b.calls.lock().expect("calls")[0], "upload sel:#file /tmp/a.png,/tmp/b.png");
        // 上传本地文件敏感 —— 默认问一次。
        let r = BrowserUpload.check_permissions(
            &serde_json::json!({ "selector": "#f", "paths": ["/x"] }),
            &PermissionContext::default(),
        );
        assert!(matches!(r, PermissionResult::Ask { reason: DecisionReason::Consent { .. }, .. }), "{r:?}");
    }

    /// 爬虫受 scope 约束、要合法起点 URL。
    #[test]
    fn 爬虫受_scope_约束() {
        let ask = BrowserCrawl.check_permissions(
            &serde_json::json!({ "url": "https://evil.test/" }),
            &PermissionContext::default(),
        );
        assert!(
            matches!(ask, PermissionResult::Ask { reason: DecisionReason::SafetyCheck { safety: SafetyKind::OutOfScope }, .. }),
            "{ask:?}"
        );
        let ok = BrowserCrawl.check_permissions(
            &serde_json::json!({ "url": "https://api.test/" }),
            &PermissionContext { rules: vec![scope_rule("api.test")], ..PermissionContext::default() },
        );
        assert!(matches!(ok, PermissionResult::Allow { .. }), "{ok:?}");
    }

    /// 报告:免确认写工件、返回全文。
    #[tokio::test]
    async fn 报告生成并返回全文() {
        let ctx = ctx_browser(interactive(Ok("x")), FakeVision::Direct, Arc::new(NullFs));
        let out = BrowserReport
            .call(
                serde_json::json!({
                    "target": "api.test",
                    "findings": [{ "title": "反射型 XSS", "severity": "high", "evidence": "<script>", "remediation": "输出编码" }]
                }),
                ctx,
            )
            .await;
        let ToolOutcome::Ok { model_content: ToolResultContent::Text { text }, .. } = out else {
            panic!("应当成功：{out:?}");
        };
        assert!(text.contains("渗透测试报告"), "{text}");
        assert!(text.contains("反射型 XSS") && text.contains("输出编码"), "{text}");
        let r = BrowserReport.check_permissions(&serde_json::json!({ "findings": [] }), &PermissionContext::default());
        assert!(matches!(r, PermissionResult::Allow { .. }), "报告免确认：{r:?}");
    }

    /// 拦截的 list/clear 不打目标，免确认（不受 scope）。
    #[test]
    fn 拦截的列和清免确认() {
        for action in ["list", "clear"] {
            let r = BrowserIntercept
                .check_permissions(&serde_json::json!({ "action": action }), &PermissionContext::default());
            assert!(matches!(r, PermissionResult::Allow { .. }), "{action}: {r:?}");
        }
    }

    /// scope 对「全部放行」免疫:未授权目标在 bypass 下仍然要问，不放行。
    ///
    /// 这是整个渗透安全骨架的关键一条 —— 走真实决策链验证，而不只是看
    /// 工具返回值。用一个复刻渗透工具形状的替身:非只读、check_permissions
    /// 走 scope_gate。
    #[test]
    fn scope_对_bypass_免疫() {
        use riot_permissions::chain::decide;
        use riot_permissions::rules::RuleSet as ChainRuleSet;

        // 复刻渗透工具:非只读，check_permissions 返回 scope_gate 的结果。
        struct PentestLike;
        #[async_trait::async_trait]
        impl Tool for PentestLike {
            fn name(&self) -> &'static str { "BrowserReplayLike" }
            fn input_schema(&self) -> schemars::Schema { schemars::schema_for!(NoInput) }
            fn prompt(&self, _c: &PromptContext) -> String { String::new() }
            fn describe(&self, _i: &serde_json::Value) -> String { String::new() }
            fn check_permissions(&self, _i: &serde_json::Value, ctx: &PermissionContext) -> PermissionResult {
                pentest_permission("evil.test", ctx)
            }
            async fn call(&self, _i: serde_json::Value, _c: ToolContext) -> ToolOutcome {
                ToolOutcome::ok_text("x")
            }
        }

        let ctx = PermissionContext {
            mode: riot_protocol::permission::PermissionModeState(Some(PermissionMode::BypassPermissions)),
            can_prompt_user: true,
            ..PermissionContext::default()
        };
        let r = decide(&PentestLike, &serde_json::json!({}), &ctx, &ChainRuleSet::default());
        assert!(
            matches!(r, PermissionResult::Ask { reason: DecisionReason::SafetyCheck { .. }, .. }),
            "bypass 下未授权目标仍要问：{r:?}"
        );
    }
}
