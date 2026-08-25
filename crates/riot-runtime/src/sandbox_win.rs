//! Windows 沙箱后端：Restricted Token + Low 完整性级别。
//!
//! 设计与取舍见 docs/SANDBOX_WINDOWS.md。四块拼成整条链路：
//! [`token`]（受限 Low 令牌）、[`label`]（目录标签的 Win32 那一下）、
//! [`spawn`]（CreateProcessAsUserW 起进程）、[`activate`]（激活序列
//! 编排）。标签的引用计数与清单记账在跨平台的
//! [`crate::sandbox_labels`]，这里通过进程级 [`REGISTRY`] 使用它。

#![allow(clippy::disallowed_methods)]

/// 进程级标签注册表：跨会话共享目录的引用计数 + 清单单写者。
///
/// 必须是进程级单例 —— 多会话共享一个内核进程，每轮各激活一次沙箱，
/// 共享目录（工作区、构建缓存）的标签谁都不能单方面撤。见
/// [`crate::sandbox_labels::LabelRegistry`] 的文档。
static REGISTRY: crate::sandbox_labels::LabelRegistry =
    crate::sandbox_labels::LabelRegistry::new();

/// 已激活的 Windows 沙箱：一枚受限 Low 令牌 + 已打标签的目录清单。
///
/// 拿到它意味着授权序列（打标签 + 建令牌，见 SANDBOX_WINDOWS.md §2）
/// 全部成功。之后每次 spawn 复用这枚令牌；`Drop` 时归还标签引用
/// （对称于激活时的 `REGISTRY.acquire`）。
pub(crate) struct WinSandbox {
    token: token::OwnedToken,
    /// 激活时打了 Low 标签的目录，`Drop` 逐个撤回。含 [`session_temp`]。
    labeled: Vec<std::path::PathBuf>,
    /// 标签清单的落盘位置，回滚时同步清账。
    ledger_path: std::path::PathBuf,
    /// 本会话专属的 temp 子目录（`<%TEMP%>/riot-sbx-*`）。spawn 时把
    /// 进程的 `TMP`/`TEMP` 指到这里，`Drop` 时整个删掉 —— 不碰全局 %TEMP%。
    session_temp: std::path::PathBuf,
}

// 令牌句柄是 `*mut c_void`（HANDLE），裸指针不自动 Send/Sync，于是
// WinSandbox 也不是 —— 但 SandboxedRunner 要求 `Send + Sync`，且 run 是
// async、`&self` 要跨 await。手动放行是安全的：Windows 句柄是**内核对象**
// 的引用，进程内任意线程都能用（CreateProcessAsUserW / CloseHandle 都不
// 绑线程），我们也从不并发改这枚只读的令牌。其余字段本就 Send + Sync。
unsafe impl Send for WinSandbox {}
unsafe impl Sync for WinSandbox {}

/// 单条命令输出的内存上限，同 proc.rs 的 DEFAULT_MAX_OUTPUT。
const MAX_OUTPUT: usize = 8 * 1024 * 1024;

impl WinSandbox {
    /// 在这枚令牌下起一条命令。语义见 [`spawn::spawn_with_token`]。
    ///
    /// 起进程前把 `TMP`/`TEMP`/`TMPDIR` 指到会话专属 temp 子目录 —— 命令写
    /// 临时文件（编译器中间产物、下载缓存）落在那儿（打了 Low 标签、可写），
    /// 而不是全局 %TEMP%（没打标签，Low 进程写不进）。build_env_block 按
    /// 大小写不敏感去重，这几条会覆盖继承来的同名变量。
    ///
    /// `[约束]` `TMPDIR` 不能漏。Bash 工具在 Windows 上跑的是 Git for
    /// Windows 的 bash（见 `tools::bash::shell_program`），而 MSYS 那套
    /// **先看 `TMPDIR`**：宿主环境里只要有它，`mktemp`、编译器的中间文件
    /// 就会落回没打标签的目录，然后以 ACCESS_DENIED 收场。
    pub(crate) async fn run(
        &self,
        mut spec: riot_protocol::tool::ProcessSpec,
        cancel: tokio_util::sync::CancellationToken,
    ) -> std::io::Result<riot_protocol::tool::ProcessOutput> {
        let tmp = self.session_temp.to_string_lossy().into_owned();
        for key in ["TMP", "TEMP", "TMPDIR"] {
            spec.env.push((key.to_owned(), tmp.clone()));
        }
        spawn::spawn_with_token((*self.token.0).0 as isize, spec, MAX_OUTPUT, cancel).await
    }
}

impl Drop for WinSandbox {
    fn drop(&mut self) {
        // 归还标签引用：归零才真撤 —— 别的会话可能正用着同一个目录
        // （同项目的工作区、~/.cargo 这类共享缓存），单方面撤会让它们
        // 正在跑的 Low 进程写到一半 ACCESS_DENIED。撤失败的账保留，
        // 交给下次启动的孤儿回收（见 sandbox_labels 的 [约束]）。
        //
        // `[约束]` 这活儿不能就地做。`SetNamedSecurityInfoW` 会把可继承
        // ACE 传播到已有子对象，`~/.cargo` 那种十万文件的树能走上好几秒，
        // 后面还跟着一个 `remove_dir_all` —— 而 Drop 往往发生在 tokio 的
        // 工作线程上（会话被回收时），同步做就是把整个 runtime 堵在那儿。
        // 挪到 spawn_blocking。
        let labeled = std::mem::take(&mut self.labeled);
        let ledger = std::mem::take(&mut self.ledger_path);
        let temp = std::mem::take(&mut self.session_temp);
        let cleanup = move || {
            REGISTRY.release(&labeled, &label::WinLabeler::standard(), &ledger);
            // 会话 temp 是我们建的，整个删掉 —— 里面是本会话命令的临时产物。
            let _ = std::fs::remove_dir_all(&temp);
        };
        match tokio::runtime::Handle::try_current() {
            // 进程正在退出时这个任务可能压根跑不起来 —— 那就正好落进清单
            // 的设计意图里：残留标签由下次启动的孤儿回收兜底。
            Ok(rt) => drop(rt.spawn_blocking(cleanup)),
            Err(_) => cleanup(),
        }
    }
}

/// 尝试激活 Windows 沙箱：给可写目录打 Low 标签、建受限令牌。
///
/// `None` = 这台机器上做不到（打标签失败：非 NTFS / 组策略锁 / 权限；
/// 或建令牌失败）。任一环失败都把已打的标签回滚干净再返回 —— 决策链
/// 据 `sandboxed` 放宽，绝不能在"其实没边界"时谎报成 true。
pub(crate) fn activate(
    policy: &crate::sandbox::SandboxPolicy,
    ledger_path: std::path::PathBuf,
    now_ms: u64,
) -> Option<WinSandbox> {
    use crate::sandbox::SandboxPolicy;

    let SandboxPolicy::WorkspaceWrite {
        writable,
        allow_network,
    } = policy
    else {
        return None; // Off 不该走到这里
    };

    // `[约束]` NoNet 档（allow_network == false）在 Windows V1 诚实降级：
    // Low IL / MIC 只管文件系统，不隔离网络，硬装成"断网了"就是假隔离。
    // 断网的现实手段（WFP / 防火墙 / AppContainer）都太重（见 §4）。
    // 返回 None → 决策链回到逐条询问，慢但不撒谎。
    if !allow_network {
        tracing::warn!("WorkspaceWriteNoNet 在 Windows 暂不隔离网络，本轮不激活沙箱（逐条询问）");
        return None;
    }

    // 会话专属 temp 子目录：建在全局 %TEMP% 下，但只给**它**打标签，
    // 不碰全局。spawn 时进程的 TMP/TEMP 指到这里（见 WinSandbox::run）。
    let session_temp = match make_session_temp() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "建会话 temp 子目录失败，本轮不隔离");
            return None;
        }
    };

    // 要打标签的目录 = 配置里的可写目录 + 会话 temp。
    let mut dirs = writable.clone();
    dirs.push(session_temp.clone());

    if let Err(e) = REGISTRY.acquire(&dirs, &label::WinLabeler::standard(), &ledger_path, now_ms) {
        // acquire 内部已退回本次的引用；temp 子目录还没进标签体系，删掉。
        tracing::warn!(error = %e, "沙箱打标签失败，本轮不隔离");
        let _ = std::fs::remove_dir_all(&session_temp);
        return None;
    }

    let token = match token::create_restricted_low_il() {
        Ok(t) => t,
        Err(e) => {
            // 标签已授权但令牌没建成 —— 归还引用（归零的会被真撤）。
            tracing::warn!(error = %e, "沙箱建令牌失败，回滚标签、本轮不隔离");
            REGISTRY.release(&dirs, &label::WinLabeler::standard(), &ledger_path);
            let _ = std::fs::remove_dir_all(&session_temp);
            return None;
        }
    };

    Some(WinSandbox {
        token,
        labeled: dirs,
        ledger_path,
        session_temp,
    })
}

/// 建一个会话专属的 temp 子目录 `<%TEMP%>/riot-sbx-<pid>-<纳秒>`。
///
/// 名字带 pid + 纳秒，避免同机多会话/多次激活撞名。建失败（磁盘满、
/// %TEMP% 不可写）由调用方降级成不隔离。
fn make_session_temp() -> std::io::Result<std::path::PathBuf> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("riot-sbx-{}-{}", std::process::id(), nonce));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// 启动时回收上次进程残留的孤儿标签。**必须在任何会话激活之前调。**
///
/// `[约束]` 只在独占拿到 `<清单>.lock` 时才动手：拿不到 = 同机还有另
/// 一个内核进程活着（双开），它的会话可能正引用着清单里的目录 ——
/// 此时批量撤标签等于踩它正在跑的构建。锁柄故意不关（`mem::forget`），
/// 独占随本进程存亡，后来的进程据此让路。进程内的并发由 [`REGISTRY`]
/// 管，这把锁只管跨进程的回收互斥。
pub(crate) fn recover_orphans(ledger_path: &std::path::Path) {
    use std::os::windows::fs::OpenOptionsExt;

    let lock_path = ledger_path.with_extension("lock");
    if let Some(parent) = lock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false) // 内容无关紧要，文件只当独占句柄用
        .share_mode(0) // 拒绝一切共享：第二个进程 open 直接失败
        .open(&lock_path)
    {
        Ok(f) => {
            std::mem::forget(f);
            crate::sandbox_labels::recover_orphans(&label::WinLabeler::standard(), ledger_path);
        }
        Err(e) => {
            tracing::info!(error = %e, "沙箱标签清单被另一个内核进程独占，跳过孤儿回收");
        }
    }
}

// 下面是 M1 的实质产物：受限 Low IL 令牌。M2 的 spawn 会用到它，
// 当前只有单元测试引用，故非测试构建标记 dead_code。
#[cfg(windows)]
#[allow(dead_code)]
mod token {
    use std::ffi::c_void;

    use windows::Win32::Foundation::{HANDLE, HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::ConvertStringSidToSidW;
    use windows::Win32::Security::{
        CreateRestrictedToken, DISABLE_MAX_PRIVILEGE, GetLengthSid, GetSidSubAuthority,
        GetSidSubAuthorityCount, GetTokenInformation, PSID, SID_AND_ATTRIBUTES, SetTokenInformation,
        TOKEN_ACCESS_MASK, TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE,
        TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenIntegrityLevel,
    };
    use windows::Win32::System::SystemServices::SE_GROUP_INTEGRITY;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::core::{Owned, w};

    /// 拥有所有权的令牌句柄。`Owned<HANDLE>` 负责在 drop 时 CloseHandle。
    pub struct OwnedToken(pub Owned<HANDLE>);

    /// 造一个"去特权 + Low 完整性"的主令牌，从当前进程令牌派生。
    ///
    /// 两步：
    /// 1. `CreateRestrictedToken` 带 `DISABLE_MAX_PRIVILEGE` —— 抹掉令牌
    ///    里的全部特权（SeDebugPrivilege 之类），受限令牌拿不到它们；
    /// 2. `SetTokenInformation(TokenIntegrityLevel)` 把完整性级别设成
    ///    Low（SID `S-1-16-4096`）—— MIC 的 no-write-up 规则据此挡写。
    pub fn create_restricted_low_il() -> windows::core::Result<OwnedToken> {
        unsafe {
            // 当前进程令牌。要 DUPLICATE 才能派生受限令牌，ASSIGN_PRIMARY
            // 让派生出的令牌能用于 CreateProcessAsUser（M2），
            // ADJUST_DEFAULT 才能改完整性级别。
            let mut current = HANDLE::default();
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_ACCESS_MASK(
                    TOKEN_DUPLICATE.0
                        | TOKEN_ASSIGN_PRIMARY.0
                        | TOKEN_QUERY.0
                        | TOKEN_ADJUST_DEFAULT.0,
                ),
                &mut current,
            )?;
            // 立即接管所有权，后面任何 `?` 早退都能正确关闭。
            let current = Owned::new(current);

            let mut restricted = HANDLE::default();
            CreateRestrictedToken(
                *current,
                DISABLE_MAX_PRIVILEGE,
                None, // 不额外禁用 SID
                None, // 不额外删特权（DISABLE_MAX_PRIVILEGE 已抹掉全部）
                None, // 不加受限 SID
                &mut restricted,
            )?;
            let restricted = Owned::new(restricted);

            set_low_integrity(*restricted)?;
            Ok(OwnedToken(restricted))
        }
    }

    /// 把令牌的完整性级别设成 Low。
    ///
    /// Low label 的 SID 是固定的 `S-1-16-4096`（4096 = 0x1000 =
    /// SECURITY_MANDATORY_LOW_RID）。用字符串 SID 转换而不是手搓
    /// `SID` 结构：少一层 subauthority 数组的内存摆布，读起来也直白。
    fn set_low_integrity(token: HANDLE) -> windows::core::Result<()> {
        unsafe {
            let mut psid = PSID::default();
            // w!() 是编译期 UTF-16 字面量，省一次运行时转换。
            ConvertStringSidToSidW(w!("S-1-16-4096"), &mut psid)?;
            // ConvertStringSidToSidW 分配的 SID 由 LocalFree 释放；
            // 用完即放，SetTokenInformation 会把内容拷进令牌。
            let _guard = LocalSid(psid);

            let mut label = TOKEN_MANDATORY_LABEL {
                Label: SID_AND_ATTRIBUTES {
                    Sid: psid,
                    // SE_GROUP_INTEGRITY 在 windows crate 里是 i32 常量，
                    // 而 Attributes 字段是 u32 —— 显式转。
                    Attributes: SE_GROUP_INTEGRITY as u32,
                },
            };
            let size = std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32 + GetLengthSid(psid);
            SetTokenInformation(
                token,
                TokenIntegrityLevel,
                &mut label as *mut _ as *const c_void,
                size,
            )
        }
    }

    /// 读回令牌的完整性级别 RID。测试用它断言 Low（0x1000）。
    pub fn integrity_rid(token: HANDLE) -> windows::core::Result<u32> {
        unsafe {
            // 先问长度。TokenIntegrityLevel 返回一个变长的
            // TOKEN_MANDATORY_LABEL（尾随 SID），一次拿不到大小。
            let mut needed = 0u32;
            let _ = GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut needed);
            let mut buf = vec![0u8; needed as usize];
            GetTokenInformation(
                token,
                TokenIntegrityLevel,
                Some(buf.as_mut_ptr() as *mut c_void),
                needed,
                &mut needed,
            )?;
            let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
            // GetSidSubAuthorityCount 返回指向计数字节的指针；最后一个
            // subauthority 才是完整性级别 RID。
            let count = *GetSidSubAuthorityCount(label.Label.Sid);
            let last = u32::from(count.saturating_sub(1));
            let rid = GetSidSubAuthority(label.Label.Sid, last);
            Ok(*rid)
        }
    }

    /// LocalFree 守卫：ConvertStringSidToSidW 用 LocalAlloc 分配 SID。
    struct LocalSid(PSID);
    impl Drop for LocalSid {
        fn drop(&mut self) {
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.0.0)));
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// 造出来的令牌完整性级别必须是 Low（0x1000）。
        ///
        /// 这是 M1 的验收点：令牌这一环在 Windows 上真的成立。RID 不对
        /// 就说明 SetTokenInformation 没生效 —— 后面拿它起的进程压根不在
        /// 低完整性里，整层沙箱是空的。
        #[test]
        fn 受限令牌是低完整性() {
            let tok = create_restricted_low_il().expect("造令牌");
            let rid = integrity_rid(*tok.0).expect("读完整性级别");
            assert_eq!(rid, 0x1000, "完整性级别必须是 Low");
        }
    }
}

// 目录标签：给可写目录打 / 去 Low 完整性标签。清单管理（跨平台、
// 孤儿回收）在 crate::sandbox_labels，这里只有 Win32 那一下。
// M2 的激活序列（见 SANDBOX_WINDOWS.md §2）把两者串起来。
#[cfg(windows)]
#[allow(dead_code)]
mod label {
    use std::ffi::c_void;
    use std::path::Path;

    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{
        ConvertStringSidToSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT, SetNamedSecurityInfoW,
    };
    use windows::Win32::Security::{
        ACE_HEADER, ACL, ACL_REVISION, AddMandatoryAce, CONTAINER_INHERIT_ACE, GetAce,
        GetLengthSid, GetSidSubAuthority, GetSidSubAuthorityCount, InitializeAcl,
        LABEL_SECURITY_INFORMATION, OBJECT_INHERIT_ACE, PSECURITY_DESCRIPTOR, PSID,
        SYSTEM_MANDATORY_LABEL_ACE,
    };
    use windows::Win32::System::SystemServices::{
        SYSTEM_MANDATORY_LABEL_ACE_TYPE, SYSTEM_MANDATORY_LABEL_NO_WRITE_UP,
    };
    use windows::core::{HSTRING, w};

    /// Low 完整性级别的 RID（`S-1-16-4096`）。
    const LOW_RID: u32 = 0x1000;

    /// 目录当前有没有**显式的**完整性标签。
    ///
    /// `None` = 没有，也就是默认完整性（Medium）—— 唯一一种我们敢动的。
    /// `Some(rid)` = 有一条 mandatory label ACE，rid 是它的级别。
    ///
    /// `[约束]` 打标签前必须问一次。清单「只记路径不记原状」那个简化
    /// （见 [`crate::sandbox_labels`] 模块头）成立的前提，就是我们只对默认
    /// 完整性的目录下手：回滚 = 写空 label 回到默认，没有"原状"要保存。
    /// 对一个本来就带标签的目录硬打，`untag` 会把用户的标签直接抹掉，而
    /// 清单里没有任何信息能把它还原。
    ///
    /// 读 label 只要 `READ_CONTROL`，不需要 `SE_SECURITY_NAME` —— 那是审计
    /// ACE 才要的特权。所以这一步在普通用户下也做得了。
    pub fn current_label_rid(dir: &Path) -> windows::core::Result<Option<u32>> {
        unsafe {
            let wide = HSTRING::from(dir.as_os_str());
            let mut sacl: *mut ACL = std::ptr::null_mut();
            let mut sd = PSECURITY_DESCRIPTOR::default();
            GetNamedSecurityInfoW(
                &wide,
                SE_FILE_OBJECT,
                LABEL_SECURITY_INFORMATION,
                None,
                None,
                None,
                Some(&mut sacl),
                &mut sd,
            )
            .ok()?;
            let _free = LocalSd(sd);

            // 没有 SACL = 没有 label = 默认完整性。绝大多数目录都走这条。
            if sacl.is_null() {
                return Ok(None);
            }
            for i in 0..u32::from((*sacl).AceCount) {
                let mut ace: *mut c_void = std::ptr::null_mut();
                if GetAce(sacl, i, &mut ace).is_err() {
                    continue;
                }
                let header = ace as *const ACE_HEADER;
                if u32::from((*header).AceType) != SYSTEM_MANDATORY_LABEL_ACE_TYPE {
                    continue;
                }
                // SID 紧跟在 ACE 头之后，`SidStart` 就是它的第一个字节。
                let label = ace as *const SYSTEM_MANDATORY_LABEL_ACE;
                let sid = PSID(&raw const (*label).SidStart as *mut c_void);
                let count = *GetSidSubAuthorityCount(sid);
                // 最后一个 subauthority 才是完整性级别 RID。
                let rid = *GetSidSubAuthority(sid, u32::from(count.saturating_sub(1)));
                return Ok(Some(rid));
            }
            Ok(None)
        }
    }

    /// 给目录打 Low 完整性标签，`no-write-up` 位让低完整性进程能写它。
    ///
    /// 只写 SACL 的 label 部分（`LABEL_SECURITY_INFORMATION`），不碰
    /// DACL / owner —— 那些不是这层要动的。容器继承（子目录/文件跟着
    /// 生效）靠 ACE 的继承标志，`AddMandatoryAce` 带 `OBJECT_INHERIT |
    /// CONTAINER_INHERIT`。
    ///
    /// `[约束]` 先体检再动手，见 [`current_label_rid`]。已经是 Low 的放行
    /// （那要么是我们上次崩溃留下的残留、要么本来就等于我们要设的值，重打
    /// 一次是幂等的）；**任何别的级别一律拒绝**，让整条激活降级成"不隔离"。
    /// 宁可这台机器上没有沙箱，也不能把用户的标签抹掉又还不回去。
    pub fn tag_low(dir: &Path) -> windows::core::Result<()> {
        if let Some(rid) = current_label_rid(dir)?
            && rid != LOW_RID
        {
            tracing::warn!(
                dir = %dir.display(),
                rid = format!("0x{rid:x}"),
                "目录已带非默认完整性标签，不动它"
            );
            return Err(windows::core::Error::new(
                windows::Win32::Foundation::E_ACCESSDENIED,
                "目录已有非默认完整性标签，打标签会抹掉它",
            ));
        }
        set_label(dir, w!("S-1-16-4096"))
    }

    /// 给目录写一条指定级别的 mandatory label（带容器继承）。
    ///
    /// 拆出来是为了让测试能造一个「本来就带非默认标签」的目录 ——
    /// [`tag_low`] 拒绝那种目录的行为，只能这么验。生产路径只用 Low。
    pub fn set_label(dir: &Path, sid_str: windows::core::PCWSTR) -> windows::core::Result<()> {
        unsafe {
            let mut sid = PSID::default();
            ConvertStringSidToSidW(sid_str, &mut sid)?;
            let _guard = LocalSid(sid);

            // ACL 要能装下 ACL 头 + 一条 mandatory ACE。ACE 大小 =
            // 固定头 + SID 主体，给足余量按页对齐。
            let acl_bytes = 256usize + GetLengthSid(sid) as usize;
            let mut buf = vec![0u8; acl_bytes];
            let acl = buf.as_mut_ptr() as *mut ACL;
            InitializeAcl(acl, acl_bytes as u32, ACL_REVISION)?;

            // 继承标志让目录下新建的文件和子目录自动带上同一条标签。
            AddMandatoryAce(
                acl,
                ACL_REVISION,
                OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE,
                SYSTEM_MANDATORY_LABEL_NO_WRITE_UP,
                sid,
            )?;

            let wide = HSTRING::from(dir.as_os_str());
            SetNamedSecurityInfoW(
                &wide,
                SE_FILE_OBJECT,
                LABEL_SECURITY_INFORMATION,
                None,
                None,
                None,
                Some(acl as *const ACL),
            )
            .ok()
        }
    }

    /// 去掉标签：写一个**空** SACL label，对象回到默认完整性
    /// （Medium）。回滚、孤儿回收、撤保护洞都走这条 —— 见 sandbox_labels
    /// 里「只记路径不记原状」的取舍。
    pub fn untag(dir: &Path) -> windows::core::Result<()> {
        unsafe {
            let acl_bytes = 256usize;
            let mut buf = vec![0u8; acl_bytes];
            let acl = buf.as_mut_ptr() as *mut ACL;
            InitializeAcl(acl, acl_bytes as u32, ACL_REVISION)?;
            // 空 label ACL = 没有 mandatory label = 默认 Medium。
            let wide = HSTRING::from(dir.as_os_str());
            SetNamedSecurityInfoW(
                &wide,
                SE_FILE_OBJECT,
                LABEL_SECURITY_INFORMATION,
                None,
                None,
                None,
                Some(acl as *const ACL),
            )
            .ok()
        }
    }

    /// Medium 完整性级别的 RID（`S-1-16-8192`，即默认完整性的显式形态）。
    const MEDIUM_RID: u32 = 0x2000;

    /// 给洞子目录打**显式 Medium 标签**（带继承）：父目录随后打 Low 时，
    /// 自动继承的传播不会顶掉子对象上已有的显式 label —— 效果是「父目录
    /// 整树 Low，这棵子树保持 Medium」。行为由测试
    /// `保护洞子目录不吃父目录的_low_标签` 在真机上钉住。
    ///
    /// 这是 `.cargo\bin` 那类**装着用户 PATH 可执行文件**的子目录的豁免
    /// 手段（"保护洞"）：从带 Low 标签的 exe 启动的进程会被降到 Low，
    /// 给 `.cargo` 整树打标等于把用户终端里的 cargo（rustup shim）一并
    /// 降权。Medium+NW 顺带保住一个逃逸面 —— 沙箱的 Low 进程写不进 bin，
    /// 就没法往 PATH 里顶一个假 cargo.exe 等用户在沙箱外执行。
    ///
    /// `[取舍]` 首选其实是"受保护的空标签"（`PROTECTED_SACL` 挡继承、
    /// 自身无标签），但设置 SACL 的 protected 位要 `SeSecurityPrivilege`，
    /// 普通用户下 `SetNamedSecurityInfoW` 直接报 0x80070522（实测）。
    /// 显式 Medium 是普通用户做得到的等效物：Medium 就是默认级别，对
    /// 一切非沙箱进程零行为差异。
    ///
    /// `[约束]` 打洞前体检，同 [`tag_low`]：默认、Medium（我们的残留，
    /// 重打幂等）、Low（上次父标签传播的残留，覆盖掉正是目的）都放行；
    /// 其它级别一律拒绝 —— 那是用户自己的标签，抹掉就还不回去了。
    pub fn hole_medium(dir: &Path) -> windows::core::Result<()> {
        if let Some(rid) = current_label_rid(dir)?
            && rid != MEDIUM_RID
            && rid != LOW_RID
        {
            return Err(windows::core::Error::new(
                windows::Win32::Foundation::E_ACCESSDENIED,
                "洞目录已有非默认完整性标签，打洞会抹掉它",
            ));
        }
        set_label(dir, w!("S-1-16-8192"))
    }

    /// LocalFree 守卫，同 token 模块。
    struct LocalSid(PSID);
    impl Drop for LocalSid {
        fn drop(&mut self) {
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.0.0)));
            }
        }
    }

    /// `GetNamedSecurityInfoW` 分配的安全描述符，同样由 LocalFree 释放。
    struct LocalSd(PSECURITY_DESCRIPTOR);
    impl Drop for LocalSd {
        fn drop(&mut self) {
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.0.0)));
            }
        }
    }

    /// 真实的目录打标签器。把 [`tag_low`] / [`untag`] 接进跨平台的
    /// [`crate::sandbox_labels::DirLabeler`]，好让引用计数与回滚编排
    /// （`LabelRegistry`）用上它 —— 那套编排的正确性在 sandbox_labels
    /// 里跨平台测过，这里只负责把 Win32 错误转成 io 错误接上去。
    ///
    /// 带一张**保护洞**清单（`(父目录, 洞)` 对）：打某个父目录的标签前，
    /// 先给它的洞设 [`protect_default`]；撤标签后再 [`unprotect_default`]。
    /// 洞放在 labeler 里而不是打标编排里，是因为激活（`LabelRegistry::
    /// acquire`）和孤儿回收（`sandbox_labels::recover_orphans`）两条路都
    /// 要经过它 —— 编排层各织一遍，漏一处的表现就是回收后 bin 恢复吃标签。
    pub struct WinLabeler {
        /// `(被打标的父目录, 保持默认完整性的子目录)`，都已 canonicalize。
        holes: Vec<(std::path::PathBuf, std::path::PathBuf)>,
    }

    impl WinLabeler {
        /// 生产装配：唯一的洞是 `~/.cargo` 下的 `bin`（rustup shim 全家，
        /// 用户 PATH 上的 cargo/rustc 就是它们 —— 吃了 Low 标签等于全机
        /// Rust 工具链启动即降权）。`.rustup` 和 pnpm 全局目录不在这里，
        /// 它们整条不打标（见 `sandbox::workspace_write` 的表）。
        pub fn standard() -> Self {
            let holes = std::env::var_os("USERPROFILE")
                .map(std::path::PathBuf::from)
                .into_iter()
                .filter_map(|home| {
                    // canonicalize 顺带过滤不存在的：`.cargo` 或 bin 不在，
                    // 洞清单就是空的，行为退回"整树打标"。
                    let cargo = home.join(".cargo").canonicalize().ok()?;
                    let bin = cargo.join("bin").canonicalize().ok()?;
                    Some((cargo, bin))
                })
                .collect();
            Self { holes }
        }

        /// 测试用：显式给洞清单，不读环境。
        #[cfg(test)]
        pub fn with_holes(holes: Vec<(std::path::PathBuf, std::path::PathBuf)>) -> Self {
            Self { holes }
        }

        fn holes_of<'a>(
            &'a self,
            dir: &'a std::path::Path,
        ) -> impl Iterator<Item = &'a std::path::Path> {
            self.holes
                .iter()
                .filter(move |(parent, _)| parent == dir)
                .map(|(_, hole)| hole.as_path())
        }
    }

    impl crate::sandbox_labels::DirLabeler for WinLabeler {
        fn tag(&self, dir: &std::path::Path) -> std::io::Result<()> {
            // 洞先设：显式 Medium 压住随后打标那一刻的继承传播，bin 子树
            // 全程不吃 Low。设洞失败就让整次 tag 失败 —— 编排层会回滚、
            // activate 返回 None 诚实降级，绝不能带着"bin 也被降权"的
            // 副作用继续激活。
            for hole in self.holes_of(dir) {
                hole_medium(hole).map_err(|e| {
                    std::io::Error::other(format!("给 {} 设保护洞失败：{e}", hole.display()))
                })?;
            }
            tag_low(dir).map_err(|e| std::io::Error::other(e.to_string()))
        }
        fn untag(&self, dir: &std::path::Path) -> std::io::Result<()> {
            untag(dir).map_err(|e| std::io::Error::other(e.to_string()))?;
            // 洞后撤（父目录的 Low 先清，洞恢复默认时才不会吃到回灌的
            // 继承）。失败只告警不上抛：标签本体已撤干净，残留的显式
            // Medium 就是默认级别的显式形态，对一切行为零影响，下次打洞
            // 幂等覆盖。上抛反而会让编排层把这条账保留，孤儿回收永远
            // 重试一个无害状态。
            for hole in self.holes_of(dir) {
                if let Err(e) = untag(hole) {
                    tracing::warn!(
                        hole = %hole.display(),
                        error = %e,
                        "撤保护洞失败（无害残留，下次打标幂等覆盖）"
                    );
                }
            }
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn scratch(name: &str) -> std::path::PathBuf {
            let d = std::env::temp_dir().join(format!("riot-label-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).expect("建目录");
            d
        }

        /// 打标签 / 读回 / 撤标签的往返。
        #[test]
        fn 打了标签读得回来_撤了就没了() {
            let d = scratch("roundtrip");
            assert_eq!(current_label_rid(&d).expect("读"), None, "新目录该是默认完整性");

            tag_low(&d).expect("打 Low 标签");
            assert_eq!(current_label_rid(&d).expect("读"), Some(0x1000));

            untag(&d).expect("撤标签");
            assert_eq!(current_label_rid(&d).expect("读"), None, "撤完该回到默认");
            let _ = std::fs::remove_dir_all(&d);
        }

        /// 保护洞：打父目录 Low 时，洞子目录（`.cargo\bin` 那类装着用户
        /// PATH 可执行文件的地方）必须保持默认完整性 —— 从 Low 标签的 exe
        /// 启动的进程会被降到 Low，全机工具链跟着报废（2026-08-25 真实
        /// 事故：`.rustup` 整树被标，宿主机 cargo 全局 os error 5）。
        /// 撤完标签后洞要恢复出厂：无标签、重新接受继承。
        #[test]
        fn 保护洞子目录不吃父目录的_low_标签() {
            use crate::sandbox_labels::DirLabeler as _;

            let d = scratch("hole");
            let bin = d.join("bin");
            std::fs::create_dir_all(&bin).expect("建 bin");
            let exe = bin.join("tool.exe");
            std::fs::write(&exe, b"stub").expect("造 exe");

            let labeler = WinLabeler::with_holes(vec![(d.clone(), bin.clone())]);

            labeler.tag(&d).expect("打标签");
            assert_eq!(current_label_rid(&d).expect("读父"), Some(0x1000));
            assert_eq!(
                current_label_rid(&bin).expect("读洞"),
                Some(0x2000),
                "洞该是显式 Medium（默认级别的显式形态）"
            );
            assert_ne!(
                current_label_rid(&exe).expect("读洞内文件"),
                Some(0x1000),
                "洞里的 exe 吃到 Low 就是启动降权"
            );

            labeler.untag(&d).expect("撤标签");
            assert_eq!(current_label_rid(&d).expect("读父"), None);
            assert_eq!(current_label_rid(&bin).expect("读洞"), None);

            // 恢复出厂 = 重新接受继承：裸打父目录（不带洞），bin 该跟着
            // 变 Low —— 撤保护没做干净的话，洞会变成永久的。
            tag_low(&d).expect("裸打");
            assert_eq!(
                current_label_rid(&bin).expect("读洞"),
                Some(0x1000),
                "撤保护后要恢复继承"
            );
            untag(&d).expect("清场");
            let _ = std::fs::remove_dir_all(&d);
        }

        /// `[约束]` 本来就带非默认标签的目录**不许**碰。
        ///
        /// 清单只记路径不记原状（见 sandbox_labels 模块头），那个简化的前提
        /// 就是这条检查存在 —— 少了它，`untag` 会把用户的标签抹掉，而清单里
        /// 没有任何信息能还原。文档 §2 承诺了这个行为，这条用例钉住它。
        #[test]
        fn 已带非默认标签的目录拒绝打标签() {
            let d = scratch("preexisting");
            // 显式写一条 Medium（S-1-16-8192）。不用 High：抬到自己级别之上
            // 要 SeRelabelPrivilege，普通用户下做不到。
            set_label(&d, w!("S-1-16-8192")).expect("造一个带标签的目录");
            assert_eq!(current_label_rid(&d).expect("读"), Some(0x2000));

            assert!(tag_low(&d).is_err(), "带别人标签的目录必须拒绝");
            assert_eq!(
                current_label_rid(&d).expect("读"),
                Some(0x2000),
                "拒绝之后原标签必须原封不动"
            );
            let _ = std::fs::remove_dir_all(&d);
        }

        /// 自己上次崩溃留下的 Low 标签不该把沙箱永久卡死 —— 重打是幂等的。
        #[test]
        fn 已经是_low_的目录可以重复打() {
            let d = scratch("idempotent");
            tag_low(&d).expect("第一次");
            tag_low(&d).expect("残留的 Low 标签不该挡住重新激活");
            assert_eq!(current_label_rid(&d).expect("读"), Some(0x1000));
            let _ = std::fs::remove_dir_all(&d);
        }
    }
}

// 用受限令牌起进程。这是 M2 的最后一块，也是 spawn 集成的核心难点。
//
// 为什么不能复用 proc.rs：tokio / std 的 Command 都不暴露「用这个令牌
// 起进程」，只有 CreateProcessAsUserW 收令牌。所以整条 spawn 自己写：
// 建管道、拼命令行/环境块、起进程、挂 Job Object、并发读、超时/取消。
// 语义对齐 proc.rs（两个并发 drain + select 等待 + 无条件杀组），只是
// 底层从 tokio::process 换成同步 Win32 + spawn_blocking。
#[cfg(windows)]
#[allow(dead_code)]
mod spawn {
    use std::ffi::c_void;
    use std::io::Read;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use riot_protocol::tool::{ProcessOutput, ProcessSpec};
    use tokio_util::sync::CancellationToken;
    use windows::Win32::Foundation::{
        CloseHandle, GENERIC_READ, HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAGS,
        SetHandleInformation,
    };
    use windows::Win32::Security::SECURITY_ATTRIBUTES;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };
    use windows::Win32::System::Pipes::CreatePipe;
    use windows::Win32::System::Threading::{
        CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW,
        DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE,
        InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, ResumeThread,
        STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW, TerminateProcess,
        UpdateProcThreadAttribute, WaitForSingleObject,
    };
    use windows::core::{PCWSTR, PWSTR};

    const EXIT_TIMEOUT: i32 = 124;
    const EXIT_CANCELLED: i32 = 130;

    /// 杀组之后再等多久 EOF。见 [`spawn_with_token`] 里的说明。
    const DRAIN_GRACE: Duration = Duration::from_secs(3);

    /// 一枚拥有所有权的内核句柄，`Drop` 时 `CloseHandle`。
    ///
    /// 存裸 `isize` 而不是 `HANDLE`：`HANDLE` 是 `*mut c_void`，不是 `Send`，
    /// 而这些句柄要跨 await、要进 `spawn_blocking`。搬成整数没有安全性损失
    /// —— Windows 句柄是**内核对象**的引用，进程内任意线程都能用、也都能关。
    struct OwnedHandle(isize);

    impl OwnedHandle {
        fn raw(&self) -> HANDLE {
            HANDLE(self.0 as *mut c_void)
        }
    }

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.raw());
            }
        }
    }

    /// 起好的子进程：进程句柄 + 它的 Job。
    ///
    /// `[约束]` 内核对象全由它独占，清理全在 `Drop` 里 —— **包括「外层
    /// future 中途被 drop」那条路径**。调度器用 `FuturesOrdered` 内联持有
    /// 工具 future，一次中断就把整批丢掉。摊成裸整数手工 CloseHandle 的
    /// 写法在三处漏东西：
    ///
    /// 1. future 被 drop → 谁都不杀、Job 句柄不关 → `KILL_ON_JOB_CLOSE`
    ///    永远不触发，子进程活到关机（`proc.rs` 那条路径有
    ///    `kill_on_drop(true)` 兜底，这里什么都没有）；
    /// 2. 收输出报错早退 → 后面的 CloseHandle 跑不到；
    /// 3. 超时/取消时主流程 CloseHandle，而 waiter 线程还卡在
    ///    `WaitForSingleObject` 上 —— 关一个正被等待的句柄是未定义行为，
    ///    而且句柄值可能已被另一条并发 spawn 复用，那个 waiter 就在等
    ///    别人的进程。
    ///
    /// 第 3 条另外靠 `process` 是 `Arc`：waiter 持有同一份所有权，最后一个
    /// 放手的才真关。
    struct Child {
        process: Arc<OwnedHandle>,
        job: OwnedHandle,
    }

    impl Child {
        /// 杀掉整个进程组。幂等 —— 正常收尾调一次，`Drop` 再兜一次。
        fn kill_group(&self) {
            unsafe {
                let _ = TerminateJobObject(self.job.raw(), 1);
            }
        }
    }

    impl Drop for Child {
        fn drop(&mut self) {
            // 无条件杀。正常路径上进程早没了（这里是 no-op），异常路径上这是
            // 唯一一次机会。它同时把三个 `spawn_blocking` 线程叫醒：waiter 等到
            // 进程退出，两个 drain 等到管道 EOF —— 那些线程取消不了，只能靠
            // 「让它们等的东西真的发生」来收。
            self.kill_group();
        }
    }

    /// 输出缓冲：读任务往里塞，主流程随时能把已经攒下的取走。
    ///
    /// `[约束]` 不能只靠 `JoinHandle` 的返回值。读任务是 `spawn_blocking`，
    /// 卡在 `ReadFile` 上时取消不了；万一写端被**别的** spawn 继承走了
    /// （见 [`create`] 的句柄继承说明），EOF 永远不来，那个任务永远不返回。
    /// 共享缓冲让主流程能「等一小会儿，然后带着已有的输出走人」。
    #[derive(Default)]
    struct Sink {
        buf: std::sync::Mutex<Vec<u8>>,
        capped: AtomicBool,
    }

    impl Sink {
        fn lock(&self) -> std::sync::MutexGuard<'_, Vec<u8>> {
            self.buf
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }

        /// 取走目前攒下的内容。返回 (内容, 是否触上限)。
        fn take(&self) -> (Vec<u8>, bool) {
            let buf = std::mem::take(&mut *self.lock());
            (buf, self.capped.load(Ordering::Relaxed))
        }
    }

    /// 用令牌起 `spec`，接管道/超时/进程组，返回输出。
    ///
    /// `token_raw` 是 `HANDLE.0 as isize` —— **不直接收 HANDLE**：HANDLE 是
    /// `*mut c_void`，不是 Send，若作为参数活到函数尾就会跨过中间的 await，
    /// 让整个 future 非 Send，而 `SandboxedRunner`（async_trait）要求返回
    /// `Send` future。收 isize（Send），进函数立刻转回 HANDLE 且只在
    /// **第一个 await 之前**的同步建进程段用掉，NLL 保证它不跨 await。
    ///
    /// 语义对齐 `proc.rs::SystemProcessRunner::run`：
    /// - stdout / stderr **并发**读（串行会死锁，见 proc.rs 注释）；
    /// - 等到「进程退出 / 超时 / 取消」任一，**无条件**杀整个 Job（正常
    ///   退出也杀，清掉可能残留的后台子进程）；
    /// - 读任务在杀组之后收 —— 写端全关了 EOF 才来。
    pub(crate) async fn spawn_with_token(
        token_raw: isize,
        spec: ProcessSpec,
        max_output: usize,
        cancel: CancellationToken,
    ) -> std::io::Result<ProcessOutput> {
        let started = Instant::now();
        let timeout = spec.timeout_ms.map(Duration::from_millis);

        // 建进程是同步 Win32，快，直接在异步上下文里做。token 在这里用完
        // （create 之后不再引用），NLL 让它在第一个 await 前就结束生命周期。
        let (child, read_out, read_err) =
            unsafe { create(HANDLE(token_raw as *mut c_void), &spec) }?;

        let out = Arc::new(Sink::default());
        let err = Arc::new(Sink::default());
        let h_out = tokio::task::spawn_blocking({
            let sink = Arc::clone(&out);
            move || drain(read_out, max_output, &sink)
        });
        let h_err = tokio::task::spawn_blocking({
            let sink = Arc::clone(&err);
            move || drain(read_err, max_output, &sink)
        });
        let waiter = tokio::task::spawn_blocking({
            let process = Arc::clone(&child.process);
            move || unsafe {
                WaitForSingleObject(process.raw(), INFINITE);
            }
        });

        let ended = tokio::select! {
            _ = waiter => Ended::Exited,
            _ = sleep_opt(timeout) => Ended::TimedOut,
            _ = cancel.cancelled() => Ended::Cancelled,
        };

        // 退出码在杀组**之前**读。Exited 分支里进程已经退了、值定死了，但
        // 反过来写的话，"select 刚返回、进程恰好也退了"这一瞬会被
        // TerminateJobObject 把真实退出码改成我们编的那个。
        let exit_code = match ended {
            Ended::Exited => unsafe { exit_code_of(child.process.raw()) },
            Ended::TimedOut => EXIT_TIMEOUT,
            Ended::Cancelled => EXIT_CANCELLED,
        };

        // 无条件杀整组（对齐 proc.rs：正常退出 ≠ 后台子进程也退了）。
        child.kill_group();

        // 杀组之后再收输出：写端此刻全关，EOF 才来。
        //
        // `[约束]` 但不能无条件等。写端有可能被**别的** spawn 继承走
        // （见 create 里的句柄继承说明，那个窗口关不掉），那样 EOF 永远
        // 不来。给一个宽限期，到点就把已经攒下的交出去 —— 丢几行尾巴，
        // 远好过整条命令挂死（而"莫名挂到超时"正是这个竞态过去的表现）。
        // 正常路径上 EOF 紧跟着杀组就到，这段等待是零成本。
        let both = async {
            let _ = tokio::join!(h_out, h_err);
        };
        if tokio::time::timeout(DRAIN_GRACE, both).await.is_err() {
            tracing::warn!(
                program = %spec.program,
                "等不到管道 EOF（写端可能被别的进程继承走了），按已收到的输出返回"
            );
        }
        let (stdout, out_capped) = out.take();
        let (stderr, err_capped) = err.take();
        if out_capped || err_capped {
            tracing::warn!(program = %spec.program, "沙箱命令输出超上限，已截断");
        }

        Ok(ProcessOutput {
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            exit_code,
            timed_out: matches!(ended, Ended::TimedOut),
            duration_ms: started.elapsed().as_millis() as u64,
        })
    }

    enum Ended {
        Exited,
        TimedOut,
        Cancelled,
    }

    /// 同步建进程：建管道、拼命令行/环境、CreateProcessAsUserW、挂 Job。
    /// 返回 Send 的 [`Child`] 和两个读端。
    ///
    /// # 句柄继承：两个方向，只能治一个
    ///
    /// `CreateProcessAsUserW` 必须 `bInheritHandles=true`（stdio 要传进去），
    /// 而它默认继承的是**本进程当前所有可继承句柄**。两个方向各有毛病：
    ///
    /// - *别人的句柄漏进我们的子进程*：既是竞态（继承到另一条 spawn 还没
    ///   关的管道写端，害它等不到 EOF），也是**沙箱漏洞** —— MIC 只在
    ///   `open` 时检查，一个继承来的、指向 Medium 对象的可写句柄，低完整性
    ///   进程照样能拿它写。用 `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` 显式白
    ///   名单彻底治掉：子进程只拿到下面列出的三个。
    /// - *我们的写端漏进别人的子进程*：白名单管不了 —— 句柄列表里的句柄
    ///   **必须**是可继承的，所以 CreateProcess 期间那两个写端就是敞着的，
    ///   而同进程里 `std`/`tokio` 的 spawn（hooks、MCP、非沙箱会话、终端
    ///   面板）一律 `bInheritHandles=true` 且不带白名单。这个窗口关不掉。
    ///
    /// 所以下面那把锁**只解决一半**：它让我们自己的并发 spawn 不互相偷
    /// 句柄（最常见的情形），对进程里别的 spawn 点无能为力。真正的兜底是
    /// [`spawn_with_token`] 里收输出的宽限期 —— 就算写端真被偷走，最坏
    /// 也只是丢掉几行尾巴，不会挂死。见 SANDBOX_WINDOWS.md §3。
    unsafe fn create(
        token: HANDLE,
        spec: &ProcessSpec,
    ) -> std::io::Result<(Child, std::fs::File, std::fs::File)> {
        // 临界区只有同步的建进程段（无 await、无阻塞等待），锁很快就放，
        // 命令起来之后照样并发跑。锁中毒接着用：临界区里没有会破坏不变量的
        // panic 点，卡死 spawn 比带毒继续更糟（同 sandbox_labels 的取舍）。
        static SPAWN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _spawn_guard = SPAWN_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe {
            let sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: std::ptr::null_mut(),
                bInheritHandle: true.into(),
            };
            // 三个句柄从建出来那一刻就有主：读端交给 `File`，写端和 NUL 交给
            // `OwnedHandle`，全都在本函数结束时释放 —— 也就是「起完就关父进程
            // 这边的写端」（不关的话读端的 EOF 永远不来），而且任何早退路径
            // 都自动做到。
            let (read_out, write_out) = pipe(&sa)?;
            let (read_err, write_err) = pipe(&sa)?;
            // stdin 给 NUL：一律立即 EOF —— 读 stdin 的命令（cat、等确认的
            // 脚本）不会挂住。对齐 proc.rs 的 Stdio::null()。
            let nul = OwnedHandle(
                CreateFileW(
                    windows::core::w!("NUL"),
                    GENERIC_READ.0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    Some(&sa),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                )
                .map_err(win_err)?
                .0 as isize,
            );

            // 白名单：子进程只继承这三个。它们都是用带 bInheritHandle 的 sa
            // 建的 —— 列表里出现不可继承的句柄会让 CreateProcess 直接失败。
            let mut attrs = handle_list(vec![nul.raw(), write_out.raw(), write_err.raw()])?;
            let si = STARTUPINFOEXW {
                StartupInfo: STARTUPINFOW {
                    // 用了 EXTENDED_STARTUPINFO_PRESENT 就要报 EX 结构的大小。
                    cb: std::mem::size_of::<STARTUPINFOEXW>() as u32,
                    dwFlags: STARTF_USESTDHANDLES,
                    hStdInput: nul.raw(),
                    hStdOutput: write_out.raw(),
                    hStdError: write_err.raw(),
                    ..Default::default()
                },
                lpAttributeList: attrs.list(),
            };

            // vars_os 而不是 vars()：后者在某个环境变量含非法 UTF-8 时直接
            // panic，一条脏变量就能崩掉整个 spawn。lossy 转换对读环境的子进程
            // 无害 —— 值本来就要拼进 UTF-16 环境块。
            let base: Vec<(String, String)> = std::env::vars_os()
                .map(|(k, v)| {
                    (
                        k.to_string_lossy().into_owned(),
                        v.to_string_lossy().into_owned(),
                    )
                })
                .collect();
            let mut env = crate::sandbox_cmdline::build_env_block(&base, &spec.env);
            let mut cmdline: Vec<u16> =
                crate::sandbox_cmdline::build_command_line(&spec.program, &spec.args)
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect();
            let cwd: Vec<u16> = spec
                .cwd
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

            let mut pi = PROCESS_INFORMATION::default();
            // CREATE_SUSPENDED：先把进程挂进 Job 再放它跑，否则它可能在
            // AssignProcessToJobObject 之前就 fork 出逃逸 Job 的子进程。
            CreateProcessAsUserW(
                Some(token),
                PCWSTR::null(),
                Some(PWSTR(cmdline.as_mut_ptr())),
                None,
                None,
                true,
                CREATE_NO_WINDOW
                    | CREATE_SUSPENDED
                    | CREATE_UNICODE_ENVIRONMENT
                    | EXTENDED_STARTUPINFO_PRESENT,
                Some(env.as_mut_ptr() as *const c_void),
                PCWSTR(cwd.as_ptr()),
                &si.StartupInfo,
                &mut pi,
            )
            .map_err(win_err)?;

            // 进程和线程句柄立刻交给 RAII，后面任何早退都不会漏。
            let process = Arc::new(OwnedHandle(pi.hProcess.0 as isize));
            let thread = OwnedHandle(pi.hThread.0 as isize);

            let job = match setup_job(process.raw()) {
                Ok(job) => OwnedHandle(job.0 as isize),
                Err(e) => {
                    // 进程此刻是挂起态、又还没进 Job，没有任何东西会收它 ——
                    // 必须显式杀，否则漏下一个永远挂起的孤儿。句柄本身由
                    // OwnedHandle 收。
                    let _ = TerminateProcess(process.raw(), 1);
                    return Err(e);
                }
            };

            ResumeThread(thread.raw());
            drop(thread);

            Ok((Child { process, job }, read_out, read_err))
        }
    }

    /// `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` 属性表，连同它引用的句柄数组。
    ///
    /// 两块内存都得活到 `CreateProcess` 返回：`UpdateProcThreadAttribute`
    /// 只记下句柄数组的**指针**，不拷贝内容。
    struct AttrList {
        buf: Vec<u8>,
        handles: Vec<HANDLE>,
    }

    impl AttrList {
        fn list(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
            LPPROC_THREAD_ATTRIBUTE_LIST(self.buf.as_mut_ptr().cast())
        }
    }

    impl Drop for AttrList {
        fn drop(&mut self) {
            unsafe { DeleteProcThreadAttributeList(self.list()) }
        }
    }

    unsafe fn handle_list(handles: Vec<HANDLE>) -> std::io::Result<AttrList> {
        unsafe {
            // 第一次调用注定失败（ERROR_INSUFFICIENT_BUFFER），只为问出大小。
            let mut size = 0usize;
            let _ = InitializeProcThreadAttributeList(None, 1, None, &mut size);
            if size == 0 {
                return Err(std::io::Error::other("问不出进程属性表的大小"));
            }
            let mut me = AttrList {
                buf: vec![0u8; size],
                handles,
            };
            // 从 Initialize 成功那一刻起就必须配一次 Delete —— 所以先让
            // AttrList 接管，再做可能失败的 Update。
            InitializeProcThreadAttributeList(Some(me.list()), 1, None, &mut size)
                .map_err(win_err)?;
            let list = me.list();
            let ptr: *const c_void = me.handles.as_ptr().cast();
            let bytes = std::mem::size_of_val(me.handles.as_slice());
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                Some(ptr),
                bytes,
                None,
                None,
            )
            .map_err(win_err)?;
            Ok(me)
        }
    }

    /// 建 Job Object、设 `KILL_ON_JOB_CLOSE`、把进程挂进去。
    ///
    /// 失败时把**本函数已建的 Job** 关掉再返回错误；进程的清理留给调用方
    /// （它知道要不要连带杀进程）。拆出来是为了用 `?` 之外的显式清理收拢
    /// 三步 Win32 的失败路径 —— 直接用 `?` 的话，中途失败会漏掉已建的 Job。
    unsafe fn setup_job(process: HANDLE) -> std::io::Result<HANDLE> {
        unsafe {
            let job = CreateJobObjectW(None, PCWSTR::null()).map_err(win_err)?;
            let mut limit = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limit.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if let Err(e) = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limit as *const _ as *const c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) {
                let _ = CloseHandle(job);
                return Err(win_err(e));
            }
            if let Err(e) = AssignProcessToJobObject(job, process) {
                let _ = CloseHandle(job);
                return Err(win_err(e));
            }
            Ok(job)
        }
    }

    /// 一根匿名管道：读端交给 `File`（RAII），写端交给 [`OwnedHandle`]。
    unsafe fn pipe(sa: &SECURITY_ATTRIBUTES) -> std::io::Result<(std::fs::File, OwnedHandle)> {
        unsafe {
            let mut read = HANDLE::default();
            let mut write = HANDLE::default();
            CreatePipe(&mut read, &mut write, Some(sa), 0).map_err(win_err)?;
            // 先接管所有权，再做可能失败的事。
            let file = std::fs::File::from_raw_handle(read.0 as std::os::windows::io::RawHandle);
            let write = OwnedHandle(write.0 as isize);
            // 读端清掉继承标志。子进程拿不到它本来就由句柄白名单保证了，
            // 这一下是防**别的** spawn 点（不带白名单）把它捎带出去 ——
            // 纯粹的卫生：不该让任何外部进程握着我们的读端。
            SetHandleInformation(read, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)).map_err(win_err)?;
            Ok((file, write))
        }
    }

    unsafe fn exit_code_of(process: HANDLE) -> i32 {
        unsafe {
            let mut code = 0u32;
            if GetExitCodeProcess(process, &mut code).is_ok() {
                code as i32
            } else {
                -1
            }
        }
    }

    /// 同步读到 EOF 或读满上限，边读边往 `sink` 里塞。
    ///
    /// 读出错就收工（不往上报）：走到这一步无非是管道断了，而此刻手里已经
    /// 有部分输出和真实退出码 —— 把它们交出去比让整条命令失败有用得多。
    fn drain(mut f: std::fs::File, cap: usize, sink: &Sink) {
        let mut chunk = [0u8; 16 * 1024];
        loop {
            let n = match f.read(&mut chunk) {
                Ok(0) => return,
                Ok(n) => n,
                Err(e) => {
                    tracing::debug!(error = %e, "读子进程输出中断");
                    return;
                }
            };
            let mut buf = sink.lock();
            let room = cap.saturating_sub(buf.len());
            if room == 0 {
                sink.capped.store(true, Ordering::Relaxed);
                // 直接返回，`f` 在这里被 drop —— 读端一关，写端下次写就拿到
                // broken pipe。这正是 `head -n 10` 让上游停下来的机制。
                return;
            }
            buf.extend_from_slice(&chunk[..n.min(room)]);
        }
    }

    async fn sleep_opt(d: Option<Duration>) {
        match d {
            Some(d) => tokio::time::sleep(d).await,
            None => std::future::pending().await,
        }
    }

    fn win_err(e: windows::core::Error) -> std::io::Error {
        std::io::Error::other(e.to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// 冒烟：用受限 Low 令牌起 `cmd /c echo hi`，验证整条 spawn 通路
        /// （管道不死锁、拿得到 stdout、退出码正常）。这是 spawn 集成最大的
        /// 运行时风险点，单独钉死；边界（工作区外写被拒）留给 M2 的接线用例。
        #[tokio::test]
        async fn 受限令牌起进程拿得到输出() {
            let tok = super::super::token::create_restricted_low_il().expect("造令牌");
            let spec = ProcessSpec {
                program: "cmd".to_owned(),
                args: vec!["/c".to_owned(), "echo hi".to_owned()],
                cwd: std::env::temp_dir(),
                env: Vec::new(),
                timeout_ms: Some(10_000),
            };
            let out = spawn_with_token((*tok.0).0 as isize, spec, 1 << 20, CancellationToken::new())
                .await
                .expect("跑得起来");
            assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
            assert!(out.stdout.contains("hi"), "stdout={:?}", out.stdout);
        }

        /// 并发 spawn：多条命令同时起，每条都要拿到**自己**完整的输出、且都
        /// 不超时。这是句柄继承竞态（SANDBOX_WINDOWS.md §3 / `create` 里那把
        /// `SPAWN_LOCK`）的回归钉。
        ///
        /// 没有那把锁时：一个 spawn 的子进程会继承到另一个 spawn 刚
        /// `CreatePipe`、还没来得及 `CloseHandle` 的可继承写端 —— 后者的读端
        /// 于是永远等不到 EOF，表现是它的 stdout 空、一直挂到超时。断言里
        /// 「不超时 + 拿到自己的标记」正是对这个失败模式的反面。
        ///
        /// `[约束]` 必须 `multi_thread` + `tokio::spawn` 才逼得出竞态：`create`
        /// 是第一个 await 之前的同步段，单线程 `join!` 会让它一条跑完再跑下
        /// 一条（临界区从不重叠），那样即便锁没了也测不出问题。真并发下多个
        /// `create` 落在不同工作线程上，才会去抢那把锁。
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn 并发起进程各拿各的输出不串扰() {
            // 令牌活到所有任务 await 完（`tok` 在函数尾才 drop），各任务只借
            // 它的裸值（isize，Copy）—— 并发只读同一枚令牌是安全的。
            let tok = super::super::token::create_restricted_low_il().expect("造令牌");
            let token = (*tok.0).0 as isize;

            // 起够多条，让临界区有充分重叠机会；每条 echo 一个唯一标记。
            let mut handles = Vec::new();
            for i in 0..12u32 {
                let marker = format!("riot-conc-{i}");
                let spec = ProcessSpec {
                    program: "cmd".to_owned(),
                    args: vec!["/c".to_owned(), format!("echo {marker}")],
                    cwd: std::env::temp_dir(),
                    env: Vec::new(),
                    // 竞态若回归，受影响的那条会挂到这里才失败；正常路径 <1s。
                    timeout_ms: Some(20_000),
                };
                handles.push(tokio::spawn(async move {
                    let out = spawn_with_token(token, spec, 1 << 20, CancellationToken::new())
                        .await
                        .expect("跑得起来");
                    (marker, out)
                }));
            }

            for h in handles {
                let (marker, out) = h.await.expect("任务正常结束");
                assert!(
                    !out.timed_out,
                    "{marker} 超时了 —— 多半是继承了别的 spawn 的写端，EOF 不来"
                );
                assert_eq!(out.exit_code, 0, "{marker} 退出码非零：stderr={}", out.stderr);
                assert!(
                    out.stdout.contains(&marker),
                    "{marker} 没拿到自己的输出：stdout={:?}",
                    out.stdout
                );
            }
        }

        /// future 被丢掉之后，子进程必须跟着死。
        ///
        /// 这是本层「无论怎么死，别往机器上漏东西」在 Windows 侧的落点。
        /// 调度器用 `FuturesOrdered` **内联**持有工具 future，用户按一次
        /// 停止就把整批 drop 掉 —— `proc.rs` 那条路径有 `kill_on_drop(true)`
        /// 兜底，这条路径全靠 [`Child`] 的 Drop。没有它的话 Job 句柄不关、
        /// `KILL_ON_JOB_CLOSE` 不触发，子进程活到关机。
        ///
        /// 验法：让命令睡一会儿再写一个标记文件，中途把 future 丢掉，然后
        /// 等过那个睡眠时间 —— 标记文件出现就说明进程没被收掉。
        #[tokio::test]
        async fn future_被丢掉时子进程跟着死() {
            let tok = super::super::token::create_restricted_low_il().expect("造令牌");
            let dir = std::env::temp_dir().join(format!("riot-drop-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("建目录");
            let marker = dir.join("survived.txt");
            let _ = std::fs::remove_file(&marker);

            let spec = ProcessSpec {
                program: "cmd".to_owned(),
                args: vec![
                    "/c".to_owned(),
                    // 睡觉用 ping 而不是 timeout.exe：后者要控制台 stdin，
                    // 而这条 spawn 路径的 stdin 一律给 NUL —— timeout 会
                    // "Input redirection is not supported" 立即退出，进程
                    // 根本活不到被 drop 的那一刻。
                    format!(
                        "ping -n 4 127.0.0.1 > NUL & echo alive > {}",
                        marker.display()
                    ),
                ],
                cwd: dir.clone(),
                env: Vec::new(),
                timeout_ms: None,
            };

            {
                let mut fut =
                    Box::pin(spawn_with_token((*tok.0).0 as isize, spec, 1 << 20, CancellationToken::new()));
                // poll 一次把进程真的起起来，然后丢掉 future。
                let started = tokio::time::timeout(std::time::Duration::from_millis(300), &mut fut).await;
                assert!(started.is_err(), "这条命令不该这么快就结束");
            }

            // 睡过命令原本的存活时间，再看标记。
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            assert!(
                !marker.exists(),
                "子进程在 future 被丢掉之后还活着并写了文件 —— Job 没被收"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

// M2 的端到端验收：把令牌 + 标签 + spawn 三块串起来，验证边界真的由
// OS 执行 —— 低完整性进程只能写「打了 Low 标签的目录」，工作区外写不进。
// 这是文档 §6 用例 1，整个 Windows 沙箱正确性的核心。机制在这里闭环
// 验证（不碰 SandboxedRunner/session.rs），确认对了再接线（下一步）。
#[cfg(all(windows, test))]
mod e2e_tests {
    use std::path::Path;

    use riot_protocol::tool::ProcessSpec;
    use tokio_util::sync::CancellationToken;

    use crate::sandbox_labels::LabelRegistry;

    #[tokio::test]
    async fn 低完整性进程只能写打了标签的目录() {
        let base = std::env::temp_dir().join(format!("riot-sbx-e2e-{}", std::process::id()));
        let work = base.join("work");
        let outside = base.join("outside");
        std::fs::create_dir_all(&work).expect("建工作区");
        std::fs::create_dir_all(&outside).expect("建外部目录");

        // 只给 work 打 Low 标签；outside 保持默认（Medium）。用测试自己的
        // 注册表实例，不碰进程级 REGISTRY。
        let ledger_path = base.join("labels.json");
        let reg = LabelRegistry::new();
        let labeler = super::label::WinLabeler::standard();
        reg.acquire(std::slice::from_ref(&work), &labeler, &ledger_path, 0)
            .expect("给 work 打标签");

        let tok = super::token::create_restricted_low_il().expect("造受限 Low 令牌");
        // 令牌句柄搬成 isize（Send），spawn_with_token 内部再转回 HANDLE。
        let token = (*tok.0).0 as isize;
        // cwd 用中性的 base（普通 Medium 目录），把「写哪」和「进程 cwd」
        // 两个变量分开 —— 写目标一律用绝对路径。
        let cwd = base.clone();

        async fn write_to(token_raw: isize, target: &Path, cwd: &Path) -> (i32, String) {
            // `cmd /c` 后面每个片段单独成 arg：build_command_line 会原样
            // 拼回 `cmd /c echo hi > <path>`。**不要**把 `echo hi>"path"`
            // 拼成一个 arg —— 那样它含空格和 `>`，会被 argv 引用规则整体
            // 套引号，而 cmd 的 `/c` 不按 argv 规则解析，重定向就坏了
            // （报 "filename syntax is incorrect"）。target 用无空格的
            // 临时路径，避免再撞引用。
            let spec = ProcessSpec {
                program: "cmd".to_owned(),
                args: vec![
                    "/c".to_owned(),
                    "echo".to_owned(),
                    "hi".to_owned(),
                    ">".to_owned(),
                    target.display().to_string(),
                ],
                cwd: cwd.to_path_buf(),
                env: Vec::new(),
                timeout_ms: Some(10_000),
            };
            let o = super::spawn::spawn_with_token(token_raw, spec, 1 << 20, CancellationToken::new())
                .await
                .expect("跑得起来");
            (o.exit_code, o.stderr)
        }

        // 工作区内：打了 Low 标签，低完整性进程写得进。
        let inside = work.join("inside.txt");
        let (ec_in, err_in) = write_to(token, &inside, &cwd).await;
        assert_eq!(ec_in, 0, "打了标签的目录该写得进：exit={ec_in} stderr={err_in:?}");
        assert!(inside.exists(), "文件该真的被创建");

        // 工作区外：没打标签（默认 Medium），Low 进程 no-write-up 被 MIC 拦。
        let out_file = outside.join("outside.txt");
        let (ec_out, _) = write_to(token, &out_file, &cwd).await;
        assert_ne!(ec_out, 0, "没打标签的目录，低完整性进程必须写不进");
        assert!(
            !out_file.exists(),
            "文件真的不该被创建 —— 边界没生效的话它就在那儿"
        );

        // 归还引用（归零即真撤）+ 清理临时目录。
        reg.release(std::slice::from_ref(&work), &labeler, &ledger_path);
        let _ = std::fs::remove_dir_all(&base);
    }
}
