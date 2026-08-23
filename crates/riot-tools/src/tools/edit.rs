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

use super::path;
use super::precondition::{ensure_loaded, verify_unchanged};
use super::text;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    /// 要修改的文件路径。
    path: String,
    /// 要被替换的原文。必须与文件内容逐字符一致，不要带行号。
    old_string: String,
    /// 替换成的新内容。
    new_string: String,
    /// 替换所有出现处。省略时要求 `old_string` 在文件中唯一。
    #[serde(default)]
    replace_all: bool,
}

pub struct Edit;

#[async_trait]
impl Tool for Edit {
    fn name(&self) -> &'static str {
        "Edit"
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "修改文件中的一段文本。\n\
         \n\
         - 改之前最好先 Read 看过相关位置；系统会自行载入全文做唯一性检查，\
         不必为了过关再读一遍整文件。\n\
         - `old_string` 要和文件内容逐字符一致，**不要带 Read 显示的行号**。\n\
         - `old_string` 必须在文件里唯一。如果它出现多次，加上前后几行\
         上下文让它唯一，或者用 `replace_all`。\n\
         - 缩进和空白也要一致。"
            .to_owned()
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

        if parsed.old_string == parsed.new_string {
            return Err(ValidationError::rejected(
                "`old_string` 和 `new_string` 完全相同，这次修改没有任何效果。\
                 请检查是不是漏改了什么。",
            ));
        }

        if parsed.old_string.is_empty() {
            return Err(ValidationError::rejected(
                "`old_string` 不能为空。要创建新文件请用 Write；\
                 要在文件末尾追加，请把末尾已有的一段内容作为 `old_string`。",
            ));
        }

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

/// 唯一性检查。
fn match_count_check(content: &str, input: &Input) -> Result<(), String> {
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
