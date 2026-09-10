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
//!   每次切回会话都要问一遍。
//! - `blobs/<sha256>`：文件正文，内容寻址。
//!
//! # 为什么内容寻址
//!
//! 切片按提问拍，而两条提问之间绝大多数基线文件根本没动。正文内联的话，
//! 会话摸过 50 个文件、聊了 200 轮，`sessions/` 里就是上百 MB 一模一样的
//! 文本。所以切片 / head / redo 层里只记 `(路径, 哈希)`，正文按哈希存一份；
//! 同一份内容不管出现在多少个切片里，盘上只有一个 blob。切片文件本身
//! 于是只有几 KB，`redo.json` 也不再随基线文件数膨胀。
//!
//! blob 只增不改（同哈希同内容）。删靠 [`gc`]：扫一遍目录里所有 JSON 收集
//! 引用，没人引用的 blob 删掉。所有写入都在 `running` 下串行，gc 也在那里跑
//! （轮次结束、Restore / Redo 之后），不会和一次正在写的切片打架。
//!
//! 老格式（正文内联的 `text` 条目）只读不写：读回来照样能回退，之后拍的
//! 切片一律走 blob。
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
use sha2::Digest as _;

/// 单个文件超过这个体积就不进切片。回退不是备份工具，大文件 / 二进制
/// 留给用户自己处理，但必须标出来，不能假装回退成功。
pub const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;

pub const HEAD_NAME: &str = "head.json";
pub const REDO_NAME: &str = "redo.json";
pub const BLOBS_DIR: &str = "blobs";

const BINARY_PROBE: usize = 8192;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FileEntry {
    /// 正文在 `blobs/<hash>`。拍照只写这种。
    Blob {
        hash: String,
    },
    /// 老格式：正文内联。只读不写。
    Text {
        content: String,
    },
    Missing,
    Skipped {
        reason: String,
    },
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

/// 基线表里的一项：v0 正文也走 blob。`None` = 会话开始时文件不存在。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineRef {
    pub path: PathBuf,
    pub before: Option<String>,
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
    pub baselines: Vec<BaselineRef>,
    #[serde(default)]
    pub discarded_live: Vec<Message>,
    #[serde(default)]
    pub discarded_archived: Vec<Message>,
}

impl FileEntry {
    /// 正文哈希。内联的老格式现算，好和 blob 条目 / 磁盘状态直接比。
    fn hash(&self) -> Option<String> {
        match self {
            FileEntry::Blob { hash } => Some(hash.clone()),
            FileEntry::Text { content } => Some(hash_bytes(content.as_bytes())),
            FileEntry::Missing | FileEntry::Skipped { .. } => None,
        }
    }

    /// 两个状态是否等价：正文按哈希比，其余按变体比。
    fn same_as(&self, other: &FileEntry) -> bool {
        match (self, other) {
            (FileEntry::Missing, FileEntry::Missing) => true,
            (FileEntry::Skipped { reason: a }, FileEntry::Skipped { reason: b }) => a == b,
            (a, b) => match (a.hash(), b.hash()) {
                (Some(x), Some(y)) => x == y,
                _ => false,
            },
        }
    }
}

impl FileSnapshot {
    pub fn get(&self, path: &Path) -> Option<&FileEntry> {
        self.files.iter().find(|f| f.path == path).map(|f| &f.entry)
    }

    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.files.iter().map(|f| f.path.as_path())
    }

    fn blob_hashes(&self) -> impl Iterator<Item = &str> {
        self.files.iter().filter_map(|f| match &f.entry {
            FileEntry::Blob { hash } => Some(hash.as_str()),
            _ => None,
        })
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

fn blobs_dir(dir: &Path) -> PathBuf {
    dir.join(BLOBS_DIR)
}

fn blob_path(dir: &Path, hash: &str) -> PathBuf {
    blobs_dir(dir).join(hash)
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

fn hash_bytes(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ── blob 存取 ──────────────────────────────────────────────────

/// 存一份正文，返回哈希。
fn store_blob(dir: &Path, bytes: &[u8]) -> std::io::Result<String> {
    let hash = hash_bytes(bytes);
    store_blob_as(dir, &hash, bytes)?;
    Ok(hash)
}

/// 已有同哈希的就不再写 —— 这正是去重的落点。
fn store_blob_as(dir: &Path, hash: &str, bytes: &[u8]) -> std::io::Result<()> {
    let path = blob_path(dir, hash);
    if path.is_file() {
        return Ok(());
    }
    std::fs::create_dir_all(blobs_dir(dir))?;
    // 临时名带哈希：同一份内容不会和别的内容抢同一个临时文件。
    let tmp = blobs_dir(dir).join(format!("{hash}.tmp"));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &path)
}

fn read_blob(dir: &Path, hash: &str) -> std::io::Result<String> {
    std::fs::read_to_string(blob_path(dir, hash))
}

/// 条目的正文。`Ok(None)` = 这条不是正文（缺失 / 跳过）。blob 丢了报错，
/// 调用方要把它当成一次失败报出来，不能装成功。
fn content_of(dir: &Path, entry: &FileEntry) -> std::io::Result<Option<String>> {
    match entry {
        FileEntry::Blob { hash } => read_blob(dir, hash).map(Some),
        FileEntry::Text { content } => Ok(Some(content.clone())),
        FileEntry::Missing | FileEntry::Skipped { .. } => Ok(None),
    }
}

/// 基线表落成 blob 引用（v0 正文按哈希存一份）。存不上的那条降级成
/// `None` 会撒谎（"会话开始时不存在"），所以直接跳过并记日志。
pub fn store_baselines(dir: &Path, baselines: &[(PathBuf, Option<String>)]) -> Vec<BaselineRef> {
    let mut out = Vec::with_capacity(baselines.len());
    for (path, before) in baselines {
        let before = match before {
            None => None,
            Some(v0) => match store_blob(dir, v0.as_bytes()) {
                Ok(hash) => Some(hash),
                Err(e) => {
                    tracing::warn!(error = %e, path = %path.display(), "基线 v0 没存进 blob，redo 层里不带它");
                    continue;
                }
            },
        };
        out.push(BaselineRef {
            path: path.clone(),
            before,
        });
    }
    out
}

/// [`store_baselines`] 的反向。blob 丢了的那条跳过（补一个错的 v0 比不补更糟）。
pub fn resolve_baselines(dir: &Path, refs: &[BaselineRef]) -> Vec<(PathBuf, Option<String>)> {
    let mut out = Vec::with_capacity(refs.len());
    for r in refs {
        let before = match &r.before {
            None => None,
            Some(hash) => match read_blob(dir, hash) {
                Ok(v0) => Some(v0),
                Err(e) => {
                    tracing::warn!(error = %e, path = %r.path.display(), "基线 v0 的 blob 读不到，跳过这条");
                    continue;
                }
            },
        };
        out.push((r.path.clone(), before));
    }
    out
}

/// 删掉没人引用的 blob。引用 = 目录里所有 JSON（切片、head、redo 各层）
/// 提到的哈希。任何一个 JSON 读不出来就整轮放弃 —— 一次瞬时读错换来
/// 一批被误删的正文，回退就会莫名失败；漏删一轮没有任何代价。
pub fn gc(dir: &Path) {
    let blobs = blobs_dir(dir);
    let Ok(rd) = std::fs::read_dir(&blobs) else {
        return;
    };
    let Some(referenced) = referenced_hashes(dir) else {
        tracing::warn!(dir = %dir.display(), "检查点目录里有读不出的 JSON，这轮不清 blob");
        return;
    };
    let mut removed = 0usize;
    for entry in rd.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // 半截临时文件（上次写到一半崩了）也一起清：写入和 gc 在同一把锁下
        // 串行，此刻还在的 .tmp 不可能是正在写的。
        if !name.ends_with(".tmp") && referenced.contains(name) {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(e) => tracing::warn!(error = %e, path = %path.display(), "孤儿 blob 没删掉"),
        }
    }
    if removed > 0 {
        tracing::debug!(dir = %dir.display(), removed, "清掉了没人引用的 blob");
    }
}

fn referenced_hashes(dir: &Path) -> Option<HashSet<String>> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut out = HashSet::new();
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let raw = std::fs::read_to_string(&path).ok()?;
        if path.file_name().and_then(|n| n.to_str()) == Some(REDO_NAME) {
            let stack = parse_redo_stack(&raw)?;
            for dump in &stack {
                out.extend(dump.files.blob_hashes().map(str::to_owned));
                out.extend(dump.baselines.iter().filter_map(|b| b.before.clone()));
            }
        } else {
            let snap: FileSnapshot = serde_json::from_str(&raw).ok()?;
            out.extend(snap.blob_hashes().map(str::to_owned));
        }
    }
    Some(out)
}

// ── 切片 / redo 栈落盘 ─────────────────────────────────────────

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
    match parse_redo_stack(&raw) {
        Some(stack) => stack,
        None => {
            tracing::warn!(path = %path.display(), "redo.json 读不懂");
            Vec::new()
        }
    }
}

fn parse_redo_stack(raw: &str) -> Option<Vec<RedoDump>> {
    if let Ok(stack) = serde_json::from_str::<Vec<RedoDump>>(raw) {
        return Some(stack);
    }
    serde_json::from_str::<RedoDump>(raw)
        .ok()
        .map(|one| vec![one])
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
/// 切片留着只是孤儿。它引用的 blob 留给下一次 [`gc`]。
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

// ── 拍照 / 比对 / 写回 ─────────────────────────────────────────

/// 按当前基线路径拍一张磁盘切片，正文存进 `dir` 的 blob 库。
pub async fn capture(dir: &Path, paths: impl IntoIterator<Item = PathBuf>) -> FileSnapshot {
    let mut files = Vec::new();
    for path in paths {
        let entry = capture_entry(dir, &path).await;
        files.push(SnapFile { entry, path });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    FileSnapshot { files }
}

/// [`read_entry`] 加落盘：是正文就存进 blob 库。
async fn capture_entry(dir: &Path, path: &Path) -> FileEntry {
    let bytes = match tokio::fs::read(path).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return FileEntry::Missing,
        Err(_) => {
            return FileEntry::Skipped {
                reason: "unreadable".into(),
            };
        }
        Ok(bytes) => bytes,
    };
    match classify_bytes(&bytes) {
        FileEntry::Blob { hash } => match store_blob_as(dir, &hash, &bytes) {
            Ok(()) => FileEntry::Blob { hash },
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(), "正文没存进 blob");
                FileEntry::Skipped {
                    reason: "store_failed".into(),
                }
            }
        },
        other => other,
    }
}

/// 磁盘上这个文件此刻的状态。正文只算哈希、不落盘 —— 比对用。
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
    if looks_binary(bytes) || std::str::from_utf8(bytes).is_err() {
        return FileEntry::Skipped {
            reason: "binary".into(),
        };
    }
    FileEntry::Blob {
        hash: hash_bytes(bytes),
    }
}

fn looks_binary(bytes: &[u8]) -> bool {
    let n = bytes.len().min(BINARY_PROBE);
    bytes[..n].contains(&0)
}

/// 回退前给确认框的数字。`head` 没有时 `dirty` 为 false。只比哈希，不读 blob。
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
            want => {
                if !want.same_as(&read_entry(&f.path).await) {
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
        if !want.same_as(&read_entry(&path).await) {
            return true;
        }
    }
    false
}

/// 把切片写回磁盘，切片外的基线文件回到会话 v0（或删除）。
pub async fn apply(
    dir: &Path,
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
            entry => {
                let content = match content_of(dir, entry) {
                    Ok(Some(c)) => c,
                    Ok(None) => continue,
                    Err(e) => {
                        tracing::warn!(error = %e, path = %f.path.display(), "切片正文的 blob 读不到");
                        failed.push(RestoreSkip {
                            path: rel(root, &f.path),
                            reason: "blob_missing".into(),
                        });
                        continue;
                    }
                };
                match write_text(&f.path, &content).await {
                    Ok(true) => {
                        cache.invalidate(&f.path);
                        restored += 1;
                    }
                    Ok(false) => {}
                    Err(e) => failed.push(RestoreSkip {
                        path: rel(root, &f.path),
                        reason: e.to_string(),
                    }),
                }
            }
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

    fn blob(s: &str) -> FileEntry {
        FileEntry::Blob {
            hash: hash_bytes(s.as_bytes()),
        }
    }

    fn text(s: &str) -> FileEntry {
        FileEntry::Text { content: s.into() }
    }

    fn blob_count(ck: &Path) -> usize {
        std::fs::read_dir(blobs_dir(ck))
            .map(|rd| rd.flatten().count())
            .unwrap_or(0)
    }

    #[tokio::test]
    async fn 拍照能认出文本缺失和过大() {
        let dir = tmp();
        let ck = dir_of(dir.path(), "s1");
        let text_file = dir.path().join("a.rs");
        let gone = dir.path().join("gone.rs");
        let big = dir.path().join("big.bin");
        tokio::fs::write(&text_file, "hello\n").await.unwrap();
        tokio::fs::write(&big, vec![b'x'; MAX_SNAPSHOT_BYTES + 8])
            .await
            .unwrap();

        let snap = capture(&ck, [text_file.clone(), gone.clone(), big.clone()]).await;
        assert_eq!(snap.get(&text_file), Some(&blob("hello\n")));
        assert_eq!(
            read_blob(&ck, &hash_bytes(b"hello\n")).unwrap(),
            "hello\n",
            "正文要真的在 blob 库里"
        );
        assert_eq!(snap.get(&gone), Some(&FileEntry::Missing));
        assert_eq!(
            snap.get(&big),
            Some(&FileEntry::Skipped {
                reason: "too_large".into()
            })
        );
        assert_eq!(blob_count(&ck), 1, "缺失和过大的不占 blob");
    }

    #[tokio::test]
    async fn 二进制按开头nul跳过() {
        let dir = tmp();
        let ck = dir_of(dir.path(), "s1");
        let path = dir.path().join("pic.png");
        tokio::fs::write(&path, [0x89, 0x50, 0x4e, 0x47, 0x00, 0x01])
            .await
            .unwrap();
        let snap = capture(&ck, [path.clone()]).await;
        assert_eq!(
            snap.get(&path),
            Some(&FileEntry::Skipped {
                reason: "binary".into()
            })
        );
    }

    #[tokio::test]
    async fn 同一份内容只存一个blob() {
        let dir = tmp();
        let ck = dir_of(dir.path(), "s1");
        let a = dir.path().join("a.rs");
        let b = dir.path().join("b.rs");
        tokio::fs::write(&a, "same\n").await.unwrap();
        tokio::fs::write(&b, "same\n").await.unwrap();

        let s1 = capture(&ck, [a.clone(), b.clone()]).await;
        let s2 = capture(&ck, [a.clone(), b.clone()]).await;
        assert_eq!(s1, s2, "内容没变，两次切片一字不差");
        assert_eq!(
            blob_count(&ck),
            1,
            "两个文件、两次切片、同一份内容 = 一个 blob"
        );

        tokio::fs::write(&a, "changed\n").await.unwrap();
        let s3 = capture(&ck, [a.clone(), b.clone()]).await;
        assert_eq!(blob_count(&ck), 2, "只有变了的内容多占一个 blob");
        assert_eq!(s3.get(&a), Some(&blob("changed\n")));
        assert_eq!(s3.get(&b), Some(&blob("same\n")));
    }

    #[tokio::test]
    async fn 写回切片内修改新建删除() {
        let dir = tmp();
        let root = dir.path();
        let ck = dir_of(root, "s1");
        let edited = root.join("a.rs");
        let created = root.join("n.rs");
        let deleted = root.join("g.rs");
        tokio::fs::write(&edited, "old\n").await.unwrap();
        tokio::fs::write(&deleted, "keep\n").await.unwrap();

        let snap = capture(&ck, [edited.clone(), deleted.clone()]).await;

        tokio::fs::write(&edited, "new\n").await.unwrap();
        tokio::fs::write(&created, "fresh\n").await.unwrap();
        tokio::fs::remove_file(&deleted).await.unwrap();

        let cache = MemoryFileState::shared();
        cache.note_baseline(edited.clone(), Some("v0\n".into()));
        cache.note_baseline(created.clone(), None);
        cache.note_baseline(deleted.clone(), Some("keep\n".into()));

        let report = apply(&ck, &snap, &cache.baselines(), cache.as_ref(), root).await;
        assert_eq!(report.failed.len(), 0, "{report:?}");
        assert_eq!(tokio::fs::read_to_string(&edited).await.unwrap(), "old\n");
        assert!(!created.exists(), "切片之后新建的文件应删掉");
        assert_eq!(tokio::fs::read_to_string(&deleted).await.unwrap(), "keep\n");
        assert!(
            cache.baselines().iter().all(|(p, _)| p != &created),
            "切片之后才出现的新建文件基线要摘掉"
        );
    }

    #[tokio::test]
    async fn 老格式内联正文照样能写回和比对() {
        let dir = tmp();
        let root = dir.path();
        let ck = dir_of(root, "s1");
        let a = root.join("a.rs");
        tokio::fs::write(&a, "now\n").await.unwrap();
        let snap = FileSnapshot {
            files: vec![SnapFile {
                path: a.clone(),
                entry: text("old\n"),
            }],
        };

        let p = preview("u1", &snap, &[], None, root).await;
        assert_eq!(p.files, 1, "内联正文和磁盘不同，算一个要改的");

        let cache = MemoryFileState::shared();
        let report = apply(&ck, &snap, &[], cache.as_ref(), root).await;
        assert_eq!(report.failed.len(), 0, "{report:?}");
        assert_eq!(tokio::fs::read_to_string(&a).await.unwrap(), "old\n");
        assert!(
            text("old\n").same_as(&blob("old\n")),
            "老格式和 blob 条目按哈希等价"
        );
    }

    #[tokio::test]
    async fn blob丢了写回报失败不装成功() {
        let dir = tmp();
        let root = dir.path();
        let ck = dir_of(root, "s1");
        let a = root.join("a.rs");
        tokio::fs::write(&a, "now\n").await.unwrap();
        let snap = FileSnapshot {
            files: vec![SnapFile {
                path: a.clone(),
                entry: blob("never-stored\n"),
            }],
        };
        let cache = MemoryFileState::shared();
        let report = apply(&ck, &snap, &[], cache.as_ref(), root).await;
        assert_eq!(report.restored, 0);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].reason, "blob_missing");
        assert_eq!(
            tokio::fs::read_to_string(&a).await.unwrap(),
            "now\n",
            "写不回去就别动"
        );
    }

    #[tokio::test]
    async fn 切片外的文件回到会话v0() {
        let dir = tmp();
        let root = dir.path();
        let ck = dir_of(root, "s1");
        let later = root.join("later.rs");
        tokio::fs::write(&later, "after-c\n").await.unwrap();

        let cache = MemoryFileState::shared();
        cache.note_baseline(later.clone(), Some("v0\n".into()));

        let snap = FileSnapshot::default();
        let report = apply(&ck, &snap, &cache.baselines(), cache.as_ref(), root).await;
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
        let ck = dir_of(dir.path(), "s1");
        let path = dir.path().join("a.rs");
        tokio::fs::write(&path, "done\n").await.unwrap();
        let head = capture(&ck, [path.clone()]).await;
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
                entry: blob("x\n"),
            }],
        };
        save_snapshot(&slice_path(&ck, "u1"), &snap).unwrap();
        save_snapshot(&head_path(&ck), &FileSnapshot::default()).unwrap();
        push_redo(&ck, &RedoDump::default()).unwrap();
        std::fs::create_dir_all(blobs_dir(&ck)).unwrap();

        assert_eq!(load_snapshot(&slice_path(&ck, "u1")).unwrap(), snap);
        assert_eq!(
            list_ids(&ck),
            vec!["u1".to_string()],
            "head / redo / blobs 目录都不算提问切片"
        );
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
                    entry: blob(tag),
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
            Some(&blob("second"))
        );
        assert!(redo_available(&ck), "更早那层还在");
        let popped = pop_redo(&ck).expect("还有一层");
        assert_eq!(
            popped.files.get(Path::new("/work/a.rs")),
            Some(&blob("first"))
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

    #[test]
    fn 基线v0走blob并能解析回来() {
        let dir = tmp();
        let ck = dir_of(dir.path(), "s1");
        let baselines = vec![
            (PathBuf::from("/work/a.rs"), Some("v0\n".to_owned())),
            (PathBuf::from("/work/n.rs"), None),
        ];
        let refs = store_baselines(&ck, &baselines);
        assert_eq!(refs.len(), 2);
        assert_eq!(
            refs[0].before.as_deref(),
            Some(hash_bytes(b"v0\n").as_str())
        );
        assert_eq!(refs[1].before, None);
        assert_eq!(resolve_baselines(&ck, &refs), baselines);

        // blob 没了的那条跳过，不能补一个错的 v0。
        std::fs::remove_file(blob_path(&ck, &hash_bytes(b"v0\n"))).unwrap();
        assert_eq!(
            resolve_baselines(&ck, &refs),
            vec![(PathBuf::from("/work/n.rs"), None)]
        );
    }

    #[test]
    fn gc只删没人引用的blob() {
        let dir = tmp();
        let ck = dir_of(dir.path(), "s1");
        let h_slice = store_blob(&ck, b"in slice\n").unwrap();
        let h_head = store_blob(&ck, b"in head\n").unwrap();
        let h_redo = store_blob(&ck, b"in redo\n").unwrap();
        let h_v0 = store_blob(&ck, b"redo v0\n").unwrap();
        let h_orphan = store_blob(&ck, b"orphan\n").unwrap();
        std::fs::write(blobs_dir(&ck).join(format!("{h_orphan}.tmp")), b"half").unwrap();

        let snap = |hash: &str| FileSnapshot {
            files: vec![SnapFile {
                path: PathBuf::from("/work/a.rs"),
                entry: FileEntry::Blob {
                    hash: hash.to_owned(),
                },
            }],
        };
        save_snapshot(&slice_path(&ck, "u1"), &snap(&h_slice)).unwrap();
        save_snapshot(&head_path(&ck), &snap(&h_head)).unwrap();
        push_redo(
            &ck,
            &RedoDump {
                files: snap(&h_redo),
                baselines: vec![BaselineRef {
                    path: PathBuf::from("/work/b.rs"),
                    before: Some(h_v0.clone()),
                }],
                ..Default::default()
            },
        )
        .unwrap();

        gc(&ck);

        for h in [&h_slice, &h_head, &h_redo, &h_v0] {
            assert!(blob_path(&ck, h).is_file(), "有人引用的不能删：{h}");
        }
        assert!(!blob_path(&ck, &h_orphan).is_file(), "孤儿要删");
        assert_eq!(blob_count(&ck), 4, "半截临时文件也要清");
    }

    #[test]
    fn 有读不懂的json时gc整轮放弃() {
        let dir = tmp();
        let ck = dir_of(dir.path(), "s1");
        let h_orphan = store_blob(&ck, b"orphan\n").unwrap();
        std::fs::write(slice_path(&ck, "u1"), "{broken").unwrap();

        gc(&ck);

        assert!(
            blob_path(&ck, &h_orphan).is_file(),
            "一个 JSON 读不出来就不知道谁被引用，宁可漏删也不能误删"
        );
    }

    #[tokio::test]
    async fn 磁盘已一致的文件不重写也不计数() {
        let dir = tmp();
        let root = dir.path();
        let ck = dir_of(root, "s1");
        let same = root.join("same.rs");
        let diff = root.join("diff.rs");
        tokio::fs::write(&same, "x\n").await.unwrap();
        tokio::fs::write(&diff, "y\n").await.unwrap();
        store_blob(&ck, b"x\n").unwrap();
        let snap = FileSnapshot {
            files: vec![
                SnapFile {
                    path: same.clone(),
                    entry: blob("x\n"),
                },
                SnapFile {
                    path: diff.clone(),
                    entry: blob("x\n"),
                },
            ],
        };
        let before = std::fs::metadata(&same).unwrap().modified().unwrap();
        let cache = MemoryFileState::shared();
        let report = apply(&ck, &snap, &[], cache.as_ref(), root).await;
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
                    entry: blob("c\n"),
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
