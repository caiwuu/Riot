//! Bash 命令前后的文件比对：把 shell 里按字面写 / 删的文件记进会话基线。
//!
//! Write / Edit / Delete 自己记基线（`note_baseline`），Bash 不记 —— 而模型
//! 复制一个文件天然会用 `cp`，于是改动栏看不见新文件、回退也不删它。
//! 照 Cursor 的边界，终端改动本来不盖；这里盖的只是**字面路径**的那几条
//! 命令（候选由 [`riot_permissions::bash::effects`] 从语法树里读出来），
//! 既不监听磁盘也不扫目录 —— 每条命令只多几次 metadata / read。脚本、
//! 生成器、`npm install`、`git checkout` 照旧看不见。
//!
//! 做法：执行前拍候选路径的**前像**，执行后再看一眼，变了才记。命令没碰到
//! 的候选（`sed` 的脚本被误当路径、`cp a d` 里 `d` 其实是目录）前后一致，
//! 自然落空 —— 所以提取那侧可以宁多勿少。
//!
//! `[约束]` 前像只认文本：基线是 `Option<String>`。本来是二进制的不记
//! （和 Delete 拒删二进制同一个理由）；本来不存在、之后成了二进制的记
//! `None` —— 回退会把它删掉，这是对的；前进时切片按 `skipped: binary`
//! 报出来。本来不存在、之后成了**目录**的不记：目录进不了切片，记了只会
//! 让回退多一条"读不了"。
//!
//! `[约束]` 记的是磁盘**原样**（含 BOM、保留 CRLF），理由见 `delete.rs`。

use std::path::{Path, PathBuf};

use riot_permissions::bash::FileOp;
use riot_protocol::tool::ToolContext;

use super::path;
use super::text;

/// 前像里正文的体积上限。超过的文件不记 —— 基线要整份放进内存和 sidecar，
/// 而回退那边的切片本来也不收 1 MiB 以上的文件。
const MAX_BASELINE_BYTES: u64 = 1024 * 1024;

/// 一条命令最多盯多少个候选路径。`rm a b c …` 几十个文件是正常的，几百个
/// 多半是生成出来的清单，按字面认没有意义。
const MAX_CANDIDATES: usize = 128;

/// 执行前拍下的前像。`None` = 那时文件不存在。
pub(crate) struct PreImages(Vec<(PathBuf, Option<String>)>);

impl PreImages {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// 一个路径此刻在磁盘上的样子。
enum Disk {
    Absent,
    Text(String),
    /// 存在，但记不了正文：二进制、超大、读不了。
    Opaque,
    Dir,
    /// 元信息都拿不到（权限之类）。前后都不能据此说"变了"。
    Unknown,
}

async fn disk_state(path: &Path, ctx: &ToolContext) -> Disk {
    let meta = match ctx.fs.metadata(path).await {
        Ok(m) => m,
        // `cp a b` 的候选之一是 `b/a`；b 成了普通文件之后这条路径报的是
        // ENOTDIR，不是 NotFound —— 一样是"不存在"，当成存在会记出一条
        // 假基线，回退时按它去删一个不存在的文件、报一条莫名的失败。
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Disk::Absent;
        }
        Err(_) => return Disk::Unknown,
    };
    if meta.is_dir {
        return Disk::Dir;
    }
    if meta.len > MAX_BASELINE_BYTES {
        return Disk::Opaque;
    }
    let Ok(bytes) = ctx.fs.read(path).await else {
        return Disk::Opaque;
    };
    if text::decode(&bytes).is_err() {
        return Disk::Opaque;
    }
    match String::from_utf8(bytes) {
        Ok(s) => Disk::Text(s),
        Err(_) => Disk::Opaque,
    }
}

/// 执行前：把命令的候选路径解析到绝对路径，拍前像。
///
/// 解析走和 Write 相同的 [`path::resolve`]（父目录 canonicalize），基线里的
/// 路径口径才和工具写的一致；形状可疑的路径直接跳过。
pub(crate) async fn capture(command: &str, ctx: &ToolContext) -> PreImages {
    let ops = riot_permissions::bash::file_effects(command);
    if ops.is_empty() {
        return PreImages(Vec::new());
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    let push = |p: PathBuf, candidates: &mut Vec<PathBuf>| {
        if !candidates.contains(&p) {
            candidates.push(p);
        }
    };

    for op in ops {
        match op {
            FileOp::Write(p) | FileOp::Remove(p) => {
                if let Some(abs) = resolve(&p, ctx).await {
                    push(abs, &mut candidates);
                }
            }
            FileOp::Transfer {
                sources,
                dest,
                moves,
            } => {
                let Some(dest_abs) = resolve(&dest, ctx).await else {
                    continue;
                };
                let dest_is_dir = dest.ends_with('/')
                    || dest.ends_with('\\')
                    || ctx.fs.metadata(&dest_abs).await.is_ok_and(|m| m.is_dir);
                for s in &sources {
                    if moves && let Some(src_abs) = resolve(s, ctx).await {
                        push(src_abs, &mut candidates);
                    }
                    let name = Path::new(s).file_name().map(PathBuf::from);
                    if dest_is_dir {
                        if let Some(name) = &name {
                            push(dest_abs.join(name), &mut candidates);
                        }
                    } else if sources.len() == 1 {
                        // 目标此刻不是目录。但同一条命令里可能先 `mkdir` 了它
                        // （`mkdir -p d && cp a d`）—— 两种落点都盯着，执行后
                        // 谁真的出现了谁算。
                        push(dest_abs.clone(), &mut candidates);
                        if let Some(name) = &name {
                            push(dest_abs.join(name), &mut candidates);
                        }
                    }
                }
            }
        }
        if candidates.len() > MAX_CANDIDATES {
            return PreImages(Vec::new());
        }
    }

    let mut pre = Vec::with_capacity(candidates.len());
    for p in candidates {
        match disk_state(&p, ctx).await {
            Disk::Absent => pre.push((p, None)),
            Disk::Text(s) => pre.push((p, Some(s))),
            // 记不了正文的不盯：改了也没法给回退一个能写回去的 v0。
            // 看不清的也不盯：说不出它之前是什么样。
            Disk::Opaque | Disk::Dir | Disk::Unknown => {}
        }
    }
    PreImages(pre)
}

/// 执行后：和前像比，变了的记基线、清先读后写缓存。
pub(crate) async fn settle(pre: PreImages, ctx: &ToolContext) {
    for (path, before) in pre.0 {
        let after = disk_state(&path, ctx).await;
        let changed = match (&before, &after) {
            // 看不清就不下结论
            (_, Disk::Unknown) => false,
            (None, Disk::Absent) => false,
            // 新建成了目录：目录进不了切片，不记
            (None, Disk::Dir) => false,
            // 新建（文本或二进制）
            (None, Disk::Text(_) | Disk::Opaque) => true,
            (Some(b), Disk::Text(a)) => b != a,
            // 删了 / 变成二进制或目录了
            (Some(_), Disk::Absent | Disk::Opaque | Disk::Dir) => true,
        };
        if !changed {
            continue;
        }
        // 同一个文件本会话先改过的话不会覆盖最初那份（只有第一次算数）。
        ctx.file_state.note_baseline(path.clone(), before);
        // 缓存里那份内容已经不是磁盘上的了。Edit 本来会按 mtime 重读，
        // 但 mtime 精度只有秒的文件系统上同一秒内改两次它看不出来。
        ctx.file_state.invalidate(&path);
    }
}

async fn resolve(raw: &str, ctx: &ToolContext) -> Option<PathBuf> {
    path::resolve(raw, ctx, false).await.ok()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pretty_assertions::assert_eq;
    use riot_protocol::id::{SessionId, ToolUseId};
    use riot_protocol::tool::{FileStateCache, FileSystem};
    use tokio_util::sync::CancellationToken;

    use super::super::memfs::{MemFileState, MemFs};
    use super::*;

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
            proc: Arc::new(crate::testing::NullProc),
            web: Arc::new(riot_protocol::web::NoWeb),
            browser: Arc::new(riot_protocol::browser::NoBrowser),
            terminal: Arc::new(riot_protocol::terminal::NoTerminal),
            vision: Arc::new(riot_protocol::vision::NoVision),
            clock: Arc::new(crate::testing::FixedClock::default()),
        };
        Harness { fs, state, ctx }
    }

    fn base() -> MemFs {
        MemFs::new().with_dir("/work").with_dir("/work/dir")
    }

    fn baseline(h: &Harness, p: &str) -> Option<Option<String>> {
        h.state
            .baselines()
            .into_iter()
            .find(|(path, _)| path == Path::new(p))
            .map(|(_, b)| b)
    }

    /// 用户撞上的那条：`cp` 出来的新文件要像 Write 新建的一样记 `None`。
    #[tokio::test]
    async fn cp_新建的文件记成新增() {
        let h = harness(base().with_file("/work/a.txt", "hello\n"));
        let pre = capture("cp a.txt a-copy.txt && ls -la a*.txt", &h.ctx).await;
        assert!(!pre.is_empty());
        // 模拟 shell 干的事
        h.fs.put("/work/a-copy.txt", "hello\n", 2000);
        settle(pre, &h.ctx).await;

        assert_eq!(baseline(&h, "/work/a-copy.txt"), Some(None));
        assert_eq!(baseline(&h, "/work/a.txt"), None, "源文件没动，不记");
        // 另一个落点候选 `a-copy.txt/a.txt`：目标成了普通文件之后它报的是
        // ENOTDIR。当成"存在"就会记出一条假基线，回退时按它去删、报失败
        assert_eq!(
            h.state.baselines().len(),
            1,
            "只有副本一条：{:?}",
            h.state.baselines()
        );
    }

    #[tokio::test]
    async fn cp_进目录_落点是目录加文件名() {
        let h = harness(base().with_file("/work/a.txt", "x"));
        let pre = capture("cp a.txt dir", &h.ctx).await;
        h.fs.put("/work/dir/a.txt", "x", 2000);
        settle(pre, &h.ctx).await;
        assert_eq!(baseline(&h, "/work/dir/a.txt"), Some(None));
        assert_eq!(baseline(&h, "/work/dir"), None, "目录本身不记");
    }

    #[tokio::test]
    async fn mv_源记删除_目标记新增() {
        let h = harness(base().with_file("/work/old.rs", "fn a() {}\n"));
        let pre = capture("mv old.rs new.rs", &h.ctx).await;
        h.fs.remove_file(Path::new("/work/old.rs")).await.unwrap();
        h.fs.put("/work/new.rs", "fn a() {}\n", 2000);
        settle(pre, &h.ctx).await;

        assert_eq!(
            baseline(&h, "/work/old.rs"),
            Some(Some("fn a() {}\n".into())),
            "源文件删前的正文是基线，回退才写得回来"
        );
        assert_eq!(baseline(&h, "/work/new.rs"), Some(None));
    }

    #[tokio::test]
    async fn 重定向覆盖已有文件_记改前正文() {
        let h = harness(base().with_file("/work/out.txt", "v0"));
        let pre = capture("echo v1 > out.txt", &h.ctx).await;
        h.fs.put("/work/out.txt", "v1\n", 2000);
        settle(pre, &h.ctx).await;
        assert_eq!(baseline(&h, "/work/out.txt"), Some(Some("v0".into())));
    }

    #[tokio::test]
    async fn rm_单个文件_记删前正文_目录跳过() {
        let h = harness(
            base()
                .with_file("/work/a.txt", "gone")
                .with_file("/work/dir/inner.txt", "keep"),
        );
        let pre = capture("rm -rf a.txt dir", &h.ctx).await;
        h.fs.remove_file(Path::new("/work/a.txt")).await.unwrap();
        settle(pre, &h.ctx).await;
        assert_eq!(baseline(&h, "/work/a.txt"), Some(Some("gone".into())));
        assert_eq!(baseline(&h, "/work/dir"), None, "目录记不了");
    }

    #[tokio::test]
    async fn 命令没碰到的候选不记() {
        // sed 的脚本被当成了一个"文件"：前后都不存在，落空
        let h = harness(base().with_file("/work/f.txt", "a"));
        let pre = capture("sed -i '' 's/a/b/' f.txt", &h.ctx).await;
        h.fs.put("/work/f.txt", "b", 2000);
        settle(pre, &h.ctx).await;
        assert_eq!(baseline(&h, "/work/f.txt"), Some(Some("a".into())));
        assert!(
            h.state.baselines().len() == 1,
            "只有真改了的那个：{:?}",
            h.state.baselines()
        );
    }

    #[tokio::test]
    async fn 前后一致什么都不记() {
        let h = harness(base().with_file("/work/a.txt", "same"));
        let pre = capture("cp a.txt b.txt", &h.ctx).await;
        // 命令失败了，什么都没发生
        settle(pre, &h.ctx).await;
        assert!(h.state.baselines().is_empty());
    }

    #[tokio::test]
    async fn 二进制前像不盯_新建二进制记新增() {
        let h = harness(base().with_file("/work/img.png", b"\x89PNG\0\0"));
        let pre = capture("cp img.png copy.png && echo x > img.png", &h.ctx).await;
        h.fs.put("/work/copy.png", b"\x89PNG\0\0", 2000);
        h.fs.put("/work/img.png", "x\n", 2000);
        settle(pre, &h.ctx).await;
        assert_eq!(
            baseline(&h, "/work/img.png"),
            None,
            "本来是二进制，记不了 v0，就不记"
        );
        assert_eq!(
            baseline(&h, "/work/copy.png"),
            Some(None),
            "新建的二进制记 None：回退会删掉它"
        );
    }

    #[tokio::test]
    async fn 先改过再被_shell_改_基线仍是最初那份() {
        let h = harness(base().with_file("/work/a.txt", "v1"));
        h.state
            .note_baseline(PathBuf::from("/work/a.txt"), Some("v0".into()));
        let pre = capture("echo v2 > a.txt", &h.ctx).await;
        h.fs.put("/work/a.txt", "v2\n", 2000);
        settle(pre, &h.ctx).await;
        assert_eq!(baseline(&h, "/work/a.txt"), Some(Some("v0".into())));
    }

    #[tokio::test]
    async fn 看不懂的命令不拍前像() {
        let h = harness(base().with_file("/work/a.txt", "x"));
        let pre = capture("cp a.txt $DEST", &h.ctx).await;
        assert!(pre.is_empty());
    }
}
