//! 子 agent 登记表：会话里跑过的每个子 agent（同步的、后台的、分叉的）。
//!
//! # 为什么同步的也登记
//!
//! `resume=<agent id>` 要能续接任何一个跑过的子 agent —— 一次同步侦察
//! 回来的报告不够细，主 agent 说"再往下挖一层"，不该从头把背景讲一遍。
//! 续接需要它的全部历史，所以每条登记项留着子 agent 的消息（内存里；
//! 测试里没有持久化通道时也得能续）。
//!
//! # 内存上限
//!
//! 一个侦察子 agent 的历史动辄几十次 Grep/Read 的结果，几百 KB 起步。
//! 只保留最近 [`KEEP_FINISHED`] 个**已结束**的登记项的历史在内存里，更早
//! 的只留视图（状态、用量），要看、要续接时按需从它的 transcript 读回来
//! （见下）。跑着的永远不丢。
//!
//! # 跨越重启 ⭐
//!
//! 登记表跟着会话落盘（照 Cursor：重启之后子 agent 的会话照样能点开）：
//!
//! - **视图**（标题、类型、模型、哪次 Task 开的、父子关系、状态、用量）
//!   在每次登记 / 收场时整份写进 `subagents/<会话>/tasks.json`
//!   （[`riot_store::TaskIndex`]）。会话水合时读回来
//!   （[`BackgroundTasks::restore`]）—— 上个进程里还在跑的那些，现在肯定
//!   不在跑了，标成"已中断"。
//! - **消息**不进快照：每个子 agent 的 transcript 本来就在同目录的
//!   `<agent>.jsonl` 里（分叉只写它自己产生的那段）。恢复回来的登记项
//!   内存里没有历史，[`BackgroundTasks::history`] 和
//!   [`BackgroundTasks::resume_source`] 发现历史不在内存就读盘。
//! - 老会话（早于快照）没有 `tasks.json`，会话水合时从对话历史里的 Task
//!   调用把视图推回来（[`reconstruct_views`]），推完当场落盘。
//!
//! `[约束]` 分叉出来的子 agent 重启后**能看不能续**：它继承的父历史没写进
//! 它自己的 transcript（那是父会话的对话），盘上那截接不回一份完整的
//! 请求历史。
//!
//! # 界面看到的
//!
//! 每个子 agent 的状态变化都推 `BackgroundTask` 事件：后台任务面板只画
//! `background == true` 的；Task 工具卡片按 `tool_use_id` 认领自己的那个，
//! 直播"标题 · 模型 · 正在做什么"。点开任何一个都能看它的会话
//! （[`BackgroundTasks::history`]）—— 跑着的也能看，消息边产生边进登记表。

use std::sync::Arc;

use riot_protocol::event::AgentEvent;
use riot_protocol::id::{AgentId, MessageId, SessionId};
use riot_protocol::message::{Attachment, Message, MessageMeta, UserContent};
use riot_protocol::task::{BackgroundTaskStatus, BackgroundTaskView, TaskNotice};
use riot_protocol::text::UiText;
use riot_protocol::ui_text;
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
    /// 顶掉一次（内容相同，只是对齐 run_job 的口径）。`None` = 不在内存
    /// （太久被瘦身掉，或重启后恢复的），要用时从 transcript 读。
    messages: Option<Vec<Message>>,
    /// 界面从第几条开始看。分叉继承的父历史（前 N 条）不给界面 —— 那是
    /// 父会话的对话，用户正对着它；续接时也不重置。只对内存里的
    /// `messages` 有意义：盘上的 transcript 本来就不含继承的父历史。
    view_from: usize,
}

/// 落盘处：`subagents/<会话>/`。登记表快照和每个子 agent 的 transcript
/// 都在这里。
struct Persist {
    transcripts: Arc<riot_store::Transcripts>,
    index: riot_store::TaskIndex,
}

/// 一个子 agent 的会话（[`BackgroundTasks::history`] 的返回）。
pub struct TaskHistory {
    pub task: BackgroundTaskView,
    pub messages: Vec<Message>,
    pub descendants: Vec<BackgroundTaskView>,
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
    /// None = 不持久化（单元测试）：登记表只活在内存里，重启即空。
    persist: Option<Persist>,
}

impl BackgroundTasks {
    /// 只在内存里的登记表（测试用）。
    pub fn new(sink: SessionSink) -> Self {
        Self {
            inner: std::sync::Mutex::new(Vec::new()),
            sink,
            persist: None,
        }
    }

    /// 跟着会话落盘的登记表。`transcripts` 是这个会话的子 agent 目录
    /// （`Transcripts::subagents_of`）：子 agent 的 transcript 开在这里，
    /// 登记表快照也写在这里。
    pub fn persisted(sink: SessionSink, transcripts: Arc<riot_store::Transcripts>) -> Self {
        let index = transcripts.task_index();
        Self {
            inner: std::sync::Mutex::new(Vec::new()),
            sink,
            persist: Some(Persist { transcripts, index }),
        }
    }

    /// 子 agent transcript 的落盘处。None = 不持久化。子 agent 的日志由
    /// Task 工具开在这里 —— 和登记表读盘用同一个目录，路径规则只有一份。
    pub fn transcripts(&self) -> Option<Arc<riot_store::Transcripts>> {
        self.persist.as_ref().map(|p| Arc::clone(&p.transcripts))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Entry>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 把此刻的全部视图写成快照。登记 / 收场后各一次；活动行的变化不写
    /// （每次工具调用一次，太频繁，而且收场那次会带上最终的用量）。
    fn save_index(&self) {
        if let Some(p) = &self.persist {
            p.index.save(self.snapshot());
        }
    }

    /// 等快照落盘。退出钩子用。
    pub async fn flush(&self) {
        if let Some(p) = &self.persist {
            p.index.flush().await;
        }
    }

    /// 读回上个进程留下的登记表快照。
    pub async fn load_index(&self) -> Vec<BackgroundTaskView> {
        match &self.persist {
            Some(p) => p.index.load().await,
            None => Vec::new(),
        }
    }

    /// 盘上有 transcript 的子 agent（首行元数据 + 第一句任务描述）。
    /// 老会话重建登记表用。
    pub fn scan_transcripts(&self) -> Vec<riot_store::ScannedTranscript> {
        match &self.persist {
            Some(p) => p.transcripts.scan(),
            None => Vec::new(),
        }
    }

    /// 从 transcript 读一个子 agent 的历史。没有持久化通道 / 文件不在 = 空。
    async fn load_transcript(&self, id: &str) -> Vec<Message> {
        match &self.persist {
            Some(p) => {
                p.transcripts
                    .load(&SessionId::from_raw(id.to_owned()))
                    .await
                    .1
            }
            None => Vec::new(),
        }
    }

    /// 重启后把登记表装回来：来自快照，或老会话从对话历史推出来的
    /// （[`reconstruct_views`]）。已在表里的 id 跳过。
    ///
    /// 上个进程里还在跑的，现在肯定不在跑了：标成已中断 —— 留着"运行中"
    /// 会长出一个永远转圈、停不掉的幽灵。历史不进内存，看和续接时按需
    /// 读 transcript。
    ///
    /// `rebuilt` = 这批视图不是原样从快照读的（老会话推出来的），要落盘。
    /// 有任何一条被改成"已中断"也落盘 —— 下次启动不用再改一遍。
    pub fn restore(&self, views: Vec<BackgroundTaskView>, rebuilt: bool) {
        let mut dirty = rebuilt;
        let mut g = self.lock();
        for mut v in views {
            if g.iter().any(|e| e.view.id == v.id) {
                continue;
            }
            if v.status == BackgroundTaskStatus::Running {
                v.status = BackgroundTaskStatus::Cancelled;
                v.activity = ui_text!("kernel.task.activity.interrupted");
                dirty = true;
            }
            let kind = Kind::from_label(&v.kind).unwrap_or(Kind::GeneralPurpose);
            g.push(Entry {
                view: v,
                kind,
                cancel: CancellationToken::new(),
                messages: None,
                view_from: 0,
            });
        }
        drop(g);
        if dirty {
            self.save_index();
        }
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
        let snapshot = view.clone();
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
        drop(g);
        self.emit(snapshot);
        self.save_index();
    }

    /// 子 agent 有了新动静：调了个工具 / 说了句话。
    pub fn activity(&self, id: &AgentId, line: UiText, tool_uses: u32, tokens: u32) {
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

    /// 一个子 agent 的会话：视图 + 界面该看的那段消息 + 它派出去的全部
    /// 子 agent（含更深层，按登记顺序）。None = 不认识。
    ///
    /// 历史不在内存（重启后恢复的、太久被瘦身掉的）就读它的 transcript。
    /// 盘上那份本来就只有它自己产生的部分（分叉继承的父历史不写进去），
    /// 不用再按 `view_from` 裁。
    pub async fn history(&self, id: &str) -> Option<TaskHistory> {
        let (task, in_memory, descendants) = {
            let g = self.lock();
            let e = g.iter().find(|e| e.view.id.as_str() == id)?;
            let in_memory = e
                .messages
                .as_ref()
                .map(|m| m[e.view_from.min(m.len())..].to_vec());
            let descendants = g
                .iter()
                .filter(|x| is_descendant_of(&g, &x.view, id))
                .map(|x| x.view.clone())
                .collect();
            (e.view.clone(), in_memory, descendants)
        };
        let messages = match in_memory {
            Some(m) => m,
            None => self.load_transcript(id).await,
        };
        Some(TaskHistory {
            task,
            messages,
            descendants,
        })
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
        e.view.activity = status_activity(status).unwrap_or_else(|| e.view.activity.clone());
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
        self.save_index();
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
    /// 历史不在内存（重启后恢复的、太久被瘦身掉的）就读它的 transcript ——
    /// 除了分叉：它继承的父历史没写进自己的 transcript，盘上那截接不回
    /// 一份完整的请求历史（见模块文档「跨越重启」）。
    ///
    /// 错误信息是写给模型看的 —— 它要据此决定换个办法（新起一个）。
    pub async fn resume_source(&self, id: &str) -> Result<ResumeSource, String> {
        let (kind, title, in_memory) = {
            let g = self.lock();
            let Some(e) = g.iter().find(|e| e.view.id.as_str() == id) else {
                return Err(format!(
                    "没有叫「{id}」的子 agent。可续接的 id 只来自本会话里 Task 工具的返回。\
                     重新发起一个新任务即可。"
                ));
            };
            if e.view.status == BackgroundTaskStatus::Running {
                return Err(format!(
                    "子 agent「{}」（{id}）还在跑，等它完成的通知到了再续接。",
                    e.view.title
                ));
            }
            (e.kind, e.view.title.clone(), e.messages.clone())
        };
        if let Some(messages) = in_memory {
            return Ok(ResumeSource {
                kind,
                title,
                messages,
            });
        }
        if kind == Kind::Fork {
            return Err(format!(
                "子 agent「{title}」（{id}）是分叉出来的，它继承的上下文在内核重启后已经不在了，\
                 不能续接。重新发起一个新任务，把要点写进 prompt。"
            ));
        }
        let messages = self.load_transcript(id).await;
        if messages.is_empty() {
            return Err(format!(
                "子 agent「{title}」（{id}）太久以前的了，历史已经不在内存里，磁盘上也没有\
                 它的记录，不能续接。重新发起一个新任务，把要点写进 prompt。"
            ));
        }
        Ok(ResumeSource {
            kind,
            title,
            messages,
        })
    }

    fn emit(&self, view: BackgroundTaskView) {
        let _ = self.sink.send(AgentEvent::BackgroundTask {
            task: Box::new(view),
        });
    }
}

/// 沿 parent 往上走，落在 `ancestor` 上的都算它的后代。层数有限（深度
/// 计数器封顶），每条线性扫一遍就够。
fn is_descendant_of<'a>(all: &'a [Entry], mut v: &'a BackgroundTaskView, ancestor: &str) -> bool {
    while let Some(p) = &v.parent {
        if p.as_str() == ancestor {
            return true;
        }
        match all.iter().find(|x| &x.view.id == p) {
            Some(px) => v = &px.view,
            None => return false,
        }
    }
    false
}

/// 收场状态对应的活动行。None = 还在跑，活动行保持原样。
fn status_activity(status: BackgroundTaskStatus) -> Option<UiText> {
    match status {
        BackgroundTaskStatus::Running => None,
        BackgroundTaskStatus::Completed => Some(ui_text!("kernel.task.activity.completed")),
        BackgroundTaskStatus::Failed => Some(ui_text!("kernel.task.activity.failed")),
        BackgroundTaskStatus::Cancelled => Some(ui_text!("kernel.task.activity.cancelled")),
    }
}

/// 老会话没有登记表快照时，从对话历史里把子 agent 的视图推回来。
///
/// 依据是历史里已有的三样东西：Task 的 tool_use（标题、类型、是否后台、
/// 哪次调用开的）、它的 tool_result（正文里带 agent id —— 同步的在脚注里，
/// 后台的在"已启动"那句里）、后台任务的完成通知（`task_notice`，给状态）。
/// 同一个 id 出现多次（续接）以最后一次为准 —— 和活着的登记表一样，最新
/// 那次调用认领它。
///
/// 只认 `on_disk`（`subagents/<会话>/` 下扫到的 transcript）里有记录的：
/// 没记录的点开也是空的，不如不列。盘上有、历史里找不到的（那一轮被压缩
/// 或删掉了；子 agent 再派的子 agent 也在这里 —— 派它的调用在父 agent 的
/// transcript 里，不在会话历史里）退回最简视图：标题取任务描述的第一句，
/// 类型按 general-purpose，状态按完成。模型名、用量历史里没有，留空。
///
/// 后台任务的状态要等通知：tool_result 只说"已启动"。通知没等到（那一轮
/// 被中断）就留在运行中，交给 [`BackgroundTasks::restore`] 标成已中断。
pub fn reconstruct_views(
    history: &[Message],
    on_disk: &[riot_store::ScannedTranscript],
) -> Vec<BackgroundTaskView> {
    use riot_protocol::message::{AssistantContent, ToolResultContent};
    use std::collections::HashMap;

    // tool_use id → Task 的入参。
    let mut calls: HashMap<&riot_protocol::id::ToolUseId, &serde_json::Value> = HashMap::new();
    let mut views: Vec<BackgroundTaskView> = Vec::new();

    for m in history {
        match m {
            Message::Assistant { content, .. } => {
                for c in content {
                    if let AssistantContent::ToolUse { id, name, input } = c
                        && name == "Task"
                    {
                        calls.insert(id, input);
                    }
                }
            }
            Message::User { content, meta, .. } => {
                if let Some(n) = &meta.task_notice {
                    if let Some(v) = views.iter_mut().find(|v| v.id == n.agent_id) {
                        v.status = n.status;
                        v.finished_at_ms = meta.created_at_ms;
                        if let Some(a) = status_activity(n.status) {
                            v.activity = a;
                        }
                    }
                    continue;
                }
                for c in content {
                    let UserContent::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } = c
                    else {
                        continue;
                    };
                    let Some(input) = calls.get(tool_use_id) else {
                        continue;
                    };
                    // 出错的调用没开出任何子 agent（参数不对、续接的还在跑、
                    // 同步跑失败时结果只有失败原因、没有脚注）—— 活着的登记表
                    // 也不会为它登记。同步跑失败的那个 transcript 在盘上有，
                    // 由下面"盘上有、历史里没有"那条路补最简视图。
                    if *is_error {
                        continue;
                    }
                    let text = match content {
                        ToolResultContent::Text { text } => text.as_str(),
                        ToolResultContent::Spilled { preview, .. } => preview.as_str(),
                        _ => continue,
                    };
                    let Some(agent_id) = find_agent_id(text) else {
                        continue;
                    };
                    let str_of = |k: &str| input.get(k).and_then(|v| v.as_str()).unwrap_or("");
                    let resume = str_of("resume").trim();
                    let fork = resume == "self";
                    let background = fork
                        || input
                            .get("run_in_background")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                    // 后台的 tool_result 只说"已启动"，状态等通知；同步的
                    // 有脚注就是跑完了。
                    let status = if background {
                        BackgroundTaskStatus::Running
                    } else {
                        BackgroundTaskStatus::Completed
                    };
                    let title = str_of("description").trim().to_owned();
                    match views.iter_mut().find(|v| v.id.as_str() == agent_id) {
                        // 续接：最新那次调用认领它；类型沿用原来的。
                        Some(v) => {
                            if !title.is_empty() {
                                v.title = title;
                            }
                            v.tool_use_id = tool_use_id.clone();
                            v.background = background;
                            v.status = status;
                            v.finished_at_ms =
                                (!background).then_some(meta.created_at_ms).flatten();
                            v.activity = status_activity(status)
                                .unwrap_or_else(|| ui_text!("kernel.task.activity.started"));
                        }
                        None => {
                            let kind = if fork {
                                Kind::Fork
                            } else {
                                Kind::from_label(str_of("subagent_type"))
                                    .unwrap_or(Kind::GeneralPurpose)
                            };
                            views.push(BackgroundTaskView {
                                id: AgentId::from_raw(agent_id),
                                title,
                                kind: kind.as_str().to_owned(),
                                model: String::new(),
                                background,
                                tool_use_id: tool_use_id.clone(),
                                parent: None,
                                status,
                                activity: status_activity(status)
                                    .unwrap_or_else(|| ui_text!("kernel.task.activity.started")),
                                tool_uses: 0,
                                tokens: 0,
                                started_at_ms: 0,
                                finished_at_ms: (!background)
                                    .then_some(meta.created_at_ms)
                                    .flatten(),
                            });
                        }
                    }
                }
            }
            Message::System { .. } => {}
        }
    }

    // 只留盘上有记录的，起跑时刻取 transcript 首行的；盘上有而历史里没有
    // 的补最简视图。
    let mut out: Vec<BackgroundTaskView> = Vec::new();
    for v in views {
        let Some(s) = on_disk.iter().find(|s| s.meta.id.as_str() == v.id.as_str()) else {
            continue;
        };
        let mut v = v;
        v.started_at_ms = s.meta.created_at_ms;
        if v.title.is_empty() {
            v.title = fallback_title(s);
        }
        out.push(v);
    }
    for s in on_disk {
        if out.iter().any(|v| v.id.as_str() == s.meta.id.as_str()) {
            continue;
        }
        out.push(BackgroundTaskView {
            id: AgentId::from_raw(s.meta.id.as_str()),
            title: fallback_title(s),
            kind: Kind::GeneralPurpose.as_str().to_owned(),
            model: String::new(),
            background: false,
            tool_use_id: riot_protocol::id::ToolUseId::default(),
            parent: None,
            status: BackgroundTaskStatus::Completed,
            activity: ui_text!("kernel.task.activity.completed"),
            tool_uses: 0,
            tokens: 0,
            started_at_ms: s.meta.created_at_ms,
            finished_at_ms: None,
        });
    }
    out.sort_by_key(|v| v.started_at_ms);
    out
}

/// 历史里找不到标题时的替代：transcript 里第一句任务描述的前 40 个字
/// （分叉的第一条是"Task: …"，去掉前缀），再不行就是 id。
fn fallback_title(s: &riot_store::ScannedTranscript) -> String {
    s.first_prompt
        .as_deref()
        .map(|p| p.trim_start_matches("Task:").trim_start_matches("任务："))
        .and_then(crate::session::title_excerpt)
        .unwrap_or_else(|| s.meta.id.as_str().to_owned())
}

/// 从 Task 的成功结果文本里捞它自己的 agent id。
///
/// 只认两处锚点 —— 同步结果末尾的脚注 `[子任务 agt_…：…]`、后台结果开头的
/// `agent id：agt_…`（都是 `subagent::TaskTool::call` 拼的字面量，从有这个
/// 功能起没变过）。不退回"第一个 `agt_`"：汇报正文里可能提到**别的**子
/// agent（它自己派的那些，写成 `agent:` 链接），错误结果里也常带别人的 id
/// （"「x」（agt_…）还在跑"），抓到就是把一次没开成的调用记到别人头上。
fn find_agent_id(text: &str) -> Option<&str> {
    if let Some(i) = text.rfind("[子任务 ")
        && let Some(id) = agent_id_at(&text[i + "[子任务 ".len()..])
    {
        return Some(id);
    }
    if let Some(i) = text.find("agent id：")
        && let Some(id) = agent_id_at(&text[i + "agent id：".len()..])
    {
        return Some(id);
    }
    None
}

/// `s` 以 `agt_` 开头时取出整个 id（`agt_` + nanoid 字符集，和前端
/// `agentIdFromResult` 同一条规则）。
fn agent_id_at(s: &str) -> Option<&str> {
    if !s.starts_with("agt_") {
        return None;
    }
    let len = s
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '_' || *c == '-'))
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    let id = &s[..len];
    (id.len() > "agt_".len() + 5).then_some(id)
}

/// 子 agent 的完成通知：一条 user 消息，正文给模型，标记给界面。
///
/// `report` 是子 agent 的最后一条回复（同步 Task 里原样作为 tool_result
/// 回去的那份），失败时是失败原因。
///
/// 前奏是给模型的行为说明（界面按 `--- 汇报 ---` 切掉后只显示汇报本体，
/// 见 `useSession.ts` 的 `stripNoticePreamble`），所以用英文和其余提示词
/// 保持一致；分隔符本身是前后端约定的字面量，改它会让卡片显示整段前奏。
pub fn notice_message(
    id: MessageId,
    view: &BackgroundTaskView,
    model: &str,
    report: &str,
    now_ms: u64,
) -> Message {
    let verb = match view.status {
        BackgroundTaskStatus::Running => "is still running",
        BackgroundTaskStatus::Completed => "has finished",
        BackgroundTaskStatus::Failed => "failed",
        BackgroundTaskStatus::Cancelled => "was stopped",
    };
    let text = format!(
        "The background subtask \"{}\" {verb} (agent id: {} · {} · {model} · {} tokens · {} tool \
         calls).\n\
         Its report follows. The user has ALREADY seen this report in the UI, so do NOT recite \
         it back — repeating it just makes them read the same thing twice. Do only what actually \
         needs doing: combine the results of several tasks, deal with a blocker or failure it \
         reports, or carry on coordinating from here. If nothing needs doing, acknowledge it in \
         one short sentence. To send it further instructions, use the Task tool with resume set \
         to the agent id above; when you mention it in a reply, write it as the link \
         [{}](agent:{}).\n\n\
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
            parent: None,
            status: BackgroundTaskStatus::Running,
            activity: ui_text!("kernel.task.activity.started"),
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

    #[tokio::test]
    async fn 跑着的不能续接_结束后能() {
        let t = BackgroundTasks::new(SessionSink::default());
        start(&t, "a", false, CancellationToken::new());
        assert!(t.resume_source("a").await.unwrap_err().contains("还在跑"));
        t.finish(
            &AgentId::from_raw("a"),
            BackgroundTaskStatus::Completed,
            vec![msg("q"), msg("r")],
            3,
            100,
            9,
        );
        let src = t.resume_source("a").await.expect("结束后可续");
        assert_eq!(src.kind, Kind::Explore);
        assert_eq!(src.messages.len(), 2);
        assert!(
            t.resume_source("nope")
                .await
                .unwrap_err()
                .contains("没有叫")
        );
    }

    #[tokio::test]
    async fn 已结束的历史只留最近若干份() {
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
            t.resume_source("t0").await.unwrap_err().contains("太久"),
            "最老的该被瘦身（没有持久化通道，盘上也没有）"
        );
        assert!(t.resume_source("t2").await.unwrap_err().contains("太久"));
        assert!(
            t.resume_source("t3").await.is_ok(),
            "第 KEEP_FINISHED 新的还在"
        );
        assert!(
            t.resume_source(&format!("t{}", KEEP_FINISHED + 2))
                .await
                .is_ok()
        );
    }

    /// 持久化的登记表：瘦身掉的历史从 transcript 读回来，看和续接都不受
    /// 影响 —— 内存上限只是内存上限，不是"太久就没了"。
    #[tokio::test]
    async fn 瘦身掉的历史从transcript读回() {
        let d = tempfile::tempdir().expect("临时目录");
        let store = Arc::new(riot_store::Transcripts::new(d.path()));
        let t = BackgroundTasks::persisted(SessionSink::default(), Arc::clone(&store));
        for i in 0..(KEEP_FINISHED + 1) {
            let id = format!("t{i}");
            // transcript 由 Task 工具写；这里替它写一份。
            let log = store.open(riot_store::TranscriptMeta {
                id: SessionId::from_raw(&id),
                root: d.path().to_path_buf(),
                created_at_ms: i as u64,
            });
            log.append(&msg(&format!("q{i}")));
            log.append(&msg(&format!("r{i}")));
            log.flush().await;
            start(&t, &id, false, CancellationToken::new());
            t.finish(
                &AgentId::from_raw(&id),
                BackgroundTaskStatus::Completed,
                vec![msg(&format!("q{i}")), msg(&format!("r{i}"))],
                0,
                0,
                0,
            );
        }
        let h = t.history("t0").await.expect("认识它");
        assert_eq!(h.messages.len(), 2, "内存里没了就读盘");
        let src = t.resume_source("t0").await.expect("读盘后可续");
        assert_eq!(src.messages[1].id().as_str(), "r0");
    }

    /// 重启：快照读回来 → 登记表照旧；上个进程里还在跑的标成已中断；
    /// 历史按需读盘；分叉能看不能续。
    #[tokio::test]
    async fn 重启后从快照恢复_跑着的标中断_历史读盘() {
        let d = tempfile::tempdir().expect("临时目录");
        let store = Arc::new(riot_store::Transcripts::new(d.path()));

        // 上一个进程：两个跑完的（一个分叉）、一个还在跑。
        {
            let t = BackgroundTasks::persisted(SessionSink::default(), Arc::clone(&store));
            for id in ["done", "fork", "live"] {
                let log = store.open(riot_store::TranscriptMeta {
                    id: SessionId::from_raw(id),
                    root: d.path().to_path_buf(),
                    created_at_ms: 1,
                });
                log.append(&msg(&format!("{id}-q")));
                log.append(&msg(&format!("{id}-r")));
                log.flush().await;
            }
            start(&t, "done", false, CancellationToken::new());
            t.finish(
                &AgentId::from_raw("done"),
                BackgroundTaskStatus::Completed,
                vec![msg("done-q"), msg("done-r")],
                2,
                50,
                9,
            );
            // 视图上的类型标签由 Task 工具按 Kind 填（快照靠它还原类型）。
            let mut fork_view = view("fork", true);
            fork_view.kind = Kind::Fork.as_str().into();
            t.start(
                fork_view,
                Kind::Fork,
                CancellationToken::new(),
                vec![msg("父1"), msg("fork-q")],
                1,
            );
            t.finish(
                &AgentId::from_raw("fork"),
                BackgroundTaskStatus::Completed,
                vec![msg("父1"), msg("fork-q"), msg("fork-r")],
                1,
                10,
                9,
            );
            start(&t, "live", true, CancellationToken::new());
            t.flush().await;
        }

        // 新进程：空表 + 读快照。
        let t = BackgroundTasks::persisted(SessionSink::default(), Arc::clone(&store));
        let views = t.load_index().await;
        assert_eq!(views.len(), 3, "三个都在快照里");
        t.restore(views, false);

        let snap = t.snapshot();
        let by = |id: &str| snap.iter().find(|v| v.id.as_str() == id).expect(id).clone();
        assert_eq!(by("done").status, BackgroundTaskStatus::Completed);
        assert_eq!(by("done").tool_uses, 2, "用量跟着快照回来");
        assert_eq!(
            by("live").status,
            BackgroundTaskStatus::Cancelled,
            "上个进程里跑着的现在肯定不在跑"
        );
        assert_eq!(by("live").activity.key, "kernel.task.activity.interrupted");
        assert!(!t.cancel(&AgentId::from_raw("live")), "中断的停不到");

        let h = t.history("done").await.expect("认识它");
        assert_eq!(h.task.title, "done");
        let ids: Vec<&str> = h.messages.iter().map(|m| m.id().as_str()).collect();
        assert_eq!(ids, ["done-q", "done-r"], "历史从 transcript 读");
        let h = t.history("fork").await.expect("分叉也能看");
        let ids: Vec<&str> = h.messages.iter().map(|m| m.id().as_str()).collect();
        assert_eq!(
            ids,
            ["fork-q", "fork-r"],
            "盘上本来就只有它自己那段，不再按 view_from 裁"
        );

        let src = t.resume_source("done").await.expect("重启后照样能续");
        assert_eq!(src.kind, Kind::Explore);
        assert_eq!(src.messages.len(), 2);
        assert!(
            t.resume_source("fork").await.unwrap_err().contains("分叉"),
            "分叉继承的父历史不在盘上，不能续"
        );

        // 改成"已中断"要落盘：下次启动直接是终态。
        t.flush().await;
        let again = store.task_index().load().await;
        assert_eq!(
            again
                .iter()
                .find(|v| v.id.as_str() == "live")
                .unwrap()
                .status,
            BackgroundTaskStatus::Cancelled
        );
    }

    /// 老会话没有快照：从对话历史推。同步的看 tool_result，后台的看通知，
    /// 续接以最后一次调用为准，盘上没记录的不列，盘上有历史里没有的补最简视图。
    #[tokio::test]
    async fn 老会话从历史重建登记表() {
        use riot_protocol::id::ToolUseId;
        use riot_protocol::message::{AssistantContent, ToolResultContent};

        let d = tempfile::tempdir().expect("临时目录");
        let store = Arc::new(riot_store::Transcripts::new(d.path()));
        for (id, at) in [
            ("agt_sync000001", 10),
            ("agt_bg00000002", 20),
            ("agt_orphan0003", 30),
        ] {
            let log = store.open(riot_store::TranscriptMeta {
                id: SessionId::from_raw(id),
                root: d.path().to_path_buf(),
                created_at_ms: at,
            });
            log.append(&msg(&format!(
                "Task: 给 {id} 的任务描述，很长很长很长很长很长很长很长很长很长"
            )));
            log.flush().await;
        }

        let call = |tu: &str, input: serde_json::Value| Message::Assistant {
            id: MessageId::from_raw(format!("a_{tu}")),
            content: vec![AssistantContent::ToolUse {
                id: ToolUseId::from_raw(tu),
                name: "Task".into(),
                input,
            }],
            usage: None,
            meta: Default::default(),
        };
        let result = |tu: &str, text: &str, is_error: bool, at: u64| Message::User {
            id: MessageId::from_raw(format!("u_{tu}")),
            content: vec![UserContent::ToolResult {
                tool_use_id: ToolUseId::from_raw(tu),
                content: ToolResultContent::text(text),
                is_error,
            }],
            meta: MessageMeta {
                created_at_ms: Some(at),
                ..Default::default()
            },
        };
        let mut bg_done = view("agt_bg00000002", true);
        bg_done.status = BackgroundTaskStatus::Completed;
        let history = vec![
            call(
                "tu1",
                serde_json::json!({ "description": "找入口", "prompt": "p", "subagent_type": "explore" }),
            ),
            // 汇报里提到了它自己派的子 agent —— 不能抓错。
            result(
                "tu1",
                "见 [子侦察](agent:agt_child0000009)。\n\n[子任务 agt_sync000001：m · 10 tokens · 2 次工具调用 · 可用 resume=\"agt_sync000001\" 续接]",
                false,
                11,
            ),
            call(
                "tu2",
                serde_json::json!({ "description": "跑测试", "prompt": "p", "run_in_background": true }),
            ),
            result(
                "tu2",
                "后台子任务已启动。agent id：agt_bg00000002（general-purpose·m），标题「跑测试」。",
                false,
                21,
            ),
            // 后台那个还在跑时就想续接：调用出错，什么都没开出来 ——
            // 不能把后台那条改名、改成这次调用的。
            call(
                "tu2b",
                serde_json::json!({ "description": "抢跑续接", "prompt": "p", "resume": "agt_bg00000002", "run_in_background": true }),
            ),
            result(
                "tu2b",
                "子 agent「跑测试」（agt_bg00000002）还在跑，等它完成的通知到了再续接。",
                true,
                23,
            ),
            notice_message(MessageId::from_raw("n1"), &bg_done, "m", "跑完了", 25),
            // 续接同步那个，换了个名字。
            call(
                "tu3",
                serde_json::json!({ "description": "再挖一层", "prompt": "p", "resume": "agt_sync000001" }),
            ),
            result(
                "tu3",
                "更多细节\n\n[子任务 agt_sync000001：m · 20 tokens · 3 次工具调用]",
                false,
                31,
            ),
            // 盘上没有记录的：不列。
            call(
                "tu4",
                serde_json::json!({ "description": "幽灵", "prompt": "p" }),
            ),
            result(
                "tu4",
                "[子任务 agt_ghost0000004：m · 0 tokens · 0 次工具调用]",
                false,
                41,
            ),
        ];

        let views = reconstruct_views(&history, &store.scan());
        let by = |id: &str| {
            views
                .iter()
                .find(|v| v.id.as_str() == id)
                .expect(id)
                .clone()
        };
        assert_eq!(views.len(), 3, "{views:#?}");

        let sync = by("agt_sync000001");
        assert_eq!(sync.title, "再挖一层", "续接以最后一次为准");
        assert_eq!(sync.kind, "explore", "类型沿用原来的");
        assert_eq!(sync.tool_use_id.as_str(), "tu3", "最新那次调用认领它");
        assert_eq!(sync.status, BackgroundTaskStatus::Completed);
        assert_eq!(sync.started_at_ms, 10, "起跑时刻取 transcript 首行");
        assert_eq!(sync.finished_at_ms, Some(31));
        assert!(!sync.background);

        let bg = by("agt_bg00000002");
        assert!(bg.background);
        assert_eq!(bg.title, "跑测试", "出错的续接调用不能改名");
        assert_eq!(bg.tool_use_id.as_str(), "tu2", "也不能抢认领");
        assert_eq!(bg.status, BackgroundTaskStatus::Completed, "状态来自通知");
        assert_eq!(bg.finished_at_ms, Some(25));

        let orphan = by("agt_orphan0003");
        assert_eq!(orphan.tool_use_id.as_str(), "", "没人认领");
        assert!(
            orphan.title.starts_with("给 agt_orphan0003 的任务描述"),
            "标题退回第一句任务描述（去掉 Task: 前缀）：{}",
            orphan.title
        );
        assert_eq!(orphan.status, BackgroundTaskStatus::Completed);

        assert!(
            views.iter().all(|v| v.id.as_str() != "agt_ghost0000004"),
            "盘上没记录的不列"
        );
        assert!(
            views.iter().all(|v| v.id.as_str() != "agt_child0000009"),
            "汇报里提到的别的子 agent 不是它自己"
        );
    }

    #[test]
    fn 从结果文本里捞agent_id() {
        assert_eq!(
            find_agent_id("正文\n[子任务 agt_abcdef123456：m · 1 tokens · 0 次工具调用]"),
            Some("agt_abcdef123456")
        );
        assert_eq!(
            find_agent_id("后台子任务已启动。agent id：agt_abcdef123456（explore·m）"),
            Some("agt_abcdef123456")
        );
        assert_eq!(
            find_agent_id("见 [x](agent:agt_child0000001)。\n[子任务 agt_self00000002：m]"),
            Some("agt_self00000002"),
            "脚注优先于正文里提到的别人"
        );
        assert_eq!(
            find_agent_id(
                "子 agent「提取端点」（agt_loose0000003）还在跑，等它完成的通知到了再续接。"
            ),
            None,
            "没有锚点的不认：错误结果里的 id 是别人的"
        );
        assert_eq!(find_agent_id("[子任务 agt_x：太短]"), None);
        assert_eq!(find_agent_id("没有 id"), None);
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
    #[tokio::test]
    async fn 会话边跑边可看_分叉跳过继承的父历史() {
        let t = BackgroundTasks::new(SessionSink::default());
        t.start(
            view("fork", true),
            Kind::Fork,
            CancellationToken::new(),
            vec![msg("父1"), msg("父2"), msg("分叉说明")],
            2,
        );
        t.push_message(&AgentId::from_raw("fork"), msg("干活1"));
        let h = t.history("fork").await.expect("认识它");
        assert_eq!(h.task.status, BackgroundTaskStatus::Running);
        let texts: Vec<String> = h
            .messages
            .iter()
            .map(|x| x.id().as_str().to_owned())
            .collect();
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
        let h = t.history("fork").await.unwrap();
        assert_eq!(
            h.messages.len(),
            4,
            "续接后界面照样从分叉点看起：{:?}",
            h.messages
        );
        assert!(t.history("nope").await.is_none());
    }

    /// 嵌套：子 agent 派的子 agent 记 parent；查父的会话时后代（含更深层）一并回来。
    #[tokio::test]
    async fn 后代按parent链找齐() {
        let t = BackgroundTasks::new(SessionSink::default());
        start(&t, "a", true, CancellationToken::new());
        let mut b = view("b", false);
        b.parent = Some(AgentId::from_raw("a"));
        t.start(b, Kind::Explore, CancellationToken::new(), vec![], 0);
        let mut c = view("c", false);
        c.parent = Some(AgentId::from_raw("b"));
        t.start(c, Kind::Explore, CancellationToken::new(), vec![], 0);
        start(&t, "other", false, CancellationToken::new());

        let h = t.history("a").await.unwrap();
        let ids: Vec<&str> = h.descendants.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, ["b", "c"], "孙子也算，无关的不算");
        assert!(t.history("c").await.unwrap().descendants.is_empty());
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
                if text.contains("报告正文") && text.contains("do NOT recite")
        ));
    }
}
