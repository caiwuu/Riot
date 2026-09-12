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

/// 头名必须是 RFC 7230 的 token：字母数字加 `!#$%&'*+-.^_`|~`。
fn is_token(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// 头值不能有控制字符（HTAB 除外）和 DEL —— 换行会被当成下一行头，其余
/// 会让 HTTP 库在组装请求时报错。非 ASCII 字节 `http` 库照收，这里也放过。
fn is_valid_value(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b == b'\t' || (b >= 0x20 && b != 0x7f))
}

/// 一条用户配置的头能不能发。`Err` 里是给日志 / 界面用的一句英文理由。
///
/// 放在这里而不是只在设置页校验：配置文件可以手改，而一条坏头会让
/// **每次**请求在组装阶段就失败，报的还是一句笼统的 transport 错。
pub fn validate_header(name: &str, value: &str) -> Result<(), &'static str> {
    if !is_token(name.trim()) {
        return Err("header name must be a token (letters, digits, !#$%&'*+-.^_`|~)");
    }
    if !is_valid_value(value) {
        return Err("header value must not contain control characters");
    }
    Ok(())
}

/// 把用户配置的额外头接到系统头后面。
///
/// - 系统头里没有 User-Agent、extra 也没配时，补上 [`default_user_agent`]；
/// - extra 里的保留头（认证 / 内容类型 / Accept）被丢掉，不覆盖系统头；
/// - 其余同名头（不分大小写）extra **覆盖** base，而不是各发一份 ——
///   `anthropic-version` 发两份服务端要么拒要么随机取一个；
/// - 名字不是 token、值带控制字符的丢掉并记 warn，不让一条坏头拖垮整个请求。
pub fn merge_headers(
    base: Vec<(String, String)>,
    extra: &[(String, String)],
) -> Vec<(String, String)> {
    let extra_has_ua = extra.iter().any(|(k, _)| is_user_agent(k.trim()));
    let mut out = base;
    if !extra_has_ua && !out.iter().any(|(k, _)| is_user_agent(k)) {
        out.push(("user-agent".into(), default_user_agent()));
    }

    for (k, v) in extra {
        let key = k.trim();
        if key.is_empty() || reserved(key) {
            continue;
        }
        if let Err(why) = validate_header(key, v) {
            // 只记名字不记值：值里可能是网关 token。
            tracing::warn!(header = key, why, "额外请求头不合法，跳过");
            continue;
        }
        out.retain(|(ek, _)| !ek.eq_ignore_ascii_case(key));
        out.push((key.to_owned(), v.clone()));
    }
    out
}

/// 只给 `Debug` 用：extra 头的名字列表。值可能是 Azure 的 `api-key`、网关
/// token 之类的凭据，和 `api_key` 一样不能进日志。
pub fn header_names(headers: &[(String, String)]) -> Vec<&str> {
    headers.iter().map(|(k, _)| k.as_str()).collect()
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

    /// 同名头 extra 覆盖 base，不是各发一份。
    #[test]
    fn 同名非保留头覆盖而不重复() {
        let mut b = base();
        b.push(("anthropic-version".into(), "2023-06-01".into()));
        let h = merge_headers(b, &[("Anthropic-Version".into(), "2024-01-01".into())]);
        let versions: Vec<_> = h
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("anthropic-version"))
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(versions, vec!["2024-01-01"], "{h:?}");
    }

    /// 一条坏头只丢它自己，别的照发；否则整个请求在组装阶段就挂。
    #[test]
    fn 非法头名或值跳过_其余照发() {
        let h = merge_headers(
            base(),
            &[
                ("bad name".into(), "x".into()),
                ("x-ok".into(), "line1\r\nInjected: y".into()),
                ("中文".into(), "x".into()),
                ("x-fine".into(), "value with spaces and 中文 is ok".into()),
            ],
        );
        assert!(!h.iter().any(|(k, _)| k == "bad name"), "{h:?}");
        assert!(!h.iter().any(|(k, _)| k == "x-ok"), "{h:?}");
        assert!(!h.iter().any(|(k, _)| k == "中文"), "{h:?}");
        assert!(
            h.iter().any(|(k, _)| k == "x-fine"),
            "合法的那条不能被连坐：{h:?}"
        );
        assert!(validate_header("x-opencode-session", "ses_1").is_ok());
        assert!(validate_header("x:y", "v").is_err());
        assert!(validate_header("x", "a\nb").is_err());
    }

    #[test]
    fn debug_只露名字() {
        let headers = vec![("api-key".to_owned(), "secret".to_owned())];
        assert_eq!(header_names(&headers), vec!["api-key"]);
    }
}
