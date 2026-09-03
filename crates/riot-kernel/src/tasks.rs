//! 子 agent 登记表：会话里跑过的每个子 agent（同步的、后台的、分叉的）。
//!
//! # 为什么同步的也登记
//!
//! `resume=<agent id>` 要能续接任何一个跑过的子 agent —— 一次同步侦察
//! 回来的报告不够细，主 agent 说"再往下挖一层"，不该从头把背景讲一遍。
//! 续接需要它的全部历史，所以每条登记项留着子 agent 的消息（内存里；
//! transcript 在磁盘上另有一份，但那份是给人看和事后审计的，续接不走
//! 读盘 —— 测试里没有持久化通道时也得能续）。
//!
//! # 内存上限
//!
//! 一个侦察子 agent 的历史动辄几十次 Grep/Read 的结果，几百 KB 起步。
//! 只保留最近 [`KEEP_FINISHED`] 个**已结束**的登记项的历史，更早的只留
//! 视图（状态、用量），续接它会得到一句"太久了，已经不能续接"。跑着的
//! 永远不丢。
//!
//! # 界面看到的
//!
//! 每个子 agent 的状态变化都推 `BackgroundTask` 事件：后台任务面板只画
//! `background == true` 的；Task 工具卡片按 `tool_use_id` 认领自己的那个，
//! 直播"标题 · 模型 · 正在做什么"。点开任何一个都能看它的会话
//! （[`BackgroundTasks::history`]）—— 跑着的也能看，消息边产生边进登记表。

use riot_protocol::event::AgentEvent;
use riot_protocol::id::{AgentId, MessageId};
use riot_protocol::message::{Attachment, Message, MessageMeta, UserContent};
use riot_protocol::task::{BackgroundTaskStatus, BackgroundTaskView, TaskNotice};
use tokio_util::sync::CancellationToken;

use crate::session::SessionSink;
use crate::subagent::Kind;

/// 保留完整历史的已结束登记项数量。
const KEEP_FINISHED: usize = 24;

struct Entry {
    view: BackgroundTaskView,
    kind: Kind,
    cancel: CancellationToken,
    /// 子 agent 的全部历史：起跑那份 + 边跑边追加的。跑完时被完整的那份
    /// 顶掉一次（内容相同，只是对齐 run_job 的口径）。`None` = 已经太久
    /// 被瘦身掉。续接和界面都从这里读。
    messages: Option<Vec<Message>>,
    /// 界面从第几条开始看。分叉继承的父历史（前 N 条）不给界面 —— 那是
    /// 父会话的对话，用户正对着它；续接时也不重置。
    view_from: usize,
}

/// 续接一个子 agent 需要的东西。
#[derive(Debug)]
pub struct ResumeSource {
    pub kind: Kind,
    pub title: String,
    pub messages: Vec<Message>,
}

pub struct BackgroundTasks {
    inner: std::sync::Mutex<Vec<Entry>>,
    sink: SessionSink,
}

impl BackgroundTasks {
    pub fn new(sink: SessionSink) -> Self {
        Self {
            inner: std::sync::Mutex::new(Vec::new()),
            sink,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Entry>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 登记一个刚要开跑的子 agent。`initial` 是它起跑时的历史，
    /// `view_from` 是界面从第几条开始看（见 [`Entry::view_from`]）。
    ///
    /// 同一个 id 已经存在（续接）就复用那条：标题按新给的换（Cursor 的
    /// 规矩：续接到新任务要改名，续原任务就别改 —— 模型自己决定传什么），
    /// 状态回到运行中，历史换成续接起跑的那份（旧历史 + 新指令），
    /// `view_from` 不动。
    pub fn start(
        &self,
        view: BackgroundTaskView,
        kind: Kind,
        cancel: CancellationToken,
        initial: Vec<Message>,
        view_from: usize,
    ) {
        let mut g = self.lock();
        if let Some(e) = g.iter_mut().find(|e| e.view.id == view.id) {
            e.view = view;
            e.kind = kind;
            e.cancel = cancel;
            e.messages = Some(initial);
        } else {
            g.push(Entry {
                view,
                kind,
                cancel,
                messages: Some(initial),
                view_from,
            });
        }
        let snapshot = g.last().map(|e| e.view.clone());
        drop(g);
        if let Some(v) = snapshot {
            self.emit(v);
        }
    }

    /// 子 agent 有了新动静：调了个工具 / 说了句话。
    pub fn activity(&self, id: &AgentId, line: String, tool_uses: u32, tokens: u32) {
        let mut g = self.lock();
        let Some(e) = g.iter_mut().find(|e| &e.view.id == id) else {
            return;
        };
        e.view.activity = line;
        e.view.tool_uses = tool_uses;
        e.view.tokens = tokens;
        let v = e.view.clone();
        drop(g);
        self.emit(v);
    }

    /// 子 agent 产生了一条消息（assistant 回复、工具结果）。界面打开着
    /// 它的会话时靠这条追上进度。
    pub fn push_message(&self, id: &AgentId, message: Message) {
        let mut g = self.lock();
        let Some(e) = g.iter_mut().find(|e| &e.view.id == id) else {
            return;
        };
        if let Some(m) = &mut e.messages {
            m.push(message);
        }
    }

    /// 一个子 agent 的会话：视图 + 界面该看的那段消息。None = 不认识。
    pub fn history(&self, id: &str) -> Option<(BackgroundTaskView, Vec<Message>)> {
        let g = self.lock();
        let e = g.iter().find(|e| e.view.id.as_str() == id)?;
        let messages = e
            .messages
            .as_ref()
            .map(|m| m[e.view_from.min(m.len())..].to_vec())
            .unwrap_or_default();
        Some((e.view.clone(), messages))
    }

    /// 子 agent 结束。历史整份存下（续接用），并把太老的瘦身掉。
    pub fn finish(
        &self,
        id: &AgentId,
        status: BackgroundTaskStatus,
        messages: Vec<Message>,
        tool_uses: u32,
        tokens: u32,
        now_ms: u64,
    ) -> Option<BackgroundTaskView> {
        let mut g = self.lock();
        let e = g.iter_mut().find(|e| &e.view.id == id)?;
        e.view.status = status;
        e.view.finished_at_ms = Some(now_ms);
        e.view.tool_uses = tool_uses;
        e.view.tokens = tokens;
        e.view.activity = match status {
            BackgroundTaskStatus::Running => e.view.activity.clone(),
            BackgroundTaskStatus::Completed => "完成".into(),
            BackgroundTaskStatus::Failed => "失败".into(),
            BackgroundTaskStatus::Cancelled => "已停止".into(),
        };
        e.messages = Some(messages);
        let view = e.view.clone();

        // 瘦身：已结束且带历史的，从最老的开始丢历史，直到只剩 KEEP_FINISHED 份。
        let mut with_history: Vec<usize> = g
            .iter()
            .enumerate()
            .filter(|(_, e)| e.view.status.is_terminal() && e.messages.is_some())
            .map(|(i, _)| i)
            .collect();
        while with_history.len() > KEEP_FINISHED {
            let oldest = with_history.remove(0);
            g[oldest].messages = None;
        }
        drop(g);
        self.emit(view.clone());
        Some(view)
    }

    /// 面板上的停止键。返回是否真的停到了一个跑着的任务。
    pub fn cancel(&self, id: &AgentId) -> bool {
        let g = self.lock();
        match g
            .iter()
            .find(|e| &e.view.id == id && e.view.status == BackgroundTaskStatus::Running)
        {
            Some(e) => {
                e.cancel.cancel();
                true
            }
            None => false,
        }
    }

    /// 关会话 / 退应用：全部停掉。
    pub fn cancel_all(&self) {
        for e in self.lock().iter() {
            if e.view.status == BackgroundTaskStatus::Running {
                e.cancel.cancel();
            }
        }
    }

    pub fn running_count(&self) -> usize {
        self.lock()
            .iter()
            .filter(|e| e.view.background && e.view.status == BackgroundTaskStatus::Running)
            .count()
    }

    /// 给界面的快照：全部子 agent。面板按 `background` 过滤，卡片按
    /// `tool_use_id` 认领。
    pub fn snapshot(&self) -> Vec<BackgroundTaskView> {
        self.lock().iter().map(|e| e.view.clone()).collect()
    }

    /// 续接：拿到某个子 agent 的类型和历史。
    ///
    /// 错误信息是写给模型看的 —— 它要据此决定换个办法（新起一个）。
    pub fn resume_source(&self, id: &str) -> Result<ResumeSource, String> {
        let g = self.lock();
        let Some(e) = g.iter().find(|e| e.view.id.as_str() == id) else {
            return Err(format!(
                "没有叫「{id}」的子 agent。可续接的 id 只来自本会话里 Task 工具的返回；\
                 内核重启后旧的 id 也会失效。重新发起一个新任务即可。"
            ));
        };
        if e.view.status == BackgroundTaskStatus::Running {
            return Err(format!(
                "子 agent「{}」（{id}）还在跑，等它完成的通知到了再续接。",
                e.view.title
            ));
        }
        match &e.messages {
            Some(m) => Ok(ResumeSource {
                kind: e.kind,
                title: e.view.title.clone(),
                messages: m.clone(),
            }),
            None => Err(format!(
                "子 agent「{}」（{id}）太久以前的了，历史已经不在内存里，不能续接。\
                 重新发起一个新任务，把要点写进 prompt。",
                e.view.title
            )),
        }
    }

    fn emit(&self, view: BackgroundTaskView) {
        let _ = self.sink.send(AgentEvent::BackgroundTask {
            task: Box::new(view),
        });
    }
}

/// 子 agent 的完成通知：一条 user 消息，正文给模型，标记给界面。
///
/// `report` 是子 agent 的最后一条回复（同步 Task 里原样作为 tool_result
/// 回去的那份），失败时是失败原因。
pub fn notice_message(
    id: MessageId,
    view: &BackgroundTaskView,
    model: &str,
    report: &str,
    now_ms: u64,
) -> Message {
    let verb = match view.status {
        BackgroundTaskStatus::Running => "仍在运行",
        BackgroundTaskStatus::Completed => "已完成",
        BackgroundTaskStatus::Failed => "失败了",
        BackgroundTaskStatus::Cancelled => "被停止了",
    };
    let text = format!(
        "后台子任务「{}」{verb}（agent id：{} · {} · {model} · {} tokens · {} 次工具调用）。\n\
         下面是它的汇报。用户已经在界面上看到了这份汇报 —— 不要复述；只做需要你做的事：\
         综合多个任务的结果、处理它报告的阻塞或失败、或据此继续协调。没有需要做的就简短\
         确认一句。要给它追加指令，用 Task 工具、resume 填上面这个 agent id；回复里提到它\
         写成链接 [{}](agent:{})。\n\n\
         --- 汇报 ---\n{report}",
        view.title,
        view.id.as_str(),
        view.kind,
        view.tokens,
        view.tool_uses,
        view.title,
        view.id.as_str(),
    );
    Message::User {
        id,
        content: vec![UserContent::Attachment(Attachment::SystemReminder { text })],
        meta: MessageMeta {
            synthetic: true,
            created_at_ms: Some(now_ms),
            task_notice: Some(TaskNotice {
                agent_id: view.id.clone(),
                title: view.title.clone(),
                status: view.status,
            }),
            ..Default::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(id: &str, background: bool) -> BackgroundTaskView {
        BackgroundTaskView {
            id: AgentId::from_raw(id),
            title: id.to_owned(),
            kind: "explore".into(),
            model: "m".into(),
            background,
            tool_use_id: riot_protocol::id::ToolUseId::from_raw(format!("tu_{id}")),
            status: BackgroundTaskStatus::Running,
            activity: String::new(),
            tool_uses: 0,
            tokens: 0,
            started_at_ms: 0,
            finished_at_ms: None,
        }
    }

    fn msg(text: &str) -> Message {
        Message::User {
            id: MessageId::from_raw(text),
            content: vec![UserContent::Text { text: text.into() }],
            meta: Default::default(),
        }
    }

    fn start(t: &BackgroundTasks, id: &str, background: bool, cancel: CancellationToken) {
        t.start(
            view(id, background),
            Kind::Explore,
            cancel,
            vec![msg("q")],
            0,
        );
    }

    #[test]
    fn 跑着的不能续接_结束后能() {
        let t = BackgroundTasks::new(SessionSink::default());
        start(&t, "a", false, CancellationToken::new());
        assert!(t.resume_source("a").unwrap_err().contains("还在跑"));
        t.finish(
            &AgentId::from_raw("a"),
            BackgroundTaskStatus::Completed,
            vec![msg("q"), msg("r")],
            3,
            100,
            9,
        );
        let src = t.resume_source("a").expect("结束后可续");
        assert_eq!(src.kind, Kind::Explore);
        assert_eq!(src.messages.len(), 2);
        assert!(t.resume_source("nope").unwrap_err().contains("没有叫"));
    }

    #[test]
    fn 已结束的历史只留最近若干份() {
        let t = BackgroundTasks::new(SessionSink::default());
        for i in 0..(KEEP_FINISHED + 3) {
            let id = format!("t{i}");
            start(&t, &id, false, CancellationToken::new());
            t.finish(
                &AgentId::from_raw(&id),
                BackgroundTaskStatus::Completed,
                vec![msg("x")],
                0,
                0,
                0,
            );
        }
        assert!(
            t.resume_source("t0").unwrap_err().contains("太久"),
            "最老的该被瘦身"
        );
        assert!(t.resume_source("t2").unwrap_err().contains("太久"));
        assert!(t.resume_source("t3").is_ok(), "第 KEEP_FINISHED 新的还在");
        assert!(t.resume_source(&format!("t{}", KEEP_FINISHED + 2)).is_ok());
    }

    #[test]
    fn 快照含全部子agent_停止只停跑着的() {
        let t = BackgroundTasks::new(SessionSink::default());
        let c_bg = CancellationToken::new();
        start(&t, "bg", true, c_bg.clone());
        start(&t, "sync", false, CancellationToken::new());
        let snap = t.snapshot();
        assert_eq!(snap.len(), 2, "同步的也要在快照里，卡片要靖它直播");
        assert!(snap.iter().any(|v| v.id.as_str() == "bg" && v.background));
        assert!(
            snap.iter()
                .any(|v| v.id.as_str() == "sync" && !v.background)
        );
        assert_eq!(t.running_count(), 1, "只数后台的");

        assert!(t.cancel(&AgentId::from_raw("bg")));
        assert!(c_bg.is_cancelled());
        t.finish(
            &AgentId::from_raw("bg"),
            BackgroundTaskStatus::Cancelled,
            vec![],
            0,
            0,
            1,
        );
        assert!(!t.cancel(&AgentId::from_raw("bg")), "已结束的停不到");
        assert_eq!(t.running_count(), 0);
    }

    /// 跑着的子 agent 会话能看：起跑那条 + 边跑边追加的；分叉只看自己
    /// 产生的那段（view_from 之后），续接也不把父历史露出来。
    #[test]
    fn 会话边跑边可看_分叉跳过继承的父历史() {
        let t = BackgroundTasks::new(SessionSink::default());
        t.start(
            view("fork", true),
            Kind::Fork,
            CancellationToken::new(),
            vec![msg("父1"), msg("父2"), msg("分叉说明")],
            2,
        );
        t.push_message(&AgentId::from_raw("fork"), msg("干活1"));
        let (v, m) = t.history("fork").expect("认识它");
        assert_eq!(v.status, BackgroundTaskStatus::Running);
        let texts: Vec<String> = m.iter().map(|x| x.id().as_str().to_owned()).collect();
        assert_eq!(texts, ["分叉说明", "干活1"], "父历史不给界面");

        t.finish(
            &AgentId::from_raw("fork"),
            BackgroundTaskStatus::Completed,
            vec![
                msg("父1"),
                msg("父2"),
                msg("分叉说明"),
                msg("干活1"),
                msg("汇报"),
            ],
            1,
            10,
            9,
        );
        // 续接：起跑历史是完整的（含父历史），view_from 不变。
        t.start(
            view("fork", true),
            Kind::Fork,
            CancellationToken::new(),
            vec![
                msg("父1"),
                msg("父2"),
                msg("分叉说明"),
                msg("干活1"),
                msg("汇报"),
                msg("再来"),
            ],
            0,
        );
        let (_, m) = t.history("fork").unwrap();
        assert_eq!(m.len(), 4, "续接后界面照样从分叉点看起：{m:?}");
        assert!(t.history("nope").is_none());
    }

    /// 通知是一轮的起点（`is_user_prompt`），而且界面靠 meta 认出它。
    #[test]
    fn 通知消息带标记且算一轮起点() {
        let mut v = view("a", true);
        v.status = BackgroundTaskStatus::Completed;
        let m = notice_message(MessageId::from_raw("m"), &v, "test-model", "报告正文", 5);
        assert!(m.is_user_prompt());
        let Message::User { meta, content, .. } = &m else {
            unreachable!()
        };
        assert_eq!(
            meta.task_notice.as_ref().map(|n| n.agent_id.as_str()),
            Some("a")
        );
        assert!(matches!(
            &content[0],
            UserContent::Attachment(Attachment::SystemReminder { text })
                if text.contains("报告正文") && text.contains("不要复述")
        ));
    }
}
