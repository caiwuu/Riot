//! 会话摘录的编排：什么时候、拿什么、写到哪。
//!
//! 文件层在 [`riot_store::digests`]，渲染在 [`riot_core::archive`]；这里把
//! 两者接到会话的生命周期上：
//!
//! - **活会话**从内存历史（界面归档 + 活历史）渲染 —— transcript 是后台
//!   写入、可能滞后几条，内存才是此刻的真相；
//! - **非活会话**从 transcript 回放渲染（[`riot_store::Transcripts::load_parts`]
//!   已经把编辑/删除/撤回/重新生成全应用过了，摘录天然只含用户保留的内容）；
//! - **启动对账**把两者对齐：缺的补、过期的重写、版本不符的重建、
//!   transcript 已经不在的孤儿删掉。
//!
//! # 两个用途，一个开关
//!
//! 摘录同时是**压缩归档**（续接消息指着它，模型翻回刚被总结掉的原文）和
//! **跨会话回忆**（系统提示词指着目录，模型翻别的会话）。前者是压缩机制
//! 的一部分，不该被用户关掉；后者才是设置里那个「历史会话回忆」开关管的
//! 东西。所以 `enabled` 只管三件事：提示词要不要提目录、INDEX.md 要不要
//! 维护、启动要不要对账 —— 每个会话自己的摘录始终写。
//!
//! # 触发点
//!
//! 一轮结束、上下文编辑/删除、压缩落地（三条路都算）、改标题、删会话。
//! 轮**中**的普通消息不触发：一轮几十条工具结果，每条都渲染整个会话是
//! O(n²)，而轮中模型自己的历史就在上下文里，没有人需要那份摘录。压缩是
//! 例外 —— 续接消息刚指了路，文件得在模型下一步去读之前是新的。
//!
//! # 并发
//!
//! 每个项目一把锁：同项目两个会话同时收工，INDEX.md 不互相踩。同一会话
//! 连续两次触发（编辑紧接着删除）用代际号去重：晚来的请求作废早来的，
//! 谁的快照新谁落盘。
//!
//! 豁免理由：宿主侧持久化编排，读写真实文件系统与时钟（同 riot-store）。

#![allow(clippy::disallowed_methods)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use riot_protocol::id::SessionId;
use riot_protocol::message::Message;
use riot_protocol::tool::Clock;
use riot_store::digests::{DIGEST_VERSION, DigestHeader, Digests};

/// 一次摘录所需的全部输入。
///
/// 自持有而不是借用：快照要在项目锁**里面**取（见 [`DigestWriter::write_with`]），
/// 借用会把会话的历史锁和这里的项目锁绑在同一个生命周期上。历史几 MB
/// 的克隆一轮一次，可接受。
pub struct DigestSnapshot {
    pub id: SessionId,
    pub root: PathBuf,
    pub title: Option<String>,
    pub created_at_ms: u64,
    /// 完整历史：界面归档 + 活历史，按时间顺序。
    pub messages: Vec<Message>,
}

/// 摘录写入器。内核进程一份，所有会话共享。
pub struct DigestWriter {
    digests: Digests,
    sessions_dir: PathBuf,
    transcripts: Arc<riot_store::Transcripts>,
    clock: Arc<dyn Clock>,
    /// 「历史会话回忆」开关（见模块文档）。关掉后提示词不提目录、不维护
    /// INDEX、不对账；每个会话自己的摘录照写 —— 压缩归档靠它。
    enabled: AtomicBool,
    /// 每个项目一把锁（见模块文档）。
    locks: tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// 对账只跑一次（宿主可能多次发 configure）。
    reconciled: AtomicBool,
}

impl DigestWriter {
    pub fn new(
        sessions_dir: PathBuf,
        transcripts: Arc<riot_store::Transcripts>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            digests: Digests::new(&sessions_dir),
            sessions_dir,
            transcripts,
            clock,
            enabled: AtomicBool::new(true),
            locks: tokio::sync::Mutex::new(HashMap::new()),
            reconciled: AtomicBool::new(false),
        }
    }

    /// 切开关。从关到开要重新对账一次：关着的时候 INDEX 被收掉了，
    /// 得补回来。
    pub fn set_enabled(&self, on: bool) {
        let was = self.enabled.swap(on, Ordering::Relaxed);
        if on && !was {
            self.reconciled.store(false, Ordering::Relaxed);
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// 一个项目的摘录目录 —— 提示词里指给模型的路径。关掉时是 None。
    pub fn project_dir(&self, root: &Path) -> Option<PathBuf> {
        self.enabled().then(|| self.digests.project_dir(root))
    }

    /// 一个会话的摘录文件 —— 压缩续接消息里指给模型的路径。不看开关：
    /// 压缩归档不是用户能关的东西。
    pub fn path_for(&self, root: &Path, id: &SessionId) -> PathBuf {
        self.digests.path_of(root, id)
    }

    async fn lock_for(&self, root: &Path) -> Arc<tokio::sync::Mutex<()>> {
        let key = riot_store::digests::project_key(root);
        Arc::clone(
            self.locks
                .lock()
                .await
                .entry(key)
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
        )
    }

    /// 从内存历史渲染并落盘，再重建该项目的 INDEX。
    ///
    /// 空历史不写文件（和 transcript"追加前不建文件"同一个理由：每个
    /// 点了 + 又没说话的会话都在磁盘上留壳，INDEX 里就是一排"无标题"）。
    /// 写失败只告警 —— 摘录是缓存，不能因为磁盘满了让一轮报错。
    pub async fn write(&self, snapshot: DigestSnapshot) -> Option<PathBuf> {
        let root = snapshot.root.clone();
        self.write_with(&root, || std::future::ready(snapshot))
            .await
    }

    /// 在项目锁**里面**取快照再写。
    ///
    /// 快照在锁外取的话，两个触发点（改名 vs. 一轮收尾）各取一份、抢同一把
    /// 锁，先取后写的那份会把新状态盖成旧的。锁里取快照，谁后拿到锁谁的
    /// 快照就更新，落盘顺序和状态顺序一致。
    pub async fn write_with<F, Fut>(&self, root: &Path, snapshot: F) -> Option<PathBuf>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = DigestSnapshot>,
    {
        let lock = self.lock_for(root).await;
        let _g = lock.lock().await;
        let snap = snapshot().await;
        if snap.messages.is_empty() {
            // 历史被删空了：把之前的摘录也收掉，INDEX 不再列它。
            let _ = self.digests.remove(&snap.root, &snap.id).await;
            self.refresh_index(&snap.root).await;
            return None;
        }
        let tz = self.clock.tz_offset_minutes();
        let header = DigestHeader {
            version: DIGEST_VERSION,
            session: snap.id.as_str().to_owned(),
            root: snap.root.clone(),
            title: snap.title,
            created_at_ms: snap.created_at_ms,
            updated_at_ms: last_stamp(&snap.messages).unwrap_or(snap.created_at_ms),
            messages: snap.messages.len(),
            tz_offset_minutes: tz,
        };
        let time = |ms: u64| riot_store::digests::format_datetime(ms, tz);
        let body = riot_core::archive::render_body(
            &snap.messages,
            &riot_core::archive::RenderOptions::digest(&time),
        );
        let text = format!("{}\n{body}", header.front_matter());
        let path = match self.digests.write(&snap.root, &snap.id, &text).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, session = %snap.id.as_str(), "会话摘录写不出去");
                return None;
            }
        };
        self.refresh_index(&snap.root).await;
        Some(path)
    }

    /// 从磁盘 transcript 回放渲染（会话不在内存里时）。`title` 给了就用，
    /// 不给就从宿主索引里找，再退回首句摘录。
    pub async fn write_from_disk(&self, id: &SessionId, title: Option<String>) -> Option<PathBuf> {
        let parts = self.transcripts.load_parts(id).await;
        let meta = parts.meta?;
        let mut all = parts.archived;
        all.extend(parts.live);
        let title = title
            .or_else(|| {
                riot_store::digests::read_index_titles(&self.sessions_dir).remove(id.as_str())
            })
            .or_else(|| first_prompt_title(&all));
        self.write(DigestSnapshot {
            id: id.clone(),
            root: meta.root,
            title,
            created_at_ms: meta.created_at_ms,
            messages: all,
        })
        .await
    }

    /// 删掉一个会话的摘录并更新 INDEX。
    pub async fn remove(&self, root: &Path, id: &SessionId) {
        let lock = self.lock_for(root).await;
        let _g = lock.lock().await;
        if let Err(e) = self.digests.remove(root, id).await {
            tracing::warn!(error = %e, "会话摘录删除失败");
        }
        self.refresh_index(root).await;
    }

    /// 重建一个项目的 INDEX.md。调用方持有该项目的锁。
    ///
    /// 回忆关着时**收掉** INDEX 而不是留着：它是"这个目录可以翻"的路标，
    /// 用户关掉功能就不该再有路标。摘录本身留着（压缩归档）。
    async fn refresh_index(&self, root: &Path) {
        let headers: Vec<DigestHeader> = self
            .digests
            .headers(root)
            .await
            .into_iter()
            .filter_map(|(_, h)| h)
            .collect();
        if headers.is_empty() {
            // 一个摘录都没有就把目录收掉，别留一个只有 INDEX 的空壳。
            let _ = tokio::fs::remove_file(self.digests.index_path(root)).await;
            let _ = tokio::fs::remove_dir(self.digests.project_dir(root)).await;
            return;
        }
        if !self.enabled() {
            let _ = tokio::fs::remove_file(self.digests.index_path(root)).await;
            return;
        }
        if let Err(e) = self.digests.write_index(root, &headers).await {
            tracing::warn!(error = %e, "会话摘录 INDEX 写不出去");
        }
    }

    /// 启动对账（只跑一次，后台低优先级）。
    ///
    /// 逐个 transcript 比对：摘录缺失、比 transcript 旧、格式版本不符 →
    /// 重建；摘录对应的 transcript 已经不在 → 删。每处理一个文件让出一次
    /// 调度，别在启动期抢 IO。
    pub async fn reconcile(self: &Arc<Self>) {
        if !self.enabled() || self.reconciled.swap(true, Ordering::Relaxed) {
            return;
        }
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let stats = this.reconcile_inner().await;
            if stats.rebuilt + stats.removed > 0 {
                tracing::info!(
                    rebuilt = stats.rebuilt,
                    removed = stats.removed,
                    "会话摘录对账完成"
                );
            }
        });
    }

    async fn reconcile_inner(&self) -> ReconcileStats {
        let mut stats = ReconcileStats::default();
        let titles = riot_store::digests::read_index_titles(&self.sessions_dir);
        let scanned = self.transcripts.scan();
        let mut known: HashSet<String> = HashSet::new();
        let mut roots: HashMap<String, PathBuf> = HashMap::new();

        for s in &scanned {
            let id = &s.meta.id;
            known.insert(id.as_str().to_owned());
            roots
                .entry(riot_store::digests::project_key(&s.meta.root))
                .or_insert_with(|| s.meta.root.clone());
            if self.needs_rebuild(&s.meta).await {
                let title = titles.get(id.as_str()).cloned().or_else(|| {
                    s.first_prompt
                        .as_deref()
                        .and_then(crate::session::title_excerpt)
                });
                if self.write_from_disk(id, title).await.is_some() {
                    stats.rebuilt += 1;
                }
            }
            tokio::task::yield_now().await;
        }

        // 孤儿：摘录在、transcript 不在。摘录是派生数据，可以直接删。
        for dir in self.digests.project_dirs().await {
            let Some(root) = self.root_of_dir(&dir).await else {
                continue;
            };
            roots
                .entry(riot_store::digests::project_key(&root))
                .or_insert(root.clone());
            let lock = self.lock_for(&root).await;
            let _g = lock.lock().await;
            for (path, header) in self.digests.headers(&root).await {
                let orphan = match &header {
                    Some(h) => !known.contains(&h.session),
                    // 头都读不出来的文件不是我们写的（或写坏了）：也删，
                    // 重建路径会补一份对的。
                    None => true,
                };
                if orphan {
                    let _ = self.digests.remove_path(&path).await;
                    stats.removed += 1;
                }
                tokio::task::yield_now().await;
            }
        }
        // 每个项目的 INDEX 都对一遍：开关关着的那段时间它被收掉了，
        // 重新打开要补回来；读几十个文件头，便宜。
        for root in roots.values() {
            let lock = self.lock_for(root).await;
            let _g = lock.lock().await;
            self.refresh_index(root).await;
        }
        stats
    }

    /// 摘录是否需要重建：缺失、比 transcript 旧、格式版本不符。
    async fn needs_rebuild(&self, meta: &riot_store::TranscriptMeta) -> bool {
        let digest_path = self.digests.path_of(&meta.root, &meta.id);
        let Some(digest_mtime) = riot_store::digests::mtime_ms(&digest_path).await else {
            return true;
        };
        let transcript_mtime = riot_store::digests::mtime_ms(&self.transcripts.path_of(&meta.id))
            .await
            .unwrap_or(0);
        if transcript_mtime > digest_mtime {
            return true;
        }
        match tokio::fs::read(&digest_path).await {
            Ok(bytes) => {
                let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
                DigestHeader::parse(&head).is_none_or(|h| h.version != DIGEST_VERSION)
            }
            Err(_) => true,
        }
    }

    /// 从项目目录里任意一个摘录的头部拿回项目根（目录名是哈希过的，
    /// 反推不出来）。目录里一个能读的都没有就返回 None，整个目录留给
    /// 下一次对账 —— 删数据的决定要有依据。
    async fn root_of_dir(&self, dir: &Path) -> Option<PathBuf> {
        let mut rd = tokio::fs::read_dir(dir).await.ok()?;
        while let Ok(Some(e)) = rd.next_entry().await {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("md") {
                continue;
            }
            if let Ok(bytes) = tokio::fs::read(&p).await {
                let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
                if let Some(h) = DigestHeader::parse(&head) {
                    return Some(h.root);
                }
            }
        }
        None
    }
}

#[derive(Default)]
struct ReconcileStats {
    rebuilt: usize,
    removed: usize,
}

/// 最后一条带时间戳的消息的时刻。
fn last_stamp(messages: &[Message]) -> Option<u64> {
    messages.iter().rev().find_map(|m| match m {
        Message::User { meta, .. } | Message::Assistant { meta, .. } => meta.created_at_ms,
        Message::System { .. } => None,
    })
}

/// 首句用户输入的摘录，标题兜底用。
fn first_prompt_title(messages: &[Message]) -> Option<String> {
    messages.iter().find_map(|m| match m {
        Message::User { content, meta, .. } if !meta.synthetic => {
            content.iter().find_map(|c| match c {
                riot_protocol::message::UserContent::Text { text } => {
                    crate::session::title_excerpt(text)
                }
                _ => None,
            })
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use riot_protocol::id::MessageId;
    use riot_protocol::message::{MessageMeta, UserContent};

    struct FixedClock;
    #[async_trait::async_trait]
    impl Clock for FixedClock {
        fn now_ms(&self) -> u64 {
            1_788_340_560_000
        }
        fn tz_offset_minutes(&self) -> i32 {
            480
        }
        async fn sleep_ms(&self, _ms: u64) {}
    }

    fn user(id: &str, text: &str, at: u64) -> Message {
        Message::User {
            id: MessageId::from_raw(id),
            content: vec![UserContent::Text { text: text.into() }],
            meta: MessageMeta {
                created_at_ms: Some(at),
                ..Default::default()
            },
        }
    }

    fn setup() -> (
        tempfile::TempDir,
        Arc<DigestWriter>,
        Arc<riot_store::Transcripts>,
    ) {
        let d = tempfile::tempdir().expect("临时目录");
        let transcripts = Arc::new(riot_store::Transcripts::new(d.path()));
        let w = Arc::new(DigestWriter::new(
            d.path().to_path_buf(),
            Arc::clone(&transcripts),
            Arc::new(FixedClock),
        ));
        (d, w, transcripts)
    }

    #[tokio::test]
    async fn 从内存写摘录_带头部和_index() {
        let (_d, w, _) = setup();
        let root = Path::new("/tmp/proj");
        let id = SessionId::from_raw("ses_a");
        let msgs = vec![user("m1", "第一句", 1_000), user("m2", "第二句", 9_000)];
        let path = w
            .write(DigestSnapshot {
                id: id.clone(),
                root: root.to_path_buf(),
                title: Some("标题".into()),
                created_at_ms: 500,
                messages: msgs,
            })
            .await
            .expect("写成功");
        let text = std::fs::read_to_string(&path).unwrap();
        let h = DigestHeader::parse(&text).expect("头部能解析");
        assert_eq!(h.updated_at_ms, 9_000, "updated 取最后一条的时间戳");
        assert_eq!(h.messages, 2);
        assert!(
            text.contains("## [1] 用户 (m1) 1970-01-01 08:00 UTC+8"),
            "{text}"
        );
        let idx = std::fs::read_to_string(w.digests.index_path(root)).unwrap();
        assert!(idx.contains("ses_a.md") && idx.contains("标题"), "{idx}");
    }

    #[tokio::test]
    async fn 空历史不写文件_删空后连_index_一起收掉() {
        let (_d, w, _) = setup();
        let root = Path::new("/tmp/proj");
        let id = SessionId::from_raw("ses_a");
        assert!(
            w.write(DigestSnapshot {
                id: id.clone(),
                root: root.to_path_buf(),
                title: None,
                created_at_ms: 1,
                messages: Vec::new(),
            })
            .await
            .is_none()
        );
        assert!(!w.digests.project_dir(root).exists(), "空历史不该建目录");

        w.write(DigestSnapshot {
            id: id.clone(),
            root: root.to_path_buf(),
            title: None,
            created_at_ms: 1,
            messages: vec![user("m1", "有话", 1_000)],
        })
        .await
        .unwrap();
        w.remove(root, &id).await;
        assert!(
            !w.digests.project_dir(root).exists(),
            "最后一个摘录删了目录要收掉"
        );
    }

    #[tokio::test]
    async fn 关掉开关后仍写摘录_但不留_index_也不指路() {
        // 开关管的是"跨会话回忆"：关掉后提示词不指路、INDEX 收掉；每个
        // 会话自己的摘录照写 —— 压缩续接消息还指着它。重新打开，对账把
        // INDEX 补回来。
        let (_d, w, transcripts) = setup();
        let root = PathBuf::from("/tmp/proj");
        let id = SessionId::from_raw("ses_a");
        // 盘上要有对应的 transcript，否则对账会把摘录当孤儿删掉。
        let log = transcripts.open(riot_store::TranscriptMeta {
            id: id.clone(),
            root: root.clone(),
            created_at_ms: 1,
        });
        log.append(&user("m1", "有话", 1_000));
        log.flush().await;
        w.set_enabled(false);
        let path = w
            .write(DigestSnapshot {
                id: id.clone(),
                root: root.clone(),
                title: None,
                created_at_ms: 1,
                messages: vec![user("m1", "有话", 1_000)],
            })
            .await
            .expect("摘录照写：压缩归档靠它");
        assert!(path.exists());
        assert_eq!(w.path_for(&root, &id), path, "续接消息拿到的路径就是它");
        assert!(w.project_dir(&root).is_none(), "提示词不该指路");
        assert!(!w.digests.index_path(&root).exists(), "路标要收掉");

        w.set_enabled(true);
        w.reconcile_inner().await;
        assert!(w.digests.index_path(&root).exists(), "重新打开要补回 INDEX");
        assert!(w.project_dir(&root).is_some());
    }

    /// 从磁盘回放渲染：被撤回的消息不出现，标题从宿主索引里拿。
    #[tokio::test]
    async fn 从磁盘回放_尊重撤回_标题来自宿主索引() {
        let (d, w, transcripts) = setup();
        let id = SessionId::from_raw("ses_a");
        let log = transcripts.open(riot_store::TranscriptMeta {
            id: id.clone(),
            root: PathBuf::from("/tmp/proj"),
            created_at_ms: 42,
        });
        log.append(&user("m1", "留下的", 1_000));
        log.append(&user("m2", "被撤回的秘密", 2_000));
        log.append_withdraw("m2");
        log.flush().await;
        std::fs::write(
            d.path().join("index.json"),
            r#"{"sessions":[{"id":"ses_a","customTitle":"我起的名"}]}"#,
        )
        .unwrap();

        let path = w.write_from_disk(&id, None).await.expect("写成功");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("留下的"));
        assert!(
            !text.contains("被撤回的秘密"),
            "用户撤回的内容不能复活：{text}"
        );
        assert!(text.contains("title: 我起的名"), "{text}");
    }

    /// 对账：缺的补、孤儿删、新鲜的不动。
    #[tokio::test]
    async fn 对账_补缺_删孤儿_不碰新鲜的() {
        let (_d, w, transcripts) = setup();
        let root = PathBuf::from("/tmp/proj");
        // 两个 transcript：a 没摘录，b 有新鲜摘录
        for (id, at) in [("ses_a", 1_000u64), ("ses_b", 2_000)] {
            let log = transcripts.open(riot_store::TranscriptMeta {
                id: SessionId::from_raw(id),
                root: root.clone(),
                created_at_ms: at,
            });
            log.append(&user("m1", "内容", at));
            log.flush().await;
        }
        let b = SessionId::from_raw("ses_b");
        // 让 b 的摘录比 transcript 新
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let b_path = w.write_from_disk(&b, Some("b 的名".into())).await.unwrap();
        let b_mtime = riot_store::digests::mtime_ms(&b_path).await.unwrap();
        // 一个孤儿摘录（transcript 不存在）
        let ghost = SessionId::from_raw("ses_ghost");
        let gh = DigestHeader {
            version: DIGEST_VERSION,
            session: "ses_ghost".into(),
            root: root.clone(),
            title: None,
            created_at_ms: 1,
            updated_at_ms: 1,
            messages: 1,
            tz_offset_minutes: 0,
        };
        w.digests
            .write(&root, &ghost, &format!("{}\n正文", gh.front_matter()))
            .await
            .unwrap();

        let stats = w.reconcile_inner().await;
        assert_eq!(stats.rebuilt, 1, "只有 a 需要补");
        assert_eq!(stats.removed, 1, "孤儿要删");
        assert!(
            w.digests
                .path_of(&root, &SessionId::from_raw("ses_a"))
                .exists()
        );
        assert!(!w.digests.path_of(&root, &ghost).exists());
        assert_eq!(
            riot_store::digests::mtime_ms(&b_path).await.unwrap(),
            b_mtime,
            "新鲜的摘录不该被重写"
        );
        let idx = std::fs::read_to_string(w.digests.index_path(&root)).unwrap();
        assert!(
            idx.contains("ses_a.md") && idx.contains("ses_b.md") && !idx.contains("ghost"),
            "{idx}"
        );
    }

    /// 格式版本变了要重建，哪怕 mtime 更新。
    #[tokio::test]
    async fn 对账_版本不符要重建() {
        let (_d, w, transcripts) = setup();
        let root = PathBuf::from("/tmp/proj");
        let id = SessionId::from_raw("ses_a");
        let log = transcripts.open(riot_store::TranscriptMeta {
            id: id.clone(),
            root: root.clone(),
            created_at_ms: 1,
        });
        log.append(&user("m1", "内容", 1));
        log.flush().await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let old = DigestHeader {
            version: DIGEST_VERSION + 100,
            session: "ses_a".into(),
            root: root.clone(),
            title: None,
            created_at_ms: 1,
            updated_at_ms: 1,
            messages: 1,
            tz_offset_minutes: 0,
        };
        w.digests
            .write(&root, &id, &format!("{}\n旧版正文", old.front_matter()))
            .await
            .unwrap();
        let stats = w.reconcile_inner().await;
        assert_eq!(stats.rebuilt, 1);
        let text = std::fs::read_to_string(w.digests.path_of(&root, &id)).unwrap();
        assert!(
            text.contains(&format!("riot_digest: {DIGEST_VERSION}")),
            "{text}"
        );
    }
}
