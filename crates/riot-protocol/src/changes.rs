//! 变更集的传输形状。
//!
//! 两个面板共用这一套形状:输入框上方的"本次会话改动"(session.changes,
//! 基线来自文件状态缓存)和侧边抽屉的"Git 改动"(session.git_changes,
//! 工作区相对 HEAD 的未提交差异)。算法都在内核侧;这里只有要跨进程
//! 走 RPC 的形状定义。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    Created,
    Modified,
    Deleted,
    /// 仅 git 视图会产生:git 认出的重命名(`--find-renames`)。
    /// 会话视图按工具落盘记账,认不出改名,不会给出这个值。
    Renamed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    Context,
    Add,
    Del,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DiffLine {
    pub kind: LineKind,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Hunk {
    /// `@@ -1,4 +1,6 @@` 那一行。
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    /// 相对项目根的路径。绝对路径在界面上又长又没有信息量。
    pub path: String,
    pub status: ChangeStatus,
    pub added: usize,
    pub removed: usize,
    pub hunks: Vec<Hunk>,
    /// 差异太大,`hunks` 只是前一截。
    pub truncated: bool,
    /// 重命名前的旧路径。仅 status = renamed 时有。
    #[serde(default)]
    pub renamed_from: Option<String>,
    /// 二进制文件:没有可读的逐行差异,`hunks` 为空、行数为 0。
    #[serde(default)]
    pub binary: bool,
}

/// `session.git_changes` 的应答:工作区相对所选基线的差异。
///
/// 和会话改动(`session.changes`)回答的问题不同:那边是"这个会话经
/// 工具改了什么",commit 之后依然在;这边跟着 git 走。基线默认是
/// 当前分支(等于 HEAD);用户换分支只换对比对象,不 checkout。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GitChanges {
    /// false = 项目目录不是 git 仓库。面板显示引导文案,而不是把
    /// "没有仓库"和"工作区干净"混成同一个空列表。
    pub repo: bool,
    pub changes: Vec<FileChange>,
    /// 当前检出的分支。detached HEAD 时为空。
    #[serde(default)]
    pub branch: Option<String>,
    /// 实际用来 `git diff` 的基线(分支名或 HEAD)。
    #[serde(default)]
    pub base: Option<String>,
    /// 下拉里的候选:本地分支 + 远程跟踪分支。
    #[serde(default)]
    pub refs: Vec<String>,
}
