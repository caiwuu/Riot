//! CreatePlan：规划模式的产物 —— 把计划写成项目里的一份文件。
//!
//! # 闭环（对照 Cursor 的 CreatePlan + Build）
//!
//! 规划模式下模型只读侦察、想清楚做法，然后调用这个工具把计划落成
//! `<项目>/.riot/plans/<名>.plan.md`。它**不阻塞、不弹窗**：文件写下就
//! 返回，模型收一句"计划已保存，用一两句话收尾"，回合结束。界面那头
//! 认出这次调用，在右侧抽屉开一枚「计划」标签渲染这份文件，输入框的
//! 发送键变成「构建」。之后两条路：
//!
//! - 用户在输入框里提意见 → 仍在规划模式，模型 Read 计划文件、用 Edit
//!   改它（规划模式唯一放行的写操作，见 `riot_permissions::chain`），
//!   面板跟着刷新；
//! - 用户点「构建」→ 界面把权限模式切回执行档、开一轮带
//!   [`riot_protocol::Nudge::BuildPlan`] 的消息，模型读回文件开始动手。
//!
//! 旧版（对照 Claude Code 的 ExitPlanMode）把计划塞在工具参数里、用权限
//! 弹窗的"批准"当出口。那条路的问题是计划只能整份重交：用户说"端口改成
//! 8200"，模型得把三页纸再生成一遍；而且批准卡长在对话流里，计划越长
//! 对话越难读。文件版两个问题都没有：改计划是一次 Edit，读计划在侧栏。
//!
//! `[约束]` 这个工具**只在规划模式可用**，但要**常驻注册**：模型在轮中
//! 经 SwitchMode 切进规划模式后，同一轮就要能调它，而工具清单是开轮时
//! 定死的。其它模式下由 [`Tool::check_permissions`] 拒掉并指路 SwitchMode。
//!
//! `[约束]` 给模型的结果**第一行**固定是 `Plan file: <相对路径>`
//! （[`PLAN_FILE_LINE_PREFIX`]）。前端就靠这一行知道文件在哪
//! （`src/lib/plan.ts`）—— 工具的 ui_payload 到不了前端，路径又是这里
//! 现编的（名字 + 调用 id），两边只能约定一个可解析的行。改这里必须改那边。

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use riot_protocol::message::ToolResultContent;
use riot_protocol::permission::{
    DecisionReason, PermissionContext, PermissionMode, PermissionResult,
};
use riot_protocol::text::UiText;
use riot_protocol::tool::{
    FileState, FileView, PromptContext, ResultBudget, Tool, ToolContext, ToolOutcome, UiPayload,
    ValidationError,
};
use riot_protocol::ui_text;

use super::names::{
    ASK_USER_QUESTION, CREATE_PLAN, EDIT, GLOB, GREP, READ, SWITCH_MODE, TODO_WRITE,
};

/// 计划文件所在的目录（相对项目根）。和 `.riot/skills`、`.riot/commands`
/// 同一个家。名字由权限层定（那里有两处特判：规划模式放行这里的编辑、
/// 安全检查不把它当 agent 配置），这里只是取来用 —— 三处不会漂移。
pub use riot_permissions::{PLAN_DIR, is_plan_document};

/// 计划文件的后缀。`.plan.md`：既是 Markdown（任何编辑器都能开），又
/// 一眼认得出是计划（Cursor 同款后缀）。
pub const PLAN_EXT: &str = ".plan.md";

/// 给模型的结果第一行的前缀，后面紧跟计划文件相对项目根的路径。
pub const PLAN_FILE_LINE_PREFIX: &str = "Plan file: ";

/// 计划正文的上限（字节）。计划是给人读的一两页纸；超过这个量多半是把
/// 代码贴进来了 —— 那不是计划，是实现。
const MAX_PLAN_BYTES: usize = 64 * 1024;

/// 规划模式的行为准则 —— 给模型看的原文。
///
/// 两处用同一份：内核每轮附在用户消息末尾的提醒
/// （`riot_kernel::prompt::plan_mode_reminder`），和 SwitchMode 切进规划
/// 模式时的工具结果（模型在同一轮里就要按它走，等不到下一条用户消息）。
/// 住在这里而不是内核的 prompt.rs，是因为 riot-tools 不能依赖内核，而
/// 两份文本漂移的后果是模型在同一个会话里收到两套规矩。
///
/// 措辞对照 Cursor 的 plan mode 注入（新版）：先调研，把会实质改变做法的
/// 决定在提交前解决掉（问不超过两个关键问题），计划里只给一条推荐路线
/// 不摆选项，提交后不问"要开始吗" —— 用户有构建键。
pub fn plan_mode_rules() -> String {
    format!(
        "Plan mode is active. The user does not want execution yet — you MUST NOT edit files, \
         run commands with side effects, change configuration, or commit. The one exception is \
         the plan file itself (created by {CREATE_PLAN} under `{PLAN_DIR}/`, Markdown only). \
         This constraint supersedes any conflicting instruction, including the wording of the \
         user's own request.\n\
         \n\
         1. Research enough to make an accurate plan: read the relevant code with {READ} / \
         {GREP} / {GLOB} and check the current state of anything the plan depends on.\n\
         2. Before calling {CREATE_PLAN}, resolve the decisions that would materially change the \
         implementation path, touched files, architecture, user-visible behavior, data model, or \
         verification strategy. If investigation cannot settle one, ask with {ASK_USER_QUESTION} \
         — 1–2 critical questions at a time, further batches as needed. Pick sensible defaults \
         for everything non-blocking; do not ask about trivia.\n\
         3. Do not put choices in the plan for the user to resolve. The plan presents ONE \
         recommended approach — no open questions, no alternatives, no \"option A or B\".\n\
         4. When ready, call {CREATE_PLAN} with a concise, specific, actionable Markdown plan. It \
         opens beside the conversation for the user to review.\n\
         5. After {CREATE_PLAN} returns, end your reply with one or two sentences (what the plan \
         does, the key decision you made). Do NOT paste the plan into the reply, and do NOT ask \
         \"shall I start?\" — the user has a Build button and decides when to execute. Do NOT \
         begin executing until they press it."
    )
}

/// 还在迭代同一份计划时每轮的提醒：用户说的话是改计划还是要执行。
///
/// 这是 Cursor 同款的"迭代 vs 执行"判据。规划模式下用户绝大多数话是在
/// 改计划（"用 Redis 做缓存"是让你把它写进计划，不是让你去写代码）；
/// 只有明确指着计划本身说"执行"才是执行 —— 而执行的入口是 SwitchMode
/// 请用户确认，不是自己动手。
///
/// 内核只在**上一轮就在规划模式且计划文件在**时给这一版，并且把文件的
/// 当前内容和路径一起附在同一条消息里（`<plan_file>`）—— 所以这里不再
/// 让模型去翻"你之前的 CreatePlan 结果"（可能已被压缩掉）或先 Read 一遍。
pub fn plan_iteration_rules() -> String {
    format!(
        "Plan mode is still active and a plan file already exists — its path and current \
         content are attached to this message as `<plan_file>`. Tell plan iteration apart from \
         execution:\n\
         - The user is ITERATING when they give feedback, request changes, or describe how \
         something should work. In plan mode, actionable phrasing (\"use Redis for the cache\", \
         \"make the poller loop over shards\", \"add error handling for the timeout case\") means \
         ADD THIS TO THE PLAN, not write the code. When in doubt, assume iteration.\n\
         - Reflect every iteration in the plan file itself: apply targeted {EDIT} calls to the \
         attached file (editing the `{PLAN_EXT}` file is the one write allowed in plan mode; no \
         {READ} needed first — the attached copy is current). Do not paste the revised plan into \
         your reply — the user reads the file. Call {CREATE_PLAN} again only if they want a \
         fundamentally different plan.\n\
         - The user wants EXECUTION only when the message refers to the plan itself and tells \
         you to carry it out, with nothing else attached: \"go ahead and implement the plan\", \
         \"execute it\", \"ok, do it\", \"ship it\". Then call {SWITCH_MODE} with \
         target_mode_id=\"agent\" — the user confirms the switch, and only after that \
         confirmation may you edit code. If the message also contains changes, it is iteration, \
         not execution.\n\
         - Questions about the plan (\"what do you think?\", \"will this affect X?\") get an \
         answer in your reply — not an edit, not execution.\n\
         You still MUST NOT edit code, run commands with side effects, or commit until the mode \
         switch is confirmed."
    )
}

/// 计划里的一条待办（对照 Cursor CreatePlanArgs 的 `todos[]`，去掉了 id ——
/// Riot 的 TodoWrite 没有 id，靠措辞对上）。
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct PlanTodo {
    /// One implementation task: imperative, specific, actionable (e.g. "add the
    /// sessions table and its migration"). Use the same wording later in TodoWrite.
    pub content: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct Input {
    /// Short title for the plan (roughly 3–8 words). Shown as the heading and used
    /// to name the plan file.
    name: String,
    /// One or two sentences: what the plan achieves and the approach you chose.
    /// Shown right under the title.
    #[serde(default)]
    overview: Option<String>,
    /// The plan body in Markdown: implementation steps in order, the files and
    /// functions each step touches, important defaults and boundaries, and how
    /// the result will be verified. Do not repeat the title as a heading — the
    /// UI already shows `name`.
    plan: String,
    /// The implementation task list derived from the plan, in execution order.
    /// Provide it for any implementation plan unless the change is truly trivial
    /// (a simple plan gets a few high-level todos); leave it empty for a purely
    /// investigative plan. These become the todo list when the user presses Build.
    #[serde(default)]
    todos: Vec<PlanTodo>,
}

/// 一次 CreatePlan 调用参数里的待办措辞，按顺序。内核和前端从历史里的
/// tool_use 取待办时走这一个入口 —— 形状只在这个文件里定义。
/// 参数解析不出来（半截流、旧 transcript）就是空。
pub fn todos_of(input: &serde_json::Value) -> Vec<String> {
    input
        .get("todos")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|t| t.get("content").and_then(|c| c.as_str()))
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

pub struct CreatePlan;

/// 计划文件相对项目根的路径。文件名 = 标题的 slug + 调用 id 的尾巴：
/// slug 让用户在目录里认得出是哪份，id 尾巴让同名的两次提交不互相覆盖
/// （Cursor 用的是随机 8 位，这里用调用 id 是为了黄金回放可复现）。
pub fn plan_rel_path(name: &str, tool_use_id: &str) -> String {
    let alnum: Vec<char> = tool_use_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let suffix: String = alnum[alnum.len().saturating_sub(6)..]
        .iter()
        .collect::<String>()
        .to_ascii_lowercase();
    let suffix = if suffix.is_empty() {
        "plan".to_owned()
    } else {
        suffix
    };
    format!("{PLAN_DIR}/{}-{suffix}{PLAN_EXT}", slug(name))
}

/// 标题 → 文件名主干。字母数字（含中日韩文字）留下，其余折成 `-`，
/// 最多 40 个字符。空了就叫 `plan`。
fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in name.trim().chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
        if out.chars().count() >= 40 {
            break;
        }
    }
    let trimmed = out.trim_end_matches('-');
    if trimmed.is_empty() {
        "plan".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// 落盘的文件内容：标题、概述、正文。正文自己带了一级标题就不再加一个
/// （模型常常不听"别重复标题"）。
fn compose_document(name: &str, overview: Option<&str>, plan: &str) -> String {
    let body = plan.trim();
    let body = if body.starts_with("# ") {
        body.split_once('\n')
            .map_or("", |(_, rest)| rest)
            .trim_start()
    } else {
        body
    };
    let mut doc = format!("# {}\n", name.trim());
    if let Some(o) = overview.map(str::trim).filter(|o| !o.is_empty()) {
        doc.push('\n');
        doc.push_str(o);
        doc.push('\n');
    }
    doc.push('\n');
    doc.push_str(body);
    doc.push('\n');
    doc
}

#[async_trait]
impl Tool for CreatePlan {
    fn name(&self) -> &str {
        CREATE_PLAN
    }

    /// 旧 transcript 里的名字还要能被解析，否则历史会话打不开。
    fn aliases(&self) -> &[&'static str] {
        &["ExitPlanMode"]
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        // 措辞对照 Cursor 的 CreatePlan 描述：质量要求（一条路线、点名文件、
        // 带验证、和请求成比例）、更新方式（改文件不重交）、只在规划模式。
        format!(
            "Create the plan the user will review in plan mode. Call it once, at the end of \
             the planning phase, when the approach is settled. It writes the plan to \
             `{PLAN_DIR}/<name>{PLAN_EXT}` and opens it beside the conversation; the user \
             reads the file, then either asks for changes or presses Build.\n\
             \n\
             Only available in plan mode. In agent mode, either do the work directly, or — for \
             a large, ambiguous, or trade-off-heavy task — call {SWITCH_MODE} with \
             target_mode_id=\"plan\" and let the user decide; do not call this tool there.\n\
             \n\
             PLAN QUALITY:\n\
             - Concise, specific, and executable without re-deciding the approach.\n\
             - Commit to ONE recommended implementation path. No unresolved questions, options, \
             or TBDs — settle them before calling this (with {ASK_USER_QUESTION} if research \
             cannot).\n\
             - Name the primary files, functions, components, configs, tests, and existing \
             utilities or patterns to reuse. When mentioning a file, write a Markdown link with \
             the path relative to the working directory, e.g. `[src/auth.rs](src/auth.rs)` — the \
             UI turns it into a link that opens the file.\n\
             - Concrete implementation steps in order; brief context only where it supports a \
             step. Include important defaults, boundaries, and validation criteria.\n\
             - Include verification: tests, commands, manual checks, or the exact condition that \
             would trigger follow-up work.\n\
             - Proportional to the request. A small change gets a short plan.\n\
             - No emojis. No Markdown tables (use bullet lists).\n\
             - If the request is investigative and no code change is planned, say so explicitly \
             rather than inventing implementation steps.\n\
             \n\
             TASK ORGANIZATION:\n\
             - `todos` is the implementation task list derived from the plan, in execution \
             order: each one a clear, specific, actionable task. Provide it for any \
             implementation plan unless the change is truly trivial; a simple plan gets a few \
             high-level todos. Leave it empty for a purely investigative plan.\n\
             - The todos become the todo list when the user presses Build — you will then track \
             them with {TODO_WRITE} using the same items and wording. Do not call {TODO_WRITE} \
             for them while still planning.\n\
             \n\
             UPDATING THE PLAN:\n\
             - Each call creates a NEW plan file. To revise an existing plan, {READ} the file and \
             apply targeted {EDIT} calls to it — editing that file is allowed in plan mode. Call \
             this tool again only for a fundamentally different plan (that also replaces the \
             todos; small revisions to the steps are reconciled when the plan is built).\n\
             \n\
             AFTER IT RETURNS:\n\
             - Finish your reply with one or two sentences: what the plan does and the key \
             decision behind it. Do NOT paste the plan into the reply and do NOT ask \"shall I \
             start?\" — the user has a Build button. You stay in plan mode until they press it."
        )
    }

    fn describe(&self, input: &serde_json::Value) -> UiText {
        match input.get("name").and_then(|v| v.as_str()) {
            Some(n) if !n.trim().is_empty() => ui_text!("tools.plan.create", name = n.trim()),
            _ => ui_text!("tools.plan.createAny"),
        }
    }

    /// 写文件，不算只读。放行与否由下面的 check_permissions 按模式定。
    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        false
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        false
    }

    /// 结果就几行，落盘毫无意义。
    fn result_budget(&self) -> ResultBudget {
        ResultBudget::Unlimited
    }

    /// 规划模式放行，其它模式拒绝并指路 SwitchMode。
    ///
    /// 直接 Allow 而不是 Passthrough：Passthrough 会落到决策链的 mode_default，
    /// 那里规划模式对一切写操作是 Deny —— 这个工具就永远调不出来。理由
    /// 用 Mode：它确实是"因为在规划模式所以放行"。
    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        ctx: &PermissionContext,
    ) -> PermissionResult {
        let mode = ctx.mode.get();
        if mode == PermissionMode::Plan {
            PermissionResult::Allow {
                updated_input: None,
                reason: DecisionReason::Mode { mode },
            }
        } else {
            PermissionResult::Deny {
                message: format!(
                    "{CREATE_PLAN} only works in plan mode. If this task is large, ambiguous, or \
                     has real trade-offs, call {SWITCH_MODE} with target_mode_id=\"plan\" and \
                     let the user decide; otherwise just do the work."
                ),
                reason: DecisionReason::Mode { mode },
            }
        }
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        let parsed: Input = serde_json::from_value(input.clone())
            .map_err(|e| ValidationError::rejected(format!("参数不对：{e}。")))?;
        if parsed.name.trim().is_empty() {
            return Err(ValidationError::rejected(
                "name 是空的。给计划起一个 3–8 个词的标题。",
            ));
        }
        if parsed.plan.trim().is_empty() {
            return Err(ValidationError::rejected(
                "plan 是空的。把计划正文（Markdown）放进 plan 字段。",
            ));
        }
        if parsed.plan.len() > MAX_PLAN_BYTES {
            return Err(ValidationError::rejected(format!(
                "计划正文 {} KB，超过 {} KB 上限。计划是给人读的步骤和取舍，不是实现 —— \
                 去掉贴进来的代码，只留步骤、涉及的文件和验证方法。",
                parsed.plan.len() / 1024,
                MAX_PLAN_BYTES / 1024
            )));
        }
        // 只拦真正的坏数据：空文本进了清单，面板上就是一行空白。点名第几项，
        // 模型能一次改对。
        if let Some(i) = parsed
            .todos
            .iter()
            .position(|t| t.content.trim().is_empty())
        {
            return Err(ValidationError::rejected(format!(
                "todos 第 {} 项的 content 是空的。每一项是一条具体、可执行的任务措辞。",
                i + 1
            )));
        }
        Ok(())
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let parsed: Input = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolOutcome::failed(format!("参数不对：{e}。")),
        };
        let rel = plan_rel_path(&parsed.name, ctx.tool_use_id.as_str());
        let abs: PathBuf = ctx.cwd.join(&rel);
        let doc = compose_document(&parsed.name, parsed.overview.as_deref(), &parsed.plan);

        if let Err(e) = ctx.fs.write(&abs, doc.as_bytes()).await {
            return ToolOutcome::failed(format!(
                "写不了计划文件 {rel}：{e}。检查项目目录是否可写；写不进去的话把计划直接写在回复里。"
            ));
        }

        // 写完就是最新状态，直接进缓存：用户说"改一下第三步"，模型可以
        // 直接 Edit，不用先 Read 一遍。
        let mtime_ms = ctx.fs.metadata(&abs).await.map(|m| m.mtime_ms).unwrap_or(0);
        ctx.file_state.put(
            abs,
            FileState {
                content: doc,
                mtime_ms,
                view: FileView::Full,
            },
        );

        // 待办数说一声：它们随计划一起落定了，模型不该在规划期间再用
        // TodoWrite 建一遍（那会让输入框上方提前挂出一份全 pending 的清单）。
        let todos_note = match parsed.todos.len() {
            0 => String::new(),
            n => format!(
                " Its {n} todos are recorded with it and become the todo list when the user \
                 presses Build — do not call {TODO_WRITE} for them now."
            ),
        };
        ToolOutcome::Ok {
            model_content: ToolResultContent::text(format!(
                "{PLAN_FILE_LINE_PREFIX}{rel}\n\
                 The plan is saved and open beside the conversation for the user to \
                 review.{todos_note}\n\
                 \n\
                 Now finish your reply with one or two sentences (what the plan does, the key \
                 decision). Do NOT paste the plan, and do NOT ask \"shall I start?\" — the user \
                 has a Build button. You remain in plan mode: if they ask for changes, {READ} \
                 `{rel}` and apply targeted {EDIT} calls to it."
            )),
            ui_payload: Some(UiPayload::Message {
                text: ui_text!("tools.plan.saved", path = rel),
            }),
            side_messages: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use riot_protocol::id::{SessionId, ToolUseId};
    use riot_protocol::permission::PermissionModeState;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::tools::memfs::{MemFileState, MemFs};

    fn ctx(fs: Arc<MemFs>) -> ToolContext {
        let id = ToolUseId::from_raw("toolu_01AbC9xY");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        ToolContext {
            session_id: SessionId::from_raw("s1"),
            tool_use_id: id.clone(),
            cwd: "/work".into(),
            artifacts_dir: "/artifacts".into(),
            cancel: CancellationToken::new(),
            progress: riot_protocol::tool::ProgressSink::new(id, tx),
            file_state: Arc::new(MemFileState::new()),
            fs,
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

    fn input() -> serde_json::Value {
        serde_json::json!({
            "name": "Teller backend",
            "overview": "FastAPI 服务 + 四张表。",
            "plan": "## 步骤\n1. 建表\n2. 写路由"
        })
    }

    /// 规划模式必须能调出来，其它模式必须拒 —— 这是"常驻注册"的另一半：
    /// 清单里一直有它，靠这里挡住 agent 模式下的误调。
    #[test]
    fn 规划模式放行_其它模式拒绝并指路_switch_mode() {
        let r = CreatePlan.check_permissions(&input(), &perm(PermissionMode::Plan));
        assert!(r.is_allow(), "规划模式下要放行：{r:?}");

        for mode in [
            PermissionMode::Default,
            PermissionMode::AcceptEdits,
            PermissionMode::BypassPermissions,
            PermissionMode::Unattended,
        ] {
            let r = CreatePlan.check_permissions(&input(), &perm(mode));
            let PermissionResult::Deny { message, .. } = r else {
                panic!("{mode:?} 下该拒：{r:?}");
            };
            assert!(
                message.contains(SWITCH_MODE),
                "拒绝要指路 SwitchMode：{message}"
            );
        }
    }

    /// 走完整决策链：规划模式对写操作一律 Deny，这个工具是唯一的例外，
    /// 顺序或理由改了它就变成一个进得去出不来的模式。
    #[test]
    fn 规划模式的决策链放它过() {
        let r = riot_permissions::decide(
            &CreatePlan,
            &input(),
            &perm(PermissionMode::Plan),
            &riot_permissions::RuleSet::default(),
        );
        assert!(r.is_allow(), "Plan 模式下 CreatePlan 必须放行：{r:?}");
    }

    #[tokio::test]
    async fn 写进计划目录并且结果首行是文件路径() {
        // 真实文件系统的 write 自建父目录（见 riot_runtime::fs）；内存替身
        // 要求目录先在，这里先摆好。
        let fs = Arc::new(MemFs::new().with_dir("/work/.riot/plans"));
        let c = ctx(Arc::clone(&fs));
        let out = CreatePlan.call(input(), c.clone()).await;
        let ToolOutcome::Ok { model_content, .. } = out else {
            panic!("该成功：{out:?}");
        };
        let ToolResultContent::Text { text } = model_content else {
            panic!("该是文本结果");
        };
        let first = text.lines().next().unwrap_or_default();
        let rel = first
            .strip_prefix(PLAN_FILE_LINE_PREFIX)
            .unwrap_or_else(|| panic!("首行要是 `{PLAN_FILE_LINE_PREFIX}<路径>`：{first}"));
        assert!(rel.starts_with(PLAN_DIR), "{rel}");
        assert!(rel.ends_with(PLAN_EXT), "{rel}");

        let on_disk = fs.text(format!("/work/{rel}")).expect("文件要写下");
        assert!(on_disk.starts_with("# Teller backend\n"), "{on_disk}");
        assert!(on_disk.contains("FastAPI 服务"), "概述要在：{on_disk}");
        assert!(on_disk.contains("1. 建表"), "正文要在：{on_disk}");
        // 写完进缓存，模型可以直接 Edit。
        assert!(
            c.file_state
                .get(Path::new(&format!("/work/{rel}")))
                .is_some()
        );
    }

    /// 待办随计划落定：结果里报数并拦住"规划期间再 TodoWrite 一遍"；空措辞
    /// 点名第几项；`todos_of` 是内核和前端取待办的唯一入口。
    #[tokio::test]
    async fn 待办随计划落定_空项被拦_解析走同一入口() {
        let fs = Arc::new(MemFs::new().with_dir("/work/.riot/plans"));
        let c = ctx(Arc::clone(&fs));
        let mut with_todos = input();
        with_todos["todos"] = serde_json::json!([
            { "content": "建表" },
            { "content": "  写路由 " },
        ]);
        CreatePlan
            .validate_input(&with_todos, &c)
            .await
            .expect("两条正常待办该过");
        let out = CreatePlan.call(with_todos.clone(), c.clone()).await;
        let ToolOutcome::Ok { model_content, .. } = out else {
            panic!("该成功：{out:?}");
        };
        let text = format!("{model_content:?}");
        assert!(text.contains("2 todos"), "{text}");
        assert!(
            text.contains(&format!("do not call {TODO_WRITE} for them now")),
            "规划期间别再建一遍：{text}"
        );
        assert_eq!(
            todos_of(&with_todos),
            vec!["建表", "写路由"],
            "去空白、保顺序"
        );

        // 没给待办：结果不提，解析出来是空。
        let out = CreatePlan.call(input(), c.clone()).await;
        let ToolOutcome::Ok { model_content, .. } = out else {
            panic!("该成功：{out:?}");
        };
        assert!(!format!("{model_content:?}").contains("todos"));
        assert!(todos_of(&input()).is_empty());
        assert!(todos_of(&serde_json::json!({ "todos": "not-an-array" })).is_empty());

        let mut blank = input();
        blank["todos"] = serde_json::json!([{ "content": "建表" }, { "content": "  " }]);
        let err = CreatePlan
            .validate_input(&blank, &c)
            .await
            .expect_err("空措辞该拦");
        assert!(format!("{err:?}").contains("第 2 项"), "{err:?}");
    }

    /// 工具描述要教 todos 的用法，并且指路真实存在的 TodoWrite。
    #[test]
    fn 描述里讲待办怎么给() {
        let p = CreatePlan.prompt(&PromptContext {
            cwd: "/work".into(),
            platform: "macos".into(),
            sandboxed: false,
            sibling_tools: Vec::new(),
            today: "2026年9月".into(),
        });
        assert!(p.contains("TASK ORGANIZATION"), "{p}");
        assert!(p.contains("`todos`"), "{p}");
        assert!(
            p.contains(TODO_WRITE),
            "要说清构建时用哪个工具接着跟踪：{p}"
        );
    }

    #[test]
    fn 文件名带标题_slug_和调用_id_尾巴() {
        let rel = plan_rel_path("Teller backend: 柜员系统!", "toolu_01AbC9xY");
        assert_eq!(rel, ".riot/plans/teller-backend-柜员系统-abc9xy.plan.md");
        // 空标题、纯符号标题也得有个名字。
        assert!(plan_rel_path("!!!", "id").ends_with("/plan-id.plan.md"));
    }

    #[test]
    fn 正文自带一级标题时不重复() {
        let doc = compose_document("计划", None, "# 计划\n\n正文");
        assert_eq!(doc.matches("# 计划").count(), 1, "{doc}");
        assert!(doc.ends_with("正文\n"));
    }

    /// 工具写出来的路径必须被权限层认成计划文件 —— 否则规划模式下模型
    /// 改不了自己刚写的计划（Edit 被 Plan-Deny 拦下）。
    #[test]
    fn 写出来的路径被权限层认成计划文件() {
        for (name, id) in [
            ("Teller backend", "toolu_01AbC9xY"),
            ("柜员系统：后端", "call_x"),
            ("", ""),
        ] {
            let rel = plan_rel_path(name, id);
            assert!(
                is_plan_document(Path::new(&rel)),
                "{rel} 不被权限层认成计划文件"
            );
            assert!(
                is_plan_document(&Path::new("/work").join(&rel)),
                "拼上项目根也要认：{rel}"
            );
        }
    }

    /// 两段准则都得指路真实存在的工具，并且说清"用户有构建键，别问要不要开始"。
    #[test]
    fn 准则文本指路工具并且不让模型问要不要开始() {
        let rules = plan_mode_rules();
        assert!(rules.contains(CREATE_PLAN) && rules.contains(ASK_USER_QUESTION));
        assert!(rules.contains("Build button"));
        let iter = plan_iteration_rules();
        assert!(iter.contains(SWITCH_MODE) && iter.contains(EDIT));
        assert!(iter.contains("assume iteration"));
        // 内核把文件内容附在同一条消息里，这里就不能再指着可能已被压缩掉的
        // 旧结果说"路径在那儿"。
        assert!(iter.contains("<plan_file>"), "{iter}");
        assert!(!iter.contains("earlier CreatePlan result"), "{iter}");
    }
}
