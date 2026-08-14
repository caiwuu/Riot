//! URL 准入与重定向策略。
//!
//! 这个文件是联网工具的安全边界，里面每一条规则都对应一个真实攻击面。
//! 它是**纯函数**，不发请求 —— 所以每条规则都能单独测，而且能测反例。
//!
//! # 三层拦截
//!
//! 1. [`normalize`] —— 抓取发出前的静态检查：协议、长度、userinfo、字面量私有 IP。
//!    浏览器走 [`normalize_for_browser`]，允许 localhost / 内网 / 本地 `file://`，
//!    且不升级 http。
//! 2. DNS 解析后的私有网段检查 —— 在 `riot-runtime` 里，因为解析结果
//!    只有那一层看得见。
//! 3. [`is_permitted_redirect`] —— 每一跳都要重新过。
//!
//! 少了任何一层都有绕过路径：只做第 1 层，`http://evil.com` 解析到
//! `127.0.0.1` 就穿了；只做第 2 层，`http://[::1]/` 这种字面量地址虽然会被
//! 拦但错误信息会很难懂；只做前两层，一个可信域名上的开放重定向就能把请求
//! 带去任何地方。

use std::path::{Path, PathBuf};

use url::{Host, Url};

// 内网判定在协议层。它同时被 `riot-runtime` 的 DNS 解析器用到，
// 两份实现迟早会漂移，而漂移的那一侧不会有任何报错。
pub use riot_protocol::web::is_private_ip;

/// URL 长度上限。
///
/// 防的是把数据编码进 URL 往外带。Claude Code 的注释里提到他们评估后放宽到
/// 2000（签名 URL 可以很长），这里沿用同一个值 —— 域名级的权限确认才是主
/// 防线，长度只是提高一点成本。
pub const MAX_URL_LENGTH: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UrlReject {
    #[error("URL 解析失败")]
    Malformed,

    #[error("URL 超过 {MAX_URL_LENGTH} 字符上限")]
    TooLong,

    #[error("只支持 http 与 https，不支持 {scheme}")]
    BadScheme { scheme: String },

    /// `file://nas/share` 这种网上邻居。本地预览不是这条路。
    #[error("`file://` 只支持本机文件，不支持主机 `{host}`")]
    RemoteFile { host: String },

    /// `https://user:pass@host/` 这种形式。
    ///
    /// 拦它有两个理由：凭证会跟着请求泄漏出去；而且 `evil.com` 可以伪装成
    /// `https://www.bank.com@evil.com/` 骗过只看前缀的人眼。
    #[error("URL 里不能带用户名或密码")]
    HasCredentials,

    #[error("取不到主机名")]
    NoHost,

    /// 单段主机名（`localhost`、`redis`、`metadata`）。
    #[error("`{host}` 不是公网可解析的主机名")]
    NotPublicHost { host: String },

    #[error("`{host}` 指向内网地址，已拒绝")]
    PrivateAddress { host: String },
}

/// 校验并规范化一个 **抓取** URL。
///
/// 成功时返回**已把 http 升级成 https 的** URL。升级而不是拒绝，是因为模型
/// 从网页里抄下来的链接经常还是 http，直接拒绝会让它反复重试同一个地址。
///
/// 同时拒绝 localhost 和字面量内网 IP —— 抓取工具默认不能打内网。
/// 内置浏览器走 [`normalize_for_browser`]，那条路允许本地地址。
pub fn normalize(raw: &str) -> Result<Url, UrlReject> {
    let mut u = parse_http_url(raw)?;
    if u.scheme() == "http" {
        // set_scheme 只在 URL 有主机时才成功，而没主机的情况 parse 已经拦掉。
        let _ = u.set_scheme("https");
    }
    check_host(&u.host().ok_or(UrlReject::NoHost)?)?;
    Ok(u)
}

/// 校验浏览器要打开的 URL。
///
/// 和 [`normalize`] 共用 http(s) 的长度 / 凭证检查，但：
/// - **不**把 http 升级成 https（本地开发服务器几乎都没有证书）
/// - **不**拒绝 localhost / 内网 IP（内置浏览器就是用来看自己刚改完的前端）
/// - **允许**本地 `file://` 和本机绝对路径（预览静态 HTML 是常见用法）
///
/// 远程 `file://host/share` 仍然拒绝 —— 那是网上邻居，不是本地预览。
/// 内网访问仍要过同意弹窗。WebFetch 继续走 [`normalize`]。
pub fn normalize_for_browser(raw: &str) -> Result<Url, UrlReject> {
    if raw.len() > MAX_URL_LENGTH {
        return Err(UrlReject::TooLong);
    }
    if let Some(u) = try_local_file(raw)? {
        return Ok(u);
    }
    parse_http_url(raw)
}

/// 是本地文件地址的话返回规范化后的 `file://` URL。
///
/// 不是文件地址返回 `Ok(None)`，交给调用方按 http(s) 继续解析。
/// 看起来像文件但非法（网上邻居、带凭证）返回 `Err`，不再回落到 http。
fn try_local_file(raw: &str) -> Result<Option<Url>, UrlReject> {
    if is_local_abs_path(raw) {
        return Url::from_file_path(raw)
            .map(Some)
            .map_err(|()| UrlReject::Malformed);
    }

    let Ok(u) = Url::parse(raw) else {
        return Ok(None);
    };
    if u.scheme() != "file" {
        return Ok(None);
    }
    Ok(Some(check_file_url(u)?))
}

/// 本机绝对路径，且看起来不像 URL。
///
/// `://` 挡的是已经带协议的地址；`//` 挡的是协议相对 URL（`//example.com`），
/// 那种在 Unix 上会被 `Path::is_absolute` 误判成本地路径。
fn is_local_abs_path(raw: &str) -> bool {
    !raw.contains("://") && !raw.starts_with("//") && Path::new(raw).is_absolute()
}

fn check_file_url(u: Url) -> Result<Url, UrlReject> {
    if !u.username().is_empty() || u.password().is_some() {
        return Err(UrlReject::HasCredentials);
    }
    match u.host_str() {
        None | Some("") | Some("localhost") => {}
        Some(host) => {
            return Err(UrlReject::RemoteFile {
                host: host.to_owned(),
            });
        }
    }
    if u.to_file_path().is_err() {
        return Err(UrlReject::Malformed);
    }
    Ok(u)
}

/// `file://` URL 对应的本机路径。其它协议返回 `None`。
pub fn local_file_path(u: &Url) -> Option<PathBuf> {
    if u.scheme() != "file" {
        return None;
    }
    u.to_file_path().ok()
}

/// 协议、长度、凭证、主机存在性。两边共用；主机是不是公网只在 [`normalize`] 里查。
fn parse_http_url(raw: &str) -> Result<Url, UrlReject> {
    if raw.len() > MAX_URL_LENGTH {
        return Err(UrlReject::TooLong);
    }

    let u = Url::parse(raw).map_err(|_| UrlReject::Malformed)?;

    match u.scheme() {
        "http" | "https" => {}
        other => {
            return Err(UrlReject::BadScheme {
                scheme: other.to_owned(),
            });
        }
    }

    if !u.username().is_empty() || u.password().is_some() {
        return Err(UrlReject::HasCredentials);
    }

    if u.host().is_none() {
        return Err(UrlReject::NoHost);
    }

    Ok(u)
}

/// 主机名层面的检查。字面量 IP 直接判段，域名要求至少有一个点。
fn check_host(host: &Host<&str>) -> Result<(), UrlReject> {
    match host {
        Host::Ipv4(ip) => {
            if is_private_ip(std::net::IpAddr::V4(*ip)) {
                return Err(UrlReject::PrivateAddress {
                    host: ip.to_string(),
                });
            }
        }
        Host::Ipv6(ip) => {
            if is_private_ip(std::net::IpAddr::V6(*ip)) {
                return Err(UrlReject::PrivateAddress {
                    host: ip.to_string(),
                });
            }
        }
        Host::Domain(d) => {
            let d = d.trim_end_matches('.');
            // 单段名字（localhost、metadata、内网短名）在公网上不可解析，
            // 出现在这里只可能是想打内网。
            //
            // 这条会误伤真正的内网使用场景，那是**故意的** —— 想访问内网
            // 应该走 MCP 工具，那条路上有明确的授权，而不是让通用抓取工具
            // 默认能打进去。
            if !d.contains('.') || d.is_empty() {
                return Err(UrlReject::NotPublicHost {
                    host: (*d).to_owned(),
                });
            }
        }
    }
    Ok(())
}

/// 这一跳重定向能不能自动跟。
///
/// 只允许**同源**跳转，外加 `www.` 前缀的增减。跨站跳转一律返回 `false`，
/// 由调用方把跳转目标交回模型，让它重新发起一次带权限确认的请求。
///
/// `[约束]` 放宽这个函数等于放宽域名白名单。一个可信域名上的开放重定向
/// （`https://docs.trusted.com/r?to=http://evil.com`）如果能被自动跟随，
/// 用户对 `docs.trusted.com` 的授权就变成了对整个互联网的授权。
pub fn is_permitted_redirect(original: &Url, redirect: &Url) -> bool {
    if original.scheme() != redirect.scheme() {
        return false;
    }
    // 比较端口用 port_or_known_default，否则 `https://a.com` 和
    // `https://a.com:443` 会被判成不同源。
    if original.port_or_known_default() != redirect.port_or_known_default() {
        return false;
    }
    if !redirect.username().is_empty() || redirect.password().is_some() {
        return false;
    }
    match (original.host_str(), redirect.host_str()) {
        (Some(a), Some(b)) => strip_www(a) == strip_www(b),
        _ => false,
    }
}

fn strip_www(host: &str) -> &str {
    host.strip_prefix("www.").unwrap_or(host)
}

/// 取出用于权限规则匹配的内容键。
///
/// http(s) 是域名粒度（`domain:docs.rs`）：用户点"总是允许"意味着"信任
/// 这个站"，而不是"以后随便抓什么都行"。
///
/// 本地文件是**目录**粒度（`file:/Users/me/proj`）：同一页的 css / js /
/// 其它 html 都在旁边，问一次就该覆盖整个目录。用整条 `file:` 的话，
/// 一次"总是允许"等于打开任意本地文件。
pub fn permission_content(u: &Url) -> String {
    if let Some(path) = local_file_path(u) {
        let dir = path.parent().unwrap_or(&path);
        return format!("file:{}", dir.display());
    }
    format!("domain:{}", u.host_str().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn u(s: &str) -> Url {
        Url::parse(s).expect("测试 URL 应当合法")
    }

    #[test]
    fn http_升级成_https() {
        // 模型从网页里抄来的链接经常是 http，拒绝会让它反复重试同一个地址
        let n = normalize("http://example.com/a").expect("应当通过");
        assert_eq!(n.as_str(), "https://example.com/a");
    }

    #[test]
    fn 拒绝非_http_协议() {
        // file:// 能读本地文件，data: 能绕过一切网络检查
        for bad in [
            "file:///etc/passwd",
            "ftp://example.com/x",
            "data:text/html,hi",
        ] {
            assert!(
                matches!(normalize(bad), Err(UrlReject::BadScheme { .. })),
                "{bad} 应当被拒"
            );
        }
    }

    #[test]
    fn 拒绝带凭证的_url() {
        assert_eq!(
            normalize("https://user:pw@example.com/"),
            Err(UrlReject::HasCredentials)
        );
        // 只有用户名没密码同样要拦 —— https://www.bank.com@evil.com/
        // 这种伪装形式里根本没有密码段
        assert_eq!(
            normalize("https://www.bank.com@evil.com/"),
            Err(UrlReject::HasCredentials)
        );
    }

    #[test]
    fn 拒绝单段主机名() {
        for bad in ["https://localhost/", "https://metadata/", "https://redis/"] {
            assert!(
                matches!(normalize(bad), Err(UrlReject::NotPublicHost { .. })),
                "{bad} 应当被拒"
            );
        }
        // 结尾的点不该让它蒙混过关
        assert!(matches!(
            normalize("https://localhost./"),
            Err(UrlReject::NotPublicHost { .. })
        ));
    }

    #[test]
    fn 拒绝字面量内网地址() {
        // 169.254.169.254 是云厂商元数据服务，读到就等于拿到实例凭证
        for bad in [
            "https://127.0.0.1/",
            "https://10.0.0.5/",
            "https://192.168.1.1/",
            "https://172.16.0.1/",
            "https://169.254.169.254/latest/meta-data/",
            "https://0.0.0.0/",
            "https://100.64.0.1/",
            "https://[::1]/",
            "https://[fe80::1]/",
            "https://[fc00::1]/",
        ] {
            assert!(
                matches!(normalize(bad), Err(UrlReject::PrivateAddress { .. })),
                "{bad} 应当被拒"
            );
        }
    }

    #[test]
    fn ipv4_映射的_ipv6_按内含地址判定() {
        // 不展开映射的话 ::ffff:169.254.169.254 是一条完整的绕过路径
        assert!(matches!(
            normalize("https://[::ffff:169.254.169.254]/"),
            Err(UrlReject::PrivateAddress { .. })
        ));
        assert!(matches!(
            normalize("https://[::ffff:127.0.0.1]/"),
            Err(UrlReject::PrivateAddress { .. })
        ));
    }

    #[test]
    fn 放行正常的公网地址() {
        for ok in [
            "https://docs.rs/tokio",
            "https://example.com:8443/x?q=1#frag",
            "https://8.8.8.8/",
        ] {
            assert!(normalize(ok).is_ok(), "{ok} 应当通过");
        }
    }

    #[test]
    fn 超长_url_被拒() {
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_LENGTH));
        assert_eq!(normalize(&long), Err(UrlReject::TooLong));
    }

    #[test]
    fn 同源重定向可以跟() {
        assert!(is_permitted_redirect(
            &u("https://a.com/x"),
            &u("https://a.com/y?z=1")
        ));
        // www 的增减是同一个站点最常见的规范化跳转
        assert!(is_permitted_redirect(
            &u("https://a.com/x"),
            &u("https://www.a.com/x")
        ));
        assert!(is_permitted_redirect(
            &u("https://www.a.com/x"),
            &u("https://a.com/x")
        ));
    }

    #[test]
    fn 跨站重定向不能跟() {
        // 这条挂了，等于用户对任何一个域名的授权都变成了对全网的授权
        assert!(!is_permitted_redirect(
            &u("https://a.com/x"),
            &u("https://evil.com/x")
        ));
        // 子域名也算跨站 —— 一个站点上任意用户内容子域都能拿来中转
        assert!(!is_permitted_redirect(
            &u("https://a.com/x"),
            &u("https://pages.a.com/x")
        ));
        // 后缀相同但不是同一个域
        assert!(!is_permitted_redirect(
            &u("https://a.com/x"),
            &u("https://evil-a.com/x")
        ));
        // 降级到 http 不能跟
        assert!(!is_permitted_redirect(
            &u("https://a.com/x"),
            &u("http://a.com/x")
        ));
        // 换端口不能跟
        assert!(!is_permitted_redirect(
            &u("https://a.com/x"),
            &u("https://a.com:8080/x")
        ));
        // 跳转目标带凭证不能跟
        assert!(!is_permitted_redirect(
            &u("https://a.com/x"),
            &u("https://u:p@a.com/x")
        ));
    }

    #[test]
    fn 默认端口与显式端口视为同源() {
        assert!(is_permitted_redirect(
            &u("https://a.com/x"),
            &u("https://a.com:443/y")
        ));
    }

    #[test]
    fn 权限内容是域名粒度() {
        assert_eq!(
            permission_content(&u("https://docs.rs/tokio/1.0/x")),
            "domain:docs.rs"
        );
    }

    #[test]
    fn 浏览器允许本地地址且保留_http() {
        let n = normalize_for_browser("http://localhost:8765/wechat.html").expect("应当通过");
        assert_eq!(n.as_str(), "http://localhost:8765/wechat.html");

        let n = normalize_for_browser("http://127.0.0.1:3000/").expect("应当通过");
        assert_eq!(n.as_str(), "http://127.0.0.1:3000/");

        let n = normalize_for_browser("http://[::1]:8080/").expect("应当通过");
        assert_eq!(n.as_str(), "http://[::1]:8080/");

        // 公网 http 也不升级 —— 浏览器打开什么就是什么
        let n = normalize_for_browser("http://example.com/a").expect("应当通过");
        assert_eq!(n.as_str(), "http://example.com/a");
    }

    #[test]
    fn 抓取仍然拒绝本地_浏览器放行() {
        // 两条路必须分叉：抓取默认打不进内网，浏览器可以看本地前端。
        assert!(matches!(
            normalize("http://localhost:8765/wechat.html"),
            Err(UrlReject::NotPublicHost { .. })
        ));
        assert!(matches!(
            normalize("http://127.0.0.1:3000/"),
            Err(UrlReject::PrivateAddress { .. })
        ));
        assert!(normalize_for_browser("http://localhost:8765/wechat.html").is_ok());
        assert!(normalize_for_browser("http://127.0.0.1:3000/").is_ok());
    }

    #[test]
    fn 浏览器允许本地文件() {
        let n = normalize_for_browser("file:///Users/me/proj/wechat.html").expect("应当通过");
        assert_eq!(n.scheme(), "file");
        assert_eq!(
            n.to_file_path().expect("能转成本地路径"),
            PathBuf::from("/Users/me/proj/wechat.html")
        );

        // 模型经常直接塞绝对路径，不带 file://
        let n = normalize_for_browser("/Users/me/proj/wechat.html").expect("应当通过");
        assert_eq!(n.scheme(), "file");
        assert_eq!(
            n.to_file_path().expect("能转成本地路径"),
            PathBuf::from("/Users/me/proj/wechat.html")
        );

        // Chromium 也认 file://localhost/...
        let n = normalize_for_browser("file://localhost/Users/me/proj/a.html").expect("应当通过");
        assert_eq!(
            n.to_file_path().expect("能转成本地路径"),
            PathBuf::from("/Users/me/proj/a.html")
        );
    }

    #[test]
    fn 浏览器拒绝远程_file_和危险协议() {
        // 网上邻居不是本地预览
        assert!(matches!(
            normalize_for_browser("file://nas.local/share/a.html"),
            Err(UrlReject::RemoteFile { .. })
        ));
        // data: 能绕过一切网络检查；javascript: 是代码执行
        for bad in ["data:text/html,hi", "javascript:alert(1)", "ftp://example.com/x"] {
            assert!(
                matches!(normalize_for_browser(bad), Err(UrlReject::BadScheme { .. })),
                "{bad} 应当被拒"
            );
        }
        assert_eq!(
            normalize_for_browser("https://user:pw@localhost/"),
            Err(UrlReject::HasCredentials)
        );
        // rust-url 把带凭证的 file:// 直接判成解析失败，两条路都是拒绝。
        assert!(normalize_for_browser("file://user:pw@localhost/tmp/a.html").is_err());
    }

    #[test]
    fn 抓取仍然拒绝_file_浏览器放行() {
        assert!(matches!(
            normalize("file:///Users/me/proj/wechat.html"),
            Err(UrlReject::BadScheme { .. })
        ));
        assert!(normalize_for_browser("file:///Users/me/proj/wechat.html").is_ok());
    }

    #[test]
    fn 本地文件的权限键是目录粒度() {
        assert_eq!(
            permission_content(&u("file:///Users/me/proj/wechat.html")),
            "file:/Users/me/proj"
        );
        assert_eq!(
            permission_content(&u("https://docs.rs/tokio/1.0/x")),
            "domain:docs.rs"
        );
    }

}
