//! WebSearch 工具。查搜索后端，把结果整理成模型能引用的形式。
//!
//! # 为什么不自动抓取搜索结果的正文
//!
//! 这是本文件最重要的一个决定。自动抓取 top N 看起来更"一步到位"，但
//! 搜索结果可以指向**任何**域名 —— 自动抓就等于绕开了 WebFetch 的域名
//! 权限，用户对 `docs.rs` 的授权会变成对搜索引擎返回的任意站点的授权。
//! 那正是整套权限模型要堵的那条外带通道。
//!
//! 所以这里只返回标题、URL、摘要。模型看完摘要自己决定读哪一篇，再调
//! WebFetch —— 那一次会正常征求用户对那个域名的同意。附带的好处是快得多，
//! 一次搜索只有一个请求，而不是一个加 N 个。
//!
//! 例外是后端**自己**带回来的正文（Tavily、Exa 这类为 LLM 设计的后端会给
//! `raw_content`）。那是用户亲手配的后端在它自己的响应里给的，不是我们
//! 额外发起的抓取，所以直接用。
//!
//! # 和 Claude Code 的差别
//!
//! Claude 的 WebSearch 把搜索整个外包给 Anthropic 服务端的
//! `web_search_20250305` 工具，客户端只解析回来的 block 流；它的
//! `isEnabled()` 对非官方渠道直接返回 false。我们接的是 DeepSeek、Kimi、
//! 本地 Ollama 这些没有服务端搜索能力的模型，所以搜索必须自己做。

use async_trait::async_trait;
use riot_protocol::message::ToolResultContent;
use riot_protocol::permission::{
    DecisionReason, PermissionContext, PermissionResult, PermissionUpdate, RuleDecision,
    UpdateScope,
};
use riot_protocol::tool::{
    InterruptBehavior, PromptContext, ResultBudget, Tool, ToolContext, ToolOutcome, UiPayload,
    ValidationError,
};
use riot_protocol::web::{SearchHit, SearchQuery, WebError};
use serde::Deserialize;

use super::markdown;

pub const WEB_SEARCH: &str = "WebSearch";

/// 默认返回条数。
///
/// 十条足够模型判断"该读哪一篇"，再多只是烧上下文 —— 搜索结果第 15 条
/// 之后的相关性通常已经掉到没用了。
const DEFAULT_MAX_RESULTS: usize = 10;
const HARD_MAX_RESULTS: usize = 20;

/// 后端自带正文时，单条正文的字符上限。
///
/// 十条 × 不限长的正文能轻松把上下文撑爆。
const RAW_CONTENT_CHARS: usize = 4_000;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    /// 搜索词。
    query: String,
    /// 只要这些域名的结果，如 `["docs.rs", "github.com"]`。
    #[serde(default)]
    allowed_domains: Option<Vec<String>>,
    /// 排除这些域名的结果。
    #[serde(default)]
    blocked_domains: Option<Vec<String>>,
    /// 最多返回几条，默认 10。
    #[serde(default)]
    max_results: Option<usize>,
}

pub struct WebSearch;

#[async_trait]
impl Tool for WebSearch {
    fn name(&self) -> &'static str {
        WEB_SEARCH
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, ctx: &PromptContext) -> String {
        format!(
            "联网搜索，返回标题、链接和摘要。\n\
             \n\
             - 用来查你的知识截止日期之后的信息：新版本、新 API、最近的\
             变更和报错。\n\
             - **当前是 {today}。**搜索最新信息时必须用这个年份，\
             不要用你印象里的年份 —— 例如要查最新的构建配置，\
             搜索词里写 {year}，不要写更早的年份。\n\
             - 这个工具只给摘要，**不返回网页正文**。看完摘要选中某一篇后，\
             用 WebFetch 抓那个链接读全文。\n\
             - `allowed_domains` 和 `blocked_domains` 不能同时给。\n\
             - 想读一个你已经知道地址的页面，直接用 WebFetch，不用先搜。\n\
             \n\
             回答用户时，必须在末尾用 markdown 链接列出引用到的来源：\n\
             \n\
             来源：\n\
             - [标题](https://example.com/a)",
            today = ctx.today,
            year = ctx.today.split('年').next().unwrap_or(&ctx.today),
        )
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        match input.get("query").and_then(|v| v.as_str()) {
            Some(q) => format!("搜索 {q}"),
            None => "联网搜索".to_owned(),
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn interrupt_behavior(&self) -> InterruptBehavior {
        InterruptBehavior::Cancel
    }

    fn result_budget(&self) -> ResultBudget {
        ResultBudget::Unlimited
    }

    fn classifier_input(&self, input: &serde_json::Value) -> Option<String> {
        input
            .get("query")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned)
    }

    /// 整工具粒度，不分域名 —— 搜索只往用户自己配的那一个后端发请求。
    ///
    /// 仍然要问一次：搜索词是用户对话内容的一部分，发给第三方服务这件事
    /// 值得让用户知道。点一次"总是允许"之后就不再打扰。
    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        ctx: &PermissionContext,
    ) -> PermissionResult {
        let rules = riot_permissions::RuleSet::new(ctx.rules.clone());
        if let Some(r) = rules.tool_rule(WEB_SEARCH, RuleDecision::Allow) {
            return PermissionResult::Allow {
                updated_input: None,
                reason: DecisionReason::Rule {
                    source: r.source,
                    pattern: WEB_SEARCH.to_owned(),
                },
            };
        }

        PermissionResult::Ask {
            message: "是否允许联网搜索？搜索词会发送给你配置的搜索后端。".to_owned(),
            suggestions: vec![PermissionUpdate::AddRule {
                tool: WEB_SEARCH.to_owned(),
                pattern: None,
                decision: RuleDecision::Allow,
                scope: UpdateScope::Session,
            }],
            // `Consent` 而非 `Rule`：没有规则命中，这只是默认要问一次。
            // 冒充成规则会让「全部放行」对搜索失效。见 chain::decide 第 3 步。
            reason: DecisionReason::Consent {
                what: WEB_SEARCH.to_owned(),
            },
        }
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        let parsed: Input = serde_json::from_value(input.clone())
            .map_err(|e| ValidationError::rejected(schema_hint(&e)))?;

        if parsed.query.trim().len() < 2 {
            return Err(ValidationError::rejected(
                "`query` 太短了。给一个具体的搜索词，比如「tokio select 宏 用法」。",
            ));
        }

        // 两个都给的时候语义是矛盾的。静默挑一个的话，用户会看到
        // 一个"过滤没生效"的结果而不知道为什么。
        let has = |v: &Option<Vec<String>>| v.as_ref().is_some_and(|d| !d.is_empty());
        if has(&parsed.allowed_domains) && has(&parsed.blocked_domains) {
            return Err(ValidationError::rejected(
                "`allowed_domains` 和 `blocked_domains` 不能同时使用，只留一个。",
            ));
        }
        Ok(())
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let parsed: Input = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolOutcome::failed(schema_hint(&e)),
        };

        let started = ctx.clock.now_ms();
        let query = parsed.query.trim().to_owned();

        let hits = match ctx
            .web
            .search(
                SearchQuery {
                    query: query.clone(),
                    max_results: parsed
                        .max_results
                        .unwrap_or(DEFAULT_MAX_RESULTS)
                        .clamp(1, HARD_MAX_RESULTS),
                    allowed_domains: parsed.allowed_domains.unwrap_or_default(),
                    blocked_domains: parsed.blocked_domains.unwrap_or_default(),
                },
                &ctx.cancel,
            )
            .await
        {
            Ok(h) => h,
            Err(WebError::Cancelled) => return ToolOutcome::Cancelled,
            Err(e) => return ToolOutcome::failed(search_hint(&e)),
        };

        if hits.is_empty() {
            return ToolOutcome::ok_text(format!(
                "没有搜到「{query}」的结果。\n\
                 换个更通用的搜索词试试，或者去掉域名过滤。\
                 中文搜不到的话，用英文关键词往往有效得多。"
            ));
        }

        let elapsed = ctx.clock.now_ms().saturating_sub(started);
        let text = format_results(&query, &hits, elapsed);

        ToolOutcome::Ok {
            model_content: ToolResultContent::text(text.clone()),
            ui_payload: Some(UiPayload::Plain { text }),
            side_messages: Vec::new(),
        }
    }
}

/// 把搜索结果排版成模型好引用的形式。
fn format_results(query: &str, hits: &[SearchHit], elapsed_ms: u64) -> String {
    let mut out = format!(
        "「{query}」的搜索结果（{} 条，耗时 {:.1}s）：\n\n",
        hits.len(),
        elapsed_ms as f64 / 1000.0
    );

    for (i, h) in hits.iter().enumerate() {
        out.push_str(&format!("{}. [{}]({})\n", i + 1, h.title.trim(), h.url));
        if !h.snippet.trim().is_empty() {
            out.push_str(&format!("   {}\n", h.snippet.trim().replace('\n', " ")));
        }
        if let Some(raw) = &h.raw_content {
            let body = markdown::truncate(raw.trim(), RAW_CONTENT_CHARS);
            if !body.is_empty() {
                out.push_str(&format!("   ---\n   {}\n", body.replace('\n', "\n   ")));
            }
        }
        out.push('\n');
    }

    // 这两句是必要的。少了第一句，模型会拿摘要当全文，然后回答得比它
    // 实际知道的更笃定；少了第二句，用户拿不到可以自己核实的链接。
    out.push_str(
        "以上只是摘要。需要细节就用 WebFetch 抓对应链接读全文，不要仅凭摘要下结论。\n\
         回答用户时必须在末尾用 markdown 链接列出引用到的来源。",
    );
    out
}

fn search_hint(e: &WebError) -> String {
    match e {
        WebError::NotConfigured { .. } => {
            "还没有配置搜索后端。请让用户打开「设置 → 联网」，打开搜索开关\
             并填入 SearXNG 地址，配好之后再重试。在此之前不要反复调用这个工具。"
                .to_owned()
        }
        WebError::Status { code, body } => format!(
            "搜索后端返回 HTTP {code}：{body}。\
             可能是地址配错了或者后端没开启 JSON 输出。告诉用户去检查设置，不要重试。"
        ),
        WebError::Transport { message } => format!(
            "连不上搜索后端：{message}。让用户检查「设置 → 联网」里的地址，不要重试。"
        ),
        WebError::Blocked { reason } => format!("搜索请求被拦截：{reason}。"),
        WebError::TooLarge { .. } => "搜索后端返回的内容过大，已放弃。".to_owned(),
        WebError::Cancelled => "已取消。".to_owned(),
    }
}

fn schema_hint(e: &serde_json::Error) -> String {
    let raw = e.to_string();
    if raw.contains("missing field `query`") {
        return "缺少必需参数 `query`。请提供搜索词。".to_owned();
    }
    if raw.contains("unknown field") {
        return format!(
            "WebSearch 接受的参数是 `query`、`allowed_domains`、\
             `blocked_domains`、`max_results`。要抓取某个具体网址请用 WebFetch。（{raw}）"
        );
    }
    format!("参数格式不对：{raw}。")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(title: &str, url: &str, snippet: &str) -> SearchHit {
        SearchHit {
            title: title.into(),
            url: url.into(),
            snippet: snippet.into(),
            raw_content: None,
        }
    }

    #[test]
    fn 结果排版成可引用的链接() {
        let out = format_results("tokio select", &[hit("Tokio", "https://tokio.rs", "摘要")], 1200);
        assert!(out.contains("[Tokio](https://tokio.rs)"), "{out}");
        assert!(out.contains("摘要"), "{out}");
        assert!(out.contains("1.2s"), "{out}");
    }

    #[test]
    fn 提醒模型摘要不等于全文() {
        // 少了这句，模型会拿一句摘要当全文，回答得比它实际知道的更笃定
        let out = format_results("q", &[hit("T", "https://a.com", "")], 0);
        assert!(out.contains("只是摘要"), "{out}");
        assert!(out.contains("WebFetch"), "{out}");
        assert!(out.contains("来源"), "{out}");
    }

    #[test]
    fn 后端自带的正文会截断() {
        let mut h = hit("T", "https://a.com", "s");
        h.raw_content = Some("正".repeat(RAW_CONTENT_CHARS + 500));
        let out = format_results("q", &[h], 0);
        assert!(out.contains("截断"), "超长正文必须截断：{}", &out[..200.min(out.len())]);
    }

    #[test]
    fn 空摘要不留空行() {
        let out = format_results("q", &[hit("T", "https://a.com", "  ")], 0);
        assert!(!out.contains("\n   \n"), "{out}");
    }
}
