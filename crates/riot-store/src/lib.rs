//! 会话 transcript 的落盘与重放。
//!
//! # 形态：append-only JSONL，每会话一个文件
//!
//! 这是 Claude Code 和 Codex 各自演化后收敛到的同一个结构，两边的理由都成立：
//!
//! - **首行是 [`Record::Meta`]**（会话的不变事实），之后每行一条进入历史的
//!   消息 —— Codex rollout 的布局。文件自带身份，索引丢了也能凭它重建列表。
//! - **消息边产生边追加**，由后台任务写盘，不阻塞 agent 主循环 —— 两家都这么做。
//!   崩溃或强杀最多丢通道里还没落盘的几条，而不是整轮。
//! - **加载时坏行跳过**：JSONL 的局部损坏只吞掉那一行；换成单个大 JSON 的话，
//!   同样的损坏吞掉的是整个会话。
//!
//! `[取舍]` 不是 SQLite（尽管 ARCHITECTURE.md 早期规划这么标过）。两个成熟实现
//! 的实践都是"正文放 JSONL 文件，数据库只做索引"：文件天然 append-only、可 grep、
//! 可单独备份，没有 schema 迁移问题。以后要快速检索，在旁边补索引即可，正文格式
//! 不用动。
//!
//! `[约束]` 一个会话同一时刻只能有一个 [`SessionLog`]。两个句柄各持一个 append
//! fd 时，POSIX 只保证单次 write 不撕裂，行与行的顺序没有保证。这条由调用方的
//! 所有权结构保证（宿主的 Session 独占句柄），这里不加锁 —— 锁也只能防同进程。
//!
//! # 为什么这里直接用真实文件系统
//!
//! clippy 的禁用清单（FileSystem trait）是给内核逻辑做黄金回放用的；持久化层
//! 正是那个抽象的"真实一侧"，mock 它没有意义。测试用临时目录注入路径。

use std::io::BufRead as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncSeekExt as _, AsyncWriteExt as _};
use tokio::sync::{mpsc, oneshot};

use riot_protocol::id::SessionId;
use riot_protocol::message::{Message, UserContent};

/// transcript 里的一行。
///
/// `[约束]` 用 `type` 标签区分。将来加新记录类型（压缩边界、检查点）时，
/// 旧版本的加载器会把不认识的行当坏行跳过而不是整个文件读不了 ——
/// 这正是"坏行跳过"策略额外买到的向前兼容。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Record {
    /// 首行：会话的不变事实。
    Meta(TranscriptMeta),
    /// 一条进入历史的消息。
    Message { message: Box<Message> },
    /// 压缩边界（Codex rollout 的 Compacted 同款语义）：**活历史从这里
    /// 重新开始**。加载时丢弃它之前累积的消息 —— 边界后的第一条通常是
    /// 带总结的续接消息。边界之前的内容留在文件里可审计，只是不再进
    /// 内存和请求。
    CompactBoundary {
        /// 压缩前后的规模，诊断用。
        before_tokens: u32,
        after_tokens: u32,
    },
    /// 重新生成：丢掉 `keep_until` 之后的历史。
    ///
    /// 加载时截断到这条消息（含）。之后继续追加的是新生成的回复。
    /// 旧加载器不认识这个类型会当坏行跳过 —— 那会把本该丢掉的回复
    /// 读回来，所以新版本必须处理它，不能靠"坏行跳过"凑合。
    Rewind {
        keep_until: String,
    },
}

/// 会话的不变事实，写在 transcript 首行。
///
/// 索引（宿主的 index.json）损坏时靠它重建会话列表 —— **transcript 是事实
/// 来源，索引只是缓存**（Codex 的 SQLite index + reconcile 同一哲学）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptMeta {
    pub id: SessionId,
    /// 会话绑定的项目根（创建时已规范化）。
    pub root: PathBuf,
    /// 创建时刻（Unix 毫秒）。由调用方传入 —— store 自己不碰时钟，
    /// 测试才能给出确定的值。
    pub created_at_ms: u64,
}

/// 一次读盘的拆分结果：模型看 `live`，界面看 `archived` + 分割线 + `live`。
#[derive(Debug, Clone, Default)]
pub struct LoadedTranscript {
    pub meta: Option<TranscriptMeta>,
    /// 最后一条压缩边界之后的活历史。
    pub live: Vec<Message>,
    /// 边界之前的消息。文件里还在，只是不再进模型上下文。
    pub archived: Vec<Message>,
}

/// 扫描一个 transcript 得到的摘要。索引重建用。
#[derive(Debug, Clone)]
pub struct ScannedTranscript {
    pub meta: TranscriptMeta,
    /// 第一条用户消息的文本。标题重建用；没说过话就是 None。
    pub first_prompt: Option<String>,
}

/// 一个目录下所有会话的 transcript。
pub struct Transcripts {
    dir: PathBuf,
}

impl Transcripts {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path_of(&self, id: &SessionId) -> PathBuf {
        // id 是 nanoid（URL-safe 字符集），直接当文件名是安全的。
        self.dir.join(format!("{}.jsonl", id.as_str()))
    }

    /// 拿到一个会话的追加句柄。
    ///
    /// 此刻**不创建文件**，首次真正追加时才建（Claude Code 的做法）：
    /// 否则每个"点了 + 又没说话"的会话都在磁盘上留一个只有元数据的空壳。
    pub fn open(&self, meta: TranscriptMeta) -> SessionLog {
        SessionLog {
            path: self.path_of(&meta.id),
            meta,
            tx: OnceLock::new(),
        }
    }

    /// 读回一个会话的全部消息。文件不存在 = 还没说过话，不是错误。
    ///
    /// `[约束]` 坏行跳过，不让单行损坏吞掉整个会话。最常见的坏行是崩溃时
    /// 写了一半的最后一行 —— 跳过它等于恢复到最后一条完整消息，正是想要
    /// 的语义。跳过要留日志：静默丢历史和静默丢配置一样，是最难排查的
    /// 一类"我的东西没了"。
    pub async fn load(&self, id: &SessionId) -> (Option<TranscriptMeta>, Vec<Message>) {
        let parts = self.load_parts(id).await;
        (parts.meta, parts.live)
    }

    /// 一次读盘拆出活历史和压缩前的归档。
    ///
    /// 模型只看 `live`（边界之后）。界面要画完整对话流，归档还在文件里，
    /// 丢掉的话用户切回会话只能看到一条"已压缩"，以为聊天记录没了。
    pub async fn load_parts(&self, id: &SessionId) -> LoadedTranscript {
        let empty = LoadedTranscript { meta: None, live: Vec::new(), archived: Vec::new() };
        let path = self.path_of(id);
        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return empty,
            Err(e) => {
                tracing::error!(error = %e, path = %path.display(), "transcript 读不出来，本次按空历史处理");
                return empty;
            }
        };

        let mut meta = None;
        let mut live = Vec::new();
        let mut archived = Vec::new();
        let mut skipped = 0usize;
        for (i, line) in bytes.split(|&b| b == b'\n').enumerate() {
            if line.is_empty() {
                continue;
            }
            match serde_json::from_slice::<Record>(line) {
                // 只认第一个 Meta。正常文件只有首行一个；损坏后修复的文件
                // 理论上可能出现第二个，后到的不能顶掉会话的真实身份。
                Ok(Record::Meta(m)) => {
                    if meta.is_none() {
                        meta = Some(m);
                    }
                }
                Ok(Record::Message { message }) => live.push(*message),
                Ok(Record::CompactBoundary { .. }) => {
                    // 活历史从边界重新开始。归档留下给界面画分割线上面的记录。
                    archived.append(&mut live);
                }
                Ok(Record::Rewind { keep_until }) => {
                    apply_rewind(&mut live, &mut archived, &keep_until);
                }
                Err(e) => {
                    skipped += 1;
                    tracing::debug!(line = i + 1, error = %e, "transcript 有读不懂的行");
                }
            }
        }
        if skipped > 0 {
            tracing::warn!(
                path = %path.display(),
                skipped,
                "transcript 里有 {skipped} 行读不懂，已跳过（多半是上次崩溃留下的半行）"
            );
        }
        LoadedTranscript { meta, live, archived }
    }

    /// 扫描目录里所有 transcript 的首行元数据和第一句用户输入。
    ///
    /// 索引丢失/损坏时重建列表靠它。每个文件只读到第一条消息为止，
    /// 不 parse 全文 —— 这条路径只在恢复场景走，但也不该为它扫几百 MB。
    pub fn scan(&self) -> Vec<ScannedTranscript> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            match scan_one(&path) {
                Some(s) => out.push(s),
                // 首行都读不出来的文件没法安全恢复（不知道 root），跳过但
                // 保留文件 —— 删数据的决定永远留给用户。
                None => tracing::warn!(path = %path.display(), "transcript 首行不是元数据，无法恢复这个会话"),
            }
        }
        out
    }

    /// 删掉一个会话的 transcript。不存在不是错误 —— 和 delete_session 的
    /// 幂等语义对齐。
    ///
    /// `[约束]` 调用前必须先 [`SessionLog::shutdown`]。Windows 上删除还被
    /// 打开的文件会失败。
    pub async fn remove(&self, id: &SessionId) -> std::io::Result<()> {
        match tokio::fs::remove_file(self.path_of(id)).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// 加载时应用一条 rewind：先在活历史里找，找不到再看归档。
/// 找不到就不动（文件坏了也不该把整份历史扔掉）。
fn apply_rewind(live: &mut Vec<Message>, archived: &mut Vec<Message>, keep_until: &str) {
    if let Some(i) = live.iter().position(|m| m.id().as_str() == keep_until) {
        live.truncate(i + 1);
        return;
    }
    if let Some(i) = archived.iter().position(|m| m.id().as_str() == keep_until) {
        archived.truncate(i + 1);
        live.clear();
    }
}

fn scan_one(path: &Path) -> Option<ScannedTranscript> {
    let f = std::fs::File::open(path).ok()?;
    let mut lines = std::io::BufReader::new(f).lines();

    let first = lines.next()?.ok()?;
    let Ok(Record::Meta(meta)) = serde_json::from_str::<Record>(&first) else {
        return None;
    };

    // 找第一条消息。正常情况它就是第二行（历史总以用户消息开头）；
    // 上限兜住"前面塞了一堆坏行"的病态文件，别在重建路径上读到天荒地老。
    let mut first_prompt = None;
    for line in lines.take(50) {
        let Ok(line) = line else { break };
        if let Ok(Record::Message { message }) = serde_json::from_str::<Record>(&line) {
            if let Message::User { content, .. } = message.as_ref() {
                first_prompt = content.iter().find_map(|c| match c {
                    UserContent::Text { text } => {
                        let t = text.trim();
                        (!t.is_empty()).then(|| t.to_owned())
                    }
                    _ => None,
                });
            }
            break;
        }
    }
    Some(ScannedTranscript { meta, first_prompt })
}

enum Cmd {
    Append(Box<Message>),
    /// 压缩边界。
    Boundary { before_tokens: u32, after_tokens: u32 },
    /// 重新生成：截断到这条消息。
    Rewind { keep_until: String },
    /// 等所有已提交的追加真正写进文件。
    Flush(oneshot::Sender<()>),
    /// 写完手上的、关掉文件句柄、此后 append 静默丢弃。
    Shutdown(oneshot::Sender<()>),
}

/// 一个会话的追加句柄。
///
/// 写盘在后台任务里（Codex `RolloutWriterTask` 的形状）：`append` 只是往
/// 通道里放一条命令，agent 主循环不等磁盘。任务惰性启动 —— 恢复会话表
/// 发生在 tokio runtime 起来之前，那时只造句柄不 spawn。
pub struct SessionLog {
    path: PathBuf,
    meta: TranscriptMeta,
    tx: OnceLock<mpsc::UnboundedSender<Cmd>>,
}

impl SessionLog {
    fn sender(&self) -> &mpsc::UnboundedSender<Cmd> {
        self.tx.get_or_init(|| {
            let (tx, rx) = mpsc::unbounded_channel();
            tokio::spawn(write_loop(self.path.clone(), self.meta.clone(), rx));
            tx
        })
    }

    /// 追加一条消息。立刻返回，真正的写盘在后台。
    ///
    /// `[约束]` 写失败只告警不上抛。持久化故障不该打断正在跑的轮次 ——
    /// 代价是这之后的历史重启找不回，但日志里有痕迹，而对话此刻还活着。
    pub fn append(&self, m: &Message) {
        if self.sender().send(Cmd::Append(Box::new(m.clone()))).is_err() {
            // shutdown 之后还有消息到达：会话正在被删除，丢弃是正确行为。
            tracing::debug!(path = %self.path.display(), "写入任务已关闭，丢弃一条追加");
        }
    }

    /// 追加一条压缩边界：告诉未来的加载者"活历史从下一条消息重新开始"。
    ///
    /// 必须在续接消息 `append` **之前**调用 —— 顺序反了的话，重启加载
    /// 会把续接消息也一起丢掉，会话醒来就是一片空白。
    pub fn append_boundary(&self, before_tokens: u32, after_tokens: u32) {
        let tx = self.sender();
        if tx.send(Cmd::Boundary { before_tokens, after_tokens }).is_err() {
            tracing::debug!(path = %self.path.display(), "写入任务已关闭，丢弃压缩边界");
        }
    }

    /// 记下一次重新生成的截断点。必须在内存历史已经截完之后调用。
    pub fn append_rewind(&self, keep_until: &str) {
        if self
            .sender()
            .send(Cmd::Rewind {
                keep_until: keep_until.to_owned(),
            })
            .is_err()
        {
            tracing::debug!(path = %self.path.display(), "写入任务已关闭，丢弃截断记录");
        }
    }

    /// 等所有已提交的追加落盘。退出钩子用；从没写过东西时是空操作。
    pub async fn flush(&self) {
        self.ack(Cmd::Flush).await;
    }

    /// 落盘并关闭文件句柄。删除会话前必须调用（Windows 删不掉开着的文件）。
    pub async fn shutdown(&self) {
        self.ack(Cmd::Shutdown).await;
    }

    async fn ack(&self, make: impl FnOnce(oneshot::Sender<()>) -> Cmd) {
        // 任务都没启动过 = 一条消息都没写过，没有要等的东西。
        let Some(tx) = self.tx.get() else { return };
        let (ack_tx, ack_rx) = oneshot::channel();
        if tx.send(make(ack_tx)).is_ok() {
            // 任务崩了（不该发生）也只是收不到 ack，不能让调用方挂死。
            let _ = ack_rx.await;
        }
    }
}

async fn write_loop(path: PathBuf, meta: TranscriptMeta, mut rx: mpsc::UnboundedReceiver<Cmd>) {
    let mut file: Option<tokio::fs::File> = None;
    let mut batch: Vec<Cmd> = Vec::new();
    loop {
        batch.clear();
        // 合批：流式输出快时一次醒来写一整批，一次 write 一次 flush。
        let n = rx.recv_many(&mut batch, 256).await;
        if n == 0 {
            break; // 所有发送端没了：会话被 drop，手上没有未写的东西
        }

        let mut lines = String::new();
        let mut acks: Vec<oneshot::Sender<()>> = Vec::new();
        let mut shutdown = false;
        for cmd in batch.drain(..) {
            let record = match cmd {
                Cmd::Append(m) => Some(Record::Message { message: m }),
                Cmd::Boundary { before_tokens, after_tokens } => {
                    Some(Record::CompactBoundary { before_tokens, after_tokens })
                }
                Cmd::Rewind { keep_until } => Some(Record::Rewind { keep_until }),
                Cmd::Flush(ack) => {
                    acks.push(ack);
                    None
                }
                Cmd::Shutdown(ack) => {
                    acks.push(ack);
                    shutdown = true;
                    None
                }
            };
            if let Some(r) = record {
                match serde_json::to_string(&r) {
                    Ok(s) => {
                        lines.push_str(&s);
                        lines.push('\n');
                    }
                    // Record 全是可序列化的普通数据，走到这里是代码错误，
                    // 但持久化层不能 panic —— 丢这一条并留痕。
                    Err(e) => tracing::error!(error = %e, "记录序列化失败，这条不落盘"),
                }
            }
        }

        if !lines.is_empty() {
            match ensure_file(&mut file, &path, &meta).await {
                Ok(f) => {
                    if let Err(e) = f.write_all(lines.as_bytes()).await {
                        tracing::error!(error = %e, path = %path.display(), "transcript 追加失败，这批消息重启后找不回");
                    // `[约束]` write_all 之后必须 flush。
                    //
                    // tokio 的 File 只是把写排进阻塞线程池就返回 ——
                    // 字节还没交给操作系统。少了这一步，下面的 ack 就在
                    // 撒谎："已提交的追加都落盘了"其实是"都排队了"，
                    // 退出钩子等到的 ack 挡不住数据丢失，机器越忙窗口越大。
                    //
                    // 这不是 fsync：只保证交给了 OS，不保证扛断电。
                    } else if let Err(e) = f.flush().await {
                        tracing::error!(error = %e, path = %path.display(), "transcript 刷盘失败，这批消息重启后找不回");
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, path = %path.display(), "transcript 打不开，这批消息重启后找不回");
                }
            }
        }

        // ack 在写完之后发 —— flush 的语义是"到此为止的都在盘上了"。
        for ack in acks {
            let _ = ack.send(());
        }
        if shutdown {
            break; // file 随之 drop，句柄关闭，之后文件可以被安全删除
        }
    }
}

/// 打开（或创建）transcript 文件。新文件先写 Meta 首行。
async fn ensure_file<'a>(
    slot: &'a mut Option<tokio::fs::File>,
    path: &Path,
    meta: &TranscriptMeta,
) -> std::io::Result<&'a mut tokio::fs::File> {
    if slot.is_none() {
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        let file = match tokio::fs::OpenOptions::new()
            .append(true)
            .create_new(true)
            .open(path)
            .await
        {
            Ok(mut f) => {
                let line = serde_json::to_string(&Record::Meta(meta.clone()))
                    .map_err(std::io::Error::other)?;
                f.write_all(format!("{line}\n").as_bytes()).await?;
                f
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let mut f = tokio::fs::OpenOptions::new().append(true).open(path).await?;
                repair_tail(&mut f, path, meta).await?;
                f
            }
            Err(e) => return Err(e),
        };
        *slot = Some(file);
    }
    Ok(slot.as_mut().expect("上面刚放进去"))
}

/// 续写已有文件前的两个修复（Codex recorder 的做法）：
///
/// - 上次崩溃可能留下没有换行的半行。先补一个换行把残行隔离掉 ——
///   加载器把它当坏行跳过；不补的话新消息会粘在残行后面，**两条**一起变坏。
/// - 文件被外力清空过（长度 0）的话，Meta 首行也没了，重写一个 ——
///   否则这个会话从此再也进不了索引重建。
async fn repair_tail(
    f: &mut tokio::fs::File,
    path: &Path,
    meta: &TranscriptMeta,
) -> std::io::Result<()> {
    let len = f.metadata().await?.len();
    if len == 0 {
        let line = serde_json::to_string(&Record::Meta(meta.clone()))
            .map_err(std::io::Error::other)?;
        f.write_all(format!("{line}\n").as_bytes()).await?;
        return Ok(());
    }

    // 读最后一个字节。用独立的读句柄 —— 追加句柄的游标语义在各平台不一致。
    let mut reader = tokio::fs::File::open(path).await?;
    reader.seek(std::io::SeekFrom::End(-1)).await?;
    let mut last = [0u8; 1];
    use tokio::io::AsyncReadExt as _;
    reader.read_exact(&mut last).await?;
    if last[0] != b'\n' {
        tracing::warn!(path = %path.display(), "transcript 尾部有半行（上次没退干净），已隔离");
        f.write_all(b"\n").await?;
    }
    Ok(())
}

// 豁免理由：测试直接读写临时目录里的文件来制造损坏、验证落盘结果。
#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use riot_protocol::id::MessageId;
    use riot_protocol::message::MessageMeta;

    fn meta(id: &str) -> TranscriptMeta {
        TranscriptMeta {
            id: SessionId::from_raw(id),
            root: PathBuf::from("/tmp/proj"),
            created_at_ms: 1_000,
        }
    }

    fn user(id: &str, text: &str) -> Message {
        Message::User {
            id: MessageId::from_raw(id),
            content: vec![UserContent::Text { text: text.into() }],
            meta: MessageMeta::default(),
        }
    }

    fn assistant(id: &str, text: &str) -> Message {
        Message::Assistant {
            id: MessageId::from_raw(id),
            content: vec![riot_protocol::message::AssistantContent::Text {
                text: text.into(),
            }],
            usage: None,
            meta: MessageMeta::default(),
        }
    }

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("临时目录")
    }

    #[tokio::test]
    async fn 追加的消息能原样读回() {
        let d = dir();
        let store = Transcripts::new(d.path());
        let log = store.open(meta("s1"));

        log.append(&user("m1", "你好"));
        log.append(&user("m2", "第二句"));
        log.flush().await;

        let (m, msgs) = store.load(&SessionId::from_raw("s1")).await;
        assert_eq!(m, Some(meta("s1")), "首行元数据要读得回来");
        assert_eq!(msgs, vec![user("m1", "你好"), user("m2", "第二句")]);
    }

    #[tokio::test]
    async fn 追加前不建文件() {
        // 每个"点了 + 又没说话"的会话都留一个空壳文件的话，
        // 目录会越积越多，索引重建也要一个个扫它们。
        let d = dir();
        let store = Transcripts::new(d.path());
        let _log = store.open(meta("s1"));
        assert!(
            !d.path().join("s1.jsonl").exists(),
            "没写过东西就不该有文件"
        );
    }

    #[tokio::test]
    async fn 没有文件时加载为空() {
        let store = Transcripts::new(dir().path());
        let (m, msgs) = store.load(&SessionId::from_raw("ghost")).await;
        assert!(m.is_none());
        assert!(msgs.is_empty(), "没说过话不是错误");
    }

    #[tokio::test]
    async fn 截断的尾行被跳过() {
        // 崩溃时最后一行只写了一半是最常见的损坏。它只该吞掉自己，
        // 前面的完整消息一条不能少。
        let d = dir();
        let store = Transcripts::new(d.path());
        let log = store.open(meta("s1"));
        log.append(&user("m1", "完整的一条"));
        log.flush().await;

        let path = d.path().join("s1.jsonl");
        let mut raw = std::fs::read_to_string(&path).expect("读原文");
        raw.push_str(r#"{"type":"message","message":{"role":"user","id":"m2","co"#);
        std::fs::write(&path, raw).expect("写截断");

        let (_, msgs) = store.load(&SessionId::from_raw("s1")).await;
        assert_eq!(msgs, vec![user("m1", "完整的一条")]);
    }

    #[tokio::test]
    async fn 中间的坏行不影响后面的() {
        let d = dir();
        let store = Transcripts::new(d.path());
        let log = store.open(meta("s1"));
        log.append(&user("m1", "前"));
        log.flush().await;

        let path = d.path().join("s1.jsonl");
        let mut raw = std::fs::read_to_string(&path).expect("读原文");
        raw.push_str("{坏行}\n");
        std::fs::write(&path, raw).expect("写坏行");

        let log = store.open(meta("s1"));
        log.append(&user("m2", "后"));
        log.flush().await;

        let (_, msgs) = store.load(&SessionId::from_raw("s1")).await;
        assert_eq!(msgs, vec![user("m1", "前"), user("m2", "后")]);
    }

    #[tokio::test]
    async fn 续写截断的文件先隔离残行() {
        // 不隔离的话，新消息粘在残行后面，两条一起变坏 ——
        // 用户丢的就不只是崩溃前的半句，还有崩溃后的新对话。
        let d = dir();
        let store = Transcripts::new(d.path());
        let log = store.open(meta("s1"));
        log.append(&user("m1", "崩溃前"));
        log.flush().await;

        let path = d.path().join("s1.jsonl");
        let mut raw = std::fs::read_to_string(&path).expect("读原文");
        raw.push_str(r#"{"type":"message","mess"#); // 半行，没有换行
        std::fs::write(&path, raw).expect("写截断");

        let log = store.open(meta("s1"));
        log.append(&user("m2", "崩溃后"));
        log.flush().await;

        let (_, msgs) = store.load(&SessionId::from_raw("s1")).await;
        assert_eq!(msgs, vec![user("m1", "崩溃前"), user("m2", "崩溃后")]);
    }

    #[tokio::test]
    async fn 清空过的文件续写时补回元数据首行() {
        let d = dir();
        let store = Transcripts::new(d.path());
        let log = store.open(meta("s1"));
        log.append(&user("m1", "x"));
        log.flush().await;

        std::fs::write(d.path().join("s1.jsonl"), "").expect("清空");

        let log = store.open(meta("s1"));
        log.append(&user("m2", "重来"));
        log.flush().await;

        let (m, msgs) = store.load(&SessionId::from_raw("s1")).await;
        // 断言消息带上文件原文：这个用例在重负载下偶发过一次失败，没抓到
        // 现场。写失败在 store 里是"告警 + 继续"（设计如此），所以万一再
        // 偶发，唯一能定位的证据就是盘上到底写进了什么。
        let raw = std::fs::read_to_string(d.path().join("s1.jsonl")).unwrap_or_default();
        assert_eq!(
            m,
            Some(meta("s1")),
            "元数据首行要补回来，否则索引重建永远看不见它。文件原文：{raw:?}"
        );
        assert_eq!(msgs, vec![user("m2", "重来")], "文件原文：{raw:?}");
    }

    #[tokio::test]
    async fn 扫描读出元数据和第一句话() {
        let d = dir();
        let store = Transcripts::new(d.path());

        let log = store.open(meta("s1"));
        log.append(&user("m1", "  第一句  "));
        log.append(&user("m2", "第二句"));
        log.flush().await;

        let scanned = store.scan();
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].meta, meta("s1"));
        assert_eq!(
            scanned[0].first_prompt.as_deref(),
            Some("第一句"),
            "标题重建要拿到去掉空白的第一句"
        );
    }

    #[tokio::test]
    async fn 扫描跳过首行损坏的文件但不删它() {
        let d = dir();
        std::fs::write(d.path().join("bad.jsonl"), "{不是元数据}\n").expect("写坏文件");
        let store = Transcripts::new(d.path());
        assert!(store.scan().is_empty(), "首行读不出 root，没法安全恢复");
        assert!(d.path().join("bad.jsonl").exists(), "删数据的决定要留给用户");
    }

    #[tokio::test]
    async fn 删除是幂等的() {
        let d = dir();
        let store = Transcripts::new(d.path());
        let log = store.open(meta("s1"));
        log.append(&user("m1", "x"));
        log.shutdown().await;

        store.remove(&SessionId::from_raw("s1")).await.expect("第一次删");
        store.remove(&SessionId::from_raw("s1")).await.expect("再删一次不是错误");
        assert!(!d.path().join("s1.jsonl").exists());
    }

    #[tokio::test]
    async fn 关闭后追加静默丢弃不恐慌() {
        // 删除会话和收尾的轮次是并发的：轮子最后几条消息可能在 shutdown
        // 之后到达。它们该被丢弃（会话都要删了），但绝不能 panic。
        let d = dir();
        let store = Transcripts::new(d.path());
        let log = store.open(meta("s1"));
        log.append(&user("m1", "x"));
        log.shutdown().await;
        log.append(&user("m2", "迟到的"));
        log.flush().await; // 也不能挂死

        let (_, msgs) = store.load(&SessionId::from_raw("s1")).await;
        assert_eq!(msgs, vec![user("m1", "x")]);
    }

    #[tokio::test]
    async fn 压缩边界之后活历史重新开始() {
        // 边界前的消息已被总结吞并。一起加载的话，总结 + 原文双份内容
        // 会把上下文撑得比压缩前还大 —— 压缩等于白做。
        let d = dir();
        let store = Transcripts::new(d.path());
        let log = store.open(meta("s1"));
        log.append(&user("m1", "压缩前的旧话"));
        log.append(&user("m2", "也被总结吞掉"));
        log.append_boundary(10_000, 800);
        log.append(&user("m3", "带总结的续接消息"));
        log.append(&user("m4", "压缩后的新对话"));
        log.flush().await;

        let (m, msgs) = store.load(&SessionId::from_raw("s1")).await;
        assert_eq!(m, Some(meta("s1")), "元数据不受边界影响");
        assert_eq!(
            msgs,
            vec![user("m3", "带总结的续接消息"), user("m4", "压缩后的新对话")],
            "边界之前的消息不进活历史（文件里仍在，可审计）"
        );

        let parts = store.load_parts(&SessionId::from_raw("s1")).await;
        assert_eq!(
            parts.archived,
            vec![user("m1", "压缩前的旧话"), user("m2", "也被总结吞掉")],
            "界面要能画出压缩前的记录"
        );
        assert_eq!(parts.live, msgs);

        // 原文还在盘上 —— 边界丢的是加载，不是数据
        let raw = std::fs::read_to_string(d.path().join("s1.jsonl")).expect("读原文");
        assert!(raw.contains("压缩前的旧话"), "旧消息留在文件里可审计");
        assert!(raw.contains("compact_boundary"), "边界记录本身要落盘");
    }

    #[tokio::test]
    async fn 重新生成截断点之后的消息不再进活历史() {
        let d = dir();
        let store = Transcripts::new(d.path());
        let log = store.open(meta("s1"));
        log.append(&user("m1", "第一句"));
        log.append(&assistant("a1", "旧答"));
        log.append(&user("m2", "第二句"));
        log.append(&assistant("a2", "要丢掉的"));
        log.append_rewind("m2");
        log.append(&assistant("a3", "新答"));
        log.flush().await;

        let parts = store.load_parts(&SessionId::from_raw("s1")).await;
        assert_eq!(
            parts.live,
            vec![
                user("m1", "第一句"),
                assistant("a1", "旧答"),
                user("m2", "第二句"),
                assistant("a3", "新答"),
            ],
            "截断点之后、新追加之前的那条助手消息不该再出现"
        );
        assert!(parts.archived.is_empty());
    }

    #[tokio::test]
    async fn 重新生成可以截回压缩边界之前() {
        let d = dir();
        let store = Transcripts::new(d.path());
        let log = store.open(meta("s1"));
        log.append(&user("m1", "压缩前"));
        log.append(&assistant("a1", "旧答"));
        log.append_boundary(1000, 100);
        log.append(&user("m2", "压缩后"));
        log.append(&assistant("a2", "新答"));
        log.append_rewind("m1");
        log.flush().await;

        let parts = store.load_parts(&SessionId::from_raw("s1")).await;
        assert_eq!(parts.archived, vec![user("m1", "压缩前")]);
        assert!(parts.live.is_empty(), "截回归档之后活历史应清空");
    }

    #[tokio::test]
    async fn 从未写过时flush和shutdown是空操作() {
        let store = Transcripts::new(dir().path());
        let log = store.open(meta("s1"));
        log.flush().await;
        log.shutdown().await;
    }
}
