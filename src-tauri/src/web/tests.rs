//! 宿主联网层的测试。
//!
//! 分两类：装配逻辑（哪块配了、哪块没配）和**打真实 socket** 的搜索链路。
//! 后者起一个只会说一句话的 TCP 服务器，覆盖单元测试碰不到的那一段 ——
//! 真的发一次 HTTP 请求。
//!
//! # 这里最要紧的一条
//!
//! [`搜索走的客户端不受内网拦截`]。抓取用的客户端装了 `PublicOnlyResolver`，
//! 连 `127.0.0.1` 会被直接拒；搜索用的客户端故意**没装**，因为 SearXNG
//! 最常见的部署就是本机 docker。
//!
//! 这个区别只写在注释里的话，下一个看到"两个 reqwest::Client 好像重复了"
//! 的人会把它们合成一个 —— 那之后所有本机部署的 SearXNG 全部失效，报出来的
//! 还是一句语焉不详的连接错误。这条测试会先一步失败。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::config::WebConfig;

fn cfg(web: WebConfig) -> AppConfig {
    AppConfig {
        web,
        ..Default::default()
    }
}

// ────────────────────────────────────────────────────────────
// 装配：每一块独立降级
// ────────────────────────────────────────────────────────────

#[tokio::test]
async fn 关掉抓取之后抓取报未配置() {
    let w = HostWeb::from_config(&cfg(WebConfig {
        fetch_enabled: false,
        ..Default::default()
    }));
    let e = w
        .get(
            WebRequest {
                url: "https://example.com".into(),
                headers: vec![],
                max_bytes: 1024,
                timeout_ms: 1000,
            },
            &CancellationToken::new(),
        )
        .await
        .expect_err("关掉了就该拒绝");
    assert!(matches!(e, WebError::NotConfigured { .. }));
}

#[tokio::test]
async fn 开关开着但没填地址等于没配搜索() {
    // 只开开关不填地址是设置页里必然出现的中间状态。这时候要报"未配置"
    // （提示去填地址），不能变成一个连接失败。
    let w = HostWeb::from_config(&cfg(WebConfig {
        search_enabled: true,
        searxng_url: "   ".into(),
        ..Default::default()
    }));
    let e = w
        .search(SearchQuery::default(), &CancellationToken::new())
        .await
        .expect_err("没地址搜不了");
    assert!(matches!(e, WebError::NotConfigured { .. }), "{e:?}");
}

#[tokio::test]
async fn 填了地址但开关没开也不搜() {
    let w = HostWeb::from_config(&cfg(WebConfig {
        search_enabled: false,
        searxng_url: "http://127.0.0.1:8080".into(),
        ..Default::default()
    }));
    let e = w
        .search(SearchQuery::default(), &CancellationToken::new())
        .await
        .expect_err("开关没开就不该搜");
    assert!(matches!(e, WebError::NotConfigured { .. }), "{e:?}");
}

#[tokio::test]
async fn 没配辅助模型时蒸馏报未配置而不是panic() {
    // 调用方（WebFetch）必须能据此降级成截断原文
    let w = HostWeb::from_config(&cfg(WebConfig::default()));
    let e = w
        .distill(
            DistillRequest {
                system: String::new(),
                user: String::new(),
                max_output_tokens: None,
            },
            &CancellationToken::new(),
        )
        .await
        .expect_err("没配就该说没配");
    assert!(matches!(e, WebError::NotConfigured { .. }));
}

#[test]
fn 辅助模型指向不存在的provider时只是不蒸馏() {
    // 配置写坏了不该连带把抓取也搞挂 —— 每一块独立降级
    let w = HostWeb::from_config(&cfg(WebConfig {
        distill_model: "不存在的家伙/some-model".into(),
        ..Default::default()
    }));
    assert!(w.distiller.is_none());
    assert!(w.fetch.is_some(), "蒸馏配坏了，抓取必须照常能用");
}

#[tokio::test]
async fn 测试连接会拒绝没有协议的地址() {
    // 用户很容易只填 127.0.0.1:8080
    let e = test_searxng("127.0.0.1:8080").await.expect_err("要报错");
    assert!(e.contains("http://"), "{e}");
}

// ────────────────────────────────────────────────────────────
// 真实 socket
// ────────────────────────────────────────────────────────────

/// 起一个假 SearXNG，返回 `(base_url, 收到的请求)`。
async fn fake_server(body: &'static str, content_type: &'static str) -> (String, Arc<Seen>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定本机随机端口");
    let port = listener.local_addr().expect("取端口").port();
    let seen = Arc::new(Seen::default());

    let sink = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 4096];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            sink.record(String::from_utf8_lossy(&buf[..n]).into_owned());

            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.flush().await;
        }
    });

    (format!("http://127.0.0.1:{port}"), seen)
}

#[derive(Default)]
struct Seen {
    count: AtomicUsize,
    last: std::sync::Mutex<String>,
}

impl Seen {
    fn record(&self, raw: String) {
        self.count.fetch_add(1, Ordering::SeqCst);
        *self.last.lock().expect("锁") = raw;
    }
    fn last(&self) -> String {
        self.last.lock().expect("锁").clone()
    }
}

const OK_JSON: &str = r#"{"results":[
    {"title":"Tokio","url":"https://tokio.rs/","content":"Rust 异步运行时"},
    {"title":"docs.rs tokio","url":"https://docs.rs/tokio/","content":"API 文档"}
]}"#;

fn web(base_url: &str) -> HostWeb {
    HostWeb::from_config(&cfg(WebConfig {
        search_enabled: true,
        searxng_url: base_url.to_owned(),
        ..Default::default()
    }))
}

fn query(q: &str) -> SearchQuery {
    SearchQuery {
        query: q.to_owned(),
        max_results: 10,
        ..Default::default()
    }
}

#[tokio::test]
async fn 搜索走的客户端不受内网拦截() {
    // 抓取用的客户端会拒绝 127.0.0.1；搜索用的必须不拒 —— 自托管
    // SearXNG 跑在本机是最常见的部署方式。两个客户端合并就会挂在这。
    let (base, _) = fake_server(OK_JSON, "application/json").await;

    let hits = web(&base)
        .search(query("tokio"), &CancellationToken::new())
        .await
        .expect("本机地址必须能搜");

    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].url, "https://tokio.rs/");
    assert_eq!(hits[0].snippet, "Rust 异步运行时");
}

#[tokio::test]
async fn 抓取走的客户端仍然拦内网() {
    // 上一条的另一半。少了这条，"两个客户端"退化成"一个不设防的客户端"
    // 也一样能测过。
    let (base, _) = fake_server(OK_JSON, "application/json").await;

    let e = HostWeb::from_config(&cfg(WebConfig::default()))
        .get(
            WebRequest {
                url: format!("{base}/"),
                headers: vec![],
                max_bytes: 1024,
                timeout_ms: 3000,
            },
            &CancellationToken::new(),
        )
        .await
        .expect_err("抓取本机地址必须被拒");

    assert!(matches!(e, WebError::Blocked { .. }), "{e:?}");
}

#[tokio::test]
async fn 请求里带上json格式和搜索词() {
    let (base, seen) = fake_server(OK_JSON, "application/json").await;

    web(&base)
        .search(query("tokio select"), &CancellationToken::new())
        .await
        .expect("搜索");

    let raw = seen.last();
    assert!(raw.starts_with("GET /search?"), "打错了路径：{raw}");
    // 没有这个参数 SearXNG 会回一整页 HTML
    assert!(raw.contains("format=json"), "{raw}");
    assert!(raw.contains("tokio"), "{raw}");
    // `auto` 会让 SearXNG 去读 Accept-Language，而我们不发那个头 ——
    // 实测结果会和查询词完全无关。
    assert!(raw.contains("language=all"), "语言必须显式写死：{raw}");
}

#[tokio::test]
async fn 域名过滤翻译成site语法() {
    let (base, seen) = fake_server(OK_JSON, "application/json").await;

    web(&base)
        .search(
            SearchQuery {
                query: "async".to_owned(),
                max_results: 10,
                allowed_domains: vec!["docs.rs".to_owned()],
                blocked_domains: vec![],
            },
            &CancellationToken::new(),
        )
        .await
        .expect("搜索");

    // site: 要发给引擎，而不是只在本地筛返回结果 —— 后者拿到的是
    // "十条里剩两条"，前者拿到的是"符合条件的十条"。
    assert!(seen.last().contains("site%3Adocs.rs"), "{}", seen.last());
}

#[tokio::test]
async fn 本地过滤兜住不认site的引擎() {
    let (base, _) = fake_server(OK_JSON, "application/json").await;

    let hits = web(&base)
        .search(
            SearchQuery {
                query: "async".to_owned(),
                max_results: 10,
                allowed_domains: vec!["docs.rs".to_owned()],
                blocked_domains: vec![],
            },
            &CancellationToken::new(),
        )
        .await
        .expect("搜索");

    // 假服务器不认 site:，照样回了 tokio.rs。本地这道筛子必须拦下它。
    assert_eq!(hits.len(), 1, "site: 不生效时本地过滤要兜住");
    assert_eq!(hits[0].url, "https://docs.rs/tokio/");
}

#[tokio::test]
async fn 实例没开json输出时给出可操作的提示() {
    // 官方 docker 镜像默认只开 HTML。接 SearXNG 十个人有八个先踩这个。
    let (base, _) =
        fake_server("<!DOCTYPE html><html><body>搜索页</body></html>", "text/html").await;

    let e = web(&base)
        .search(query("tokio"), &CancellationToken::new())
        .await
        .expect_err("HTML 响应必须报错");

    let msg = e.to_string();
    assert!(msg.contains("settings.yml"), "得说清改哪个文件：{msg}");
    assert!(msg.contains("formats"), "得说清加哪一项：{msg}");
}

#[tokio::test]
async fn 连不上时说清是哪个地址() {
    // 端口 9 是 discard 服务，本机基本不会有人监听
    let e = web("http://127.0.0.1:9")
        .search(query("tokio"), &CancellationToken::new())
        .await
        .expect_err("连不上必须报错");

    assert!(matches!(e, WebError::Transport { .. }), "{e:?}");
    assert!(e.to_string().contains("127.0.0.1:9"), "错误里要带上地址：{e}");
}

#[tokio::test]
async fn 已取消的搜索不发请求() {
    let (base, seen) = fake_server(OK_JSON, "application/json").await;
    let cancel = CancellationToken::new();
    cancel.cancel();

    let e = web(&base)
        .search(query("tokio"), &cancel)
        .await
        .expect_err("取消后不该继续");

    assert_eq!(e, WebError::Cancelled);
    assert_eq!(seen.count.load(Ordering::SeqCst), 0, "取消后不该发出请求");
}

#[tokio::test]
async fn 结果条数不超过上限() {
    let (base, _) = fake_server(OK_JSON, "application/json").await;

    let hits = web(&base)
        .search(
            SearchQuery {
                query: "tokio".to_owned(),
                max_results: 1,
                ..Default::default()
            },
            &CancellationToken::new(),
        )
        .await
        .expect("搜索");

    assert_eq!(hits.len(), 1);
}

#[tokio::test]
async fn 测试连接连通但没结果算失败() {
    // "通了但没结果"和"成功"对用户来说要做的事完全不同：前者要去
    // SearXNG 里启用搜索引擎，后者什么都不用做。
    let (base, _) = fake_server(r#"{"results":[]}"#, "application/json").await;

    let e = test_searxng(&base).await.expect_err("空结果不算连接成功");
    assert!(e.contains("搜索引擎"), "{e}");
}

#[tokio::test]
async fn 测试连接成功时报条数() {
    let (base, _) = fake_server(OK_JSON, "application/json").await;
    let msg = test_searxng(&base).await.expect("应当连通");
    assert!(msg.contains('2'), "{msg}");
}


