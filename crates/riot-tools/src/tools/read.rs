//! Read 工具。

use std::path::PathBuf;

use async_trait::async_trait;
use riot_protocol::permission::{PermissionContext, PermissionResult};
use riot_protocol::tool::{
    FileState, FileView, PromptContext, ResultBudget, Tool, ToolContext, ToolOutcome, UiPayload,
    ValidationError,
};
use serde::Deserialize;

use super::path;
use super::text::{self, DecodeError};

/// 单次最多返回的行数。
const MAX_LINES: usize = 2000;

/// 单次最多返回的字节数。
///
/// 行数够用不代表字节够用 —— 一个 minified 的 bundle 可能只有 3 行、
/// 但有 8MB。两个上限都要有。
const MAX_BYTES: usize = 256 * 1024;

/// 单行最多返回的字符数。
///
/// 超长行几乎总是压缩产物或数据文件。整行塞给模型会挤掉上下文预算,
/// 而它对理解代码没有帮助。
const MAX_LINE_CHARS: usize = 2000;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    /// 要读取的文件路径。可以是相对于工作目录的路径。
    path: String,
    /// 从第几行开始读，从 1 开始。省略则从头开始。
    #[serde(default)]
    offset: Option<usize>,
    /// 最多读几行。省略则读到上限为止。
    #[serde(default)]
    limit: Option<usize>,
}

pub struct Read;

#[async_trait]
impl Tool for Read {
    fn name(&self) -> &'static str {
        "Read"
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        format!(
            "读取文件内容。返回结果每行带行号，格式是 `行号\\t内容`。\n\
             \n\
             - 行号是显示用的，不是文件内容的一部分。用 Edit 时 `old_string` \
             不要带行号。\n\
             - 一次最多返回 {MAX_LINES} 行；文件更长时用 `offset` 继续读。\n\
             - 超过 {MAX_LINE_CHARS} 字符的行会被截断。\n\
             - 读取整个文件之后才能用 Edit 修改它。"
        )
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => format!("读取 {p}"),
            None => "读取文件".to_owned(),
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    /// `[约束]` 必须是 `Unlimited`。
    ///
    /// 否则会产生"Read → 结果太大落盘成文件 → 模型又去 Read 那个文件"
    /// 的循环。见 ARCHITECTURE.md §6.7
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
        // 读操作交给通用决策链。凭证文件的拦截在 safety 层，
        // 这里表态反而会绕过它。
        PermissionResult::Passthrough
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        let parsed: Input = serde_json::from_value(input.clone())
            .map_err(|e| ValidationError::rejected(schema_hint(&e)))?;

        if parsed.offset == Some(0) {
            return Err(ValidationError::rejected(
                "`offset` 从 1 开始计数。要从文件开头读就省略这个参数。",
            ));
        }
        if parsed.limit == Some(0) {
            return Err(ValidationError::rejected(
                "`limit` 必须大于 0。要读到上限为止就省略这个参数。",
            ));
        }
        Ok(())
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let parsed: Input = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolOutcome::failed(schema_hint(&e)),
        };

        let resolved = match path::resolve(&parsed.path, &ctx, true).await {
            Ok(p) => p,
            Err(e) => return ToolOutcome::failed(e.for_model()),
        };

        let meta = match ctx.fs.metadata(&resolved).await {
            Ok(m) => m,
            Err(e) => return ToolOutcome::failed(io_hint(&parsed.path, &e)),
        };

        if meta.is_dir {
            return ToolOutcome::failed(format!(
                "{} 是一个目录，不是文件。用 Glob 或 Bash 的 ls 来列出目录内容。",
                parsed.path
            ));
        }

        let bytes = match ctx.fs.read(&resolved).await {
            Ok(b) => b,
            Err(e) => return ToolOutcome::failed(io_hint(&parsed.path, &e)),
        };

        let file = match text::decode(&bytes) {
            Ok(f) => f,
            Err(DecodeError::Binary { reason }) => {
                return ToolOutcome::failed(format!(
                    "{} 是二进制文件（{reason}），无法作为文本读取。",
                    parsed.path
                ));
            }
        };

        if file.content.is_empty() {
            // 返回空字符串的话模型会以为工具坏了，然后反复重试
            return ToolOutcome::ok_text(format!("{}：文件为空。", parsed.path));
        }

        let render = render(&file.content, parsed.offset, parsed.limit);

        // 只有完整读到文件末尾才算 Full 视图。
        //
        // `[约束]` 截断过的内容必须标成 Partial —— Edit 会拒绝 Partial
        // 视图。这是"模型没看到全文就动手改"的唯一防线。
        let view = if render.is_complete {
            FileView::Full
        } else {
            FileView::Partial {
                offset: render.start_line - 1,
                limit: render.line_count,
            }
        };

        ctx.file_state.put(
            resolved.clone(),
            FileState {
                content: file.content.clone(),
                mtime_ms: meta.mtime_ms,
                view,
            },
        );

        let mut body = text::with_line_numbers(&render.text, render.start_line);
        if let Some(note) = render.note {
            body.push_str(&format!("\n{note}\n"));
        }

        ToolOutcome::Ok {
            model_content: riot_protocol::message::ToolResultContent::text(body),
            ui_payload: Some(UiPayload::FileRead {
                path: resolved,
                line_count: render.line_count,
                truncated: !render.is_complete,
            }),
            side_messages: Vec::new(),
        }
    }
}

struct Rendered {
    text: String,
    start_line: usize,
    line_count: usize,
    is_complete: bool,
    note: Option<String>,
}

fn render(content: &str, offset: Option<usize>, limit: Option<usize>) -> Rendered {
    let total = text::line_count(content);
    let start_line = offset.unwrap_or(1).max(1);
    let skip = start_line - 1;
    let want = limit.unwrap_or(MAX_LINES).min(MAX_LINES);

    let mut out: Vec<String> = Vec::new();
    let mut bytes = 0usize;
    let mut truncated_lines = 0usize;
    let mut hit_byte_cap = false;

    for line in content.lines().skip(skip).take(want) {
        let (rendered, was_cut) = clamp_line(line);
        if was_cut {
            truncated_lines += 1;
        }

        // 字节上限。至少给一行，否则超长单行文件会返回空内容 ——
        // 那看起来像"文件是空的"，比截断更容易误导。
        if bytes + rendered.len() > MAX_BYTES && !out.is_empty() {
            hit_byte_cap = true;
            break;
        }
        bytes += rendered.len();
        out.push(rendered);
    }

    let shown = out.len();
    let last_line = skip + shown;
    let is_complete = skip == 0 && last_line >= total && !hit_byte_cap && truncated_lines == 0;

    let mut notes = Vec::new();
    if truncated_lines > 0 {
        notes.push(format!(
            "有 {truncated_lines} 行超过 {MAX_LINE_CHARS} 字符已被截断"
        ));
    }
    if hit_byte_cap {
        notes.push(format!("已达到 {} KB 的单次返回上限", MAX_BYTES / 1024));
    }
    if last_line < total {
        notes.push(format!(
            "文件共 {total} 行，这里显示到第 {last_line} 行。用 offset={} 继续读",
            last_line + 1
        ));
    }

    let note = if notes.is_empty() {
        None
    } else {
        // 用 system-reminder 包起来，和文件内容区分开 ——
        // 否则模型可能把它当成文件的一部分。
        Some(format!(
            "<system-reminder>{}。</system-reminder>",
            notes.join("；")
        ))
    };

    Rendered {
        text: out.join("\n"),
        start_line,
        line_count: shown,
        is_complete,
        note,
    }
}

fn clamp_line(line: &str) -> (String, bool) {
    if line.chars().count() <= MAX_LINE_CHARS {
        return (line.to_owned(), false);
    }
    let cut: String = line.chars().take(MAX_LINE_CHARS).collect();
    (format!("{cut}… [此行已截断]"), true)
}

/// serde 的原始错误对模型没用，转成祈使句。
///
/// 见 ARCHITECTURE.md §6.5 —— *"the model is not great at generating
/// valid input"*，这一层翻译的投入回报很高。
fn schema_hint(e: &serde_json::Error) -> String {
    let raw = e.to_string();
    if raw.contains("missing field `path`") {
        return "缺少必需参数 `path`。请提供要读取的文件路径。".to_owned();
    }
    if raw.contains("unknown field") {
        return format!(
            "Read 只接受 `path`、`offset`、`limit` 三个参数。（{raw}）"
        );
    }
    format!("参数格式不对：{raw}。请检查参数类型。")
}

fn io_hint(path: &str, e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => {
            format!("文件 {path} 不存在。请先确认路径，可以用 Glob 查找。")
        }
        std::io::ErrorKind::PermissionDenied => {
            format!("没有读取 {path} 的权限。")
        }
        _ => format!("读取 {path} 失败：{e}"),
    }
}
