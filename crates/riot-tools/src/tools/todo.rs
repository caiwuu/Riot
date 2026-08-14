//! TodoWrite：结构化的任务清单。
//!
//! # 形状（对照 Claude Code 的 TodoWriteTool）
//!
//! - 输入是**整表替换**：`{ todos: [{ content, status, activeForm }] }`，
//!   没有 id、没有增量操作 —— 状态机在模型的上下文里，工具只是把它
//!   摆出来给用户看。
//! - 每项两种措辞：`content` 用祈使式（"跑测试"），`activeForm` 用进行式
//!   （"正在跑测试"）—— 界面上进行中的条目用后者，读起来才像状态而不是命令。
//! - 成功返回**固定短文案**，不回显清单：清单已经在 tool_use 输入里进了
//!   上下文，回显一遍是白花的 token（CC 同款）。
//! - 权限显式 Allow：记待办没有副作用面，弹窗只会训练用户无脑点允许。
//!   显式 Allow 也让它在**规划模式可用** —— 规划期间列任务清单正是
//!   该鼓励的行为。
//!
//! `[取舍]` 语义规则（"恰好一个 in_progress"、"完成立刻标记"）写在 prompt
//! 里而**不在代码里强制** —— CC 也不强制。硬校验的代价是模型在补记、
//! 重排这类合理操作上反复被打回；prompt 约束加界面呈现已经足够形成压力。
//! 代码只拦真正的坏数据：空文本、未知状态。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use riot_protocol::permission::{DecisionReason, PermissionContext, PermissionResult};
use riot_protocol::tool::{PromptContext, Tool, ToolContext, ToolOutcome, UiPayload};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    /// 祈使式描述，如「跑全部测试」。
    pub content: String,
    pub status: TodoStatus,
    /// 进行式描述，如「正在跑全部测试」。执行中的条目在界面上用它。
    pub active_form: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct Input {
    /// 完整的新清单（整表替换，不是增量）。
    todos: Vec<TodoItem>,
}

pub struct TodoWrite;

#[async_trait]
impl Tool for TodoWrite {
    fn name(&self) -> &str {
        "TodoWrite"
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        // CC 的 prompt 精华，保留全部行为规则，压掉冗长的示例。
        "维护本次会话的任务清单。主动、频繁地用它 —— 它让你自己不漏步骤，\
         也让用户看得到进展。\n\n\
         ## 什么时候用\n\
         - 任务要三步以上，或不是一眼能做完的\n\
         - 用户给了多件事（列表、逗号分隔）\n\
         - 收到新指令时：立刻把需求记成待办\n\
         - 开始做某一项前：把它标成 in_progress（同一时刻**只有一项**进行中）\n\
         - 做完一项：**立刻**标 completed，不要攒一批再改\n\n\
         ## 什么时候不用\n\
         单一直白的小事、纯问答 —— 直接做比记录更快。\n\n\
         ## 状态与措辞\n\
         - status: pending / in_progress / completed\n\
         - content 用祈使式（「跑测试」），activeForm 用进行式（「正在跑测试」），两者都必填\n\
         - 每次调用传**完整的新清单**（整表替换），不再相关的条目直接删掉\n\n\
         ## 完成的标准\n\
         只有真正做完才标 completed。测试在红、实现了一半、卡在报错上 —— \
         都保持 in_progress，另加一条描述阻塞的待办。"
            .into()
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let n = input
            .get("todos")
            .and_then(|v| v.as_array())
            .map_or(0, Vec::len);
        format!("更新任务清单（{n} 项）")
    }

    /// 显式放行：记待办没有副作用面。这也让它在规划模式可用 ——
    /// 规划期间把计划落成清单正是该鼓励的行为（决策链在 mode_default
    /// 之前就兑现工具自己的 Allow）。
    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Preapproved { what: "任务清单".into() },
        }
    }

    async fn call(&self, input: serde_json::Value, _ctx: ToolContext) -> ToolOutcome {
        let input: Input = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => {
                return ToolOutcome::failed(format!(
                    "参数不对：{e}。形状是 {{\"todos\": [{{\"content\", \"status\", \"activeForm\"}}]}}，\
                     status 取 pending / in_progress / completed。"
                ));
            }
        };
        // 只拦真正的坏数据。空文本进了清单，界面上就是一行空白 ——
        // 而模型看到"第 N 项 content 是空的"能立刻改对。
        for (i, t) in input.todos.iter().enumerate() {
            if t.content.trim().is_empty() {
                return ToolOutcome::failed(format!("第 {} 项的 content 是空的", i + 1));
            }
            if t.active_form.trim().is_empty() {
                return ToolOutcome::failed(format!(
                    "第 {} 项缺 activeForm（进行式措辞，如「正在跑测试」）",
                    i + 1
                ));
            }
        }

        let total = input.todos.len();
        let done = input.todos.iter().filter(|t| t.status == TodoStatus::Completed).count();
        let doing = input
            .todos
            .iter()
            .find(|t| t.status == TodoStatus::InProgress)
            .map(|t| t.active_form.clone());

        ToolOutcome::Ok {
            // 固定文案，不回显清单（它已经在 tool_use 输入里）。CC 同款
            // 措辞意图：确认 + 提醒继续用。
            model_content: riot_protocol::message::ToolResultContent::text(
                "清单已更新。继续用它跟踪进度：开工前标 in_progress，做完立刻标 completed。",
            ),
            ui_payload: Some(UiPayload::Plain {
                text: match doing {
                    Some(active) => format!("{done}/{total} 完成 · {active}"),
                    None => format!("{done}/{total} 完成"),
                },
            }),
            side_messages: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FixedClock, NullFileState, NullFs, NullProc};
    use riot_protocol::id::{SessionId, ToolUseId};
    use riot_protocol::permission::PermissionModeState;
    use riot_protocol::tool::ProgressSink;
    use std::sync::Arc;

    fn ctx() -> ToolContext {
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
            fs: Arc::new(NullFs),
            proc: Arc::new(NullProc),
            web: Arc::new(riot_protocol::web::NoWeb),
            browser: Arc::new(riot_protocol::browser::NoBrowser),
            vision: Arc::new(riot_protocol::vision::NoVision),
            clock: Arc::new(FixedClock::default()),
        }
    }

    fn todos(json: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "todos": json })
    }

    #[tokio::test]
    async fn 正常清单返回固定文案不回显() {
        let out = TodoWrite
            .call(
                todos(serde_json::json!([
                    { "content": "跑测试", "status": "completed", "activeForm": "正在跑测试" },
                    { "content": "修 bug", "status": "in_progress", "activeForm": "正在修 bug" },
                ])),
                ctx(),
            )
            .await;
        let ToolOutcome::Ok { model_content, ui_payload, .. } = out else {
            panic!("该成功");
        };
        let text = format!("{model_content:?}");
        assert!(text.contains("清单已更新"));
        assert!(
            !text.contains("跑测试"),
            "不回显清单 —— 它已经在 tool_use 输入里，回显是白花的 token"
        );
        // UI 摘要带进行中的措辞（进行式）
        assert!(format!("{ui_payload:?}").contains("正在修 bug"));
    }

    #[tokio::test]
    async fn 空文本被拦_报第几项() {
        let out = TodoWrite
            .call(
                todos(serde_json::json!([
                    { "content": "好的", "status": "pending", "activeForm": "x" },
                    { "content": "  ", "status": "pending", "activeForm": "x" },
                ])),
                ctx(),
            )
            .await;
        let ToolOutcome::Failed { error_for_model, .. } = out else {
            panic!("该失败");
        };
        assert!(error_for_model.contains("第 2 项"), "要点名第几项：{error_for_model}");
    }

    #[tokio::test]
    async fn 未知状态给出取值范围() {
        let out = TodoWrite
            .call(
                todos(serde_json::json!([
                    { "content": "x", "status": "doing", "activeForm": "x" },
                ])),
                ctx(),
            )
            .await;
        let ToolOutcome::Failed { error_for_model, .. } = out else {
            panic!("该失败");
        };
        assert!(
            error_for_model.contains("in_progress"),
            "报错要带合法取值，模型才能一次改对：{error_for_model}"
        );
    }

    #[test]
    fn 显式放行且规划模式可用() {
        // 弹窗确认一个待办清单只会训练用户无脑点允许；规划模式里列清单
        // 更是该鼓励的行为。走完整决策链断言,不只断言工具自己的返回值。
        let ctx = PermissionContext {
            mode: PermissionModeState(Some(riot_protocol::permission::PermissionMode::Plan)),
            rules: Vec::new(),
            sandboxed: false,
            can_prompt_user: true,
        };
        let r = riot_permissions::decide(
            &TodoWrite,
            &todos(serde_json::json!([])),
            &ctx,
            &riot_permissions::RuleSet::default(),
        );
        assert!(
            r.is_allow(),
            "规划模式下 TodoWrite 必须静默放行：{r:?}"
        );
    }
}
