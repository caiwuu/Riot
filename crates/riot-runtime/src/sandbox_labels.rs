//! Low 完整性标签的目录清单。
//!
//! Windows 沙箱靠给可写目录打 Low 标签放行写（见 docs/SANDBOX_WINDOWS.md
//! §2）。标签是**持久的文件系统状态** —— 进程崩溃后不会自己消失，所以
//! 打了哪些目录必须落盘，下次启动才能把残留收干净（对照
//! `process_lifecycle` 的哲学：无论怎么死，别往机器上漏东西）。
//!
//! 这个文件是**跨平台的纯逻辑**：清单的数据结构、持久化、孤儿识别。
//! 实际的打标签 / 去标签是 Win32 调用，在 [`crate::sandbox_win`]，只在
//! Windows 上编译。分开是为了让清单逻辑能在任何平台上单元测试 —— 它是
//! 孤儿回收正确性的所在，而那条正确性和 OS 无关。
//!
//! `[取舍]` 清单只记「路径 + 打标时间」，**不记原标签**。因为只对
//! 「当前是默认完整性（无显式 label / Medium）」的目录打 Low 标签，
//! 回滚就是删掉那条 Low ACE、回到默认 —— 没有"原状"要保存。本来就带
//! 非默认 label 的目录（罕见）在打标签阶段检测到就跳过并降级激活
//! （见 sandbox_win）。这把"记录原状"简化成了"记录我动过谁"。

#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 清单里的一条：一个被打了 Low 标签的目录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelRecord {
    pub path: PathBuf,
    /// 打标时间（纪元毫秒）。诊断用 —— 一堆很旧的残留说明有会话没干净退出。
    pub labeled_at_ms: u64,
}

/// 打了 Low 标签的目录清单，落盘一份。
///
/// 生命周期：激活时 [`record`](Self::record) 每个可写目录；正常退出
/// [`forget`](Self::forget) 掉（去标签成功后）；启动时 [`load`](Self::load)
/// 读到的每条都是**上次没清干净的残留**，交给回收例程逐个去标签。
pub struct LabelLedger {
    path: PathBuf,
    entries: Vec<LabelRecord>,
}

impl LabelLedger {
    /// 读清单。文件不存在 = 空清单（没装过沙箱，正常）。
    ///
    /// `[约束]` 清单损坏（JSON 解析失败）也返回空清单 + 告警，**不报错**。
    /// 坏清单卡死启动是最糟的失败模式 —— 沙箱是可选增强，它的元数据坏了
    /// 不该让整个应用起不来。代价是漏收几个残留标签，而残留标签的危害
    /// 本就很小（见 §2）。
    pub fn load(path: PathBuf) -> Self {
        let entries = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Vec<LabelRecord>>(&bytes) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "沙箱标签清单损坏，按空清单处理");
                    Vec::new()
                }
            },
            Err(_) => Vec::new(), // 不存在 = 从没打过标签
        };
        Self { path, entries }
    }

    /// 记一个目录并落盘。幂等：已记过的不重复加，但刷新时间戳。
    ///
    /// `[约束]` 先落盘**再**返回 —— 打标签这个副作用发生在调用方那侧，
    /// 顺序必须是"先记录意图、再执行副作用"。反过来的话，记录前崩溃会
    /// 留下一个打了标签却不在清单里的目录，回收永远收不到它。
    pub fn record(&mut self, dir: &Path, now_ms: u64) -> std::io::Result<()> {
        match self.entries.iter_mut().find(|e| e.path == dir) {
            Some(e) => e.labeled_at_ms = now_ms,
            None => self.entries.push(LabelRecord {
                path: dir.to_path_buf(),
                labeled_at_ms: now_ms,
            }),
        }
        self.flush()
    }

    /// 去掉一个目录并落盘。去标签成功后调 —— 不在清单里的静默忽略。
    pub fn forget(&mut self, dir: &Path) -> std::io::Result<()> {
        let before = self.entries.len();
        self.entries.retain(|e| e.path != dir);
        if self.entries.len() == before {
            return Ok(()); // 没这条，不用写盘
        }
        self.flush()
    }

    /// 当前记录的所有目录。启动后立即调 = 上次残留的孤儿清单。
    pub fn dirs(&self) -> Vec<PathBuf> {
        self.entries.iter().map(|e| e.path.clone()).collect()
    }

    /// 原子落盘：写临时文件再 rename。
    ///
    /// `[约束]` 不能直接覆盖写目标文件。写到一半崩溃会留下一个截断的
    /// JSON —— 下次 load 解析失败，虽然容错成空清单，但那等于把整份
    /// 残留记录一次性丢了，全变成收不到的孤儿。rename 在同目录是原子的：
    /// 要么旧清单、要么新清单，没有中间态。
    fn flush(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(&self.entries)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger_path() -> (tempfile::TempDir, PathBuf) {
        let t = tempfile::tempdir().expect("临时目录");
        let p = t.path().join("sandbox-labels.json");
        (t, p)
    }

    #[test]
    fn 记录往返() {
        let (_t, p) = ledger_path();
        let mut led = LabelLedger::load(p.clone());
        led.record(Path::new("/work/a"), 100).expect("记 a");
        led.record(Path::new("/work/b"), 200).expect("记 b");

        // 重新 load —— 模拟下次启动读到的残留。
        let reloaded = LabelLedger::load(p);
        let mut dirs = reloaded.dirs();
        dirs.sort();
        assert_eq!(dirs, vec![PathBuf::from("/work/a"), PathBuf::from("/work/b")]);
    }

    #[test]
    fn 记录幂等_不重复加() {
        let (_t, p) = ledger_path();
        let mut led = LabelLedger::load(p);
        led.record(Path::new("/work/a"), 100).expect("记");
        led.record(Path::new("/work/a"), 300).expect("再记");
        assert_eq!(led.dirs(), vec![PathBuf::from("/work/a")], "同一目录只该一条");
    }

    #[test]
    fn 去标签后清单里消失() {
        let (_t, p) = ledger_path();
        let mut led = LabelLedger::load(p.clone());
        led.record(Path::new("/work/a"), 100).expect("记");
        led.record(Path::new("/work/b"), 100).expect("记");
        led.forget(Path::new("/work/a")).expect("去 a");

        assert_eq!(LabelLedger::load(p).dirs(), vec![PathBuf::from("/work/b")]);
    }

    #[test]
    fn 不存在的清单是空的不报错() {
        let (_t, p) = ledger_path();
        assert!(LabelLedger::load(p).dirs().is_empty());
    }

    /// 崩溃残留：手写一份清单，模拟上次没干净退出，启动时要能读回来回收。
    #[test]
    fn 上次残留被读成孤儿清单() {
        let (_t, p) = ledger_path();
        std::fs::write(
            &p,
            r#"[{"path":"/work/left","labeled_at_ms":42}]"#,
        )
        .expect("写残留清单");

        let led = LabelLedger::load(p);
        assert_eq!(
            led.dirs(),
            vec![PathBuf::from("/work/left")],
            "残留目录必须被读出来交给回收"
        );
    }

    /// 损坏的清单不能卡死启动 —— 按空处理，最多漏收几个残留。
    #[test]
    fn 损坏的清单按空处理() {
        let (_t, p) = ledger_path();
        std::fs::write(&p, "{ 这不是合法 JSON").expect("写坏清单");
        assert!(
            LabelLedger::load(p).dirs().is_empty(),
            "坏清单该降级成空，不该 panic 或报错"
        );
    }

    /// 写到一半崩溃不该毁掉已有清单 —— 原子 rename 保证要么旧要么新。
    /// 这里验正常路径的原子性副产物：临时文件用完不残留。
    #[test]
    fn 落盘不留临时文件() {
        let (_t, p) = ledger_path();
        let mut led = LabelLedger::load(p.clone());
        led.record(Path::new("/work/a"), 100).expect("记");
        assert!(!p.with_extension("json.tmp").exists(), "临时文件该被 rename 掉");
        assert!(p.exists(), "目标清单该在");
    }
}
