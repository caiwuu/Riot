//! 会话摘录（digest）：给模型翻的、按项目分目录的历史会话派生文件。
//!
//! # 为什么不让模型直接读 transcript
//!
//! transcript（[`crate::Transcripts`]）是事实来源，但它对"回忆过去的会话"
//! 这件事有三处不合用：
//!
//! - **一行一条完整消息，含工具结果全文**。一条 Bash 输出就是几十 KB 的
//!   一行，Grep 工具按行返回、有字符上限 —— 命中一次就把整个返回额度吃光。
//! - **它是事件日志，不是当前状态**。用户做过的编辑、删除、撤回、重新
//!   生成在文件里是追加的记录，原始消息还在；直接 grep 会把用户明确删掉
//!   的内容翻出来。
//! - **所有项目混在一个目录**，只靠首行 `meta.root` 区分，模型很容易串台。
//!
//! 摘录是从**回放后**的历史渲染出来的 Markdown（渲染在 riot-core 的
//! `archive` 模块）：一条消息一个小节、工具结果截头、思考丢弃，按项目
//! 放进各自的目录，再附一份 `INDEX.md` 总览。transcript 一个字不动，
//! 摘录随时可以全量重建 —— 它是缓存，不是数据。
//!
//! # 目录布局
//!
//! ```text
//! <sessions_dir>/digests/<项目键>/INDEX.md
//! <sessions_dir>/digests/<项目键>/<会话 id>.md
//! ```
//!
//! 项目键由规范化后的项目根派生（见 [`project_key`]）。
//!
//! # 这一层只管文件
//!
//! 路径派生、原子写、删除、front matter 的读写、INDEX 的生成。什么时候
//! 重渲染、渲染成什么样，是内核和 riot-core 的事。放在 riot-store 是因为
//! 宿主在内核不在的时候也要能删文件 —— 和 [`crate::Transcripts::remove`]
//! 同一个理由。
//!
//! 豁免理由：持久化层，操作真实文件系统（同 lib.rs）。

#![allow(clippy::disallowed_methods)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use riot_protocol::id::SessionId;
use sha2::Digest as _;

/// 摘录格式版本。渲染逻辑变了就加一；启动对账发现版本不符会重建。
pub const DIGEST_VERSION: u32 = 1;

/// 摘录目录名（挂在 sessions 目录下）。
pub const DIR_NAME: &str = "digests";

/// 每个项目目录里的总览文件名。
pub const INDEX_FILE: &str = "INDEX.md";

/// 项目根 → 目录名。
///
/// 非 `[A-Za-z0-9]` 一律换成 `-`（Cursor 的 `~/.cursor/projects/<键>` 同款），
/// 再拼上路径 SHA-256 的前 8 个十六进制字符。只做替换的话
/// `~/code/riot` 和 `~/code-riot` 会撞成同一个目录；哈希让键对同一路径
/// 确定、对不同路径不同。替换后的前缀留着是给人看的：用户在访达里
/// 打开这个目录要能认出是哪个项目。
///
/// 前缀截到 60 字符：路径可以很长，目录名有上限（255 字节），而且键的
/// 唯一性由哈希保证，前缀只负责可读。
pub fn project_key(root: &Path) -> String {
    let raw = root.to_string_lossy();
    let mut slug = String::with_capacity(raw.len());
    let mut last_dash = true;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug: String = slug.trim_end_matches('-').chars().take(60).collect();
    let slug = slug.trim_end_matches('-');
    let hash = sha2::Sha256::digest(raw.as_bytes());
    let short: String = hash.iter().take(4).map(|b| format!("{b:02x}")).collect();
    if slug.is_empty() {
        short
    } else {
        format!("{slug}-{short}")
    }
}

/// 摘录文件头部的元数据。渲染成 front matter 写在文件最前面；
/// INDEX 和启动对账靠读回它工作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestHeader {
    pub version: u32,
    pub session: String,
    pub root: PathBuf,
    /// 会话标题（手动名优先，其次首句摘录）。None = 还没说过话。
    pub title: Option<String>,
    pub created_at_ms: u64,
    /// 最后一条带时间戳的消息的时刻；没有就退回创建时刻。
    pub updated_at_ms: u64,
    pub messages: usize,
    /// 渲染时刻的本地时区偏移。文件里的人类可读时间按它算，
    /// 写进头部是为了读回来的人（和 INDEX）知道那些时间是哪个时区的。
    pub tz_offset_minutes: i32,
}

impl DigestHeader {
    /// 渲染成 front matter：`---` 包起来的 `key: value` 行。
    ///
    /// 机器读的是 `*_ms` 数值字段；人和模型读的是同名的可读时间。
    /// 两份都写是因为解析只想认数字，而模型 grep 时想看见 `2026-09-02`。
    pub fn front_matter(&self) -> String {
        let mut out = String::new();
        out.push_str("---\n");
        out.push_str(&format!("riot_digest: {}\n", self.version));
        out.push_str(&format!("session: {}\n", self.session));
        out.push_str(&format!("root: {}\n", self.root.display()));
        if let Some(t) = &self.title {
            out.push_str(&format!("title: {}\n", one_line(t)));
        }
        out.push_str(&format!(
            "created: {}\n",
            format_datetime(self.created_at_ms, self.tz_offset_minutes)
        ));
        out.push_str(&format!("created_ms: {}\n", self.created_at_ms));
        out.push_str(&format!(
            "updated: {}\n",
            format_datetime(self.updated_at_ms, self.tz_offset_minutes)
        ));
        out.push_str(&format!("updated_ms: {}\n", self.updated_at_ms));
        out.push_str(&format!("messages: {}\n", self.messages));
        out.push_str(&format!("tz_offset_minutes: {}\n", self.tz_offset_minutes));
        out.push_str("---\n");
        out
    }

    /// 从文件开头解析 front matter。认不出（不是摘录、老版本写坏了）
    /// 返回 None —— 调用方按"需要重建"处理。
    pub fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        if lines.next()?.trim() != "---" {
            return None;
        }
        let mut fields: HashMap<&str, &str> = HashMap::new();
        for line in lines {
            if line.trim() == "---" {
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                fields.insert(k.trim(), v.trim());
            }
        }
        Some(Self {
            version: fields.get("riot_digest")?.parse().ok()?,
            session: (*fields.get("session")?).to_owned(),
            root: PathBuf::from(*fields.get("root")?),
            title: fields
                .get("title")
                .filter(|t| !t.is_empty())
                .map(|t| (*t).to_owned()),
            created_at_ms: fields.get("created_ms")?.parse().ok()?,
            updated_at_ms: fields.get("updated_ms")?.parse().ok()?,
            messages: fields.get("messages")?.parse().ok()?,
            tz_offset_minutes: fields
                .get("tz_offset_minutes")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
        })
    }
}

/// 一个目录下所有项目的摘录。
pub struct Digests {
    dir: PathBuf,
}

impl Digests {
    /// `sessions_dir` 是 transcript 所在的目录；摘录放它下面的 `digests/`。
    pub fn new(sessions_dir: impl AsRef<Path>) -> Self {
        Self {
            dir: sessions_dir.as_ref().join(DIR_NAME),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 一个项目的摘录目录。
    pub fn project_dir(&self, root: &Path) -> PathBuf {
        self.dir.join(project_key(root))
    }

    /// 一个会话的摘录文件。
    pub fn path_of(&self, root: &Path, id: &SessionId) -> PathBuf {
        self.project_dir(root).join(format!("{}.md", id.as_str()))
    }

    pub fn index_path(&self, root: &Path) -> PathBuf {
        self.project_dir(root).join(INDEX_FILE)
    }

    /// 原子写一个会话的摘录：同目录临时文件 → rename。
    ///
    /// 另一个会话里的模型可能正在 Read 这个文件 —— 原子替换保证它读到的
    /// 要么是旧的完整版，要么是新的完整版，绝不是半个。
    pub async fn write(
        &self,
        root: &Path,
        id: &SessionId,
        contents: &str,
    ) -> std::io::Result<PathBuf> {
        let path = self.path_of(root, id);
        write_atomic(&path, contents).await?;
        Ok(path)
    }

    /// 删掉一个会话的摘录。不存在不是错误（和 transcript 删除同语义）。
    pub async fn remove(&self, root: &Path, id: &SessionId) -> std::io::Result<()> {
        match tokio::fs::remove_file(self.path_of(root, id)).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// 一个项目目录里所有摘录的头部。INDEX.md 本身不算。
    ///
    /// 只读每个文件的前几 KB —— 头部就在那里，整读的话对账要扫的是全部
    /// 会话的全部正文。读不懂头部的文件返回 `None` 头，调用方决定重建
    /// 还是删除。
    pub async fn headers(&self, root: &Path) -> Vec<(PathBuf, Option<DigestHeader>)> {
        let dir = self.project_dir(root);
        let Ok(mut rd) = tokio::fs::read_dir(&dir).await else {
            return Vec::new();
        };
        let mut out = Vec::new();
        while let Ok(Some(entry)) = rd.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            if path.file_name().and_then(|n| n.to_str()) == Some(INDEX_FILE) {
                continue;
            }
            let header = read_header(&path).await;
            out.push((path, header));
        }
        out
    }

    /// 重写一个项目的 INDEX.md。`headers` 由调用方给（多半刚从
    /// [`Self::headers`] 拿到），这里只负责排序、渲染、原子落盘。
    pub async fn write_index(&self, root: &Path, headers: &[DigestHeader]) -> std::io::Result<()> {
        let text = render_index(root, headers);
        write_atomic(&self.index_path(root), &text).await
    }

    /// 列出所有项目目录（对账用）。
    pub async fn project_dirs(&self) -> Vec<PathBuf> {
        let Ok(mut rd) = tokio::fs::read_dir(&self.dir).await else {
            return Vec::new();
        };
        let mut out = Vec::new();
        while let Ok(Some(entry)) = rd.next_entry().await {
            if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                out.push(entry.path());
            }
        }
        out
    }

    /// 删掉一个项目目录里的一个摘录文件（按路径），并在目录空了之后
    /// 把目录也收掉 —— 孤儿清理用。
    pub async fn remove_path(&self, path: &Path) -> std::io::Result<()> {
        match tokio::fs::remove_file(path).await {
            Ok(()) | Err(_) => {}
        }
        if let Some(dir) = path.parent() {
            // 只剩 INDEX.md（或空）就整个收掉。删不掉不是错误。
            let mut only_index = true;
            if let Ok(mut rd) = tokio::fs::read_dir(dir).await {
                while let Ok(Some(e)) = rd.next_entry().await {
                    if e.file_name().to_str() != Some(INDEX_FILE) {
                        only_index = false;
                        break;
                    }
                }
            }
            if only_index {
                let _ = tokio::fs::remove_file(dir.join(INDEX_FILE)).await;
                let _ = tokio::fs::remove_dir(dir).await;
            }
        }
        Ok(())
    }
}

/// 只读文件开头解析头部。
async fn read_header(path: &Path) -> Option<DigestHeader> {
    use tokio::io::AsyncReadExt as _;
    let mut f = tokio::fs::File::open(path).await.ok()?;
    let mut buf = vec![0u8; 4096];
    let n = f.read(&mut buf).await.ok()?;
    buf.truncate(n);
    let text = String::from_utf8_lossy(&buf);
    DigestHeader::parse(&text)
}

/// 临时文件 + rename。临时名带进程 id 和一个计数，两个内核实例（不该
/// 有，但防御）同时写同一个文件也不会互相覆盖对方的半成品。
async fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt as _;
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("摘录路径没有父目录"))?;
    tokio::fs::create_dir_all(dir).await?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("digest");
    let tmp = dir.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let result = async {
        let mut f = tokio::fs::File::create(&tmp).await?;
        f.write_all(contents.as_bytes()).await?;
        f.flush().await?;
        f.sync_data().await?;
        drop(f);
        tokio::fs::rename(&tmp, path).await
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    result
}

/// INDEX.md：一个项目的会话总览，按最近活动倒序。
///
/// 表格一行一个会话。标题里的 `|` 和换行洗掉，否则表格散架。
pub fn render_index(root: &Path, headers: &[DigestHeader]) -> String {
    let mut sorted: Vec<&DigestHeader> = headers.iter().collect();
    sorted.sort_by(|a, b| {
        b.updated_at_ms
            .cmp(&a.updated_at_ms)
            .then_with(|| a.session.cmp(&b.session))
    });
    let mut out = String::new();
    out.push_str(&format!(
        "# {} 的历史会话\n\n<!-- 由 Riot 自动生成，会被覆盖；每个会话的完整摘录在同目录的 <会话 id>.md -->\n\n",
        root.display()
    ));
    if sorted.is_empty() {
        out.push_str("（还没有会话）\n");
        return out;
    }
    out.push_str("| 最近活动 | 标题 | 消息数 | 文件 |\n|---|---|---|---|\n");
    for h in sorted {
        let title = h
            .title
            .as_deref()
            .map(one_line)
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "（无标题）".to_owned());
        out.push_str(&format!(
            "| {} | {} | {} | {}.md |\n",
            format_datetime(h.updated_at_ms, h.tz_offset_minutes),
            title.replace('|', "\\|"),
            h.messages,
            h.session
        ));
    }
    out
}

/// 从宿主写的 `index.json` 里读会话标题（手动名优先，其次自动名）。
///
/// 索引是宿主私有的 UI 元数据，这里只做**宽容的只读**：字段缺了、格式
/// 变了、文件不在，都只是拿不到标题，绝不报错。内核在会话不活跃时
/// （启动对账、宿主改了一个没打开的会话的名字）没有别的地方能拿到标题。
pub fn read_index_titles(sessions_dir: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(raw) = std::fs::read(sessions_dir.join("index.json")) else {
        return out;
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&raw) else {
        return out;
    };
    let Some(items) = v.get("sessions").and_then(|s| s.as_array()) else {
        return out;
    };
    for s in items {
        let Some(id) = s.get("id").and_then(|i| i.as_str()) else {
            continue;
        };
        let title = s
            .get("customTitle")
            .and_then(|t| t.as_str())
            .filter(|t| !t.trim().is_empty())
            .or_else(|| s.get("autoTitle").and_then(|t| t.as_str()))
            .filter(|t| !t.trim().is_empty());
        if let Some(t) = title {
            out.insert(id.to_owned(), t.to_owned());
        }
    }
    out
}

/// 文件的修改时刻（Unix 毫秒）。拿不到（文件不在）是 None。
pub async fn mtime_ms(path: &Path) -> Option<u64> {
    let md = tokio::fs::metadata(path).await.ok()?;
    let t = md.modified().ok()?;
    t.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `2026-09-02 17:16 UTC+8`。
///
/// 自带公历换算而不是引 chrono：整个 crate 只在这里需要"毫秒 → 年月日
/// 时分"，一个二十行的算法（Howard Hinnant 的 civil_from_days）换一个
/// 依赖，不划算。
pub fn format_datetime(epoch_ms: u64, tz_offset_minutes: i32) -> String {
    let shifted = epoch_ms.saturating_add_signed(i64::from(tz_offset_minutes) * 60_000);
    let days = (shifted / 86_400_000) as i64;
    let (y, m, d) = civil_from_days(days);
    let minutes_of_day = (shifted / 60_000) % (24 * 60);
    format!(
        "{y}-{m:02}-{d:02} {:02}:{:02} {}",
        minutes_of_day / 60,
        minutes_of_day % 60,
        tz_label(tz_offset_minutes)
    )
}

fn tz_label(offset_minutes: i32) -> String {
    if offset_minutes == 0 {
        return "UTC".to_owned();
    }
    let sign = if offset_minutes < 0 { '-' } else { '+' };
    let abs = offset_minutes.unsigned_abs();
    if abs.is_multiple_of(60) {
        format!("UTC{sign}{}", abs / 60)
    } else {
        format!("UTC{sign}{}:{:02}", abs / 60, abs % 60)
    }
}

/// 天序号（1970-01-01 = 0）→ 公历年月日。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(id: &str, updated: u64, title: Option<&str>) -> DigestHeader {
        DigestHeader {
            version: DIGEST_VERSION,
            session: id.into(),
            root: PathBuf::from("/tmp/proj"),
            title: title.map(str::to_owned),
            created_at_ms: 1_000,
            updated_at_ms: updated,
            messages: 3,
            tz_offset_minutes: 480,
        }
    }

    #[test]
    fn 项目键对同一路径确定_对不同路径不同() {
        let a = project_key(Path::new("/Users/me/code/riot"));
        assert_eq!(a, project_key(Path::new("/Users/me/code/riot")));
        // 只差标点的两个路径：光做替换会撞车，哈希把它们分开。
        let b = project_key(Path::new("/Users/me/code-riot"));
        assert_ne!(a, b, "{a}");
        assert!(a.starts_with("Users-me-code-riot-"), "前缀要可读：{a}");
    }

    #[test]
    fn 项目键处理_windows_盘符和超长路径() {
        let k = project_key(Path::new(r"D:\work\proj"));
        assert!(!k.contains(':') && !k.contains('\\'), "{k}");
        let long = format!("/{}", "a".repeat(500));
        let k = project_key(Path::new(&long));
        assert!(k.len() < 80, "目录名要有上限：{}", k.len());
    }

    #[test]
    fn 头部渲染后能原样解析() {
        let h = header("ses_1", 5_000, Some("修  多余空白\n的标题"));
        let text = format!("{}\n## [1] 用户\n正文", h.front_matter());
        let back = DigestHeader::parse(&text).expect("能解析");
        assert_eq!(back.session, "ses_1");
        assert_eq!(back.root, PathBuf::from("/tmp/proj"));
        assert_eq!(back.title.as_deref(), Some("修 多余空白 的标题"));
        assert_eq!(back.updated_at_ms, 5_000);
        assert_eq!(back.messages, 3);
        assert_eq!(back.tz_offset_minutes, 480);
        assert_eq!(back.version, DIGEST_VERSION);
    }

    #[test]
    fn 不是摘录的文件解析为_none() {
        assert!(DigestHeader::parse("# 随便一个 md").is_none());
        assert!(
            DigestHeader::parse("---\nsession: x\n---\n").is_none(),
            "缺字段也不认"
        );
    }

    #[test]
    fn 时间格式化带时区() {
        // 2026-09-02 09:16 UTC = 17:16 UTC+8
        let ms = 1_788_340_560_000;
        assert_eq!(format_datetime(ms, 480), "2026-09-02 17:16 UTC+8");
        assert_eq!(format_datetime(ms, 0), "2026-09-02 09:16 UTC");
        assert_eq!(format_datetime(ms, 330), "2026-09-02 14:46 UTC+5:30");
        assert_eq!(format_datetime(0, 0), "1970-01-01 00:00 UTC");
    }

    #[test]
    fn 索引按最近活动倒序_标题里的竖线要转义() {
        let hs = vec![
            header("old", 1_000, Some("旧 | 的")),
            header("new", 9_000, None),
        ];
        let s = render_index(Path::new("/tmp/proj"), &hs);
        let new_at = s.find("new.md").expect("有 new");
        let old_at = s.find("old.md").expect("有 old");
        assert!(new_at < old_at, "新的排前面：{s}");
        assert!(s.contains("旧 \\| 的"), "{s}");
        assert!(s.contains("（无标题）"), "{s}");
    }

    #[tokio::test]
    async fn 原子写后目录里没有临时文件_且能读回头部() {
        let d = tempfile::tempdir().expect("临时目录");
        let store = Digests::new(d.path());
        let root = Path::new("/tmp/proj");
        let id = SessionId::from_raw("ses_1");
        let h = header("ses_1", 5_000, Some("标题"));
        let path = store
            .write(root, &id, &format!("{}\n正文", h.front_matter()))
            .await
            .expect("写入");
        assert!(path.exists());
        let names: Vec<String> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().all(|n| !n.ends_with(".tmp")),
            "不能留临时文件：{names:?}"
        );

        let hs = store.headers(root).await;
        assert_eq!(hs.len(), 1);
        assert_eq!(hs[0].1.as_ref().map(|h| h.session.as_str()), Some("ses_1"));

        store.write_index(root, &[h]).await.expect("写索引");
        // INDEX 不算摘录
        assert_eq!(store.headers(root).await.len(), 1);
        let idx = std::fs::read_to_string(store.index_path(root)).unwrap();
        assert!(idx.contains("ses_1.md"), "{idx}");
    }

    #[tokio::test]
    async fn 删除幂等_目录空了就收掉() {
        let d = tempfile::tempdir().expect("临时目录");
        let store = Digests::new(d.path());
        let root = Path::new("/tmp/proj");
        let id = SessionId::from_raw("ses_1");
        store.write(root, &id, "x").await.unwrap();
        store.write_index(root, &[]).await.unwrap();
        store.remove(root, &id).await.expect("删");
        store.remove(root, &id).await.expect("再删也成功");
        // remove_path 收目录
        store.remove_path(&store.path_of(root, &id)).await.unwrap();
        assert!(!store.project_dir(root).exists(), "只剩 INDEX 的目录要收掉");
    }

    #[test]
    fn 从宿主索引里读标题_手动名优先_缺文件不报错() {
        let d = tempfile::tempdir().expect("临时目录");
        assert!(read_index_titles(d.path()).is_empty());
        std::fs::write(
            d.path().join("index.json"),
            r#"{"sessions":[
                {"id":"a","autoTitle":"自动","customTitle":"手动"},
                {"id":"b","autoTitle":"只有自动"},
                {"id":"c"},
                {"noid":true}
            ]}"#,
        )
        .unwrap();
        let t = read_index_titles(d.path());
        assert_eq!(t.get("a").map(String::as_str), Some("手动"));
        assert_eq!(t.get("b").map(String::as_str), Some("只有自动"));
        assert!(!t.contains_key("c"));
    }
}
