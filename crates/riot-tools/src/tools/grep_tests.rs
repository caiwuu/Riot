//! Grep 工具的测试。
//!
//! 重头是 argv 构造。那串参数决定了搜索的语义和安全性，但它不出现在任何
//! 输出里 —— 改错了不会有报错，只会让结果悄悄变得不对（少了 `--no-config`
//! 时依赖用户的环境，少了 `-e` 时以 `-` 开头的搜索词变成 flag）。

use std::sync::Arc;

use riot_protocol::tool::{Tool, ToolContext, ToolOutcome};
use pretty_assertions::assert_eq;
use tokio_util::sync::CancellationToken;

use super::Grep;
use super::fakeproc::{FakeProc, Script};
use super::memfs::{MemFileState, MemFs};

struct Harness {
    proc: Arc<FakeProc>,
    ctx: ToolContext,
}

fn harness(proc: FakeProc) -> Harness {
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
        fs: Arc::new(
            MemFs::new()
                .with_dir("/work")
                .with_dir("/work/src")
                .with_dir("/etc"),
        ),
        proc: Arc::clone(&proc) as Arc<_>,
        web: Arc::new(riot_protocol::web::NoWeb),
        browser: Arc::new(riot_protocol::browser::NoBrowser),
        clock: Arc::new(crate::testing::FixedClock::default()),
    };

    Harness { proc, ctx }
}

/// FakeProc 按 args 最后一项索引脚本，而 Grep 的最后一项是搜索根。
fn proc_for(root: &str, script: Script) -> FakeProc {
    FakeProc::new().on(root, script)
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
        ToolOutcome::Failed {
            error_for_model, ..
        } => error_for_model.clone(),
        ToolOutcome::Cancelled => "<cancelled>".into(),
    }
}

fn is_ok(o: &ToolOutcome) -> bool {
    matches!(o, ToolOutcome::Ok { .. })
}

fn args_of(h: &Harness) -> Vec<String> {
    h.proc.last_spec().expect("起过进程").args
}

/// 找出某个 flag 后面跟的值。
fn value_after(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
}

// ── argv 构造 ─────────────────────────────────────────

#[tokio::test]
async fn 忽略用户的_ripgrep_配置() {
    // `[约束]` 用户的 RIPGREP_CONFIG_PATH 里可能有 --smart-case、--hidden，
    // 那会让同一次搜索在不同机器上给出不同结果，而模型看不到那份配置。
    let h = harness(proc_for("/work", Script::ok("")));
    grep(&h, serde_json::json!({ "pattern": "foo" })).await;

    assert!(args_of(&h).contains(&"--no-config".to_owned()));
}

#[tokio::test]
async fn 关掉颜色输出() {
    let h = harness(proc_for("/work", Script::ok("")));
    grep(&h, serde_json::json!({ "pattern": "foo" })).await;

    assert!(args_of(&h).iter().any(|a| a == "--color=never"));
}

#[tokio::test]
async fn pattern_走_e_而不是位置参数() {
    // `[约束]` 直接当位置参数的话，搜 `--force` 这种词会被 rg 当成 flag。
    // 结果不是报错就是激活一个碰巧存在的开关。
    let h = harness(proc_for("/work", Script::ok("")));
    grep(&h, serde_json::json!({ "pattern": "--force" })).await;

    let a = args_of(&h);
    assert_eq!(value_after(&a, "-e").as_deref(), Some("--force"));
}

#[tokio::test]
async fn 路径前有双横线分隔() {
    let h = harness(proc_for("/work", Script::ok("")));
    grep(&h, serde_json::json!({ "pattern": "foo" })).await;

    let a = args_of(&h);
    let dash = a.iter().position(|x| x == "--").expect("要有 -- 分隔符");
    assert_eq!(a.last().map(String::as_str), Some("/work"));
    assert!(dash < a.len() - 1, "-- 必须在路径之前");
}

#[tokio::test]
async fn shell_元字符原样传递不做转义() {
    // 走的是 argv 不是 shell，所以这些字符只是普通字符。
    // 这个测试守的是"以后有人改成拼 shell 命令"这件事 —— 那时候
    // 每个搜索词都得转义，漏一个就是命令注入。
    let nasty = r#"foo$(rm -rf /);bar`whoami`"#;
    let h = harness(proc_for("/work", Script::ok("")));
    grep(&h, serde_json::json!({ "pattern": nasty })).await;

    let spec = h.proc.last_spec().expect("起过进程");
    assert_eq!(spec.program, "rg", "不能经过 shell");
    assert_eq!(value_after(&spec.args, "-e").as_deref(), Some(nasty));
}

#[tokio::test]
async fn 输出模式映射到对应的_flag() {
    for (mode, flag) in [
        ("files_with_matches", "--files-with-matches"),
        ("count", "--count"),
    ] {
        let h = harness(proc_for("/work", Script::ok("")));
        grep(
            &h,
            serde_json::json!({ "pattern": "foo", "output_mode": mode }),
        )
        .await;
        assert!(args_of(&h).iter().any(|a| a == flag), "{mode} 少了 {flag}");
    }
}

#[tokio::test]
async fn content_模式带行号和文件名() {
    let h = harness(proc_for("/work", Script::ok("")));
    grep(&h, serde_json::json!({ "pattern": "foo" })).await;

    let a = args_of(&h);
    assert!(a.iter().any(|x| x == "--line-number"));
    assert!(a.iter().any(|x| x == "--with-filename"));
}

#[tokio::test]
async fn glob_和大小写选项被传下去() {
    let h = harness(proc_for("/work", Script::ok("")));
    grep(
        &h,
        serde_json::json!({ "pattern": "foo", "glob": "*.rs", "case_insensitive": true }),
    )
    .await;

    let a = args_of(&h);
    assert_eq!(value_after(&a, "--glob").as_deref(), Some("*.rs"));
    assert!(a.iter().any(|x| x == "--ignore-case"));
}

#[tokio::test]
async fn 上下文行数被传下去() {
    let h = harness(proc_for("/work", Script::ok("")));
    grep(
        &h,
        serde_json::json!({ "pattern": "foo", "context_lines": 3 }),
    )
    .await;

    assert_eq!(value_after(&args_of(&h), "--context").as_deref(), Some("3"));
}

// ── 退出码 ────────────────────────────────────────────

#[tokio::test]
async fn 没匹配到不算失败() {
    // `[约束]` rg 用退出码 1 表示"没找到"。当成错误的话，模型会
    // 反复调参数重试 —— 而正确的下一步是换个搜索词或者接受这个结果。
    let h = harness(proc_for("/work", Script::fail(1, "")));
    let out = grep(&h, serde_json::json!({ "pattern": "不存在的东西" })).await;

    assert!(is_ok(&out), "没匹配到是正常结果，不是失败");
    let t = text_of(&out);
    assert!(t.contains("没有找到"), "{t}");
    assert!(t.contains("不存在的东西"), "要回显搜索词：{t}");
}

#[tokio::test]
async fn 没匹配时提醒_gitignore() {
    // 最常见的困惑是"文件明明在那儿"。搜不到往往是被 gitignore 挡了。
    let h = harness(proc_for("/work", Script::fail(1, "")));
    let t = text_of(&grep(&h, serde_json::json!({ "pattern": "x" })).await);

    assert!(t.contains("gitignore"), "{t}");
}

#[tokio::test]
async fn 退出码_2_是真的失败() {
    let h = harness(proc_for(
        "/work",
        Script::fail(2, "rg: /work/x: 权限不足\n"),
    ));
    let out = grep(&h, serde_json::json!({ "pattern": "x" })).await;

    assert!(!is_ok(&out));
    assert!(text_of(&out).contains("权限不足"));
}

#[tokio::test]
async fn 找不到_rg_时给出替代方案() {
    let h = harness(FakeProc::new().default_script(Script::Spawn(std::io::ErrorKind::NotFound)));
    let t = text_of(&grep(&h, serde_json::json!({ "pattern": "x" })).await);

    assert!(t.contains("ripgrep"), "{t}");
    assert!(t.contains("grep"), "要给出退路：{t}");
}

#[tokio::test]
async fn 超时给出缩小范围的建议() {
    let h = harness(proc_for(
        "/work",
        Script::Timeout {
            stdout: String::new(),
            stderr: String::new(),
        },
    ));
    let t = text_of(&grep(&h, serde_json::json!({ "pattern": "x" })).await);

    assert!(t.contains("超过"), "{t}");
    assert!(t.contains("glob") || t.contains("缩小"), "{t}");
}

// ── 结果处理 ──────────────────────────────────────────

#[tokio::test]
async fn 返回匹配内容() {
    let h = harness(proc_for(
        "/work",
        Script::ok("src/a.rs:12:fn foo()\nsrc/b.rs:3:foo();\n"),
    ));
    let out = grep(&h, serde_json::json!({ "pattern": "foo" })).await;

    assert!(is_ok(&out));
    let t = text_of(&out);
    assert!(t.contains("src/a.rs:12"));
    assert!(t.contains("src/b.rs:3"));
}

#[tokio::test]
async fn 结果过多时截断并给出下一步() {
    let big: String = (0..2000)
        .map(|i| format!("src/f{i}.rs:1:匹配内容\n"))
        .collect();
    let h = harness(proc_for("/work", Script::ok(&big)));
    let t = text_of(&grep(&h, serde_json::json!({ "pattern": "x" })).await);

    assert!(t.contains("共 2000 条"), "要说明总数：{}", &t[t.len().saturating_sub(300)..]);
    assert!(t.contains("count"), "要给出摸清分布的办法");
    assert!(t.lines().count() < 600);
}

#[tokio::test]
async fn head_limit_限制条数() {
    let big: String = (0..100).map(|i| format!("f{i}.rs:1:x\n")).collect();
    let h = harness(proc_for("/work", Script::ok(&big)));
    let out = grep(
        &h,
        serde_json::json!({ "pattern": "x", "head_limit": 5 }),
    )
    .await;

    let t = text_of(&out);
    let hits = t.lines().filter(|l| l.contains(".rs:1:x")).count();
    assert_eq!(hits, 5);
}

#[tokio::test]
async fn 结果不多时不加提示() {
    let h = harness(proc_for("/work", Script::ok("a.rs:1:x\nb.rs:2:x\n")));
    let t = text_of(&grep(&h, serde_json::json!({ "pattern": "x" })).await);

    assert!(!t.contains("system-reminder"), "{t}");
}

// ── 搜索根 ────────────────────────────────────────────

#[tokio::test]
async fn 项目目录之外也能搜() {
    // 曾经断言"必须拒绝"。边界撤掉了 —— 在隔壁仓库里搜是正当需求。
    // 敏感目标（`~/.ssh` 之类）由 safety 层管，不在这一层。
    let h = harness(proc_for("/etc", Script::ok("/etc/hosts:1:root")));
    let out = grep(
        &h,
        serde_json::json!({ "pattern": "root", "path": "/etc" }),
    )
    .await;

    assert!(is_ok(&out), "{}", text_of(&out));
}

#[tokio::test]
async fn 相对路径解析到工作目录下() {
    let h = harness(proc_for("/work/src", Script::ok("")));
    grep(
        &h,
        serde_json::json!({ "pattern": "x", "path": "src" }),
    )
    .await;

    assert_eq!(args_of(&h).last().map(String::as_str), Some("/work/src"));
}

// ── 参数校验 ──────────────────────────────────────────

#[tokio::test]
async fn 非法正则在本地就被拦下() {
    // 让 rg 去报错的话，模型收到的是一段 regex 内部诊断，
    // 而且白等一次进程启动。
    let h = harness(FakeProc::new());
    let out = grep(&h, serde_json::json!({ "pattern": "foo(" })).await;

    assert!(!is_ok(&out));
    assert_eq!(h.proc.call_count(), 0, "不该起进程");
    let t = text_of(&out);
    assert!(t.contains("转义"), "要告诉模型怎么修：{t}");
}

#[tokio::test]
async fn 空_pattern_被拒并指向_glob() {
    let h = harness(FakeProc::new());
    let out = grep(&h, serde_json::json!({ "pattern": "" })).await;

    assert!(!is_ok(&out));
    assert!(text_of(&out).contains("Glob"), "要指出该用哪个工具");
}

#[tokio::test]
async fn 上下文行数用错模式时说明原因() {
    let h = harness(FakeProc::new());
    let out = grep(
        &h,
        serde_json::json!({ "pattern": "x", "output_mode": "count", "context_lines": 2 }),
    )
    .await;

    assert!(!is_ok(&out));
    assert!(text_of(&out).contains("content"), "{}", text_of(&out));
}

#[tokio::test]
async fn 未知的输出模式列出可选值() {
    let h = harness(FakeProc::new());
    let err = Grep
        .validate_input(
            &serde_json::json!({ "pattern": "x", "output_mode": "json" }),
            &h.ctx,
        )
        .await
        .expect_err("应该拒绝");

    let m = err.to_string();
    assert!(m.contains("files_with_matches"), "{m}");
    assert!(!m.contains("unknown variant"), "别贴原始错误：{m}");
}

#[tokio::test]
async fn 缺少_pattern_给出祈使句() {
    let h = harness(FakeProc::new());
    let err = Grep
        .validate_input(&serde_json::json!({}), &h.ctx)
        .await
        .expect_err("应该拒绝");

    let m = err.to_string();
    assert!(m.contains("pattern"), "{m}");
    assert!(!m.contains("missing field"), "{m}");
}

// ── 工具属性 ──────────────────────────────────────────

#[tokio::test]
async fn 只读且可并发() {
    let input = serde_json::json!({ "pattern": "x" });
    assert!(Grep.is_read_only(&input));
    assert!(Grep.is_concurrency_safe(&input));
}

#[tokio::test]
async fn 失败不级联() {
    // 搜索之间没有隐式依赖，一个搜不到不代表另一个没意义
    assert!(!Grep.cascades_on_failure());
}

#[tokio::test]
async fn 权限判定不自己表态() {
    // 自己返回 Allow 会绕过 safety 层 ——
    // `Grep -l "BEGIN PRIVATE KEY" ~/.ssh` 也是一次读取。
    let ctx = riot_protocol::permission::PermissionContext {
        mode: Default::default(),
        rules: vec![],
        sandboxed: false,
        can_prompt_user: true,
    };
    assert!(matches!(
        Grep.check_permissions(&serde_json::json!({ "pattern": "x" }), &ctx),
        riot_protocol::permission::PermissionResult::Passthrough
    ));
}
