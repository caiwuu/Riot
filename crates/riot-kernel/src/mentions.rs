//! `@` 文件引用：用户在消息里点名的文件，发送时连内容一起带上。
//!
//! ```text
//! 看看 @src/main.rs 里的启动顺序，和 @"docs/设计 稿.md" 对一下
//! ```
//!
//! # 为什么直接塞内容，而不是只给路径
//!
//! 附件按钮（非图片文件）走的是"只给路径、模型自己 Read"那条路，理由是
//! 一个文件几万 token、九成用不上。`@` 引用反过来：用户是**明确点名**
//! 让模型看这个文件的，只给路径就是白白多一次往返 —— 而且模型多半会
//! 把整个文件读进来，省不下任何东西。
//!
//! 但"明确点名"不等于"多大都塞"：超过 [`MAX_FILE_CHARS`] 就只带前面
//! 一段并告诉模型剩下的自己 Read，总量再受 [`MAX_TOTAL_CHARS`] 兜底 ——
//! 用户一句 `@node_modules` 不该把上下文吃光。
//!
//! # 与记忆文件里的 @ 的区别
//!
//! [`crate::memory`] 里的 `@路径` 是**文件内**的 include（AGENTS.md 引用
//! 别的文档），递归展开、静默失败。这里是**用户消息**里的引用：不递归
//! （用户没让你顺着读下去），失败要说出来（用户以为自己附上了）。
//!
//! 豁免理由：宿主层，读的是用户在自己项目里点名的文件。

#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use riot_protocol::message::Attachment;
use riot_protocol::tool::{FileState, FileStateCache, FileView};

/// 单个引用最多带多少字符。约等于 600 行代码 —— 再大的文件，用户想让
/// 模型看的多半也只是其中一段。
const MAX_FILE_CHARS: usize = 24 * 1024;

/// 一条消息里所有引用加起来的上限。`@a @b @c` 三个大文件也不该
/// 把这一轮的上下文吃掉一半。
const MAX_TOTAL_CHARS: usize = 64 * 1024;

/// 目录引用最多列几个条目。
const MAX_DIR_ENTRIES: usize = 50;

/// 一条被解析出来的引用。
#[derive(Debug, Clone, PartialEq)]
pub struct Mention {
    /// 用户原样写的那串（不含 `@`），用于回显和报错。
    pub raw: String,
    /// 解析后的绝对路径。
    pub path: PathBuf,
}

/// 界面上选中的引用（已经是路径，不用从文本里认）。
pub fn from_paths(paths: &[String], cwd: &Path) -> Vec<Mention> {
    paths
        .iter()
        .filter_map(|p| {
            resolve(p, cwd).map(|path| Mention {
                raw: p.clone(),
                path,
            })
        })
        .collect()
}

/// 合并两路引用（正文里手打的 + 界面选中的），按解析后的路径去重。
pub fn merge(mut a: Vec<Mention>, b: Vec<Mention>) -> Vec<Mention> {
    for m in b {
        if !a.iter().any(|x| x.path == m.path) {
            a.push(m);
        }
    }
    a
}

/// 从用户消息里挑出 `@` 引用。
///
/// 规则（和 [`crate::memory`] 的行内引用一致，用户在两处的直觉该相同）：
/// - `@` 前面得是边界 —— 挡的是 `user@host` 和邮箱，中文正文里的
///   `读下@src/a.rs` 算（见 [`is_mention_boundary`]）；
/// - 反引号包住的内容不扫 —— 用户在讲 `` `@types/node` `` 这个包名；
/// - `@"带 空格 的/路径"` 用引号括起来；
/// - 裸路径到下一个空白为止，且要长得像路径（含 `/`，或 `.` / `~` 开头，
///   或带扩展名）—— 否则 `@这里` 这种中文口语会被误当引用。
pub fn parse(text: &str, cwd: &Path) -> Vec<Mention> {
    let mut out: Vec<Mention> = Vec::new();
    let mut in_fence = false;

    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        for raw in extract(line) {
            let Some(path) = resolve(&raw, cwd) else {
                continue;
            };
            // 同一条消息里引用两遍同一个文件只带一份。
            if out.iter().any(|m| m.path == path) {
                continue;
            }
            out.push(Mention { raw, path });
        }
    }
    out
}

fn extract(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut in_tick = false;
    // `@` 前面得是"边界"才算引用。行首默认是。
    let mut prev_is_boundary = true;

    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            in_tick = !in_tick;
            prev_is_boundary = true;
            i += 1;
            continue;
        }
        if in_tick || c != '@' || !prev_is_boundary {
            prev_is_boundary = is_mention_boundary(c);
            i += 1;
            continue;
        }

        // 引号形式：@"..." —— 路径里有空格时唯一的写法。
        if chars.get(i + 1) == Some(&'"') {
            let start = i + 2;
            if let Some(end) = (start..chars.len()).find(|&j| chars[j] == '"') {
                let raw: String = chars[start..end].iter().collect();
                if !raw.trim().is_empty() {
                    out.push(raw);
                }
                i = end + 1;
                prev_is_boundary = false;
                continue;
            }
        }

        let raw: String = chars[i + 1..]
            .iter()
            .take_while(|c| !c.is_whitespace() && !is_stop_punct(**c))
            .collect();
        let cleaned = trim_trailing_punctuation(&raw);
        if looks_like_path(cleaned) {
            out.push(cleaned.to_owned());
        }
        i += 1 + raw.chars().count();
        prev_is_boundary = false;
    }
    out
}

/// `@` 前面这个字符算不算边界。
///
/// 反着定义：只有 ASCII 标识符字符（外加邮箱本地部分能出现的那几个）
/// 才**不是**边界 —— 那正是 `me@example.com`、`user@host` 的形状，也是
/// 这条规则唯一要挡的东西。
///
/// `[约束]` 中文不写空格。"读下@src/a.rs" 里 `下` 必须算边界，否则中文
/// 用户不在 `@` 前面敲个空格就引用不了文件 —— 而输入框里的引用块发出去
/// 就是这个形状，块紧贴着前一个字是常态。
fn is_mention_boundary(c: char) -> bool {
    !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-'))
}

/// 中文标点不是空白，`@src/main.rs，还有` 会把半句话吞进路径 —— 这些
/// 字符在路径里几乎不可能出现，遇到就断开。
fn is_stop_punct(c: char) -> bool {
    matches!(
        c,
        '，' | '。'
            | '；'
            | '：'
            | '、'
            | '！'
            | '？'
            | '）'
            | '（'
            | '「'
            | '」'
            | '《'
            | '》'
            | '“'
            | '”'
    )
}

/// 引用常出现在句子里（"看看 @src/main.rs."），句读不算路径的一部分。
fn trim_trailing_punctuation(s: &str) -> &str {
    s.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '"', '\''])
}

/// 长得像不像路径。挡的是中文口语（`@这里`、`@我`）——它们不是引用，
/// 展开成"读不到这个路径"的提示只会给模型添乱。
fn looks_like_path(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if s.contains('/') || s.starts_with('.') || s.starts_with('~') {
        return true;
    }
    // 不带斜杠的裸名（`@README.md`、`@src`）：限定 ASCII 路径字符。
    // 中文文件名要引用，写成 `@目录/文件` 或 `@"中文名.md"`。
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// 相对路径以**项目根**为基准（不是文件所在目录 —— 用户是在项目语境里
/// 说话）。`~/` 展开成 HOME，绝对路径原样。
fn resolve(raw: &str, cwd: &Path) -> Option<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(rel) = raw.strip_prefix("~/") {
        let home = std::env::var("HOME").ok()?;
        return Some(Path::new(&home).join(rel));
    }
    let p = Path::new(raw);
    Some(if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    })
}

/// 把引用读成附件。
///
/// `file_state` 是可选的工作集登记：登记之后模型可以直接 Edit 这个文件，
/// 不用先 Read 一遍（先读后写协议认这份缓存）。截断过的内容登记成
/// `Partial`（给模型看的范围），缓存里仍是全文。
pub fn expand(mentions: &[Mention], file_state: Option<&dyn FileStateCache>) -> Vec<Attachment> {
    let mut out = Vec::new();
    let mut budget = MAX_TOTAL_CHARS;

    for m in mentions {
        let meta = match std::fs::metadata(&m.path) {
            Ok(meta) => meta,
            Err(e) => {
                out.push(note(format!(
                    "The user wrote @{} in their message, but that path cannot be read ({e}). If \
                     you need it, ask them to confirm the path — do NOT guess one, since a \
                     guessed path silently gives you the wrong file.",
                    m.raw
                )));
                continue;
            }
        };

        if meta.is_dir() {
            out.push(note(list_dir(&m.path)));
            continue;
        }

        if budget == 0 {
            out.push(note(format!(
                "The user also referenced {}, but this message already carries enough files, so \
                 it was not attached. Read it with the Read tool if you need it.",
                m.path.display()
            )));
            continue;
        }

        let content = match std::fs::read_to_string(&m.path) {
            Ok(c) => c,
            Err(e) => {
                // 二进制（xlsx 等）也要落成 UserFile：界面靠 `kind: user_file`
                // 在气泡里画出引用块。只发 SystemReminder 的话，切回会话
                // 就只剩正文里一串 `@路径` 纯文字。
                out.push(Attachment::UserFile {
                    path: m.path.clone(),
                    content: format!(
                        "Could not be read as text ({e}); it is probably a binary file. Use Read \
                         on it if you need its contents."
                    ),
                });
                continue;
            }
        };

        let cap = MAX_FILE_CHARS.min(budget);
        let total = content.chars().count();
        let truncated = total > cap;
        let body: String = if truncated {
            content.chars().take(cap).collect()
        } else {
            content.clone()
        };
        budget = budget.saturating_sub(body.chars().count());

        if let Some(fs) = file_state {
            // 登记进工作集：模型可以直接改。截断只影响给模型看的那段，
            // 缓存里是全文，Edit 按全文做唯一性检查。
            let view = if truncated {
                FileView::Partial {
                    offset: 0,
                    limit: body.lines().count(),
                }
            } else {
                FileView::Full
            };
            fs.put(
                m.path.clone(),
                FileState {
                    content: content.clone(),
                    mtime_ms: mtime_ms(&meta),
                    view,
                },
            );
        }

        let tail = if truncated {
            format!(
                "\n\n[Only the first {} characters are attached, out of {total}. Read the rest \
                 with Read as you need it.]",
                body.chars().count()
            )
        } else {
            String::new()
        };
        out.push(Attachment::UserFile {
            path: m.path.clone(),
            content: format!("{body}{tail}"),
        });
    }
    out
}

fn note(text: String) -> Attachment {
    Attachment::SystemReminder { text }
}

fn list_dir(dir: &Path) -> String {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return format!(
            "The user referenced the directory {}, but it could not be listed.",
            dir.display()
        );
    };
    let mut names: Vec<String> = rd
        .flatten()
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();
    names.sort();
    let total = names.len();
    names.truncate(MAX_DIR_ENTRIES);
    let more = if total > MAX_DIR_ENTRIES {
        format!("\n… and {} more entries", total - MAX_DIR_ENTRIES)
    } else {
        String::new()
    };
    format!(
        "The user referenced the directory {}, which contains:\n{}{more}",
        dir.display(),
        names.join("\n")
    )
}

fn mtime_ms(m: &std::fs::Metadata) -> u64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ────────────────────────────────────────────────────────────
// 补全菜单的文件搜索
// ────────────────────────────────────────────────────────────

/// 补全菜单一次最多给几条。
const SEARCH_LIMIT: usize = 12;

/// 文件清单的缓存寿命。`@` 之后每敲一个字都要搜一次，每次都 spawn
/// 一遍 ripgrep 在大仓库上会让菜单发飘；文件增删几秒后才出现在菜单里
/// 是可以接受的代价。
const CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(5);

type FileCache = std::sync::Mutex<Option<(PathBuf, std::time::Instant, Vec<String>)>>;

static FILES: std::sync::LazyLock<FileCache> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

/// 一次最多收多少个路径。大仓库全量收下来只会让菜单变慢，而用户
/// 要找的文件几乎不可能排在第五万个。
const MAX_FILES: usize = 50_000;

/// 给 `@` 补全菜单用的文件搜索：返回项目内相对路径。
///
/// `limit` 是最多几条；`None` 用补全菜单的默认（十来条）。文件树的筛选
/// 框也走这条，那边一屏能摆几十上百条，传自己的上限。
///
/// `[约束]` 走 `ignore` crate 自己遍历，**不 spawn ripgrep**。工具层的
/// Glob/Grep 依赖外部 rg 是另一回事（那边失败了模型看得到错误，能换
/// 个做法）；补全菜单没有这种余地 —— 用户机器上没装 rg 的话，敲 `@`
/// 会毫无反应，而且看不出为什么。`ignore` 正是 ripgrep 的遍历引擎，
/// .gitignore / .ignore / 全局 gitignore 的语义一致。
pub async fn search_files(root: &Path, query: &str, limit: Option<usize>) -> Vec<String> {
    let limit = limit.unwrap_or(SEARCH_LIMIT);
    let all = file_list(root).await;
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return all.into_iter().take(limit).collect();
    }

    // 子序列匹配（`smrs` 能命中 `src/main.rs`），排序按"匹配得紧不紧"
    // 再按路径短优先 —— 顶层文件通常比深处的更可能是用户要的。
    let mut hits: Vec<(usize, usize, String)> = all
        .into_iter()
        .filter_map(|p| {
            let lower = p.to_lowercase();
            // 直接子串命中算最紧（span = 查询长度）。
            let span = match lower.find(&q) {
                Some(_) => q.len(),
                None => subsequence_span(&lower, &q)?,
            };
            Some((span, p.len(), p))
        })
        .collect();
    hits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    hits.into_iter().take(limit).map(|(_, _, p)| p).collect()
}

/// 把 `query` 当子序列去匹配，返回跨度（越小越紧）。匹配不上是 None。
///
/// `smrs` 能命中 `src/main.rs`；跨度用来排序 —— 字符挨得越近，越像
/// 用户想找的那个。
fn subsequence_span(haystack: &str, query: &str) -> Option<usize> {
    let mut start = None;
    let mut chars = query.chars();
    let mut want = chars.next()?;
    for (i, c) in haystack.char_indices() {
        if c != want {
            continue;
        }
        let first = *start.get_or_insert(i);
        match chars.next() {
            Some(next) => want = next,
            None => return Some(i - first + 1),
        }
    }
    None
}

#[cfg(test)]
mod search_tests {
    use super::*;

    #[tokio::test]
    async fn 遍历尊重_gitignore_且不依赖外部程序() {
        // 这个用例存在的理由：第一版 spawn `rg`，而用户机器上没装 ——
        // 敲 @ 毫无反应，还看不出为什么。补全菜单不能依赖外部二进制。
        let t = tempfile::tempdir().expect("目录");
        let root = t.path();
        std::fs::write(root.join(".gitignore"), "ignored.txt\ntarget/\n").expect("写");
        std::fs::write(root.join("keep.rs"), "x").expect("写");
        std::fs::write(root.join("ignored.txt"), "x").expect("写");
        std::fs::create_dir_all(root.join("target")).expect("建目录");
        std::fs::write(root.join("target/big.o"), "x").expect("写");
        std::fs::create_dir_all(root.join(".git")).expect("建目录");
        std::fs::write(root.join(".git/HEAD"), "x").expect("写");
        std::fs::create_dir_all(root.join(".github/workflows")).expect("建目录");
        std::fs::write(root.join(".github/workflows/ci.yml"), "x").expect("写");

        let got = walk(root);
        assert!(got.contains(&"keep.rs".to_owned()));
        assert!(
            got.contains(&".github/workflows/ci.yml".to_owned()),
            "点开头的目录要进 —— 用户会引用 CI 配置：{got:?}"
        );
        assert!(!got.contains(&"ignored.txt".to_owned()), "gitignore 要生效");
        assert!(
            !got.iter().any(|p| p.starts_with("target/")),
            "构建产物不该进菜单"
        );
        assert!(
            !got.iter().any(|p| p.starts_with(".git/")),
            ".git 内部不该进菜单"
        );

        // 缓存这一层也要通：搜索走的是它。
        let hits = search_files(root, "keep", None).await;
        assert_eq!(hits, vec!["keep.rs".to_owned()]);
    }

    #[test]
    fn 子序列匹配与紧凑度() {
        assert_eq!(subsequence_span("src/main.rs", "smrs"), Some(11));
        assert_eq!(
            subsequence_span("src/main.rs", "main"),
            Some(4),
            "连着的跨度最小"
        );
        assert_eq!(subsequence_span("src/main.rs", "xyz"), None);
        assert_eq!(subsequence_span("abc", ""), None, "空查询不该冒充匹配");
    }
}

async fn file_list(root: &Path) -> Vec<String> {
    if let Some((cached_root, at, files)) = FILES.lock().expect("文件缓存锁不该中毒").as_ref()
        && cached_root == root
        && at.elapsed() < CACHE_TTL
    {
        return files.clone();
    }

    // 遍历是同步阻塞的（几万次 stat），扔给阻塞线程池 —— 占着 async
    // 线程扫一个大仓库，界面上表现为整个应用卡住。
    let base = root.to_path_buf();
    let files = tokio::task::spawn_blocking(move || walk(&base))
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "遍历任务没跑成，@ 补全菜单这次是空的");
            Vec::new()
        });

    *FILES.lock().expect("文件缓存锁不该中毒") =
        Some((root.to_path_buf(), std::time::Instant::now(), files.clone()));
    files
}

/// 走一遍项目目录，返回相对路径。
fn walk(root: &Path) -> Vec<String> {
    ignore::WalkBuilder::new(root)
        // 点文件要进：`.github/workflows/ci.yml`、`.cargo/config.toml`
        // 都是用户会引用的。`.git/` 单独排掉（下面的 filter_entry）。
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|e| {
            let rel = e.path().strip_prefix(root).ok()?;
            let s = rel.to_string_lossy();
            // 分隔符统一成 `/`。这串字符串既是菜单项也是 `@` 引用的原文，
            // Windows 的 `\` 会让补全出来的路径和用户手敲的 `/` 对不上 ——
            // 同一个文件两种写法，缓存、搜索、解析都得跟着糊。join 的时候
            // `/` 在 Windows 上照样好使，反过来不成立。
            Some(if cfg!(windows) {
                s.replace('\\', "/")
            } else {
                s.into_owned()
            })
        })
        .take(MAX_FILES)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raws(text: &str) -> Vec<String> {
        parse(text, Path::new("/proj"))
            .into_iter()
            .map(|m| m.raw)
            .collect()
    }

    #[test]
    fn 认出常见写法() {
        assert_eq!(raws("看看 @src/main.rs"), vec!["src/main.rs"]);
        assert_eq!(
            raws("@README.md 里写了"),
            vec!["README.md"],
            "带扩展名的裸文件名也算"
        );
        assert_eq!(
            raws("对比 @./a/b.txt 和 @/etc/hosts"),
            vec!["./a/b.txt", "/etc/hosts"]
        );
        assert_eq!(
            raws("@\"docs/设计 稿.md\" 看下"),
            vec!["docs/设计 稿.md"],
            "引号裹住带空格的"
        );
    }

    #[test]
    fn 中文不打空格也认() {
        // 中文正文里没有空格的习惯，而输入框里的引用块紧贴着前一个字
        // 发出去就是这个形状 —— 不认的话气泡里剩一串裸路径。
        assert_eq!(raws("读下@src/main.rs"), vec!["src/main.rs"]);
        assert_eq!(
            raws("读下@\"/tmp/报表 (1).xlsx\""),
            vec!["/tmp/报表 (1).xlsx"],
            "引号形式同理"
        );
        // 后面紧贴着中文是另一回事：路径里本来就可能有中文
        //（`@docs/设计.md`），断不了。界面发出去的块因此会加引号，
        // 见 App.tsx 的 `mentionToken`。
        assert_eq!(raws("看@\"docs/设计.md\"再说"), vec!["docs/设计.md"]);
    }

    #[test]
    fn 不该被误当引用的() {
        assert!(
            raws("联系 me@example.com").is_empty(),
            "邮箱不算 —— @ 紧跟在 ASCII 标识符后面"
        );
        assert!(raws("ssh user@host").is_empty(), "user@host 同理");
        assert!(
            raws("装 `@types/node` 这个包").is_empty(),
            "行内代码里的不算"
        );
        assert!(raws("@这里 改一下").is_empty(), "中文口语不像路径");
        assert!(raws("```\n@src/main.rs\n```").is_empty(), "代码块整段跳过");
    }

    #[test]
    fn 句读不算路径的一部分() {
        // 中文标点不是空白，不特判的话半句话都会被当成路径。
        assert_eq!(raws("见 @src/main.rs。"), vec!["src/main.rs"]);
        assert_eq!(raws("见 @src/main.rs，还有别的"), vec!["src/main.rs"]);
        assert_eq!(
            raws("见 @src/main.rs、@a/b.rs"),
            vec!["src/main.rs", "a/b.rs"]
        );
        assert_eq!(raws("看 @docs/a.md."), vec!["docs/a.md"]);
    }

    #[test]
    fn 不带斜杠的裸名也认_但中文口语不认() {
        assert_eq!(raws("@src 目录看下"), vec!["src"], "目录名常常不带斜杠");
        assert!(raws("@这里 改一下").is_empty());
        assert!(raws("@我 来处理").is_empty());
    }

    #[test]
    fn 相对路径以项目根为准_同一文件只带一次() {
        let ms = parse("@src/a.rs 和 @src/a.rs", Path::new("/proj"));
        assert_eq!(ms.len(), 1, "重复引用只带一份");
        assert_eq!(ms[0].path, PathBuf::from("/proj/src/a.rs"));
    }

    #[test]
    fn 界面选中的引用与手打的合并去重() {
        // 同一个文件既在文本里写了 @、又在界面上选了块，只能带一份 ——
        // 带两份就是同样的内容在上下文里堆两遍。
        let cwd = Path::new("/proj");
        let typed = parse("看看 @src/a.rs", cwd);
        let picked = from_paths(&["src/a.rs".into(), "src/b.rs".into()], cwd);
        let merged = merge(typed, picked);

        let paths: Vec<_> = merged.iter().map(|m| m.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/proj/src/a.rs"),
                PathBuf::from("/proj/src/b.rs")
            ]
        );
    }

    #[test]
    fn 读进内容并登记工作集() {
        let t = tempfile::tempdir().expect("目录");
        let f = t.path().join("a.txt");
        std::fs::write(&f, "hello").expect("写");

        let state = riot_runtime::MemoryFileState::shared();
        let got = expand(
            &parse("@a.txt", t.path()),
            Some(state.as_ref() as &dyn FileStateCache),
        );

        match &got[..] {
            [Attachment::UserFile { path, content }] => {
                assert_eq!(path, &f);
                assert_eq!(content, "hello");
            }
            other => panic!("该是一个 UserFile：{other:?}"),
        }
        let cached = state
            .get(&f)
            .expect("该登记进工作集，否则模型要再 Read 一遍");
        assert_eq!(cached.view, FileView::Full);
    }

    #[test]
    fn 超大文件截断且标成部分视图() {
        // 给模型看的那段被截断，缓存仍登记成 Partial。
        let t = tempfile::tempdir().expect("目录");
        let f = t.path().join("big.txt");
        std::fs::write(&f, "x".repeat(MAX_FILE_CHARS + 100)).expect("写");

        let state = riot_runtime::MemoryFileState::shared();
        let got = expand(
            &parse("@big.txt", t.path()),
            Some(state.as_ref() as &dyn FileStateCache),
        );
        match &got[..] {
            [Attachment::UserFile { content, .. }] => {
                assert!(
                    content.contains("Read the rest with Read"),
                    "要告诉模型还有后文"
                );
            }
            other => panic!("该是一个 UserFile：{other:?}"),
        }
        assert!(
            matches!(state.get(&f).expect("有").view, FileView::Partial { .. }),
            "截断过的必须是 Partial，否则模型会以为看过全文"
        );
    }

    #[test]
    fn 读不到的引用要说出来() {
        // 静默跳过的话，用户以为附上了，模型却说"你没给我文件"。
        let t = tempfile::tempdir().expect("目录");
        let got = expand(&parse("@nope.txt", t.path()), None);
        match &got[..] {
            [Attachment::SystemReminder { text }] => assert!(text.contains("nope.txt")),
            other => panic!("该有一条说明：{other:?}"),
        }
    }

    #[test]
    fn 二进制引用也落成_user_file() {
        // 只发 SystemReminder 的话，切回会话前端找不到 user_file，
        // 气泡里的引用块会退回一串 `@路径` 纯文字。
        let t = tempfile::tempdir().expect("目录");
        let f = t.path().join("a.xlsx");
        std::fs::write(&f, [0xff, 0xfe, 0x00, 0x01]).expect("写");
        let got = expand(
            &[Mention {
                raw: "a.xlsx".into(),
                path: f.clone(),
            }],
            None,
        );
        match &got[..] {
            [Attachment::UserFile { path, content }] => {
                assert_eq!(path, &f);
                assert!(content.contains("Could not be read as text"), "{content}");
            }
            other => panic!("该是一个 UserFile：{other:?}"),
        }
    }

    #[test]
    fn 目录引用列出条目() {
        let t = tempfile::tempdir().expect("目录");
        std::fs::create_dir(t.path().join("sub")).expect("建目录");
        std::fs::write(t.path().join("sub/a.txt"), "x").expect("写");
        let got = expand(&parse("@sub", t.path()), None);
        match &got[..] {
            [Attachment::SystemReminder { text }] => assert!(text.contains("a.txt")),
            other => panic!("该列目录：{other:?}"),
        }
    }

    #[test]
    fn 总量有上限() {
        let t = tempfile::tempdir().expect("目录");
        let big = "y".repeat(MAX_FILE_CHARS);
        for i in 0..4 {
            std::fs::write(t.path().join(format!("f{i}.txt")), &big).expect("写");
        }
        let got = expand(&parse("@f0.txt @f1.txt @f2.txt @f3.txt", t.path()), None);
        let total: usize = got
            .iter()
            .filter_map(|a| match a {
                Attachment::UserFile { content, .. } => Some(content.chars().count()),
                _ => None,
            })
            .sum();
        assert!(total <= MAX_TOTAL_CHARS + 200, "总量要有兜底，实际 {total}");
        assert!(
            got.iter().any(|a| matches!(a, Attachment::SystemReminder { text } if text.contains("was not attached"))),
            "被挡下的要告诉模型"
        );
    }
}
