//! 按域名征求同意。WebFetch 和内置浏览器共用。
//!
//! `[约束]` 两者必须走同一份判定。
//!
//! 用户点"总是允许 example.com"表达的是"我信任这个站"，那个判断和用哪个
//! 工具去访问无关。各写一份的后果有两层:同一个域名被问两遍（用户会以为
//! 按钮没生效），以及两边的 `DecisionReason` 迟早分叉 —— 而那个字段决定
//! 「全部放行」管不管用，分叉之后就是"抓取放行了、浏览没放行"这种没法
//! 解释的状态。

use riot_permissions::{MatchMode, RuleSet};
use riot_protocol::permission::{
    DecisionReason, PermissionContext, PermissionResult, PermissionUpdate, RuleDecision,
    UpdateScope,
};
use url::Url;

use super::preapproved;
use super::url as weburl;

/// 对某个 URL 的域名做权限判定。
///
/// `tool` 是发起访问的工具名 —— 规则匹配按工具区分，但**内容键是共享的**
/// (`domain:<host>`)，所以用户为 WebFetch 允许过的域名，浏览器这边同样
/// 命中。
pub fn decide_for_domain(tool: &str, u: &Url, ctx: &PermissionContext) -> PermissionResult {
    let content = weburl::permission_content(u);
    let host = u.host_str().unwrap_or_default();
    let rules = RuleSet::new(ctx.rules.clone());

    for want in [RuleDecision::Deny, RuleDecision::Ask, RuleDecision::Allow] {
        let Some(r) = rules.content_rule(tool, &content, want, MatchMode::Raw) else {
            continue;
        };
        let reason = DecisionReason::Rule {
            source: r.source,
            pattern: r.pattern.clone().unwrap_or_default(),
        };
        return match want {
            RuleDecision::Deny => PermissionResult::Deny {
                message: format!("已配置规则禁止访问 {content}。"),
                reason,
            },
            RuleDecision::Ask => PermissionResult::Ask {
                message: ask_message(tool, host),
                suggestions: suggestions(tool, &content),
                reason,
            },
            RuleDecision::Allow => PermissionResult::Allow {
                updated_input: None,
                reason,
            },
        };
    }

    // 官方文档站免确认。不这么做的话，用户查第三个文档时就会直接开
    // 「全部放行」—— 那比这份白名单危险得多。
    if preapproved::is_preapproved(host, u.path()) {
        return PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Preapproved { what: content },
        };
    }

    // 没有规则命中，也不在白名单 —— 问一次。
    //
    // `[约束]` 理由必须是 `Consent` 而不是 `Rule`。这里根本没有规则，
    // 冒充成规则会让决策链以为"用户明确要求问这个域名"，于是「全部放行」
    // 对这个工具永久失效。见 chain::decide 第 3 步 —— 这条是真实踩过的。
    PermissionResult::Ask {
        message: ask_message(tool, host),
        suggestions: suggestions(tool, &content),
        reason: DecisionReason::Consent { what: content },
    }
}

fn ask_message(tool: &str, host: &str) -> String {
    if tool == "WebFetch" {
        format!("是否允许抓取 {host}？")
    } else {
        format!("是否允许在浏览器里打开 {host}？")
    }
}

/// "总是允许"要记成什么规则。
fn suggestions(tool: &str, content: &str) -> Vec<PermissionUpdate> {
    vec![PermissionUpdate::AddRule {
        tool: tool.to_owned(),
        pattern: Some(content.to_owned()),
        decision: RuleDecision::Allow,
        scope: UpdateScope::Session,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use riot_protocol::permission::{PermissionRule, RuleSource};

    fn ctx(rules: Vec<PermissionRule>) -> PermissionContext {
        PermissionContext {
            rules,
            can_prompt_user: true,
            ..Default::default()
        }
    }

    fn url(s: &str) -> Url {
        Url::parse(s).expect("测试 URL")
    }

    #[test]
    fn 陌生域名要问_且理由是_consent() {
        // `[约束]` 理由不能写成 Rule。写错的话「全部放行」对这个工具
        // 永久失效 —— 用户开了放行还在被问，而且找不到是哪条规则。
        let r = decide_for_domain("WebFetch", &url("https://unknown.test/x"), &ctx(vec![]));
        assert!(
            matches!(r, PermissionResult::Ask { reason: DecisionReason::Consent { .. }, .. }),
            "{r:?}"
        );
    }

    #[test]
    fn 为一个工具允许的域名对另一个也生效() {
        // 用户信任的是站点，不是工具。分开记的话同一个域名会被问两遍。
        let allow = PermissionRule {
            tool: "BrowserNavigate".into(),
            pattern: Some("domain:example.com".into()),
            decision: RuleDecision::Allow,
            source: RuleSource::Session,
        };
        let r = decide_for_domain(
            "BrowserNavigate",
            &url("https://example.com/a"),
            &ctx(vec![allow]),
        );
        assert!(matches!(r, PermissionResult::Allow { .. }), "{r:?}");
    }

    #[test]
    fn 内容键和_webfetch_一致() {
        // 两边必须用同一个键，否则"允许过"这件事传递不过去。
        assert_eq!(
            weburl::permission_content(&url("https://example.com/a?b=1")),
            "domain:example.com"
        );
    }

    #[test]
    fn deny_规则优先于白名单() {
        // 白名单是便利，用户写的禁令是意图。顺序反了就是"我明明禁了它"。
        let deny = PermissionRule {
            tool: "WebFetch".into(),
            pattern: Some("domain:docs.rs".into()),
            decision: RuleDecision::Deny,
            source: RuleSource::User,
        };
        let r = decide_for_domain("WebFetch", &url("https://docs.rs/x"), &ctx(vec![deny]));
        assert!(matches!(r, PermissionResult::Deny { .. }), "{r:?}");
    }
}
