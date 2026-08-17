//! 会话变更集的传输形状。
//!
//! "本会话改了哪些文件、哪些行"面板的数据。算法在内核侧(riot-kernel 的
//! changes 模块:基线来自文件状态缓存,当前内容现读磁盘);这里只有要跨
//! 进程走 RPC(session.changes)的形状定义。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    Created,
    Modified,
    Deleted,
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
}
