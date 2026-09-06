//! 远程访问的准入：令牌、来源校验、失败限速。
//!
//! 这扇门后面是"以你的身份在这台机器上跑命令"。所以这里的每一条都按
//! **fail-closed** 写：拿不到令牌 = 服务不起；Origin 对不上 = 拒；连续猜错 =
//! 这个来源先歇一会。没有任何一条"方便起见先放过去"。
//!
//! 不做的事（以及为什么）：
//! - **不做 TLS**。证书要么自签（手机上一堆警告，用户学会点"仍然继续"之后
//!   任何中间人都能过），要么要域名。局域网直连本来就在自己的 Wi-Fi 里；
//!   出公网走 Tailscale Serve / Cloudflare Tunnel / nginx，它们的证书管理
//!   比我们自己搞一套靠谱得多。
//! - **不做多用户**。一台机器一个 Riot 一个主人，令牌就是主人的身份。

// 宿主层：真实随机源、真实时钟。见 clippy.toml。
#![allow(clippy::disallowed_methods)]

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine as _;

/// 令牌长度（字节）。32 字节 = 256 位，base64url 之后 43 个字符。
const TOKEN_BYTES: usize = 32;

/// 连续猜错多少次开始限速。
const MAX_FAILURES: u32 = 5;
/// 限速多久。
const LOCKOUT: Duration = Duration::from_secs(60);

/// 生成一枚新令牌（base64url，无填充）。
pub fn generate_token() -> String {
    use rand::TryRngCore as _;
    let mut bytes = [0u8; TOKEN_BYTES];
    // 豁免理由：这是安全令牌，必须是系统随机源；注入 IdGenerator 在这里
    // 没有意义（内核的确定性回放不经过宿主的鉴权）。
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .expect("系统随机源不可用");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// 常数时间比较。长度不同直接假，但不提前返回 —— 两个字节串谁长谁短本身
/// 不是秘密（令牌长度是公开的），泄漏的只有"对了几位"，而那正是这函数
/// 要藏的。
pub fn token_matches(expected: &str, given: &str) -> bool {
    let a = expected.as_bytes();
    let b = given.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 按来源 IP 记失败次数。
///
/// 目的不是抗住有组织的暴力破解（256 位令牌不需要这个），是让**误配**
/// 不至于刷屏：一台手机上存了旧令牌的页面每秒重连一次，日志里就是每秒
/// 一条鉴权失败。限速之后它一分钟只能试五次。
#[derive(Default)]
pub struct Throttle {
    inner: Mutex<HashMap<IpAddr, Entry>>,
}

struct Entry {
    failures: u32,
    locked_until: Option<Instant>,
}

impl Throttle {
    /// 这个来源现在能不能试。
    pub fn allows(&self, ip: IpAddr) -> bool {
        let mut g = self.inner.lock().expect("限速表锁");
        match g.get_mut(&ip) {
            Some(e) => match e.locked_until {
                Some(until) if Instant::now() < until => false,
                Some(_) => {
                    // 锁到期，清零重来。
                    e.failures = 0;
                    e.locked_until = None;
                    true
                }
                None => true,
            },
            None => true,
        }
    }

    /// 记一次失败。到阈值就上锁。
    pub fn record_failure(&self, ip: IpAddr) {
        let mut g = self.inner.lock().expect("限速表锁");
        let e = g.entry(ip).or_insert(Entry {
            failures: 0,
            locked_until: None,
        });
        e.failures += 1;
        if e.failures >= MAX_FAILURES {
            e.locked_until = Some(Instant::now() + LOCKOUT);
        }
        // 表不能无限长：只保留还有意义的条目（锁着的、或最近失败过的）。
        if g.len() > 1024 {
            let now = Instant::now();
            g.retain(|_, e| e.locked_until.is_some_and(|u| u > now));
        }
    }

    /// 成功了就把这个来源的账清掉。
    pub fn record_success(&self, ip: IpAddr) {
        self.inner.lock().expect("限速表锁").remove(&ip);
    }
}

/// WebSocket 握手的来源校验。
///
/// 浏览器发起 WebSocket 时会带 `Origin`，而任何网页都能对任意地址发起连接
/// —— 没有这层的话，用户在同一个浏览器里打开一个恶意页面，那页面就能连上
/// 本机的 7823 端口。令牌仍然拦得住它（它拿不到 localStorage 里别的源的
/// 令牌），这层是纵深防御：万一令牌从别处漏了，至少还得从对的页面发起。
///
/// 规则：`Origin` 的 authority（host:port）必须等于请求的 `Host`；没带
/// `Origin` 的（命令行客户端、curl）放过 —— 它们不受同源策略约束，拦它们
/// 没有意义，令牌那关照样要过。反向代理场景下两者可能对不上，用
/// `RIOT_REMOTE_ALLOWED_ORIGINS`（逗号分隔的完整 origin）显式放行。
pub fn origin_allowed(origin: Option<&str>, host: Option<&str>, extra_allowed: &[String]) -> bool {
    let Some(origin) = origin else {
        return true;
    };
    if extra_allowed
        .iter()
        .any(|o| o.trim_end_matches('/') == origin)
    {
        return true;
    }
    let Some(host) = host else {
        return false;
    };
    // origin 形如 `http://192.168.1.5:7823`，去掉 scheme 比 authority。
    let authority = origin
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(origin)
        .trim_end_matches('/');
    authority.eq_ignore_ascii_case(host)
}

/// 限速按哪个 IP 记账。
///
/// 直连时就是 TCP 对端。反向代理后所有客户端的对端都是代理那一个 IP，
/// 一台配错的手机会把代理后面的所有设备一起锁一分钟；这时该看
/// `X-Forwarded-For` 的第一跳。但这个头任何人都能伪造，**只在用户明确说了
/// "我在代理后面"**（配了 `RIOT_REMOTE_ALLOWED_ORIGINS`）时才信它 —— 直连
/// 场景下信它等于让攻击者随便挑一个 IP 顶账。
pub fn client_ip(peer: IpAddr, forwarded_for: Option<&str>, behind_proxy: bool) -> IpAddr {
    if !behind_proxy {
        return peer;
    }
    forwarded_for
        .and_then(|v| v.split(',').next())
        .and_then(|s| s.trim().parse::<IpAddr>().ok())
        .unwrap_or(peer)
}

/// 环境变量里额外放行的 origin。
pub fn extra_allowed_origins() -> Vec<String> {
    std::env::var("RIOT_REMOTE_ALLOWED_ORIGINS")
        .ok()
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 令牌是_43_位_base64url() {
        let t = generate_token();
        assert_eq!(t.len(), 43);
        assert!(
            t.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert_ne!(generate_token(), t, "两次生成不能相同");
    }

    #[test]
    fn 令牌比较要全等() {
        assert!(token_matches("abc", "abc"));
        assert!(!token_matches("abc", "abd"));
        assert!(!token_matches("abc", "ab"));
        assert!(!token_matches("", "a"));
    }

    #[test]
    fn 来源必须和_host_同源() {
        assert!(
            origin_allowed(None, Some("x:1"), &[]),
            "没有 Origin 的非浏览器客户端放过"
        );
        assert!(origin_allowed(
            Some("http://192.168.1.5:7823"),
            Some("192.168.1.5:7823"),
            &[]
        ));
        assert!(origin_allowed(
            Some("http://LOCALHOST:7823"),
            Some("localhost:7823"),
            &[]
        ));
        assert!(!origin_allowed(
            Some("http://evil.example"),
            Some("localhost:7823"),
            &[]
        ));
        assert!(!origin_allowed(Some("http://localhost:7823"), None, &[]));
        // 反向代理：显式放行列表
        assert!(origin_allowed(
            Some("https://riot.tail1234.ts.net"),
            Some("127.0.0.1:7823"),
            &["https://riot.tail1234.ts.net/".to_owned()]
        ));
    }

    #[test]
    fn 只在明确处于代理后时才信_forwarded_for() {
        let peer: IpAddr = "10.0.0.1".parse().expect("ip");
        let real: IpAddr = "192.168.1.7".parse().expect("ip");
        // 直连：这个头谁都能伪造，不看。
        assert_eq!(client_ip(peer, Some("192.168.1.7"), false), peer);
        // 代理后：取第一跳。
        assert_eq!(client_ip(peer, Some("192.168.1.7, 10.0.0.1"), true), real);
        // 头缺失或不是 IP：退回对端。
        assert_eq!(client_ip(peer, None, true), peer);
        assert_eq!(client_ip(peer, Some("not-an-ip"), true), peer);
    }

    #[test]
    fn 连错五次之后同一来源被挡住() {
        let t = Throttle::default();
        let ip: IpAddr = "10.0.0.9".parse().expect("ip");
        for _ in 0..4 {
            t.record_failure(ip);
            assert!(t.allows(ip));
        }
        t.record_failure(ip);
        assert!(!t.allows(ip), "第五次失败后锁住");
        // 别的来源不受影响
        assert!(t.allows("10.0.0.10".parse().expect("ip")));
        t.record_success(ip);
        assert!(t.allows(ip), "成功一次就清账");
    }
}
