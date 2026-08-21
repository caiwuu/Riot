//! WebFetch / WebSearch 的集成测试。
//!
//! 这里测的是**工具外壳的行为**：权限表态、重定向怎么交回模型、蒸馏失败
//! 怎么降级、缓存有没有真的省下请求。纯逻辑（URL 准入、字符集、LRU）在
//! 各自文件的单元测试里。

use std::sync::Arc;

use riot_protocol::permission::{
    DecisionReason, PermissionContext, PermissionMode, PermissionResult, PermissionRule,
    RuleDecision, RuleSource,
};
use riot_protocol::tool::{Tool, ToolContext, ToolOutcome};
use riot_protocol::web::SearchHit;
use tokio_util::sync::CancellationToken;

use super::cache::PageCache;
use super::{WebFetch, WebSearch};
use crate::testing::{FakeWeb, FixedClock, NullFileState, NullFs, NullProc};

struct Harness {
    ctx: ToolContext,
    web: Arc<FakeWeb>,
    clock: Arc<FixedClock>,
}

fn harness(web: FakeWeb) -> Harness {
    let web = Arc::new(web);
    let clock = Arc::new(FixedClock::new(1_767_225_600_000));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

    let ctx = ToolContext {
        session_id: riot_protocol::id::SessionId::from_raw("s1"),
        tool_use_id: riot_protocol::id::ToolUseId::from_raw("t1"),
        cwd: "/work".into(),
        artifacts_dir: "/artifacts".into(),
        cancel: CancellationToken::new(),
        progress: riot_protocol::tool::ProgressSink::new(
            riot_protocol::id::ToolUseId::from_raw("t1"),
            tx,
        ),
        file_state: Arc::new(NullFileState),
        fs: Arc::new(NullFs),
        proc: Arc::new(NullProc),
        web: Arc::clone(&web) as Arc<_>,
        browser: Arc::new(riot_protocol::browser::NoBrowser),
        terminal: Arc::new(riot_protocol::terminal::NoTerminal),
        vision: Arc::new(riot_protocol::vision::NoVision),
        clock: Arc::clone(&clock) as Arc<_>,
    };

    Harness { ctx, web, clock }
}

fn fetch_tool() -> WebFetch {
    WebFetch::new(Arc::new(PageCache::default()))
}

fn text_of(o: &ToolOutcome) -> String {
    match o {
        ToolOutcome::Ok { model_content, .. } => format!("{model_content:?}"),
        ToolOutcome::Failed {
            error_for_model, ..
        } => error_for_model.clone(),
        ToolOutcome::Cancelled => "<cancelled>".to_owned(),
    }
}

fn rule(tool: &str, pattern: Option<&str>, decision: RuleDecision) -> PermissionRule {
    PermissionRule {
        tool: tool.to_owned(),
        pattern: pattern.map(ToOwned::to_owned),
        decision,
        source: RuleSource::Session,
    }
}

fn perm_ctx(rules: Vec<PermissionRule>) -> PermissionContext {
    PermissionContext {
        rules,
        can_prompt_user: true,
        ..Default::default()
    }
}

// ────────────────────────────────────────────────────────────
// WebFetch：权限
// ────────────────────────────────────────────────────────────

#[test]
fn 白名单文档站免确认() {
    let r = fetch_tool().check_permissions(
        &serde_json::json!({ "url": "https://docs.rs/tokio", "prompt": "看用法" }),
        &perm_ctx(vec![]),
    );
    assert!(
        matches!(
            r,
            PermissionResult::Allow {
                reason: DecisionReason::Preapproved { .. },
                ..
            }
        ),
        "查文档每次都弹窗，用户第三次就会去开全部允许 —— 那比白名单危险得多。实际：{r:?}"
    );
}

#[test]
fn 陌生域名要确认并给出域名级建议() {
    let r = fetch_tool().check_permissions(
        &serde_json::json!({ "url": "https://random-blog.example/a", "prompt": "读" }),
        &perm_ctx(vec![]),
    );
    let PermissionResult::Ask { suggestions, .. } = r else {
        panic!("陌生域名必须确认，实际：{r:?}");
    };
    // 建议必须是域名级的。整工具级的"总是允许 WebFetch"意味着以后可以抓
    // 任何站点 —— 用户在一个具体域名的弹窗上点"总是允许"绝不是这个意思。
    let riot_protocol::permission::PermissionUpdate::AddRule { pattern, .. } = &suggestions[0]
    else {
        panic!("建议应当是 AddRule：{suggestions:?}");
    };
    assert_eq!(pattern.as_deref(), Some("domain:random-blog.example"));
}

#[test]
fn 域名级_allow_规则生效() {
    let r = fetch_tool().check_permissions(
        &serde_json::json!({ "url": "https://blog.example/a", "prompt": "读" }),
        &perm_ctx(vec![rule(
            "WebFetch",
            Some("domain:blog.example"),
            RuleDecision::Allow,
        )]),
    );
    assert!(matches!(r, PermissionResult::Allow { .. }), "{r:?}");
}

#[test]
fn 域名级_deny_压过白名单() {
    // 用户明确禁掉一个域名之后，不该有任何路径能把它打开
    let r = fetch_tool().check_permissions(
        &serde_json::json!({ "url": "https://docs.rs/x", "prompt": "读" }),
        &perm_ctx(vec![rule(
            "WebFetch",
            Some("domain:docs.rs"),
            RuleDecision::Deny,
        )]),
    );
    assert!(matches!(r, PermissionResult::Deny { .. }), "{r:?}");
}

#[test]
fn 通配域名规则可用() {
    let r = fetch_tool().check_permissions(
        &serde_json::json!({ "url": "https://a.internal.corp/x", "prompt": "读" }),
        &perm_ctx(vec![rule(
            "WebFetch",
            Some("domain:*.corp"),
            RuleDecision::Deny,
        )]),
    );
    assert!(matches!(r, PermissionResult::Deny { .. }), "{r:?}");
}

#[test]
fn 非法_url_不表态() {
    // 表态成 Deny 的话模型会去申请权限，而正确的下一步是修 URL
    let r = fetch_tool().check_permissions(
        &serde_json::json!({ "url": "不是网址", "prompt": "读" }),
        &perm_ctx(vec![]),
    );
    assert_eq!(r, PermissionResult::Passthrough);
}

#[test]
fn 陌生域名的询问理由是同意请求而非规则() {
    // 这个理由不是给日志看的装饰 —— 决策链靠它区分"可被 bypass 压过的
    // 例行询问"和"用户明确要求问的"。写成 Rule 会让「全部放行」失效。
    let r = fetch_tool().check_permissions(
        &serde_json::json!({ "url": "https://www.rust-lang.org", "prompt": "读" }),
        &perm_ctx(vec![]),
    );
    assert!(
        matches!(
            r,
            PermissionResult::Ask {
                reason: DecisionReason::Consent { .. },
                ..
            }
        ),
        "实际：{r:?}"
    );
}

#[test]
fn 规则要求询问时理由仍是规则() {
    // 上一条的边界。用户亲手写的 ask 规则要保持对 bypass 免疫，
    // 靠的就是理由停留在 Rule。
    let r = fetch_tool().check_permissions(
        &serde_json::json!({ "url": "https://blog.example/a", "prompt": "读" }),
        &perm_ctx(vec![rule(
            "WebFetch",
            Some("domain:blog.example"),
            RuleDecision::Ask,
        )]),
    );
    assert!(
        matches!(
            r,
            PermissionResult::Ask {
                reason: DecisionReason::Rule { .. },
                ..
            }
        ),
        "实际：{r:?}"
    );
}

// ────────────────────────────────────────────────────────────
// WebFetch：与决策链的联动
//
// 上面那些只测了工具单独表态。真正决定弹不弹框的是整条链，
// 而这两者曾经不一致 —— 工具说 Ask，链就直接照办，「全部放行」被跳过。
// ────────────────────────────────────────────────────────────

/// 走完整条决策链，返回 allow / ask / deny。
fn chain_says(url: &str, mode: PermissionMode, rules: Vec<PermissionRule>) -> &'static str {
    let mut ctx = perm_ctx(rules.clone());
    ctx.mode = riot_protocol::permission::PermissionModeState(Some(mode));
    match riot_permissions::decide(
        &fetch_tool(),
        &serde_json::json!({ "url": url, "prompt": "读" }),
        &ctx,
        &riot_permissions::RuleSet::new(rules),
    ) {
        PermissionResult::Allow { .. } => "allow",
        PermissionResult::Ask { .. } => "ask",
        PermissionResult::Deny { .. } => "deny",
        PermissionResult::Passthrough => "passthrough",
    }
}

#[test]
fn 全部放行下抓取陌生域名不再询问() {
    assert_eq!(
        chain_says(
            "https://www.rust-lang.org",
            PermissionMode::BypassPermissions,
            vec![]
        ),
        "allow"
    );
}

#[test]
fn 全部放行不影响其他模式下的域名确认() {
    for mode in [
        PermissionMode::Default,
        PermissionMode::AcceptEdits,
        PermissionMode::Plan,
    ] {
        assert_eq!(
            chain_says("https://www.rust-lang.org", mode, vec![]),
            "ask",
            "{mode:?}"
        );
    }
}

#[test]
fn 全部放行压不过用户写的_deny_规则() {
    assert_eq!(
        chain_says(
            "https://blog.example/a",
            PermissionMode::BypassPermissions,
            vec![rule(
                "WebFetch",
                Some("domain:blog.example"),
                RuleDecision::Deny
            )],
        ),
        "deny"
    );
}

#[test]
fn 全部放行压不过用户写的_ask_规则() {
    assert_eq!(
        chain_says(
            "https://blog.example/a",
            PermissionMode::BypassPermissions,
            vec![rule(
                "WebFetch",
                Some("domain:blog.example"),
                RuleDecision::Ask
            )],
        ),
        "ask",
        "用户写下「问我一下」之后，切到全部放行不等于撤回它"
    );
}

// ────────────────────────────────────────────────────────────
// WebFetch：入参校验
// ────────────────────────────────────────────────────────────

#[tokio::test]
async fn 拒绝内网地址() {
    let h = harness(FakeWeb::new());
    let err = fetch_tool()
        .validate_input(
            &serde_json::json!({
                "url": "http://169.254.169.254/latest/meta-data/",
                "prompt": "读"
            }),
            &h.ctx,
        )
        .await
        .expect_err("云元数据地址必须拒绝");
    assert!(err.to_string().contains("内网"), "{err}");
}

#[tokio::test]
async fn 拒绝空_prompt() {
    let h = harness(FakeWeb::new());
    let err = fetch_tool()
        .validate_input(
            &serde_json::json!({ "url": "https://docs.rs/x", "prompt": "  " }),
            &h.ctx,
        )
        .await
        .expect_err("空 prompt 应当拒绝");
    assert!(err.to_string().contains("prompt"), "{err}");
}

// ────────────────────────────────────────────────────────────
// WebFetch：抓取
// ────────────────────────────────────────────────────────────

#[tokio::test]
async fn 抓取并转成_markdown() {
    let h = harness(FakeWeb::new().page(
        "https://docs.rs/x",
        "text/html",
        "<h1>标题</h1><p>正文内容</p><script>noise()</script>",
    ));

    let out = fetch_tool()
        .call(
            serde_json::json!({ "url": "https://docs.rs/x", "prompt": "读" }),
            h.ctx.clone(),
        )
        .await;

    let t = text_of(&out);
    assert!(t.contains("标题"), "{t}");
    assert!(t.contains("正文内容"), "{t}");
    assert!(!t.contains("noise()"), "script 内容不该出现：{t}");
    assert!(t.contains("来源"), "必须带上来源地址：{t}");
}

#[tokio::test]
async fn 没配辅助模型时给原文而不是失败() {
    // 拿不到摘要总比拿不到网页强
    let h = harness(FakeWeb::new().page("https://a.example/p", "text/html", "<p>原始正文</p>"));

    let out = fetch_tool()
        .call(
            serde_json::json!({ "url": "https://a.example/p", "prompt": "读" }),
            h.ctx.clone(),
        )
        .await;

    assert!(!out.is_error(), "没配辅助模型不该让整个工具失败：{out:?}");
    assert!(text_of(&out).contains("原始正文"));
}

#[tokio::test]
async fn 配了辅助模型就用蒸馏结果() {
    let h = harness(
        FakeWeb::new()
            .page("https://a.example/p", "text/html", "<p>很长的原始正文</p>")
            .with_distiller("提炼后的要点"),
    );

    let out = fetch_tool()
        .call(
            serde_json::json!({ "url": "https://a.example/p", "prompt": "要点" }),
            h.ctx.clone(),
        )
        .await;

    assert!(text_of(&out).contains("提炼后的要点"), "{}", text_of(&out));
}

#[tokio::test]
async fn 同源重定向自动跟随() {
    let h = harness(
        FakeWeb::new()
            .redirect("https://a.example/old", 301, "https://a.example/new")
            .page("https://a.example/new", "text/html", "<p>新页面</p>"),
    );

    let out = fetch_tool()
        .call(
            serde_json::json!({ "url": "https://a.example/old", "prompt": "读" }),
            h.ctx.clone(),
        )
        .await;

    assert!(text_of(&out).contains("新页面"), "{}", text_of(&out));
    assert_eq!(h.web.requested().len(), 2, "应当跟了一跳");
}

#[tokio::test]
async fn 跨站重定向不跟而是交回模型() {
    // 这条挂了，用户对一个域名的授权就变成了对全网的授权
    let h = harness(
        FakeWeb::new()
            .redirect("https://a.example/r", 302, "https://evil.example/x")
            .page("https://evil.example/x", "text/html", "<p>不该被抓到</p>"),
    );

    let out = fetch_tool()
        .call(
            serde_json::json!({ "url": "https://a.example/r", "prompt": "读" }),
            h.ctx.clone(),
        )
        .await;

    let t = text_of(&out);
    assert!(t.contains("evil.example"), "要把新地址交回模型：{t}");
    assert!(t.contains("再调一次"), "要让模型重新发起：{t}");
    assert!(!t.contains("不该被抓到"), "跨站目标不该被抓取：{t}");
    assert_eq!(
        h.web.requested(),
        vec!["https://a.example/r"],
        "跨站目标一个字节都不该请求"
    );
}

#[tokio::test]
async fn 重定向到内网地址被拦截() {
    let h = harness(FakeWeb::new().redirect(
        "https://a.example/r",
        302,
        "http://169.254.169.254/latest/meta-data/",
    ));

    let out = fetch_tool()
        .call(
            serde_json::json!({ "url": "https://a.example/r", "prompt": "读" }),
            h.ctx.clone(),
        )
        .await;

    assert!(out.is_error(), "重定向到内网必须失败：{out:?}");
    assert_eq!(h.web.requested().len(), 1, "内网地址不该被请求");
}

#[tokio::test]
async fn 重定向循环会终止() {
    // 每跳都重新计时，没有跳数上限的话工具会挂到用户手动中断
    let h = harness(
        FakeWeb::new()
            .redirect("https://a.example/x", 302, "https://a.example/y")
            .redirect("https://a.example/y", 302, "https://a.example/x"),
    );

    let out = fetch_tool()
        .call(
            serde_json::json!({ "url": "https://a.example/x", "prompt": "读" }),
            h.ctx.clone(),
        )
        .await;

    assert!(out.is_error(), "{out:?}");
    assert!(text_of(&out).contains("循环"), "{}", text_of(&out));
    assert!(
        h.web.requested().len() <= super::pipeline::MAX_REDIRECTS + 1,
        "跳数没有被限制：{}",
        h.web.requested().len()
    );
}

#[tokio::test]
async fn 缓存命中不再发请求() {
    let tool = fetch_tool();
    let h = harness(FakeWeb::new().page("https://a.example/p", "text/html", "<p>正文</p>"));
    let args = serde_json::json!({ "url": "https://a.example/p", "prompt": "读" });

    let _ = tool.call(args.clone(), h.ctx.clone()).await;
    let _ = tool.call(args.clone(), h.ctx.clone()).await;
    assert_eq!(h.web.requested().len(), 1, "第二次应当走缓存");

    // 过了 TTL 就要重新抓
    h.clock.advance(super::cache::DEFAULT_TTL_MS + 1);
    let _ = tool.call(args, h.ctx.clone()).await;
    assert_eq!(h.web.requested().len(), 2, "过期后应当重新抓");
}

#[tokio::test]
async fn 状态码错误给出可执行的建议() {
    let h = harness(FakeWeb::new().status("https://a.example/p", 403, "Forbidden"));

    let out = fetch_tool()
        .call(
            serde_json::json!({ "url": "https://a.example/p", "prompt": "读" }),
            h.ctx.clone(),
        )
        .await;

    let t = text_of(&out);
    assert!(out.is_error());
    // 只贴 "HTTP 403" 的话模型会换个 header 重试；要告诉它换条路
    assert!(t.contains("不要重试"), "{t}");
    assert!(t.contains("登录"), "{t}");
}

#[tokio::test]
async fn 没配联网能力时提示去设置() {
    let h = harness(FakeWeb::new()); // 没登记任何页面 → 404
    let out = fetch_tool()
        .call(
            serde_json::json!({ "url": "https://a.example/p", "prompt": "读" }),
            h.ctx.clone(),
        )
        .await;
    assert!(out.is_error(), "{out:?}");
}

// ────────────────────────────────────────────────────────────
// WebSearch
// ────────────────────────────────────────────────────────────

fn hits() -> Vec<SearchHit> {
    vec![
        SearchHit {
            title: "Tokio 文档".into(),
            url: "https://tokio.rs/x".into(),
            snippet: "异步运行时".into(),
            raw_content: None,
        },
        SearchHit {
            title: "某博客".into(),
            url: "https://blog.example/y".into(),
            snippet: "实践经验".into(),
            raw_content: None,
        },
    ]
}

#[tokio::test]
async fn 搜索返回可引用的链接列表() {
    let h = harness(FakeWeb::new().search_hits(hits()));
    let out = WebSearch
        .call(
            serde_json::json!({ "query": "tokio select" }),
            h.ctx.clone(),
        )
        .await;

    let t = text_of(&out);
    assert!(t.contains("[Tokio 文档](https://tokio.rs/x)"), "{t}");
    assert!(t.contains("异步运行时"), "{t}");
}

#[tokio::test]
async fn 搜索不会去抓取结果页() {
    // 自动抓取会绕开 WebFetch 的域名权限 —— 搜索结果可以指向任何站点
    let h = harness(FakeWeb::new().search_hits(hits()));
    let _ = WebSearch
        .call(serde_json::json!({ "query": "tokio" }), h.ctx.clone())
        .await;
    assert!(
        h.web.requested().is_empty(),
        "搜索阶段一个页面都不该抓：{:?}",
        h.web.requested()
    );
}

#[tokio::test]
async fn 搜索无结果时给出改进建议() {
    let h = harness(FakeWeb::new().search_hits(vec![]));
    let out = WebSearch
        .call(
            serde_json::json!({ "query": "不存在的东西" }),
            h.ctx.clone(),
        )
        .await;

    assert!(!out.is_error(), "没搜到不是失败：{out:?}");
    assert!(text_of(&out).contains("英文关键词"), "{}", text_of(&out));
}

#[tokio::test]
async fn 两种域名过滤不能同时给() {
    let h = harness(FakeWeb::new());
    let err = WebSearch
        .validate_input(
            &serde_json::json!({
                "query": "abc",
                "allowed_domains": ["a.com"],
                "blocked_domains": ["b.com"]
            }),
            &h.ctx,
        )
        .await
        .expect_err("语义矛盾的参数应当拒绝");
    assert!(err.to_string().contains("不能同时"), "{err}");
}

#[tokio::test]
async fn 搜索词太短会被拒() {
    let h = harness(FakeWeb::new());
    assert!(
        WebSearch
            .validate_input(&serde_json::json!({ "query": "a" }), &h.ctx)
            .await
            .is_err()
    );
}

#[test]
fn 搜索首次使用要确认() {
    let r = WebSearch.check_permissions(&serde_json::json!({ "query": "x" }), &perm_ctx(vec![]));
    assert!(matches!(r, PermissionResult::Ask { .. }), "{r:?}");
}

#[test]
fn 允许过之后不再确认() {
    let r = WebSearch.check_permissions(
        &serde_json::json!({ "query": "x" }),
        &perm_ctx(vec![rule("WebSearch", None, RuleDecision::Allow)]),
    );
    assert!(matches!(r, PermissionResult::Allow { .. }), "{r:?}");
}

#[test]
fn 全部放行下搜索不再确认() {
    // 和 WebFetch 同一个毛病：工具的 Ask 曾经让整条链跳过 bypass。
    let mut ctx = perm_ctx(vec![]);
    ctx.mode =
        riot_protocol::permission::PermissionModeState(Some(PermissionMode::BypassPermissions));
    let r = riot_permissions::decide(
        &WebSearch,
        &serde_json::json!({ "query": "x" }),
        &ctx,
        &riot_permissions::RuleSet::default(),
    );
    assert!(matches!(r, PermissionResult::Allow { .. }), "{r:?}");
}

#[test]
fn 提示词里写死当前年月() {
    // 不写的话模型会按知识截止日期构造搜索词，拿回一堆过时结果还深信不疑
    let ctx = riot_protocol::tool::PromptContext {
        cwd: "/w".into(),
        platform: "test".into(),
        sibling_tools: Vec::new(),
        today: "2026年8月".into(),
    };
    let p = WebSearch.prompt(&ctx);
    assert!(p.contains("2026年8月"), "{p}");
    assert!(p.contains("2026"), "{p}");
    // 分工要写清，否则模型会拿 WebSearch 当抓取工具反复调
    assert!(p.contains("WebFetch"), "{p}");
}
