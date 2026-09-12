//! Delete 工具。
//!
//! 删除一个文件。它存在的理由不是"模型没法删文件"—— Bash 的 `rm` 一直
//! 可以 —— 而是**回退**和**确认**：检查点切片、改动栏、Restore / Redo 都
//! 认工具层记下的基线（[`FileStateCache::note_baseline`]）；`rm` 只有按
//! 字面写出路径时才被 `bash_effects` 顺带记上，而且弹窗里只有一行命令，
//! acceptEdits 下还可能被规则放行。Delete 把要删的正文摆进弹窗、永不自动
//! 放行。照 Cursor 的做法：编辑器工具（Write / Edit / Delete）碰过的文件
//! 进检查点，终端改动只做字面比对，不监控整个工作区补漏。
//!
//! `[约束]` 只删**单个文本文件**，不删目录、不删二进制。基线是
//! `Option<String>`，二进制放不进去；塞一份 lossy 文本进去，回退写回的
//! 就是一份坏文件。目录递归删掉的东西同样一层都记不下来。这两种都指回
//! Bash，并明说那样不进回退 —— 让模型和用户都知道边界在哪，而不是悄悄
//! 少记一份。
//!
//! `[约束]` 基线记**磁盘原样**（含 BOM、保留 CRLF），不是解码归一化后
//! 的文本。回退时 v0 会被原样写回去，归一化过的写回去等于把 CRLF 文件
//! 改成了 LF。
//!
//! 不要求先 Read。删除是整文件粒度的操作，用户在弹窗里看到的就是要批的
//! 全部内容（路径 + 正文前若干行）；而且它**不**归入 acceptEdits 的自动
//! 放行（见 `riot_permissions::chain::is_edit_tool`）—— 删文件比改文件
//! 重一档，Cursor 的自动运行模式默认也单独保护删除。

use std::path::PathBuf;

use async_trait::async_trait;
use riot_protocol::message::ToolResultContent;
use riot_protocol::permission::{PermissionContext, PermissionResult};
use riot_protocol::text::UiText;
use riot_protocol::tool::{
    PromptContext, Tool, ToolContext, ToolOutcome, UiPayload, ValidationError,
};
use riot_protocol::ui_text;
use serde::Deserialize;

use super::names::{BASH, DELETE, EDIT, GLOB, WRITE};
use super::path;
use super::text::{self, DecodeError};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    /// Path of the file to delete. May be relative to the working directory.
    path: String,
}

pub struct Delete;

#[async_trait]
impl Tool for Delete {
    fn name(&self) -> &'static str {
        DELETE
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        format!(
            "Deletes one file from disk.\n\
             \n\
             Usage:\n\
             - Deletes exactly one existing **text file**. Directories and binary files \
             are rejected; the call fails and nothing is removed.\n\
             - The file's content is recorded before deletion, so the deletion shows up \
             in the session's change list and is undone by a checkpoint restore. The \
             user is shown the content being removed and must approve it.\n\
             - No prior read is required, but do not delete a file you have not looked \
             at unless the user explicitly named it.\n\
             \n\
             ### When to Use\n\
             \n\
             1. Removing a file the user asked you to remove.\n\
             2. Removing a file you created earlier in this session and no longer need.\n\
             3. Removing the old file after moving its content elsewhere with {WRITE}.\n\
             \n\
             ### When NOT to Use\n\
             \n\
             1. Emptying or truncating a file — use {EDIT} or {WRITE} instead. \
             Deleting and recreating loses the file's history.\n\
             2. Deleting a directory, a binary file, or many files at once. Use {BASH} \
             (`rm`, `rm -r`) for those — and tell the user that those removals are \
             NOT covered by checkpoint restore.\n\
             3. Deleting build output, caches, or dependency folders. That is {BASH} \
             territory (`cargo clean`, `rm -rf node_modules`); this tool is for source \
             files the user would want to get back.\n\
             4. Guessing a path. If unsure the file exists or which one is meant, \
             locate it with {GLOB} first.\n\
             \n\
             <good-example>\n\
             {DELETE}(path: \"src/legacy/old_parser.rs\")\n\
             </good-example>\n\
             <reasoning>\n\
             One source file the user asked to remove. Its content is recorded, so \
             the change list shows it as deleted and a restore brings it back.\n\
             </reasoning>\n\
             \n\
             <bad-example>\n\
             {DELETE}(path: \"target\")\n\
             </bad-example>\n\
             <reasoning>\n\
             A directory. The call is rejected; build output is removed with {BASH} \
             (`cargo clean`), not with a source-file tool.\n\
             </reasoning>"
        )
    }

    fn describe(&self, input: &serde_json::Value) -> UiText {
        match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => ui_text!("tools.delete.file", path = p),
            None => ui_text!("tools.delete.any"),
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        false
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        false
    }

    fn is_destructive(&self, _input: &serde_json::Value) -> bool {
        true
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
        // 交给通用决策链：敏感路径（.git/、.zshrc）在 safety 层拦，这里
        // 返回 Allow 会绕过它。默认模式下落到"逐次询问"。
        PermissionResult::Passthrough
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        let parsed: Input = serde_json::from_value(input.clone())
            .map_err(|e| ValidationError::rejected(schema_hint(&e)))?;

        // must_exist = true：不存在的路径在这里就报"文件不存在"，
        // 让模型在权限弹窗之前拿到反馈。
        let resolved = path::resolve(&parsed.path, ctx, true)
            .await
            .map_err(|e| ValidationError::rejected(e.for_model()))?;

        load_text(&resolved, &parsed.path, ctx)
            .await
            .map(|_| ())
            .map_err(ValidationError::rejected)
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

        // validate_input 到这里之间隔着权限弹窗。目录 / 二进制的判定重做
        // 一遍，正文也是此刻现读的 —— 基线要的是删除前一瞬的样子。
        let before = match load_text(&resolved, &parsed.path, &ctx).await {
            Ok(text) => text,
            Err(msg) => return ToolOutcome::failed(msg),
        };
        let lines = text::line_count(&before);

        if let Err(e) = ctx.fs.remove_file(&resolved).await {
            return ToolOutcome::failed(remove_hint(&parsed.path, &e));
        }

        // 删成功之后才记。失败的尝试不算改动。同一个文件本会话先改过的
        // 话，这里不会覆盖最初那份基线（只有第一次算数）。
        ctx.file_state.note_baseline(resolved.clone(), Some(before));
        // 先读后写缓存里那份内容已经对应一个不存在的文件。留着的话，
        // 模型随后 Write 同名新文件时会被当成"覆盖旧文件"去比 mtime。
        ctx.file_state.invalidate(&resolved);

        ToolOutcome::Ok {
            model_content: ToolResultContent::text(format!(
                "已删除 {}（{lines} 行）。",
                parsed.path
            )),
            ui_payload: Some(UiPayload::FileDelete {
                path: resolved,
                lines,
            }),
            side_messages: Vec::new(),
        }
    }
}

/// 读出要删的文件的**原样**文本。目录和二进制在这里拒掉。
///
/// 两个调用点（`validate_input` 早退、`call` 决定性的那次）措辞必须一样，
/// 所以只写一份。
async fn load_text(
    resolved: &std::path::Path,
    raw: &str,
    ctx: &ToolContext,
) -> Result<String, String> {
    let meta = ctx
        .fs
        .metadata(resolved)
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                format!("文件 {raw} 不存在。请确认路径是否正确，可以用 {GLOB} 查找。")
            }
            _ => format!("无法访问 {raw}：{e}"),
        })?;
    if meta.is_dir {
        return Err(format!(
            "{raw} 是目录。{DELETE} 只删单个文件；要删目录请用 {BASH} 的 `rm -r`，\
             并告诉用户那样的删除不会进入回退记录。"
        ));
    }

    let bytes = ctx
        .fs
        .read(resolved)
        .await
        .map_err(|e| format!("无法读取 {raw} 以记录删除前的内容：{e}"))?;

    // 只借 decode 判二进制（NUL、非 UTF-8）。基线要磁盘原样，不用它
    // 归一化过的 content。
    if let Err(DecodeError::Binary { reason }) = text::decode(&bytes) {
        return Err(binary_hint(raw, reason));
    }
    // decode 已经验过 UTF-8（BOM 本身也是合法 UTF-8），这里理论上不会失败。
    String::from_utf8(bytes).map_err(|_| binary_hint(raw, "不是有效的 UTF-8"))
}

fn binary_hint(raw: &str, reason: &str) -> String {
    format!(
        "{raw} 是二进制文件（{reason}）。{DELETE} 只删文本文件 —— 二进制内容记不进\
         回退基线。如果确实要删，用 {BASH} 的 `rm`，并告诉用户那样回退时找不回来。"
    )
}

fn schema_hint(e: &serde_json::Error) -> String {
    let raw = e.to_string();
    if raw.contains("missing field `path`") {
        return "缺少必需参数 `path`。请提供要删除的文件路径。".to_owned();
    }
    format!("参数格式不对：{raw}。")
}

fn remove_hint(path: &str, e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => format!("{path} 已经不存在，无需删除。"),
        std::io::ErrorKind::PermissionDenied => format!("没有删除 {path} 的权限。"),
        _ => format!("删除 {path} 失败：{e}"),
    }
}
