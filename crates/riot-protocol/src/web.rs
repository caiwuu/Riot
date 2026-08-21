//! 联网能力的注入点。
//!
//! `WebFetch` 和 `WebSearch` 需要三件工具层自己给不了的东西：发 HTTP 请求、
//! 调搜索后端、用辅助模型蒸馏长文本。三件都是非确定性的，都依赖用户配置，
//! 所以都在 [`WebAccess`] 这个 trait 后面。
//!
//! # 为什么是一个 trait 而不是三个
//!
//! 它们的生命周期完全一致：都由宿主在装配会话时构造，都要**每次调用现读配置**
//! （用户可能在会话中途改了搜索后端或辅助模型），都只被这两个工具用到。
//! 拆成三个字段塞进 [`crate::tool::ToolContext`] 只是让每个测试替身多写两遍
//! 空实现，换不到任何解耦。
//!
//! # 什么不在这里
//!
//! URL 校验、重定向策略、HTML 转 Markdown、缓存 —— 那些是**纯逻辑**，
//! 留在 `riot-tools` 里，可以脱离网络测。这个 trait 只包非确定性的那一层。
//!
//! `[约束]` 实现必须**不跟随重定向**，并把 `Location` 原样交回。跟随重定向的
//! 决策权在工具层：跨站跳转要重新过一遍域名权限，客户端自动跟掉就等于让
//! 一个可信域名上的开放重定向漏洞绕过白名单。见 `tools::web::url::is_permitted_redirect`。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

#[async_trait]
pub trait WebAccess: Send + Sync {
    /// 发一个 GET。
    ///
    /// `[约束]` 不跟随重定向，不做 URL 校验 —— 两件事都在工具层做过了。
    /// 实现只负责两件工具层做不到的事：真的发请求，以及**在连接前拒绝
    /// 解析到私有网段的主机**（DNS 解析发生在这一层，工具层看不到）。
    async fn get(
        &self,
        req: WebRequest,
        cancel: &CancellationToken,
    ) -> Result<WebResponse, WebError>;

    /// 执行一次搜索。
    ///
    /// 没配后端时返回 [`WebError::NotConfigured`] —— 工具层据此给模型一条
    /// 能让用户去配置的提示，而不是假装搜了个空结果。
    async fn search(
        &self,
        query: SearchQuery,
        cancel: &CancellationToken,
    ) -> Result<Vec<SearchHit>, WebError>;

    /// 用辅助模型把长文本按 prompt 提炼。
    ///
    /// 这是整条链路上最省 token 的一步：十万字的网页进去，几百字出来，
    /// 主循环的上下文不会被网页噪音撑爆。
    ///
    /// 没配辅助模型时返回 [`WebError::NotConfigured`]，**调用方必须能降级**
    /// （截断后原样返回），而不是让整个 WebFetch 失败 —— 拿不到摘要总比
    /// 拿不到网页强。
    async fn distill(
        &self,
        req: DistillRequest,
        cancel: &CancellationToken,
    ) -> Result<String, WebError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// 响应体字节上限。超过就中断下载并返回 [`WebError::TooLarge`]。
    ///
    /// `[约束]` 必须在**流式读取时**逐块判断，不能等下完再看长度 ——
    /// 服务端可以不报 `Content-Length`，那样"下完再看"等于没有上限。
    pub max_bytes: u64,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebResponse {
    pub status: u16,
    pub status_text: String,
    /// `content-type` 响应头，取不到就是空串。
    pub content_type: String,
    pub body: Vec<u8>,
    /// 3xx 时的跳转目标，**已按请求 URL 解析成绝对地址**。
    ///
    /// 相对地址的解析放在实现里而不是工具层：工具层要再解析一次就得自己
    /// 带一个 URL 库，而 `Location: /foo` 这种相对跳转非常常见。
    pub location: Option<String>,
}

impl WebResponse {
    pub fn is_redirect(&self) -> bool {
        matches!(self.status, 301 | 302 | 303 | 307 | 308)
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WebError {
    /// 用户还没配这个能力。**不是错误，是缺配置** —— 提示要指向设置页。
    #[error("{what} 尚未配置")]
    NotConfigured { what: String },

    /// 被安全策略挡下（私有网段、协议不对、URL 太长）。
    #[error("已拦截：{reason}")]
    Blocked { reason: String },

    #[error("连接失败：{message}")]
    Transport { message: String },

    #[error("HTTP {code}：{body}")]
    Status { code: u16, body: String },

    #[error("响应超过 {limit} 字节上限")]
    TooLarge { limit: u64 },

    #[error("已取消")]
    Cancelled,
}

// ────────────────────────────────────────────────────────────
// 搜索
// ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchQuery {
    pub query: String,
    pub max_results: usize,
    /// 只要这些域名的结果。与 `blocked_domains` 互斥，由工具层校验。
    pub allowed_domains: Vec<String>,
    pub blocked_domains: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    /// 搜索引擎给的摘要。可能为空。
    #[serde(default)]
    pub snippet: String,
    /// 后端直接返回的正文。
    ///
    /// Tavily / Exa 这类为 LLM 设计的后端会带上它，此时工具层可以跳过
    /// "再抓一次网页"那一步。SearXNG 只给 snippet，这里是 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_content: Option<String>,
}

// ────────────────────────────────────────────────────────────
// 蒸馏
// ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistillRequest {
    pub system: String,
    pub user: String,
    pub max_output_tokens: Option<u32>,
}

// ────────────────────────────────────────────────────────────
// 内网地址判定
// ────────────────────────────────────────────────────────────

/// 这个 IP 是不是不该被抓取工具访问。
///
/// 放在协议层是因为它有两个调用方，而且**两个都不能少**：
///
/// - `riot-tools` 用它拦字面量地址（`http://[::1]/`）；
/// - `riot-runtime` 用它拦 DNS 解析结果（`http://metadata.evil.com/`
///   解析到 `169.254.169.254`）。
///
/// 复制一份到两边是不可接受的 —— 安全谓词的两份实现迟早会漂移，而漂移
/// 的那一侧不会有任何报错。
///
/// # 判据是"可信网络内部"，不是"保留网段"
///
/// 拦的是**因为本进程身处某个可信网络才够得着**的地址：本机服务、
/// 局域网设备、云厂商元数据服务。这条界线决定了取舍 —— 保留但不构成
/// 内网攻击面的网段（如 `198.18.0.0/15`，见 [`is_private_v4`]）不在此列。
/// 按"是不是保留网段"来拦会误伤，而误伤在这里的代价是功能完全不可用。
pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_v4(v4),
        IpAddr::V6(v6) => is_private_v6(v6),
    }
}

/// # 为什么 `198.18.0.0/15` **不**在拦截名单里
///
/// 它是 RFC 2544 的基准测试段，按"保留网段"分类确实该拦。但它同时是
/// Clash / mihomo / Surge 这些代理工具 **fake-ip 模式的默认地址池**
/// （mihomo 的默认值就是 `198.18.0.1/16`）。
///
/// fake-ip 模式下，每一个域名都被解析成这个段里的一个合成地址，再由
/// TUN 层映射回域名去路由 —— `docs.rs`、`github.com`、`baidu.com` 全部
/// 落在这里。拦掉它的后果不是"少抓一个站"，是**一个网页都抓不了**。
///
/// 放行它的代价接近于零：这个段既不是本机服务、也不是局域网设备、
/// 更不是元数据服务所在的地方，不构成内网攻击面。真正危险的那几段
/// （`127/8`、`10/8`、`172.16/12`、`192.168/16`、`169.254/16`）一个没动。
///
/// `[前提]` 要知道的是，**处在 fake-ip 代理之下时，基于 DNS 的 SSRF
/// 防护本来就已经失效** —— 所有域名都解析成假地址，我们根本看不到真实
/// 目的地。拦掉 `198.18/15` 并不能把这层防护找回来，只是把"没有防护"
/// 变成"没有功能"。这种环境下真正还在起作用的是：字面量地址检查
/// （`http://169.254.169.254/` 在 DNS 之前就被拒）、代理自己的
/// `fake-ip-filter`（本机名照常返回真实的 127.0.0.1，仍会被拦）、
/// 以及按域名逐个征求用户同意。
fn is_private_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        // 100.64.0.0/10 运营商级 NAT，也是 Tailscale 用的段 —— 那确实
        // 是个私有网络，拦对了。std 的 is_shared() 还没稳定。
        || (o[0] == 100 && (64..128).contains(&o[1]))
        // 240.0.0.0/4 保留段。没有任何东西该住在这里。
        || o[0] >= 240
}

fn is_private_v6(ip: Ipv6Addr) -> bool {
    // IPv4 映射地址（::ffff:127.0.0.1）必须按它内含的 v4 地址判 ——
    // 否则 `http://[::ffff:169.254.169.254]/` 就是一条完整的绕过路径。
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_private_v4(v4);
    }
    let seg = ip.segments();
    ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        // fc00::/7 唯一本地地址
        || (seg[0] & 0xfe00) == 0xfc00
        // fe80::/10 链路本地
        || (seg[0] & 0xffc0) == 0xfe80
}

// ────────────────────────────────────────────────────────────
// fail-closed 默认实现
// ────────────────────────────────────────────────────────────

/// 什么都不会做的 [`WebAccess`]。
///
/// 用在两个地方：不关心联网的单元测试，以及宿主还没装配好联网能力时的占位。
/// 三个方法一律返回 [`WebError::NotConfigured`] —— 默认值必须是"不能上网"，
/// 反过来的话，忘了装配的表现是"工具悄悄用了某个默认后端"。
pub struct NoWeb;

#[async_trait]
impl WebAccess for NoWeb {
    async fn get(
        &self,
        _req: WebRequest,
        _cancel: &CancellationToken,
    ) -> Result<WebResponse, WebError> {
        Err(WebError::NotConfigured {
            what: "联网访问".to_owned(),
        })
    }

    async fn search(
        &self,
        _query: SearchQuery,
        _cancel: &CancellationToken,
    ) -> Result<Vec<SearchHit>, WebError> {
        Err(WebError::NotConfigured {
            what: "搜索后端".to_owned(),
        })
    }

    async fn distill(
        &self,
        _req: DistillRequest,
        _cancel: &CancellationToken,
    ) -> Result<String, WebError> {
        Err(WebError::NotConfigured {
            what: "辅助模型".to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn 默认实现一律拒绝() {
        // 默认值搞反的后果是静默的：忘了装配联网能力时，工具会用上某个
        // 兜底后端，而没有任何地方会报错。
        let w = NoWeb;
        let c = CancellationToken::new();

        assert!(matches!(
            w.get(
                WebRequest {
                    url: "https://example.com".into(),
                    headers: vec![],
                    max_bytes: 1024,
                    timeout_ms: 1000,
                },
                &c
            )
            .await,
            Err(WebError::NotConfigured { .. })
        ));
        assert!(matches!(
            w.search(SearchQuery::default(), &c).await,
            Err(WebError::NotConfigured { .. })
        ));
        assert!(matches!(
            w.distill(
                DistillRequest {
                    system: String::new(),
                    user: String::new(),
                    max_output_tokens: None,
                },
                &c
            )
            .await,
            Err(WebError::NotConfigured { .. })
        ));
    }

    #[test]
    fn 内网网段判定() {
        // 这两条各自对应一个调用方：字面量地址和 DNS 解析结果
        for bad in [
            "127.0.0.1",
            "10.0.0.5",
            "192.168.1.1",
            "172.16.0.1",
            "169.254.169.254",
            "0.0.0.0",
            "100.64.0.1",
            "255.255.255.255",
        ] {
            assert!(
                is_private_ip(bad.parse().expect("v4")),
                "{bad} 应当判为内网"
            );
        }
        for bad in ["::1", "fe80::1", "fc00::1", "::ffff:169.254.169.254"] {
            assert!(
                is_private_ip(bad.parse().expect("v6")),
                "{bad} 应当判为内网"
            );
        }
        for ok in ["1.1.1.1", "8.8.8.8", "93.184.216.34"] {
            assert!(!is_private_ip(ok.parse().expect("v4")), "{ok} 是公网地址");
        }
        assert!(!is_private_ip("2606:4700::1111".parse().expect("v6")));
    }

    #[test]
    fn fake_ip_网段必须放行() {
        // 198.18.0.0/15 是 Clash / mihomo / Surge 在 fake-ip 模式下的
        // 默认地址池（mihomo 默认 198.18.0.1/16）。那种模式下**每一个**
        // 域名都解析到这里 —— 拦掉它等于 WebFetch 一个网页都抓不了。
        //
        // 它按 RFC 2544 确实是保留段，所以很容易被"顺手补全保留网段"
        // 的改动加回来。这条测试就是拦那种改动的。
        for addr in ["198.18.0.1", "198.18.2.40", "198.19.255.255"] {
            assert!(
                !is_private_ip(addr.parse().expect("v4")),
                "{addr} 属于 fake-ip 网段，拦掉它会让所有抓取失效"
            );
        }
    }

    #[test]
    fn 重定向状态码判定() {
        let r = |status| WebResponse {
            status,
            status_text: String::new(),
            content_type: String::new(),
            body: vec![],
            location: None,
        };
        for s in [301, 302, 303, 307, 308] {
            assert!(r(s).is_redirect(), "{s} 应该算重定向");
        }
        // 304 不算 —— 它没有 Location，当成重定向会让工具去读一个不存在的字段
        assert!(!r(304).is_redirect());
        assert!(!r(200).is_redirect());
        assert!(r(200).is_success());
    }
}
