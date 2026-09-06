//! 给网页版用的目录浏览：一层一层翻宿主机的目录树。
//!
//! 桌面窗口选项目目录走系统对话框；浏览器里没有这个东西 —— 手机上的文件
//! 选择器选的是手机的文件，而项目目录在跑 Riot 的那台机器上。所以要一个
//! 宿主侧的"列出这个目录下有哪些子目录"，前端拿它画一个最朴素的目录选择器。
//!
//! 只列目录、不列文件（这是"选项目根"，不是文件管理器）；跳过点开头的
//! 目录，和系统对话框的默认行为一致。不做任何围栏：这条命令和 `add_project`
//! 一样，是主人自己在挑目录，主人本来就能看自己机器上的一切。

// 宿主层：读真实目录。见 clippy.toml。
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use serde::Serialize;

/// 一层目录的内容。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirBrowse {
    /// 规范化后的当前目录。
    pub path: String,
    /// 上一级。已经在根了就是 `None`。
    pub parent: Option<String>,
    /// 子目录，按名字排序。
    pub entries: Vec<DirEntry>,
    /// 读目录时出的错（权限不够之类）。有错时 `entries` 是空的，但 `path`
    /// 和 `parent` 照常给 —— 用户至少能退回上一级。
    pub error: Option<String>,
    /// 请求的路径不存在（或不是目录），这次列的是退回去的家目录。带上
    /// 原路径让界面说清楚"你要的那个没有"，而不是静默跳走 —— 用户手打
    /// 错一个字母，看到的不该是"怎么跑到家目录了"。
    pub missing: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    pub name: String,
    pub path: String,
}

/// 列出 `path` 下的子目录。`None` 或空串从家目录开始；不存在的路径退回家目录。
pub async fn browse(path: Option<String>) -> DirBrowse {
    let requested = path.filter(|p| !p.trim().is_empty()).map(PathBuf::from);
    let missing = requested
        .as_ref()
        .filter(|p| !p.is_dir())
        .map(|p| p.display().to_string());
    let start = requested
        .filter(|p| p.is_dir())
        .or_else(home)
        .unwrap_or_else(|| PathBuf::from("/"));
    let dir = tokio::fs::canonicalize(&start)
        .await
        .unwrap_or(start)
        .to_path_buf();
    let dir = crate::fence::strip_verbatim(dir);

    let parent = dir.parent().map(|p| p.display().to_string());
    let mut entries = Vec::new();
    let mut error = None;
    match tokio::fs::read_dir(&dir).await {
        Ok(mut rd) => {
            while let Ok(Some(e)) = rd.next_entry().await {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let is_dir = match e.file_type().await {
                    Ok(t) if t.is_symlink() => e.path().is_dir(),
                    Ok(t) => t.is_dir(),
                    Err(_) => false,
                };
                if !is_dir {
                    continue;
                }
                entries.push(DirEntry {
                    path: e.path().display().to_string(),
                    name,
                });
            }
        }
        Err(e) => error = Some(format!("读不了这个目录：{e}")),
    }
    entries.sort_by_key(|e| e.name.to_lowercase());
    DirBrowse {
        path: dir.display().to_string(),
        parent,
        entries,
        error,
        missing,
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| Path::new(p).is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn 只列子目录且跳过点目录() {
        let root = tempfile::tempdir().expect("临时目录");
        std::fs::create_dir(root.path().join("b-dir")).expect("建目录");
        std::fs::create_dir(root.path().join("A-dir")).expect("建目录");
        std::fs::create_dir(root.path().join(".hidden")).expect("建目录");
        std::fs::File::create(root.path().join("file.txt")).expect("建文件");

        let out = browse(Some(root.path().display().to_string())).await;
        let names: Vec<_> = out.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["A-dir", "b-dir"], "按名排序、不含文件和点目录");
        assert!(out.parent.is_some());
        assert!(out.error.is_none());
        assert!(out.missing.is_none());
    }

    #[tokio::test]
    async fn 不存在的路径退回家目录_并说明要的那个没有() {
        let out = browse(Some("/definitely/not/here".to_owned())).await;
        assert!(Path::new(&out.path).is_dir());
        assert_eq!(out.missing.as_deref(), Some("/definitely/not/here"));
        // 没传路径就是正常从家目录开始，不算"没找到"。
        assert!(browse(None).await.missing.is_none());
    }
}
