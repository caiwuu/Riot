//! 用户气泡检查点：每个提问发出时的文件切片，用来按对话回退。
//!
//! 切片不进 transcript —— transcript 只记对话事实；文件底稿可能有整份
//! 源码那么大，而且和基线一样是给界面 / 回退用的对照，不是模型要看的历史。
//!
//! 目录：`sessions/<id>.checkpoints/`
//! - `<userMsgId>.json`：这条提问发出时、工具还没跑的磁盘切片
//! - `head.json`：最近一轮结束后的磁盘（脏检查用）
//! - `redo.json`：Restore 栈（每次回退压一层现场；前进弹一层）。
//!   连续回退再前进，更早那次回退还能再前进。
//!   `[不变量]` 文件存在 ⇔ 栈非空。`redo_available` 只看存在性，不解析 ——
//!   每次切回会话都要问一遍，而一层就是整份基线文件的全文。
//!
//! 豁免理由：读写的是用户项目里的真实文件和会话目录下的旁路切片，和
//! `changes.rs` 的基线一样不参与黄金回放。

#![allow(clippy::disallowed_methods)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use riot_protocol::changes::{RestorePreview, RestoreResult, RestoreSkip};
use riot_protocol::message::Message;
use riot_protocol::tool::FileStateCache;
use serde::{Deserialize, Serialize};

/// 单个文件超过这个体积就不进切片。回退不是备份工具，大文件 / 二进制
/// 留给用户自己处理，但必须标出来，不能假装回退成功。
pub const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;

pub const HEAD_NAME: &str = "head.json";
pub const REDO_NAME: &str = "redo.json";

const BINARY_PROBE: usize = 8192;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FileEntry {
    Text { content: String },
    Missing,
    Skipped { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapFile {
    pub path: PathBuf,
    pub entry: FileEntry,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSnapshot {
    #[serde(default)]
    pub files: Vec<SnapFile>,
}

/// Restore 之前的现场：磁盘切片 + 基线表 + 被截掉的对话。对话要另存一份，
/// 因为活会话里 Rewind 已经把它们从内存摘掉；重启重放靠 transcript 里的
/// `Unrewind`，不必再读这里。
///
/// 基线表也要带：回退会把「切片之后才新建」的文件删掉并 `forget_baseline`，
/// 前进只把文件写回来是不够的 —— 基线不回来，改动栏里它就永远消失了。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RedoDump {
    pub files: FileSnapshot,
    #[serde(default)]
    pub baselines: Vec<(PathBuf, Option<String>)>,
    #[serde(default)]
    pub discarded_live: Vec<Message>,
    #[serde(default)]
    pub discarded_archived: Vec<Message>,
}

impl FileSnapshot {
    pub fn get(&self, path: &Path) -> Option<&FileEntry> {
        self.files.iter().find(|f| f.path == path).map(|f| &f.entry)
    }

    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.files.iter().map(|f| f.path.as_path())
    }
}

pub fn dir_of(sessions_dir: &Path, id: &str) -> PathBuf {
    sessions_dir.join(format!("{id}.checkpoints"))
}

pub fn slice_path(dir: &Path, user_msg_id: &str) -> PathBuf {
    dir.join(format!("{}.json", sanitize_id(user_msg_id)))
}

pub fn head_path(dir: &Path) -> PathBuf {
    dir.join(HEAD_NAME)
}

pub fn redo_path(dir: &Path) -> PathBuf {
    dir.join(REDO_NAME)
}

fn sanitize_id(id: &str) -> &str {
    if id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && !id.is_empty()
    {
        id
    } else {
        "_"
    }
}

pub fn save_snapshot(path: &Path, snap: &FileSnapshot) -> std::io::Result<()> {
    write_json(path, snap)
}

pub fn load_snapshot(path: &Path) -> Option<FileSnapshot> {
    read_json(path)
}

/// 压一层现场。盘上是数组；老格式单对象当只有一层。
pub fn push_redo(dir: &Path, dump: &RedoDump) -> std::io::Result<()> {
    let mut stack = load_redo_stack(dir);
    stack.push(dump.clone());
    write_json(&redo_path(dir), &stack)
}

/// 弹出最近一层。空了就删文件 —— 读不懂的文件也删，否则 `redo_available`
/// 会一直说有、前进却一直失败。
pub fn pop_redo(dir: &Path) -> Option<RedoDump> {
    let mut stack = load_redo_stack(dir);
    let dump = stack.pop();
    if stack.is_empty() {
        clear_redo(dir);
    } else if let Err(e) = write_json(&redo_path(dir), &stack) {
        tracing::warn!(error = %e, "redo 栈写回失败，前进之后可能丢更早的层");
    }
    dump
}

fn load_redo_stack(dir: &Path) -> Vec<RedoDump> {
    let path = redo_path(dir);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "redo.json 读失败");
            return Vec::new();
        }
    };
    if let Ok(stack) = serde_json::from_str::<Vec<RedoDump>>(&raw) {
        return stack;
    }
    if let Ok(one) = serde_json::from_str::<RedoDump>(&raw) {
        return vec![one];
    }
    tracing::warn!(path = %path.display(), "redo.json 读不懂");
    Vec::new()
}

/// 只看文件在不在（见模块文档的不变量）。`push_redo` 只在写成功时留下
/// 文件，`pop_redo` 弹空或读不懂就删，所以存在 ⇔ 有层可弹。
pub fn redo_available(dir: &Path) -> bool {
    redo_path(dir).is_file()
}

pub fn clear_redo(dir: &Path) {
    let path = redo_path(dir);
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(error = %e, path = %path.display(), "redo.json 没删掉");
    }
}

/// 删掉一条提问的切片。提问被撤回（模型没开口就停）时用：消息不在了，
/// 切片留着只是孤儿。
pub fn remove_slice(dir: &Path, user_msg_id: &str) {
    let path = slice_path(dir, user_msg_id);
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(error = %e, path = %path.display(), "撤回的提问切片没删掉");
    }
}

pub fn list_ids(dir: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if stem == "head" || stem == "redo" || stem == "_" {
            continue;
        }
        out.push(stem.to_owned());
    }
    out.sort();
    out
}

pub fn remove_all(dir: &Path) {
    if let Err(e) = std::fs::remove_dir_all(dir)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(error = %e, path = %dir.display(), "检查点目录没删干净");
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string(value).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "检查点读失败");
            return None;
        }
    };
    match serde_json::from_str(&raw) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "检查点读不懂");
            None
        }
    }
}

/// 按当前基线路径拍一张磁盘切片。
pub async fn capture(paths: impl IntoIterator<Item = PathBuf>) -> FileSnapshot {
    let mut files = Vec::new();
    for path in paths {
        files.push(SnapFile {
            entry: read_entry(&path).await,
            path,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    FileSnapshot { files }
}

async fn read_entry(path: &Path) -> FileEntry {
    match tokio::fs::read(path).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => FileEntry::Missing,
        Err(_) => FileEntry::Skipped {
            reason: "unreadable".into(),
        },
        Ok(bytes) => classify_bytes(&bytes),
    }
}

fn classify_bytes(bytes: &[u8]) -> FileEntry {
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return FileEntry::Skipped {
            reason: "too_large".into(),
        };
    }
    if looks_binary(bytes) {
        return FileEntry::Skipped {
            reason: "binary".into(),
        };
    }
    match String::from_utf8(bytes.to_vec()) {
        Ok(content) => FileEntry::Text { content },
        Err(_) => FileEntry::Skipped {
            reason: "binary".into(),
        },
    }
}

fn looks_binary(bytes: &[u8]) -> bool {
    let n = bytes.len().min(BINARY_PROBE);
    bytes[..n].contains(&0)
}

/// 回退前给确认框的数字。`head` 没有时 `dirty` 为 false。
pub async fn preview(
    message_id: &str,
    snap: &FileSnapshot,
    current: &[(PathBuf, Option<String>)],
    head: Option<&FileSnapshot>,
    root: &Path,
) -> RestorePreview {
    let mut skipped = Vec::new();
    let mut files = 0usize;
    let mut after_slice = 0usize;
    let snap_paths: HashSet<&Path> = snap.paths().collect();

    for f in &snap.files {
        match &f.entry {
            FileEntry::Skipped { reason } => skipped.push(RestoreSkip {
                path: rel(root, &f.path),
                reason: reason.clone(),
            }),
            FileEntry::Text { content } => {
                let disk = tokio::fs::read_to_string(&f.path).await.ok();
                if disk.as_deref() != Some(content) {
                    files += 1;
                }
            }
            FileEntry::Missing => {
                if tokio::fs::try_exists(&f.path).await.unwrap_or(false) {
                    files += 1;
                }
            }
        }
    }

    for (path, before) in current {
        if snap_paths.contains(path.as_path()) {
            continue;
        }
        after_slice += 1;
        let disk = tokio::fs::read_to_string(path).await.ok();
        if disk.as_deref() != before.as_deref() {
            files += 1;
        }
    }

    RestorePreview {
        message_id: message_id.to_owned(),
        files,
        dirty: is_dirty(head, current).await,
        skipped,
        after_slice,
    }
}

async fn is_dirty(head: Option<&FileSnapshot>, current: &[(PathBuf, Option<String>)]) -> bool {
    let Some(head) = head else {
        return false;
    };
    let mut paths: HashSet<PathBuf> = head.files.iter().map(|f| f.path.clone()).collect();
    for (p, _) in current {
        paths.insert(p.clone());
    }
    for path in paths {
        let want = head.get(&path).cloned().unwrap_or(FileEntry::Missing);
        if want != read_entry(&path).await {
            return true;
        }
    }
    false
}

/// 把切片写回磁盘，切片外的基线文件回到会话 v0（或删除）。
pub async fn apply(
    snap: &FileSnapshot,
    current: &[(PathBuf, Option<String>)],
    cache: &dyn FileStateCache,
    root: &Path,
) -> RestoreResult {
    let mut restored = 0usize;
    let mut deleted = 0usize;
    let mut skipped = Vec::new();
    let mut failed = Vec::new();
    let snap_paths: HashSet<&Path> = snap.paths().collect();

    for f in &snap.files {
        match &f.entry {
            FileEntry::Skipped { reason } => {
                skipped.push(RestoreSkip {
                    path: rel(root, &f.path),
                    reason: reason.clone(),
                });
            }
            FileEntry::Text { content } => match write_text(&f.path, content).await {
                Ok(true) => {
                    cache.invalidate(&f.path);
                    restored += 1;
                }
                Ok(false) => {}
                Err(e) => failed.push(RestoreSkip {
                    path: rel(root, &f.path),
                    reason: e.to_string(),
                }),
            },
            FileEntry::Missing => match remove_file(&f.path).await {
                Ok(true) => {
                    cache.invalidate(&f.path);
                    deleted += 1;
                }
                Ok(false) => {}
                Err(e) => failed.push(RestoreSkip {
                    path: rel(root, &f.path),
                    reason: e.to_string(),
                }),
            },
        }
    }

    for (path, before) in current {
        if snap_paths.contains(path.as_path()) {
            continue;
        }
        match before {
            None => match remove_file(path).await {
                Ok(true) => {
                    cache.invalidate(path);
                    cache.forget_baseline(path);
                    deleted += 1;
                }
                Ok(false) => {
                    cache.forget_baseline(path);
                }
                Err(e) => failed.push(RestoreSkip {
                    path: rel(root, path),
                    reason: e.to_string(),
                }),
            },
            Some(v0) => match write_text(path, v0).await {
                Ok(true) => {
                    cache.invalidate(path);
                    restored += 1;
                }
                Ok(false) => {}
                Err(e) => failed.push(RestoreSkip {
                    path: rel(root, path),
                    reason: e.to_string(),
                }),
            },
        }
    }

    RestoreResult {
        restored,
        deleted,
        skipped,
        failed,
        redo_available: false,
    }
}

/// 磁盘已经一样就不写：`restored` 和预览里的 `files` 口径一致，也不白刷
/// mtime 去惊动 watcher / 增量构建。返回是否真写了。
async fn write_text(path: &Path, content: &str) -> std::io::Result<bool> {
    if let Ok(disk) = tokio::fs::read(path).await
        && disk == content.as_bytes()
    {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    tokio::fs::write(path, content.as_bytes()).await?;
    Ok(true)
}

async fn remove_file(path: &Path) -> std::io::Result<bool> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use riot_runtime::MemoryFileState;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("临时目录")
    }

    #[tokio::test]
    async fn 拍照能认出文本缺失和过大() {
        let dir = tmp();
        let text = dir.path().join("a.rs");
        let gone = dir.path().join("gone.rs");
        let big = dir.path().join("big.bin");
        tokio::fs::write(&text, "hello\n").await.unwrap();
        tokio::fs::write(&big, vec![b'x'; MAX_SNAPSHOT_BYTES + 8])
            .await
            .unwrap();

        let snap = capture([text.clone(), gone.clone(), big.clone()]).await;
        assert_eq!(
            snap.get(&text),
            Some(&FileEntry::Text {
                content: "hello\n".into()
            })
        );
        assert_eq!(snap.get(&gone), Some(&FileEntry::Missing));
        assert_eq!(
            snap.get(&big),
            Some(&FileEntry::Skipped {
                reason: "too_large".into()
            })
        );
    }

    #[tokio::test]
    async fn 二进制按开头nul跳过() {
        let dir = tmp();
        let path = dir.path().join("pic.png");
        tokio::fs::write(&path, [0x89, 0x50, 0x4e, 0x47, 0x00, 0x01])
            .await
            .unwrap();
        let snap = capture([path.clone()]).await;
        assert_eq!(
            snap.get(&path),
            Some(&FileEntry::Skipped {
                reason: "binary".into()
            })
        );
    }

    #[tokio::test]
    async fn 写回切片内修改新建删除() {
        let dir = tmp();
        let root = dir.path();
        let edited = root.join("a.rs");
        let created = root.join("n.rs");
        let deleted = root.join("g.rs");
        tokio::fs::write(&edited, "old\n").await.unwrap();
        tokio::fs::write(&deleted, "keep\n").await.unwrap();

        let snap = FileSnapshot {
            files: vec![
                SnapFile {
                    path: edited.clone(),
                    entry: FileEntry::Text {
                        content: "old\n".into(),
                    },
                },
                SnapFile {
                    path: deleted.clone(),
                    entry: FileEntry::Text {
                        content: "keep\n".into(),
                    },
                },
            ],
        };

        tokio::fs::write(&edited, "new\n").await.unwrap();
        tokio::fs::write(&created, "fresh\n").await.unwrap();
        tokio::fs::remove_file(&deleted).await.unwrap();

        let cache = MemoryFileState::shared();
        cache.note_baseline(edited.clone(), Some("v0\n".into()));
        cache.note_baseline(created.clone(), None);
        cache.note_baseline(deleted.clone(), Some("keep\n".into()));

        let report = apply(&snap, &cache.baselines(), cache.as_ref(), root).await;
        assert_eq!(report.failed.len(), 0);
        assert_eq!(tokio::fs::read_to_string(&edited).await.unwrap(), "old\n");
        assert!(!created.exists(), "切片之后新建的文件应删掉");
        assert_eq!(tokio::fs::read_to_string(&deleted).await.unwrap(), "keep\n");
        assert!(
            cache.baselines().iter().all(|(p, _)| p != &created),
            "切片之后才出现的新建文件基线要摘掉"
        );
    }

    #[tokio::test]
    async fn 切片外的文件回到会话v0() {
        let dir = tmp();
        let root = dir.path();
        let later = root.join("later.rs");
        tokio::fs::write(&later, "after-c\n").await.unwrap();

        let cache = MemoryFileState::shared();
        cache.note_baseline(later.clone(), Some("v0\n".into()));

        let snap = FileSnapshot::default();
        let report = apply(&snap, &cache.baselines(), cache.as_ref(), root).await;
        assert_eq!(report.failed.len(), 0);
        assert_eq!(tokio::fs::read_to_string(&later).await.unwrap(), "v0\n");
        assert!(
            cache.baselines().iter().any(|(p, _)| p == &later),
            "会话开始就有的文件基线保留，改动栏会发现它回到了 v0"
        );
    }

    #[tokio::test]
    async fn 脏检查对着head() {
        let dir = tmp();
        let path = dir.path().join("a.rs");
        tokio::fs::write(&path, "done\n").await.unwrap();
        let head = capture([path.clone()]).await;
        assert!(!is_dirty(Some(&head), &[(path.clone(), Some("v0\n".into()))]).await);

        tokio::fs::write(&path, "hand\n").await.unwrap();
        assert!(is_dirty(Some(&head), &[(path.clone(), Some("v0\n".into()))]).await);
        assert!(!is_dirty(None, &[(path, Some("v0\n".into()))]).await);
    }

    #[test]
    fn 落盘能读回且列出用户切片() {
        let dir = tmp();
        let ck = dir_of(dir.path(), "s1");
        let snap = FileSnapshot {
            files: vec![SnapFile {
                path: PathBuf::from("/work/a.rs"),
                entry: FileEntry::Text {
                    content: "x\n".into(),
                },
            }],
        };
        save_snapshot(&slice_path(&ck, "u1"), &snap).unwrap();
        save_snapshot(&head_path(&ck), &FileSnapshot::default()).unwrap();
        push_redo(&ck, &RedoDump::default()).unwrap();

        assert_eq!(load_snapshot(&slice_path(&ck, "u1")).unwrap(), snap);
        assert_eq!(list_ids(&ck), vec!["u1".to_string()]);
        assert!(redo_available(&ck));
        clear_redo(&ck);
        assert!(!redo_available(&ck));
        remove_all(&ck);
        assert!(!ck.exists(), "删会话要把整个检查点目录摘掉");
    }

    #[test]
    fn 连续回退压栈_前进只弹一层() {
        let dir = tmp();
        let ck = dir_of(dir.path(), "s1");
        let dump = |tag: &str| RedoDump {
            files: FileSnapshot {
                files: vec![SnapFile {
                    path: PathBuf::from("/work/a.rs"),
                    entry: FileEntry::Text {
                        content: tag.into(),
                    },
                }],
            },
            ..Default::default()
        };
        push_redo(&ck, &dump("first")).unwrap();
        push_redo(&ck, &dump("second")).unwrap();
        assert!(redo_available(&ck));
        let popped = pop_redo(&ck).expect("有一层");
        assert_eq!(
            popped.files.get(Path::new("/work/a.rs")),
            Some(&FileEntry::Text {
                content: "second".into()
            })
        );
        assert!(redo_available(&ck), "更早那层还在");
        let popped = pop_redo(&ck).expect("还有一层");
        assert_eq!(
            popped.files.get(Path::new("/work/a.rs")),
            Some(&FileEntry::Text {
                content: "first".into()
            })
        );
        assert!(!redo_available(&ck));
    }

    #[test]
    fn 老格式单对象redo仍能读成一层() {
        let dir = tmp();
        let ck = dir_of(dir.path(), "s1");
        std::fs::create_dir_all(&ck).unwrap();
        let one = RedoDump::default();
        write_json(&redo_path(&ck), &one).unwrap();
        assert!(redo_available(&ck));
        assert!(pop_redo(&ck).is_some());
        assert!(!redo_available(&ck));
    }

    #[test]
    fn 读不懂的redo弹一次就清掉() {
        let dir = tmp();
        let ck = dir_of(dir.path(), "s1");
        std::fs::create_dir_all(&ck).unwrap();
        std::fs::write(redo_path(&ck), "{not json").unwrap();
        assert!(redo_available(&ck), "文件在就先说有，交给 pop 去判");
        assert!(pop_redo(&ck).is_none());
        assert!(
            !redo_available(&ck),
            "坏文件必须删掉，否则界面一直画前进、点了一直失败"
        );
    }

    #[test]
    fn 撤回删切片() {
        let dir = tmp();
        let ck = dir_of(dir.path(), "s1");
        save_snapshot(&slice_path(&ck, "u1"), &FileSnapshot::default()).unwrap();
        save_snapshot(&slice_path(&ck, "u2"), &FileSnapshot::default()).unwrap();
        remove_slice(&ck, "u1");
        remove_slice(&ck, "never-there");
        assert_eq!(list_ids(&ck), vec!["u2".to_string()]);
    }

    #[tokio::test]
    async fn 磁盘已一致的文件不重写也不计数() {
        let dir = tmp();
        let root = dir.path();
        let same = root.join("same.rs");
        let diff = root.join("diff.rs");
        tokio::fs::write(&same, "x\n").await.unwrap();
        tokio::fs::write(&diff, "y\n").await.unwrap();
        let text = |s: &str| FileEntry::Text { content: s.into() };
        let snap = FileSnapshot {
            files: vec![
                SnapFile {
                    path: same.clone(),
                    entry: text("x\n"),
                },
                SnapFile {
                    path: diff.clone(),
                    entry: text("x\n"),
                },
            ],
        };
        let before = std::fs::metadata(&same).unwrap().modified().unwrap();
        let cache = MemoryFileState::shared();
        let report = apply(&snap, &[], cache.as_ref(), root).await;
        assert_eq!(report.restored, 1, "只有真变了的那个算写回：{report:?}");
        assert_eq!(
            std::fs::metadata(&same).unwrap().modified().unwrap(),
            before
        );
        assert_eq!(tokio::fs::read_to_string(&diff).await.unwrap(), "x\n");
    }

    #[test]
    fn 非法id不会写出路径穿越() {
        let dir = tmp();
        let ck = dir_of(dir.path(), "s1");
        let path = slice_path(&ck, "../evil");
        assert_eq!(path.file_name().unwrap(), "_.json");
    }

    #[tokio::test]
    async fn 预览会计切片外和跳过() {
        let dir = tmp();
        let root = dir.path();
        let a = root.join("a.rs");
        let later = root.join("later.rs");
        tokio::fs::write(&a, "now\n").await.unwrap();
        tokio::fs::write(&later, "d\n").await.unwrap();

        let snap = FileSnapshot {
            files: vec![
                SnapFile {
                    path: a.clone(),
                    entry: FileEntry::Text {
                        content: "c\n".into(),
                    },
                },
                SnapFile {
                    path: root.join("pic.png"),
                    entry: FileEntry::Skipped {
                        reason: "binary".into(),
                    },
                },
            ],
        };
        let current = vec![(a, Some("v0\n".into())), (later, None)];
        let p = preview("u1", &snap, &current, None, root).await;
        assert_eq!(p.files, 2);
        assert_eq!(p.after_slice, 1);
        assert_eq!(p.skipped.len(), 1);
        assert!(!p.dirty);
    }
}
