//! Windows 沙箱后端：Restricted Token + Low 完整性级别。
//!
//! 设计与取舍见 docs/SANDBOX_WINDOWS.md。这里是 **M1 骨架**：
//! 只做整条链路的地基 —— 造一个"去掉全部特权、完整性级别压到 Low"的
//! 令牌。用这个令牌起进程（M2）、给可写目录打 Low 标签（M2）尚未接入，
//! 所以 [`supported`] 仍返回 false：拿不到 `ActiveSandbox`，
//! `ctx.sandboxed` 保持 false，决策链行为不变 —— 半成品绝不谎报。
//!
//! 为什么先啃令牌：它是后续所有步骤的前提，且**能独立验证**（造完读回
//! 它的完整性级别断言是 Low），不像 spawn 集成要连着管道/超时一起才测
//! 得动。M1 在 Windows CI 上把这块的 FFI 签名和运行时行为都钉死，M2
//! 接 spawn 时就不必再怀疑令牌这一环。

#![allow(clippy::disallowed_methods)]

/// 已激活的 Windows 沙箱：一枚受限 Low 令牌 + 已打标签的目录清单。
///
/// 拿到它意味着授权序列（打标签 + 建令牌，见 SANDBOX_WINDOWS.md §2）
/// 全部成功。之后每次 spawn 复用这枚令牌；`Drop` 时把标签回滚干净
/// （对称于激活时的 `authorize_writable`）。
pub(crate) struct WinSandbox {
    token: token::OwnedToken,
    /// 激活时打了 Low 标签的目录，`Drop` 逐个撤回。
    labeled: Vec<std::path::PathBuf>,
    /// 标签清单的落盘位置，回滚时同步清账。
    ledger_path: std::path::PathBuf,
}

/// 单条命令输出的内存上限，同 proc.rs 的 DEFAULT_MAX_OUTPUT。
const MAX_OUTPUT: usize = 8 * 1024 * 1024;

impl WinSandbox {
    /// 在这枚令牌下起一条命令。语义见 [`spawn::spawn_with_token`]。
    pub(crate) async fn run(
        &self,
        spec: riot_protocol::tool::ProcessSpec,
        cancel: tokio_util::sync::CancellationToken,
    ) -> std::io::Result<riot_protocol::tool::ProcessOutput> {
        spawn::spawn_with_token(*self.token.0, spec, MAX_OUTPUT, cancel).await
    }
}

impl Drop for WinSandbox {
    fn drop(&mut self) {
        // 撤标签 + 清账。尽力而为：untag 失败记日志，交给下次启动的孤儿
        // 回收兜底（对照 sandbox_labels 的 [约束]）。
        use crate::sandbox_labels::{DirLabeler, LabelLedger};
        let labeler = label::WinLabeler;
        let mut ledger = LabelLedger::load(self.ledger_path.clone());
        for dir in &self.labeled {
            if let Err(e) = labeler.untag(dir) {
                tracing::warn!(dir = %dir.display(), error = %e, "退出时撤标签失败，留给孤儿回收");
            }
            let _ = ledger.forget(dir);
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
    use crate::sandbox_labels::{LabelLedger, authorize_writable};

    let SandboxPolicy::WorkspaceWrite { writable, .. } = policy else {
        return None; // Off 不该走到这里
    };

    let mut ledger = LabelLedger::load(ledger_path.clone());
    if let Err(e) = authorize_writable(writable, &label::WinLabeler, &mut ledger, now_ms) {
        // authorize 内部已回滚它打过的标签。
        tracing::warn!(error = %e, "沙箱打标签失败，本轮不隔离");
        return None;
    }

    let token = match token::create_restricted_low_il() {
        Ok(t) => t,
        Err(e) => {
            // 标签打上了但令牌没建成 —— 手动回滚，别留孤儿。
            tracing::warn!(error = %e, "沙箱建令牌失败，回滚标签、本轮不隔离");
            use crate::sandbox_labels::DirLabeler;
            let labeler = label::WinLabeler;
            for dir in writable {
                let _ = labeler.untag(dir);
                let _ = ledger.forget(dir);
            }
            return None;
        }
    };

    Some(WinSandbox {
        token,
        labeled: writable.clone(),
        ledger_path,
    })
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
    use std::path::Path;

    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{
        ConvertStringSidToSidW, SE_FILE_OBJECT, SetNamedSecurityInfoW,
    };
    use windows::Win32::Security::{
        ACL, ACL_REVISION, AddMandatoryAce, CONTAINER_INHERIT_ACE, GetLengthSid, InitializeAcl,
        LABEL_SECURITY_INFORMATION, OBJECT_INHERIT_ACE, PSID,
    };
    use windows::Win32::System::SystemServices::SYSTEM_MANDATORY_LABEL_NO_WRITE_UP;
    use windows::core::{HSTRING, w};

    /// 给目录打 Low 完整性标签，`no-write-up` 位让低完整性进程能写它。
    ///
    /// 只写 SACL 的 label 部分（`LABEL_SECURITY_INFORMATION`），不碰
    /// DACL / owner —— 那些不是这层要动的。容器继承（子目录/文件跟着
    /// 生效）靠 ACE 的继承标志，`AddMandatoryAce` 带 `OBJECT_INHERIT |
    /// CONTAINER_INHERIT`。
    pub fn tag_low(dir: &Path) -> windows::core::Result<()> {
        unsafe {
            let mut low_sid = PSID::default();
            ConvertStringSidToSidW(w!("S-1-16-4096"), &mut low_sid)?;
            let _guard = LocalSid(low_sid);

            // ACL 要能装下 ACL 头 + 一条 mandatory ACE。ACE 大小 =
            // 固定头 + SID 主体，给足余量按页对齐。
            let acl_bytes = 256usize + GetLengthSid(low_sid) as usize;
            let mut buf = vec![0u8; acl_bytes];
            let acl = buf.as_mut_ptr() as *mut ACL;
            InitializeAcl(acl, acl_bytes as u32, ACL_REVISION)?;

            // 继承标志让目录下新建的文件和子目录自动带上同一条 Low 标签。
            AddMandatoryAce(
                acl,
                ACL_REVISION,
                OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE,
                SYSTEM_MANDATORY_LABEL_NO_WRITE_UP,
                low_sid,
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

    /// 去掉 Low 标签：写一个**空** SACL label，对象回到默认完整性
    /// （Medium）。回滚和孤儿回收都走这条 —— 见 sandbox_labels 里
    /// 「只记路径不记原状」的取舍。
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

    /// LocalFree 守卫，同 token 模块。
    struct LocalSid(PSID);
    impl Drop for LocalSid {
        fn drop(&mut self) {
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.0.0)));
            }
        }
    }

    /// 真实的目录打标签器。把 [`tag_low`] / [`untag`] 接进跨平台的
    /// [`crate::sandbox_labels::DirLabeler`]，好让激活序列的回滚编排
    /// （`authorize_writable`）用上它 —— 那套编排的正确性在 sandbox_labels
    /// 里跨平台测过，这里只负责把 Win32 错误转成 io 错误接上去。
    pub struct WinLabeler;

    impl crate::sandbox_labels::DirLabeler for WinLabeler {
        fn tag(&self, dir: &std::path::Path) -> std::io::Result<()> {
            tag_low(dir).map_err(|e| std::io::Error::other(e.to_string()))
        }
        fn untag(&self, dir: &std::path::Path) -> std::io::Result<()> {
            untag(dir).map_err(|e| std::io::Error::other(e.to_string()))
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
    use std::io::Read;
    use std::os::windows::io::FromRawHandle;
    use std::time::{Duration, Instant};

    use riot_protocol::tool::{ProcessOutput, ProcessSpec};
    use tokio_util::sync::CancellationToken;
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE, SetHandleInformation, HANDLE_FLAGS, HANDLE_FLAG_INHERIT};
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
        GetExitCodeProcess, INFINITE, PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES,
        STARTUPINFOW, WaitForSingleObject,
    };
    use windows::core::{PCWSTR, PWSTR};

    const EXIT_TIMEOUT: i32 = 124;
    const EXIT_CANCELLED: i32 = 130;

    /// 建好、已在跑的子进程。字段都 `Send`（读端是 `File`，句柄搬成
    /// `isize`），好 move 进 `spawn_blocking`。
    struct Spawned {
        process: isize,
        job: isize,
        read_out: std::fs::File,
        read_err: std::fs::File,
    }

    /// 用 `token` 起 `spec`，接管道/超时/进程组，返回输出。
    ///
    /// 语义对齐 `proc.rs::SystemProcessRunner::run`：
    /// - stdout / stderr **并发**读（串行会死锁，见 proc.rs 注释）；
    /// - 等到「进程退出 / 超时 / 取消」任一，**无条件**杀整个 Job（正常
    ///   退出也杀，清掉可能残留的后台子进程）；
    /// - 读任务在杀组之后 await —— 写端全关了 EOF 才来。
    pub(crate) async fn spawn_with_token(
        token: HANDLE,
        spec: ProcessSpec,
        max_output: usize,
        cancel: CancellationToken,
    ) -> std::io::Result<ProcessOutput> {
        let started = Instant::now();
        let timeout = spec.timeout_ms.map(Duration::from_millis);

        // 建进程是同步 Win32，快，直接在异步上下文里做。
        let sp = unsafe { create(token, &spec) }?;
        let (process, job) = (sp.process, sp.job);

        let h_out = tokio::task::spawn_blocking(move || drain(sp.read_out, max_output));
        let h_err = tokio::task::spawn_blocking(move || drain(sp.read_err, max_output));
        let waiter = tokio::task::spawn_blocking(move || unsafe {
            WaitForSingleObject(HANDLE(process as *mut _), INFINITE);
        });

        let ended = tokio::select! {
            _ = waiter => Ended::Exited,
            _ = sleep_opt(timeout) => Ended::TimedOut,
            _ = cancel.cancelled() => Ended::Cancelled,
        };

        // 无条件杀整组（对齐 proc.rs：正常退出 ≠ 后台子进程也退了）。
        unsafe {
            let _ = TerminateJobObject(HANDLE(job as *mut _), 1);
        }

        // 杀组之后再收输出：写端此刻全关，drain 才等得到 EOF。
        let (stdout, out_capped) = h_out.await.map_err(join_err)??;
        let (stderr, err_capped) = h_err.await.map_err(join_err)??;
        if out_capped || err_capped {
            tracing::warn!(program = %spec.program, "沙箱命令输出超上限，已截断");
        }

        let exit_code = match ended {
            Ended::Exited => unsafe { exit_code_of(process) },
            Ended::TimedOut => EXIT_TIMEOUT,
            Ended::Cancelled => EXIT_CANCELLED,
        };
        unsafe {
            let _ = CloseHandle(HANDLE(process as *mut _));
            let _ = CloseHandle(HANDLE(job as *mut _));
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

    /// 同步建进程：建管道、拼命令行/环境、CreateProcessAsUserW、挂 Job、
    /// 关父进程持有的写端。返回 Send 的句柄束。
    unsafe fn create(token: HANDLE, spec: &ProcessSpec) -> std::io::Result<Spawned> {
        unsafe {
            // 写端要能被子进程继承，读端不能（否则子进程持有读端，EOF 不来）。
            let sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: std::ptr::null_mut(),
                bInheritHandle: true.into(),
            };
            let (read_out, write_out) = pipe(&sa)?;
            let (read_err, write_err) = pipe(&sa)?;
            // stdin 给 NUL：一律立即 EOF —— 读 stdin 的命令（cat、等确认的
            // 脚本）不会挂住。对齐 proc.rs 的 Stdio::null()。
            let nul = CreateFileW(
                windows::core::w!("NUL"),
                GENERIC_READ.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                Some(&sa),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
            .map_err(win_err)?;

            let si = STARTUPINFOW {
                cb: std::mem::size_of::<STARTUPINFOW>() as u32,
                dwFlags: STARTF_USESTDHANDLES,
                hStdInput: nul,
                hStdOutput: write_out,
                hStdError: write_err,
                ..Default::default()
            };

            let base: Vec<(String, String)> = std::env::vars().collect();
            let mut env = crate::sandbox_cmdline::build_env_block(&base, &spec.env);
            let mut cmdline: Vec<u16> = crate::sandbox_cmdline::build_command_line(&spec.program, &spec.args)
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
                CREATE_NO_WINDOW | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
                Some(env.as_mut_ptr() as *mut std::ffi::c_void),
                PCWSTR(cwd.as_ptr()),
                &si,
                &mut pi,
            )
            .map_err(win_err)?;

            // 起完就关父进程这边的写端和 NUL —— 写端不关，读端 EOF 永远不来。
            let _ = CloseHandle(write_out);
            let _ = CloseHandle(write_err);
            let _ = CloseHandle(nul);

            let job = CreateJobObjectW(None, PCWSTR::null()).map_err(win_err)?;
            let mut limit = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limit.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limit as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .map_err(win_err)?;
            AssignProcessToJobObject(job, pi.hProcess).map_err(win_err)?;

            ResumeThread(pi.hThread);
            let _ = CloseHandle(pi.hThread);

            Ok(Spawned {
                process: pi.hProcess.0 as isize,
                job: job.0 as isize,
                read_out: std::fs::File::from_raw_handle(read_out.0 as std::os::windows::io::RawHandle),
                read_err: std::fs::File::from_raw_handle(read_err.0 as std::os::windows::io::RawHandle),
            })
        }
    }

    unsafe fn pipe(sa: &SECURITY_ATTRIBUTES) -> std::io::Result<(HANDLE, HANDLE)> {
        unsafe {
            let mut read = HANDLE::default();
            let mut write = HANDLE::default();
            CreatePipe(&mut read, &mut write, Some(sa), 0).map_err(win_err)?;
            // 读端清掉继承标志：只让写端进子进程。
            SetHandleInformation(read, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)).map_err(win_err)?;
            Ok((read, write))
        }
    }

    unsafe fn exit_code_of(process: isize) -> i32 {
        unsafe {
            let mut code = 0u32;
            if GetExitCodeProcess(HANDLE(process as *mut _), &mut code).is_ok() {
                code as i32
            } else {
                -1
            }
        }
    }

    /// 同步读到 EOF 或读满上限。返回 (内容, 是否触上限)。
    fn drain(mut f: std::fs::File, cap: usize) -> std::io::Result<(Vec<u8>, bool)> {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 16 * 1024];
        loop {
            let n = f.read(&mut chunk)?;
            if n == 0 {
                return Ok((buf, false));
            }
            let room = cap.saturating_sub(buf.len());
            if room == 0 {
                return Ok((buf, true));
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

    fn join_err(e: tokio::task::JoinError) -> std::io::Error {
        std::io::Error::other(format!("读取输出的任务异常：{e}"))
    }

    use std::os::windows::ffi::OsStrExt;

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
            let out = spawn_with_token(*tok.0, spec, 1 << 20, CancellationToken::new())
                .await
                .expect("跑得起来");
            assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
            assert!(out.stdout.contains("hi"), "stdout={:?}", out.stdout);
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

    use crate::sandbox_labels::{DirLabeler, LabelLedger, authorize_writable};

    #[tokio::test]
    async fn 低完整性进程只能写打了标签的目录() {
        let base = std::env::temp_dir().join(format!("riot-sbx-e2e-{}", std::process::id()));
        let work = base.join("work");
        let outside = base.join("outside");
        std::fs::create_dir_all(&work).expect("建工作区");
        std::fs::create_dir_all(&outside).expect("建外部目录");

        // 只给 work 打 Low 标签；outside 保持默认（Medium）。
        let mut ledger = LabelLedger::load(base.join("labels.json"));
        let labeler = super::label::WinLabeler;
        authorize_writable(std::slice::from_ref(&work), &labeler, &mut ledger, 0)
            .expect("给 work 打标签");

        let tok = super::token::create_restricted_low_il().expect("造受限 Low 令牌");
        let token = *tok.0; // HANDLE 是 Copy，搬进闭包
        // cwd 用中性的 base（普通 Medium 目录），把「写哪」和「进程 cwd」
        // 两个变量分开 —— 写目标一律用绝对路径。
        let cwd = base.clone();

        async fn write_to(
            token: windows::Win32::Foundation::HANDLE,
            target: &Path,
            cwd: &Path,
        ) -> (i32, String) {
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
            let o = super::spawn::spawn_with_token(token, spec, 1 << 20, CancellationToken::new())
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

        // 回滚标签 + 清理临时目录。
        let _ = labeler.untag(&work);
        let _ = std::fs::remove_dir_all(&base);
    }
}
