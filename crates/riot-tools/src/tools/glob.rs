//! Glob 工具。按**文件名**找文件。
//!
//! 和 [`super::grep`] 的分工：Grep 搜内容，Glob 搜路径。没有这个工具的话，
//! "项目里的 Python 文件在哪"只能靠 `Bash(find ...)`—— 那要过一遍命令权限
//! 弹窗，还绕开了 .gitignore。
//!
//! 底层是 ripgrep 的遍历库（`ignore`），不是它的二进制 —— 理由见
//! [`super::search`] 的模块文档。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use riot_protocol::permission::{PermissionContext, PermissionResult};
use riot_protocol::tool::{
    PromptContext, ResultBudget, Tool, ToolContext, ToolOutcome, UiPayload, ValidationError,
};
use serde::Deserialize;

use super::names::{BASH, GLOB, GREP, READ};
use super::{path, search};

/// 一次遍历最多看多少个文件。理由同 Grep：超过这个数说明范围没圈对。
const MAX_FILES: usize = 100_000;

/// 返回给模型的路径条数上限。
///
/// 比 Grep 的 500 行小：路径列表的信息密度低得多，300 条已经足够
/// 判断"东西大概在哪儿"，再多只是烧上下文。
const MAX_RESULTS: usize = 300;

/// 愿意为排序花掉的 stat 次数。
///
/// 超过这个数就退回字典序 —— 几千次 stat 的耗时会盖过遍历本身，
/// 而结果多到这个程度时模型该做的是缩小 pattern，不是仔细读列表。
const STAT_BUDGET: usize = 1000;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    /// Filename pattern, such as `**/*.rs` or `src/**/test_*.py`.
    pattern: String,
    /// Root directory to search from. Omit to search the working directory.
    #[serde(default)]
    path: Option<String>,
    /// Maximum number of paths to return.
    #[serde(default)]
    head_limit: Option<usize>,
}

pub struct Glob;

#[async_trait]
impl Tool for Glob {
    fn name(&self) -> &'static str {
        GLOB
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        format!(
            "Finds files by name. Returns a list of paths, most recently modified first.\n\
             \n\
             Usage:\n\
             - `pattern` is a glob, not a regex: `**/*.rs` is every Rust file, \
             `src/**/*.ts` only the ones under src, `**/test_*.py` only files whose \
             name starts with test_. `.` is a literal dot here.\n\
             - ALWAYS prefer this over `{BASH}(\"find …\")` or `ls -R`: no command \
             approval, and .gitignore'd files and .git are skipped, so you do not get \
             10000 paths out of `node_modules` or `target`.\n\
             - At most {MAX_RESULTS} paths come back. Hitting that cap means the pattern \
             was too broad — narrow it or set `path` to a subtree.\n\
             - Recency ordering is the useful part when you are looking for the file \
             someone just touched.\n\
             \n\
             ### When to Use\n\
             \n\
             1. You know roughly what the file is called or what extension it has.\n\
             2. Getting a feel for a project's layout: `**/*.toml`, `**/Dockerfile`, \
             `src/**/mod.rs`.\n\
             3. Enumerating the files a change has to cover, when membership is decided \
             by path rather than by content.\n\
             \n\
             ### When NOT to Use\n\
             \n\
             1. Looking for a symbol, a string, or any file *content* — that is {GREP}. \
             File names rarely tell you where a function lives.\n\
             2. As a substitute for reading. A path list is not evidence about what the \
             code does; follow up with {GREP} or {READ}.\n\
             3. Broad sweeps like `**/*` or `**` to \"see the project\". That returns \
             {MAX_RESULTS} arbitrary paths and answers nothing. Ask a narrower question.\n\
             4. Checking whether one specific path exists. Just {READ} it; the error \
             tells you, in one call instead of two.\n\
             5. NEVER shell out to `find`, `ls -R`, or `git ls-files` for this. Those \
             also ignore the repository's ignore rules and need command approval.\n\
             \n\
             <good-example>\n\
             {GLOB}(pattern: \"**/migrations/*.sql\")\n\
             </good-example>\n\
             <reasoning>\n\
             Membership really is decided by path here, and the recency ordering puts \
             the newest migration first — which is usually the one being asked about.\n\
             </reasoning>\n\
             \n\
             <bad-example>\n\
             {GLOB}(pattern: \"**/*auth*\") … then {READ} on each hit, looking for the \
             login handler\n\
             </bad-example>\n\
             <reasoning>\n\
             The handler may live in `session.rs` and never appear in a filename. \
             `{GREP}(pattern: \"fn login\")` finds it directly and gives you the line \
             number too.\n\
             </reasoning>"
        )
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let pat = input
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or("...");
        match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => format!("在 {p} 里查找 {pat}"),
            None => format!("查找 {pat}"),
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    /// 结果在工具内部就截断到 [`MAX_RESULTS`] 了，不需要外层的落盘机制。
    fn result_budget(&self) -> ResultBudget {
        ResultBudget::Unlimited
    }

    fn target_path(&self, input: &serde_json::Value) -> Option<PathBuf> {
        input
            .get("path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        // 交给通用决策链。理由同 Grep：列出 `~/.ssh` 下的文件名同样是
        // 一次读取，这里表态会绕过 safety 层对凭证目录的拦截。
        PermissionResult::Passthrough
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        let parsed: Input = serde_json::from_value(input.clone())
            .map_err(|e| ValidationError::rejected(schema_hint(&e)))?;

        if parsed.pattern.trim().is_empty() {
            return Err(ValidationError::rejected(
                "`pattern` 不能为空。要列出目录下所有文件用 `**/*`。",
            ));
        }
        Ok(())
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let parsed: Input = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolOutcome::failed(schema_hint(&e)),
        };

        // 搜索根要过围栏。不查的话 `path: "../../"` 就能列出工作目录外面。
        let root = match &parsed.path {
            Some(p) => match path::resolve(p, &ctx, true).await {
                Ok(r) => r,
                Err(e) => return ToolOutcome::failed(e.for_model()),
            },
            None => ctx.cwd.clone(),
        };

        // 遍历是同步的重活，扔给阻塞线程池（理由同 Grep）。
        let cancel = ctx.cancel.clone();
        let deadline = search::Deadline::new(Arc::clone(&ctx.clock), search::TIME_BUDGET_SECS);
        let pattern = parsed.pattern.clone();
        let walked = tokio::task::spawn_blocking(move || {
            search::walk(&root, Some(&pattern), MAX_FILES, &cancel, &deadline)
        })
        .await;

        let walked = match walked {
            Ok(Ok(w)) => w,
            Ok(Err(e)) => return ToolOutcome::failed(e),
            Err(e) => return ToolOutcome::failed(format!("遍历没能完成：{e}")),
        };

        // `[约束]` "没找到"不是失败。报成失败会让模型换个参数把同一件事
        // 再做一遍，而正确的下一步是换个思路或接受它。
        if walked.files.is_empty() {
            return ToolOutcome::ok_text(no_match_text(&parsed));
        }

        let mut files: Vec<String> = walked
            .files
            .iter()
            .map(|p| p.display().to_string())
            .collect();

        let total = files.len();
        let by_mtime = sort_by_mtime(&mut files, &ctx).await;

        let limit = parsed.head_limit.unwrap_or(MAX_RESULTS).min(MAX_RESULTS);
        files.truncate(limit);
        let shown = files.len();

        let listing = files.join("\n");
        let mut body = listing.clone();

        if shown < total {
            body.push_str(&format!(
                "\n\n<system-reminder>共 {total} 个文件，这里显示前 {shown} 个（{}）。\
                 让 `pattern` 更具体，或用 `path` 缩小到某个子目录。</system-reminder>",
                if by_mtime {
                    "按最近修改排序"
                } else {
                    "文件太多，按路径排序"
                }
            ));
        }

        ToolOutcome::Ok {
            model_content: riot_protocol::message::ToolResultContent::text(body),
            ui_payload: Some(UiPayload::Plain { text: listing }),
            side_messages: Vec::new(),
        }
    }
}

/// 按最近修改排序，返回是否真的排成了。
///
/// 最近改过的文件几乎总是当前任务相关的那些 —— 结果被截断时，这个顺序
/// 决定了模型看到的是不是有用的那一批。文件太多就退回字典序：
/// 那时 stat 的开销盖过收益，而且字典序至少是稳定可预期的。
async fn sort_by_mtime(files: &mut [String], ctx: &ToolContext) -> bool {
    if files.len() > STAT_BUDGET {
        files.sort();
        return false;
    }

    let mut stamped: Vec<(u64, &String)> = Vec::with_capacity(files.len());
    for f in files.iter() {
        // stat 失败（刚被删掉、权限不够）当作最旧，排在后面。
        // 为一个拿不到时间的文件放弃整次排序不划算。
        let mtime = ctx
            .fs
            .metadata(std::path::Path::new(f))
            .await
            .map(|m| m.mtime_ms)
            .unwrap_or(0);
        stamped.push((mtime, f));
    }

    // 次序按路径兜底：同一秒写出的一批文件（生成代码、checkout）
    // 否则顺序不稳，同样的调用两次给出不同结果。
    stamped.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));

    let sorted: Vec<String> = stamped.into_iter().map(|(_, f)| f.clone()).collect();
    files.clone_from_slice(&sorted);
    true
}

fn no_match_text(input: &Input) -> String {
    let where_ = input.path.as_deref().unwrap_or("工作目录");
    format!(
        "在{where_}里没有找到匹配 `{}` 的文件。\n\
         注意 .gitignore 里的文件和 .git 目录不会被列出。\n\
         `pattern` 是 glob 不是正则 —— 找所有 Rust 文件应该写 `**/*.rs`。",
        input.pattern
    )
}

fn schema_hint(e: &serde_json::Error) -> String {
    let raw = e.to_string();
    if raw.contains("missing field `pattern`") {
        return "缺少必需参数 `pattern`。请提供文件名匹配模式，如 `**/*.rs`。".to_owned();
    }
    if raw.contains("unknown field") {
        return format!(
            "Glob 接受的参数是 `pattern`、`path`、`head_limit`。\
             搜索文件内容请改用 Grep。（{raw}）"
        );
    }
    format!("参数格式不对：{raw}。请检查参数类型。")
}
