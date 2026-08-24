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

/// 给目录打 / 去 Low 完整性标签的能力。
///
/// 抽成 trait 是为了让**编排逻辑**（[`authorize_writable`] 的"逐个打、
/// 失败回滚"）跨平台可测 —— 真实实现是 Win32（`sandbox_win::WinLabeler`，
/// 只在 Windows 编译），测试用假的。编排的正确性（尤其是回滚干不干净）
/// 和 OS 无关，不该只能在 Windows CI 上验。
pub trait DirLabeler {
    /// 给目录打 Low 标签，让低完整性进程能写它。
    fn tag(&self, dir: &Path) -> std::io::Result<()>;
    /// 去掉 Low 标签，回到默认完整性。
    fn untag(&self, dir: &Path) -> std::io::Result<()>;
}

/// 给一组可写目录打 Low 标签并记账；**任一失败就把已打的全部回滚**。
///
/// 这是 Windows 沙箱激活序列的授权准备步（SANDBOX_WINDOWS.md §2 步骤
/// 1-2）。它必须是**全有或全无**：
///
/// `[约束]` 打到第 N 个失败（比如撞上 FAT32 卷、组策略锁 SACL），前
/// N-1 个已经打上的标签必须撤掉、清单必须清干净，然后整体失败让
/// `activate()` 返回 None。留一半的话，那些目录就成了没人认领的孤儿
/// （打了 Low 标签、却不在任何活跃会话的记录里）——虽然下次启动的孤儿
/// 回收兜得住，但"本次激活失败"不该依赖"下次启动清理"来收拾自己的烂摊子。
///
/// 回滚时 `untag` 自己也可能失败（目录刚被删等）—— 尽力而为，记日志，
/// 不因为回滚里的二次失败而 panic 或吞掉原始错误。
pub fn authorize_writable<L: DirLabeler>(
    dirs: &[PathBuf],
    labeler: &L,
    ledger: &mut LabelLedger,
    now_ms: u64,
) -> std::io::Result<()> {
    let mut done: Vec<PathBuf> = Vec::with_capacity(dirs.len());
    for dir in dirs {
        match labeler.tag(dir).and_then(|()| ledger.record(dir, now_ms)) {
            Ok(()) => done.push(dir.clone()),
            Err(e) => {
                // 回滚已打的（逆序，纯粹是对称好读，标签之间无依赖）。
                for d in done.iter().rev() {
                    if let Err(re) = labeler.untag(d) {
                        tracing::warn!(dir = %d.display(), error = %re, "回滚标签失败，留给下次启动的孤儿回收");
                    }
                    let _ = ledger.forget(d);
                }
                return Err(e);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

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

    /// 假 labeler：记下每次 tag / untag，可指定「第几次 tag 失败」和
    /// 「untag 是否也失败」，用来验编排的回滚。
    struct FakeLabeler {
        tag_calls: RefCell<Vec<PathBuf>>,
        untag_calls: RefCell<Vec<PathBuf>>,
        /// tag 到第几个（1-based）返回失败；0 = 从不失败。
        fail_tag_at: usize,
        /// untag 也失败（测回滚里的二次失败不 panic）。
        untag_fails: bool,
    }

    impl FakeLabeler {
        fn new(fail_tag_at: usize) -> Self {
            Self {
                tag_calls: RefCell::new(Vec::new()),
                untag_calls: RefCell::new(Vec::new()),
                fail_tag_at,
                untag_fails: false,
            }
        }
    }

    impl DirLabeler for FakeLabeler {
        fn tag(&self, dir: &Path) -> std::io::Result<()> {
            self.tag_calls.borrow_mut().push(dir.to_path_buf());
            if self.tag_calls.borrow().len() == self.fail_tag_at {
                return Err(std::io::Error::other("打标签失败（模拟 FAT32/组策略）"));
            }
            Ok(())
        }
        fn untag(&self, dir: &Path) -> std::io::Result<()> {
            self.untag_calls.borrow_mut().push(dir.to_path_buf());
            if self.untag_fails {
                return Err(std::io::Error::other("去标签也失败"));
            }
            Ok(())
        }
    }

    fn dirs(n: usize) -> Vec<PathBuf> {
        (0..n).map(|i| PathBuf::from(format!("/work/d{i}"))).collect()
    }

    #[test]
    fn 授权全成功_每个目录都打标签并记账() {
        let (_t, p) = ledger_path();
        let mut led = LabelLedger::load(p);
        let lab = FakeLabeler::new(0);
        let ds = dirs(3);

        authorize_writable(&ds, &lab, &mut led, 100).expect("该全成功");

        assert_eq!(lab.tag_calls.borrow().len(), 3);
        assert!(lab.untag_calls.borrow().is_empty(), "成功路径不该回滚");
        let mut recorded = led.dirs();
        recorded.sort();
        assert_eq!(recorded, ds);
    }

    /// 打到第 3 个失败：前 2 个已打的必须被 untag，清单必须清空。
    #[test]
    fn 中途失败把已打的全部回滚() {
        let (_t, p) = ledger_path();
        let mut led = LabelLedger::load(p.clone());
        let lab = FakeLabeler::new(3);
        let ds = dirs(5);

        let r = authorize_writable(&ds, &lab, &mut led, 100);
        assert!(r.is_err(), "第 3 个失败该整体失败");

        // tag 试了 3 个（第 3 个失败后不再往下），untag 回滚前 2 个。
        assert_eq!(lab.tag_calls.borrow().len(), 3);
        assert_eq!(lab.untag_calls.borrow().len(), 2, "前两个要被撤回");
        assert!(led.dirs().is_empty(), "清单必须清干净，不留孤儿");
        // 重新 load 也是空 —— 回滚的 forget 落了盘。
        assert!(LabelLedger::load(p).dirs().is_empty());
    }

    /// 第一个就失败：没有已打的可回滚，清单本就空。
    #[test]
    fn 第一个就失败不留痕() {
        let (_t, p) = ledger_path();
        let mut led = LabelLedger::load(p);
        let lab = FakeLabeler::new(1);
        assert!(authorize_writable(&dirs(3), &lab, &mut led, 100).is_err());
        assert!(lab.untag_calls.borrow().is_empty(), "没打成的不用回滚");
        assert!(led.dirs().is_empty());
    }

    /// 回滚里 untag 二次失败：尽力而为，不 panic、不吞原始错误。
    #[test]
    fn 回滚失败也不panic() {
        let (_t, p) = ledger_path();
        let mut led = LabelLedger::load(p);
        let mut lab = FakeLabeler::new(2);
        lab.untag_fails = true;
        let r = authorize_writable(&dirs(3), &lab, &mut led, 100);
        assert!(r.is_err(), "原始失败要如实返回");
        assert_eq!(lab.untag_calls.borrow().len(), 1, "仍尝试回滚了第 1 个");
    }
}
