//! Edit 工具。
//!
//! 核心是**唯一性要求**:`old_string` 必须在文件里恰好出现一次,
//! 否则拒绝。
//!
//! `[约束]` 不能"默认改第一处"。模型给出的 `old_string` 常常是从记忆
//! 里重建的片段,它以为自己指的是某一处,实际匹配到另一处 —— 改错了
//! 不报错、不崩溃,只是代码悄悄坏掉。要求唯一,把这个判断推回给模型
//! (让它加上下文行),比替它猜安全。

use std::path::PathBuf;

use async_trait::async_trait;
use riot_protocol::message::ToolResultContent;
use riot_protocol::permission::{PermissionContext, PermissionResult};
use riot_protocol::tool::{
    DiffHunk, FileState, FileView, PromptContext, Tool, ToolContext, ToolOutcome, UiPayload,
    ValidationError,
};
use serde::Deserialize;

use super::names::{BASH, EDIT, GREP, READ, WRITE};
use super::path;
use super::precondition::{ensure_loaded, verify_unchanged};
use super::text;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    /// Path of the file to modify. May be relative to the working directory.
    path: String,
    /// The text to replace. Must match the file byte for byte, including
    /// indentation. Never include the line-number prefix shown by Read.
    old_string: String,
    /// The replacement text.
    new_string: String,
    /// Replace every occurrence. When omitted, `old_string` must be unique
    /// in the file or the call fails.
    #[serde(default)]
    replace_all: bool,
}

pub struct Edit;

#[async_trait]
impl Tool for Edit {
    fn name(&self) -> &'static str {
        EDIT
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        format!(
            "Performs an exact string replacement in one file. This is the default way \
             to change existing code.\n\
             \n\
             Usage:\n\
             - `old_string` must match the file byte for byte: same indentation \
             (tabs vs. spaces), same trailing whitespace, same line breaks.\n\
             - NEVER include the `line<TAB>` prefix that {READ} puts in front of every \
             line. That prefix is display only and is not in the file.\n\
             - The call FAILS and changes nothing if `old_string` occurs zero times or \
             more than once. Two recovery paths, in this order: (a) extend `old_string` \
             with the lines above and below until exactly one place matches; \
             (b) pass `replace_all: true` when you genuinely mean every occurrence, \
             such as renaming a local variable throughout a file.\n\
             - Read the region first if you are not sure of the exact text. The file is \
             loaded in full for the uniqueness check either way, so you never need to \
             re-{READ} a whole file just to satisfy this tool.\n\
             - Batch independent edits to different files in one message; they run \
             in parallel.\n\
             \n\
             ### When to Use\n\
             \n\
             1. Any change to a file that already exists — this is the default.\n\
             2. Renaming a symbol inside a single file, with `replace_all: true`.\n\
             3. Deleting a block: pass the block as `old_string` and \"\" as `new_string`.\n\
             \n\
             ### When NOT to Use\n\
             \n\
             1. Creating a new file — use {WRITE}. {EDIT} needs existing text to anchor to.\n\
             2. Replacing most of a file you have read in full — one {WRITE} is cheaper \
             and clearer than a dozen edits.\n\
             3. Guessing at `old_string` from memory or from a {GREP} snippet after \
             several edits to the same file. Re-read the region: your earlier edits \
             moved and changed it, and a stale `old_string` fails or, worse, matches \
             the wrong place.\n\
             4. Renaming a symbol across the repository. Locate every file with {GREP} \
             first, then send one {EDIT} per file. Editing blind leaves callers broken.\n\
             5. Using `replace_all` for a short or common string. `replace_all: true` on \
             `id` rewrites every `id` in the file, including comments and unrelated \
             identifiers. Make the string long enough to be unambiguous instead.\n\
             6. Reformatting a whole file — run the project's formatter through {BASH} \
             rather than hand-editing.\n\
             \n\
             <good-example>\n\
             {EDIT}(\n\
             \x20 path: \"src/server.rs\",\n\
             \x20 old_string: \"fn connect(&self) -> Result<Conn> {{\\n        \
             let timeout = 30;\",\n\
             \x20 new_string: \"fn connect(&self) -> Result<Conn> {{\\n        \
             let timeout = 60;\"\n\
             )\n\
             </good-example>\n\
             <reasoning>\n\
             `let timeout = 30;` alone appears in three functions. Including the \
             enclosing signature makes exactly one place match, so the right function \
             is changed.\n\
             </reasoning>\n\
             \n\
             <bad-example>\n\
             {EDIT}(path: \"src/server.rs\", old_string: \"let timeout = 30;\", \
             new_string: \"let timeout = 60;\")\n\
             </bad-example>\n\
             <reasoning>\n\
             Three matches, so the edit is rejected. Retrying with `replace_all: true` \
             would be worse: it changes all three call sites when you meant one.\n\
             </reasoning>\n\
             \n\
             <bad-example>\n\
             {EDIT}(path: \"src/server.rs\", old_string: \"  42\\tlet timeout = 30;\", …)\n\
             </bad-example>\n\
             <reasoning>\n\
             `42\\t` is {READ}'s line-number prefix, not file content, so nothing matches. \
             Strip the prefix and keep the file's own indentation.\n\
             </reasoning>"
        )
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => format!("修改 {p}"),
            None => "修改文件".to_owned(),
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        false
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        false
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
        // 交给通用决策链。敏感路径（.git/、.zshrc）的拦截在 safety 层，
        // 这里返回 Allow 会绕过它 —— 见 ARCHITECTURE.md §9.2 关于
        // "第 3 步的 Allow 不是终点"的说明。
        PermissionResult::Passthrough
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        let parsed: Input = serde_json::from_value(input.clone())
            .map_err(|e| ValidationError::rejected(schema_hint(&e)))?;

        // 先做不碰文件系统的那几项，省掉一次注定要失败的解析和读盘。
        shape_check(&parsed).map_err(ValidationError::rejected)?;

        let resolved = path::resolve(&parsed.path, ctx, true)
            .await
            .map_err(|e| ValidationError::rejected(e.for_model()))?;

        let state = ensure_loaded(&resolved, ctx)
            .await
            .map_err(ValidationError::rejected)?;

        // 唯一性在这里先查一次，让模型在权限弹窗之前就拿到反馈。
        // call() 里还会再查 —— 那次才是决定性的。
        match_count_check(&state.content, &parsed).map_err(ValidationError::rejected)?;

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

        if let Some(msg) = path::detour_risk(&parsed.path, &resolved, &ctx.cwd, false) {
            return ToolOutcome::failed(msg);
        }

        let state = match ensure_loaded(&resolved, &ctx).await {
            Ok(s) => s,
            Err(msg) => return ToolOutcome::failed(msg),
        };

        // TOCTOU 复查。validate_input 到这里之间隔着权限弹窗，
        // 用户可能盯着弹窗想了半分钟。
        if let Err(msg) = verify_unchanged(&resolved, &state.content, &ctx).await {
            return ToolOutcome::failed(msg);
        }

        if let Err(msg) = match_count_check(&state.content, &parsed) {
            return ToolOutcome::failed(msg);
        }

        let updated = if parsed.replace_all {
            state
                .content
                .replace(&parsed.old_string, &parsed.new_string)
        } else {
            state
                .content
                .replacen(&parsed.old_string, &parsed.new_string, 1)
        };

        // 保持原文件的换行风格和 BOM。
        //
        // 不保持的话，改一行会让整个文件每一行都进 diff —— 真正的改动
        // 淹没在几百行换行符变更里，code review 直接失效。
        let original = match ctx.fs.read(&resolved).await.map(|b| text::decode(&b)) {
            Ok(Ok(f)) => f,
            _ => {
                return ToolOutcome::failed(format!(
                    "无法重新读取 {} 以确认文件格式。",
                    parsed.path
                ));
            }
        };

        let bytes = text::encode(&updated, original.newline, original.bom);
        if let Err(e) = ctx.fs.write(&resolved, &bytes).await {
            return ToolOutcome::failed(write_hint(&parsed.path, &e));
        }

        let mtime_ms = ctx
            .fs
            .metadata(&resolved)
            .await
            .map(|m| m.mtime_ms)
            .unwrap_or(state.mtime_ms);

        // 改动前的样子留一份给会话改动视图。写成功之后才记 —— 失败的
        // 尝试不算改动。重复改同一个文件时只有第一份会留下。
        ctx.file_state
            .note_baseline(resolved.clone(), Some(state.content.clone()));

        ctx.file_state.put(
            resolved.clone(),
            FileState {
                content: updated.clone(),
                mtime_ms,
                view: FileView::Full,
            },
        );

        let replaced = if parsed.replace_all {
            state.content.matches(&parsed.old_string).count()
        } else {
            1
        };

        ToolOutcome::Ok {
            model_content: ToolResultContent::text(format!(
                "已修改 {}（替换了 {replaced} 处）。",
                parsed.path
            )),
            ui_payload: Some(UiPayload::FileDiff {
                path: resolved,
                hunks: hunks(&state.content, &updated),
            }),
            side_messages: Vec::new(),
        }
    }
}

/// 不碰文件系统就能判掉的两种无效输入。
///
/// 单独成函数是因为它有两个调用点，而两处的措辞必须一模一样:
/// `validate_input`（早退，省掉解析和读盘）和 [`match_count_check`]
/// （`call()` 路径上的那道，见下）。各写一份迟早漂移。
fn shape_check(input: &Input) -> Result<(), String> {
    if input.old_string.is_empty() {
        return Err("`old_string` 不能为空。要创建新文件请用 Write；\
                    要在文件末尾追加，请把末尾已有的一段内容作为 `old_string`。"
            .to_owned());
    }

    if input.old_string == input.new_string {
        return Err(
            "`old_string` 和 `new_string` 完全相同，这次修改没有任何效果。\
                    请检查是不是漏改了什么。"
                .to_owned(),
        );
    }

    Ok(())
}

/// 唯一性检查。
///
/// `[约束]` 空 `old_string` 必须在这里拦，不能只放在 `validate_input`。
/// 放过去的后果不是"改错一处"而是整个文件被打烂:`"".matches("")` 等于
/// 字符数 + 1，大于 1，`replace_all` 于是走放行分支，接着
/// `content.replace("", new)` 会在**每个字符边界**插一份 new ——
/// `"abc"` 变成 `"XaXbXcX"`，然后正常落盘、正常返回"已修改"。
/// 先读后写协议和 `verify_unchanged` 全部通过，用户拿到的是一次"成功"。
///
/// 触发它不需要攻击者：模型想在文件末尾追加内容时就会写出
/// `{"old_string": "", "new_string": "...", "replace_all": true}`。
fn match_count_check(content: &str, input: &Input) -> Result<(), String> {
    shape_check(input)?;

    let n = content.matches(&input.old_string).count();

    if n == 0 {
        return Err(no_match_hint(content, &input.old_string, &input.path));
    }

    if n > 1 && !input.replace_all {
        return Err(format!(
            "`old_string` 在 {} 里出现了 {n} 次，无法确定要改哪一处。\
             请在 `old_string` 前后各加几行上下文让它唯一；\
             如果确实要全部替换，设置 `replace_all: true`。",
            input.path
        ));
    }

    Ok(())
}

/// 匹配不上时给出可操作的线索。
///
/// 只说"没找到"的话，模型的下一步通常是原样重试。多数匹配失败有具体
/// 原因，指出来能省一整轮。
fn no_match_hint(content: &str, old: &str, path: &str) -> String {
    // 最常见的失败：把 Read 输出的行号一起复制进来了
    if looks_like_line_numbered(old) {
        return format!(
            "在 {path} 里找不到 `old_string`。它看起来带着 Read 输出的行号 —— \
             行号是显示用的，不是文件内容。请去掉行号和后面的 tab 再试。"
        );
    }

    let trimmed = old.trim();
    if !trimmed.is_empty() && content.contains(trimmed) {
        return format!(
            "在 {path} 里找不到 `old_string`，但去掉首尾空白后能匹配上。\
             缩进或行尾空格对不上，请让 `old_string` 与文件内容逐字符一致。"
        );
    }

    let normalized = old.replace("\r\n", "\n");
    if normalized != old && content.contains(&normalized) {
        return format!(
            "在 {path} 里找不到 `old_string`，换行符不一致。\
             请用 `\\n` 而不是 `\\r\\n`。"
        );
    }

    if let Some(first) = old.lines().next()
        && !first.trim().is_empty()
        && content.contains(first.trim())
    {
        return format!(
            "在 {path} 里找不到完整的 `old_string`，但它的第一行能匹配上。\
             后面几行可能有出入 —— 请重新 Read 确认这段内容的当前样子。"
        );
    }

    format!(
        "在 {path} 里找不到 `old_string`。请重新 Read 这个文件，\
         确认要修改的内容确实存在且逐字符一致。"
    )
}

/// 检测 `   123\tcode` 这种形状。
pub(crate) fn looks_like_line_numbered(s: &str) -> bool {
    let mut lines = s.lines().filter(|l| !l.trim().is_empty());
    let mut checked = 0;

    let all_numbered = lines.by_ref().take(3).all(|line| {
        checked += 1;
        let Some((head, _)) = line.split_once('\t') else {
            return false;
        };
        !head.trim().is_empty() && head.trim().chars().all(|c| c.is_ascii_digit())
    });

    checked > 0 && all_numbered
}

/// 极简 diff：找出变化区间。
///
/// 不做完整的 Myers 算法 —— UI 只需要知道"哪几段变了"来做高亮，
/// 精确的最小编辑脚本对这个用途没有额外价值。
fn hunks(before: &str, after: &str) -> Vec<DiffHunk> {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();

    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();

    let max_suffix = (a.len() - prefix).min(b.len() - prefix);
    let suffix = (0..max_suffix)
        .take_while(|i| a[a.len() - 1 - i] == b[b.len() - 1 - i])
        .count();

    if prefix == a.len() && prefix == b.len() {
        return Vec::new();
    }

    let old_lines = a.len() - prefix - suffix;
    let new_lines = b.len() - prefix - suffix;

    let mut content = String::new();
    for line in &a[prefix..prefix + old_lines] {
        content.push('-');
        content.push_str(line);
        content.push('\n');
    }
    for line in &b[prefix..prefix + new_lines] {
        content.push('+');
        content.push_str(line);
        content.push('\n');
    }

    vec![DiffHunk {
        old_start: prefix + 1,
        old_lines,
        new_start: prefix + 1,
        new_lines,
        content,
    }]
}

fn schema_hint(e: &serde_json::Error) -> String {
    let raw = e.to_string();
    for (field, hint) in [
        ("path", "要修改的文件路径"),
        ("old_string", "要被替换的原文"),
        ("new_string", "替换成的新内容"),
    ] {
        if raw.contains(&format!("missing field `{field}`")) {
            return format!("缺少必需参数 `{field}`（{hint}）。");
        }
    }
    format!("参数格式不对：{raw}。")
}

fn write_hint(path: &str, e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => format!("没有写入 {path} 的权限。"),
        _ => format!("写入 {path} 失败：{e}"),
    }
}
