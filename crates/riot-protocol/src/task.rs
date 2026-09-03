//! 后台子 agent（Task 工具 `run_in_background`）的对外形状。
//!
//! 子 agent 有两种跑法：同步 —— 父的那次工具调用一直等到它跑完，结果
//! 作为 tool_result 回去；后台 —— 工具调用立刻返回一个 agent id，子 agent
//! 在会话上继续跑，**完成时以一条用户侧消息通知父 agent**（见
//! [`TaskNotice`]）。后者是"主 agent 只协调、重活移出前台"这套编排
//! 范式的根基：主 agent 委派完就结束回合，被通知唤醒，而不是空转等待。
//!
//! 这里只放三方（内核 / 宿主 / 前端）都要看的类型：任务的界面视图、
//! 状态枚举、通知消息上的标记。执行本身在内核。

use crate::id::AgentId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// 后台任务的生命周期。
///
/// 没有"排队"态：后台子 agent 一登记就开跑 —— 并发数由模型自己克制
/// （Task 工具的提示词里讲了），不做队列。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskStatus {
    Running,
    /// 正常说完了（含到达步数上限被停下 —— 那种情况汇报里会注明）。
    Completed,
    /// 内部错误：provider 挂了、装配失败。汇报是错误原因。
    Failed,
    /// 被用户（面板上的停止键）或会话关闭取消。
    Cancelled,
}

impl BackgroundTaskStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

/// 一个子 agent 在界面上的样子（同步的、后台的都有一份）。
///
/// 随 [`crate::event::AgentEvent::BackgroundTask`] 每次变化推一份全量；
/// 切回会话时随 `session.resume` 快照整批回来。不进 transcript ——
/// 它描述的是一个活状态，落盘重放会长出永远"运行中"的幽灵；"跑过、
/// 结果是什么"由通知消息（[`TaskNotice`]）和 Task 的 tool_result 记在
/// 历史里。
///
/// 名字里的 Background 是历史包袱：最初只给后台任务用，后来同步子 agent
/// 也要在 Task 卡片上直播"标题 · 模型 · 正在做什么"，于是全都登记，
/// 靠 `background` 区分该不该进后台任务面板。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BackgroundTaskView {
    pub id: AgentId,
    /// 模型给的任务名（Task 工具的 `description`）。续接时可以换。
    pub title: String,
    /// `explore` / `general-purpose` / `fork`。
    pub kind: String,
    /// 它实际用的模型名（便宜档生效时和主模型不同）。
    #[serde(default)]
    pub model: String,
    /// 后台跑的（进面板、完成时发通知）还是同步跑的（只在 Task 卡片上）。
    #[serde(default)]
    pub background: bool,
    /// 把它开出来的那次 Task 调用。卡片靠它认领自己的子 agent。
    #[serde(default)]
    pub tool_use_id: crate::id::ToolUseId,
    pub status: BackgroundTaskStatus,
    /// 最近一行活动（正在调哪个工具、刚说的第一句话）。面板上滚动显示。
    pub activity: String,
    pub tool_uses: u32,
    pub tokens: u32,
    pub started_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_ms: Option<u64>,
}

/// 打在完成通知消息上的标记（`MessageMeta::task_notice`）。
///
/// 通知本体是一条 user 消息，正文放在 `SystemReminder` 附件里给模型读；
/// 这份标记给界面 —— 靠它把那条消息画成"后台任务完成"卡片而不是
/// 用户气泡。meta 不进 wire 格式，模型只看得到附件里的文字。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskNotice {
    pub agent_id: AgentId,
    pub title: String,
    pub status: BackgroundTaskStatus,
}
