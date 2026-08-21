//! 会话索引：侧边栏需要的会话元数据，一个小 JSON 文件。
//!
//! # 索引是缓存，transcript 是事实
//!
//! 对话正文在 riot-store 的 JSONL 里（每会话一个文件）。这里只存两类东西：
//!
//! - **列表元数据**（seq、创建时间、标题）—— 为了启动时不用扫全部 transcript
//!   就能画出侧边栏（Claude Code 用"只读文件尾 64KB"达到同一目的，那个方案
//!   依赖"退出时把元数据再追加一遍"的补救，脆；Codex 用 SQLite 索引。小索引
//!   文件是两者取其轻）。
//! - **可变会话状态**（权限模式、采样覆盖、venv、追加提示词）—— 它们不属于
//!   对话内容，塞进 transcript 就得定义"哪行说了算"的合并规则。
//!
//! 索引损坏或丢失时从 transcript 的 Meta 首行重建（[`load`]）：会话和对话
//! 一条不丢，丢的只有上面第二类可变状态 —— 和"transcript 是事实来源"的
//! 定位一致。损坏的原文件先挪去备份，和 config.json 的处理同一条数据安全线。
//!
//! `[约束]` 写入必须原子（临时文件 + rename）。索引在每次会话变更后整体重写，
//! 中途断电留下半个 JSON 的话，下次启动就走重建 —— 但能不走就不走。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use riot_protocol::permission::PermissionMode;

use crate::config::Sampling;

/// 索引里的一个会话。字段全部 `default`：加载老索引不能因为缺字段而整体失败
/// （和 AppConfig 的向后兼容约束同源）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedSession {
    pub id: String,
    /// 会话绑定的项目根（创建时已规范化）。目录后来没了也照样恢复 ——
    /// 历史仍然可读，删对话的决定留给用户；工具会在使用时报真实错误。
    pub root: String,
    pub seq: u64,
    pub created_at_ms: u64,
    /// 用户手动改的标题。None = 用自动标题。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_title: Option<String>,
    /// 自动标题：第一句用户输入的截断。缓存在这里而不是每次从历史推导 ——
    /// 历史是惰性加载的，而启动画侧边栏就要标题。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_title: Option<String>,
    #[serde(default = "default_mode")]
    pub mode: PermissionMode,
    #[serde(default, skip_serializing_if = "Sampling::is_empty")]
    pub sampling: Sampling,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_venv: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// 会话级思考策略。默认 = 不干预（不发任何思考参数）。
    #[serde(
        default,
        skip_serializing_if = "riot_protocol::ThinkingPolicy::is_default"
    )]
    pub thinking: riot_protocol::ThinkingPolicy,
}

fn default_mode() -> PermissionMode {
    PermissionMode::Default
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SessionIndex {
    #[serde(default)]
    pub sessions: Vec<PersistedSession>,
}

fn index_path(sessions_dir: &Path) -> PathBuf {
    sessions_dir.join("index.json")
}

/// 读索引。读不到（不存在/损坏）就从 transcript 重建。
///
/// 同步执行 —— 恢复发生在 Tauri runtime 起来之前，那里没有 async 上下文。
pub fn load(sessions_dir: &Path, transcripts: &riot_store::Transcripts) -> SessionIndex {
    let p = index_path(sessions_dir);
    // 豁免理由：宿主持久化层，读的是自己的索引文件。
    #[allow(clippy::disallowed_methods)]
    match std::fs::read_to_string(&p) {
        Ok(raw) => match serde_json::from_str::<SessionIndex>(&raw) {
            Ok(idx) => idx,
            Err(e) => {
                // 和 config.json 同一条线：损坏的原文件必须先挪走再重建，
                // 否则下一次保存就把它覆盖了，而它里面可能还有能捞的标题。
                tracing::warn!(error = %e, "会话索引读不懂，备份后从 transcript 重建");
                backup_unreadable(&p);
                rebuild(transcripts)
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => rebuild(transcripts),
        Err(e) => {
            tracing::error!(error = %e, "会话索引读取失败，从 transcript 重建");
            rebuild(transcripts)
        }
    }
}

/// 原子写索引：临时文件 + rename。
pub fn save(sessions_dir: &Path, index: &SessionIndex) -> std::io::Result<()> {
    // 豁免理由：宿主持久化层，写的是自己的索引文件。
    #[allow(clippy::disallowed_methods)]
    {
        std::fs::create_dir_all(sessions_dir)?;
        let json = serde_json::to_string_pretty(index).map_err(std::io::Error::other)?;
        let tmp = sessions_dir.join("index.json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, index_path(sessions_dir))
    }
}

/// 从 transcript 首行重建索引。
///
/// seq 按创建时间重排 —— 原始序号丢了，但相对顺序（侧边栏的排列）保得住。
/// 可变状态（模式、采样等）回到默认值：那些只存在于索引里，丢了就是丢了，
/// 对话本身一条不少。
fn rebuild(transcripts: &riot_store::Transcripts) -> SessionIndex {
    let mut scanned = transcripts.scan();
    if scanned.is_empty() {
        return SessionIndex::default();
    }
    scanned.sort_by_key(|s| s.meta.created_at_ms);

    let sessions = scanned
        .into_iter()
        .enumerate()
        .map(|(i, s)| PersistedSession {
            id: s.meta.id.as_str().to_owned(),
            root: s.meta.root.display().to_string(),
            seq: i as u64,
            created_at_ms: s.meta.created_at_ms,
            custom_title: None,
            auto_title: s
                .first_prompt
                .as_deref()
                .and_then(crate::session::title_excerpt),
            mode: PermissionMode::Default,
            sampling: Sampling::default(),
            python_venv: None,
            system_prompt: None,
            thinking: riot_protocol::ThinkingPolicy::default(),
        })
        .collect();
    tracing::info!("会话索引已从 transcript 重建");
    SessionIndex { sessions }
}

/// 把读不懂的索引挪到旁边。挪不动就保留原文件（下次启动还有机会捞）。
fn backup_unreadable(p: &Path) {
    let bak = free_backup_path(p);
    let Some(bak) = bak else {
        tracing::error!("索引备份名全被占用，保留原文件不动");
        return;
    };
    // 豁免理由：宿主持久化层。
    #[allow(clippy::disallowed_methods)]
    if let Err(e) = std::fs::rename(p, &bak) {
        tracing::error!(error = %e, "索引备份失败，保留原文件");
    }
}

/// 找一个没被占用的备份名。全占满返回 None —— 备份的意义就是不被覆盖。
fn free_backup_path(p: &Path) -> Option<PathBuf> {
    const MAX_BACKUPS: u32 = 100;
    let first = p.with_extension("json.bak");
    if !first.exists() {
        return Some(first);
    }
    (1..MAX_BACKUPS)
        .map(|n| p.with_extension(format!("json.{n}.bak")))
        .find(|c| !c.exists())
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use riot_protocol::id::{MessageId, SessionId};
    use riot_protocol::message::{Message, MessageMeta, UserContent};

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("临时目录")
    }

    fn one(id: &str, seq: u64) -> PersistedSession {
        PersistedSession {
            id: id.into(),
            root: "/tmp/proj".into(),
            seq,
            created_at_ms: 1_000 + seq,
            custom_title: None,
            auto_title: Some("标题".into()),
            mode: PermissionMode::Default,
            sampling: Sampling::default(),
            python_venv: None,
            system_prompt: None,
            thinking: riot_protocol::ThinkingPolicy::default(),
        }
    }

    #[test]
    fn 保存后能读回() {
        let d = dir();
        let transcripts = riot_store::Transcripts::new(d.path());
        let idx = SessionIndex {
            sessions: vec![one("s1", 0), one("s2", 1)],
        };
        save(d.path(), &idx).expect("保存");

        let back = load(d.path(), &transcripts);
        assert_eq!(back.sessions, idx.sessions);
    }

    #[test]
    fn 没有索引也没有transcript时为空() {
        let d = dir();
        let transcripts = riot_store::Transcripts::new(d.path());
        assert!(load(d.path(), &transcripts).sessions.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn 索引损坏时备份并从transcript重建() {
        let d = dir();
        let transcripts = riot_store::Transcripts::new(d.path());

        // 先造一份真实的 transcript
        let log = transcripts.open(riot_store::TranscriptMeta {
            id: SessionId::from_raw("s1"),
            root: "/tmp/proj".into(),
            created_at_ms: 42,
        });
        log.append(&Message::User {
            id: MessageId::from_raw("m1"),
            content: vec![UserContent::Text {
                text: "重建后的标题".into(),
            }],
            meta: MessageMeta::default(),
        });
        log.flush().await;

        // 索引写坏
        std::fs::write(d.path().join("index.json"), "{坏的").expect("写坏索引");

        let idx = load(d.path(), &transcripts);
        assert_eq!(idx.sessions.len(), 1, "会话必须从 transcript 里捞回来");
        assert_eq!(idx.sessions[0].id, "s1");
        assert_eq!(idx.sessions[0].root, "/tmp/proj");
        assert_eq!(idx.sessions[0].created_at_ms, 42);
        assert_eq!(idx.sessions[0].auto_title.as_deref(), Some("重建后的标题"));

        assert!(
            d.path().join("index.json.bak").exists(),
            "损坏的索引要备份，不能直接扔"
        );
    }

    #[test]
    fn 缺字段的老索引能读() {
        // 向后兼容：以后加字段，老索引不能整体解析失败 ——
        // 那表现为"升级后我的会话全没了"。
        let d = dir();
        let transcripts = riot_store::Transcripts::new(d.path());
        std::fs::write(
            d.path().join("index.json"),
            r#"{"sessions":[{"id":"s1","root":"/w","seq":0,"createdAtMs":1}]}"#,
        )
        .expect("写老索引");

        let idx = load(d.path(), &transcripts);
        assert_eq!(idx.sessions.len(), 1);
        assert_eq!(idx.sessions[0].mode, PermissionMode::Default);
        assert!(idx.sessions[0].custom_title.is_none());
        assert!(
            idx.sessions[0].thinking.is_default(),
            "缺 thinking 字段回默认 = 不发思考参数，老会话行为不变"
        );
    }
}
