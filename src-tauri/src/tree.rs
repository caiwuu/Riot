//! 文件树的目录列表。
//!
//! 右侧抽屉里的文件浏览器一次只要一层：用户点开哪个目录才列哪个目录。
//! 不整棵预扫 —— `@` 补全那边的 [`crate::mentions`] 已经证明过一个几万
//! 文件的仓库扫一遍是秒级，而树是逐层点开的，把整棵树先扫完再显示
//! 首层是白等。
//!
//! # 边界
//!
//! 根是**会话根**，不是预览那套多根并集（见 [`crate::preview`]）：这是
//! "项目浏览器"，列的就是这个项目。相对路径经 [`Fence`] 解析，越界即拒；
//! 目录里指向围栏外的符号链接标成 `isSymlink` 且 `isDir = false` ——
//! 前端画成普通文件、不可展开。跟进去列它等于把 `~/.ssh -> link` 这类
//! 目录整个端到界面上。指向围栏内目录的链接照常可展开：下一层的
//! `list_dir` 会再过一次围栏，链接目标变了也拦得住。
//!
//! `.git` 一律不列（和 `mentions::walk` 一致）：几万个对象文件对用户
//! 没有可读性，模型也从不引用它们。别的点文件照列 —— `.github`、
//! `.cargo`、`.venv` 都是用户会点开看的。
//!
//! 单目录条目数有上限。`node_modules/.pnpm` 这种目录几万个子项，一次
//! IPC 全端过去、前端一次全渲染，两头都卡。截断的数量报回去，前端显示
//! 一行"还有 N 项"，用户知道少了什么。
//!
//! 豁免理由：宿主层操作真实文件系统。围栏判断要看真实的 symlink，注入
//! FileSystem 抽象会让边界检查失去意义（同 [`crate::fence`]）。

#![allow(clippy::disallowed_methods)]

use std::path::Path;

use crate::fence::{Fence, FenceError};

/// 单目录最多返回这么多条目。5000 已经远超任何人会翻的量。
pub const MAX_ENTRIES: usize = 5000;

/// 目录里的一项。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    pub name: String,
    /// 能不能展开。符号链接指向围栏内目录时也是 true。
    pub is_dir: bool,
    /// 是符号链接。前端在名字旁标一下；`is_dir = false` 的链接可能是
    /// 指向文件，也可能是指向围栏外 —— 两种都只能"打开"不能"展开"。
    pub is_symlink: bool,
}

/// 一层目录的内容。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirListing {
    /// 已排序：目录在前，同类按名字不分大小写排。
    pub entries: Vec<DirEntry>,
    /// 超出 [`MAX_ENTRIES`] 被截掉的条数。0 = 全给了。
    pub truncated: usize,
}

/// 列 `root` 下的 `rel` 目录。`rel` 为空串即根本身。
///
/// 错误文案是给人看的：前端把它摆在那个目录节点下面。
pub fn list_dir(root: &Path, rel: &str) -> Result<DirListing, String> {
    let fence = Fence::new(root).map_err(|e| match e {
        FenceError::Unresolvable { .. } => "项目目录不存在或读不到".to_owned(),
        other => other.to_string(),
    })?;
    // 前端统一用 `/` 拼相对路径（和 `@` 引用同一约定），Windows 上
    // `Path::join` 认得 `/`，不用换。
    let dir = fence.resolve(rel).map_err(|e| match e {
        FenceError::Escaped { .. } => "这个目录在项目之外".to_owned(),
        FenceError::Unresolvable { .. } => "目录不存在".to_owned(),
    })?;

    let read = std::fs::read_dir(&dir).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => "目录不存在".to_owned(),
        std::io::ErrorKind::PermissionDenied => "没有权限读这个目录".to_owned(),
        std::io::ErrorKind::NotADirectory => "这不是目录".to_owned(),
        _ => format!("读目录失败：{e}"),
    })?;

    let mut entries: Vec<DirEntry> = Vec::new();
    for entry in read.filter_map(Result::ok) {
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        // `file_type()` 不跟随符号链接：链接自己的类型。
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let is_symlink = kind.is_symlink();
        let is_dir = if is_symlink {
            // 跟到目标看是不是目录 —— 但只认围栏内的目标。`resolve` 会
            // canonicalize 过去，指向外面的直接 Err，这里就当它不可展开。
            fence
                .resolve(entry.path())
                .ok()
                .is_some_and(|target| target.is_dir())
        } else {
            kind.is_dir()
        };
        entries.push(DirEntry {
            name: name.to_string_lossy().into_owned(),
            is_dir,
            is_symlink,
        });
    }

    // 目录在前；同类不分大小写排，全同再按原名 —— 排序结果要稳定，
    // 两次列同一个目录不能一会儿 `README.md` 在 `readme.txt` 前一会儿后。
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });

    let truncated = entries.len().saturating_sub(MAX_ENTRIES);
    entries.truncate(MAX_ENTRIES);
    Ok(DirListing { entries, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(l: &DirListing) -> Vec<&str> {
        l.entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn 目录在前_名字不分大小写_git_不列() {
        let t = tempfile::tempdir().expect("目录");
        let root = t.path();
        std::fs::create_dir_all(root.join(".git/objects")).expect("建 .git");
        std::fs::create_dir_all(root.join("src")).expect("建 src");
        std::fs::create_dir_all(root.join(".github")).expect("建 .github");
        std::fs::write(root.join("README.md"), "").expect("写");
        std::fs::write(root.join("b.rs"), "").expect("写");
        std::fs::write(root.join("a.rs"), "").expect("写");

        let got = list_dir(root, "").expect("列根目录");
        assert_eq!(
            names(&got),
            vec![".github", "src", "a.rs", "b.rs", "README.md"]
        );
        assert_eq!(got.truncated, 0);
        assert!(got.entries[0].is_dir);
        assert!(!got.entries[2].is_dir);
    }

    #[test]
    fn 子目录按相对路径列() {
        let t = tempfile::tempdir().expect("目录");
        let root = t.path();
        std::fs::create_dir_all(root.join("src/deep")).expect("建目录");
        std::fs::write(root.join("src/main.rs"), "").expect("写");

        let got = list_dir(root, "src").expect("列 src");
        assert_eq!(names(&got), vec!["deep", "main.rs"]);
    }

    /// 这条是这个模块存在的理由：webview 里的一行 `invoke` 不能把项目外
    /// 的目录列出来。`..` 穿越和绝对路径都得挡住。
    #[test]
    fn 越界的路径列不到() {
        let t = tempfile::tempdir().expect("目录");
        let outside = tempfile::tempdir().expect("围栏外目录");
        std::fs::write(outside.path().join("secret"), "").expect("写");

        assert!(list_dir(t.path(), "../..").is_err());
        assert!(list_dir(t.path(), &outside.path().display().to_string()).is_err());
    }

    #[test]
    fn 不存在的目录和文件路径都报人话() {
        let t = tempfile::tempdir().expect("目录");
        std::fs::write(t.path().join("a.txt"), "").expect("写");

        assert_eq!(list_dir(t.path(), "nope").unwrap_err(), "目录不存在");
        let err = list_dir(t.path(), "a.txt").unwrap_err();
        // 平台差异：macOS/Linux 给 NotADirectory，Windows 给别的 kind。
        assert!(
            err == "这不是目录" || err.starts_with("读目录失败"),
            "文案：{err}"
        );
    }

    #[test]
    fn 超出上限的部分报截断数() {
        let t = tempfile::tempdir().expect("目录");
        let many = t.path().join("many");
        std::fs::create_dir_all(&many).expect("建目录");
        for i in 0..(MAX_ENTRIES + 7) {
            std::fs::write(many.join(format!("f{i:05}")), "").expect("写");
        }

        let got = list_dir(t.path(), "many").expect("列");
        assert_eq!(got.entries.len(), MAX_ENTRIES);
        assert_eq!(got.truncated, 7);
    }

    /// 围栏内一个指向围栏外目录的符号链接：能看见它，但不能展开 ——
    /// 展开等于把 `~/.ssh` 整个端到界面上。
    #[cfg(unix)]
    #[test]
    fn 指向围栏外的链接不可展开_围栏内的可以() {
        let t = tempfile::tempdir().expect("目录");
        let outside = tempfile::tempdir().expect("围栏外目录");
        let root = t.path();
        std::fs::create_dir_all(root.join("real")).expect("建目录");
        std::os::unix::fs::symlink(outside.path(), root.join("escape")).expect("建链接");
        std::os::unix::fs::symlink(root.join("real"), root.join("inside")).expect("建链接");

        let got = list_dir(root, "").expect("列根目录");
        let by_name = |n: &str| got.entries.iter().find(|e| e.name == n).expect("有这项");
        let escape = by_name("escape");
        assert!(escape.is_symlink);
        assert!(!escape.is_dir, "围栏外的链接不能标成可展开");
        let inside = by_name("inside");
        assert!(inside.is_symlink);
        assert!(inside.is_dir, "围栏内的目录链接可以展开");

        // 就算前端硬发一次，越界链接的下一层也列不到。
        assert!(list_dir(root, "escape").is_err());
    }
}
