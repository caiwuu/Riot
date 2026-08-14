//! ExitPlanMode：规划模式的出口。
//!
//! # 闭环（对照 Claude Code 的 ExitPlanMode）
//!
//! 规划模式下模型只读侦察、写出计划；计划成熟后调用这个工具**提交计划
//! 等待批准**。批准走的就是普通的权限弹窗管线：
//!
//! - `check_permissions` 返回 `Ask`，理由是 [`DecisionReason::Consent`] ——
//!   决策链会把它暂存并在 mode_default 的 Plan-Deny **之前**兑现
//!   （chain.rs 第 3 → 6.5 步），所以规划模式挡不住它自己的出口；
//! - `suggestions` 带两个 [`PermissionUpdate::SetMode`]（自动接受编辑 /
//!   逐步确认），弹窗按它渲染成两个批准按钮；用户点哪个，宿主的
//!   HostGate 就把会话切到哪个模式 —— **同一轮内立即生效**，模型的
//!   下一个工具调用已经按新模式判定；
//! - 拒绝时用户的反馈会作为 tool_result 喂回模型，会话停在规划模式。
//!
//! `[取舍]` 计划以**参数**传入而不是写在磁盘文件里（CC v2 用 plan 文件，
//! 支持增量编辑和跨会话引用）。参数版是 CC v1 的形状：少一整套"计划文件
//! 在哪、怎么改、压缩后怎么引用"的状态面，而计划本身在 tool_use 输入里
//! 进了历史和 transcript，不会丢。等有真实的"改计划再提交"需求再升级。

use async_trait::async_trait;
use serde::Deserialize;

use riot_protocol::permission::{
    DecisionReason, PermissionContext, PermissionMode, PermissionResult, PermissionUpdate,
    UpdateScope,
};
use riot_protocol::tool::{PromptContext, Tool, ToolContext, ToolOutcome, UiPayload};

#[derive(Deserialize, schemars::JsonSchema)]
struct Input {
    /// 完整的实施计划（Markdown）。用户会逐字看到它 —— 写清楚要做什么、
    /// 动哪些文件、按什么顺序、怎么验证。
    ///
    /// 字段本身只在反序列化时校验存在性（call 里 plan 的消费者是
    /// preview_of 和弹窗，不是这段代码），所以这里 allow dead_code。
    #[allow(dead_code)]
    plan: String,
}

pub struct ExitPlanMode;

#[async_trait]
impl Tool for ExitPlanMode {
    fn name(&self) -> &str {
        "ExitPlanMode"
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        // 措辞对照 CC：明确"这就是问'计划可以吗'的唯一方式"，否则模型
        // 会用普通文本问一句然后干等 —— 那条路上没有任何按钮。
        "规划完成、准备好让用户批准时调用。把完整的实施计划（Markdown）放进 \
         plan 参数：做什么、动哪些文件、什么顺序、怎么验证。用户会看到计划\
         原文并选择批准或打回。\n\n\
         重要：不要用普通回复问「这个计划可以吗？」「要开始吗？」——提交计划\
         就是问这句话的唯一方式。批准后规划模式自动退出，你就可以动手了；\
         被打回则继续留在规划模式，按用户的反馈修改计划再提交。"
            .into()
    }

    fn describe(&self, _input: &serde_json::Value) -> String {
        "提交计划等待批准".into()
    }

    /// 不是只读：批准会切换权限模式。靠 `check_permissions` 的 Ask 在
    /// mode_default 之前兑现，规划模式的写拦截碰不到它。
    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        false
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        PermissionResult::Ask {
            message: "计划已就绪，批准后退出规划模式开始执行".into(),
            // 顺序即弹窗按钮顺序：CC 的默认主选项是"自动接受编辑"——
            // 批准计划之后再逐个确认每次编辑，等于把刚做的决定再问一遍。
            suggestions: vec![
                PermissionUpdate::SetMode {
                    mode: PermissionMode::AcceptEdits,
                    scope: UpdateScope::Session,
                },
                PermissionUpdate::SetMode {
                    mode: PermissionMode::Default,
                    scope: UpdateScope::Session,
                },
            ],
            reason: DecisionReason::Consent { what: "按计划开始执行".into() },
        }
    }

    async fn call(&self, input: serde_json::Value, _ctx: ToolContext) -> ToolOutcome {
        // 走到这里说明用户已批准（gate 在 call 之前）。计划全文已经在
        // tool_use 输入里进了历史，结果只需要交代状态和下一步 ——
        // 重复一遍计划是白花的 token。
        let ok: Result<Input, _> = serde_json::from_value(input);
        if let Err(e) = ok {
            return ToolOutcome::failed(format!("参数不对：{e}。把计划放进 plan 字段。"));
        }
        ToolOutcome::Ok {
            model_content: riot_protocol::message::ToolResultContent::text(
                "用户已批准计划，规划模式已退出。开始执行：如果任务有多步，\
                 先用 TodoWrite 把计划落成待办清单，然后按顺序动手。",
            ),
            ui_payload: Some(UiPayload::Plain { text: "计划已批准，开始执行".into() }),
            side_messages: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 询问带两档模式建议_理由是同意() {
        let r = ExitPlanMode.check_permissions(
            &serde_json::json!({ "plan": "……" }),
            &PermissionContext::default(),
        );
        let PermissionResult::Ask { suggestions, reason, .. } = r else {
            panic!("必须走询问，批准是用户的事：{r:?}");
        };
        assert_eq!(
            suggestions,
            vec![
                PermissionUpdate::SetMode {
                    mode: PermissionMode::AcceptEdits,
                    scope: UpdateScope::Session,
                },
                PermissionUpdate::SetMode {
                    mode: PermissionMode::Default,
                    scope: UpdateScope::Session,
                },
            ],
            "两个批准档位，自动接受编辑在前（批准计划后再逐个确认编辑，等于把刚做的决定再问一遍）"
        );
        assert!(
            reason.yields_to_bypass(),
            "必须是 Consent 类理由 —— 决策链靠它把询问暂存到 mode_default 之前兑现，\
             否则规划模式的写拦截会把出口自己拦死"
        );
    }

    #[tokio::test]
    async fn 规划模式的决策链放它过() {
        // 集成断言：在 Plan 模式下走完整决策链，出口工具必须得到 Ask
        // 而不是被 mode_default 的 Plan-Deny 拦下。这条链的顺序改了的话，
        // 规划模式会变成一个进得去出不来的死胡同。
        use riot_protocol::permission::PermissionModeState;

        let ctx = PermissionContext {
            mode: PermissionModeState(Some(PermissionMode::Plan)),
            rules: Vec::new(),
            sandboxed: false,
            can_prompt_user: true,
        };
        let rules = riot_permissions::RuleSet::default();
        let r = riot_permissions::decide(
            &ExitPlanMode,
            &serde_json::json!({ "plan": "x" }),
            &ctx,
            &rules,
        );
        assert!(
            matches!(r, PermissionResult::Ask { .. }),
            "Plan 模式下 ExitPlanMode 必须能问出去：{r:?}"
        );
    }
}
