//! Write 工具。
//!
//! 覆盖或创建一个文件。
//!
//! `[约束]` 覆盖已存在的文件同样要走先读后写协议。这一点容易被认为
//! 多余 —— "反正是全量覆盖,读不读有什么关系" —— 但要防的恰恰是全量
//! 覆盖:模型基于半小时前的印象重写整个文件,把这期间用户的改动、
//! 以及它自己前几步的改动一起抹掉。

use std::path::PathBuf;

use async_trait::async_trait;
use riot_protocol::message::ToolResultContent;
use riot_protocol::permission::{PermissionContext, PermissionResult};
use riot_protocol::tool::{
    FileState, FileView, PromptContext, Tool, ToolContext, ToolOutcome, UiPayload,
    ValidationError,
};
use serde::Deserialize;

use super::path;
use super::precondition::{check_fresh, verify_unchanged};
use super::text::{self, Newline};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    /// 要写入的文件路径。
    path: String,
    /// 文件的完整内容。
    content: String,
}

pub struct Write;

#[async_trait]
impl Tool for Write {
    fn name(&self) -> &'static str {
        "Write"
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "写入文件，内容会完全覆盖原有内容。\n\
         \n\
         - 覆盖已存在的文件前必须先用 Read 读过它。\n\
         - 只改动文件的一部分时优先用 Edit —— 全量覆盖容易丢掉你没看到的内容。\n\
         - 创建新文件不需要先 Read。"
            .to_owned()
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => format!("写入 {p}"),
            None => "写入文件".to_owned(),
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
        PermissionResult::Passthrough
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        let parsed: Input = serde_json::from_value(input.clone())
            .map_err(|e| ValidationError::rejected(schema_hint(&e)))?;

        // must_exist = false：新文件也要过围栏
        let resolved = path::resolve(&parsed.path, ctx, false)
            .await
            .map_err(|e| ValidationError::rejected(e.for_model()))?;

        // 文件已存在才需要先读后写
        if ctx.fs.metadata(&resolved).await.is_ok() {
            check_fresh(&resolved, ctx)
                .await
                .map_err(|s| ValidationError::rejected(s.for_model(&parsed.path)))?;
        }

        Ok(())
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let parsed: Input = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolOutcome::failed(schema_hint(&e)),
        };

        let resolved = match path::resolve(&parsed.path, &ctx, false).await {
            Ok(p) => p,
            Err(e) => return ToolOutcome::failed(e.for_model()),
        };

        let existing = ctx.fs.metadata(&resolved).await.ok();
        let created = existing.is_none();

        // 覆盖之前的样子。新建时是 None —— 会话改动视图靠它区分"新增文件"
        // 和"改了文件"。
        let mut before: Option<String> = None;

        // 已存在的文件：复查先读后写 + TOCTOU
        let (newline, bom) = if existing.is_some() {
            let state = match check_fresh(&resolved, &ctx).await {
                Ok(s) => s,
                Err(stale) => return ToolOutcome::failed(stale.for_model(&parsed.path)),
            };

            if let Err(msg) = verify_unchanged(&resolved, &state.content, &ctx).await {
                return ToolOutcome::failed(msg);
            }
            before = Some(state.content.clone());

            // 保持原文件的换行风格和 BOM。用户的文件是 CRLF 的话，
            // 全量覆盖成 LF 会让整个文件进 diff。
            match ctx.fs.read(&resolved).await.map(|b| text::decode(&b)) {
                Ok(Ok(f)) => (f.newline, f.bom),
                // 原来是二进制，现在要写文本 —— 用平台默认风格
                _ => (Newline::Lf, false),
            }
        } else {
            (Newline::Lf, false)
        };

        let bytes = text::encode(&parsed.content, newline, bom);
        let len = bytes.len() as u64;

        if let Err(e) = ctx.fs.write(&resolved, &bytes).await {
            return ToolOutcome::failed(write_hint(&parsed.path, &e, created));
        }

        let mtime_ms = ctx
            .fs
            .metadata(&resolved)
            .await
            .map(|m| m.mtime_ms)
            .unwrap_or(0);

        ctx.file_state.note_baseline(resolved.clone(), before);

        // 写完就是最新状态，直接进缓存 —— 否则模型写完还得再 Read
        // 一次才能 Edit，白白多一轮。
        ctx.file_state.put(
            resolved.clone(),
            FileState {
                content: parsed.content.clone(),
                mtime_ms,
                view: FileView::Full,
            },
        );

        let verb = if created { "创建" } else { "覆盖" };
        ToolOutcome::Ok {
            model_content: ToolResultContent::text(format!(
                "已{verb} {}（{} 行）。",
                parsed.path,
                text::line_count(&parsed.content)
            )),
            ui_payload: Some(UiPayload::FileWrite {
                path: resolved,
                bytes: len,
                created,
            }),
            side_messages: Vec::new(),
        }
    }
}

fn schema_hint(e: &serde_json::Error) -> String {
    let raw = e.to_string();
    if raw.contains("missing field `path`") {
        return "缺少必需参数 `path`。请提供要写入的文件路径。".to_owned();
    }
    if raw.contains("missing field `content`") {
        return "缺少必需参数 `content`。请提供文件的完整内容。".to_owned();
    }
    format!("参数格式不对：{raw}。")
}

fn write_hint(path: &str, e: &std::io::Error, created: bool) -> String {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => format!("没有写入 {path} 的权限。"),
        std::io::ErrorKind::NotFound if created => {
            format!("{path} 的上级目录不存在。请先创建目录，或者检查路径是否正确。")
        }
        _ => format!("写入 {path} 失败：{e}"),
    }
}
