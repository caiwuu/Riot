//! Read / Write / Edit 的测试。

use std::sync::Arc;

use riot_protocol::id::{SessionId, ToolUseId};
use riot_protocol::tool::{FileStateCache, FileView, Tool, ToolContext, ToolOutcome};
use pretty_assertions::assert_eq;
use tokio_util::sync::CancellationToken;

use super::memfs::{MemFileState, MemFs};
use super::{Edit, Read, Write};

struct Harness {
    fs: Arc<MemFs>,
    state: Arc<MemFileState>,
    ctx: ToolContext,
}

fn harness(fs: MemFs) -> Harness {
    let fs = Arc::new(fs);
    let state = Arc::new(MemFileState::new());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

    let ctx = ToolContext {
        session_id: SessionId::from_raw("s1"),
        tool_use_id: ToolUseId::from_raw("t1"),
        cwd: "/work".into(),
        artifacts_dir: "/artifacts".into(),
        cancel: CancellationToken::new(),
        progress: riot_protocol::tool::ProgressSink::new(ToolUseId::from_raw("t1"), tx),
        file_state: Arc::clone(&state) as Arc<_>,
        fs: Arc::clone(&fs) as Arc<_>,
        proc: Arc::new(super::super::testing::NullProc),
        web: Arc::new(riot_protocol::web::NoWeb),
        browser: Arc::new(riot_protocol::browser::NoBrowser),
        terminal: Arc::new(riot_protocol::terminal::NoTerminal),
        vision: Arc::new(riot_protocol::vision::NoVision),
        clock: Arc::new(super::super::testing::FixedClock::default()),
    };

    Harness { fs, state, ctx }
}

fn base_fs() -> MemFs {
    MemFs::new().with_dir("/work").with_dir("/work/src")
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

async fn read(h: &Harness, args: serde_json::Value) -> ToolOutcome {
    if let Err(e) = Read.validate_input(&args, &h.ctx).await {
        return ToolOutcome::failed(e.to_string());
    }
    Read.call(args, h.ctx.clone()).await
}

async fn write(h: &Harness, args: serde_json::Value) -> ToolOutcome {
    if let Err(e) = Write.validate_input(&args, &h.ctx).await {
        return ToolOutcome::failed(e.to_string());
    }
    Write.call(args, h.ctx.clone()).await
}

async fn edit(h: &Harness, args: serde_json::Value) -> ToolOutcome {
    if let Err(e) = Edit.validate_input(&args, &h.ctx).await {
        return ToolOutcome::failed(e.to_string());
    }
    Edit.call(args, h.ctx.clone()).await
}

// ── Read ──────────────────────────────────────────────

#[tokio::test]
async fn 读取文件带行号() {
    let h = harness(base_fs().with_file("/work/a.txt", "hello\nworld"));
    let out = read(&h, serde_json::json!({ "path": "a.txt" })).await;

    assert_eq!(text_of(&out), "     1\thello\n     2\tworld\n");
}

#[tokio::test]
async fn 相对路径基于_cwd() {
    let h = harness(base_fs().with_file("/work/src/main.rs", "fn main() {}"));
    assert!(is_ok(&read(&h, serde_json::json!({ "path": "src/main.rs" })).await));
}

#[tokio::test]
async fn 读取后写入状态缓存() {
    let h = harness(base_fs().with_file("/work/a.txt", "x"));
    read(&h, serde_json::json!({ "path": "a.txt" })).await;

    let s = h.state.get(std::path::Path::new("/work/a.txt")).expect("有缓存");
    assert_eq!(s.content, "x");
    assert_eq!(s.view, FileView::Full);
}

#[tokio::test]
async fn offset_从_1_开始() {
    let h = harness(base_fs().with_file("/work/a.txt", "a\nb\nc\nd"));
    let out = read(&h, serde_json::json!({ "path": "a.txt", "offset": 2, "limit": 2 })).await;

    let t = text_of(&out);
    assert!(t.contains("     2\tb"), "{t}");
    assert!(t.contains("     3\tc"), "{t}");
    assert!(!t.contains("\ta\n"), "不该包含第一行：{t}");
}

#[tokio::test]
async fn offset_为_0_被拒并说明从_1_开始() {
    let h = harness(base_fs().with_file("/work/a.txt", "a"));
    let out = read(&h, serde_json::json!({ "path": "a.txt", "offset": 0 })).await;

    assert!(!is_ok(&out));
    assert!(text_of(&out).contains("从 1 开始"), "{}", text_of(&out));
}

#[tokio::test]
async fn 部分读取标记为_partial_视图() {
    // 这是"模型没看到全文就动手改"的唯一防线
    let h = harness(base_fs().with_file("/work/a.txt", "a\nb\nc\nd"));
    read(&h, serde_json::json!({ "path": "a.txt", "offset": 2, "limit": 2 })).await;

    let s = h.state.get(std::path::Path::new("/work/a.txt")).expect("有缓存");
    assert!(
        matches!(s.view, FileView::Partial { .. }),
        "部分读取必须标 Partial，否则 Edit 会放行"
    );
}

#[tokio::test]
async fn 超过行数上限时截断并提示() {
    let big = (1..=3000).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
    let h = harness(base_fs().with_file("/work/big.txt", big));
    let out = read(&h, serde_json::json!({ "path": "big.txt" })).await;

    let t = text_of(&out);
    assert!(t.contains("共 3000 行"), "要告诉模型总行数：{}", &t[t.len() - 200..]);
    assert!(t.contains("offset=2001"), "要给出继续读的方法");

    let s = h.state.get(std::path::Path::new("/work/big.txt")).expect("有缓存");
    assert!(matches!(s.view, FileView::Partial { .. }));
}

#[tokio::test]
async fn 超长行被截断() {
    let long = "x".repeat(5000);
    let h = harness(base_fs().with_file("/work/min.js", &long));
    let out = read(&h, serde_json::json!({ "path": "min.js" })).await;

    let t = text_of(&out);
    assert!(t.contains("此行已截断"), "{}", &t[..200.min(t.len())]);
    assert!(t.len() < 5000, "整行塞给模型会挤掉上下文预算");
}

#[tokio::test]
async fn 二进制文件被拒绝() {
    let h = harness(base_fs().with_file("/work/a.bin", b"\x7FELF\0\0\0"));
    let out = read(&h, serde_json::json!({ "path": "a.bin" })).await;

    assert!(!is_ok(&out));
    assert!(text_of(&out).contains("二进制"));
}

// ── Read:图片 ─────────────────────────────────────

/// 换掉替身的图片能力。默认的 NoVision 只够测"没配"那条路。
fn with_vision(mut h: Harness, v: super::super::testing::FakeVision) -> Harness {
    h.ctx.vision = Arc::new(v);
    h
}

/// 模型自己能看图时，图片原样进结果，一个字节不差。
#[tokio::test]
async fn 能看图的模型读图片拿到图片块() {
    let png = b"\x89PNG\r\n\x1a\n-fake-bytes";
    let h = with_vision(
        harness(base_fs().with_file("/work/shot.png", png)),
        super::super::testing::FakeVision::Direct,
    );
    let out = read(&h, serde_json::json!({ "path": "shot.png" })).await;

    let ToolOutcome::Ok { model_content, .. } = &out else {
        panic!("应当成功：{}", text_of(&out));
    };
    let riot_protocol::message::ToolResultContent::Image { media_type, data, path } =
        model_content
    else {
        panic!("应当是图片内容块：{model_content:?}");
    };
    assert_eq!(media_type, "image/png");
    use base64::Engine as _;
    assert_eq!(
        base64::engine::general_purpose::STANDARD.decode(data).expect("合法 base64"),
        png,
        "假 PNG 解不开，压缩要原样放行 —— 字节不能有任何加工"
    );
    assert_eq!(
        path.as_deref(),
        Some(std::path::Path::new("/work/shot.png")),
        "要带上原文件路径，界面按它显示原图"
    );
    // `[约束]` 图片不进文件状态缓存。记了的话 Edit 会以为"读过全文"，
    // 给二进制开了一条被改写的路。
    assert!(
        h.state.get(std::path::Path::new("/work/shot.png")).is_none(),
        "图片不该写 file_state"
    );
}

/// 扩展名大小写不敏感，jpg 映射到 image/jpeg。
#[tokio::test]
async fn 大写的_jpg_扩展名也认() {
    let h = with_vision(
        harness(base_fs().with_file("/work/S.JPG", b"\xFF\xD8\xFF-fake")),
        super::super::testing::FakeVision::Direct,
    );
    let out = read(&h, serde_json::json!({ "path": "S.JPG" })).await;

    let ToolOutcome::Ok {
        model_content: riot_protocol::message::ToolResultContent::Image { media_type, .. },
        ..
    } = &out
    else {
        panic!("应当是图片内容块：{}", text_of(&out));
    };
    assert_eq!(media_type, "image/jpeg");
}

/// 看不了图的模型拿到转述文字，而且要说清描述的是哪个文件；
/// 图片本体留给界面贴出来。
///
/// `[约束]` 这条路必须**不返回纯图片**。返回了的话图片会在 provider 层被
/// 吞掉，模型只知道"有张图"，然后自己去 shell 里想办法。
#[tokio::test]
async fn 看不了图时读图片返回转述文字() {
    let h = with_vision(
        harness(base_fs().with_file("/work/shot.png", b"\x89PNG-fake")),
        super::super::testing::FakeVision::Describe("（转述）两栏布局，左边是导航".into()),
    );
    let out = read(&h, serde_json::json!({ "path": "shot.png" })).await;

    let ToolOutcome::Ok { model_content, .. } = &out else {
        panic!("应当成功：{}", text_of(&out));
    };
    let riot_protocol::message::ToolResultContent::DescribedImage { data, text, .. } =
        model_content
    else {
        panic!("应当是带转述的图片：{model_content:?}");
    };
    assert!(text.contains("shot.png"), "要说清描述的是哪个文件：{text}");
    assert!(text.contains("两栏布局"), "要带上转述内容：{text}");
    use base64::Engine as _;
    assert_eq!(
        base64::engine::general_purpose::STANDARD.decode(data).expect("合法 base64"),
        b"\x89PNG-fake",
        "图片本体要留给界面显示"
    );
}

/// 没配视觉兼容时要报"配一下"，不能落进普通的二进制报错。
///
/// `[约束]` 能力边界必须被表达出来（见 riot_protocol::vision 模块注释）:
/// 模型收到"二进制文件"只会换个歪路子接着试，收到"去设置里配"才会停。
#[tokio::test]
async fn 没配视觉兼容时读图片报未配置() {
    // harness 默认就是 NoVision
    let h = harness(base_fs().with_file("/work/shot.png", b"\x89PNG-fake"));
    let out = read(&h, serde_json::json!({ "path": "shot.png" })).await;

    assert!(!is_ok(&out));
    let t = text_of(&out);
    assert!(t.contains("视觉兼容"), "要指向设置项：{t}");
    assert!(!t.contains("二进制"), "不该落进二进制报错：{t}");
    assert!(t.contains("shell"), "要拦住 shell 解码的歪路：{t}");
}

/// 超大图片明确拒绝，并给出下一步。
#[tokio::test]
async fn 超大图片被拒并给出下一步() {
    let h = with_vision(
        harness(base_fs().with_file("/work/huge.png", vec![0u8; 3_500_001])),
        super::super::testing::FakeVision::Direct,
    );
    let out = read(&h, serde_json::json!({ "path": "huge.png" })).await;

    assert!(!is_ok(&out));
    let t = text_of(&out);
    assert!(t.contains("上限"), "{t}");
    assert!(t.contains("缩小"), "要给出下一步：{t}");
}

#[tokio::test]
async fn 非_utf8_不做_lossy_解码() {
    // lossy + 后续 Edit 全量写回 = 原始字节永久丢失，全程不报错
    let h = harness(base_fs().with_file("/work/latin.txt", b"caf\xE9"));
    let out = read(&h, serde_json::json!({ "path": "latin.txt" })).await;

    assert!(!is_ok(&out), "宁可拒绝也不能悄悄替换成 U+FFFD");
}

#[tokio::test]
async fn 空文件给出明确提示() {
    // 返回空字符串的话模型会以为工具坏了，然后反复重试
    let h = harness(base_fs().with_file("/work/empty.txt", ""));
    let out = read(&h, serde_json::json!({ "path": "empty.txt" })).await;

    assert!(is_ok(&out));
    assert!(text_of(&out).contains("文件为空"));
}

#[tokio::test]
async fn 读目录给出替代方案() {
    let h = harness(base_fs());
    let out = read(&h, serde_json::json!({ "path": "src" })).await;

    assert!(!is_ok(&out));
    let t = text_of(&out);
    assert!(t.contains("目录"), "{t}");
    assert!(t.contains("Glob") || t.contains("ls"), "要给出下一步：{t}");
}

#[tokio::test]
async fn 文件不存在给出可操作提示() {
    let h = harness(base_fs());
    let out = read(&h, serde_json::json!({ "path": "nope.txt" })).await;

    assert!(!is_ok(&out));
    assert!(text_of(&out).contains("不存在"));
}

#[tokio::test]
async fn 项目目录之外的文件可以读() {
    // 曾经这里断言"必须拒绝"。边界撤掉了 —— 参考隔壁仓库、读一份共享
    // 配置都是正当需求，工具层不该替权限层做判断。
    let h = harness(base_fs().with_file("/other/repo/README.md", "hello"));
    let out = read(&h, serde_json::json!({ "path": "/other/repo/README.md" })).await;

    assert!(is_ok(&out), "{}", text_of(&out));
    assert!(text_of(&out).contains("hello"));
}

#[tokio::test]
async fn 指向外面的_symlink_可以跟() {
    // 同上：解析后落在哪里不再是工具层的判断依据。
    let h = harness(
        base_fs()
            .with_file("/other/notes.md", "note")
            .with_link("/work/link", "/other/notes.md"),
    );
    let out = read(&h, serde_json::json!({ "path": "link" })).await;

    assert!(is_ok(&out), "{}", text_of(&out));
}

#[tokio::test]
async fn 形状可疑的路径仍然被拒() {
    // 边界没了，但畸形路径还是拦。和"在不在项目里"无关 ——
    // NUL 字节、NTFS 数据流这些本身就不该出现在正常路径里。
    let h = harness(base_fs());
    let out = read(&h, serde_json::json!({ "path": "/work/notes.txt:hidden" })).await;

    assert!(!is_ok(&out), "可疑构造必须拒绝：{}", text_of(&out));
}

#[tokio::test]
async fn 解析后的路径也要查形状() {
    // `[约束]` 字面路径干干净净，symlink 指向的目标却带别名构造。
    // 只对字面路径查形状的话这里会放过去 —— 而实际打开的是解析后那个。
    let h = harness(
        base_fs()
            .with_file("/work/NUL", "device-lookalike")
            .with_link("/work/innocent", "/work/NUL"),
    );
    let out = read(&h, serde_json::json!({ "path": "innocent" })).await;

    assert!(!is_ok(&out), "解析后的可疑路径必须拒绝：{}", text_of(&out));
}

#[tokio::test]
async fn read_的结果预算是无限的() {
    // 否则会产生 "Read → 结果落盘 → 模型又去 Read 那个文件" 的循环
    assert_eq!(
        Read.result_budget(),
        riot_protocol::tool::ResultBudget::Unlimited
    );
}

// ── Write ─────────────────────────────────────────────

#[tokio::test]
async fn 创建新文件不需要先读() {
    let h = harness(base_fs());
    let out = write(&h, serde_json::json!({ "path": "new.txt", "content": "hi" })).await;

    assert!(is_ok(&out), "{}", text_of(&out));
    assert_eq!(h.fs.text("/work/new.txt").as_deref(), Some("hi"));
}

#[tokio::test]
async fn 覆盖已存在的文件要先读() {
    // 要防的是模型基于半小时前的印象重写整个文件
    let h = harness(base_fs().with_file("/work/a.txt", "原有内容"));
    let out = write(&h, serde_json::json!({ "path": "a.txt", "content": "新内容" })).await;

    assert!(!is_ok(&out));
    assert!(text_of(&out).contains("Read"), "{}", text_of(&out));
    assert_eq!(
        h.fs.text("/work/a.txt").as_deref(),
        Some("原有内容"),
        "拒绝之后文件不能被改动"
    );
}

#[tokio::test]
async fn 读过之后可以覆盖() {
    let h = harness(base_fs().with_file("/work/a.txt", "old"));
    read(&h, serde_json::json!({ "path": "a.txt" })).await;

    let out = write(&h, serde_json::json!({ "path": "a.txt", "content": "new" })).await;
    assert!(is_ok(&out), "{}", text_of(&out));
    assert_eq!(h.fs.text("/work/a.txt").as_deref(), Some("new"));
}

#[tokio::test]
async fn 外部改动后拒绝覆盖() {
    let h = harness(base_fs().with_file("/work/a.txt", "v1"));
    read(&h, serde_json::json!({ "path": "a.txt" })).await;

    // 用户在编辑器里保存了
    h.fs.put("/work/a.txt", "用户的改动", 9999);

    let out = write(&h, serde_json::json!({ "path": "a.txt", "content": "v2" })).await;
    assert!(!is_ok(&out));
    assert_eq!(
        h.fs.text("/work/a.txt").as_deref(),
        Some("用户的改动"),
        "用户的改动不能被静默盖掉"
    );
}

#[tokio::test]
async fn 覆盖保持_crlf() {
    // 不保持的话，全量覆盖会让整个文件进 diff
    let h = harness(base_fs().with_file("/work/a.txt", b"a\r\nb\r\n"));
    read(&h, serde_json::json!({ "path": "a.txt" })).await;
    write(&h, serde_json::json!({ "path": "a.txt", "content": "x\ny\n" })).await;

    assert_eq!(h.fs.content("/work/a.txt").as_deref(), Some(&b"x\r\ny\r\n"[..]));
}

#[tokio::test]
async fn 覆盖保持_bom() {
    let h = harness(base_fs().with_file("/work/a.txt", b"\xEF\xBB\xBFold"));
    read(&h, serde_json::json!({ "path": "a.txt" })).await;
    write(&h, serde_json::json!({ "path": "a.txt", "content": "new" })).await;

    assert_eq!(
        h.fs.content("/work/a.txt").as_deref(),
        Some(&b"\xEF\xBB\xBFnew"[..])
    );
}

#[tokio::test]
async fn 写入后进缓存不用再读一次() {
    let h = harness(base_fs());
    write(&h, serde_json::json!({ "path": "new.txt", "content": "a\nb" })).await;

    let s = h.state.get(std::path::Path::new("/work/new.txt")).expect("有缓存");
    assert_eq!(s.content, "a\nb");
    assert_eq!(s.view, FileView::Full);

    // 直接 Edit 应该能成
    let out = edit(
        &h,
        serde_json::json!({ "path": "new.txt", "old_string": "a", "new_string": "z" }),
    )
    .await;
    assert!(is_ok(&out), "{}", text_of(&out));
}

#[tokio::test]
async fn 项目目录之外也能写() {
    // `[前提]` 工具层放行不等于没人管。写操作照样过权限闸：默认模式
    // 逐次询问，弹窗里显示解析后的绝对路径；敏感目标另有 safety 层。
    let h = harness(base_fs().with_dir("/other"));
    let out = write(&h, serde_json::json!({ "path": "/other/note.md", "content": "x" })).await;

    assert!(is_ok(&out), "{}", text_of(&out));
    assert_eq!(h.fs.content("/other/note.md").as_deref(), Some(&b"x"[..]));
}

#[tokio::test]
async fn 父目录不存在时给出可操作提示() {
    let h = harness(base_fs());
    let out = write(&h, serde_json::json!({ "path": "a/b/c.txt", "content": "x" })).await;

    assert!(!is_ok(&out));
    assert!(text_of(&out).contains("目录"), "{}", text_of(&out));
}

// ── Edit ──────────────────────────────────────────────

async fn edit_harness() -> Harness {
    let h = harness(base_fs().with_file("/work/a.rs", "fn a() {}\nfn b() {}\n"));
    read(&h, serde_json::json!({ "path": "a.rs" })).await;
    h
}

#[tokio::test]
async fn 正常替换() {
    let h = edit_harness().await;
    let out = edit(
        &h,
        serde_json::json!({ "path": "a.rs", "old_string": "fn a() {}", "new_string": "fn a() { x }" }),
    )
    .await;

    assert!(is_ok(&out), "{}", text_of(&out));
    assert_eq!(
        h.fs.text("/work/a.rs").as_deref(),
        Some("fn a() { x }\nfn b() {}\n")
    );
}

#[tokio::test]
async fn 没读过就改被拒() {
    let h = harness(base_fs().with_file("/work/a.rs", "x"));
    let out = edit(
        &h,
        serde_json::json!({ "path": "a.rs", "old_string": "x", "new_string": "y" }),
    )
    .await;

    assert!(!is_ok(&out));
    assert!(text_of(&out).contains("Read"));
    assert_eq!(h.fs.text("/work/a.rs").as_deref(), Some("x"));
}

#[tokio::test]
async fn 只读过一部分就改被拒() {
    // 模型以为看到了全文，把"这个函数只出现一次"当成事实
    let h = harness(base_fs().with_file("/work/a.rs", "a\nb\nc\nd"));
    read(&h, serde_json::json!({ "path": "a.rs", "offset": 1, "limit": 2 })).await;

    let out = edit(
        &h,
        serde_json::json!({ "path": "a.rs", "old_string": "a", "new_string": "z" }),
    )
    .await;

    assert!(!is_ok(&out));
    assert!(text_of(&out).contains("完整"), "{}", text_of(&out));
}

#[tokio::test]
async fn 多处匹配时拒绝而不是改第一处() {
    // 改错了不报错、不崩溃，只是代码悄悄坏掉
    let h = harness(base_fs().with_file("/work/a.rs", "let x = 1;\nlet x = 2;\n"));
    read(&h, serde_json::json!({ "path": "a.rs" })).await;

    let out = edit(
        &h,
        serde_json::json!({ "path": "a.rs", "old_string": "let x", "new_string": "let y" }),
    )
    .await;

    assert!(!is_ok(&out));
    let t = text_of(&out);
    assert!(t.contains("2 次"), "要说清出现了几次：{t}");
    assert!(t.contains("上下文") || t.contains("replace_all"), "要给出两条出路：{t}");
    assert_eq!(
        h.fs.text("/work/a.rs").as_deref(),
        Some("let x = 1;\nlet x = 2;\n"),
        "拒绝之后文件不能被改动"
    );
}

#[tokio::test]
async fn replace_all_替换全部() {
    let h = harness(base_fs().with_file("/work/a.rs", "let x = 1;\nlet x = 2;\n"));
    read(&h, serde_json::json!({ "path": "a.rs" })).await;

    let out = edit(
        &h,
        serde_json::json!({
            "path": "a.rs", "old_string": "let x", "new_string": "let y", "replace_all": true
        }),
    )
    .await;

    assert!(is_ok(&out), "{}", text_of(&out));
    assert_eq!(
        h.fs.text("/work/a.rs").as_deref(),
        Some("let y = 1;\nlet y = 2;\n")
    );
    assert!(text_of(&out).contains("2 处"));
}

#[tokio::test]
async fn 新旧完全相同被拒() {
    let h = edit_harness().await;
    let out = edit(
        &h,
        serde_json::json!({ "path": "a.rs", "old_string": "fn a", "new_string": "fn a" }),
    )
    .await;

    assert!(!is_ok(&out));
    assert!(text_of(&out).contains("没有任何效果"));
}

#[tokio::test]
async fn 空的_old_string_被拒并给出替代方案() {
    let h = edit_harness().await;
    let out = edit(
        &h,
        serde_json::json!({ "path": "a.rs", "old_string": "", "new_string": "x" }),
    )
    .await;

    assert!(!is_ok(&out));
    assert!(text_of(&out).contains("Write"), "{}", text_of(&out));
}

#[tokio::test]
async fn 外部改动后拒绝编辑() {
    let h = edit_harness().await;
    h.fs.put("/work/a.rs", "用户改过了", 9999);

    let out = edit(
        &h,
        serde_json::json!({ "path": "a.rs", "old_string": "fn a() {}", "new_string": "z" }),
    )
    .await;

    assert!(!is_ok(&out));
    assert_eq!(h.fs.text("/work/a.rs").as_deref(), Some("用户改过了"));
}

#[tokio::test]
async fn mtime_没变但内容变了仍然拒绝() {
    // mtime 的精度在 HFS+ 和部分 NFS 上只有 1 秒。用户在同一秒内保存文件，
    // mtime 完全可能不变 —— 那种情况下只查 mtime 等于没查。
    //
    // 这个测试守的是 verify_unchanged 里的内容比对。把它删掉之后
    // mtime 检查照样通过，用户的改动被静默覆盖。
    let h = edit_harness().await;

    let before = h.fs.metadata_mtime("/work/a.rs");
    h.fs.put("/work/a.rs", "用户在同一秒内改成了这样\n", before);

    let out = edit(
        &h,
        serde_json::json!({ "path": "a.rs", "old_string": "fn a() {}", "new_string": "z" }),
    )
    .await;

    assert!(!is_ok(&out), "mtime 骗过了检查，必须靠内容比对拦住");
    assert_eq!(
        h.fs.text("/work/a.rs").as_deref(),
        Some("用户在同一秒内改成了这样\n"),
        "用户的改动不能被覆盖"
    );
}

#[tokio::test]
async fn 带行号的_old_string_给出针对性提示() {
    // 最常见的失败：把 Read 输出的行号一起复制进来了
    let h = edit_harness().await;
    let out = edit(
        &h,
        serde_json::json!({
            "path": "a.rs",
            "old_string": "     1\tfn a() {}",
            "new_string": "fn a() { x }"
        }),
    )
    .await;

    assert!(!is_ok(&out));
    let t = text_of(&out);
    assert!(t.contains("行号"), "只说'没找到'的话模型会原样重试：{t}");
}

#[tokio::test]
async fn 缩进不一致给出针对性提示() {
    // 文件用 tab 缩进，模型给的是空格。这是最常见的一类匹配失败，
    // 而"没找到"这三个字对模型完全没用 —— 它会原样重试。
    let h = harness(base_fs().with_file("/work/a.rs", "\tindented();\n"));
    read(&h, serde_json::json!({ "path": "a.rs" })).await;

    let out = edit(
        &h,
        serde_json::json!({
            "path": "a.rs",
            "old_string": "    indented();",
            "new_string": "    other();"
        }),
    )
    .await;

    assert!(!is_ok(&out));
    let t = text_of(&out);
    assert!(
        t.contains("缩进"),
        "去掉首尾空白后能匹配上，要指出这一点：{t}"
    );
}

#[tokio::test]
async fn 编辑保持_crlf() {
    // 不保持的话，改一行会让整个文件每一行都进 diff
    let h = harness(base_fs().with_file("/work/a.txt", b"a\r\nb\r\nc\r\n"));
    read(&h, serde_json::json!({ "path": "a.txt" })).await;

    let out = edit(
        &h,
        serde_json::json!({ "path": "a.txt", "old_string": "b", "new_string": "B" }),
    )
    .await;

    assert!(is_ok(&out), "{}", text_of(&out));
    assert_eq!(
        h.fs.content("/work/a.txt").as_deref(),
        Some(&b"a\r\nB\r\nc\r\n"[..]),
        "只有 b 那一行该变"
    );
}

#[tokio::test]
async fn 编辑保持_bom() {
    let h = harness(base_fs().with_file("/work/a.txt", b"\xEF\xBB\xBFhello world"));
    read(&h, serde_json::json!({ "path": "a.txt" })).await;

    edit(
        &h,
        serde_json::json!({ "path": "a.txt", "old_string": "world", "new_string": "there" }),
    )
    .await;

    assert_eq!(
        h.fs.content("/work/a.txt").as_deref(),
        Some(&b"\xEF\xBB\xBFhello there"[..])
    );
}

#[tokio::test]
async fn 编辑后缓存更新() {
    let h = edit_harness().await;
    edit(
        &h,
        serde_json::json!({ "path": "a.rs", "old_string": "fn a() {}", "new_string": "fn z() {}" }),
    )
    .await;

    let s = h.state.get(std::path::Path::new("/work/a.rs")).expect("有缓存");
    assert!(s.content.contains("fn z()"), "缓存要跟上，否则连续两次 Edit 会失败");

    // 连续第二次 Edit
    let out = edit(
        &h,
        serde_json::json!({ "path": "a.rs", "old_string": "fn b() {}", "new_string": "fn c() {}" }),
    )
    .await;
    assert!(is_ok(&out), "{}", text_of(&out));
}

#[tokio::test]
async fn 编辑围栏外的文件被拒() {
    let h = harness(base_fs().with_dir("/etc").with_file("/etc/hosts", "127.0.0.1"));
    let out = edit(
        &h,
        serde_json::json!({ "path": "/etc/hosts", "old_string": "127", "new_string": "0" }),
    )
    .await;

    assert!(!is_ok(&out));
    assert_eq!(h.fs.text("/etc/hosts").as_deref(), Some("127.0.0.1"));
}

// ── call() 自己就是一道关口 ────────────────────────────
//
// 执行管线是 validate_input → 权限决策（可能弹窗等用户）→ call。
// 两处检查不是冗余，分工不同：
//
// - validate_input 让模型在弹窗之前就拿到反馈，省一轮往返；
// - call 是最后一道关口，防的是弹窗那段时间里发生的变化。
//
// 下面这组测试**绕过 validate_input 直接调 call**。少了它们，把 call
// 里的检查整段删掉不会有任何测试失败 —— 而那正是唯一真正拦得住
// TOCTOU 的地方。

#[tokio::test]
async fn write_的_call_独立拦住先读后写() {
    let h = harness(base_fs().with_file("/work/a.txt", "原有内容"));

    let out = Write
        .call(
            serde_json::json!({ "path": "a.txt", "content": "新内容" }),
            h.ctx.clone(),
        )
        .await;

    assert!(!is_ok(&out), "call 不能依赖 validate_input 已经查过");
    assert_eq!(h.fs.text("/work/a.txt").as_deref(), Some("原有内容"));
    // 拒绝的理由要精确。"文件被改了"和"你还没读过"对模型是两条完全
    // 不同的指令 —— 前者让它重读确认，后者让它先读。给错了它会空转。
    assert!(text_of(&out).contains("还没有读过"), "{}", text_of(&out));
}

#[tokio::test]
async fn edit_的_call_独立拦住先读后写() {
    let h = harness(base_fs().with_file("/work/a.rs", "fn a() {}"));

    let out = Edit
        .call(
            serde_json::json!({
                "path": "a.rs", "old_string": "fn a() {}", "new_string": "fn z() {}"
            }),
            h.ctx.clone(),
        )
        .await;

    assert!(!is_ok(&out));
    assert_eq!(h.fs.text("/work/a.rs").as_deref(), Some("fn a() {}"));
    assert!(text_of(&out).contains("还没有读过"), "{}", text_of(&out));
}

#[tokio::test]
async fn edit_的_call_独立拦住多处匹配() {
    let h = harness(base_fs().with_file("/work/a.rs", "let x = 1;\nlet x = 2;\n"));
    read(&h, serde_json::json!({ "path": "a.rs" })).await;

    let out = Edit
        .call(
            serde_json::json!({ "path": "a.rs", "old_string": "let x", "new_string": "let y" }),
            h.ctx.clone(),
        )
        .await;

    assert!(!is_ok(&out), "唯一性检查在 call 里也要有");
    assert_eq!(
        h.fs.text("/work/a.rs").as_deref(),
        Some("let x = 1;\nlet x = 2;\n")
    );
}

#[tokio::test]
async fn write_的_call_独立拦住可疑路径() {
    // `call` 不能假设一定过了 validate_input —— 工具可能被直接调用。
    // 边界检查没了，形状检查还得在这条路上照样生效。
    let h = harness(base_fs());

    let out = Write
        .call(
            serde_json::json!({ "path": "/work/evil.txt:ads", "content": "x" }),
            h.ctx.clone(),
        )
        .await;

    assert!(!is_ok(&out));
}

// ── 内部函数 ──────────────────────────────────────────

#[test]
fn 行号形状检测() {
    use super::edit::looks_like_line_numbered;

    assert!(looks_like_line_numbered("     1\tfn a() {}"));
    assert!(looks_like_line_numbered("    12\ta\n    13\tb"));

    assert!(!looks_like_line_numbered("fn a() {}"));
    assert!(!looks_like_line_numbered("a\tb"), "普通的 tab 分隔不算");
    assert!(!looks_like_line_numbered(""));
}
