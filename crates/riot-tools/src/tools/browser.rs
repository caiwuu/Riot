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
//! `[约束]` 导航用的内容键是 `domain:<host>`，**和 WebFetch 完全一致**。
//!
//! 用户点"总是允许 example.com"表达的是"我信任这个站"，那个判断和用哪个
//! 工具去访问无关。各用各的键会让同一个域名被问两遍，而且「全部放行」
//! 只对其中一边生效 —— 那种不一致用户根本没法理解。

use std::sync::Arc;

use riot_protocol::browser::BrowserUnavailable;
use riot_protocol::message::ToolResultContent;
use riot_protocol::permission::{DecisionReason, PermissionContext, PermissionResult};
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
    /// 要打开的地址，必须含协议（`https://...`）。
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
         BrowserConsole 看报错。适合验证自己刚改完的前端。"
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

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        let raw = input.get("url").and_then(|v| v.as_str()).unwrap_or_default();
        weburl::normalize(raw).map(|_| ()).map_err(|e| {
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
        let Ok(u) = weburl::normalize(raw) else {
            return PermissionResult::Passthrough;
        };
        super::web::consent::decide_for_domain(self.name(), &u, ctx)
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let raw = input.get("url").and_then(|v| v.as_str()).unwrap_or_default();
        let Ok(u) = weburl::normalize(raw) else {
            return ToolOutcome::failed("URL 无法解析。请给出含协议的完整地址。");
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
         用它；只想知道页面上有什么元素的话，BrowserSnapshot 更省。"
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
        let data = match ctx.browser.screenshot().await {
            Ok(d) => d,
            Err(e) => return ToolOutcome::failed(unavailable_hint(&e)),
        };

        if data.len() > MAX_SHOT_B64 {
            return ToolOutcome::failed(format!(
                "截图有 {} KB，超过上限。这个页面太长或太复杂 —— \
                 先用 BrowserSnapshot 定位到关心的区域，或者把窗口调窄再截。",
                data.len() / 1024
            ));
        }

        ToolOutcome::Ok {
            model_content: ToolResultContent::Image {
                media_type: "image/png".into(),
                data,
            },
            ui_payload: None,
            side_messages: Vec::new(),
        }
    }
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

// ── 共用 ──────────────────────────────────────────────

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
pub fn tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(BrowserNavigate),
        Arc::new(BrowserSnapshot),
        Arc::new(BrowserScreenshot),
        Arc::new(BrowserConsole),
    ]
}
