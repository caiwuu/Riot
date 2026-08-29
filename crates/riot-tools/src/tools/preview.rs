//! PreviewFile：把一个本地文件展示给用户看。
//!
//! 工具本身不搬内容 —— 前端在实时事件流里看到这次调用**成功**，就把
//! 文件在右侧预览面板里打开（Browser* 工具自动弹浏览器抽屉的同一条路，
//! 见 useSession 的监听）。这里只做两件事：把路径解析成绝对路径、确认
//! 文件真的存在 —— 失败时给模型一句能改对的话，前端也就不会开出一个
//! 指向不存在文件的标签。
//!
//! 权限显式 Allow：给用户**看**一个文件没有副作用面，和 TodoWrite 同理，
//! 弹窗只会训练用户无脑点允许。

use async_trait::async_trait;
use serde::Deserialize;
use std::path::PathBuf;

use riot_protocol::permission::{DecisionReason, PermissionContext, PermissionResult};
use riot_protocol::tool::{PromptContext, Tool, ToolContext, ToolOutcome};

use super::path;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    /// 要展示的文件路径。可以是相对于工作目录的路径。
    path: String,
}

pub struct PreviewFile;

#[async_trait]
impl Tool for PreviewFile {
    fn name(&self) -> &str {
        "PreviewFile"
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "把一个本地文件展示给用户：在应用右侧的预览面板里打开，用户立刻能看到。\n\
         \n\
         ## 什么时候用\n\
         - 用户让你「预览 / 打开看看 / 展示」某个文件\n\
         - 你生成了文档、报表、图片等交付物，想让用户直接看到成品\n\
         \n\
         ## 支持的类型\n\
         pdf / docx / xlsx / csv / pptx / markdown 和常见代码文件在面板内渲染；\
         png、jpg 等图片全屏查看；名单外的类型会退化成在访达中定位，尽量不要对\
         它们使用。\n\
         \n\
         ## 注意\n\
         - 这个工具**不返回文件内容**。你自己要读内容用 Read，不要用它代替 Read。\n\
         - 一次一个文件。要展示多个就调用多次，每个文件会各占一个预览标签。"
            .into()
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => format!("预览 {p}"),
            None => "预览文件".to_owned(),
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    /// 显式放行：给用户看一个文件没有副作用面。这也让它在规划模式可用 ——
    /// 讨论一份文档时把它摆到用户眼前正是该鼓励的行为。
    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Preapproved {
                what: "文件预览".into(),
            },
        }
    }

    fn target_path(&self, input: &serde_json::Value) -> Option<PathBuf> {
        input
            .get("path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let input: Input = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => {
                return ToolOutcome::failed(format!(
                    "参数不对：{e}。形状是 {{\"path\": \"文件路径\"}}。"
                ));
            }
        };
        let resolved = match path::resolve(&input.path, &ctx, true).await {
            Ok(p) => p,
            Err(e) => return ToolOutcome::failed(e.for_model()),
        };
        // resolve 刚确认过存在，metadata 失败只剩"刚好被删了"这种竞态 ——
        // 按不存在处理。
        match ctx.fs.metadata(&resolved).await {
            Ok(m) if m.is_dir => {
                return ToolOutcome::failed(
                    "这是一个目录。PreviewFile 只能展示单个文件 —— 把要展示的文件的完整路径传进来。",
                );
            }
            Ok(_) => {}
            Err(_) => {
                return ToolOutcome::failed(format!(
                    "文件 {} 不存在。请确认路径是否正确，可以用 Glob 查找。",
                    resolved.display()
                ));
            }
        }
        ToolOutcome::ok_text(format!(
            "已在预览面板向用户展示 {}。这条结果不包含文件内容 —— 需要内容用 Read。",
            path::display_relative(&resolved, &ctx.cwd)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FixedClock, NullFileState, NullProc};
    use crate::tools::memfs::MemFs;
    use riot_protocol::id::{SessionId, ToolUseId};
    use riot_protocol::permission::PermissionModeState;
    use riot_protocol::tool::ProgressSink;
    use std::sync::Arc;

    fn ctx(fs: Arc<MemFs>) -> ToolContext {
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
            fs,
            proc: Arc::new(NullProc),
            web: Arc::new(riot_protocol::web::NoWeb),
            browser: Arc::new(riot_protocol::browser::NoBrowser),
            terminal: Arc::new(riot_protocol::terminal::NoTerminal),
            vision: Arc::new(riot_protocol::vision::NoVision),
            clock: Arc::new(FixedClock::default()),
        }
    }

    #[tokio::test]
    async fn 文件存在时成功且不回显内容() {
        let fs = Arc::new(MemFs::new().with_file("/w/报告.md", "机密内容"));
        let out = PreviewFile
            .call(serde_json::json!({ "path": "报告.md" }), ctx(fs))
            .await;
        let ToolOutcome::Ok { model_content, .. } = out else {
            panic!("该成功");
        };
        let text = format!("{model_content:?}");
        assert!(text.contains("报告.md"), "要点名文件：{text}");
        assert!(
            !text.contains("机密内容"),
            "不搬内容 —— 读内容是 Read 的事：{text}"
        );
    }

    #[tokio::test]
    async fn 文件不存在时指路() {
        let fs = Arc::new(MemFs::new());
        let out = PreviewFile
            .call(serde_json::json!({ "path": "/w/没有.pdf" }), ctx(fs))
            .await;
        let ToolOutcome::Failed {
            error_for_model, ..
        } = out
        else {
            panic!("该失败");
        };
        assert!(
            error_for_model.contains("不存在"),
            "要说清不存在：{error_for_model}"
        );
    }

    #[test]
    fn 显式放行且规划模式可用() {
        let ctx = PermissionContext {
            mode: PermissionModeState(Some(riot_protocol::permission::PermissionMode::Plan)),
            rules: Vec::new(),
            sandboxed: false,
            can_prompt_user: true,
        };
        let r = riot_permissions::decide(
            &PreviewFile,
            &serde_json::json!({ "path": "x.pdf" }),
            &ctx,
            &riot_permissions::RuleSet::default(),
        );
        assert!(r.is_allow(), "预览必须静默放行：{r:?}");
    }
}
