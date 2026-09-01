//! 把东西摆到用户眼前：[`PreviewFile`] 是一个本地文件，[`ShowBrowser`] 是
//! 内置浏览器面板。
//!
//! 两个工具都不搬内容 —— 前端在实时事件流里看到这次调用**成功**，就把对应
//! 的面板打开（见 useSession 的监听）。这里只确认真的有东西可看，失败时给
//! 模型一句能改对的话，前端也就不会开出一个空面板、或者指向不存在文件的标签。
//!
//! `[约束]` 「要不要把面板推到用户面前」这个判断归模型，不归前端。前端一度
//! 按工具名前缀猜：任何 `Browser*` 调用都把抽屉弹出来。于是 BrowserSecrets、
//! BrowserFuzz 这类纯后台分析在抢屏幕，连说明里写着"**不切走**用户当前看的
//! 那一页"的 BrowserReadTab 也在抢。名字前缀表达不了意图 —— 只有模型知道
//! 这一步是"给你看"还是"我自己查"。
//!
//! 权限显式 Allow：给用户**看**一个东西没有副作用面，和 TodoWrite 同理，
//! 弹窗只会训练用户无脑点允许。

use async_trait::async_trait;
use serde::Deserialize;
use std::path::PathBuf;

use riot_protocol::browser::BLANK_PAGE;
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

/// [`ShowBrowser`] 不要参数 —— 展示的永远是当前那一页。
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoInput {}

pub struct ShowBrowser;

#[async_trait]
impl Tool for ShowBrowser {
    fn name(&self) -> &str {
        "ShowBrowser"
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(NoInput)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "把内置浏览器面板打开给用户，让他看到你正在操作的这一页。\n\
         \n\
         ## 什么时候用\n\
         - 用户让你「打开看看 / 跑起来给我看 / 演示一下」\n\
         - 你改完前端、自己验过了，想让他直接看到效果\n\
         - 接下来几步要在页面上操作，他跟着看才知道你在做什么\n\
         \n\
         ## 什么时候不要用\n\
         别的 Browser* 工具**不会**自动把面板弹出来，弹不弹由你判断。抢用户\
         的屏幕是一件打扰的事 —— 纯粹是你自己在查（读结构、抓包、扫描、\
         对比多个标签页），就不要调它。一段工作里调一次就够，面板会一直开着。\n\
         \n\
         ## 注意\n\
         - 先用 BrowserNavigate 打开地址再展示；还没有页面时这个工具会失败。\n\
         - 它**不返回页面内容**。你自己要看页面用 BrowserSnapshot / BrowserView。\n\
         - 请用户亲自操作（登录、验证码）用 BrowserHandoff —— 那个自带把面板\
         带到眼前，不用先调这个。"
            .into()
    }

    fn describe(&self, _input: &serde_json::Value) -> String {
        "打开浏览器面板给用户看".to_owned()
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    /// 同 [`PreviewFile`]：给用户看一眼没有副作用面，规划模式下也该鼓励。
    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Preapproved {
                what: "浏览器面板".into(),
            },
        }
    }

    async fn call(&self, _input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        // `current_url` 是信息性查询，不会为了回答它把浏览器起起来 —— 空串
        // 表示进程没起来（或崩了），BLANK_PAGE 表示起来了但停在空白页。两种
        // 都没东西可给用户看，而模型的下一步是同一个，所以不分支。
        let url = ctx.browser.current_url().await;
        if url.is_empty() || url == BLANK_PAGE {
            return ToolOutcome::failed(
                "浏览器里还没有打开任何页面。先用 BrowserNavigate 打开地址，再把它展示给用户。",
            );
        }
        ToolOutcome::ok_text(format!(
            "已经把浏览器面板打开给用户，他现在看到的是 {url}。\
             这条结果不包含页面内容 —— 你自己要看用 BrowserSnapshot 或 BrowserView。"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeBrowser, FixedClock, NullFileState, NullProc};
    use crate::tools::memfs::MemFs;
    use riot_protocol::browser::{BrowserAccess, NoBrowser};
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
        for tool in [&PreviewFile as &dyn Tool, &ShowBrowser] {
            let r = riot_permissions::decide(
                tool,
                &serde_json::json!({ "path": "x.pdf" }),
                &ctx,
                &riot_permissions::RuleSet::default(),
            );
            assert!(r.is_allow(), "{} 必须静默放行：{r:?}", tool.name());
        }
    }

    /// 只换掉浏览器，文件系统那套在这两个用例里用不上。
    fn browser_ctx(browser: Arc<dyn BrowserAccess>) -> ToolContext {
        ToolContext {
            browser,
            ..ctx(Arc::new(MemFs::new()))
        }
    }

    /// 一页都没有时不能开一个空面板 —— 要给模型指回 BrowserNavigate。
    #[tokio::test]
    async fn 没有页面时让模型先导航() {
        for browser in [
            // 浏览器压根没起来。
            Arc::new(NoBrowser) as Arc<dyn BrowserAccess>,
            // 起来了但停在空白页 —— 同样没东西可看。
            Arc::new(FakeBrowser {
                url: BLANK_PAGE.to_owned(),
                ..FakeBrowser::default()
            }),
        ] {
            let out = ShowBrowser
                .call(serde_json::json!({}), browser_ctx(browser))
                .await;
            let ToolOutcome::Failed {
                error_for_model, ..
            } = out
            else {
                panic!("没有页面就不该成功：{out:?}");
            };
            assert!(
                error_for_model.contains("BrowserNavigate"),
                "要指回导航：{error_for_model}"
            );
        }
    }

    #[tokio::test]
    async fn 有页面时点名地址且不回显内容() {
        let browser = Arc::new(FakeBrowser {
            url: "https://例子.test/看板".to_owned(),
            ..FakeBrowser::default()
        });
        let out = ShowBrowser
            .call(serde_json::json!({}), browser_ctx(browser))
            .await;
        let ToolOutcome::Ok { model_content, .. } = out else {
            panic!("该成功");
        };
        let text = format!("{model_content:?}");
        assert!(text.contains("例子.test"), "要点名当前地址：{text}");
        assert!(
            text.contains("BrowserSnapshot"),
            "要说清它不给页面内容：{text}"
        );
    }
}
