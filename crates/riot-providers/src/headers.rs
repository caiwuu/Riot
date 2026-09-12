//! 请求头组装：系统头 + 默认 User-Agent + 用户额外头。
//!
//! `[约束]` 认证和内容类型不能被 extra 覆盖。否则用户配错一行
//! `Authorization: Bearer xxx` 就会把 auth.json 里的 key 顶掉，
//! 表现是 401，而排查会先去怀疑密钥存档。

/// 这一层发出去的默认 UA。网页抓取和更新检查已经用 `Riot/{version}`，
/// LLM 请求原先走 reqwest 默认 UA，OpenCode 一类网关明确不喜欢那种
/// 通用 SDK 标识。
pub fn default_user_agent() -> String {
    format!("Riot/{}", env!("CARGO_PKG_VERSION"))
}

fn reserved(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization" | "x-api-key" | "content-type" | "accept"
    )
}

fn is_user_agent(name: &str) -> bool {
    name.eq_ignore_ascii_case("user-agent")
}

/// 把用户配置的额外头接到系统头后面。
///
/// - 系统头里没有 User-Agent、extra 也没配时，补上 [`default_user_agent`]；
/// - extra 里的 User-Agent 覆盖默认值；
/// - extra 里的保留头（认证 / 内容类型 / Accept）被丢掉，不覆盖系统头。
pub fn merge_headers(
    base: Vec<(String, String)>,
    extra: &[(String, String)],
) -> Vec<(String, String)> {
    let extra_has_ua = extra.iter().any(|(k, _)| is_user_agent(k));
    let mut out = base;
    if !extra_has_ua && !out.iter().any(|(k, _)| is_user_agent(k)) {
        out.push(("user-agent".into(), default_user_agent()));
    }

    for (k, v) in extra {
        let key = k.trim();
        if key.is_empty() || reserved(key) {
            continue;
        }
        if is_user_agent(key) {
            out.retain(|(ek, _)| !is_user_agent(ek));
        }
        out.push((key.to_owned(), v.clone()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Vec<(String, String)> {
        vec![
            ("content-type".into(), "application/json".into()),
            ("accept".into(), "text/event-stream".into()),
            ("authorization".into(), "Bearer sk-secret".into()),
        ]
    }

    #[test]
    fn 没配就带默认_ua() {
        let h = merge_headers(base(), &[]);
        assert!(
            h.iter()
                .any(|(k, v)| is_user_agent(k) && v.starts_with("Riot/")),
            "{h:?}"
        );
        assert!(
            h.iter()
                .any(|(k, v)| k == "authorization" && v == "Bearer sk-secret")
        );
    }

    #[test]
    fn extra_的_ua_覆盖默认() {
        let h = merge_headers(base(), &[("User-Agent".into(), "my-agent/1.0".into())]);
        let uas: Vec<_> = h
            .iter()
            .filter(|(k, _)| is_user_agent(k))
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(uas, vec!["my-agent/1.0"], "{h:?}");
    }

    #[test]
    fn 不能覆盖认证和内容类型() {
        let h = merge_headers(
            base(),
            &[
                ("Authorization".into(), "Bearer hijacked".into()),
                ("x-api-key".into(), "stolen".into()),
                ("Content-Type".into(), "text/plain".into()),
                ("Accept".into(), "*/*".into()),
                ("x-opencode-session".into(), "ses_abc".into()),
            ],
        );
        assert_eq!(
            h.iter()
                .find(|(k, _)| k == "authorization")
                .map(|(_, v)| v.as_str()),
            Some("Bearer sk-secret")
        );
        assert!(
            !h.iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("x-api-key") && v == "stolen")
        );
        assert_eq!(
            h.iter()
                .find(|(k, _)| k == "content-type")
                .map(|(_, v)| v.as_str()),
            Some("application/json")
        );
        assert_eq!(
            h.iter()
                .find(|(k, _)| k == "accept")
                .map(|(_, v)| v.as_str()),
            Some("text/event-stream")
        );
        assert!(
            h.iter()
                .any(|(k, v)| k == "x-opencode-session" && v == "ses_abc"),
            "{h:?}"
        );
    }

    #[test]
    fn 空名字丢掉() {
        let h = merge_headers(base(), &[("  ".into(), "x".into())]);
        assert!(!h.iter().any(|(_, v)| v == "x"), "{h:?}");
    }
}
