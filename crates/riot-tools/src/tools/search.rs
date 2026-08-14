//! Glob 和 Grep 共用的遍历与搜索。
//!
//! # 为什么不 spawn ripgrep
//!
//! 早先这两个工具是 `spawn("rg")`：gitignore 处理、编码嗅探、并行遍历
//! 的工程量确实远超"匹配正则"本身，借现成的二进制很划算。
//!
//! 但对一个**桌面应用**来说，"用户装了 rg、而且它恰好在 PATH 里"是个
//! 会反复落空的假设：从 Dock 启动的应用继承不到 shell 的 PATH（和 API
//! key 那条是同一个坑），`brew install` 也救不了它。两个最常用的工具
//! 就这么静默坏掉，用户看到的只是"找不到 ripgrep"。
//!
//! 所以改成直接用 ripgrep 的**库**：`ignore` 是它的遍历引擎，
//! `grep-searcher` / `grep-regex` 是它的搜索引擎。行为同源（gitignore
//! 语义、二进制跳过、上下文行），只是不再需要外部进程。
//!
//! # 顺序是确定的
//!
//! 用串行遍历 + 按路径排序，而不是 rg 的并行遍历。慢一点，但同一次
//! 调用两次给出同样的结果 —— 模型看到的是被截断的前 N 条，顺序不稳
//! 意味着"再搜一次"会换一批答案。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use grep_regex::RegexMatcher;
use grep_searcher::sinks::UTF8;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder};
use riot_protocol::tool::Clock;
use tokio_util::sync::CancellationToken;

/// 遍历/搜索的时间上限（秒）。
///
/// 超过它基本意味着走进了不该走的地方（挂载的网络盘、几十万个文件的
/// 目录）。到点就带着已有结果收工，而不是让模型干等。
pub const TIME_BUDGET_SECS: u64 = 30;

/// 到点没有？
///
/// 走注入的 Clock 而不是 `Instant::now()`：工具的输出要能回放，
/// 而"这次搜索有没有超时"直接决定结果里有没有那句"没走完"。
pub struct Deadline {
    clock: Arc<dyn Clock>,
    until_ms: u64,
}

impl Deadline {
    pub fn new(clock: Arc<dyn Clock>, budget_secs: u64) -> Self {
        let until_ms = clock.now_ms().saturating_add(budget_secs * 1000);
        Self { clock, until_ms }
    }

    fn passed(&self) -> bool {
        self.clock.now_ms() > self.until_ms
    }
}

/// 一次遍历的产物。
#[derive(Debug)]
pub struct Walked {
    pub files: Vec<PathBuf>,
    /// 是否因为超时/取消提前收工（结果不完整）。
    pub cut_short: bool,
}

/// 走一遍目录，返回文件路径。
///
/// `glob` 是 gitignore 风格的白名单（`**/*.rs`、`src/*.toml`），和
/// ripgrep 的 `--glob` 同一套语义 —— 它就是同一个库。
///
/// `[约束]` 白名单**压过 .gitignore**：`glob: "**/*.rs"` 会连
/// gitignore 掉的 `.rs` 一起搜出来。这条不直观，但和 ripgrep 一致
///（实测 `rg --files -g '**/*.rs'` 就是这个结果），而"和 rg 一样"
/// 比"我觉得应该怎样"更值得守 —— 用户拿 rg 验证我们的输出时，
/// 对不上才是真的麻烦。
pub fn walk(
    root: &Path,
    glob: Option<&str>,
    limit: usize,
    cancel: &CancellationToken,
    deadline: &Deadline,
) -> Result<Walked, String> {
    let mut b = ignore::WalkBuilder::new(root);
    b.hidden(false) // 点开头的目录也要进：.github/workflows、.cargo/config.toml
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false) // 不是 git 仓库时 .gitignore 也照样认
        .follow_links(false)
        .filter_entry(|e| e.file_name() != ".git")
        // 顺序确定（见模块文档）。
        .sort_by_file_path(std::cmp::Ord::cmp);

    if let Some(g) = glob {
        let mut ob = ignore::overrides::OverrideBuilder::new(root);
        ob.add(g).map_err(|e| glob_hint(g, &e))?;
        b.overrides(ob.build().map_err(|e| glob_hint(g, &e))?);
    }

    let mut files = Vec::new();
    let mut cut_short = false;

    for entry in b.build() {
        if cancel.is_cancelled() || deadline.passed() {
            cut_short = true;
            break;
        }
        // 读不进去的目录（权限）跳过就好 —— 为一个子目录放弃整次遍历
        // 不划算，而且用户多半也不关心它。
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        files.push(entry.into_path());
        if files.len() >= limit {
            cut_short = true;
            break;
        }
    }
    Ok(Walked { files, cut_short })
}

fn glob_hint(g: &str, e: &ignore::Error) -> String {
    format!("`glob` 不是合法的模式：{g}（{e}）。例子：`**/*.rs`、`src/**/*.ts`。")
}

/// 搜索的三种输出形态。和工具层的 `OutputMode` 一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 匹配行本身（可带上下文）。
    Content { context: usize },
    /// 只要文件名。
    FilesWithMatches,
    /// 每个文件几处匹配。
    Count,
}

pub struct Found {
    /// 已经按 ripgrep 的格式排好的行：`路径:行号:内容`。
    pub lines: Vec<String>,
    pub cut_short: bool,
}

/// 在一批文件里搜正则。
///
/// `[约束]` 单个文件出错（权限、坏编码、搜到一半 IO 失败）只跳过它，
/// 不中断整次搜索 —— 一个读不了的文件不该让另外九十九个的结果消失。
pub fn grep(
    files: &[PathBuf],
    pattern: &str,
    case_insensitive: bool,
    mode: Mode,
    cancel: &CancellationToken,
    deadline: &Deadline,
) -> Result<Found, String> {
    let matcher = grep_regex::RegexMatcherBuilder::new()
        .case_insensitive(case_insensitive)
        .build(pattern)
        .map_err(|e| format!("`pattern` 不是合法的正则：{e}"))?;

    let mut searcher = builder(mode);
    let mut lines = Vec::new();
    let mut cut_short = false;

    for path in files {
        if cancel.is_cancelled() || deadline.passed() {
            cut_short = true;
            break;
        }
        match mode {
            Mode::Content { .. } => search_content(&mut searcher, &matcher, path, &mut lines),
            Mode::FilesWithMatches => {
                if has_match(&mut searcher, &matcher, path) {
                    lines.push(path.display().to_string());
                }
            }
            Mode::Count => {
                let n = count(&mut searcher, &matcher, path);
                if n > 0 {
                    lines.push(format!("{}:{n}", path.display()));
                }
            }
        }
    }
    Ok(Found { lines, cut_short })
}

fn builder(mode: Mode) -> Searcher {
    let mut b = SearcherBuilder::new();
    // 遇到 NUL 就停：二进制文件里的"匹配"对模型没有意义，还可能是几 MB
    // 的乱码。ripgrep 默认也是这么干的。
    b.binary_detection(BinaryDetection::quit(0));
    b.line_number(true);
    if let Mode::Content { context } = mode {
        b.before_context(context);
        b.after_context(context);
    }
    b.build()
}

/// 匹配行 `路径:行号:内容`，上下文行 `路径-行号-内容`（rg 同款分隔符：
/// 一眼能看出哪几行是真的命中）。
fn search_content(
    searcher: &mut Searcher,
    matcher: &RegexMatcher,
    path: &Path,
    out: &mut Vec<String>,
) {
    let shown = path.display().to_string();
    let mut sink = ContextSink {
        path: &shown,
        out,
        // grep-searcher 的 UTF8 sink 不区分匹配行和上下文行，所以自己实现。
        matched: true,
    };
    let _ = searcher.search_path(matcher, path, &mut sink);
}

struct ContextSink<'a> {
    path: &'a str,
    out: &'a mut Vec<String>,
    matched: bool,
}

impl grep_searcher::Sink for ContextSink<'_> {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _s: &Searcher,
        m: &grep_searcher::SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
        self.matched = true;
        push_line(self.out, self.path, ':', m.line_number(), m.bytes());
        Ok(true)
    }

    fn context(
        &mut self,
        _s: &Searcher,
        c: &grep_searcher::SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        push_line(self.out, self.path, '-', c.line_number(), c.bytes());
        Ok(true)
    }
}

fn push_line(out: &mut Vec<String>, path: &str, sep: char, line: Option<u64>, bytes: &[u8]) {
    // lossy 是对的：搜索结果只给模型看，不会被原样写回任何地方。
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_end_matches(['\n', '\r']);
    match line {
        Some(n) => out.push(format!("{path}{sep}{n}{sep}{text}")),
        None => out.push(format!("{path}{sep}{text}")),
    }
}

fn has_match(searcher: &mut Searcher, matcher: &RegexMatcher, path: &Path) -> bool {
    let mut hit = false;
    // 第一处匹配就够了：返回 false 让搜索器停下，别把整个文件读完。
    let _ = searcher.search_path(
        matcher,
        path,
        UTF8(|_, _| {
            hit = true;
            Ok(false)
        }),
    );
    hit
}

fn count(searcher: &mut Searcher, matcher: &RegexMatcher, path: &Path) -> usize {
    let mut n = 0usize;
    let _ = searcher.search_path(
        matcher,
        path,
        UTF8(|_, _| {
            n += 1;
            Ok(true)
        }),
    );
    n
}

// 这一组测试要在真实文件系统上建树 —— 测的就是"遍历真目录"这件事，
// 注入的 MemFs 在这里没有意义（ignore 走的是真磁盘）。
#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn tree() -> tempfile::TempDir {
        let t = tempfile::tempdir().expect("临时目录");
        let p = t.path();
        std::fs::write(p.join(".gitignore"), "ignored.rs\ntarget/\n").expect("写");
        std::fs::write(p.join("a.rs"), "fn foo() {}\nlet x = 1;\nfoo();\n").expect("写");
        std::fs::write(p.join("b.txt"), "foo in text\n").expect("写");
        std::fs::write(p.join("ignored.rs"), "fn foo() {}\n").expect("写");
        std::fs::create_dir_all(p.join("src")).expect("目录");
        std::fs::write(p.join("src/c.rs"), "// FOO here\n").expect("写");
        std::fs::create_dir_all(p.join("target")).expect("目录");
        std::fs::write(p.join("target/d.rs"), "fn foo() {}\n").expect("写");
        t
    }

    fn test_deadline() -> Deadline {
        Deadline::new(Arc::new(crate::testing::FixedClock::default()), TIME_BUDGET_SECS)
    }

    fn names(files: &[PathBuf], root: &Path) -> Vec<String> {
        files
            .iter()
            .map(|f| {
                f.strip_prefix(root)
                    .unwrap_or(f)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    #[test]
    fn 遍历尊重_gitignore_并跳过_git() {
        let t = tree();
        std::fs::create_dir_all(t.path().join(".git")).expect("目录");
        std::fs::write(t.path().join(".git/HEAD"), "x").expect("写");

        let w = walk(t.path(), None, 1000, &CancellationToken::new(), &test_deadline()).expect("遍历");
        let got = names(&w.files, t.path());
        assert!(got.contains(&"a.rs".to_owned()));
        assert!(got.contains(&"src/c.rs".to_owned()));
        assert!(!got.contains(&"ignored.rs".to_owned()), "gitignore 要生效：{got:?}");
        assert!(!got.iter().any(|p| p.starts_with("target/")), "{got:?}");
        assert!(!got.iter().any(|p| p.starts_with(".git/")), "{got:?}");
        assert!(!w.cut_short);
    }

    #[test]
    fn glob_过滤与_rg_同语义() {
        let t = tree();
        let w = walk(t.path(), Some("**/*.rs"), 1000, &CancellationToken::new(), &test_deadline()).expect("遍历");
        let got = names(&w.files, t.path());
        // 只要 .rs —— 包括被 gitignore 掉的那个：显式点名压过忽略规则，
        // 和 `rg --files -g '**/*.rs'` 的实测结果一致（见 walk 的注释）。
        assert_eq!(
            got,
            vec!["a.rs".to_owned(), "ignored.rs".to_owned(), "src/c.rs".to_owned()],
            "{got:?}"
        );

        let w = walk(t.path(), Some("src/*.rs"), 1000, &CancellationToken::new(), &test_deadline()).expect("遍历");
        assert_eq!(names(&w.files, t.path()), vec!["src/c.rs".to_owned()]);
    }

    #[test]
    fn 坏的_glob_给出可读的错() {
        let e = walk(Path::new("."), Some("["), 10, &CancellationToken::new(), &test_deadline())
            .expect_err("坏模式该报错");
        assert!(e.contains("glob"), "{e}");
    }

    #[test]
    fn 顺序确定_且尊重上限() {
        let t = tree();
        let once = walk(t.path(), None, 1000, &CancellationToken::new(), &test_deadline()).expect("遍历");
        let twice = walk(t.path(), None, 1000, &CancellationToken::new(), &test_deadline()).expect("遍历");
        assert_eq!(once.files, twice.files, "两次调用必须给出同样的顺序");

        let cut = walk(t.path(), None, 2, &CancellationToken::new(), &test_deadline()).expect("遍历");
        assert_eq!(cut.files.len(), 2);
        assert!(cut.cut_short, "被上限截断要说出来");
    }

    #[test]
    fn 取消之后立刻收工() {
        let t = tree();
        let c = CancellationToken::new();
        c.cancel();
        let w = walk(t.path(), None, 1000, &c, &test_deadline()).expect("遍历");
        assert!(w.files.is_empty());
        assert!(w.cut_short);
    }

    #[test]
    fn 内容模式带路径行号() {
        let t = tree();
        let files = walk(t.path(), Some("a.rs"), 100, &CancellationToken::new(), &test_deadline())
            .expect("遍历")
            .files;
        let found = grep(&files, "foo", false, Mode::Content { context: 0 }, &CancellationToken::new(), &test_deadline())
            .expect("搜索");
        assert_eq!(found.lines.len(), 2, "两处匹配：{:?}", found.lines);
        assert!(found.lines[0].ends_with(":1:fn foo() {}"), "{:?}", found.lines);
        assert!(found.lines[1].ends_with(":3:foo();"), "{:?}", found.lines);
    }

    #[test]
    fn 上下文行用短横线区分() {
        let t = tree();
        let files = walk(t.path(), Some("a.rs"), 100, &CancellationToken::new(), &test_deadline())
            .expect("遍历")
            .files;
        let found = grep(&files, "let x", false, Mode::Content { context: 1 }, &CancellationToken::new(), &test_deadline())
            .expect("搜索");
        // 前一行、匹配行、后一行；只有中间那条用 `:`。
        assert_eq!(found.lines.len(), 3, "{:?}", found.lines);
        assert!(found.lines[0].contains("-1-fn foo"), "{:?}", found.lines);
        assert!(found.lines[1].contains(":2:let x"), "{:?}", found.lines);
        assert!(found.lines[2].contains("-3-foo();"), "{:?}", found.lines);
    }

    #[test]
    fn 大小写与另外两种模式() {
        let t = tree();
        let files = walk(t.path(), None, 100, &CancellationToken::new(), &test_deadline()).expect("遍历").files;
        let cancel = CancellationToken::new();

        let sensitive = grep(&files, "FOO", false, Mode::FilesWithMatches, &cancel, &test_deadline()).expect("搜索");
        assert_eq!(sensitive.lines.len(), 1, "只有 src/c.rs 是大写的");

        let insensitive = grep(&files, "FOO", true, Mode::FilesWithMatches, &cancel, &test_deadline()).expect("搜索");
        assert_eq!(insensitive.lines.len(), 3, "忽略大小写后三个文件都算：{:?}", insensitive.lines);

        let counted = grep(&files, "foo", true, Mode::Count, &cancel, &test_deadline()).expect("搜索");
        assert!(
            counted.lines.iter().any(|l| l.ends_with("a.rs:2")),
            "a.rs 里两处：{:?}",
            counted.lines
        );
    }

    #[test]
    fn 二进制文件不进结果() {
        // 不挡的话，一个 .so 里的"匹配"能吐出几 MB 乱码。
        let t = tempfile::tempdir().expect("目录");
        std::fs::write(t.path().join("bin.dat"), b"foo\x00\x01foo").expect("写");
        let files = walk(t.path(), None, 10, &CancellationToken::new(), &test_deadline()).expect("遍历").files;
        let found = grep(&files, "foo", false, Mode::Content { context: 0 }, &CancellationToken::new(), &test_deadline())
            .expect("搜索");
        assert!(found.lines.len() <= 1, "NUL 之后不该继续：{:?}", found.lines);
    }

    #[test]
    fn 读不了的文件不影响其它() {
        let t = tree();
        let mut files = walk(t.path(), Some("**/*.rs"), 100, &CancellationToken::new(), &test_deadline())
            .expect("遍历")
            .files;
        let real = files.len();
        files.insert(0, t.path().join("不存在.rs"));
        let found = grep(&files, "foo", true, Mode::FilesWithMatches, &CancellationToken::new(), &test_deadline())
            .expect("搜索");
        assert_eq!(found.lines.len(), real, "其余文件照常出结果：{:?}", found.lines);
    }
}
