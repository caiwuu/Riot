//! SwitchMode：模型主动建议换工作方式（对照 Cursor 的 SwitchMode）。
//!
//! # 它解决什么
//!
//! 用户在 agent 模式里丢过来一句"加个用户认证"。这活有取舍（session 还是
//! JWT、存哪、中间件怎么接）、动的文件多，直接开干多半要推翻重来。Cursor
//! 的做法是让模型**先建议切到规划模式**，用户点「切换」才切 —— 模型判断
//! "该不该先规划"，用户保留否决权。这个工具就是那张卡的后端。
//!
//! # 它怎么工作
//!
//! 和 AskUserQuestion 同一条路：[`Tool::check_permissions`] 返回 `Ask`，
//! 理由是 [`DecisionReason::UserChoice`]，`suggestions` 里带一条
//! [`PermissionUpdate::SetMode`]。宿主把它渲染成对话流里的一张卡（说明 +
//! 「切换」/「拒绝」）；用户点「切换」，宿主在放行时顺手把 SetMode 落到
//! 会话的 mode_live 上 —— **同一轮内立即生效**，模型的下一个工具调用已经
//! 按新模式判定（规划模式下 CreatePlan 放行、Edit 代码被拒）。拒绝时
//! 宿主给模型一句"用户不想切，别再问"。
//!
//! `[约束]` 理由必须是 `UserChoice`：它让「全部放行」和无人值守也把卡片
//! 弹出来（切模式是用户的决定，不是权限问题），也让 Auto 模式的判危不去
//! 替用户答。换成 `Consent` 的话，开着 bypass 的用户会发现模式自己变了。
//!
//! 切进规划模式的结果里带完整的规划准则（[`plan_mode_rules`]）：模型在
//! 这一轮里就要照着走，等不到下一条用户消息的提醒。

use async_trait::async_trait;
use serde::Deserialize;

use riot_protocol::message::ToolResultContent;
use riot_protocol::permission::{
    DecisionReason, PermissionContext, PermissionMode, PermissionResult, PermissionUpdate,
    UpdateScope,
};
use riot_protocol::text::UiText;
use riot_protocol::tool::{
    PromptContext, ResultBudget, Tool, ToolContext, ToolOutcome, UiPayload, ValidationError,
};
use riot_protocol::ui_text;

use super::names::{ASK_USER_QUESTION, CREATE_PLAN, GLOB, GREP, READ, SWITCH_MODE, TODO_WRITE};
use super::plan::plan_mode_rules;

/// 能切去的两种工作方式。多任务是独立开关不在这里；权限档（默认 /
/// 编辑放行 / 全部放行……）是用户的事，模型无权建议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TargetMode {
    Agent,
    Plan,
}

impl TargetMode {
    fn id(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Plan => "plan",
        }
    }
}

#[derive(Deserialize, schemars::JsonSchema)]
struct Input {
    /// The mode to switch to.
    target_mode_id: TargetMode,
    /// One or two sentences for the user: what about this task makes the other
    /// mode the better fit. Shown on the confirmation card.
    explanation: String,
}

pub struct SwitchMode;

/// 从工具入参里取目标模式，供宿主渲染卡片（说明文字走 preview_of）。
pub fn target_of(input: &serde_json::Value) -> Option<TargetMode> {
    serde_json::from_value::<Input>(input.clone())
        .ok()
        .map(|i| i.target_mode_id)
}

/// 卡片上的说明文字。
pub fn explanation_of(input: &serde_json::Value) -> Option<String> {
    input
        .get("explanation")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

#[async_trait]
impl Tool for SwitchMode {
    fn name(&self) -> &str {
        SWITCH_MODE
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        // 措辞对照 Cursor 的 SwitchMode 描述（何时切 / 何时别切 / 各模式的
        // 判据和例子 / 用户必须点头），按我们只有 agent 与 plan 两档改写。
        format!(
            "Switch the working mode to better match the current task. The user must confirm: \
             this call shows them a card with your explanation and a Switch button, and nothing \
             changes until they press it. If they decline, the result says so — do not ask again \
             in this conversation; continue in the current mode.\n\
             \n\
             ## When to switch\n\
             Switch proactively when:\n\
             1. **Planning needed** — the task is large, ambiguous, or has significant trade-offs \
             to settle before code is written.\n\
             2. **Complexity emerges** — what looked simple turns out to involve architectural \
             decisions or several viable approaches.\n\
             3. **The request changes shape** — the user moves from refining a plan to asking for \
             the code, or the reverse.\n\
             4. **You are stuck** — several attempts without progress suggest stepping back to plan.\n\
             \n\
             ## When NOT to switch\n\
             - Simple, clear tasks you can finish quickly in the current mode.\n\
             - Mid-implementation while you are making good progress.\n\
             - Minor clarifying questions — just ask them ({ASK_USER_QUESTION}).\n\
             - The current mode is working.\n\
             \n\
             ## Modes\n\
             ### plan — Plan mode\n\
             Read-only collaborative mode for designing the approach before coding. You research \
             with {READ} / {GREP} / {GLOB}, settle the decisions, then call {CREATE_PLAN}; the \
             user reviews the plan file beside the conversation and presses Build when satisfied.\n\
             Switch to plan when: there are multiple valid approaches with significant trade-offs; \
             architectural decisions are needed (e.g. \"add caching\" — Redis vs in-memory vs \
             file-based); the change touches many files or systems (large refactors, migrations); \
             requirements are unclear and you need to explore before you can scope the work; you \
             would otherwise ask several clarifying questions.\n\
             Examples: \"Add user authentication\" → plan (session vs JWT, storage, middleware). \
             \"Refactor the database layer\" → plan (large scope, architectural impact). \"Make \
             the app faster\" → plan (profile first, several optimization strategies). \"Add a \
             comment to this function\" → stay in agent.\n\
             \n\
             ### agent — Agent mode\n\
             Default implementation mode with full tool access.\n\
             Switch to agent when: you are in plan mode and the user explicitly asks you to \
             implement the plan now (\"go ahead\", \"do it\", \"ship it\") with no further changes \
             attached. The Build button does this for them; this tool is for when they say it in \
             the chat instead.\n\
             \n\
             ## Notes\n\
             - Be proactive: do not wait for the user to ask for a plan. Suggesting plan mode for \
             a big task is a favor to them; starting a large change without one is not.\n\
             - Keep `explanation` to one or two sentences the user can judge: what makes this a \
             planning task.\n\
             - Do not over-switch. If the current mode is working, stay in it."
        )
    }

    fn describe(&self, input: &serde_json::Value) -> UiText {
        match target_of(input) {
            Some(t) => ui_text!("tools.mode.switchTo", mode = t.id()),
            None => ui_text!("tools.mode.switchAny"),
        }
    }

    /// 不碰任何文件；切模式是用户在卡片上做的决定。
    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    /// 两张卡叠在对话末尾，用户答完第一张才看得见第二张。
    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        false
    }

    fn result_budget(&self) -> ResultBudget {
        ResultBudget::Unlimited
    }

    /// 已经在目标模式 → 拒（不打扰用户）；否则问用户。
    ///
    /// 建议里的 `SetMode` 是给宿主落地用的默认值：切 plan 就是 plan；切 agent
    /// 给最严的 Default —— 前端知道用户进规划前用的是哪一档（编辑放行 /
    /// 自动判危……），会在应答里换成那一档。这里给 Default 只是兜底，
    /// 不能在这里替用户放宽。
    fn check_permissions(
        &self,
        input: &serde_json::Value,
        ctx: &PermissionContext,
    ) -> PermissionResult {
        // 参数坏了交给 validate_input 报（它在闸之前跑）。
        let Some(target) = target_of(input) else {
            return PermissionResult::Passthrough;
        };
        let current = ctx.mode.get();
        let in_plan = current == PermissionMode::Plan;
        let already = match target {
            TargetMode::Plan => in_plan,
            TargetMode::Agent => !in_plan,
        };
        if already {
            return PermissionResult::Deny {
                message: format!(
                    "Already in {} mode — no switch needed. Continue with the task.",
                    target.id()
                ),
                reason: DecisionReason::Mode { mode: current },
            };
        }
        let mode = match target {
            TargetMode::Plan => PermissionMode::Plan,
            TargetMode::Agent => PermissionMode::Default,
        };
        PermissionResult::Ask {
            message: ui_text!("tools.mode.askSwitch", mode = target.id()),
            suggestions: vec![PermissionUpdate::SetMode {
                mode,
                scope: UpdateScope::Session,
            }],
            reason: DecisionReason::UserChoice { remembered: false },
        }
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        let parsed: Input = serde_json::from_value(input.clone()).map_err(|e| {
            ValidationError::rejected(format!(
                "参数不对：{e}。target_mode_id 只能是 \"plan\" 或 \"agent\"，explanation 是给用户看的一两句话。"
            ))
        })?;
        if parsed.explanation.trim().is_empty() {
            return Err(ValidationError::rejected(
                "explanation 是空的。用户要看着理由决定切不切 —— 一两句话说清这个任务为什么该换模式。",
            ));
        }
        Ok(())
    }

    async fn call(&self, input: serde_json::Value, _ctx: ToolContext) -> ToolOutcome {
        // 走到这里说明用户点了「切换」，宿主已经把 mode_live 改了。
        let Some(target) = target_of(&input) else {
            return ToolOutcome::failed("参数解析不了，重新给 target_mode_id 和 explanation。");
        };
        let text = match target {
            TargetMode::Plan => format!(
                "The user agreed. You are now in plan mode for the rest of this conversation \
                 (until they press Build or switch back).\n\n{}",
                plan_mode_rules()
            ),
            TargetMode::Agent => format!(
                "The user agreed. Plan mode is over; you are in agent mode now. If a plan exists \
                 (you created it with {CREATE_PLAN}), its current content and path are attached \
                 to the user's latest message as `<plan_file>`, with the plan's todos listed under \
                 it — implement that: materialize those todos with {TODO_WRITE} (same items, same \
                 wording, in order; derive them from the steps only if the plan defines none), \
                 work through them, and verify as the plan says. If the attachment is missing, \
                 {READ} the plan file first. Do not re-ask for approval."
            ),
        };
        ToolOutcome::Ok {
            model_content: ToolResultContent::text(text),
            ui_payload: Some(UiPayload::Message {
                text: ui_text!("tools.mode.switched", mode = target.id()),
            }),
            side_messages: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use riot_protocol::id::{SessionId, ToolUseId};
    use riot_protocol::permission::PermissionModeState;
    use tokio_util::sync::CancellationToken;

    use super::*;

    /// 这个工具不碰 fs / proc / 网络，上下文里的注入项全给占位实现。
    fn ctx() -> ToolContext {
        let id = ToolUseId::from_raw("t1");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        ToolContext {
            session_id: SessionId::from_raw("s1"),
            tool_use_id: id.clone(),
            cwd: "/work".into(),
            artifacts_dir: "/artifacts".into(),
            cancel: CancellationToken::new(),
            progress: riot_protocol::tool::ProgressSink::new(id, tx),
            file_state: Arc::new(crate::tools::memfs::MemFileState::new()),
            fs: Arc::new(crate::tools::memfs::MemFs::new()),
            proc: Arc::new(crate::testing::NullProc),
            web: Arc::new(riot_protocol::web::NoWeb),
            browser: Arc::new(riot_protocol::browser::NoBrowser),
            terminal: Arc::new(riot_protocol::terminal::NoTerminal),
            vision: Arc::new(riot_protocol::vision::NoVision),
            clock: Arc::new(crate::testing::FixedClock::default()),
        }
    }

    fn perm(mode: PermissionMode) -> PermissionContext {
        PermissionContext {
            mode: PermissionModeState(Some(mode)),
            rules: Vec::new(),
            sandboxed: false,
            can_prompt_user: true,
        }
    }

    fn input(target: &str) -> serde_json::Value {
        serde_json::json!({ "target_mode_id": target, "explanation": "改动面大，先定方案。" })
    }

    /// 切 plan 的询问要带 SetMode(plan)，理由是 UserChoice —— 这是和决策链
    /// 约好的暗号（bypass / 无人值守下也要把卡弹出来）。
    #[test]
    fn 从_agent_切_plan_是带_set_mode_的用户选择() {
        let r = SwitchMode.check_permissions(&input("plan"), &perm(PermissionMode::Default));
        let PermissionResult::Ask {
            suggestions,
            reason,
            ..
        } = r
        else {
            panic!("该问用户：{r:?}");
        };
        assert_eq!(
            suggestions,
            vec![PermissionUpdate::SetMode {
                mode: PermissionMode::Plan,
                scope: UpdateScope::Session,
            }]
        );
        assert!(
            matches!(reason, DecisionReason::UserChoice { .. }),
            "理由要是 UserChoice：{reason:?}"
        );
        assert!(
            !reason.yields_to_bypass(),
            "切模式不能被「全部放行」替用户答"
        );
    }

    #[test]
    fn 已在目标模式时拒绝而不打扰用户() {
        let r = SwitchMode.check_permissions(&input("plan"), &perm(PermissionMode::Plan));
        assert!(matches!(r, PermissionResult::Deny { .. }), "{r:?}");
        let r = SwitchMode.check_permissions(&input("agent"), &perm(PermissionMode::AcceptEdits));
        assert!(matches!(r, PermissionResult::Deny { .. }), "{r:?}");
    }

    /// 走完整决策链：每种模式下（含规划模式的写拦截、无人值守的放行收敛）
    /// 卡片都得弹出来。少一种，用户就会发现模式"自己变了"或者模型白等。
    #[test]
    fn 决策链在各模式下都把询问问出去() {
        for (mode, target) in [
            (PermissionMode::Default, "plan"),
            (PermissionMode::AcceptEdits, "plan"),
            (PermissionMode::Auto, "plan"),
            (PermissionMode::BypassPermissions, "plan"),
            (PermissionMode::Unattended, "plan"),
            (PermissionMode::Plan, "agent"),
        ] {
            let r = riot_permissions::decide(
                &SwitchMode,
                &input(target),
                &perm(mode),
                &riot_permissions::RuleSet::default(),
            );
            assert!(
                matches!(r, PermissionResult::Ask { .. }),
                "{mode:?} → {target} 必须仍是询问，实际：{r:?}"
            );
        }
    }

    #[test]
    fn 没人能答时收敛成拒绝() {
        // 后台分叉出来的子 agent 没有界面。切模式对它没有意义，拒了就行。
        let mut ctx = perm(PermissionMode::Default);
        ctx.can_prompt_user = false;
        let r = riot_permissions::decide(
            &SwitchMode,
            &input("plan"),
            &ctx,
            &riot_permissions::RuleSet::default(),
        );
        assert!(matches!(r, PermissionResult::Deny { .. }), "{r:?}");
    }

    #[tokio::test]
    async fn 切进规划模式的结果带完整准则() {
        let out = SwitchMode.call(input("plan"), ctx()).await;
        let ToolOutcome::Ok { model_content, .. } = out else {
            panic!("该成功：{out:?}");
        };
        let text = format!("{model_content:?}");
        assert!(text.contains("plan mode"), "{text}");
        assert!(text.contains(CREATE_PLAN), "要指路 CreatePlan：{text}");
    }

    #[test]
    fn 描述里不写_task_大写单词() {
        // registry 的封闭词表测试把 `Task` 当工具名查；主 agent 的 Task 由
        // 内核另注册，builtin() 里没有，描述里出现它就是一条红。
        let p = SwitchMode.prompt(&PromptContext {
            cwd: "/work".into(),
            platform: "macos".into(),
            sandboxed: false,
            sibling_tools: Vec::new(),
            today: "2026年9月".into(),
        });
        assert!(!p.contains("Task "), "{p}");
    }
}
