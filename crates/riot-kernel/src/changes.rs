//! 本次会话改了哪些文件、哪些行。
//!
//! # 为什么不用 `git diff`
//!
//! 用户问的是"**这个会话**动了什么，有没有手滑多改"。`git diff` 回答的
//! 是另一个问题 —— 工作区相对 HEAD 的全部差异，里面混着用户自己没提交
//! 的改动、别的会话留下的改动。两者混在一起，恰恰答不上原来那个问题。
//!
//! 而 Edit / Write 都强制先读（见 `precondition::check_fresh`），所以每个
//! 被改的文件在动手之前必然经过文件状态缓存 —— 那份内容就是天然的基线，
//! 精确到"本会话经工具写下的改动"。项目不在 git 里也照样能用。
//!
//! `[已知局限]` 模型用 Bash 重定向写文件（`echo > f`）绕过了工具层，
//! 这里看不见。这是刻意的取舍：抓 Bash 的写入要解析 shell 语义，而
//! 权限层已经在拦这类命令了。
//!
//! 豁免理由：宿主层，读的是用户自己项目里的文件。

#![allow(clippy::disallowed_methods)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use similar::{ChangeTag, TextDiff};

/// 单个文件超过这么多行差异就截断。一次全量重写能产出几千行，
/// 全塞给界面只会让面板卡住 —— 那种改动本来也不是靠逐行读来 review 的。
const MAX_LINES_PER_FILE: usize = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    Created,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    Context,
    Add,
    Del,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffLine {
    pub kind: LineKind,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hunk {
    /// `@@ -1,4 +1,6 @@` 那一行。
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    /// 相对项目根的路径。绝对路径在界面上又长又没有信息量。
    pub path: String,
    pub status: ChangeStatus,
    pub added: usize,
    pub removed: usize,
    pub hunks: Vec<Hunk>,
    /// 差异太大，`hunks` 只是前一截。
    pub truncated: bool,
}

/// 算出一个会话的净改动。
///
/// `baselines` 来自文件状态缓存；当前内容现读磁盘 —— 用户可能在 agent
/// 改完之后自己又动过，以磁盘为准才是他此刻要 review 的东西。
pub async fn collect(root: &Path, baselines: Vec<(PathBuf, Option<String>)>) -> Vec<FileChange> {
    let mut out = Vec::new();
    for (path, before) in baselines {
        let after = tokio::fs::read_to_string(&path).await.ok();
        let Some(change) = diff_one(root, &path, before.as_deref(), after.as_deref()) else {
            continue;
        };
        out.push(change);
    }
    // 按路径排序：每次打开面板顺序都一样，眼睛才能记住位置。
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn diff_one(
    root: &Path,
    path: &Path,
    before: Option<&str>,
    after: Option<&str>,
) -> Option<FileChange> {
    let status = match (before, after) {
        // 建了又删，等于没动过。
        (None, None) => return None,
        (None, Some(_)) => ChangeStatus::Created,
        (Some(_), None) => ChangeStatus::Deleted,
        (Some(b), Some(a)) => {
            // 改回原样也算没动过 —— 报一个空 diff 只会让人白点一次。
            if b == a {
                return None;
            }
            ChangeStatus::Modified
        }
    };

    let diff = TextDiff::from_lines(before.unwrap_or(""), after.unwrap_or(""));
    let mut added = 0usize;
    let mut removed = 0usize;
    let mut hunks = Vec::new();
    let mut shown = 0usize;
    let mut truncated = false;

    for group in diff.grouped_ops(3) {
        let Some(first) = group.first() else { continue };
        let Some(last) = group.last() else { continue };
        let old = first.old_range().start..last.old_range().end;
        let new = first.new_range().start..last.new_range().end;
        let mut lines = Vec::new();

        for op in &group {
            for change in diff.iter_changes(op) {
                let kind = match change.tag() {
                    ChangeTag::Equal => LineKind::Context,
                    ChangeTag::Insert => {
                        added += 1;
                        LineKind::Add
                    }
                    ChangeTag::Delete => {
                        removed += 1;
                        LineKind::Del
                    }
                };
                if shown >= MAX_LINES_PER_FILE {
                    truncated = true;
                    continue;
                }
                shown += 1;
                lines.push(DiffLine {
                    kind,
                    // 行尾换行由渲染负责，带着它会让每行后面多一个空行。
                    text: change.value().trim_end_matches('\n').to_owned(),
                });
            }
        }

        if lines.is_empty() {
            continue;
        }
        hunks.push(Hunk {
            header: format!(
                "@@ -{},{} +{},{} @@",
                old.start + 1,
                old.len(),
                new.start + 1,
                new.len()
            ),
            lines,
        });
    }

    Some(FileChange {
        path: rel(root, path),
        status,
        added,
        removed,
        hunks,
        truncated,
    })
}

/// 相对项目根的显示路径。不在根下面（模型引了外部文件）就给绝对路径。
fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

// ── 基线落盘 ──────────────────────────────────────────────
//
// 基线在 MemoryFileState 里，重启进程就没了。改动面板靠它对比，
// 不落盘的话用户一重启就看到"这个会话还没有改过文件"。
//
// 不进 transcript：那是对话事实；基线是给界面用的对照底稿，而且可能
// 有整份文件那么大。不进 index.json：索引要保持小，启动只为画侧边栏。

use riot_protocol::id::ToolUseId;
use riot_protocol::message::{AssistantContent, Message, ToolResultContent, UserContent};
use riot_protocol::tool::{FileState, FileStateCache};
use riot_runtime::MemoryFileState;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct BaselineDump {
    #[serde(default)]
    files: Vec<BaselineRec>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct BaselineRec {
    path: PathBuf,
    #[serde(default)]
    before: Option<String>,
}

pub fn baselines_path(sessions_dir: &Path, id: &str) -> PathBuf {
    sessions_dir.join(format!("{id}.baselines.json"))
}

pub fn save_baselines(path: &Path, items: &[(PathBuf, Option<String>)]) -> std::io::Result<()> {
    let dump = BaselineDump {
        files: items
            .iter()
            .map(|(p, b)| BaselineRec {
                path: p.clone(),
                before: b.clone(),
            })
            .collect(),
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string(&dump).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

pub fn load_baselines(path: &Path) -> Vec<(PathBuf, Option<String>)> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "基线文件读失败，改动面板先空着");
            return Vec::new();
        }
    };
    match serde_json::from_str::<BaselineDump>(&raw) {
        Ok(d) => d.files.into_iter().map(|r| (r.path, r.before)).collect(),
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "基线文件读不懂，改动面板先空着");
            Vec::new()
        }
    }
}

pub fn remove_baselines(path: &Path) {
    if let Err(e) = std::fs::remove_file(path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(error = %e, path = %path.display(), "基线文件没删掉");
    }
}

/// 工具改文件时顺便把基线写到磁盘。和内存缓存共用同一份 `MemoryFileState`。
pub struct PersistingBaselines {
    inner: Arc<MemoryFileState>,
    path: PathBuf,
}

impl PersistingBaselines {
    pub fn new(inner: Arc<MemoryFileState>, path: PathBuf) -> Self {
        Self { inner, path }
    }
}

impl FileStateCache for PersistingBaselines {
    fn get(&self, path: &Path) -> Option<FileState> {
        self.inner.get(path)
    }

    fn put(&self, path: PathBuf, state: FileState) {
        self.inner.put(path, state);
    }

    fn invalidate(&self, path: &Path) {
        self.inner.invalidate(path);
    }

    fn recent(&self, limit: usize) -> Vec<(PathBuf, FileState)> {
        self.inner.recent(limit)
    }

    fn note_baseline(&self, path: PathBuf, before: Option<String>) {
        self.inner.note_baseline(path, before);
        if let Err(e) = save_baselines(&self.path, &self.inner.baselines()) {
            tracing::warn!(error = %e, "基线没写上盘，重启后这次改动会从面板里消失");
        }
    }

    fn baselines(&self) -> Vec<(PathBuf, Option<String>)> {
        self.inner.baselines()
    }
}

/// 没有 sidecar 时，从对话记录里把基线捞回来。
///
/// Edit / Write 都要求先 Read，所以第一次改之前的 Read 结果就是基线。
/// 压缩把结果清掉之后，新建文件还能从「已创建」认出来；覆盖/编辑就
/// 只能对着磁盘反推，推不出就跳过，不强行编一份假 diff。
pub fn reconstruct_baselines(cwd: &Path, messages: &[Message]) -> Vec<(PathBuf, Option<String>)> {
    let mut pending: HashMap<ToolUseId, PendingUse> = HashMap::new();
    let mut last_read: HashMap<PathBuf, String> = HashMap::new();
    let mut first: HashMap<PathBuf, Option<String>> = HashMap::new();

    for msg in messages {
        match msg {
            Message::Assistant { content, .. } => {
                for c in content {
                    if let AssistantContent::ToolUse { id, name, input } = c
                        && matches!(name.as_str(), "Read" | "Write" | "Edit")
                    {
                        pending.insert(
                            id.clone(),
                            PendingUse {
                                name: name.clone(),
                                input: input.clone(),
                            },
                        );
                    }
                }
            }
            Message::User { content, .. } => {
                for c in content {
                    let UserContent::ToolResult { tool_use_id, content, is_error } = c else {
                        continue;
                    };
                    let Some(use_) = pending.remove(tool_use_id) else { continue };
                    if *is_error {
                        continue;
                    }
                    let Some(raw) = use_.input.get("path").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let path = resolve_in(cwd, raw);
                    match use_.name.as_str() {
                        "Read" => {
                            if let Some(text) = result_text(content)
                                && let Some(body) = from_read_result(text)
                            {
                                last_read.insert(path, body);
                            }
                        }
                        "Write" | "Edit" => {
                            if first.contains_key(&path) {
                                continue;
                            }
                            let text = result_text(content).unwrap_or("");
                            let before = if use_.name == "Write" && text.contains("已创建") {
                                None
                            } else {
                                last_read.get(&path).cloned().or_else(|| {
                                    recover_edit_baseline(&path, &use_.input)
                                })
                            };
                            // 覆盖写但既没有先前的 Read、也反推不了：宁缺毋假。
                            if before.is_none() && !(use_.name == "Write" && text.contains("已创建")) {
                                continue;
                            }
                            first.insert(path, before);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    first.into_iter().collect()
}

struct PendingUse {
    name: String,
    input: serde_json::Value,
}

fn resolve_in(cwd: &Path, raw: &str) -> PathBuf {
    let p = PathBuf::from(raw);
    if p.is_absolute() { p } else { cwd.join(p) }
}

fn result_text(c: &ToolResultContent) -> Option<&str> {
    match c {
        ToolResultContent::Text { text } => Some(text),
        _ => None,
    }
}

/// 把 Read 给模型的带行号文本还原成文件内容。对不上格式就当没有。
fn from_read_result(text: &str) -> Option<String> {
    if text.contains("未显示") || text.contains("已截断") {
        return None;
    }
    let mut out = String::new();
    let mut any = false;
    for line in text.lines() {
        let Some((num, rest)) = line.split_once('\t') else {
            if line.is_empty() {
                continue;
            }
            return None;
        };
        if num.trim().parse::<usize>().is_err() {
            return None;
        }
        out.push_str(rest);
        out.push('\n');
        any = true;
    }
    any.then_some(out)
}

/// 磁盘上的当前内容往回退一次 Edit。没有 old/new 或对不上就放弃。
fn recover_edit_baseline(path: &Path, input: &serde_json::Value) -> Option<String> {
    let old = input.get("old_string")?.as_str()?;
    let new = input.get("new_string")?.as_str()?;
    if new.is_empty() {
        return None;
    }
    let current = std::fs::read_to_string(path).ok()?;
    if !current.contains(new) {
        return None;
    }
    let replace_all = input.get("replace_all").and_then(|v| v.as_bool()).unwrap_or(false);
    Some(if replace_all {
        current.replace(new, old)
    } else {
        current.replacen(new, old, 1)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from("/work")
    }

    #[test]
    fn 改动统计到行() {
        let c = diff_one(
            &root(),
            Path::new("/work/a.rs"),
            Some("one\ntwo\nthree\n"),
            Some("one\n2\nthree\nfour\n"),
        )
        .expect("有改动");

        assert_eq!(c.path, "a.rs", "路径按项目根相对化");
        assert_eq!(c.status, ChangeStatus::Modified);
        assert_eq!((c.added, c.removed), (2, 1));
        assert!(!c.truncated);
    }

    /// 手滑白改一遍（改完又改回去）不该出现在 review 列表里 ——
    /// 点开发现是空的，比不列出来更浪费时间。
    #[test]
    fn 改回原样不算改动() {
        assert!(
            diff_one(
                &root(),
                Path::new("/work/a.rs"),
                Some("same\n"),
                Some("same\n")
            )
            .is_none()
        );
    }

    #[test]
    fn 新建和删除分得出来() {
        let created = diff_one(&root(), Path::new("/work/n.rs"), None, Some("hi\n"))
            .expect("新建也是改动");
        assert_eq!(created.status, ChangeStatus::Created);
        assert_eq!(created.added, 1);

        let deleted = diff_one(&root(), Path::new("/work/g.rs"), Some("bye\n"), None)
            .expect("删除也是改动");
        assert_eq!(deleted.status, ChangeStatus::Deleted);
        assert_eq!(deleted.removed, 1);

        assert!(
            diff_one(&root(), Path::new("/work/x.rs"), None, None).is_none(),
            "建了又删等于没动过"
        );
    }

    /// 全量重写能产出几千行，面板扛不住。截断要如实标出来，
    /// 不然用户会以为"就改了这么多"。
    #[test]
    fn 超大改动会截断并标记() {
        let before = String::new();
        let after: String = (0..MAX_LINES_PER_FILE + 50)
            .map(|i| format!("line {i}\n"))
            .collect();

        let c = diff_one(&root(), Path::new("/work/big.rs"), Some(&before), Some(&after))
            .expect("有改动");

        assert!(c.truncated);
        let shown: usize = c.hunks.iter().map(|h| h.lines.len()).sum();
        assert_eq!(shown, MAX_LINES_PER_FILE);
        assert_eq!(
            c.added,
            MAX_LINES_PER_FILE + 50,
            "计数是全量，截的只是显示"
        );
    }

    #[test]
    fn 基线落盘能读回() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = baselines_path(dir.path(), "s1");
        save_baselines(
            &path,
            &[
                (PathBuf::from("/work/a.rs"), Some("old\n".into())),
                (PathBuf::from("/work/n.rs"), None),
            ],
        )
        .expect("写");

        let got = load_baselines(&path);
        assert_eq!(got.len(), 2);
        assert!(got.iter().any(|(p, b)| p == Path::new("/work/a.rs") && b.as_deref() == Some("old\n")));
        assert!(got.iter().any(|(p, b)| p == Path::new("/work/n.rs") && b.is_none()));
    }

    #[test]
    fn 没有基线文件就是空的() {
        let dir = tempfile::tempdir().expect("临时目录");
        assert!(load_baselines(&baselines_path(dir.path(), "nope")).is_empty());
    }

    fn msg_use(id: &str, name: &str, input: serde_json::Value) -> Message {
        Message::Assistant {
            id: riot_protocol::id::MessageId::from_raw("m"),
            content: vec![AssistantContent::ToolUse {
                id: ToolUseId::from_raw(id),
                name: name.into(),
                input,
            }],
            usage: None,
            meta: riot_protocol::message::MessageMeta::default(),
        }
    }

    fn msg_result(id: &str, text: &str) -> Message {
        Message::User {
            id: riot_protocol::id::MessageId::from_raw("u"),
            content: vec![UserContent::ToolResult {
                tool_use_id: ToolUseId::from_raw(id),
                content: ToolResultContent::text(text),
                is_error: false,
            }],
            meta: riot_protocol::message::MessageMeta::default(),
        }
    }

    #[test]
    fn 从对话能认出新建文件() {
        let cwd = Path::new("/work");
        let msgs = [
            msg_use("w1", "Write", serde_json::json!({ "path": "n.rs", "content": "hi\n" })),
            msg_result("w1", "已创建 n.rs（1 行）。"),
        ];
        let got = reconstruct_baselines(cwd, &msgs);
        assert_eq!(got, vec![(PathBuf::from("/work/n.rs"), None)]);
    }

    #[test]
    fn 从先前的read结果恢复覆盖前的内容() {
        let cwd = Path::new("/work");
        let read = "     1\told line\n";
        let msgs = [
            msg_use("r1", "Read", serde_json::json!({ "path": "a.rs" })),
            msg_result("r1", read),
            msg_use("w1", "Write", serde_json::json!({ "path": "a.rs", "content": "new\n" })),
            msg_result("w1", "已覆盖 a.rs（1 行）。"),
        ];
        let got = reconstruct_baselines(cwd, &msgs);
        assert_eq!(got, vec![(PathBuf::from("/work/a.rs"), Some("old line\n".into()))]);
    }

    #[test]
    fn 同一文件只记第一次改之前() {
        let cwd = Path::new("/work");
        let msgs = [
            msg_use("r1", "Read", serde_json::json!({ "path": "a.rs" })),
            msg_result("r1", "     1\tfirst\n"),
            msg_use("e1", "Edit", serde_json::json!({
                "path": "a.rs", "old_string": "first", "new_string": "second"
            })),
            msg_result("e1", "已修改 a.rs（替换了 1 处）。"),
            msg_use("r2", "Read", serde_json::json!({ "path": "a.rs" })),
            msg_result("r2", "     1\tsecond\n"),
            msg_use("e2", "Edit", serde_json::json!({
                "path": "a.rs", "old_string": "second", "new_string": "third"
            })),
            msg_result("e2", "已修改 a.rs（替换了 1 处）。"),
        ];
        let got = reconstruct_baselines(cwd, &msgs);
        assert_eq!(got, vec![(PathBuf::from("/work/a.rs"), Some("first\n".into()))]);
    }
}
