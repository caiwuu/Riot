//! 工作区路径围栏。
//!
//! # 为什么不用 tauri-plugin-fs
//!
//! 那个插件从设计上就禁止路径穿越：`../path/to/file` 一律拒绝。而 agent 要
//! 操作用户任意工作区，这不是配置能绕开的限制。
//!
//! 自己写反而能实现**更有意义**的边界：以「用户显式打开的工作区」为根，
//! canonicalize 后做组件级前缀校验。这比 capability 的静态 glob 更贴合
//! agent 的动态语义。
//!
//! # 四个必须一起做对的点
//!
//! 1. **必须 canonicalize 后再比较。**光看字符串，`root/../../etc/passwd`
//!    会被判定为在围栏内。
//! 2. **必须按路径组件比较，不能按字符串前缀。**`/work` 的字符串前缀能匹配
//!    `/workspace-of-someone-else`。`Path::starts_with` 是按组件走的，用它。
//! 3. **要创建的文件还不存在，canonicalize 会失败。**得先解析最近的存在祖先，
//!    再把剩余部分拼回去。漏掉这条的话，agent 一个文件都创建不了。
//! 4. **符号链接。**canonicalize 会跟随 symlink，所以工作区内一个指向 `/etc`
//!    的链接会被正确识别为越界 —— 这正是我们要的。

// 这里必须用真实文件系统，注入 FileSystem trait 会让整个安全边界变成假的：
// canonicalize 要看真实的 inode 和 symlink 才能判断越界，mock 出来的
// "已解析路径" 证明不了任何事。这也是 fence 不参与黄金回放的原因。
#![allow(clippy::disallowed_methods)]

use std::path::{Component, Path, PathBuf};

/// 和工具侧的形状检查共用一份实现 —— 两边都要拿 `canonicalize` 的结果去做
/// 字符串比较，各留一份迟早只修一边。见 [`riot_permissions::fence`]。
pub(crate) use riot_permissions::fence::strip_verbatim;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FenceError {
    #[error("路径越界: {} 不在工作区 {} 内", path.display(), root.display())]
    Escaped { path: PathBuf, root: PathBuf },
    #[error("无法解析路径 {}: {msg}", path.display())]
    Unresolvable { path: PathBuf, msg: String },
}

/// 工作区围栏。构造时 root 已经过 canonicalize。
#[derive(Debug, Clone)]
pub struct Fence {
    root: PathBuf,
}

/// 哪些路径现在不是目录。侧栏标失效项目用，不改配置、不建围栏。
///
/// 只看 `is_dir`：canonicalize 在目录已删时必然失败，这里不需要再走
/// 一遍围栏。传入的字符串原样返回，方便前端按项目列表的 key 对上。
pub fn missing_dirs(paths: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<String> {
    paths
        .into_iter()
        .map(|p| p.as_ref().to_owned())
        .filter(|p| !Path::new(p).is_dir())
        .collect()
}

impl Fence {
    /// `root` 必须已存在。
    pub fn new(root: impl AsRef<Path>) -> Result<Self, FenceError> {
        let root = root.as_ref();
        let canonical = std::fs::canonicalize(root).map_err(|e| FenceError::Unresolvable {
            path: root.to_path_buf(),
            msg: e.to_string(),
        })?;
        Ok(Self {
            root: strip_verbatim(canonical),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 把一个（可能是相对的、可能还不存在的）路径解析成围栏内的绝对路径。
    ///
    /// 越界返回 `Escaped`。这是**唯一**该被文件操作命令调用的入口 ——
    /// 任何绕过它直接用用户传入路径的地方都是漏洞。
    pub fn resolve(&self, requested: impl AsRef<Path>) -> Result<PathBuf, FenceError> {
        let requested = requested.as_ref();
        let joined = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            self.root.join(requested)
        };

        let resolved = canonicalize_lexically_existing(&joined)?;

        if resolved.starts_with(&self.root) {
            Ok(resolved)
        } else {
            Err(FenceError::Escaped {
                path: resolved,
                root: self.root.clone(),
            })
        }
    }
}

/// canonicalize 一条可能尚不存在的路径。
///
/// 向上找到最近的存在祖先做真 canonicalize（这一步会解开 symlink），
/// 再把剩余组件按词法拼回去。剩余部分不可能含 symlink —— 它们还不存在。
fn canonicalize_lexically_existing(path: &Path) -> Result<PathBuf, FenceError> {
    let mut existing = path.to_path_buf();
    // 存 OsString 而非 &OsStr —— 借用自 existing 的话下面 pop 不了。
    let mut tail: Vec<std::ffi::OsString> = Vec::new();

    loop {
        match std::fs::canonicalize(&existing) {
            Ok(base) => {
                // 和 Fence::new 用同一套归一形式。root 剥了前缀而这里不剥的话，
                // starts_with 按组件比较（VerbatimDisk ≠ Disk），一切都成越界。
                let mut out = strip_verbatim(base);
                for part in tail.iter().rev() {
                    out.push(part);
                }
                return Ok(out);
            }
            Err(_) => {
                let Some(name) = existing.file_name().map(std::ffi::OsString::from) else {
                    return Err(FenceError::Unresolvable {
                        path: path.to_path_buf(),
                        msg: "向上找不到任何存在的祖先".into(),
                    });
                };
                // `..` 和 `.` 不能当普通组件往回拼 —— 那样等于放行穿越。
                // 遇到就先词法规约整条路径再重来。
                if matches!(
                    Path::new(&name).components().next(),
                    Some(Component::ParentDir | Component::CurDir)
                ) {
                    let normalized = lexical_normalize(path);
                    if normalized == path {
                        return Err(FenceError::Unresolvable {
                            path: path.to_path_buf(),
                            msg: "路径无法规约".into(),
                        });
                    }
                    return canonicalize_lexically_existing(&normalized);
                }
                tail.push(name);
                if !existing.pop() {
                    return Err(FenceError::Unresolvable {
                        path: path.to_path_buf(),
                        msg: "向上找不到任何存在的祖先".into(),
                    });
                }
            }
        }
    }
}

/// 纯词法地消解 `.` 和 `..`，不碰文件系统。
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 缺失目录能找出来() {
        let dir = tempfile::tempdir().expect("建临时目录");
        let gone = dir.path().join("nope").to_string_lossy().into_owned();
        let live = dir.path().to_string_lossy().into_owned();
        assert_eq!(missing_dirs([&gone, &live]), vec![gone]);
    }

    fn tmp_fence() -> (tempfile::TempDir, Fence) {
        let dir = tempfile::tempdir().expect("建临时目录");
        let fence = Fence::new(dir.path()).expect("建围栏");
        (dir, fence)
    }

    #[test]
    fn 围栏内的相对路径正常解析() {
        let (dir, fence) = tmp_fence();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();

        let got = fence.resolve("a.rs").expect("应该放行");
        assert!(got.starts_with(fence.root()));
        assert_eq!(got.file_name().unwrap(), "a.rs");
    }

    #[test]
    fn 尚不存在的文件也能解析() {
        let (_dir, fence) = tmp_fence();
        let got = fence
            .resolve("src/deep/new_file.rs")
            .expect("要创建的文件必须能解析，否则 agent 一个文件都建不了");
        assert!(got.ends_with("src/deep/new_file.rs"));
        assert!(got.starts_with(fence.root()));
    }

    #[test]
    fn 父目录穿越被拦截() {
        let (_dir, fence) = tmp_fence();
        let err = fence.resolve("../../../etc/passwd").unwrap_err();
        assert!(matches!(err, FenceError::Escaped { .. }), "实际: {err:?}");
    }

    #[test]
    fn 绝对路径越界被拦截() {
        let (_dir, fence) = tmp_fence();
        let err = fence.resolve("/etc/passwd").unwrap_err();
        assert!(matches!(err, FenceError::Escaped { .. }), "实际: {err:?}");
    }

    #[test]
    fn 指向围栏外的符号链接被拦截() {
        let (dir, fence) = tmp_fence();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "x").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path().join("secret"), dir.path().join("link")).unwrap();
        // Windows 上创建符号链接要管理员或开发者模式。CI 的 runner 有这个
        // 权限，普通开发机常常没有 —— 造不出链接就没得测，跳过而不是红。
        #[cfg(windows)]
        if let Err(e) = std::os::windows::fs::symlink_file(
            outside.path().join("secret"),
            dir.path().join("link"),
        ) {
            eprintln!("跳过：这台机器创建不了符号链接（{e}）");
            return;
        }

        let err = fence.resolve("link").unwrap_err();
        assert!(
            matches!(err, FenceError::Escaped { .. }),
            "symlink 逃逸必须拦住，实际: {err:?}"
        );
    }

    #[test]
    fn 同前缀的兄弟目录不算围栏内() {
        // 经典 bug：字符串前缀匹配会把 /tmp/work-evil 判定为在 /tmp/work 内。
        let parent = tempfile::tempdir().unwrap();
        let work = parent.path().join("work");
        let evil = parent.path().join("work-evil");
        std::fs::create_dir(&work).unwrap();
        std::fs::create_dir(&evil).unwrap();
        std::fs::write(evil.join("f"), "").unwrap();

        let fence = Fence::new(&work).unwrap();
        let err = fence.resolve(evil.join("f")).unwrap_err();
        assert!(
            matches!(err, FenceError::Escaped { .. }),
            "work-evil 不是 work 的子目录，实际: {err:?}"
        );
    }

    #[test]
    fn 围栏内绕一圈回来仍算围栏内() {
        let (dir, fence) = tmp_fence();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();

        let got = fence.resolve("sub/../a.rs").expect("这在围栏内，应放行");
        assert!(got.starts_with(fence.root()));
    }

    /// Windows 的 canonicalize 给的是 `\\?\D:\…`。它一旦漏出去，前端按
    /// 字符串分组项目就会多出一个 `\\?\` 开头的幽灵项目。
    #[cfg(windows)]
    #[test]
    fn 根和解析结果都不带_verbatim_前缀() {
        let (dir, fence) = tmp_fence();
        let root = fence.root().to_string_lossy().into_owned();
        assert!(
            !root.starts_with(r"\\?\"),
            "root 漏出了 verbatim 形式：{root}"
        );

        std::fs::write(dir.path().join("a.rs"), "").unwrap();
        let existing = fence.resolve("a.rs").expect("放行");
        let missing = fence.resolve("sub/new.rs").expect("放行");
        for p in [existing, missing] {
            let s = p.to_string_lossy().into_owned();
            assert!(!s.starts_with(r"\\?\"), "resolve 漏出了 verbatim 形式：{s}");
        }
    }

    // strip_verbatim 自身的单元测试在 riot-permissions::fence —— 实现搬过去
    // 和工具侧共用了。上面这条留着，它测的是围栏这一层的集成行为。
}
