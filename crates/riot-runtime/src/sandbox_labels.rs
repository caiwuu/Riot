//! Low 完整性标签的目录清单与引用计数。
//!
//! Windows 沙箱靠给可写目录打 Low 标签放行写（见 docs/SANDBOX_WINDOWS.md
//! §2）。标签是**持久的文件系统状态** —— 进程崩溃后不会自己消失，所以
//! 打了哪些目录必须落盘，下次启动才能把残留收干净（对照
//! `process_lifecycle` 的哲学：无论怎么死，别往机器上漏东西）。
//!
//! 这个文件是**跨平台的纯逻辑**：清单的数据结构、持久化、引用计数编排、
//! 孤儿回收。实际的打标签 / 去标签是 Win32 调用，在 [`crate::sandbox_win`]，
//! 只在 Windows 上编译。分开是为了让这套正确性（回滚干不干净、计数平不平、
//! 清单丢不丢记录）能在任何平台上单元测试 —— 它们和 OS 无关。
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
        // 临时名带 pid：清单是**全机一份**的，同机双开内核时两个进程会各自
        // flush。共用一个固定的 `.json.tmp` 会让两次「写临时文件 → rename」
        // 交错，rename 出去的可能是半份别人的内容 —— 而原子 rename 的全部
        // 意义就是避免这个。回收有独占锁挡双开，落盘没有。
        let tmp = self
            .path
            .with_extension(format!("json.tmp{}", std::process::id()));
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

/// 进程级的标签引用计数注册表。
///
/// 为什么需要它，而不是每次激活各自打/撤标签：多会话共享一个内核进程
/// （ARCHITECTURE §2.4），沙箱是每轮对话激活一次的，而 writable 里有
/// **跨会话共享**的目录（同项目的工作区、`~/.cargo` 这类构建缓存）。
/// 各自为政的话，会话 A 一轮结束就把 B 正在用的目录撤了标签 —— B 的
/// Low 进程构建到一半突然写不进（MIC no-write-up），表现是"编译莫名
/// 其妙失败"，正是设计文档最怕的那种沙箱失败模式。计数归零才真撤。
///
/// 它同时是清单的**单写者**：所有打标/撤标/记账都在同一把锁里做。
/// 之前每个激活点独立 load 全量、flush 全量覆盖，两个会话并发时后写
/// 的把先写的记录整个盖掉（lost update）—— 清单的意义是崩溃后回收
/// 孤儿标签，丢一条记录 = 崩溃后那个目录永远对全机 Low 进程可写。
///
/// `[约束]` 跨**进程**它管不了（计数在内存里）。同机双开内核时靠
/// 回收例程的独占锁避免互踩（见 `sandbox_win::recover_orphans`），
/// 双开下的激活/释放互踩是接受的残余风险 —— 正常部署一宿主一内核。
pub struct LabelRegistry {
    /// 目录 → 活跃引用数。BTreeMap 而不是 HashMap：静态实例要求
    /// `new` 是 const fn，`HashMap::new` 不是。
    counts: std::sync::Mutex<std::collections::BTreeMap<PathBuf, usize>>,
}

impl LabelRegistry {
    pub const fn new() -> Self {
        Self {
            counts: std::sync::Mutex::new(std::collections::BTreeMap::new()),
        }
    }

    /// 锁中毒就接着用：注册表状态只有"计数"，持锁代码不含会 panic 的
    /// 不变量破坏点，卡死沙箱激活比带毒继续更糟。
    fn lock(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::BTreeMap<PathBuf, usize>> {
        self.counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 给一组可写目录各 +1 引用；**首个引用**才真打标签并记账。
    /// **任一失败就把本次已加的引用全部退回**（本次真打的标签跟着撤），
    /// 然后整体失败让 `activate()` 返回 None —— 全有或全无，"本次激活
    /// 失败"不该依赖"下次启动清理"来收拾自己的烂摊子。
    ///
    /// `[约束]` 顺序是**先记账、再打标签**（`record` 的文档一直这么要求）：
    /// 两步之间崩溃留下的是"记了账、没打成"，回收时空撤一次，无害；
    /// 反过来是"打了标签、没记账"，回收永远找不到它。
    pub fn acquire<L: DirLabeler>(
        &self,
        dirs: &[PathBuf],
        labeler: &L,
        ledger_path: &Path,
        now_ms: u64,
    ) -> std::io::Result<()> {
        let mut counts = self.lock();
        let mut ledger = LabelLedger::load(ledger_path.to_path_buf());
        let mut done = 0usize;
        for dir in dirs {
            let first_ref = counts.get(dir).copied().unwrap_or(0) == 0;
            let result = if first_ref {
                // 上次崩溃可能留了这条账（标签还挂在目录上）。失败回滚时
                // 不能把别人的旧账销掉 —— 只销本次新记的。
                let had_record = ledger.dirs().iter().any(|d| d == dir);
                match ledger.record(dir, now_ms).and_then(|()| labeler.tag(dir)) {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        if !had_record {
                            let _ = ledger.forget(dir);
                        }
                        Err(e)
                    }
                }
            } else {
                Ok(()) // 已有活跃引用 = 标签已在，只加计数。
            };
            match result {
                Ok(()) => {
                    *counts.entry(dir.clone()).or_insert(0) += 1;
                    done += 1;
                }
                Err(e) => {
                    // 退回本次已加的引用（逆序，纯粹是对称好读）。归零的
                    // 会被 release_one 真撤；别的会话还在用的只减计数。
                    for d in dirs[..done].iter().rev() {
                        Self::release_one(&mut counts, d, labeler, &mut ledger);
                    }
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// 给一组目录各 -1 引用；归零才撤标签并销账。
    ///
    /// 撤标签失败时**账保留**（不 forget）：孤儿回收按清单重试，销了账
    /// 它就成了永远带 Low 标签、又没人记得的目录。这也是为什么这个函数
    /// 不返回错误 —— 失败的善后已经安排好了，调用方（Drop）也没法处理。
    pub fn release<L: DirLabeler>(&self, dirs: &[PathBuf], labeler: &L, ledger_path: &Path) {
        let mut counts = self.lock();
        let mut ledger = LabelLedger::load(ledger_path.to_path_buf());
        for dir in dirs {
            Self::release_one(&mut counts, dir, labeler, &mut ledger);
        }
    }

    fn release_one<L: DirLabeler>(
        counts: &mut std::collections::BTreeMap<PathBuf, usize>,
        dir: &Path,
        labeler: &L,
        ledger: &mut LabelLedger,
    ) {
        let Some(n) = counts.get_mut(dir) else {
            // release 多于 acquire 是调用方 bug，但撤一个不属于自己的
            // 标签比记一条日志严重得多 —— 只警告，不动标签。
            tracing::warn!(dir = %dir.display(), "释放了未登记的标签引用（acquire/release 不配对）");
            return;
        };
        *n -= 1;
        if *n > 0 {
            return; // 还有别的会话在用，标签留着。
        }
        counts.remove(dir);
        match labeler.untag(dir) {
            Ok(()) => {
                let _ = ledger.forget(dir);
            }
            Err(e) => {
                tracing::warn!(dir = %dir.display(), error = %e, "撤标签失败，账保留给下次启动的孤儿回收");
            }
        }
    }
}

impl Default for LabelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// 启动时的孤儿回收：清单里的每一条都是上次进程没干净退出留下的残留
/// Low 标签，逐个撤掉、销账。
///
/// `[约束]` 必须在**任何会话激活之前**调 —— 此刻本进程没有活跃引用，
/// 撤是安全的。跨进程的互斥（同机双开内核）由调用方负责（见
/// `sandbox_win::recover_orphans` 的独占锁）。
///
/// 目录已经不存在的直接销账（标签随目录一起没了）；撤失败的留账，
/// 下次启动再试。
pub fn recover_orphans<L: DirLabeler>(labeler: &L, ledger_path: &Path) {
    let mut ledger = LabelLedger::load(ledger_path.to_path_buf());
    let dirs = ledger.dirs();
    if dirs.is_empty() {
        return;
    }
    tracing::info!(count = dirs.len(), "回收上次残留的沙箱 Low 标签");
    for dir in dirs {
        if !dir.exists() {
            let _ = ledger.forget(&dir);
            continue;
        }
        match labeler.untag(&dir) {
            Ok(()) => {
                let _ = ledger.forget(&dir);
            }
            Err(e) => {
                tracing::warn!(dir = %dir.display(), error = %e, "回收残留标签失败，账保留下次再试");
            }
        }
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

    /// 假 labeler：记下每次 tag / untag，可指定「第几次 tag 失败」和
    /// 「untag 是否也失败」，用来验编排的回滚。Mutex 而不是 RefCell：
    /// 并发测试要跨线程共享它。
    struct FakeLabeler {
        tag_calls: std::sync::Mutex<Vec<PathBuf>>,
        untag_calls: std::sync::Mutex<Vec<PathBuf>>,
        /// tag 到第几个（1-based）返回失败；0 = 从不失败。
        fail_tag_at: usize,
        /// untag 也失败（测回滚里的二次失败不 panic）。
        untag_fails: bool,
    }

    impl FakeLabeler {
        fn new(fail_tag_at: usize) -> Self {
            Self {
                tag_calls: std::sync::Mutex::new(Vec::new()),
                untag_calls: std::sync::Mutex::new(Vec::new()),
                fail_tag_at,
                untag_fails: false,
            }
        }
        fn tags(&self) -> Vec<PathBuf> {
            self.tag_calls.lock().expect("锁").clone()
        }
        fn untags(&self) -> Vec<PathBuf> {
            self.untag_calls.lock().expect("锁").clone()
        }
    }

    impl DirLabeler for FakeLabeler {
        fn tag(&self, dir: &Path) -> std::io::Result<()> {
            let mut calls = self.tag_calls.lock().expect("锁");
            calls.push(dir.to_path_buf());
            if calls.len() == self.fail_tag_at {
                return Err(std::io::Error::other("打标签失败（模拟 FAT32/组策略）"));
            }
            Ok(())
        }
        fn untag(&self, dir: &Path) -> std::io::Result<()> {
            self.untag_calls.lock().expect("锁").push(dir.to_path_buf());
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
        let reg = LabelRegistry::new();
        let lab = FakeLabeler::new(0);
        let ds = dirs(3);

        reg.acquire(&ds, &lab, &p, 100).expect("该全成功");

        assert_eq!(lab.tags().len(), 3);
        assert!(lab.untags().is_empty(), "成功路径不该回滚");
        let mut recorded = LabelLedger::load(p).dirs();
        recorded.sort();
        assert_eq!(recorded, ds);
    }

    /// 打到第 3 个失败：前 2 个已打的必须被 untag，清单必须清空。
    #[test]
    fn 中途失败把已打的全部回滚() {
        let (_t, p) = ledger_path();
        let reg = LabelRegistry::new();
        let lab = FakeLabeler::new(3);
        let ds = dirs(5);

        let r = reg.acquire(&ds, &lab, &p, 100);
        assert!(r.is_err(), "第 3 个失败该整体失败");

        // tag 试了 3 个（第 3 个失败后不再往下），untag 回滚前 2 个。
        assert_eq!(lab.tags().len(), 3);
        assert_eq!(lab.untags().len(), 2, "前两个要被撤回");
        assert!(LabelLedger::load(p).dirs().is_empty(), "清单必须清干净，不留孤儿");
    }

    /// 第一个就失败：没有已打的可回滚，清单本就空。
    #[test]
    fn 第一个就失败不留痕() {
        let (_t, p) = ledger_path();
        let reg = LabelRegistry::new();
        let lab = FakeLabeler::new(1);
        assert!(reg.acquire(&dirs(3), &lab, &p, 100).is_err());
        assert!(lab.untags().is_empty(), "没打成的不用回滚");
        assert!(LabelLedger::load(p).dirs().is_empty());
    }

    /// 回滚里 untag 二次失败：尽力而为，不 panic、不吞原始错误。
    /// 撤失败的那条**账要保留** —— 孤儿回收按它下次重试。
    #[test]
    fn 回滚失败也不panic_且账保留() {
        let (_t, p) = ledger_path();
        let reg = LabelRegistry::new();
        let mut lab = FakeLabeler::new(2);
        lab.untag_fails = true;
        let r = reg.acquire(&dirs(3), &lab, &p, 100);
        assert!(r.is_err(), "原始失败要如实返回");
        assert_eq!(lab.untags().len(), 1, "仍尝试回滚了第 1 个");
        assert_eq!(
            LabelLedger::load(p).dirs(),
            vec![PathBuf::from("/work/d0")],
            "撤不掉的那条账必须留着，否则孤儿回收找不到它"
        );
    }

    /// 共享目录的第二个引用不重复打标签、不重复记账。
    #[test]
    fn 二次引用不重复打标签() {
        let (_t, p) = ledger_path();
        let reg = LabelRegistry::new();
        let lab = FakeLabeler::new(0);
        let shared = dirs(1);

        reg.acquire(&shared, &lab, &p, 100).expect("会话 A");
        reg.acquire(&shared, &lab, &p, 200).expect("会话 B");

        assert_eq!(lab.tags().len(), 1, "标签只该打一次");
        assert_eq!(LabelLedger::load(p).dirs().len(), 1, "账也只该一条");
    }

    /// 会话 A 释放时 B 还在用：标签必须留着（1b 的回归 —— 撤了 B 的
    /// Low 进程会构建到一半写不进）。B 也释放后才真撤。
    #[test]
    fn 归零才撤标签() {
        let (_t, p) = ledger_path();
        let reg = LabelRegistry::new();
        let lab = FakeLabeler::new(0);
        let shared = dirs(1);

        reg.acquire(&shared, &lab, &p, 100).expect("会话 A");
        reg.acquire(&shared, &lab, &p, 200).expect("会话 B");

        reg.release(&shared, &lab, &p);
        assert!(lab.untags().is_empty(), "B 还在用，不许撤");
        assert_eq!(LabelLedger::load(p.clone()).dirs().len(), 1, "账也还在");

        reg.release(&shared, &lab, &p);
        assert_eq!(lab.untags().len(), 1, "归零才撤");
        assert!(LabelLedger::load(p).dirs().is_empty(), "账销掉");
    }

    /// 中途失败的回滚只退**本次**加的引用 —— 不能把别的会话正用着的
    /// 标签撤掉。
    #[test]
    fn 失败回滚不踩别人的引用() {
        let (_t, p) = ledger_path();
        let reg = LabelRegistry::new();
        // 第 2 次 tag 失败：A 打 shared（第 1 次），B 打 bad（第 2 次）。
        let lab = FakeLabeler::new(2);
        let shared = PathBuf::from("/work/shared");
        let bad = PathBuf::from("/work/bad");

        reg.acquire(std::slice::from_ref(&shared), &lab, &p, 100)
            .expect("会话 A");
        let r = reg.acquire(&[shared.clone(), bad.clone()], &lab, &p, 200);
        assert!(r.is_err(), "bad 打不上，B 整体失败");

        assert!(lab.untags().is_empty(), "shared 还有 A 的引用，回滚不许撤它");
        assert_eq!(
            LabelLedger::load(p.clone()).dirs(),
            vec![shared.clone()],
            "A 的账不能被 B 的回滚销掉"
        );

        // A 正常结束：这时才轮到真撤。
        reg.release(std::slice::from_ref(&shared), &lab, &p);
        assert_eq!(lab.untags(), vec![shared], "A 释放后才撤");
    }

    /// 无状态 labeler，给并发测试用（FakeLabeler 的 Mutex 也行，但这里
    /// 根本不关心调用记录）。
    struct NoopLabeler;
    impl DirLabeler for NoopLabeler {
        fn tag(&self, _dir: &Path) -> std::io::Result<()> {
            Ok(())
        }
        fn untag(&self, _dir: &Path) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// 1a 的回归：两个"会话"并发 acquire 不同目录，清单不许互相盖掉。
    /// 修之前每个激活点各持一份全量内存清单、flush 全量覆盖 —— 并发时
    /// 后写的把先写的记录抹掉。注册表把清单收成单写者后不该再发生。
    #[test]
    fn 并发acquire不丢清单记录() {
        let (_t, p) = ledger_path();
        let reg = std::sync::Arc::new(LabelRegistry::new());

        let handles: Vec<_> = (0..4)
            .map(|i| {
                let reg = std::sync::Arc::clone(&reg);
                let p = p.clone();
                std::thread::spawn(move || {
                    let ds: Vec<PathBuf> =
                        (0..3).map(|j| PathBuf::from(format!("/w/s{i}-d{j}"))).collect();
                    reg.acquire(&ds, &NoopLabeler, &p, 100).expect("并发授权");
                })
            })
            .collect();
        for h in handles {
            h.join().expect("线程正常结束");
        }

        assert_eq!(
            LabelLedger::load(p).dirs().len(),
            12,
            "4 个并发会话 × 3 目录，一条都不许丢"
        );
    }

    /// 孤儿回收：清单里的残留被逐个撤掉并销账。
    #[test]
    fn 孤儿回收撤标签并销账() {
        let t = tempfile::tempdir().expect("临时目录");
        let p = t.path().join("sandbox-labels.json");
        // 残留指向真实存在的目录（exists() 检查要过）。
        let d1 = t.path().join("left1");
        let d2 = t.path().join("left2");
        std::fs::create_dir_all(&d1).expect("建 d1");
        std::fs::create_dir_all(&d2).expect("建 d2");
        let mut led = LabelLedger::load(p.clone());
        led.record(&d1, 42).expect("残留 1");
        led.record(&d2, 43).expect("残留 2");
        drop(led);

        let lab = FakeLabeler::new(0);
        recover_orphans(&lab, &p);

        assert_eq!(lab.untags().len(), 2, "两条残留都要撤");
        assert!(LabelLedger::load(p).dirs().is_empty(), "账销干净");
    }

    /// 目录已经没了的残留：标签随目录一起没了，直接销账、不调 untag。
    #[test]
    fn 孤儿回收_目录不存在的直接销账() {
        let (_t, p) = ledger_path();
        let mut led = LabelLedger::load(p.clone());
        led.record(Path::new("/不存在/的/目录"), 42).expect("残留");
        drop(led);

        let lab = FakeLabeler::new(0);
        recover_orphans(&lab, &p);

        assert!(lab.untags().is_empty(), "没有对象可撤");
        assert!(LabelLedger::load(p).dirs().is_empty(), "账照样销");
    }

    /// 回收失败：账保留，下次启动再试。
    #[test]
    fn 孤儿回收失败账保留() {
        let t = tempfile::tempdir().expect("临时目录");
        let p = t.path().join("sandbox-labels.json");
        let d = t.path().join("stuck");
        std::fs::create_dir_all(&d).expect("建目录");
        let mut led = LabelLedger::load(p.clone());
        led.record(&d, 42).expect("残留");
        drop(led);

        let mut lab = FakeLabeler::new(0);
        lab.untag_fails = true;
        recover_orphans(&lab, &p);

        assert_eq!(LabelLedger::load(p).dirs(), vec![d], "撤不掉就留账");
    }
}
