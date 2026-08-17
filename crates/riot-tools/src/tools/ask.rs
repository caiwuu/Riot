//! `AskUserQuestion`：向用户提一个结构化的多选问题。
//!
//! # 为什么要有这个工具
//!
//! 没有它的时候，模型想让用户做决定只能把选项写进普通回复，然后**停下来**
//! 等下一条消息。那有两个问题：选项散在散文里要人自己找，而且模型经常
//! 不停 —— 它列完选项就自己挑一个继续干了。
//!
//! # 它怎么工作
//!
//! [`Tool::check_permissions`] 永远返回 `Ask`，宿主把它渲染成对话流里的
//! 一张选项卡。用户选完（或自己打字），宿主把答案写进工具输入的
//! [`CHOSEN_KEY`] 字段，然后才真正 `call` —— 也就是说这个工具的 `call`
//! 只做一件事：把用户的选择转成模型能读的一句话。
//!
//! `[约束]` 走权限的 ask 通道不是偷懒。那条通道已经解决了超时按拒绝处理、
//! 中断时补齐 tool_result 配对、子 agent 不许弹窗（`can_prompt_user` 为
//! false 时 ask 收敛成 deny）这几件事，每一个都是踩过坑才对的。

use async_trait::async_trait;
use riot_protocol::permission::{
    AskChoiceOption, DecisionReason, PermissionContext, PermissionResult,
};
use riot_protocol::tool::{
    PromptContext, ResultBudget, Tool, ToolContext, ToolOutcome, UiPayload, ValidationError,
};
use serde::Deserialize;

/// 宿主把用户的选择写进工具输入的哪个键。
///
/// 双下划线前缀且**不在 JSON Schema 里** —— 模型看不到它，也就不会自己
/// 填一个假答案然后跳过提问。
pub const CHOSEN_KEY: &str = "__chosen";

/// 用户点「其他」自己填写时，宿主把这段话编进 [`CHOSEN_KEY`]：
/// `__other:` + 原文。模型给的选项 id 不准用这个前缀（`validate_input` 会拦）。
///
/// 走已有的 `choice: Vec<String>` 而不是另开协议字段：提问的答案本来就是
/// 一串标识，自由文本是其中一种。前端的 `OTHER_PREFIX` 必须和这里对齐。
pub const OTHER_PREFIX: &str = "__other:";

/// 选项数量的上下限。
///
/// 下限 2：一个选项的"选择"不是选择，是通知；那种情况该直接说，不该弹窗。
/// 上限 6：再多就该让用户自己打字了 —— 一屏摆不下的按钮列表比一句话难读。
const MIN_OPTIONS: usize = 2;
const MAX_OPTIONS: usize = 6;

#[derive(Deserialize, schemars::JsonSchema)]
struct Input {
    /// 问题本身。一句话，别写成一段。
    question: String,
    /// 候选项，2 到 6 个。把你推荐的那个放第一个。
    options: Vec<InputOption>,
    /// 允许用户选多项。默认单选。
    #[serde(default)]
    allow_multiple: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct InputOption {
    /// 稳定标识，回传给你的就是这个。用短的英文小写词。
    id: String,
    /// 给用户看的文案。
    label: String,
}

pub struct AskUserQuestion;

/// 从原始输入里取宿主回灌的选择。
fn chosen(input: &serde_json::Value) -> Vec<String> {
    input
        .get(CHOSEN_KEY)
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// 把工具输入里的选项拆出来，供宿主渲染卡片。
///
/// 放在这里而不是宿主里：选项的字段名是这个工具的输入契约，宿主再抄一份
/// 就成了两个真相 —— 改了字段名只有一边跟着改，表现是卡片上一片空白。
pub fn preview_parts(input: &serde_json::Value) -> Option<(String, Vec<AskChoiceOption>, bool)> {
    let parsed: Input = serde_json::from_value(input.clone()).ok()?;
    Some((
        parsed.question,
        parsed
            .options
            .into_iter()
            .map(|o| AskChoiceOption { id: o.id, label: o.label })
            .collect(),
        parsed.allow_multiple,
    ))
}

/// 一条选择怎么写成给模型看的话。
///
/// 点了现成选项：`label（id）`。自己填写：`自己填写：原文`，前缀不外泄 ——
/// `__other:` 是宿主和工具之间的编码，模型不该看见实现细节。
fn describe_pick(id: &str, options: &[AskChoiceOption]) -> String {
    if let Some(text) = id.strip_prefix(OTHER_PREFIX) {
        let t = text.trim();
        return if t.is_empty() {
            "自己填写（空）".into()
        } else {
            format!("自己填写：{t}")
        };
    }
    options
        .iter()
        .find(|o| o.id == id)
        .map(|o| format!("{}（{id}）", o.label))
        .unwrap_or_else(|| id.to_owned())
}

#[async_trait]
impl Tool for AskUserQuestion {
    fn name(&self) -> &str {
        "AskUserQuestion"
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "把一个需要用户拍板的决定做成选择题问他。对话里会出现一张选项卡，\
         他点完（或自己打字）你才继续。\n\n\
         什么时候用：有多条都合理但取舍不同的路（存哪里、用哪个库、要不要兼容旧数据）；\
         你已经试过几种办法都不行，需要他决定下一步；任务的范围本身不清楚，\
         猜错就要重做一大片。\n\n\
         什么时候不用：答案能从代码或需求里读出来（那就去读，别问）；\
         只是想确认一个显而易见的默认做法（直接做）；\
         问题需要的是一段解释而不是一个选择（那就正常提问，用普通回复）。\n\n\
         一次只问一个决定。把你推荐的选项放第一个，并在 label 末尾写「（推荐）」。\
         选项要是**具体的做法**而不是「是/否」—— 「是」在用户看来往往不知道指的是哪个方案。\n\n\
         用户可能不选你给的选项，而是自己写一段。按他写的继续，不要再确认一遍。\n\n\
         不要把选项写进普通回复里让用户自己挑，那样你会在他回答之前就自己动手了。"
            .into()
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let q = input.get("question").and_then(|v| v.as_str()).unwrap_or("请用户决定");
        format!("提问：{q}")
    }

    /// 只是问一句话，不碰任何东西。
    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    /// 不能并行。两张选择卡叠在对话末尾，用户答完第一张才看得见第二张，
    /// 而模型这时已经拿着半份答案在走了。提示词里也写了「一次只问一个」。
    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        false
    }

    /// 用户答完就继续，结果只有一行，落盘毫无意义。
    fn result_budget(&self) -> ResultBudget {
        ResultBudget::Unlimited
    }

    /// 永远 Ask —— 这个工具的全部意义就是出那张卡。
    ///
    /// 注意这里返回 Ask 而不是 Passthrough：Passthrough 会被决策链按模式
    /// 收敛，在 `bypassPermissions` / `Unattended` 下变成自动放行，那时
    /// 用户根本看不到问题，而模型收到一个空答案。
    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        PermissionResult::Ask {
            message: "模型想让你做一个决定".into(),
            suggestions: Vec::new(),
            reason: DecisionReason::UserChoice { remembered: false },
        }
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        let parsed: Input = serde_json::from_value(input.clone())
            .map_err(|e| ValidationError::rejected(format!("参数不对：{e}")))?;

        if parsed.question.trim().is_empty() {
            return Err(ValidationError::rejected("question 是空的。"));
        }
        if parsed.options.len() < MIN_OPTIONS {
            return Err(ValidationError::rejected(format!(
                "至少要 {MIN_OPTIONS} 个选项 —— 只有一个的话那不是选择题，直接说就行。"
            )));
        }
        if parsed.options.len() > MAX_OPTIONS {
            return Err(ValidationError::rejected(format!(
                "最多 {MAX_OPTIONS} 个选项，现在有 {}。选项再多就该让用户自己打字了。",
                parsed.options.len()
            )));
        }
        let mut seen = std::collections::HashSet::new();
        for o in &parsed.options {
            if o.id.trim().is_empty() || o.label.trim().is_empty() {
                return Err(ValidationError::rejected("每个选项的 id 和 label 都不能为空。"));
            }
            if o.id.starts_with("__") {
                return Err(ValidationError::rejected(
                    "选项 id 不能以 __ 开头 —— 那是界面留给「用户自己填写」的前缀。",
                ));
            }
            if !seen.insert(o.id.trim()) {
                return Err(ValidationError::rejected(format!(
                    "选项 id「{}」重复了 —— 回传的答案会分不清是哪一个。",
                    o.id
                )));
            }
        }
        Ok(())
    }

    async fn call(&self, input: serde_json::Value, _ctx: ToolContext) -> ToolOutcome {
        let picked = chosen(&input);
        let Some((question, options, _)) = preview_parts(&input) else {
            return ToolOutcome::failed("参数解析不了，重新组织一下 question 和 options。");
        };

        // 走到 call 说明权限闸已经放行。没有选择只有一种可能：宿主放行了
        // 但没带上答案（旧版界面、或者被规则/模式直接 allow 掉了）。
        // 这时**不能**假装用户选了第一个 —— 那是替他做决定。
        if picked.is_empty() {
            return ToolOutcome::failed(
                "没有收到用户的选择。不要重复提问，也不要自己挑一个 —— \
                 用普通回复把这个决定和各选项的取舍讲清楚，然后停下来等他说。",
            );
        }

        // 回给模型的是 label，附上 id：label 才是用户实际读到并点下的那句话，
        // 只给 id 的话模型得自己回想它当初写的映射，容易记错。
        // 「其他」是用户自己写的，没有 id ↔ label 映射，前缀剥掉后原文送出。
        let lines: Vec<String> = picked.iter().map(|id| describe_pick(id, &options)).collect();
        let answer = lines.join("、");

        ToolOutcome::Ok {
            model_content: riot_protocol::message::ToolResultContent::text(format!(
                "用户选了：{answer}\n\n按这个继续，不要再确认一遍。"
            )),
            ui_payload: Some(UiPayload::Plain {
                text: format!("{question} → {answer}"),
            }),
            side_messages: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use riot_protocol::id::{SessionId, ToolUseId};
    use riot_protocol::permission::AskPreview;
    use tokio_util::sync::CancellationToken;

    use super::*;

    /// 这个工具不碰 fs / proc / 网络，上下文里的注入项全给占位实现就够。
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

    fn input(n: usize) -> serde_json::Value {
        let options: Vec<_> = (0..n)
            .map(|i| serde_json::json!({ "id": format!("o{i}"), "label": format!("选项 {i}") }))
            .collect();
        serde_json::json!({ "question": "存哪里？", "options": options })
    }

    #[tokio::test]
    async fn 选项数量越界要报得能改() {
        let t = AskUserQuestion;
        let ctx = ctx();

        let one = t.validate_input(&input(1), &ctx).await.expect_err("一个选项不该过");
        assert!(one.to_string().contains("直接说"), "{}", one.to_string());

        let many = t.validate_input(&input(9), &ctx).await.expect_err("九个不该过");
        assert!(many.to_string().contains("最多"), "{}", many.to_string());

        t.validate_input(&input(2), &ctx).await.expect("两个该过");
        t.validate_input(&input(6), &ctx).await.expect("六个该过");
    }

    #[tokio::test]
    async fn 重复的选项_id_要拦下() {
        // 不拦的话用户点了第二个，模型收到的 id 和第一个一样 —— 它会
        // 按错的那条路走，而且没有任何迹象表明出了问题。
        let t = AskUserQuestion;
        let bad = serde_json::json!({
            "question": "q",
            "options": [{ "id": "same", "label": "甲" }, { "id": "same", "label": "乙" }]
        });
        let e = t
            .validate_input(&bad, &ctx())
            .await
            .expect_err("重复 id 不该过");
        assert!(e.to_string().contains("重复"), "{}", e.to_string());
    }

    /// 这个工具在任何模式下都必须问用户。
    ///
    /// 返回 Passthrough 的话，bypass / Unattended 模式下决策链会自动放行 ——
    /// 用户压根看不到问题，模型拿一个空答案继续跑。
    #[test]
    fn 任何模式下都是_ask() {
        use riot_protocol::permission::{PermissionMode, PermissionModeState};
        let t = AskUserQuestion;
        for mode in [
            PermissionMode::Default,
            PermissionMode::AcceptEdits,
            PermissionMode::BypassPermissions,
            PermissionMode::Unattended,
        ] {
            let ctx = PermissionContext {
                mode: PermissionModeState(Some(mode)),
                ..Default::default()
            };
            assert!(
                matches!(t.check_permissions(&input(2), &ctx), PermissionResult::Ask { .. }),
                "{mode:?} 下没有问用户"
            );
        }
    }

    #[tokio::test]
    async fn 没收到选择时不替用户挑一个() {
        // 自己挑一个是最坏的失败方式:用户以为自己还没决定，而模型已经
        // 按某条路走下去了。
        let t = AskUserQuestion;
        let out = t.call(input(3), ctx()).await;
        let ToolOutcome::Failed { error_for_model, .. } = out else {
            panic!("没有选择时不该成功：{out:?}");
        };
        assert!(error_for_model.contains("不要自己挑"), "{error_for_model}");
    }

    #[tokio::test]
    async fn 选择回给模型时带上_label_和_id() {
        let t = AskUserQuestion;
        let mut i = input(3);
        i[CHOSEN_KEY] = serde_json::json!(["o1"]);
        let out = t.call(i, ctx()).await;
        let ToolOutcome::Ok { model_content, .. } = out else {
            panic!("该成功：{out:?}");
        };
        let text = format!("{model_content:?}");
        assert!(text.contains("选项 1"), "要带 label：{text}");
        assert!(text.contains("o1"), "要带 id：{text}");
    }

    #[tokio::test]
    async fn 多选时把所有选择都回传() {
        let t = AskUserQuestion;
        let mut i = input(4);
        i["allow_multiple"] = serde_json::json!(true);
        i[CHOSEN_KEY] = serde_json::json!(["o0", "o2"]);
        let out = t.call(i, ctx()).await;
        let ToolOutcome::Ok { model_content, .. } = out else {
            panic!("该成功：{out:?}");
        };
        let text = format!("{model_content:?}");
        assert!(text.contains("选项 0") && text.contains("选项 2"), "{text}");
    }

    #[tokio::test]
    async fn 自己填写的原文回给模型且不带前缀() {
        let t = AskUserQuestion;
        let mut i = input(2);
        i[CHOSEN_KEY] = serde_json::json!([format!("{OTHER_PREFIX}用 sqlite")]);
        let out = t.call(i, ctx()).await;
        let ToolOutcome::Ok { model_content, .. } = out else {
            panic!("该成功：{out:?}");
        };
        let text = format!("{model_content:?}");
        assert!(text.contains("自己填写：用 sqlite"), "{text}");
        assert!(!text.contains(OTHER_PREFIX), "编码前缀不该漏给模型：{text}");
    }

    #[tokio::test]
    async fn 多选可以混着自己填写() {
        let t = AskUserQuestion;
        let mut i = input(3);
        i["allow_multiple"] = serde_json::json!(true);
        i[CHOSEN_KEY] = serde_json::json!(["o0", format!("{OTHER_PREFIX}再加日志")]);
        let out = t.call(i, ctx()).await;
        let ToolOutcome::Ok { model_content, .. } = out else {
            panic!("该成功：{out:?}");
        };
        let text = format!("{model_content:?}");
        assert!(text.contains("选项 0") && text.contains("自己填写：再加日志"), "{text}");
    }

    #[tokio::test]
    async fn 选项_id_不能占用自己填写的前缀() {
        let t = AskUserQuestion;
        let bad = serde_json::json!({
            "question": "q",
            "options": [
                { "id": "__other:x", "label": "甲" },
                { "id": "ok", "label": "乙" }
            ]
        });
        let e = t.validate_input(&bad, &ctx()).await.expect_err("保留前缀不该过");
        assert!(e.to_string().contains("__"), "{}", e.to_string());
    }

    /// 卡片的字段来源必须和工具的输入契约同源。
    #[test]
    fn preview_能从输入拆出选项() {
        let (q, options, multi) = preview_parts(&input(2)).expect("该拆得出来");
        assert_eq!(q, "存哪里？");
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].id, "o0");
        assert!(!multi);

        // 顺带确认协议里的 Choice 变体拼得起来（改了字段名这里会编译失败）。
        let _ = AskPreview::Choice { question: q, options, allow_multiple: multi };
    }
}
