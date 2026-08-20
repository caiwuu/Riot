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

/// 对某个 URL 做权限判定。
///
/// `tool` 是发起访问的工具名 —— 规则匹配按工具区分，但**内容键是共享的**
/// （http(s) 是 `domain:<host>`，本地文件是 `file:<目录>`），所以用户为
/// WebFetch 允许过的域名，浏览器这边同样命中。
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
                message: ask_message(tool, u),
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
    // 本地文件不在白名单里：空 host 不该碰巧命中任何条目。
    if u.scheme() != "file" && preapproved::is_preapproved(host, u.path()) {
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
        message: ask_message(tool, u),
        suggestions: suggestions(tool, &content),
        reason: DecisionReason::Consent { what: content },
    }
}

fn ask_message(tool: &str, u: &Url) -> String {
    let label = permission_label(u);
    if tool == "WebFetch" {
        format!("是否允许抓取 {label}？")
    } else if u.scheme() == "file" {
        format!("是否允许在浏览器里打开本地文件 {label}？")
    } else {
        format!("是否允许在浏览器里打开 {label}？")
    }
}

/// 弹窗上给人看的目标。文件显示完整路径，网站显示主机名。
fn permission_label(u: &Url) -> String {
    weburl::local_file_path(u)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| u.host_str().unwrap_or_default().to_owned())
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

    /// 本地文件用例的平台差异。判定逻辑两个平台一样，只有字面量得换：
    /// Windows 的 file URL 必须带盘符，否则 `Url::to_file_path` 不认，
    /// 这个 URL 会在 [`weburl::normalize_for_browser`] 那层就被判畸形。
    ///
    /// `DISPLAY` 是弹窗上给人看的形态 —— [`permission_label`] 走的是
    /// `PathBuf::display`，Windows 上是反斜杠。
    #[cfg(windows)]
    mod local {
        pub const FILE_URL: &str = "file:///C:/Users/me/proj/wechat.html";
        pub const SAME_DIR_URL: &str = "file:///C:/Users/me/proj/other.html";
        pub const DISPLAY: &str = r"C:\Users\me\proj\wechat.html";
        /// 内容键拼的也是 `Path::display`，不是 URL 形式。
        pub const DIR_KEY: &str = r"file:C:\Users\me\proj";
    }

    #[cfg(not(windows))]
    mod local {
        pub const FILE_URL: &str = "file:///Users/me/proj/wechat.html";
        pub const SAME_DIR_URL: &str = "file:///Users/me/proj/other.html";
        pub const DISPLAY: &str = "/Users/me/proj/wechat.html";
        pub const DIR_KEY: &str = "file:/Users/me/proj";
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

    #[test]
    fn 本地文件要问且键是目录粒度() {
        let u = url(local::FILE_URL);
        let r = decide_for_domain("BrowserNavigate", &u, &ctx(vec![]));
        let PermissionResult::Ask {
            message,
            suggestions,
            reason,
        } = r
        else {
            panic!("本地文件必须确认，实际：{r:?}");
        };
        assert!(
            message.contains(local::DISPLAY),
            "要让人看见完整路径：{message}"
        );
        assert!(
            matches!(reason, DecisionReason::Consent { .. }),
            "理由必须是 Consent，否则「全部放行」失效：{reason:?}"
        );
        let riot_protocol::permission::PermissionUpdate::AddRule { pattern, .. } = &suggestions[0]
        else {
            panic!("建议应当是 AddRule：{suggestions:?}");
        };
        assert_eq!(pattern.as_deref(), Some(local::DIR_KEY));
    }

    #[test]
    fn 同一目录的本地文件共享允许规则() {
        let allow = PermissionRule {
            tool: "BrowserNavigate".into(),
            pattern: Some(local::DIR_KEY.into()),
            decision: RuleDecision::Allow,
            source: RuleSource::Session,
        };
        let r = decide_for_domain(
            "BrowserNavigate",
            &url(local::SAME_DIR_URL),
            &ctx(vec![allow]),
        );
        assert!(matches!(r, PermissionResult::Allow { .. }), "{r:?}");
    }
}
