//! 先读后写协议。
//!
//! `[约束]` 修改一个已存在的文件之前，必须先完整 Read 过它，而且从那次
//! Read 到现在文件没有被外部改动。
//!
//! 三条检查各自防的是不同的事故:
//!
//! 1. **没读过就改** —— 模型凭猜测写 `old_string`，改中了同名的另一处;
//! 2. **只读过一部分** —— 模型以为看到了全文，把"这个函数只出现一次"
//!    当成事实，而它在没读到的那半边还有一个;
//! 3. **读完之后文件变了** —— 用户在编辑器里改了同一个文件，agent 的
//!    写入会把用户的改动整个盖掉，而且没有任何提示。
//!
//! 第 3 条要查两次:决策时一次，真正写入前再一次。中间隔着权限弹窗，
//! 用户可能盯着弹窗想了半分钟，这半分钟里什么都可能发生。
//!
//! 见 ARCHITECTURE.md §6.6

use std::path::Path;

use riot_protocol::tool::{FileState, FileView, ToolContext};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Staleness {
    /// 没有读过这个文件。
    NeverRead,
    /// 只读过一部分。
    PartialOnly { offset: usize, limit: usize },
    /// 读过之后文件被外部改动了。
    ChangedOnDisk { cached_ms: u64, disk_ms: u64 },
}

impl Staleness {
    /// 转成给模型的纠错指令。
    ///
    /// `[约束]` 每一条都要明确说"下一步做什么"。只说"文件已过期"的话，
    /// 模型的下一步往往是重试同样的调用。
    pub fn for_model(&self, path: &str) -> String {
        match self {
            Staleness::NeverRead => format!(
                "还没有读过 {path}。请先用 Read 读取这个文件，然后再修改它。"
            ),
            Staleness::PartialOnly { .. } => format!(
                "只读取了 {path} 的一部分。修改文件前需要看到完整内容 —— \
                 请不带 offset/limit 重新 Read 一次。"
            ),
            Staleness::ChangedOnDisk { .. } => format!(
                "{path} 在你读取之后被外部修改过。请重新 Read 获取最新内容，\
                 确认你的改动仍然适用，再重新提交修改。"
            ),
        }
    }
}

/// 检查一个已存在的文件能不能被修改。
///
/// 返回缓存里的状态 —— 调用方用它做内容比对。
pub async fn check_fresh(
    resolved: &Path,
    ctx: &ToolContext,
) -> Result<FileState, Staleness> {
    let Some(state) = ctx.file_state.get(resolved) else {
        return Err(Staleness::NeverRead);
    };

    if let FileView::Partial { offset, limit } = state.view {
        return Err(Staleness::PartialOnly { offset, limit });
    }

    // 拿不到磁盘 mtime 时放行 —— 文件可能刚被删掉，那个错误由后续的
    // 写入操作报出来会更准确。这里不是判断文件存不存在的地方。
    let Ok(meta) = ctx.fs.metadata(resolved).await else {
        return Ok(state);
    };

    if meta.mtime_ms > state.mtime_ms {
        return Err(Staleness::ChangedOnDisk {
            cached_ms: state.mtime_ms,
            disk_ms: meta.mtime_ms,
        });
    }

    Ok(state)
}

/// 写入前的最后一道复查：磁盘上的内容和我们以为的还一样吗？
///
/// `[约束]` 这一步不能省，而且必须比对**内容**而不是 mtime。
///
/// mtime 的精度在某些文件系统上只有 1 秒（HFS+、部分 NFS）。用户在同
/// 一秒内保存文件，mtime 完全可能不变 —— 那种情况下只查 mtime 等于没查。
/// 内容比对没有这个问题。
pub async fn verify_unchanged(
    resolved: &Path,
    expected: &str,
    ctx: &ToolContext,
) -> Result<(), String> {
    let Ok(bytes) = ctx.fs.read(resolved).await else {
        // 读不到就交给写入去报错
        return Ok(());
    };

    let Ok(current) = super::text::decode(&bytes) else {
        return Err(format!(
            "{} 现在是二进制内容，无法安全修改。",
            resolved.display()
        ));
    };

    if current.content != expected {
        return Err(format!(
            "{} 在这次操作进行期间被修改了。请重新 Read 后再试。",
            resolved.display()
        ));
    }

    Ok(())
}
