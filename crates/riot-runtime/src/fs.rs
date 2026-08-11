//! 真实文件系统。
//!
//! `[约束]` 写文件走**同目录临时文件 + rename**，不是直接覆盖。
//!
//! 直接 `write` 到目标路径的话，进程在写到一半时挂掉会留下一个截断的文件，
//! 而原内容已经没了。对着一个正在被编辑的源码文件，这就是数据丢失。
//! rename 在同一文件系统内是原子的：要么是旧内容，要么是新内容。
//!
//! "同目录"是必须的 —— 跨文件系统 rename 会失败（EXDEV），退化成拷贝，
//! 原子性就没了。`/tmp` 在很多机器上是独立挂载。

#![allow(clippy::disallowed_methods)]
#![allow(clippy::disallowed_types)]
// 这个文件就是 OS 交互层本身，禁用列表针对的是内核逻辑。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use riot_protocol::tool::{FileMeta, FileState, FileStateCache, FileSystem};

#[derive(Default)]
pub struct SystemFs;

impl SystemFs {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl FileSystem for SystemFs {
    async fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        tokio::fs::read(path).await
    }

    async fn write(&self, path: &Path, data: &[u8]) -> std::io::Result<()> {
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        tokio::fs::create_dir_all(dir).await?;

        // 文件名带 pid 和一个计数器，避免同一进程内并发写同名文件时打架。
        let tmp = dir.join(format!(
            ".{}.{}.{}.tmp",
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unnamed".into()),
            std::process::id(),
            next_seq(),
        ));

        // 保住原文件的权限位。不这么做的话，一个 755 的脚本被编辑之后
        // 会变成 644，然后"明明没改什么它就不能执行了"。
        let mode = existing_mode(path).await;

        if let Err(e) = tokio::fs::write(&tmp, data).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e);
        }

        #[cfg(unix)]
        if let Some(m) = mode {
            use std::os::unix::fs::PermissionsExt;
            let _ = tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(m)).await;
        }
        #[cfg(not(unix))]
        let _ = mode;

        if let Err(e) = tokio::fs::rename(&tmp, path).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e);
        }
        Ok(())
    }

    async fn metadata(&self, path: &Path) -> std::io::Result<FileMeta> {
        let m = tokio::fs::metadata(path).await?;
        Ok(FileMeta {
            mtime_ms: mtime_ms(&m),
            len: m.len(),
            is_dir: m.is_dir(),
        })
    }

    async fn read_dir(&self, path: &Path) -> std::io::Result<Vec<PathBuf>> {
        let mut rd = tokio::fs::read_dir(path).await?;
        let mut out = Vec::new();
        while let Some(e) = rd.next_entry().await? {
            out.push(e.path());
        }
        // 目录顺序在不同文件系统上不一样。排序让工具输出可复现 ——
        // 否则同一个提示在两台机器上会得到不同的上下文。
        out.sort();
        Ok(out)
    }

    async fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        tokio::fs::canonicalize(path).await
    }
}

async fn existing_mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = tokio::fs::metadata(path).await.ok()?;
        Some(m.permissions().mode())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

fn next_seq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

fn mtime_ms(m: &std::fs::Metadata) -> u64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        // 拿不到 mtime 时返回 0 而不是当前时间。
        //
        // `[约束]` 这个选择关系到"先读后写"协议的安全性：返回 now() 的话，
        // 读和写两次拿到的值必然不同，看起来像"文件被改过"，于是每次编辑
        // 都失败；返回 0 则两次一致，协议退化成只靠内容比对 —— 那一层
        // 仍然在，所以是安全的。
        .unwrap_or(0)
}

/// 进程内的文件状态缓存。
///
/// 不持久化。进程重启后模型必须重新读文件才能编辑 —— 这正是想要的：
/// 缓存里的内容是上一次进程看到的，磁盘上的文件这期间完全可能被改过。
#[derive(Default)]
pub struct MemoryFileState {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    map: HashMap<PathBuf, FileState>,
    /// 访问顺序，最新在后。压缩后恢复工作集要按这个顺序。
    order: Vec<PathBuf>,
}

impl MemoryFileState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }
}

impl FileStateCache for MemoryFileState {
    fn get(&self, path: &Path) -> Option<FileState> {
        self.inner.lock().ok()?.map.get(path).cloned()
    }

    fn put(&self, path: PathBuf, state: FileState) {
        let Ok(mut g) = self.inner.lock() else { return };
        g.order.retain(|p| p != &path);
        g.order.push(path.clone());
        g.map.insert(path, state);
    }

    fn invalidate(&self, path: &Path) {
        let Ok(mut g) = self.inner.lock() else { return };
        g.map.remove(path);
        g.order.retain(|p| p != path);
    }

    fn recent(&self, limit: usize) -> Vec<(PathBuf, FileState)> {
        let Ok(g) = self.inner.lock() else {
            return Vec::new();
        };
        g.order
            .iter()
            .rev()
            .take(limit)
            .filter_map(|p| g.map.get(p).map(|s| (p.clone(), s.clone())))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use riot_protocol::tool::FileView;

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("riot-fs-{}-{}", std::process::id(), next_seq()));
        std::fs::create_dir_all(&d).expect("建临时目录");
        d
    }

    #[tokio::test]
    async fn 读写往返() {
        let d = tmpdir();
        let f = d.join("a.txt");
        let fs = SystemFs::new();

        fs.write(&f, "内容".as_bytes()).await.expect("写");
        assert_eq!(fs.read(&f).await.expect("读"), "内容".as_bytes());
    }

    #[tokio::test]
    async fn 写入不留临时文件() {
        // 临时文件泄漏会污染用户的工作目录，而且 git status 会一直有噪声
        let d = tmpdir();
        let fs = SystemFs::new();
        fs.write(&d.join("a.txt"), b"x").await.expect("写");

        let left: Vec<_> = fs
            .read_dir(&d)
            .await
            .expect("列目录")
            .into_iter()
            .filter(|p| p.to_string_lossy().contains(".tmp"))
            .collect();
        assert!(left.is_empty(), "残留临时文件：{left:?}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn 覆盖写保留可执行位() {
        // 丢权限位的话，一个可执行脚本被编辑之后就跑不了了，
        // 而错误信息完全不会提到权限
        use std::os::unix::fs::PermissionsExt;

        let d = tmpdir();
        let f = d.join("run.sh");
        std::fs::write(&f, b"#!/bin/sh\n").expect("建文件");
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).expect("改权限");

        SystemFs::new()
            .write(&f, b"#!/bin/sh\necho hi\n")
            .await
            .expect("写");

        let mode = std::fs::metadata(&f).expect("元信息").permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "可执行位丢了");
    }

    #[tokio::test]
    async fn 写入会自动建目录() {
        let d = tmpdir();
        let f = d.join("a/b/c.txt");
        SystemFs::new().write(&f, b"x").await.expect("写");
        assert!(f.exists());
    }

    #[tokio::test]
    async fn 目录列表有序() {
        // 无序的话同一个提示在两台机器上会得到不同的上下文
        let d = tmpdir();
        for n in ["c", "a", "b"] {
            std::fs::write(d.join(n), b"").expect("建文件");
        }
        let names: Vec<_> = SystemFs::new()
            .read_dir(&d)
            .await
            .expect("列目录")
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[tokio::test]
    async fn 元信息带上_mtime() {
        let d = tmpdir();
        let f = d.join("a.txt");
        let fs = SystemFs::new();
        fs.write(&f, b"x").await.expect("写");

        let m = fs.metadata(&f).await.expect("元信息");
        assert!(!m.is_dir);
        assert_eq!(m.len, 1);
        assert!(m.mtime_ms > 0, "先读后写协议依赖 mtime");
    }

    #[tokio::test]
    async fn 读不存在的文件报错() {
        let e = SystemFs::new()
            .read(Path::new("/nonexistent-riot-xyz"))
            .await
            .expect_err("应该失败");
        assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn 缓存存取() {
        let c = MemoryFileState::new();
        let p = PathBuf::from("/a");
        let s = FileState {
            content: "x".into(),
            mtime_ms: 1,
            view: FileView::Full,
        };
        c.put(p.clone(), s.clone());
        assert_eq!(c.get(&p), Some(s));

        c.invalidate(&p);
        assert_eq!(c.get(&p), None);
    }

    #[test]
    fn 最近访问的排在前面() {
        // 压缩后恢复工作集要按这个顺序挑文件
        let c = MemoryFileState::new();
        for n in ["a", "b", "c"] {
            c.put(
                PathBuf::from(n),
                FileState {
                    content: n.into(),
                    mtime_ms: 0,
                    view: FileView::Full,
                },
            );
        }
        let recent: Vec<_> = c
            .recent(2)
            .into_iter()
            .map(|(p, _)| p.to_string_lossy().to_string())
            .collect();
        assert_eq!(recent, vec!["c", "b"]);
    }

    #[test]
    fn 重复访问会提到最前() {
        let c = MemoryFileState::new();
        let st = |s: &str| FileState {
            content: s.into(),
            mtime_ms: 0,
            view: FileView::Full,
        };
        c.put(PathBuf::from("a"), st("a"));
        c.put(PathBuf::from("b"), st("b"));
        c.put(PathBuf::from("a"), st("a2"));

        let recent: Vec<_> = c
            .recent(9)
            .into_iter()
            .map(|(p, _)| p.to_string_lossy().to_string())
            .collect();
        assert_eq!(recent, vec!["a", "b"], "重复 put 不应该产生两条记录");
    }
}
