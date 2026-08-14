//! Grep 工具的测试。
//!
//! 跑的是**真搜索**：临时目录 + 真文件。以前这里断言的是拼给 ripgrep 的
//! argv（那时工具是 spawn 子进程的），换成库实现之后那些断言连编都编不过，
//! 而且它们本来就只能证明"参数拼对了"，证明不了搜出来的东西对不对。
//!
//! 路径解析仍走注入的 fs（MemFs）—— 围栏检查是它的职责，和搜索无关。

// 建真目录、写真文件：测的就是在真实文件系统上的行为。
#![allow(clippy::disallowed_methods)]

use std::path::Path;
use std::sync::Arc;

use pretty_assertions::assert_eq;
use riot_protocol::tool::{Tool, ToolContext, ToolOutcome};
use tokio_util::sync::CancellationToken;

use super::Grep;
use super::fakeproc::FakeProc;
use super::memfs::{MemFileState, MemFs};

struct Harness {
    ctx: ToolContext,
    /// 临时目录活着，测试结束才清掉。
    _dir: tempfile::TempDir,
}

/// 建一棵小树：
/// ```text
/// a.rs         fn foo() {}  / let x = 1; / foo();
/// b.txt        foo in text
/// ignored.rs   fn foo() {}      （被 .gitignore 掉）
/// src/c.rs     // FOO here
/// ```
fn harness() -> Harness {
    let dir = tempfile::tempdir().expect("临时目录");
    let p = dir.path();
    std::fs::write(p.join(".gitignore"), "ignored.rs\n").expect("写");
    std::fs::write(p.join("a.rs"), "fn foo() {}\nlet x = 1;\nfoo();\n").expect("写");
    std::fs::write(p.join("b.txt"), "foo in text\n").expect("写");
    std::fs::write(p.join("ignored.rs"), "fn foo() {}\n").expect("写");
    std::fs::create_dir_all(p.join("src")).expect("目录");
    std::fs::write(p.join("src/c.rs"), "// FOO here\n").expect("写");

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = ToolContext {
        session_id: riot_protocol::id::SessionId::from_raw("s1"),
        tool_use_id: riot_protocol::id::ToolUseId::from_raw("t1"),
        cwd: p.to_path_buf(),
        artifacts_dir: "/artifacts".into(),
        cancel: CancellationToken::new(),
        progress: riot_protocol::tool::ProgressSink::new(
            riot_protocol::id::ToolUseId::from_raw("t1"),
            tx,
        ),
        file_state: Arc::new(MemFileState::new()),
        // 只用于 `path` 参数的围栏解析；搜索本身走真实文件系统。
        fs: Arc::new(MemFs::new().with_dir(p.to_path_buf()).with_dir(p.join("src"))),
        proc: Arc::new(FakeProc::new()),
        web: Arc::new(riot_protocol::web::NoWeb),
        browser: Arc::new(riot_protocol::browser::NoBrowser),
        vision: Arc::new(riot_protocol::vision::NoVision),
        clock: Arc::new(crate::testing::FixedClock::default()),
    };
    Harness { ctx, _dir: dir }
}

async fn grep(h: &Harness, args: serde_json::Value) -> ToolOutcome {
    if let Err(e) = Grep.validate_input(&args, &h.ctx).await {
        return ToolOutcome::failed(e.to_string());
    }
    Grep.call(args, h.ctx.clone()).await
}

fn text_of(o: &ToolOutcome) -> String {
    match o {
        ToolOutcome::Ok { model_content, .. } => match model_content {
            riot_protocol::message::ToolResultContent::Text { text } => text.clone(),
            other => panic!("非文本结果：{other:?}"),
        },
        ToolOutcome::Failed { error_for_model, .. } => error_for_model.clone(),
        ToolOutcome::Cancelled => "<cancelled>".into(),
    }
}

fn is_ok(o: &ToolOutcome) -> bool {
    matches!(o, ToolOutcome::Ok { .. })
}

/// 输出里出现的文件名（去掉目录前缀，断言好读）。
fn names(text: &str, root: &Path) -> Vec<String> {
    text.lines()
        .filter(|l| l.starts_with(root.to_string_lossy().as_ref()))
        .map(|l| l.trim_start_matches(root.to_string_lossy().as_ref()).trim_start_matches('/').to_owned())
        .collect()
}

// ── 参数校验 ──────────────────────────────────────────

#[tokio::test]
async fn 空_pattern_指路到_glob() {
    let h = harness();
    let o = grep(&h, serde_json::json!({ "pattern": "" })).await;
    assert!(!is_ok(&o));
    assert!(text_of(&o).contains("Glob"), "{}", text_of(&o));
}

#[tokio::test]
async fn 坏正则当场拒掉并教怎么转义() {
    // 让搜索引擎去报错的话，模型收到的是一段 regex 内部诊断。
    let h = harness();
    let o = grep(&h, serde_json::json!({ "pattern": "foo(" })).await;
    assert!(!is_ok(&o));
    let t = text_of(&o);
    assert!(t.contains("转义"), "{t}");
}

#[tokio::test]
async fn 上下文行数只在_content_模式有意义() {
    let h = harness();
    let o = grep(
        &h,
        serde_json::json!({ "pattern": "foo", "output_mode": "count", "context_lines": 2 }),
    )
    .await;
    assert!(!is_ok(&o));
    assert!(text_of(&o).contains("context_lines"));
}

#[tokio::test]
async fn 未知参数列出可用的() {
    let h = harness();
    let o = grep(&h, serde_json::json!({ "pattern": "foo", "regex": true })).await;
    assert!(!is_ok(&o));
    let t = text_of(&o);
    assert!(t.contains("output_mode") && t.contains("head_limit"), "{t}");
}

// ── 搜索结果 ──────────────────────────────────────────

#[tokio::test]
async fn content_模式给出路径行号和内容() {
    let h = harness();
    let o = grep(&h, serde_json::json!({ "pattern": "let x" })).await;
    assert!(is_ok(&o));
    let t = text_of(&o);
    assert!(t.contains("a.rs:2:let x = 1;"), "{t}");
}

#[tokio::test]
async fn 上下文行跟着出来() {
    let h = harness();
    let o = grep(
        &h,
        serde_json::json!({ "pattern": "let x", "context_lines": 1 }),
    )
    .await;
    let t = text_of(&o);
    assert!(t.contains("-1-fn foo"), "前一行要在：{t}");
    assert!(t.contains(":2:let x"), "匹配行用冒号：{t}");
    assert!(t.contains("-3-foo();"), "后一行要在：{t}");
}

#[tokio::test]
async fn 三种输出模式() {
    let h = harness();
    let root = h.ctx.cwd.clone();

    let files = grep(
        &h,
        serde_json::json!({ "pattern": "foo", "output_mode": "files_with_matches" }),
    )
    .await;
    assert_eq!(names(&text_of(&files), &root), vec!["a.rs", "b.txt"]);

    let counted = grep(
        &h,
        serde_json::json!({ "pattern": "foo", "output_mode": "count" }),
    )
    .await;
    let t = text_of(&counted);
    assert!(t.contains("a.rs:2"), "a.rs 里两处：{t}");
}

#[tokio::test]
async fn glob_过滤文件() {
    let h = harness();
    let o = grep(
        &h,
        serde_json::json!({ "pattern": "foo", "glob": "*.txt", "output_mode": "files_with_matches" }),
    )
    .await;
    assert_eq!(names(&text_of(&o), &h.ctx.cwd), vec!["b.txt"]);
}

#[tokio::test]
async fn 大小写开关() {
    let h = harness();
    let sensitive = grep(
        &h,
        serde_json::json!({ "pattern": "FOO", "output_mode": "files_with_matches" }),
    )
    .await;
    assert_eq!(names(&text_of(&sensitive), &h.ctx.cwd), vec!["src/c.rs"]);

    let insensitive = grep(
        &h,
        serde_json::json!({
            "pattern": "FOO", "case_insensitive": true, "output_mode": "files_with_matches"
        }),
    )
    .await;
    assert_eq!(
        names(&text_of(&insensitive), &h.ctx.cwd),
        vec!["a.rs", "b.txt", "src/c.rs"]
    );
}

#[tokio::test]
async fn gitignore_掉的文件不搜() {
    // 但显式 glob 点名时会搜 —— 那条语义在 search 模块里有测试。
    let h = harness();
    let o = grep(
        &h,
        serde_json::json!({ "pattern": "fn foo", "output_mode": "files_with_matches" }),
    )
    .await;
    let got = names(&text_of(&o), &h.ctx.cwd);
    assert_eq!(got, vec!["a.rs"], "ignored.rs 不该在里面：{got:?}");
}

#[tokio::test]
async fn 没匹配不算失败_并提醒_gitignore() {
    // `[约束]` 报成失败的话模型会去调参数重试，而正确的下一步是换个词。
    let h = harness();
    let o = grep(&h, serde_json::json!({ "pattern": "绝不存在的词" })).await;
    assert!(is_ok(&o), "没搜到是有效答案");
    let t = text_of(&o);
    assert!(t.contains("没有找到"), "{t}");
    assert!(t.contains("gitignore"), "要提醒忽略规则：{t}");
}

#[tokio::test]
async fn 正则里的_shell_元字符只是普通字符() {
    // 没有子进程，所以不存在转义问题。搜一段带 `$(` `;` 的字面量。
    let h = harness();
    std::fs::write(h.ctx.cwd.join("d.sh"), "echo $(date); ls\n").expect("写");
    let o = grep(
        &h,
        serde_json::json!({ "pattern": r"\$\(date\); ls", "output_mode": "files_with_matches" }),
    )
    .await;
    assert_eq!(names(&text_of(&o), &h.ctx.cwd), vec!["d.sh"]);
}

// ── 结果上限 ──────────────────────────────────────────

#[tokio::test]
async fn head_limit_限制条数并说明() {
    let h = harness();
    for i in 0..20 {
        std::fs::write(h.ctx.cwd.join(format!("m{i}.txt")), "needle\n").expect("写");
    }
    let o = grep(
        &h,
        serde_json::json!({ "pattern": "needle", "head_limit": 3 }),
    )
    .await;
    let t = text_of(&o);
    let lines: Vec<&str> = t.lines().filter(|l| l.contains("needle")).collect();
    assert_eq!(lines.len(), 3, "只要 3 条：{t}");
    assert!(t.contains("共 20 条结果"), "要说清被截断了：{t}");
}

#[tokio::test]
async fn 结果不多时不加提示() {
    let h = harness();
    let o = grep(&h, serde_json::json!({ "pattern": "let x" })).await;
    assert!(!text_of(&o).contains("system-reminder"), "别加多余的噪音");
}

#[tokio::test]
async fn 指定子目录只搜那里() {
    let h = harness();
    let o = grep(
        &h,
        serde_json::json!({
            "pattern": "foo",
            "path": h.ctx.cwd.join("src").to_string_lossy(),
            "case_insensitive": true,
            "output_mode": "files_with_matches"
        }),
    )
    .await;
    assert_eq!(names(&text_of(&o), &h.ctx.cwd), vec!["src/c.rs"]);
}
