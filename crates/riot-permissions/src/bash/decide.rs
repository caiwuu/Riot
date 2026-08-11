//! 把命令分析结果转成权限决策。
//!
//! 这是 `Bash` 工具 `check_permissions` 的实现,对应决策链的第 3 步。
//!
//! # 聚合规则
//!
//! `[约束]` 子命令逐条跑规则,**任一 deny → 整条 deny;任一 ask → ask;
//! 全部 allow 才 allow。**
//!
//! 方向不能反。`npm test && rm -rf /` 里 `npm test` 被规则允许,
//! 但整条命令必须问 —— 用户授权的是"跑测试",不是"跑测试顺便删库"。
//!
//! # 返回 Allow 不代表放行
//!
//! 这个函数的返回值会被 [`crate::chain::decide`] 的第 3 步接收,`Allow`
//! 之后还要过第 4 步的安全检查。所以这里判定"命令本身没问题"就够了,
//! 不需要重复做敏感路径检查。
//!
//! 见 ARCHITECTURE.md §9.3

use riot_protocol::permission::{
    DecisionReason, PermissionContext, PermissionResult, PermissionUpdate, RuleDecision,
    SafetyKind, UpdateScope,
};

use super::ast::{Analysis, ComplexReason, Complexity, SubCommand, analyze};
use super::readonly::is_read_only;
use crate::rules::{MatchMode, RuleSet};

pub fn decide(command: &str, ctx: &PermissionContext, rules: &RuleSet) -> PermissionResult {
    match analyze(command) {
        Analysis::TooComplex(c) => too_complex(&c),
        Analysis::Simple(subs) => decide_subs(&subs, ctx, rules),
    }
}

/// 不认识的结构一律 Ask,不做"看起来安全"的推断。
///
/// # 「看不懂」和「危险」是两回事
///
/// 这里产出的 Ask 分两档,区别只在 [`DecisionReason`],而那个字段决定了
/// 「全部放行」管不管用:
///
/// - **危险**（`SafetyCheck`,对放行免疫）:`eval`/`source` 执行运行时才
///   确定的内容、`LD_PRELOAD=` 之类劫持动态链接、重定向写向 shell 启动
///   脚本或密钥。这几样都精确、都指向"拿到超出写代码范围的能力"。
/// - **看不懂**（`Unverifiable`,放行可压过）:变量展开、命令替换、循环、
///   普通重定向、后台执行、解析失败。分析器只是不敢断言,不是发现了危险。
///
/// `[约束]` 第二档**必须**能被放行压过。它在正常开发里触发得极其频繁 ——
/// 模型干活必然要用 `$VAR`、`$(...)`、`for` 和管道。都按安全发现处理的话,
/// 「全部放行」会退化成一个 `echo $HOME` 都跑不过去的模式,而同一时刻
/// `rm -rf node_modules` 却是静默放行的。那个倒置真实发生过。
fn too_complex(c: &Complexity) -> PermissionResult {
    let message = format!("{}\n\n{}", explain(c.reason), c.detail);

    let reason = match c.reason {
        // 精确且危险:目标明确,不是启发式猜测
        ComplexReason::SensitiveRedirect(kind) => DecisionReason::SafetyCheck { safety: kind },
        ComplexReason::DynamicExecution | ComplexReason::DangerousAssignment => {
            DecisionReason::SafetyCheck {
                safety: SafetyKind::CommandInjection,
            }
        }
        // 其余都是"静态分析给不出结论"
        _ => DecisionReason::Unverifiable {
            what: c.detail.clone(),
        },
    };

    PermissionResult::Ask {
        message,
        // 不给"永久允许"建议。这类命令的危险在于内容不确定,
        // 记住一条规则等于给一个自己都说不清边界的授权。
        suggestions: Vec::new(),
        reason,
    }
}

/// 弹窗标题上那一句。
///
/// 短。完整命令就在下面的预览里,这里只点出"为什么拦你" —— 不解释
/// shell 语法,也不教育用户。
fn explain(r: ComplexReason) -> &'static str {
    match r {
        ComplexReason::CommandSubstitution => "含命令替换,执行内容运行时才确定。",
        ComplexReason::ProcessSubstitution => "含进程替换（`<(...)`）。",
        ComplexReason::Expansion => "含变量展开,结果取决于当前环境。",
        ComplexReason::Background => "会在后台运行（`&`）。",
        ComplexReason::Redirect => "会重定向写入文件。",
        ComplexReason::SensitiveRedirect(_) => "会写入 shell 启动脚本、密钥或凭证。",
        ComplexReason::ControlFlow => "含子 shell、循环或条件结构。",
        ComplexReason::DynamicExecution => "会执行运行时才确定的内容（`eval` / `source`）。",
        ComplexReason::DangerousAssignment => "设置了改变动态链接或命令查找的环境变量。",
        ComplexReason::ParseError => "无法解析这条命令。",
        ComplexReason::TooManyCommands => "子命令太多,无法逐条审查。",
        ComplexReason::TooLong => "命令过长。",
        ComplexReason::NestedWrappers => "包装嵌套过深。",
        ComplexReason::UnknownNode => "含无法识别的 shell 结构。",
    }
}

fn decide_subs(
    subs: &[SubCommand],
    ctx: &PermissionContext,
    rules: &RuleSet,
) -> PermissionResult {
    if subs.is_empty() {
        // 空命令或纯注释。没有副作用,但也没有意义。
        return PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Rule {
                source: riot_protocol::permission::RuleSource::Policy,
                pattern: "<空命令>".into(),
            },
        };
    }

    let mut pending_ask: Option<PermissionResult> = None;
    let mut all_allowed = true;

    for sub in subs {
        match sub_decision(sub, rules) {
            SubVerdict::Deny { pattern, source } => {
                return PermissionResult::Deny {
                    message: format!("`{}` 被规则禁止", sub.matchable),
                    reason: DecisionReason::Rule { source, pattern },
                };
            }
            SubVerdict::Ask { pattern, source } => {
                all_allowed = false;
                pending_ask.get_or_insert_with(|| PermissionResult::Ask {
                    message: format!("是否允许运行 `{}`？", sub.matchable),
                    suggestions: vec![allow_suggestion(&sub.matchable)],
                    reason: DecisionReason::Rule { source, pattern },
                });
            }
            SubVerdict::Allowed => {}
            SubVerdict::NoRule => all_allowed = false,
        }
    }

    // 任一 ask 就整条 ask —— 即使别的子命令都被允许了
    if let Some(ask) = pending_ask {
        return ask;
    }

    if all_allowed {
        return PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Rule {
                source: riot_protocol::permission::RuleSource::Project,
                pattern: subs
                    .iter()
                    .map(|s| s.matchable.as_str())
                    .collect::<Vec<_>>()
                    .join(" && "),
            },
        };
    }

    // 没有规则命中。只读命令直接放行,其余交给通用决策链。
    if is_read_only(subs) {
        return PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Mode {
                mode: ctx.mode.get(),
            },
        };
    }

    // 沙箱内的命令由 OS 挡住文件系统和网络,策略层可以放宽。
    //
    // `[约束]` 只放宽走到这里的 —— 也就是"没有任何规则命中、不是只读"
    // 的那部分。上面已经 return 的 Deny 和 Ask 不受影响:沙箱挡不住
    // "这条命令干的事和用户以为的不一样",而那正是 Ask 存在的理由。
    if ctx.sandboxed {
        return PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Sandbox,
        };
    }

    PermissionResult::Passthrough
}

enum SubVerdict {
    Allowed,
    Ask {
        pattern: String,
        source: riot_protocol::permission::RuleSource,
    },
    Deny {
        pattern: String,
        source: riot_protocol::permission::RuleSource,
    },
    NoRule,
}

fn sub_decision(sub: &SubCommand, rules: &RuleSet) -> SubVerdict {
    // 顺序就是严格性:deny 先看
    for (want, make) in [
        (
            RuleDecision::Deny,
            (|p, s| SubVerdict::Deny { pattern: p, source: s }) as fn(_, _) -> _,
        ),
        (RuleDecision::Ask, |p, s| SubVerdict::Ask {
            pattern: p,
            source: s,
        }),
    ] {
        if let Some(r) = rules.content_rule("Bash", &sub.matchable, want, MatchMode::AstVerified) {
            return make(r.pattern.clone().unwrap_or_default(), r.source);
        }
    }

    if rules
        .content_rule(
            "Bash",
            &sub.matchable,
            RuleDecision::Allow,
            MatchMode::AstVerified,
        )
        .is_some()
    {
        return SubVerdict::Allowed;
    }

    SubVerdict::NoRule
}

fn allow_suggestion(matchable: &str) -> PermissionUpdate {
    PermissionUpdate::AddRule {
        tool: "Bash".into(),
        pattern: Some(matchable.to_owned()),
        decision: RuleDecision::Allow,
        scope: UpdateScope::Session,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::ctx_with;
    use riot_protocol::permission::{PermissionMode, PermissionRule, RuleSource};
    use pretty_assertions::assert_eq;

    fn rules(pairs: Vec<(&str, RuleDecision)>) -> RuleSet {
        RuleSet::new(
            pairs
                .into_iter()
                .map(|(p, d)| PermissionRule {
                    tool: "Bash".into(),
                    pattern: Some(p.into()),
                    decision: d,
                    source: RuleSource::Project,
                })
                .collect(),
        )
    }

    fn verdict(cmd: &str, rs: &RuleSet) -> &'static str {
        match decide(cmd, &ctx_with(PermissionMode::Default), rs) {
            PermissionResult::Allow { .. } => "allow",
            PermissionResult::Ask { .. } => "ask",
            PermissionResult::Deny { .. } => "deny",
            PermissionResult::Passthrough => "passthrough",
        }
    }

    #[test]
    fn 只读命令直接放行() {
        assert_eq!(verdict("ls -la", &RuleSet::default()), "allow");
        assert_eq!(verdict("git status", &RuleSet::default()), "allow");
    }

    #[test]
    fn 写命令交给通用决策链() {
        assert_eq!(verdict("rm -rf /tmp/x", &RuleSet::default()), "passthrough");
    }

    #[test]
    fn 规则允许的命令放行() {
        let rs = rules(vec![("npm run *", RuleDecision::Allow)]);
        assert_eq!(verdict("npm run build", &rs), "allow");
    }

    #[test]
    fn 任一子命令被禁则整条被禁() {
        // 用户授权的是"跑测试",不是"跑测试顺便删库"
        let rs = rules(vec![
            ("npm test", RuleDecision::Allow),
            ("rm *", RuleDecision::Deny),
        ]);
        assert_eq!(verdict("npm test && rm -rf /", &rs), "deny");
    }

    #[test]
    fn 任一子命令要问则整条要问() {
        let rs = rules(vec![
            ("npm test", RuleDecision::Allow),
            ("curl *", RuleDecision::Ask),
        ]);
        assert_eq!(verdict("npm test && curl example.com", &rs), "ask");
    }

    #[test]
    fn 部分子命令有规则时不算全允许() {
        // `npm test` 被允许,`rm` 没有规则 —— 不能因为"有一条 allow"就放行
        let rs = rules(vec![("npm test", RuleDecision::Allow)]);
        assert_eq!(
            verdict("npm test && rm -rf /", &rs),
            "passthrough",
            "没规则的子命令要交给通用链,不能跟着 allow 一起放行"
        );
    }

    #[test]
    fn 全部子命令都被允许才放行() {
        let rs = rules(vec![
            ("npm test", RuleDecision::Allow),
            ("echo *", RuleDecision::Allow),
        ]);
        assert_eq!(verdict("npm test && echo done", &rs), "allow");
    }

    #[test]
    fn 只读加非只读的组合不放行() {
        assert_eq!(
            verdict("ls && rm -rf /tmp/x", &RuleSet::default()),
            "passthrough"
        );
    }

    #[test]
    fn 复杂命令一律询问() {
        for cmd in [
            "rm -rf $(cat target)",
            "npm test &",
            "eval \"$CMD\"",
            "LD_PRELOAD=/evil.so ls",
            "ls > /etc/passwd",
        ] {
            assert_eq!(verdict(cmd, &RuleSet::default()), "ask", "{cmd}");
        }
    }

    #[test]
    fn 复杂命令即使有_allow_规则也要问() {
        // 规则匹配的是静态文本,而这类命令的内容是运行时决定的。
        // `Bash(ls *)` 不该让 `ls $(rm -rf /)` 通过。
        let rs = rules(vec![("ls *", RuleDecision::Allow)]);
        assert_eq!(verdict("ls $(rm -rf /)", &rs), "ask");
    }

    #[test]
    fn 复杂命令不给永久允许建议() {
        // 记住一条内容不确定的规则,等于给一个自己都说不清边界的授权
        match decide(
            "rm -rf $(cat target)",
            &ctx_with(PermissionMode::Default),
            &RuleSet::default(),
        ) {
            PermissionResult::Ask { suggestions, .. } => {
                assert!(suggestions.is_empty());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn 包装剥离后能匹配规则() {
        // 否则用户得为 `npm test`、`timeout 30 npm test`、
        // `nice npm test` 各写一遍规则
        let rs = rules(vec![("npm test", RuleDecision::Allow)]);
        assert_eq!(verdict("timeout 30 npm test", &rs), "allow");
        assert_eq!(verdict("nice -n 10 npm test", &rs), "allow");
    }

    #[test]
    fn 剥离不会让_sudo_匹配到普通规则() {
        let rs = rules(vec![("rm /tmp/*", RuleDecision::Allow)]);
        assert_eq!(
            verdict("sudo rm /tmp/x", &rs),
            "passthrough",
            "sudo 改变的是权限,不能剥掉"
        );
    }

    #[test]
    fn 引号里的分隔符不会绕过规则() {
        // `Bash(git commit *)` 应该匹配整条,而不是被 `&&` 切成两半
        let rs = rules(vec![("git commit *", RuleDecision::Allow)]);
        assert_eq!(verdict("git commit -m 'fix && cleanup'", &rs), "allow");
    }

    #[test]
    fn deny_规则压过只读判定() {
        // 只读不等于用户想让它跑
        let rs = rules(vec![("ls *", RuleDecision::Deny)]);
        assert_eq!(verdict("ls -la", &rs), "deny");
    }
}
