//! 工具目录瘦身：延迟加载 + ToolSearch。
//!
//! # 问题
//!
//! MCP 服务器多了之后，工具描述和 schema 会吃掉可观的上下文 —— 它们进
//! 请求的 tools 数组，每一轮都付费，而大多数轮次一个都用不到。
//!
//! # 机制（Claude Code ToolSearchTool 的客户端版本）
//!
//! Claude Code 当前的实现靠 Anthropic 的服务端 beta（`defer_loading`
//! 剥离 + `tool_reference` 展开），Riot 走的是各家通用的 OpenAI 兼容
//! 协议，没有那两样。这里做纯客户端的等价物：
//!
//! - 延迟工具（`Tool::should_defer`，目前即 MCP 工具）不进 tools 数组，
//!   [`crate::scheduler::Scheduler::specs`] 把它们过滤掉；
//! - 模型从 ToolSearch 的描述里看到它们的**名字**；
//! - `ToolSearch(query: "select:<名字>")` 把完整定义（描述 + 参数 schema）
//!   作为工具结果文本返回，并把该工具标记为"已发现"；
//! - 已发现的工具从下一次请求起进 tools 数组，之后和普通工具无异；
//! - 未发现就直接调用会被调度器拦下，报错教模型先 ToolSearch ——
//!   模型没见过 schema，编出来的参数不可信（fail-closed）。
//!
//! `[约束]` 只在延迟候选的总量超过 [`DEFER_THRESHOLD_CHARS`] 时启用。
//! 只有两三个工具时，省下的上下文抵不过多一跳 ToolSearch 的往返 ——
//! Claude Code 的 auto 模式（阈值 = 窗口的 10%）是同一个判断。

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde::Deserialize;

use riot_protocol::tool::{PromptContext, Tool, ToolContext, ToolOutcome, UiPayload};

/// 启用延迟加载的门槛（延迟候选的描述 + schema 总字符数）。
///
/// 约合 1 万多 token —— 128k 窗口的 10% 上下（Claude Code auto 模式的
/// 默认比例）。低于它时全部工具直接进请求，没有 ToolSearch 这一跳。
pub const DEFER_THRESHOLD_CHARS: usize = 40_000;

/// 一个延迟工具的完整定义快照。构造时算好 —— prompt / schema 在一轮内
/// 不变，搜索和取回都不需要再碰工具本体。
struct Entry {
    name: String,
    description: String,
    schema_json: String,
    /// 小写的搜索文本（名字拆词 + 描述），打分用。
    search_name_parts: Vec<String>,
    search_description: String,
}

/// 本轮的延迟工具池 + 会话级的"已发现"集合。
///
/// 池每轮重建（工具集可能变了）；`discovered` 由会话持有、跨轮共享 ——
/// 模型这一轮加载过的工具，下一轮不该要求它再加载一遍。
pub struct DeferredPool {
    entries: Vec<Entry>,
    discovered: Arc<RwLock<HashSet<String>>>,
}

impl DeferredPool {
    /// 从工具集里挑出延迟候选建池。`discovered` 是会话级集合。
    pub fn new(
        tools: &[Arc<dyn Tool>],
        ctx: &PromptContext,
        discovered: Arc<RwLock<HashSet<String>>>,
    ) -> Self {
        let entries = tools
            .iter()
            .filter(|t| t.should_defer())
            .map(|t| {
                let name = t.name().to_owned();
                let description = t.prompt(ctx);
                let schema_json = serde_json::to_string(&t.input_schema())
                    .unwrap_or_else(|_| "{}".into());
                Entry {
                    search_name_parts: split_name(&name),
                    search_description: description.to_lowercase(),
                    name,
                    description,
                    schema_json,
                }
            })
            .collect();
        Self { entries, discovered }
    }

    /// 延迟候选的总体积。低于阈值就别启用 —— 省的不如多跳一次的贵。
    pub fn total_chars(&self) -> usize {
        self.entries
            .iter()
            .map(|e| e.name.len() + e.description.len() + e.schema_json.len())
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 这个工具此刻是否应该对模型**隐藏**（延迟中且还没被发现）。
    pub fn is_hidden(&self, name: &str) -> bool {
        self.entries.iter().any(|e| e.name == name)
            && !self
                .discovered
                .read()
                .expect("discovered 锁不该中毒")
                .contains(name)
    }

    fn discover(&self, name: &str) {
        self.discovered
            .write()
            .expect("discovered 锁不该中毒")
            .insert(name.to_owned());
    }

    fn names(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.name.as_str()).collect()
    }

    fn get(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name.eq_ignore_ascii_case(name))
    }
}

/// 把工具名拆成可搜索的词。`mcp__github__create_issue` →
/// ["mcp","github","create","issue"]；CamelCase 同理拆开。
fn split_name(name: &str) -> Vec<String> {
    let mut spaced = String::with_capacity(name.len() + 8);
    let mut prev_lower = false;
    for c in name.chars() {
        if c == '_' || c == '-' {
            spaced.push(' ');
            prev_lower = false;
        } else if c.is_ascii_uppercase() && prev_lower {
            spaced.push(' ');
            spaced.push(c.to_ascii_lowercase());
            prev_lower = false;
        } else {
            prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
            spaced.push(c.to_ascii_lowercase());
        }
    }
    spaced.split_whitespace().map(ToOwned::to_owned).collect()
}

#[derive(Deserialize, schemars::JsonSchema)]
struct Input {
    /// `select:<名字>`（可逗号分隔多个）精确取回；或若干关键词做搜索。
    query: String,
    /// 关键词搜索最多返回几个。默认 5。
    #[serde(default)]
    max_results: Option<usize>,
}

/// 见模块文档。
pub struct ToolSearch {
    pool: Arc<DeferredPool>,
}

impl ToolSearch {
    pub fn new(pool: Arc<DeferredPool>) -> Self {
        Self { pool }
    }

    /// 把命中的工具渲染成结果文本并标记为已发现。
    fn render_matches(&self, names: &[&str]) -> String {
        let mut out = String::from(
            "以下工具已加载，从下一步起可以直接调用（定义如下）：\n\n<functions>\n",
        );
        for name in names {
            let Some(e) = self.pool.get(name) else { continue };
            self.pool.discover(&e.name);
            // 和请求里 tools 数组的字段一致（name/description/parameters），
            // 模型见过这个形状。
            let line = serde_json::json!({
                "name": e.name,
                "description": e.description,
                "parameters": serde_json::from_str::<serde_json::Value>(&e.schema_json)
                    .unwrap_or(serde_json::Value::Null),
            });
            out.push_str(&line.to_string());
            out.push('\n');
        }
        out.push_str("</functions>");
        out
    }
}

#[async_trait]
impl Tool for ToolSearch {
    fn name(&self) -> &str {
        "ToolSearch"
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        let mut p = String::from(
            "取回延迟工具的完整定义。下面这些工具**存在但还没加载**——\
             只有名字，没有参数 schema，加载之前不能直接调用：\n\n",
        );
        for name in self.pool.names() {
            p.push_str("- ");
            p.push_str(name);
            p.push('\n');
        }
        p.push_str(
            "\n用法：\n\
             - `select:名字` 或 `select:名字1,名字2` —— 按名字精确取回\n\
             - 若干关键词 —— 按名字和描述搜索，返回最匹配的几个\n\n\
             结果里 <functions> 块内的工具从下一步起可以直接调用。\
             需要用哪个就先取哪个，不要凭名字猜参数。",
        );
        p
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let q = input.get("query").and_then(|v| v.as_str()).unwrap_or("?");
        format!("查找工具：{q}")
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn call(&self, input: serde_json::Value, _ctx: ToolContext) -> ToolOutcome {
        let input: Input = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutcome::failed(format!("参数不对：{e}")),
        };
        let query = input.query.trim();
        let max_results = input.max_results.unwrap_or(5).clamp(1, 20);

        // select: 精确取回（支持逗号分隔多选）。
        // 名字打错时报可用清单 —— 不报的话模型只会换个错法再试。
        if let Some(rest) = query.strip_prefix("select:") {
            let mut found: Vec<&str> = Vec::new();
            let mut missing: Vec<&str> = Vec::new();
            for raw in rest.split(',') {
                let raw = raw.trim();
                if raw.is_empty() {
                    continue;
                }
                match self.pool.get(raw) {
                    Some(e) => found.push(e.name.as_str()),
                    None => missing.push(raw),
                }
            }
            if found.is_empty() {
                return ToolOutcome::failed(format!(
                    "没找到：{}。可用的延迟工具：{}",
                    missing.join("、"),
                    self.pool.names().join("、"),
                ));
            }
            let names: Vec<&str> = found.clone();
            let mut text = self.render_matches(&names);
            if !missing.is_empty() {
                text.push_str(&format!("\n\n（没找到：{}）", missing.join("、")));
            }
            return ToolOutcome::Ok {
                ui_payload: Some(UiPayload::Plain {
                    text: format!("已加载 {} 个工具：{}", names.len(), names.join("、")),
                }),
                model_content: riot_protocol::message::ToolResultContent::text(text),
                side_messages: Vec::new(),
            };
        }

        // 裸名字精确命中的快速通道：模型（尤其是压缩后）经常直接用名字
        // 而不带 select: 前缀，能认出来就别逼它重试。
        if let Some(e) = self.pool.get(query) {
            let name = e.name.clone();
            let text = self.render_matches(&[name.as_str()]);
            return ToolOutcome::Ok {
                ui_payload: Some(UiPayload::Plain { text: format!("已加载工具：{name}") }),
                model_content: riot_protocol::message::ToolResultContent::text(text),
                side_messages: Vec::new(),
            };
        }

        // 关键词搜索：名字整词命中权重最高，名字部分命中次之，描述再次。
        let terms: Vec<String> = query
            .to_lowercase()
            .split_whitespace()
            .map(ToOwned::to_owned)
            .collect();
        if terms.is_empty() {
            return ToolOutcome::failed(format!(
                "query 是空的。用 select:名字 精确取回，或给关键词。可用：{}",
                self.pool.names().join("、"),
            ));
        }

        let mut scored: Vec<(usize, &Entry)> = self
            .pool
            .entries
            .iter()
            .map(|e| {
                let mut score = 0usize;
                for term in &terms {
                    if e.search_name_parts.iter().any(|p| p == term) {
                        score += 10;
                    } else if e.search_name_parts.iter().any(|p| p.contains(term.as_str())) {
                        score += 5;
                    }
                    if e.search_description.contains(term.as_str()) {
                        score += 2;
                    }
                }
                (score, e)
            })
            .filter(|(s, _)| *s > 0)
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));

        if scored.is_empty() {
            return ToolOutcome::failed(format!(
                "没有匹配「{query}」的工具。可用的延迟工具：{}。\
                 换关键词再搜，或直接 select: 其中一个。",
                self.pool.names().join("、"),
            ));
        }

        let names: Vec<&str> = scored
            .iter()
            .take(max_results)
            .map(|(_, e)| e.name.as_str())
            .collect();
        let text = self.render_matches(&names);
        ToolOutcome::Ok {
            ui_payload: Some(UiPayload::Plain {
                text: format!("已加载 {} 个工具：{}", names.len(), names.join("、")),
            }),
            model_content: riot_protocol::message::ToolResultContent::text(text),
            side_messages: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FixedClock, NullFileState, NullFs, NullProc};
    use riot_protocol::id::{SessionId, ToolUseId};
    use riot_protocol::permission::PermissionResult;
    use riot_protocol::tool::ProgressSink;
    use std::path::PathBuf;

    /// 一个可配置的假延迟工具。
    struct Deferred {
        name: String,
        desc: String,
    }

    #[async_trait]
    impl Tool for Deferred {
        fn name(&self) -> &str {
            &self.name
        }
        fn input_schema(&self) -> schemars::Schema {
            schemars::json_schema!({ "type": "object", "properties": { "x": { "type": "string" } } })
        }
        fn prompt(&self, _: &PromptContext) -> String {
            self.desc.clone()
        }
        fn describe(&self, _: &serde_json::Value) -> String {
            "d".into()
        }
        fn should_defer(&self) -> bool {
            true
        }
        fn check_permissions(
            &self,
            _: &serde_json::Value,
            _: &riot_protocol::permission::PermissionContext,
        ) -> PermissionResult {
            PermissionResult::Passthrough
        }
        async fn call(&self, _: serde_json::Value, _: ToolContext) -> ToolOutcome {
            ToolOutcome::ok_text("ran")
        }
    }

    fn tool(name: &str, desc: &str) -> Arc<dyn Tool> {
        Arc::new(Deferred { name: name.into(), desc: desc.into() })
    }

    fn prompt_ctx() -> PromptContext {
        PromptContext {
            cwd: PathBuf::from("/w"),
            platform: "test".into(),
            sibling_tools: Vec::new(),
            today: "2026年8月".into(),
        }
    }

    fn pool(tools: &[Arc<dyn Tool>]) -> Arc<DeferredPool> {
        Arc::new(DeferredPool::new(
            tools,
            &prompt_ctx(),
            Arc::new(RwLock::new(HashSet::new())),
        ))
    }

    fn ctx() -> ToolContext {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let id = ToolUseId::from_raw("t1");
        ToolContext {
            session_id: SessionId::from_raw("s1"),
            tool_use_id: id.clone(),
            cwd: "/w".into(),
            artifacts_dir: "/a".into(),
            cancel: tokio_util::sync::CancellationToken::new(),
            progress: ProgressSink::new(id, tx),
            file_state: Arc::new(NullFileState),
            fs: Arc::new(NullFs),
            proc: Arc::new(NullProc),
            web: Arc::new(riot_protocol::web::NoWeb),
            browser: Arc::new(riot_protocol::browser::NoBrowser),
            terminal: Arc::new(riot_protocol::terminal::NoTerminal),
            vision: Arc::new(riot_protocol::vision::NoVision),
            clock: Arc::new(FixedClock::default()),
        }
    }

    fn ok_text(out: ToolOutcome) -> String {
        match out {
            ToolOutcome::Ok { model_content, .. } => format!("{model_content:?}"),
            other => panic!("该成功：{other:?}"),
        }
    }

    #[tokio::test]
    async fn select_取回定义并标记已发现() {
        let tools = [tool("mcp__gh__create_issue", "在 GitHub 上建 issue")];
        let p = pool(&tools);
        assert!(p.is_hidden("mcp__gh__create_issue"), "取回前对模型隐藏");

        let ts = ToolSearch::new(Arc::clone(&p));
        let text = ok_text(
            ts.call(serde_json::json!({ "query": "select:mcp__gh__create_issue" }), ctx())
                .await,
        );
        assert!(text.contains("create_issue"), "要有名字");
        assert!(text.contains("GitHub"), "要有描述");
        assert!(text.contains("properties"), "要有参数 schema：{text}");
        assert!(!p.is_hidden("mcp__gh__create_issue"), "取回后不再隐藏");
    }

    #[tokio::test]
    async fn select_支持多选和部分命中() {
        let tools = [tool("mcp__a__x", "da"), tool("mcp__b__y", "db")];
        let p = pool(&tools);
        let ts = ToolSearch::new(Arc::clone(&p));

        let text = ok_text(
            ts.call(serde_json::json!({ "query": "select:mcp__a__x, mcp__nope__z" }), ctx())
                .await,
        );
        assert!(text.contains("da"), "命中的要返回");
        assert!(text.contains("没找到") && text.contains("mcp__nope__z"), "缺的要点名：{text}");
        assert!(!p.is_hidden("mcp__a__x"));
        assert!(p.is_hidden("mcp__b__y"), "没选的不该被顺带发现");
    }

    #[tokio::test]
    async fn 裸名字精确命中不需要前缀() {
        // 压缩后模型经常直接给名字。认不出来的话它要多一轮重试。
        let tools = [tool("mcp__gh__create_issue", "d")];
        let p = pool(&tools);
        let ts = ToolSearch::new(Arc::clone(&p));
        ok_text(ts.call(serde_json::json!({ "query": "mcp__gh__create_issue" }), ctx()).await);
        assert!(!p.is_hidden("mcp__gh__create_issue"));
    }

    #[tokio::test]
    async fn 关键词搜索按名字和描述打分() {
        let tools = [
            tool("mcp__gh__create_issue", "在仓库里创建 issue"),
            tool("mcp__gh__list_repos", "列出仓库"),
            tool("mcp__slack__send_message", "发 Slack 消息"),
        ];
        let p = pool(&tools);
        let ts = ToolSearch::new(Arc::clone(&p));

        let text = ok_text(
            ts.call(serde_json::json!({ "query": "issue create", "max_results": 1 }), ctx())
                .await,
        );
        assert!(text.contains("create_issue"), "名字整词双命中该排第一：{text}");
        assert!(!text.contains("slack"), "无关的不该进结果");
    }

    #[tokio::test]
    async fn 搜不到时报可用清单() {
        let tools = [tool("mcp__a__x", "d")];
        let ts = ToolSearch::new(pool(&tools));
        match ts.call(serde_json::json!({ "query": "毫无关系的词" }), ctx()).await {
            ToolOutcome::Failed { error_for_model, .. } => {
                assert!(
                    error_for_model.contains("mcp__a__x"),
                    "要带清单，否则模型只会换个错词再搜：{error_for_model}"
                );
            }
            other => panic!("该失败：{other:?}"),
        }
    }

    #[tokio::test]
    async fn 发现状态跨池共享() {
        // discovered 是会话级的：下一轮重建池之后，上一轮加载过的工具
        // 必须还是"已发现"—— 否则模型每轮都要重新加载一遍。
        let discovered = Arc::new(RwLock::new(HashSet::new()));
        let tools = [tool("mcp__a__x", "d")];

        let p1 = Arc::new(DeferredPool::new(&tools, &prompt_ctx(), Arc::clone(&discovered)));
        ToolSearch::new(Arc::clone(&p1))
            .call(serde_json::json!({ "query": "select:mcp__a__x" }), ctx())
            .await;

        let p2 = DeferredPool::new(&tools, &prompt_ctx(), discovered);
        assert!(!p2.is_hidden("mcp__a__x"), "新一轮的池要认上一轮的发现");
    }

    #[test]
    fn 名字清单进_prompt_定义不进() {
        let tools = [tool("mcp__gh__create_issue", "这段描述很长不该出现在清单里")];
        let ts = ToolSearch::new(pool(&tools));
        let p = ts.prompt(&prompt_ctx());
        assert!(p.contains("mcp__gh__create_issue"), "名字要在");
        assert!(!p.contains("这段描述"), "描述绝不能进清单 —— 那正是要省的东西");
    }

    #[test]
    fn 拆词覆盖_mcp_和驼峰() {
        assert_eq!(split_name("mcp__gh__create_issue"), vec!["mcp", "gh", "create", "issue"]);
        assert_eq!(split_name("WebSearch"), vec!["web", "search"]);
    }

    #[test]
    fn 阈值统计包含名字描述和schema() {
        let tools = [tool("mcp__a__x", "描述")];
        let p = pool(&tools);
        assert!(p.total_chars() > "mcp__a__x描述".len(), "schema 也要算进去");
    }
}
