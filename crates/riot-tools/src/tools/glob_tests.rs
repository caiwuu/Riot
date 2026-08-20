//! Glob 工具的测试。
//!
//! 跑的是**真遍历**：临时目录 + 真文件。以前这里断言的是拼给 ripgrep 的
//! argv（那时工具是 spawn 子进程的）—— 换成库实现之后那些断言没有了对象，
//! 而且它们本来就只能证明"参数拼对了"。
//!
//! 排序仍走注入的 fs（MemFs 记 mtime）：那部分是工具自己的逻辑，
//! 和遍历无关，用真文件写 mtime 反而不好控制。

// 建真目录、写真文件：测的就是在真实文件系统上的行为。
#![allow(clippy::disallowed_methods)]

use std::path::Path;
use std::sync::Arc;

use pretty_assertions::assert_eq;
use riot_protocol::tool::{Tool, ToolContext, ToolOutcome};
use tokio_util::sync::CancellationToken;

use super::Glob;
use super::fakeproc::FakeProc;
use super::memfs::{MemFileState, MemFs};

struct Harness {
    ctx: ToolContext,
    fs: Arc<MemFs>,
    _dir: tempfile::TempDir,
}

/// 建一棵小树。`files` 里的每个相对路径都会被真的创建出来。
fn harness(files: &[&str]) -> Harness {
    let dir = tempfile::tempdir().expect("临时目录");
    let root = dir.path();
    std::fs::write(root.join(".gitignore"), "ignored.rs\ntarget/\n").expect("写");
    for f in files {
        let p = root.join(f);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("目录");
        }
        std::fs::write(&p, "x").expect("写");
    }

    // MemFs 只服务两件事：`path` 参数的围栏解析，和排序要读的 mtime。
    let mut fs = MemFs::new().with_dir(root.to_path_buf());
    for f in files {
        let p = root.join(f);
        if let Some(parent) = p.parent() {
            fs = fs.with_dir(parent.to_path_buf());
        }
        fs = fs.with_file(p.clone(), "x");
    }
    let fs = Arc::new(fs);

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = ToolContext {
        session_id: riot_protocol::id::SessionId::from_raw("s1"),
        tool_use_id: riot_protocol::id::ToolUseId::from_raw("t1"),
        cwd: root.to_path_buf(),
        artifacts_dir: "/artifacts".into(),
        cancel: CancellationToken::new(),
        progress: riot_protocol::tool::ProgressSink::new(
            riot_protocol::id::ToolUseId::from_raw("t1"),
            tx,
        ),
        file_state: Arc::new(MemFileState::new()),
        fs: Arc::clone(&fs) as Arc<_>,
        proc: Arc::new(FakeProc::new()),
        web: Arc::new(riot_protocol::web::NoWeb),
        browser: Arc::new(riot_protocol::browser::NoBrowser),
        terminal: Arc::new(riot_protocol::terminal::NoTerminal),
        vision: Arc::new(riot_protocol::vision::NoVision),
        clock: Arc::new(crate::testing::FixedClock::default()),
    };
    Harness { ctx, fs, _dir: dir }
}

async fn glob(h: &Harness, args: serde_json::Value) -> ToolOutcome {
    if let Err(e) = Glob.validate_input(&args, &h.ctx).await {
        return ToolOutcome::failed(e.to_string());
    }
    Glob.call(args, h.ctx.clone()).await
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

/// 结果里的相对路径（去掉临时目录前缀，断言好读）。
///
/// 分隔符一律归一成 `/`：Windows 上工具输出的是 `\`，不归一的话下面每条
/// 断言都要写两份。
fn names(text: &str, root: &Path) -> Vec<String> {
    let prefix = root.to_string_lossy().into_owned();
    text.lines()
        .filter(|l| l.starts_with(&prefix))
        .map(|l| {
            l[prefix.len()..]
                .trim_start_matches(['/', '\\'])
                .replace('\\', "/")
        })
        .collect()
}

// ── 参数校验 ──────────────────────────────────────────

#[tokio::test]
async fn 空_pattern_给出可用写法() {
    let h = harness(&["a.rs"]);
    let o = glob(&h, serde_json::json!({ "pattern": "  " })).await;
    assert!(!is_ok(&o));
    assert!(text_of(&o).contains("**/*"), "{}", text_of(&o));
}

// ── 遍历 ──────────────────────────────────────────────

#[tokio::test]
async fn 按扩展名找文件() {
    let h = harness(&["a.rs", "src/b.rs", "c.txt"]);
    let o = glob(&h, serde_json::json!({ "pattern": "**/*.rs" })).await;
    let mut got = names(&text_of(&o), &h.ctx.cwd);
    got.sort();
    assert_eq!(got, vec!["a.rs", "src/b.rs"]);
}

#[tokio::test]
async fn 限定子目录() {
    let h = harness(&["a.rs", "src/b.rs"]);
    let o = glob(&h, serde_json::json!({ "pattern": "src/*.rs" })).await;
    assert_eq!(names(&text_of(&o), &h.ctx.cwd), vec!["src/b.rs"]);
}

#[tokio::test]
async fn 进点开头的目录() {
    // .github/workflows/ci.yml、.cargo/config.toml 都是用户会问起的文件。
    // 跳过隐藏目录的话，这个工具会显得时灵时不灵。
    let h = harness(&[".github/workflows/ci.yml"]);
    let o = glob(&h, serde_json::json!({ "pattern": "**/*.yml" })).await;
    assert_eq!(
        names(&text_of(&o), &h.ctx.cwd),
        vec![".github/workflows/ci.yml"]
    );
}

#[tokio::test]
async fn 不列出_git_内部() {
    // `**/*` 会把 .git 整个捞出来的话，答案会被 object 文件淹没。
    let h = harness(&["a.rs", ".git/HEAD", ".git/objects/ab/cdef"]);
    let o = glob(&h, serde_json::json!({ "pattern": "**/*" })).await;
    let got = names(&text_of(&o), &h.ctx.cwd);
    assert!(!got.iter().any(|p| p.starts_with(".git/")), "{got:?}");
    assert!(got.contains(&"a.rs".to_owned()), "{got:?}");
}

#[tokio::test]
async fn 没找到是成功不是失败() {
    // `[约束]` 报成失败会让模型换个参数把同一件事再做一遍。
    // "没有这样的文件"本身就是一个有用的答案。
    let h = harness(&["a.rs"]);
    let o = glob(&h, serde_json::json!({ "pattern": "**/*.zig" })).await;
    assert!(is_ok(&o), "无匹配必须是成功：{o:?}");
    let t = text_of(&o);
    assert!(t.contains("没有找到"), "{t}");
    assert!(t.contains(".gitignore"), "要说明为什么可能看不到：{t}");
}

#[tokio::test]
async fn 坏的_glob_给出例子() {
    let h = harness(&["a.rs"]);
    let o = glob(&h, serde_json::json!({ "pattern": "[" })).await;
    assert!(!is_ok(&o));
    assert!(text_of(&o).contains("**/*.rs"), "{}", text_of(&o));
}

// ── 排序与上限 ────────────────────────────────────────

#[tokio::test]
async fn 最近修改的排在前面() {
    // 结果被截断时，这个顺序决定了模型看到的是不是有用的那一批。
    let h = harness(&["old.rs", "mid.rs", "new.rs"]);
    let root = h.ctx.cwd.clone();
    h.fs.put(root.join("old.rs"), "x", 1_000);
    h.fs.put(root.join("mid.rs"), "x", 2_000);
    h.fs.put(root.join("new.rs"), "x", 3_000);

    let o = glob(&h, serde_json::json!({ "pattern": "**/*.rs" })).await;
    assert_eq!(
        names(&text_of(&o), &root),
        vec!["new.rs", "mid.rs", "old.rs"],
        "应该按 mtime 从新到旧"
    );
}

#[tokio::test]
async fn stat_不到的文件排在最后而不是让整次调用失败() {
    let h = harness(&["here.rs", "gone.rs"]);
    let root = h.ctx.cwd.clone();
    h.fs.put(root.join("here.rs"), "x", 5_000);
    // gone.rs 在真实磁盘上有、MemFs 里没记 mtime → 当作最旧。

    let o = glob(&h, serde_json::json!({ "pattern": "**/*.rs" })).await;
    assert!(is_ok(&o), "一个 stat 不到的文件不该让整次调用失败");
    assert_eq!(names(&text_of(&o), &root), vec!["here.rs", "gone.rs"]);
}

#[tokio::test]
async fn 结果过多时截断并说明() {
    let names_: Vec<String> = (0..350).map(|i| format!("f{i:03}.rs")).collect();
    let refs: Vec<&str> = names_.iter().map(String::as_str).collect();
    let h = harness(&refs);

    let o = glob(&h, serde_json::json!({ "pattern": "**/*.rs" })).await;
    let t = text_of(&o);
    assert_eq!(names(&t, &h.ctx.cwd).len(), 300, "上限 300 条");
    assert!(t.contains("共 350 个文件"), "要说清被截断了：{t}");
}

#[tokio::test]
async fn head_limit_限制条数() {
    let h = harness(&["a.rs", "b.rs", "c.rs"]);
    let o = glob(&h, serde_json::json!({ "pattern": "**/*.rs", "head_limit": 2 })).await;
    assert_eq!(names(&text_of(&o), &h.ctx.cwd).len(), 2);
}
