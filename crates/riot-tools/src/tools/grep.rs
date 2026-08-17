//! Grep 工具。
//!
//! 底层是 ripgrep 的**库**（`grep-searcher` / `ignore`），不是它的
//! 二进制 —— 理由见 [`super::search`] 的模块文档：桌面应用不能假设
//! 用户装了 rg、而且它恰好在 PATH 里。
//!
//! 顺带解决了命令注入这一整类问题：没有子进程，模型给的 pattern 里
//! 有 `$(...)` 还是 `;` 都只是正则字符，不需要任何转义。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use riot_protocol::permission::{PermissionContext, PermissionResult};
use riot_protocol::tool::{
    PromptContext, ResultBudget, Tool, ToolContext, ToolOutcome, UiPayload, ValidationError,
};
use serde::Deserialize;

use super::{path, search};

/// 一次搜索最多看多少个文件。
///
/// 超过这个数说明范围没圈对（搜到了 node_modules 或者整个 home）。
/// 带着已有结果收工，并让模型知道结果不完整。
const MAX_FILES: usize = 100_000;

/// 返回给模型的字符上限。
const MAX_CHARS: usize = 30_000;

/// 返回给模型的行数上限。
const MAX_LINES: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, schemars::JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
enum OutputMode {
    /// 显示匹配的行。
    #[default]
    Content,
    /// 只显示有匹配的文件路径。
    FilesWithMatches,
    /// 每个文件的匹配次数。
    Count,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    /// 正则表达式，Rust regex 语法。
    pattern: String,
    /// 搜索的目录或文件。省略则搜索工作目录。
    #[serde(default)]
    path: Option<String>,
    /// 文件名过滤，如 `*.rs` 或 `**/test/*.py`。
    #[serde(default)]
    glob: Option<String>,
    /// 输出形式。默认 `content`。
    #[serde(default)]
    output_mode: OutputMode,
    /// 忽略大小写。
    #[serde(default)]
    case_insensitive: bool,
    /// 匹配行前后各带几行上下文。只在 `content` 模式下有效。
    #[serde(default)]
    context_lines: Option<usize>,
    /// 最多返回几条结果。
    #[serde(default)]
    head_limit: Option<usize>,
}

pub struct Grep;

#[async_trait]
impl Tool for Grep {
    fn name(&self) -> &'static str {
        "Grep"
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "在文件内容里搜索正则。基于 ripgrep，会自动跳过 .gitignore 里的文件。\n\
         \n\
         - 优先用这个而不是 Bash 里的 `grep`／`rg`：更快，输出也已经整理过。\n\
         - `pattern` 是 Rust regex 语法。字面量里的 `.`、`(`、`[` 等需要转义。\n\
         - 先用 `files_with_matches` 摸清范围，再对具体文件用 `content` 细看，\
         比一次拉回几百行更省上下文。\n\
         - 结果过多时会截断，缩小 `glob` 范围或让 `pattern` 更具体。"
            .to_owned()
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let pat = input
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or("...");
        match input.get("glob").and_then(|v| v.as_str()) {
            Some(g) => format!("在 {g} 里搜索 {pat}"),
            None => format!("搜索 {pat}"),
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    /// 结果在工具内部就截断到 [`MAX_CHARS`] 了,不需要外层的落盘机制。
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
        // 交给通用决策链。这里表态会绕过 safety 层对凭证目录的拦截 ——
        // `Grep -l "BEGIN PRIVATE KEY" ~/.ssh` 也是一次读取。
        PermissionResult::Passthrough
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        let parsed: Input = serde_json::from_value(input.clone())
            .map_err(|e| ValidationError::rejected(schema_hint(&e)))?;

        if parsed.pattern.is_empty() {
            return Err(ValidationError::rejected(
                "`pattern` 不能为空。要列出文件请用 Glob。",
            ));
        }

        // 正则先在本地编译一次。让 ripgrep 去报错的话,模型收到的是
        // 一段 rust regex 的内部诊断,而且白等一次进程启动。
        if let Err(e) = regex_lite::Regex::new(&parsed.pattern) {
            return Err(ValidationError::rejected(format!(
                "`pattern` 不是合法的正则：{e}。搜索字面量时记得转义 \
                 `.`、`(`、`[`、`*`、`+`、`?`、`|` 这些字符。"
            )));
        }

        if parsed.context_lines.is_some() && parsed.output_mode != OutputMode::Content {
            return Err(ValidationError::rejected(
                "`context_lines` 只在 `output_mode` 为 `content` 时有意义。",
            ));
        }
        Ok(())
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let parsed: Input = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolOutcome::failed(schema_hint(&e)),
        };

        // 搜索根也要过围栏。不查的话 `path: "../../"` 就能读到工作目录外面。
        let root = match &parsed.path {
            Some(p) => match path::resolve(p, &ctx, true).await {
                Ok(r) => r,
                Err(e) => return ToolOutcome::failed(e.for_model()),
            },
            None => ctx.cwd.clone(),
        };

        // 遍历和搜索是同步的重活（几万次 stat + 读文件），扔给阻塞
        // 线程池 —— 占着 async 线程搜一个大仓库，界面上就是整个应用卡住。
        let cancel = ctx.cancel.clone();
        let deadline = search::Deadline::new(Arc::clone(&ctx.clock), search::TIME_BUDGET_SECS);
        let mode = match parsed.output_mode {
            OutputMode::Content => search::Mode::Content {
                context: parsed.context_lines.unwrap_or(0),
            },
            OutputMode::FilesWithMatches => search::Mode::FilesWithMatches,
            OutputMode::Count => search::Mode::Count,
        };
        let glob = parsed.glob.clone();
        let pattern = parsed.pattern.clone();
        let ci = parsed.case_insensitive;

        let found = tokio::task::spawn_blocking(move || {
            let walked = search::walk(&root, glob.as_deref(), MAX_FILES, &cancel, &deadline)?;
            let mut found = search::grep(&walked.files, &pattern, ci, mode, &cancel, &deadline)?;
            found.cut_short |= walked.cut_short;
            Ok::<_, String>(found)
        })
        .await;

        let found = match found {
            Ok(Ok(f)) => f,
            Ok(Err(e)) => return ToolOutcome::failed(e),
            // 阻塞任务 panic 了。这是代码错误，但工具层不能跟着崩。
            Err(e) => return ToolOutcome::failed(format!("搜索没能完成：{e}")),
        };

        if found.lines.is_empty() {
            // `[约束]` 搜索没走完的时候**不能**说"没找到"。那是把"没搜"
            // 说成"不存在"，而模型会拿它当结论 —— 一次超时的搜索会变成
            // "这个仓库里没有这个东西"。这条路必须是失败，让它缩小范围重来。
            if found.cut_short {
                return ToolOutcome::failed(format!(
                    "搜索没走完（超过 {}s 或文件太多），已经走过的部分里没有匹配 —— \
                     这**不能**说明它不存在。用 `path` 缩小范围，或者加 `glob` \
                     过滤文件类型，再搜一次。",
                    search::TIME_BUDGET_SECS
                ));
            }
            // 真的搜完了没搜到：这不是失败。报成失败的话模型会去调参数重试，
            // 而正确的下一步是换个词或者接受这个事实。
            return ToolOutcome::ok_text(no_match_text(&parsed));
        }

        let clamped = clamp(&found.lines.join("\n"), parsed.head_limit);
        let body = render_body(&clamped, found.cut_short);

        ToolOutcome::Ok {
            model_content: riot_protocol::message::ToolResultContent::text(body),
            ui_payload: Some(UiPayload::Plain {
                text: clamped.text,
            }),
            side_messages: Vec::new(),
        }
    }
}

/// 把结果和两种"不完整"提示拼成给模型的正文。
///
/// 抽成纯函数是为了能直接测。这两句提示决定模型把手里这份结果当「全部」
/// 还是「一部分」，而漏掉它们不报错、不崩，只会让结论悄悄变错 —— 那是
/// 这个仓库里最难查的一类 bug，值得有断言直接守着。
fn render_body(clamped: &Clamped, cut_short: bool) -> String {
    let mut body = clamped.text.clone();
    if let Some(note) = &clamped.note {
        body.push_str(&format!("\n\n<system-reminder>{note}</system-reminder>"));
    }
    if cut_short {
        body.push_str(&format!(
            "\n\n<system-reminder>搜索没走完（超过 {}s 或文件太多），\
             上面只是已经找到的部分。用 `path` 缩小范围，或者加 `glob` \
             过滤文件类型。</system-reminder>",
            search::TIME_BUDGET_SECS
        ));
    }
    body
}


struct Clamped {
    text: String,
    note: Option<String>,
}

fn clamp(stdout: &str, head_limit: Option<usize>) -> Clamped {
    let all: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    let total = all.len();

    let limit = head_limit.unwrap_or(MAX_LINES).min(MAX_LINES);
    let mut kept: Vec<&str> = Vec::new();
    let mut chars = 0usize;
    let mut hit_chars = false;

    for l in all.iter().take(limit) {
        let n = l.chars().count() + 1;
        if chars + n > MAX_CHARS && !kept.is_empty() {
            hit_chars = true;
            break;
        }
        chars += n;
        kept.push(l);
    }

    let shown = kept.len();
    let note = if shown < total {
        Some(format!(
            "共 {total} 条结果，这里显示前 {shown} 条{}。\
             请用 `glob` 缩小范围或让 `pattern` 更具体；\
             想先看分布可以用 `output_mode: \"count\"`。",
            if hit_chars { "（已达字符上限）" } else { "" }
        ))
    } else {
        None
    };

    Clamped {
        text: kept.join("\n"),
        note,
    }
}

fn no_match_text(input: &Input) -> String {
    let where_ = match (&input.path, &input.glob) {
        (Some(p), Some(g)) => format!("{p} 下的 {g}"),
        (Some(p), None) => p.clone(),
        (None, Some(g)) => g.clone(),
        (None, None) => "工作目录".to_owned(),
    };
    format!(
        "在{where_}里没有找到匹配 `{}` 的内容。\n\
         注意 .gitignore 里的文件不会被搜索。",
        input.pattern
    )
}



fn schema_hint(e: &serde_json::Error) -> String {
    let raw = e.to_string();
    if raw.contains("missing field `pattern`") {
        return "缺少必需参数 `pattern`。请提供要搜索的正则表达式。".to_owned();
    }
    if raw.contains("unknown variant") {
        return "`output_mode` 只能是 `content`、`files_with_matches` 或 `count`。".to_owned();
    }
    if raw.contains("unknown field") {
        return format!(
            "Grep 接受的参数是 `pattern`、`path`、`glob`、`output_mode`、\
             `case_insensitive`、`context_lines`、`head_limit`。（{raw}）"
        );
    }
    format!("参数格式不对：{raw}。请检查参数类型。")
}

/// 给同 crate 测试用的窄口子。
///
/// 只暴露一个函数，不把 [`Clamped`] 变成公开类型 —— 测试要断言的是
/// 「这段正文对不对」，不是那个中间结构。
#[cfg(test)]
pub(super) mod testing {
    pub fn render_body(text: &str, note: Option<&str>, cut_short: bool) -> String {
        super::render_body(
            &super::Clamped {
                text: text.to_owned(),
                note: note.map(ToOwned::to_owned),
            },
            cut_short,
        )
    }
}
