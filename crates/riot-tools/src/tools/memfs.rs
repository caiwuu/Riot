//! 内存文件系统与文件状态缓存，供工具测试使用。
//!
//! 真实文件系统在测试里有两个问题:mtime 精度不可控(某些平台只有秒级),
//! symlink 在 Windows 上需要特权。用内存实现两个都绕开,而且能精确构造
//! "在两次读取之间文件变了"这种 TOCTOU 场景。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use riot_protocol::tool::{FileMeta, FileState, FileStateCache, FileSystem};

#[derive(Default)]
pub struct MemFs {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    files: HashMap<PathBuf, (Vec<u8>, u64)>,
    dirs: Vec<PathBuf>,
    /// canonicalize 时的替换映射，用来模拟 symlink。
    links: HashMap<PathBuf, PathBuf>,
}

impl MemFs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_file(self, path: impl Into<PathBuf>, content: impl AsRef<[u8]>) -> Self {
        self.put(path, content, 1000);
        self
    }

    pub fn with_dir(self, path: impl Into<PathBuf>) -> Self {
        self.inner.lock().expect("锁未中毒").dirs.push(path.into());
        self
    }

    /// 模拟 symlink：`canonicalize(from)` 返回 `to`。
    pub fn with_link(self, from: impl Into<PathBuf>, to: impl Into<PathBuf>) -> Self {
        self.inner
            .lock()
            .expect("锁未中毒")
            .links
            .insert(from.into(), to.into());
        self
    }

    pub fn put(&self, path: impl Into<PathBuf>, content: impl AsRef<[u8]>, mtime_ms: u64) {
        self.inner
            .lock()
            .expect("锁未中毒")
            .files
            .insert(path.into(), (content.as_ref().to_vec(), mtime_ms));
    }

    /// 当前的 mtime。用来构造"内容变了但 mtime 没变"的场景 ——
    /// HFS+ 和部分 NFS 的 mtime 精度只有 1 秒。
    pub fn metadata_mtime(&self, path: impl AsRef<Path>) -> u64 {
        self.inner
            .lock()
            .expect("锁未中毒")
            .files
            .get(path.as_ref())
            .map(|(_, m)| *m)
            .unwrap_or(0)
    }

    pub fn content(&self, path: impl AsRef<Path>) -> Option<Vec<u8>> {
        self.inner
            .lock()
            .expect("锁未中毒")
            .files
            .get(path.as_ref())
            .map(|(c, _)| c.clone())
    }

    pub fn text(&self, path: impl AsRef<Path>) -> Option<String> {
        self.content(path)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    }
}

fn not_found(path: &Path) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("{} 不存在", path.display()),
    )
}

#[async_trait]
impl FileSystem for MemFs {
    async fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        let g = self.inner.lock().expect("锁未中毒");
        let Some((content, _)) = g.files.get(path) else {
            return Err(not_found(path));
        };
        Ok(content.clone())
    }

    async fn write(&self, path: &Path, data: &[u8]) -> std::io::Result<()> {
        let mut g = self.inner.lock().expect("锁未中毒");

        // 父目录必须存在，和真实文件系统一致
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
            && !g.dirs.iter().any(|d| d == parent)
            && !g.files.keys().any(|f| f.parent() == Some(parent))
        {
            return Err(not_found(parent));
        }

        let mtime = g.files.get(path).map(|(_, m)| m + 1).unwrap_or(1000);
        g.files.insert(path.to_path_buf(), (data.to_vec(), mtime));
        Ok(())
    }

    async fn metadata(&self, path: &Path) -> std::io::Result<FileMeta> {
        let g = self.inner.lock().expect("锁未中毒");

        if g.dirs.iter().any(|d| d == path) {
            return Ok(FileMeta {
                mtime_ms: 0,
                len: 0,
                is_dir: true,
            });
        }

        let Some((content, mtime_ms)) = g.files.get(path) else {
            return Err(not_found(path));
        };

        Ok(FileMeta {
            mtime_ms: *mtime_ms,
            len: content.len() as u64,
            is_dir: false,
        })
    }

    async fn read_dir(&self, path: &Path) -> std::io::Result<Vec<PathBuf>> {
        let g = self.inner.lock().expect("锁未中毒");
        Ok(g.files
            .keys()
            .filter(|f| f.parent() == Some(path))
            .cloned()
            .collect())
    }

    async fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        let g = self.inner.lock().expect("锁未中毒");

        if let Some(target) = g.links.get(path) {
            return Ok(target.clone());
        }
        if g.files.contains_key(path) || g.dirs.iter().any(|d| d == path) {
            return Ok(path.to_path_buf());
        }
        Err(not_found(path))
    }
}

/// 内存版 [`FileStateCache`]。
#[derive(Default)]
pub struct MemFileState {
    inner: Mutex<Vec<(PathBuf, FileState)>>,
}

impl MemFileState {
    pub fn new() -> Self {
        Self::default()
    }
}

impl FileStateCache for MemFileState {
    fn get(&self, path: &Path) -> Option<FileState> {
        self.inner
            .lock()
            .expect("锁未中毒")
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, s)| s.clone())
    }

    fn put(&self, path: PathBuf, state: FileState) {
        let mut g = self.inner.lock().expect("锁未中毒");
        g.retain(|(p, _)| p != &path);
        g.insert(0, (path, state));
    }

    fn invalidate(&self, path: &Path) {
        self.inner
            .lock()
            .expect("锁未中毒")
            .retain(|(p, _)| p != path);
    }

    fn recent(&self, limit: usize) -> Vec<(PathBuf, FileState)> {
        self.inner
            .lock()
            .expect("锁未中毒")
            .iter()
            .take(limit)
            .cloned()
            .collect()
    }
}
