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
pub async fn resolve(raw: &str, ctx: &ToolContext, must_exist: bool) -> Result<PathBuf, PathError> {
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
async fn resolve_parent(absolute: &Path, ctx: &ToolContext) -> Result<Option<PathBuf>, PathError> {
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
    path.strip_prefix(cwd).unwrap_or(path).display().to_string()
}

/// 解析之后，这条路径是不是"换了个人"。
///
/// `[约束]` 每一个会落盘或读取内容的工具，在真正动手之前都要过这一道。
///
/// 权限层判定用的是模型给的**原始字符串**（走
/// [`riot_protocol::tool::Tool::target_path`]），而真正读写用的是
/// `canonicalize` 之后的路径 —— 中间隔着一个符号链接。工作区里一个名叫
/// `docs/notes.md` 的链接指向 `~/.ssh/authorized_keys`：
/// [`riot_permissions::safety`] 看到的是 `docs/notes.md`（无风险），
/// acceptEdits 下自动放行，落盘落在 `authorized_keys` 上。默认模式也不
/// 安全 —— 弹窗上显示的是 `docs/notes.md`，用户批准的路径和实际写入的
/// 路径不是同一个。而链接的来源不需要是本地攻击者:git 能把符号链接提交
/// 进仓库，clone 一个别人的仓库就够了。
///
/// 这里只拦"解析之后才变敏感"的情形。原始路径本来就敏感时不管 ——
/// 那条已经在权限层被看见、被问过了，再拦一次就是把用户刚给的授权作废。
///
/// 收敛成**失败**而不是询问，是因为这里已经在 `call()` 里、过了闸，
/// 没有再问一次的通道。给模型的话要指向出路:用真实路径重来一次，
/// 那样用户在弹窗里看到的就是他实际要批准的东西。
pub fn detour_risk(raw: &str, resolved: &Path, cwd: &Path, read_only: bool) -> Option<String> {
    use riot_permissions::safety::write_target_risk;

    let given = Path::new(raw);
    let literal = if given.is_absolute() {
        given.to_path_buf()
    } else {
        cwd.join(given)
    };

    // 字面路径本身就敏感 —— 权限层已经看见了，这里不重复拦。
    if write_target_risk(&literal, read_only).is_some() {
        return None;
    }

    let kind = write_target_risk(resolved, read_only)?;
    Some(format!(
        "{raw} 解析之后指向 {}，而这是一个敏感目标（{}）。\
         授权是按你给的那个路径做的，和实际会被改动的文件不是同一个，\
         所以这次调用没有执行。如果确实要动它，请直接用真实路径重新调用。",
        resolved.display(),
        riot_permissions::safety::describe(kind, resolved)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detour(raw: &str, resolved: &str) -> Option<String> {
        detour_risk(raw, Path::new(resolved), Path::new("/work"), false)
    }

    #[test]
    fn 解析后才变敏感的路径被拦下() {
        // git 能把符号链接提交进仓库 —— clone 一个别人的仓库就够了。
        // acceptEdits 下这次写入会自动放行，而 safety 只看到 notes.md。
        let msg = detour("docs/notes.md", "/Users/u/.ssh/authorized_keys")
            .expect("链接指向 SSH 目录，必须拦");
        assert!(
            msg.contains("authorized_keys"),
            "要把真实目标说出来，否则模型不知道发生了什么：{msg}"
        );
    }

    #[test]
    fn 字面上就敏感的路径不重复拦() {
        // 这条已经在权限层被看见、被问过了。再拦一次等于把用户刚给的
        // 授权作废 —— 用户点了"允许"，工具却报错。
        assert_eq!(detour("/Users/u/.zshrc", "/Users/u/.zshrc"), None);
        assert_eq!(detour(".git/hooks/pre-commit", "/work/.git/hooks/pre-commit"), None);
    }

    #[test]
    fn 普通文件的解析结果不误伤() {
        // 每次 Write 都会过这一道，误报一次就是一次莫名其妙的失败
        assert_eq!(detour("src/main.rs", "/work/src/main.rs"), None);
        assert_eq!(detour("../sibling/a.rs", "/sibling/a.rs"), None);
    }

    #[test]
    fn 相对路径按_cwd_判字面形态() {
        // 相对路径不先拼 cwd 的话，`.zshrc`（工作区内的普通文件）会和
        // 解析出来的 `/Users/u/.zshrc` 一样敏感，于是判成"本来就敏感"
        // 而放过真正的绕道
        let msg = detour("notes", "/Users/u/.zshrc").expect("解析后指向 shell rc");
        assert!(msg.contains(".zshrc"), "{msg}");
    }

    #[test]
    fn 读路径上凭证同样要拦() {
        // 凭证是"读到即泄露"，一个指向私钥的链接能把它送进对话历史
        let msg = detour_risk(
            "docs/readme.md",
            Path::new("/Users/u/.ssh/id_rsa"),
            Path::new("/work"),
            true,
        )
        .expect("读也要拦");
        assert!(msg.contains("id_rsa"), "{msg}");
    }
}
