//! 子 agent 登记表的落盘快照：`subagents/<会话>/tasks.json`。
//!
//! 登记表本身（内核 `tasks::BackgroundTasks`）住在内存里。没有这份快照，
//! 内核一重启它就是空的：Task 卡片认领不到自己的子 agent、后台任务面板
//! 一片空白、点开子 agent 只能看到"记录已不在"—— 而它的 transcript
//! 明明还躺在旁边（同目录的 `<agent>.jsonl`）。快照记的正是 transcript
//! 里**没有**的那部分：标题、类型、模型、是哪次 Task 调用开的、父子
//! 关系、收场状态和用量。消息本身不进这里，看和续接时读 transcript。
//!
//! # 整份覆盖，不是追加
//!
//! 一个会话的子 agent 最多几十个、每条几百字节，整份重写比"追加 + 加载
//! 时合并"简单，也不会出现半行。先写临时文件再改名：崩在写的中途留下
//! 的是上一份完整快照，不是半个 JSON。
//!
//! # 写盘在后台
//!
//! 和 [`crate::SessionLog`] 同一形状：[`TaskIndex::save`] 只把快照放进
//! 通道就返回，登记表的锁不等磁盘；写任务惰性启动，同一批里只写最后
//! 一份（连续几次 start/finish 合成一次写）。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt as _;
use tokio::sync::{mpsc, oneshot};

use riot_protocol::task::BackgroundTaskView;

/// 文件名。放在会话的子 agent 目录下，和 `<agent>.jsonl` 并排；
/// [`crate::Transcripts::scan`] 只认 `.jsonl`，不会把它当成一个子 agent。
const FILE_NAME: &str = "tasks.json";

/// 文件的形状。包一层对象而不是裸数组：以后要加字段（版本号、写入
/// 时刻）不用换文件名。
#[derive(Debug, Default, Serialize, Deserialize)]
struct IndexFile {
    #[serde(default)]
    tasks: Vec<BackgroundTaskView>,
}

enum Cmd {
    Save(Vec<BackgroundTaskView>),
    Flush(oneshot::Sender<()>),
}

/// 一个会话的子 agent 登记表快照句柄。由 [`crate::Transcripts::task_index`]
/// 造，只对 [`crate::Transcripts::subagents_of`] 返回的目录有意义。
pub struct TaskIndex {
    path: PathBuf,
    tx: OnceLock<mpsc::UnboundedSender<Cmd>>,
}

impl TaskIndex {
    pub(crate) fn new(dir: &Path) -> Self {
        Self {
            path: dir.join(FILE_NAME),
            tx: OnceLock::new(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 写一份快照。立刻返回，真正写盘在后台。
    ///
    /// `[约束]` 写失败只告警不上抛。快照是缓存：丢了它，重启后的表现退回
    /// "登记表一片空白"的老样子，不该为它打断正在跑的子 agent。
    pub fn save(&self, tasks: Vec<BackgroundTaskView>) {
        let tx = self.tx.get_or_init(|| {
            let (tx, rx) = mpsc::unbounded_channel();
            tokio::spawn(write_loop(self.path.clone(), rx));
            tx
        });
        if tx.send(Cmd::Save(tasks)).is_err() {
            tracing::debug!(path = %self.path.display(), "登记表写入任务已退出，丢弃一份快照");
        }
    }

    /// 等所有已提交的快照落盘。退出钩子和测试用；从没写过时是空操作。
    pub async fn flush(&self) {
        let Some(tx) = self.tx.get() else { return };
        let (ack_tx, ack_rx) = oneshot::channel();
        if tx.send(Cmd::Flush(ack_tx)).is_ok() {
            let _ = ack_rx.await;
        }
    }

    /// 读回快照。没有文件 = 这个会话没开过子 agent（或早于本功能），
    /// 不是错误；读不懂 = 告警 + 空 —— 登记表回到重启前的老行为，不比
    /// 以前差，而删数据的决定永远留给用户。
    pub async fn load(&self) -> Vec<BackgroundTaskView> {
        let bytes = match tokio::fs::read(&self.path).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(e) => {
                tracing::warn!(error = %e, path = %self.path.display(), "子 agent 登记表读不出来，本次按空处理");
                return Vec::new();
            }
        };
        match serde_json::from_slice::<IndexFile>(&bytes) {
            Ok(f) => f.tasks,
            Err(e) => {
                tracing::warn!(error = %e, path = %self.path.display(), "子 agent 登记表读不懂，本次按空处理");
                Vec::new()
            }
        }
    }
}

async fn write_loop(path: PathBuf, mut rx: mpsc::UnboundedReceiver<Cmd>) {
    let mut batch: Vec<Cmd> = Vec::new();
    loop {
        batch.clear();
        let n = rx.recv_many(&mut batch, 64).await;
        if n == 0 {
            break;
        }
        let mut latest: Option<Vec<BackgroundTaskView>> = None;
        let mut acks: Vec<oneshot::Sender<()>> = Vec::new();
        for cmd in batch.drain(..) {
            match cmd {
                Cmd::Save(tasks) => latest = Some(tasks),
                Cmd::Flush(ack) => acks.push(ack),
            }
        }
        if let Some(tasks) = latest
            && let Err(e) = write_atomic(&path, &tasks).await
        {
            tracing::error!(error = %e, path = %path.display(), "子 agent 登记表写失败，重启后这些子 agent 的记录找不回");
        }
        for ack in acks {
            let _ = ack.send(());
        }
    }
}

/// 先写临时文件再改名，崩在中途也不会留下半个 JSON。
async fn write_atomic(path: &Path, tasks: &[BackgroundTaskView]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let json = serde_json::to_vec(&IndexFile {
        tasks: tasks.to_vec(),
    })
    .map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = tokio::fs::File::create(&tmp).await?;
        f.write_all(&json).await?;
        f.flush().await?;
    }
    tokio::fs::rename(&tmp, path).await
}

// 豁免理由：测试直接读临时目录里的文件验证落盘结果。
#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::Transcripts;
    use riot_protocol::id::{AgentId, SessionId, ToolUseId};
    use riot_protocol::task::BackgroundTaskStatus;
    use riot_protocol::ui_text;

    fn view(id: &str) -> BackgroundTaskView {
        BackgroundTaskView {
            id: AgentId::from_raw(id),
            title: format!("任务 {id}"),
            kind: "explore".into(),
            model: "m".into(),
            background: true,
            tool_use_id: ToolUseId::from_raw(format!("tu_{id}")),
            parent: None,
            status: BackgroundTaskStatus::Completed,
            activity: ui_text!("kernel.task.activity.completed"),
            tool_uses: 3,
            tokens: 1200,
            started_at_ms: 10,
            finished_at_ms: Some(20),
        }
    }

    #[tokio::test]
    async fn 写下的快照能原样读回_且最后一份为准() {
        let d = tempfile::tempdir().expect("临时目录");
        let store = Transcripts::new(d.path());
        let sub = store.subagents_of(&SessionId::from_raw("s1"));
        let index = sub.task_index();

        index.save(vec![view("a")]);
        index.save(vec![view("a"), view("b")]);
        index.flush().await;

        assert!(
            d.path().join("subagents/s1/tasks.json").is_file(),
            "落在会话的子 agent 目录下"
        );
        let back = sub.task_index().load().await;
        assert_eq!(back, vec![view("a"), view("b")], "同一批里最后一份为准");
        assert!(
            store.scan().is_empty(),
            "快照不是 .jsonl，索引重建不该把它当成会话"
        );
    }

    #[tokio::test]
    async fn 没有文件时读回空() {
        let d = tempfile::tempdir().expect("临时目录");
        let sub = Transcripts::new(d.path()).subagents_of(&SessionId::from_raw("s1"));
        assert!(sub.task_index().load().await.is_empty());
    }

    #[tokio::test]
    async fn 读不懂的文件按空处理且不删() {
        let d = tempfile::tempdir().expect("临时目录");
        let dir = d.path().join("subagents/s1");
        std::fs::create_dir_all(&dir).expect("建目录");
        std::fs::write(dir.join("tasks.json"), "{坏掉了").expect("写坏文件");
        let sub = Transcripts::new(d.path()).subagents_of(&SessionId::from_raw("s1"));
        assert!(sub.task_index().load().await.is_empty());
        assert!(dir.join("tasks.json").exists(), "删数据的决定留给用户");
    }

    #[tokio::test]
    async fn 从未写过时flush是空操作() {
        let d = tempfile::tempdir().expect("临时目录");
        let sub = Transcripts::new(d.path()).subagents_of(&SessionId::from_raw("s1"));
        sub.task_index().flush().await;
        assert!(!d.path().join("subagents/s1/tasks.json").exists());
    }
}
