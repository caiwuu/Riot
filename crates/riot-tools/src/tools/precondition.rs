//! 先读后写协议。
//!
//! Write 覆盖整个文件，必须先 Read 过，而且从那次 Read 到现在磁盘
//! 没有被外部改动 —— 否则会把用户的改动整份盖掉。
//!
//! Edit 是精确替换。`old_string` 必须在**当前磁盘全文**里唯一命中，
//! 这已经能挡住猜错位置。再逼模型"先完整 Read"是死锁：Read 单次最多
//! 2000 行，超长文件永远是 Partial，报错 → 重读 → 还是 Partial。
//! 所以 Edit 自己从磁盘载入全文，不把"请再读一遍"踢回给模型。
//!
//! 写入前的内容比对（[`verify_unchanged`]）两边都要做。中间隔着权限
//! 弹窗，用户可能盯着弹窗想了半分钟。
//!
//! 见 ARCHITECTURE.md §6.6

use std::path::Path;

use riot_protocol::tool::{FileState, FileView, ToolContext};

use super::text;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Staleness {
    /// 没有读过这个文件。
    NeverRead,
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
            Staleness::NeverRead => {
                format!("还没有读过 {path}。请先用 Read 读取这个文件，然后再修改它。")
            }
            Staleness::ChangedOnDisk { .. } => format!(
                "{path} 在你读取之后被外部修改过。请重新 Read 获取最新内容，\
                 确认你的改动仍然适用，再重新提交修改。"
            ),
        }
    }
}

/// Write 用：没读过或磁盘变了就拒绝。只看过一段则升成 Full。
pub async fn check_fresh(resolved: &Path, ctx: &ToolContext) -> Result<FileState, Staleness> {
    let Some(state) = ctx.file_state.get(resolved) else {
        return Err(Staleness::NeverRead);
    };

    // 拿不到磁盘 mtime 时放行 —— 文件可能刚被删掉，那个错误由后续的
    // 写入操作报出来会更准确。这里不是判断文件存不存在的地方。
    let Ok(meta) = ctx.fs.metadata(resolved).await else {
        return Ok(promote_full(resolved, state, ctx));
    };

    if meta.mtime_ms > state.mtime_ms {
        return Err(Staleness::ChangedOnDisk {
            cached_ms: state.mtime_ms,
            disk_ms: meta.mtime_ms,
        });
    }

    Ok(promote_full(resolved, state, ctx))
}

/// Edit 用：缓存没有、只看过一段、或磁盘更新了，都从磁盘载入全文。
///
/// 不把"请先 Read / 请再读一遍"踢回给模型。唯一性检查按这份全文做。
pub async fn ensure_loaded(resolved: &Path, ctx: &ToolContext) -> Result<FileState, String> {
    if let Some(state) = ctx.file_state.get(resolved)
        && let Ok(meta) = ctx.fs.metadata(resolved).await
        && meta.mtime_ms <= state.mtime_ms
    {
        return Ok(promote_full(resolved, state, ctx));
    }

    load_from_disk(resolved, ctx).await
}

/// Read 截断给模型看的那一份，缓存里已经是全文。升成 Full，后续 Write
/// 不再被同一条"请再读一遍"拦住。
fn promote_full(resolved: &Path, state: FileState, ctx: &ToolContext) -> FileState {
    if state.view == FileView::Full {
        return state;
    }

    let full = FileState {
        view: FileView::Full,
        ..state
    };
    ctx.file_state.put(resolved.to_path_buf(), full.clone());
    full
}

async fn load_from_disk(resolved: &Path, ctx: &ToolContext) -> Result<FileState, String> {
    let meta = ctx
        .fs
        .metadata(resolved)
        .await
        .map_err(|e| format!("读不了 {}：{e}", resolved.display()))?;
    if meta.is_dir {
        return Err(format!("{} 是目录，不能修改。", resolved.display()));
    }

    let bytes = ctx
        .fs
        .read(resolved)
        .await
        .map_err(|e| format!("读不了 {}：{e}", resolved.display()))?;
    let file = text::decode(&bytes)
        .map_err(|e| format!("{} 无法作为文本修改（{e}）。", resolved.display()))?;

    let state = FileState {
        content: file.content,
        mtime_ms: meta.mtime_ms,
        view: FileView::Full,
    };
    ctx.file_state.put(resolved.to_path_buf(), state.clone());
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
