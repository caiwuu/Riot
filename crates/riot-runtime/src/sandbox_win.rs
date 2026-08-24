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

/// 这台机器支持沙箱吗。
///
/// `[约束]` M1 恒 false：令牌能造，但还没接到 spawn 上，没有真实边界。
/// 返回 true 会让 `activate()` 交出 `ActiveSandbox`，决策链据此放宽 ——
/// 那就是在没有边界的情况下谎报。M2 接通 spawn 后改成真正的能力探测。
pub(crate) fn supported() -> bool {
    false
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
