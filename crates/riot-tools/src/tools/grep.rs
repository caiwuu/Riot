//! Grep 工具。
//!
//! 底层是 ripgrep。选它而不是自己实现,是因为 gitignore 处理、编码嗅探、
//! 多线程遍历这些东西的工程量远超"匹配正则"本身。
//!
//! 参数通过 argv 传给子进程,**不经过 shell**。这不是实现细节 —— 它意味着
//! 模型给的 pattern 里就算有 `$(...)` 或 `;` 也只是普通字符。走 shell 的话
//! 每一个搜索词都得先做一遍转义,而漏掉一处就是命令注入。

use std::path::PathBuf;

use async_trait::async_trait;
use riot_protocol::permission::{PermissionContext, PermissionResult};
use riot_protocol::tool::{
    ProcessSpec, PromptContext, ResultBudget, Tool, ToolContext, ToolOutcome, UiPayload,
    ValidationError,
};
use serde::Deserialize;

use super::path;

/// 搜索超时。
///
/// ripgrep 很快,超过这个时间基本意味着搜到了不该搜的地方
/// (挂载的网络盘、巨大的 node_modules)。
const TIMEOUT_MS: u64 = 30_000;

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

        let spec = ProcessSpec {
            program: "rg".to_owned(),
            args: build_args(&parsed, &root),
            cwd: ctx.cwd.clone(),
            env: Vec::new(),
            timeout_ms: Some(TIMEOUT_MS),
        };

        let out = match ctx.proc.run(spec, ctx.cancel.clone()).await {
            Ok(o) => o,
            Err(e) => return ToolOutcome::failed(spawn_hint(&e)),
        };

        if out.timed_out {
            return ToolOutcome::failed(format!(
                "搜索超过 {}s 未完成。请用 `path` 缩小范围，或者加上 `glob` 过滤文件类型。",
                TIMEOUT_MS / 1000
            ));
        }

        // ripgrep 的退出码：0 有匹配，1 无匹配，2 出错。
        //
        // `[约束]` 1 和 2 必须分开。合并的话"没搜到"会被报成搜索失败,
        // 模型会去调参数重试 —— 而正确的下一步是换个词或者接受这个事实。
        match out.exit_code {
            1 => return ToolOutcome::ok_text(no_match_text(&parsed)),
            0 => {}
            _ => return ToolOutcome::failed(rg_error_hint(&out.stderr)),
        }

        let clamped = clamp(&out.stdout, parsed.head_limit);
        let mut body = if clamped.text.is_empty() {
            // rg 说有匹配却没给内容，属于不该发生的情况。
            // 返回空字符串会让模型以为工具坏了。
            no_match_text(&parsed)
        } else {
            clamped.text.clone()
        };

        if let Some(note) = clamped.note {
            body.push_str(&format!("\n\n<system-reminder>{note}</system-reminder>"));
        }

        ToolOutcome::Ok {
            model_content: riot_protocol::message::ToolResultContent::text(body),
            ui_payload: Some(UiPayload::Plain {
                text: clamped.text,
            }),
            side_messages: Vec::new(),
        }
    }
}

fn build_args(input: &Input, root: &std::path::Path) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();

    // `[约束]` --no-config 不能省。用户的 RIPGREP_CONFIG_PATH 里可能有
    // --smart-case、--hidden 之类的开关,那会让同一次搜索在不同机器上
    // 给出不同结果,而模型和我们都看不到那份配置。
    a.push("--no-config".into());
    a.push("--color=never".into());

    match input.output_mode {
        OutputMode::Content => {
            a.push("--line-number".into());
            a.push("--with-filename".into());
            if let Some(n) = input.context_lines {
                a.push("--context".into());
                a.push(n.to_string());
            }
        }
        OutputMode::FilesWithMatches => a.push("--files-with-matches".into()),
        OutputMode::Count => a.push("--count".into()),
    }

    if input.case_insensitive {
        a.push("--ignore-case".into());
    }
    if let Some(g) = &input.glob {
        a.push("--glob".into());
        a.push(g.clone());
    }

    // `[约束]` pattern 必须走 `-e`。直接当位置参数的话,以 `-` 开头的
    // 搜索词(比如搜 `--force`)会被当成 flag 解析 —— 表现是一个看不懂的
    // "unknown option"错误,或者更糟,一个碰巧存在的 flag 被激活。
    a.push("-e".into());
    a.push(input.pattern.clone());

    // `--` 之后全是路径。搜索根同理可能以 `-` 开头。
    a.push("--".into());
    a.push(root.to_string_lossy().into_owned());
    a
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

fn spawn_hint(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => {
            "找不到 ripgrep（rg）。请先安装它，或者改用 Bash 里的 grep。".to_owned()
        }
        _ => format!("启动搜索失败：{e}"),
    }
}

fn rg_error_hint(stderr: &str) -> String {
    let first = stderr.lines().next().unwrap_or("").trim();
    if first.is_empty() {
        return "搜索失败，没有更多信息。".to_owned();
    }
    format!("搜索失败：{first}")
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
