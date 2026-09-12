//! WebFetch 工具。抓一个 URL，转成 Markdown，按 prompt 提炼后返回。
//!
//! # 权限粒度是域名，不是工具
//!
//! 用户点"总是允许"时想表达的是"我信任 docs.rs"，不是"以后随便抓什么都
//! 行"。所以规则内容写成 `domain:docs.rs`，[`WebFetch::check_permissions`]
//! 自己做匹配 —— 通用决策链的内容级维度只认 Bash 的命令串。
//!
//! # 为什么 is_read_only 是 true
//!
//! 它不动工作区里的任何东西，规划模式下也应该能查文档。但**不能**因此
//! 走到决策链第 7 步的"只读一律放行" —— 抓取是一条数据外带通道。
//! [`WebFetch::check_permissions`] 在第 3 步就给出 Ask/Allow 的明确表态，
//! 兜底逻辑够不到。

use async_trait::async_trait;
use base64::Engine as _;
use riot_protocol::message::ToolResultContent;
use riot_protocol::permission::{PermissionContext, PermissionResult};
use riot_protocol::text::UiText;
use riot_protocol::tool::{
    InterruptBehavior, PromptContext, ResultBudget, Tool, ToolContext, ToolOutcome, UiPayload,
    ValidationError,
};
use riot_protocol::ui_text;
use riot_protocol::vision::DescribeRequest;
use riot_protocol::web::WebError;
use serde::Deserialize;
use std::sync::Arc;
use url::Url;

use super::cache::PageCache;
use super::markdown::{self, MAX_CONTENT_CHARS};
use super::pipeline::{self, Fetched, FetchedImage};
use super::preapproved;
use super::url as weburl;
use crate::tools::read::MAX_IMAGE_BYTES;
use crate::tools::shrink;

pub const WEB_FETCH: &str = "WebFetch";

/// 可信来源且已经是 Markdown、又不太长时，直接给原文不蒸馏。
///
/// 官方文档站的代码示例被小模型摘要一遍基本就废了 —— 模型会照着一段被
/// 改写过的示例写代码，然后编译不过。
const RAW_PASSTHROUGH_CHARS: usize = 30_000;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    /// 要抓取的完整 URL。
    url: String,
    /// 想从这个页面里得到什么。会交给辅助模型用来提炼正文。
    prompt: String,
}

pub struct WebFetch {
    cache: Arc<PageCache>,
}

impl WebFetch {
    pub fn new(cache: Arc<PageCache>) -> Self {
        Self { cache }
    }
}

#[async_trait]
impl Tool for WebFetch {
    fn name(&self) -> &'static str {
        WEB_FETCH
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "抓取一个网页，转成 Markdown，再按你给的 prompt 提炼要点。\n\
         \n\
         - `url` 要是完整地址；http 会自动升级成 https。\n\
         - `prompt` 写清你想从这个页面里得到什么，例如\
         「取出 axum 的路由定义示例」，不要只写「总结一下」。\n\
         - 想搜索而不是读某个具体页面，用 WebSearch。\n\
         - 抓取 GitHub 的 issue/PR 时，优先用 `Bash(gh ...)`，\
         那条路能拿到结构化数据而且不受登录限制。\n\
         - 需要登录才能看的页面（私有仓库、内部文档）抓不到，别反复重试。\n\
         - 同一个 URL 15 分钟内重复抓取会走缓存，不用担心重复请求。\n\
         - 跳转到别的站点时，工具会把新地址告诉你，你需要用新地址\
         **再调一次**（这一次会重新征求用户同意）。\n\
         - 地址指向图片（png / jpg / gif / webp）时，图片会作为图返回给你看，\
         同时显示在界面上给用户看；`prompt` 写你想从图里看什么。\
         不要用 Bash 下载图片再想办法解码。"
            .to_owned()
    }

    fn describe(&self, input: &serde_json::Value) -> UiText {
        let url = input.get("url").and_then(|v| v.as_str()).unwrap_or("");
        match url::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
        {
            Some(h) => ui_text!("tools.webFetch.host", host = h),
            None => ui_text!("tools.webFetch.any"),
        }
    }

    /// 不动工作区。见模块顶部关于这个返回值的说明。
    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn interrupt_behavior(&self) -> InterruptBehavior {
        InterruptBehavior::Cancel
    }

    /// 内部已经截断到 [`MAX_CONTENT_CHARS`]，不需要外层再落盘。
    fn result_budget(&self) -> ResultBudget {
        ResultBudget::Unlimited
    }

    fn classifier_input(&self, input: &serde_json::Value) -> Option<String> {
        input
            .get("url")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned)
    }

    fn check_permissions(
        &self,
        input: &serde_json::Value,
        ctx: &PermissionContext,
    ) -> PermissionResult {
        let raw = input
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or_default();

        // 解析不了的 URL 交给 validate_input 去报错。这里表态成 Deny 的话，
        // 模型收到的是"没权限"，它会去请求权限而不是修 URL。
        let Ok(u) = weburl::normalize(raw) else {
            return PermissionResult::Passthrough;
        };

        // 判定和内置浏览器共用一份。用户信任的是站点，不是工具 ——
        // 各写一份会让同一个域名被问两遍。见 consent 模块。
        super::consent::decide_for_domain(WEB_FETCH, &u, ctx)
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        let parsed: Input = serde_json::from_value(input.clone())
            .map_err(|e| ValidationError::rejected(schema_hint(&e)))?;

        weburl::normalize(&parsed.url).map_err(|e| {
            ValidationError::rejected(format!(
                "URL `{}` 不能抓取：{e}。请给一个完整的公网 http(s) 地址。",
                parsed.url
            ))
        })?;

        if parsed.prompt.trim().is_empty() {
            return Err(ValidationError::rejected(
                "`prompt` 不能为空。写清你想从这个页面里得到什么，\
                 例如「取出安装步骤」。",
            ));
        }
        Ok(())
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let parsed: Input = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolOutcome::failed(schema_hint(&e)),
        };

        let u = match weburl::normalize(&parsed.url) {
            Ok(u) => u,
            Err(e) => return ToolOutcome::failed(format!("URL 不能抓取：{e}")),
        };

        let started = ctx.clock.now_ms();

        let fetched = match pipeline::fetch_page(&u, &ctx, &self.cache).await {
            Ok(f) => f,
            Err(WebError::Cancelled) => return ToolOutcome::Cancelled,
            Err(e) => return ToolOutcome::failed(fetch_hint(&e, &parsed.url)),
        };

        let page = match fetched {
            // 跨站跳转不自动跟。把新地址交回模型，让它重新发起 —— 那一次
            // 会重新征求用户对新域名的同意。
            Fetched::CrossHost { from, to, status } => {
                let text = format!(
                    "检测到跨站跳转，没有自动跟随。\n\n\
                     原地址：{from}\n跳转到：{to}\n状态码：{status}\n\n\
                     如果这个跳转符合预期，请用新地址再调一次 WebFetch：\n\
                     - url: \"{to}\"\n- prompt: \"{}\"",
                    parsed.prompt
                );
                return ToolOutcome::Ok {
                    model_content: ToolResultContent::text(text.clone()),
                    ui_payload: Some(UiPayload::Plain { text }),
                    side_messages: Vec::new(),
                };
            }
            Fetched::Image(img) => return image_outcome(img, &u, &parsed.prompt, &ctx).await,
            Fetched::Page(p) => p,
        };

        let trusted = preapproved::is_preapproved(u.host_str().unwrap_or_default(), u.path());

        // 可信来源 + 已经是 Markdown + 不太长 → 原样给，别让小模型改写代码示例。
        let body = if trusted
            && page.content_type.contains("markdown")
            && page.content.chars().count() < RAW_PASSTHROUGH_CHARS
        {
            page.content.clone()
        } else {
            pipeline::distill_or_truncate(&page.content, &parsed.prompt, trusted, &ctx).await
        };

        if ctx.cancel.is_cancelled() {
            return ToolOutcome::Cancelled;
        }

        let elapsed = ctx.clock.now_ms().saturating_sub(started);
        let text = format!(
            "{}\n\n---\n来源：{} （{} 字节，耗时 {:.1}s）",
            markdown::truncate(&body, MAX_CONTENT_CHARS),
            u.as_str(),
            page.raw_bytes,
            elapsed as f64 / 1000.0
        );

        ToolOutcome::Ok {
            model_content: ToolResultContent::text(text.clone()),
            ui_payload: Some(UiPayload::Plain { text }),
            side_messages: Vec::new(),
        }
    }
}

/// 模型收得了的图片类型，以及落盘用的扩展名。
///
/// `[约束]` 和 Read、宿主 `read_image` 是同一份清单：界面按 `path` 读原图
/// 时走的就是 `read_image`，这里认了它不认的类型，界面上就是一张裂图。
fn supported_image(media_type: &str) -> Option<(&'static str, &'static str)> {
    match media_type {
        "image/png" => Some(("image/png", "png")),
        "image/jpeg" => Some(("image/jpeg", "jpg")),
        "image/gif" => Some(("image/gif", "gif")),
        "image/webp" => Some(("image/webp", "webp")),
        _ => None,
    }
}

/// 抓回来的图交给模型。和 Read 读图、浏览器截图同一套分支。
///
/// 原图落到工件目录，界面按路径显示原图；进消息、发给模型的是压缩图
/// （见 shrink 模块）。`prompt` 在模型自己能看图时用不上 —— 它看着图自己
/// 找答案；走视觉兼容时它就是转述的侧重点。
async fn image_outcome(
    img: FetchedImage,
    url: &Url,
    prompt: &str,
    ctx: &ToolContext,
) -> ToolOutcome {
    let Some((media_type, ext)) = supported_image(&img.media_type) else {
        return ToolOutcome::failed(format!(
            "{url} 是一张 {} 图片，模型只收 png / jpg / gif / webp。\
             找这张图的其它格式版本，或者告诉用户这张图看不了。\
             不要用 Bash 下载后转换 —— 转换出来的图仍然进不了对话。",
            img.media_type
        ));
    };

    if img.bytes.len() > MAX_IMAGE_BYTES {
        return ToolOutcome::failed(format!(
            "{url} 的图片有 {} KB，超过单张图片 {} KB 的上限。\
             找这张图的缩略图或更小的版本；不要用 Bash 下载后缩小再读。",
            img.bytes.len() / 1024,
            MAX_IMAGE_BYTES / 1024,
        ));
    }

    // 原图落盘给界面用。写不进就没有原图，界面退到压缩图 —— 图能看就行。
    // tool_use_id 全局唯一，天然不撞名。
    let path = ctx
        .artifacts_dir
        .join(format!("{}.{ext}", ctx.tool_use_id.as_str()));
    let path = ctx.fs.write(&path, &img.bytes).await.ok().map(|()| path);

    // 压不了（gif、损坏数据）就原样发，MAX_IMAGE_BYTES 已经兜过底。
    let (data, media_type) = match shrink::for_model(&img.bytes) {
        Some(s) => (s.data, s.media_type),
        None => (
            base64::engine::general_purpose::STANDARD.encode(&img.bytes),
            media_type,
        ),
    };

    let caption = ui_text!(
        "tools.webFetch.image",
        host = url.host_str().unwrap_or_default(),
        media = media_type,
        kb = img.bytes.len() / 1024
    );

    if ctx.vision.accepts_images() {
        return ToolOutcome::Ok {
            model_content: ToolResultContent::Image {
                media_type: media_type.into(),
                data,
                path,
            },
            ui_payload: Some(UiPayload::Message { text: caption }),
            side_messages: Vec::new(),
        };
    }

    // 看不了图的模型走视觉兼容。转述自带"当作亲眼所见、不暴露管道"的
    // 指示（宿主实现负责），这里只补上"描述的是哪个地址"。图片本体留给
    // 界面（DescribedImage）。
    match ctx
        .vision
        .describe(DescribeRequest {
            media_type: media_type.into(),
            data: data.clone(),
            focus: format!("调用方抓取这张图片是想：{prompt}"),
        })
        .await
    {
        Ok(text) => ToolOutcome::Ok {
            model_content: ToolResultContent::DescribedImage {
                media_type: media_type.into(),
                data,
                path,
                text: format!("{url}：\n{text}"),
            },
            ui_payload: Some(UiPayload::Message { text: caption }),
            side_messages: Vec::new(),
        },
        Err(e) => ToolOutcome::failed(format!(
            "{url} 是图片，但没能交给模型：{e}\n\
             不要试图用 Bash 下载或转换它 —— 问题不在图片，在于没有能看图的模型。"
        )),
    }
}

/// 把网络错误翻译成模型能据此改变行为的话。
///
/// `[约束]` 不要直接贴原始错误。"HTTP 403" 只会让模型换个 header 重试，
/// 而正确的下一步是换条路（用 gh CLI、或者告诉用户这页需要登录）。
fn fetch_hint(e: &WebError, url: &str) -> String {
    match e {
        WebError::NotConfigured { .. } => {
            "联网功能尚未启用。请让用户在「设置 → 联网」里打开网页抓取。".to_owned()
        }
        WebError::Blocked { reason } => {
            format!("{url} 被安全策略拦截：{reason}。不要重试，换一个公网地址。")
        }
        WebError::TooLarge { limit } => format!(
            "页面超过 {} MB 上限，无法处理。请换一个更具体的子页面。",
            limit / 1024 / 1024
        ),
        WebError::Status { code, body } => match code {
            401 | 403 => format!(
                "{url} 返回 {code}，这个页面需要登录或被拒绝访问。\
                 不要重试 —— 如果是 GitHub 上的内容，改用 `Bash(gh ...)`；\
                 否则告诉用户这个地址无法公开访问。"
            ),
            404 => format!(
                "{url} 不存在（404）。检查地址是否拼错，或者先用 WebSearch 找到正确的页面。"
            ),
            429 => format!("{url} 触发了限流（429）。换个信息来源，不要立刻重试。"),
            500..=599 => format!("{url} 服务端错误（{code}）。这是对方的问题，换个来源。"),
            _ => format!("{url} 返回 HTTP {code}：{body}"),
        },
        WebError::Transport { message } => {
            format!("连不上 {url}：{message}。检查地址是否正确，或者换个来源。")
        }
        WebError::Cancelled => "已取消。".to_owned(),
    }
}

fn schema_hint(e: &serde_json::Error) -> String {
    let raw = e.to_string();
    if raw.contains("missing field `url`") {
        return "缺少必需参数 `url`。请提供完整的网页地址。".to_owned();
    }
    if raw.contains("missing field `prompt`") {
        return "缺少必需参数 `prompt`。请说明你想从这个页面里得到什么。".to_owned();
    }
    if raw.contains("unknown field") {
        return format!("WebFetch 只接受 `url` 和 `prompt` 两个参数。（{raw}）");
    }
    format!("参数格式不对：{raw}。")
}
