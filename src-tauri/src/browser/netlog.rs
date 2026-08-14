//! 把累积的 `Network.*` CDP 事件整形成给模型看的文本。
//!
//! 纯函数:输入是 [`super::taps`] 攒下的事件数组，输出是文本。抓包、看细节、
//! 安全审计都在这里，好单独测 —— 而"开 Network.enable、取响应体"这些要跟
//! 浏览器打交道的活留在 [`super::access`]。
//!
//! # 事件形状
//!
//! - `Network.requestWillBeSent`：`params.requestId`、`params.request.{url,method,headers}`
//! - `Network.responseReceived`：`params.requestId`、`params.response.{url,status,headers,mimeType}`
//! - `Network.loadingFinished`：`params.requestId`、`params.encodedDataLength`

use std::collections::HashMap;

use serde_json::Value;

/// list 一次最多列多少条。太多会把上下文冲垮，而模型要的通常是"最近发生
/// 了哪些请求"。
const MAX_LIST: usize = 200;

/// 响应体在 detail 里最多显示多少字。
pub const MAX_BODY: usize = 4000;

/// 一条请求从事件里攒起来的样子。
#[derive(Default)]
struct Req {
    method: String,
    url: String,
    status: Option<i64>,
    mime: String,
    size: Option<f64>,
}

/// 把事件按 requestId 归拢成请求表，保留首次出现的顺序。
fn assemble(events: &[Value]) -> Vec<(String, Req)> {
    let mut map: HashMap<String, Req> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    for e in events {
        let id = e["params"]["requestId"].as_str().unwrap_or_default().to_owned();
        if id.is_empty() {
            continue;
        }
        let method = e["method"].as_str().unwrap_or_default();
        let entry = map.entry(id.clone()).or_insert_with(|| {
            order.push(id.clone());
            Req::default()
        });
        match method {
            "Network.requestWillBeSent" => {
                let req = &e["params"]["request"];
                if entry.url.is_empty() {
                    entry.url = req["url"].as_str().unwrap_or_default().to_owned();
                }
                if entry.method.is_empty() {
                    entry.method = req["method"].as_str().unwrap_or("GET").to_owned();
                }
            }
            "Network.responseReceived" => {
                let resp = &e["params"]["response"];
                entry.status = resp["status"].as_i64();
                entry.mime = resp["mimeType"].as_str().unwrap_or_default().to_owned();
                if entry.url.is_empty() {
                    entry.url = resp["url"].as_str().unwrap_or_default().to_owned();
                }
            }
            "Network.loadingFinished" => {
                entry.size = e["params"]["encodedDataLength"].as_f64();
            }
            _ => {}
        }
    }

    order.into_iter().filter_map(|id| map.remove(&id).map(|r| (id, r))).collect()
}

/// 列出抓到的请求。`filter` 是 URL 子串（大小写不敏感）。
pub fn list(events: &[Value], filter: Option<&str>) -> String {
    let reqs = assemble(events);
    if reqs.is_empty() {
        return "还没抓到网络请求。先调一次这个工具开着累积，再刷新或操作页面。".to_owned();
    }
    let needle = filter.map(str::to_ascii_lowercase);
    let mut shown = 0;
    let mut out = String::new();
    for (id, r) in &reqs {
        if let Some(n) = &needle
            && !r.url.to_ascii_lowercase().contains(n.as_str())
        {
            continue;
        }
        if shown >= MAX_LIST {
            out.push_str("…（更多请求已略去，用 filter 缩小范围）\n");
            break;
        }
        let status = r.status.map_or_else(|| "…".to_owned(), |s| s.to_string());
        let size = r.size.map_or_else(String::new, |b| format!(" {}B", b as i64));
        let mime = if r.mime.is_empty() { String::new() } else { format!(" {}", r.mime) };
        out.push_str(&format!("#{id} {} {} {status}{mime}{size}\n", r.method, r.url));
        shown += 1;
    }
    if out.is_empty() {
        return format!("抓到 {} 条请求，但没有匹配 filter 的。", reqs.len());
    }
    out
}

/// 取某条请求的请求头/响应头（从事件里）。响应体由调用方另取后拼进来。
/// 返回 `None` 表示这个 id 没抓到。
pub fn detail_headers(events: &[Value], request_id: &str) -> Option<String> {
    let mut req_line = String::new();
    let mut req_headers = String::new();
    let mut resp_line = String::new();
    let mut resp_headers = String::new();

    for e in events {
        if e["params"]["requestId"].as_str() != Some(request_id) {
            continue;
        }
        match e["method"].as_str().unwrap_or_default() {
            "Network.requestWillBeSent" => {
                let req = &e["params"]["request"];
                req_line = format!(
                    "{} {}",
                    req["method"].as_str().unwrap_or("GET"),
                    req["url"].as_str().unwrap_or_default()
                );
                req_headers = fmt_headers(&req["headers"]);
            }
            "Network.responseReceived" => {
                let resp = &e["params"]["response"];
                resp_line = format!(
                    "{} {}",
                    resp["status"].as_i64().unwrap_or(0),
                    resp["statusText"].as_str().unwrap_or("")
                );
                resp_headers = fmt_headers(&resp["headers"]);
            }
            _ => {}
        }
    }

    if req_line.is_empty() && resp_line.is_empty() {
        return None;
    }
    let mut out = String::new();
    out.push_str(&format!("请求: {req_line}\n{req_headers}\n"));
    out.push_str(&format!("响应: {resp_line}\n{resp_headers}"));
    Some(out)
}

/// CDP 的 headers 是一个 `{name: value}` 对象。排序输出，稳定好读。
fn fmt_headers(headers: &Value) -> String {
    let Some(obj) = headers.as_object() else {
        return String::new();
    };
    let mut lines: Vec<String> = obj
        .iter()
        .map(|(k, v)| format!("  {k}: {}", v.as_str().unwrap_or_default()))
        .collect();
    lines.sort();
    lines.join("\n")
}

/// 安全审计:找主文档的响应头，检查该有而没有的安全头、以及明显的弱配置。
///
/// `page_url` 用来认出哪条是主文档（导航到的那个地址）。
pub fn audit(events: &[Value], page_url: &str) -> String {
    // 主文档:type=Document 的响应，退而求其次用 URL 匹配。
    let doc = events.iter().find(|e| {
        e["method"].as_str() == Some("Network.responseReceived")
            && (e["params"]["type"].as_str() == Some("Document")
                || e["params"]["response"]["url"].as_str() == Some(page_url))
    });
    let Some(doc) = doc else {
        return "还没抓到主文档的响应。先开着抓包再刷新页面，然后再审计。".to_owned();
    };

    let headers = &doc["params"]["response"]["headers"];
    // 头名大小写不敏感，统一小写来查。
    let lower: HashMap<String, String> = headers
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(k, v)| (k.to_ascii_lowercase(), v.as_str().unwrap_or_default().to_owned()))
                .collect()
        })
        .unwrap_or_default();
    let has = |k: &str| lower.contains_key(k);
    let get = |k: &str| lower.get(k).cloned().unwrap_or_default();

    let mut findings: Vec<String> = Vec::new();

    if !has("content-security-policy") {
        findings.push("缺少 Content-Security-Policy:没有 CSP，XSS 少了一道纵深防线。".into());
    }
    if !has("strict-transport-security") {
        findings.push("缺少 Strict-Transport-Security (HSTS):可能被降级到 HTTP 中间人。".into());
    }
    if !has("x-frame-options") && !get("content-security-policy").contains("frame-ancestors") {
        findings.push("缺少 X-Frame-Options / frame-ancestors:可能被点击劫持（iframe 套壳）。".into());
    }
    if !has("x-content-type-options") {
        findings.push("缺少 X-Content-Type-Options: nosniff:浏览器可能按内容猜类型。".into());
    }
    if !has("referrer-policy") {
        findings.push("缺少 Referrer-Policy:跳转外链时可能泄露完整来源 URL。".into());
    }
    let acao = get("access-control-allow-origin");
    if acao == "*" {
        let creds = get("access-control-allow-credentials");
        if creds.eq_ignore_ascii_case("true") {
            findings.push(
                "CORS 高危:Access-Control-Allow-Origin: * 同时 Allow-Credentials: true——\
                 规范上不该同时出现，多半是配置错误，可能导致带凭证的跨源读取。"
                    .into(),
            );
        } else {
            findings.push("CORS 宽松:Access-Control-Allow-Origin: *（任意源可读响应）。".into());
        }
    }

    if findings.is_empty() {
        return "主文档响应头的安全配置没发现明显问题。".to_owned();
    }
    let mut out = String::from("响应头安全审计发现:\n");
    for (i, f) in findings.iter().enumerate() {
        out.push_str(&format!("{}. {f}\n", i + 1));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sent(id: &str, method: &str, url: &str) -> Value {
        json!({
            "method": "Network.requestWillBeSent",
            "params": { "requestId": id, "request": { "url": url, "method": method, "headers": { "Accept": "*/*" } } }
        })
    }
    fn recv(id: &str, status: i64, mime: &str, url: &str, headers: Value) -> Value {
        json!({
            "method": "Network.responseReceived",
            "params": { "requestId": id, "type": "Document",
                "response": { "url": url, "status": status, "statusText": "OK", "mimeType": mime, "headers": headers } }
        })
    }

    #[test]
    fn list_按请求归拢并显示状态() {
        let events = vec![
            sent("1", "GET", "https://x.test/"),
            recv("1", 200, "text/html", "https://x.test/", json!({})),
            sent("2", "POST", "https://x.test/api/login"),
            recv("2", 401, "application/json", "https://x.test/api/login", json!({})),
        ];
        let out = list(&events, None);
        assert!(out.contains("#1 GET https://x.test/ 200"), "{out}");
        assert!(out.contains("#2 POST https://x.test/api/login 401"), "{out}");
    }

    #[test]
    fn list_按_url_子串过滤() {
        let events = vec![
            sent("1", "GET", "https://x.test/style.css"),
            sent("2", "POST", "https://x.test/api/login"),
        ];
        let out = list(&events, Some("api"));
        assert!(out.contains("/api/login"), "{out}");
        assert!(!out.contains("style.css"), "过滤该排除掉不匹配的：{out}");
    }

    #[test]
    fn detail_给出请求和响应头() {
        let events = vec![
            sent("7", "GET", "https://x.test/"),
            recv("7", 200, "text/html", "https://x.test/", json!({ "Server": "nginx" })),
        ];
        let d = detail_headers(&events, "7").expect("有这条");
        assert!(d.contains("GET https://x.test/"), "{d}");
        assert!(d.contains("Server: nginx"), "{d}");
        assert!(detail_headers(&events, "999").is_none(), "没有的 id 返回 None");
    }

    #[test]
    fn audit_挑出缺失的安全头() {
        // 一个啥安全头都没有的响应，应当把几条主要的都点出来。
        let events = vec![recv("1", 200, "text/html", "https://x.test/", json!({}))];
        let out = audit(&events, "https://x.test/");
        assert!(out.contains("Content-Security-Policy"), "{out}");
        assert!(out.contains("HSTS"), "{out}");
        assert!(out.contains("X-Frame-Options"), "{out}");
    }

    #[test]
    fn audit_识别_cors_高危组合() {
        let headers = json!({
            "Access-Control-Allow-Origin": "*",
            "Access-Control-Allow-Credentials": "true",
        });
        let events = vec![recv("1", 200, "text/html", "https://x.test/", headers)];
        let out = audit(&events, "https://x.test/");
        assert!(out.contains("CORS 高危"), "带凭证的通配 CORS 要标高危：{out}");
    }

    #[test]
    fn audit_配置齐全时不误报() {
        let headers = json!({
            "Content-Security-Policy": "default-src 'self'; frame-ancestors 'none'",
            "Strict-Transport-Security": "max-age=63072000",
            "X-Frame-Options": "DENY",
            "X-Content-Type-Options": "nosniff",
            "Referrer-Policy": "no-referrer",
        });
        let events = vec![recv("1", 200, "text/html", "https://x.test/", headers)];
        let out = audit(&events, "https://x.test/");
        assert!(out.contains("没发现明显问题"), "{out}");
    }
}
