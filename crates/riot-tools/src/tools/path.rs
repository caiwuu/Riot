//! 工具侧的路径解析。
//!
//! 把模型给的路径变成一个可以真正读写的绝对路径:相对路径按会话的工作
//! 目录展开,存在的目标做 `canonicalize`,不存在的解析父目录。
//!
//! # 这里不再有工作目录围栏
//!
//! 早先每个路径都要落在会话绑定的目录内,越界直接拒绝。那条限制去掉了,
//! 项目目录只作分类和相对路径的基准 —— 跨目录读写是正常需求。挡在危险
//! 操作前面的是权限层(逐次询问 + 敏感路径检查),不是这里。见
//! [`riot_permissions::fence`] 的模块文档。
//!
//! 形状检查留着:NUL 字节、NTFS 数据流、DOS 设备名这些和边界无关,
//! 它们本身就不该出现在一个正常路径里。
//!
//! `[约束]` Windows 上 `canonicalize` 返回的是 verbatim 形式(`\\?\D:\…`),
//! 每个结果都要过 [`fence::strip_verbatim`] 再往下走。漏掉的话形状检查会
//! 拿盘符的冒号当 NTFS 数据流,把每一个存在的文件都拒掉。

use std::path::{Path, PathBuf};

use riot_permissions::fence::{self, FenceViolation};
use riot_protocol::tool::ToolContext;

#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("{0}")]
    Fence(#[from] FenceViolation),

    #[error("路径为空")]
    Empty,

    #[error("{path} 不存在")]
    NotFound { path: PathBuf },

    #[error("无法解析路径 {path}：{source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl PathError {
    /// 转成给模型的纠错指令。
    ///
    /// `[约束]` 用祈使句说"该怎么做",不要贴原始错误。见 ARCHITECTURE.md §6.5
    pub fn for_model(&self) -> String {
        match self {
            PathError::Fence(FenceViolation::Suspicious { path, reason }) => format!(
                "路径 {} 含有不被接受的构造（{reason}）。请使用普通的文件路径。",
                path.display()
            ),
            PathError::Empty => "路径参数不能为空。请提供文件路径。".to_owned(),
            PathError::NotFound { path } => format!(
                "文件 {} 不存在。请确认路径是否正确，可以用 Glob 查找。",
                path.display()
            ),
            PathError::Io { path, .. } => format!(
                "无法访问路径 {} 所在的目录。请确认上级目录存在。",
                path.display()
            ),
        }
    }
}

/// 解析并校验一个路径。
///
/// `must_exist = false` 时用于 Write 创建新文件 —— 这时解析父目录。
pub async fn resolve(
    raw: &str,
    ctx: &ToolContext,
    must_exist: bool,
) -> Result<PathBuf, PathError> {
    if raw.trim().is_empty() {
        return Err(PathError::Empty);
    }

    let given = Path::new(raw);
    let absolute = if given.is_absolute() {
        given.to_path_buf()
    } else {
        ctx.cwd.join(given)
    };

    // 形状检查要在碰文件系统之前 —— 那些别名构造的目的就是让解析结果
    // 和字面看起来不一样，先解析再检查等于自己把证据抹了。
    fence::check_shape(&absolute)?;

    let resolved = if must_exist {
        // NotFound 要和别的 IO 错误分开。合并的话，模型看到的是
        // "无法访问上级目录"，于是跑去检查目录 —— 而真正的问题是
        // 它把文件名拼错了。
        // strip_verbatim 不能省：Windows 的 canonicalize 返回 `\\?\D:\…`，
        // 那个前缀会让下面第二道形状检查把每一个存在的文件都判成"设备路径
        // 前缀 + NTFS 数据流"（盘符的冒号），Read/Write 于是全线失败。
        Some(
            ctx.fs
                .canonicalize(&absolute)
                .await
                .map(fence::strip_verbatim)
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        PathError::NotFound {
                            path: absolute.clone(),
                        }
                    } else {
                        PathError::Io {
                            path: absolute.clone(),
                            source: e,
                        }
                    }
                })?,
        )
    } else {
        // 目标可能还不存在。解析父目录 —— 这是 symlink 能藏身的地方:
        // `work/link/new.txt` 里 `link` 指向 /etc，而 new.txt 不存在，
        // 所以对完整路径 canonicalize 会失败，检查就被跳过了。
        resolve_parent(&absolute, ctx).await?
    };

    // 解析后再查一次形状。symlink 可以指向一个字面上看不出问题、
    // 解析后却含可疑构造的目标。
    if let Some(r) = &resolved {
        fence::check_shape(r)?;
    }

    Ok(resolved.unwrap_or(absolute))
}

/// 解析父目录并拼回文件名。
///
/// 父目录也不存在时返回 `None` —— 让字面检查兜底。Write 会在真正
/// 写入时因为目录不存在而失败，那个错误信息比这里编一个更准确。
async fn resolve_parent(
    absolute: &Path,
    ctx: &ToolContext,
) -> Result<Option<PathBuf>, PathError> {
    let (Some(parent), Some(name)) = (absolute.parent(), absolute.file_name()) else {
        return Ok(None);
    };

    match ctx.fs.canonicalize(parent).await {
        Ok(p) => Ok(Some(fence::strip_verbatim(p).join(name))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(PathError::Io {
            path: parent.to_path_buf(),
            source: e,
        }),
    }
}

/// 展示给用户/模型的短路径。绝对路径太长会挤爆 UI。
pub fn display_relative(path: &Path, cwd: &Path) -> String {
    path.strip_prefix(cwd)
        .unwrap_or(path)
        .display()
        .to_string()
}
