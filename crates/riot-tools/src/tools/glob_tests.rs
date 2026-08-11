//! Glob 工具的测试。
//!
//! 重头和 Grep 一样在 argv 构造，尤其是两个 `--glob` 的**先后顺序**：
//! 排除 .git 的那条必须在用户 pattern 之后，否则 `**/*` 会把整个 object
//! 库放回结果里。这个错误不会有任何报错，只会让答案悄悄变成垃圾。

use std::sync::Arc;

use riot_protocol::tool::{Tool, ToolContext, ToolOutcome};
use pretty_assertions::assert_eq;
use tokio_util::sync::CancellationToken;

use super::Glob;
use super::fakeproc::{FakeProc, Script};
use super::memfs::{MemFileState, MemFs};

struct Harness {
    proc: Arc<FakeProc>,
    ctx: ToolContext,
}

fn harness(proc: FakeProc, fs: MemFs) -> Harness {
    let proc = Arc::new(proc);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

    let ctx = ToolContext {
        session_id: riot_protocol::id::SessionId::from_raw("s1"),
        tool_use_id: riot_protocol::id::ToolUseId::from_raw("t1"),
        cwd: "/work".into(),
        cancel: CancellationToken::new(),
        progress: riot_protocol::tool::ProgressSink::new(
            riot_protocol::id::ToolUseId::from_raw("t1"),
            tx,
        ),
        file_state: Arc::new(MemFileState::new()),
        fs: Arc::new(fs),
        proc: Arc::clone(&proc) as Arc<_>,
        web: Arc::new(riot_protocol::web::NoWeb),
        browser: Arc::new(riot_protocol::browser::NoBrowser),
        clock: Arc::new(crate::testing::FixedClock::default()),
    };

    Harness { proc, ctx }
}

fn base_fs() -> MemFs {
    MemFs::new().with_dir("/work").with_dir("/work/src").with_dir("/etc")
}

/// FakeProc 按 args 最后一项索引脚本，而 Glob 的最后一项是搜索根。
fn proc_for(root: &str, script: Script) -> FakeProc {
    FakeProc::new().on(root, script)
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

fn args_of(h: &Harness) -> Vec<String> {
    h.proc.last_spec().expect("起过进程").args
}

// ── argv 构造 ─────────────────────────────────────────

#[tokio::test]
async fn 列文件而不是搜内容() {
    let h = harness(proc_for("/work", Script::ok("/work/a.rs\n")), base_fs());
    glob(&h, serde_json::json!({ "pattern": "**/*.rs" })).await;

    let args = args_of(&h);
    assert!(args.contains(&"--files".to_owned()), "少了 --files：{args:?}");
    assert!(
        args.contains(&"--no-config".to_owned()),
        "少了 --no-config，用户的 rg 配置会让结果因机器而异：{args:?}"
    );
}

#[tokio::test]
async fn 进点开头的目录() {
    // .github/workflows/ci.yml、.cargo/config.toml 都是用户会问起的文件。
    // 少了 --hidden 的话这个工具会显得时灵时不灵。
    let h = harness(proc_for("/work", Script::ok("/work/a.yml\n")), base_fs());
    glob(&h, serde_json::json!({ "pattern": "**/*.yml" })).await;

    assert!(args_of(&h).contains(&"--hidden".to_owned()));
}

#[tokio::test]
async fn 排除_git_的_glob_排在用户_pattern_之后() {
    // `[约束]` ripgrep 里后写的 glob 优先级更高。顺序反过来的话，
    // `**/*` 这种 pattern 会把 .git 整个放回结果 —— 没有任何报错，
    // 只是答案被 object 文件淹没。
    let h = harness(proc_for("/work", Script::ok("/work/a\n")), base_fs());
    glob(&h, serde_json::json!({ "pattern": "**/*" })).await;

    let args = args_of(&h);
    let user = args.iter().position(|a| a == "**/*").expect("用户 pattern 在");
    let exclude = args.iter().position(|a| a == "!.git/").expect("排除 .git 在");
    assert!(
        exclude > user,
        "排除 .git 必须排在用户 pattern 之后，否则会被覆盖：{args:?}"
    );
}

#[tokio::test]
async fn 路径参数在双横线之后() {
    // 搜索根可能以 `-` 开头，不隔开会被当成 flag
    let h = harness(proc_for("/work", Script::ok("")), base_fs());
    glob(&h, serde_json::json!({ "pattern": "**/*" })).await;

    let args = args_of(&h);
    let dashdash = args.iter().position(|a| a == "--").expect("有 --");
    assert_eq!(args.last().map(String::as_str), Some("/work"));
    assert!(dashdash < args.len() - 1);
}

// ── 结果处理 ───────────────────────────────────────────

#[tokio::test]
async fn 没找到是成功不是失败() {
    // `[约束]` 报成失败会让模型换个参数把同一件事再做一遍。
    // "没有这样的文件"本身就是一个有用的答案。
    let h = harness(proc_for("/work", Script::fail(1, "")), base_fs());
    let out = glob(&h, serde_json::json!({ "pattern": "**/*.zig" })).await;

    assert!(is_ok(&out), "无匹配必须是成功：{out:?}");
    let t = text_of(&out);
    assert!(t.contains("没有找到"), "{t}");
    assert!(t.contains(".gitignore"), "要说明为什么可能看不到：{t}");
}

#[tokio::test]
async fn rg_出错才是失败() {
    let h = harness(proc_for("/work", Script::fail(2, "regex parse error")), base_fs());
    let out = glob(&h, serde_json::json!({ "pattern": "**/*.rs" })).await;

    assert!(!is_ok(&out));
    assert!(text_of(&out).contains("regex parse error"));
}

#[tokio::test]
async fn 最近修改的排在前面() {
    // 结果被截断时，这个顺序决定了模型看到的是不是有用的那一批
    let fs = base_fs()
        .with_file("/work/old.rs", "x")
        .with_file("/work/new.rs", "x")
        .with_file("/work/mid.rs", "x");
    fs.put("/work/old.rs", "x", 1_000);
    fs.put("/work/mid.rs", "x", 2_000);
    fs.put("/work/new.rs", "x", 3_000);

    let h = harness(
        proc_for("/work", Script::ok("/work/old.rs\n/work/new.rs\n/work/mid.rs\n")),
        fs,
    );
    let out = glob(&h, serde_json::json!({ "pattern": "**/*.rs" })).await;

    assert_eq!(
        text_of(&out),
        "/work/new.rs\n/work/mid.rs\n/work/old.rs",
        "应该按 mtime 从新到旧"
    );
}

#[tokio::test]
async fn stat_不到的文件排在最后而不是让整次调用失败() {
    let fs = base_fs().with_file("/work/here.rs", "x");
    fs.put("/work/here.rs", "x", 5_000);

    let h = harness(
        proc_for("/work", Script::ok("/work/ghost.rs\n/work/here.rs\n")),
        fs,
    );
    let out = glob(&h, serde_json::json!({ "pattern": "**/*.rs" })).await;

    assert!(is_ok(&out), "一个文件 stat 不到不该毁掉整次查找");
    assert_eq!(text_of(&out), "/work/here.rs\n/work/ghost.rs");
}

#[tokio::test]
async fn 同一时间的文件按路径稳定排序() {
    // 生成代码、git checkout 会让一批文件共享同一个 mtime。
    // 不兜底的话同样的调用两次给出不同顺序。
    let fs = base_fs()
        .with_file("/work/b.rs", "x")
        .with_file("/work/a.rs", "x");
    fs.put("/work/b.rs", "x", 7_000);
    fs.put("/work/a.rs", "x", 7_000);

    let h = harness(proc_for("/work", Script::ok("/work/b.rs\n/work/a.rs\n")), fs);
    let out = glob(&h, serde_json::json!({ "pattern": "**/*.rs" })).await;

    assert_eq!(text_of(&out), "/work/a.rs\n/work/b.rs");
}

#[tokio::test]
async fn 结果过多时截断并说明怎么缩小() {
    let listing: String = (0..50)
        .map(|i| format!("/work/f{i}.rs\n"))
        .collect::<Vec<_>>()
        .join("");
    let h = harness(proc_for("/work", Script::ok(&listing)), base_fs());

    let out = glob(&h, serde_json::json!({ "pattern": "**/*.rs", "head_limit": 5 })).await;
    let t = text_of(&out);

    assert_eq!(t.lines().filter(|l| l.starts_with("/work/")).count(), 5);
    assert!(t.contains("共 50 个文件"), "{t}");
    assert!(t.contains("`pattern` 更具体"), "要告诉模型下一步怎么做：{t}");
}

// ── 参数校验 ───────────────────────────────────────────

#[tokio::test]
async fn 项目目录之外也能搜() {
    // 曾经断言"必须拒绝"。边界撤掉了 —— 在隔壁仓库里找文件是正当需求。
    let h = harness(proc_for("/other", Script::ok("/other/a.rs")), base_fs().with_dir("/other"));
    let out = glob(&h, serde_json::json!({ "pattern": "**/*.rs", "path": "/other" })).await;

    assert!(is_ok(&out), "{}", text_of(&out));
}

#[tokio::test]
async fn 空_pattern_被拒并给出替代写法() {
    let h = harness(proc_for("/work", Script::ok("")), base_fs());
    let out = glob(&h, serde_json::json!({ "pattern": "  " })).await;

    assert!(!is_ok(&out));
    assert!(text_of(&out).contains("**/*"), "光说不行还得说怎么做");
}

#[tokio::test]
async fn 认错参数时指向_grep() {
    // 模型最容易犯的错是拿 Glob 当 Grep 用
    let h = harness(proc_for("/work", Script::ok("")), base_fs());
    let out = glob(
        &h,
        serde_json::json!({ "pattern": "**/*.rs", "output_mode": "content" }),
    )
    .await;

    assert!(!is_ok(&out));
    assert!(text_of(&out).contains("Grep"), "{}", text_of(&out));
}
