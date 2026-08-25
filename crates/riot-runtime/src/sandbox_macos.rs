//! macOS seatbelt 后端。
//!
//! `sandbox-exec` 接一份 SBPL profile，把命令关进"读全开、写限目录"的
//! 边界里。跨平台的策略与激活在 [`crate::sandbox`]，这里只有 macOS 专有的
//! profile 生成和 argv 包装。

#![allow(clippy::disallowed_methods)]

use std::path::Path;

use riot_protocol::tool::ProcessSpec;

use crate::sandbox::SandboxPolicy;

/// macOS 上的沙箱执行器。
const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// 总是允许写的设备节点。
///
/// 不给的话，任何往 `/dev/null` 丢输出的命令（`cmd 2>/dev/null` 是最常见的
/// shell 惯用法）都会直接失败 —— 而那跟安全一点关系都没有。
const DEV_WRITABLE: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
    "/dev/stdout",
    "/dev/stderr",
    "/dev/fd",
    "/dev/tty",
    "/dev/dtracehelper",
];

/// 这台机器支持沙箱吗。`sandbox-exec` 不在就做不到。
pub(crate) fn supported() -> bool {
    Path::new(SANDBOX_EXEC).is_file()
}

/// 这份 profile 真的能被 `sandbox-exec` 接受吗。
///
/// 拿一个立刻成功退出的程序跑一遍，只看退出码。存在的理由是
/// `sandbox-exec` 的解析错误只有一句 "failed to parse"，而且是在**每条
/// 命令**上报 —— 不在激活时验一次的话，一个含怪字符的工作区路径会让
/// `sandboxed` 报成 true、然后所有命令一律失败。
///
/// `[约束]` 用 `/usr/bin/true` 而不是 `true`：profile 里没有对 PATH 的
/// 任何假设，而 `sandbox-exec` 找不到程序时的退出码和 profile 解析失败
/// 撞在一起，会把"这台机器怪"误判成"profile 坏了"。
pub(crate) fn profile_accepted(policy: &SandboxPolicy) -> bool {
    std::process::Command::new(SANDBOX_EXEC)
        .arg("-p")
        .arg(profile(policy))
        .arg("/usr/bin/true")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// 生成 seatbelt profile（SBPL）。
///
/// 规则是**后写的覆盖先写的**：先 `allow default` 放开一切，再 `deny
/// file-write*` 收掉所有写，最后按目录逐个放回来。反过来写的话最后那条
/// deny 会把前面的 allow 全盖掉，表现是"什么都写不了"。
pub(crate) fn profile(policy: &SandboxPolicy) -> String {
    let SandboxPolicy::WorkspaceWrite {
        writable,
        allow_network,
    } = policy
    else {
        // Off 拿不到 ActiveSandbox，见 SandboxPolicy::activate。
        return "(version 1)\n(allow default)\n".to_owned();
    };

    let mut p = String::from("(version 1)\n(allow default)\n(deny file-write*)\n");
    p.push_str("(allow file-write*\n");
    for dir in writable {
        p.push_str("  (subpath ");
        p.push_str(&sbpl_str(&dir.to_string_lossy()));
        p.push_str(")\n");
    }
    for dev in DEV_WRITABLE {
        p.push_str("  (subpath ");
        p.push_str(&sbpl_str(dev));
        p.push_str(")\n");
    }
    p.push_str(")\n");

    if !allow_network {
        p.push_str("(deny network*)\n");
    }
    p
}

/// 把一条命令改写成"在沙箱里跑这条命令"。
pub(crate) fn wrap(policy: &SandboxPolicy, spec: ProcessSpec) -> ProcessSpec {
    let mut args = vec!["-p".to_owned(), profile(policy), spec.program];
    args.extend(spec.args);
    ProcessSpec {
        program: SANDBOX_EXEC.to_owned(),
        args,
        ..spec
    }
}

/// SBPL 字符串字面量。反斜杠和引号要转义，否则一个带空格或引号的路径
/// 会让整份 profile 语法错误 —— 而 `sandbox-exec` 报的错只有一句
/// "failed to parse"，指不回是哪条路径。
fn sbpl_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn policy_for(dir: &Path) -> SandboxPolicy {
        // 只放工作区，不带临时目录和缓存 —— 测的是边界本身。
        SandboxPolicy::WorkspaceWrite {
            writable: vec![dir.canonicalize().expect("规范化")],
            allow_network: true,
        }
    }

    #[test]
    fn profile_把写收紧到给定目录() {
        // 用真实存在的临时目录，不写死 `/tmp`：`policy_for` 要 canonicalize。
        let dir = tempfile::tempdir().expect("临时目录");
        let real = dir.path().canonicalize().expect("规范化");

        let p = profile(&policy_for(dir.path()));

        assert!(p.starts_with("(version 1)\n(allow default)\n(deny file-write*)\n"));
        assert!(
            p.contains(&format!("(subpath {})", sbpl_str(&real.to_string_lossy()))),
            "给定目录要进 profile：{p}"
        );
        assert!(p.contains("(subpath \"/dev/null\")"));
        assert!(!p.contains("deny network"), "默认不断网");
    }

    #[test]
    fn 断网策略写进_profile() {
        let p = profile(&SandboxPolicy::WorkspaceWrite {
            writable: vec![],
            allow_network: false,
        });
        assert!(p.contains("(deny network*)"));
    }

    /// 带空格和引号的路径不能把 profile 撑破。sandbox-exec 的解析错误
    /// 只有一句 "failed to parse"，指不回是哪条路径 —— 只能在这里拦。
    #[test]
    fn 路径里的引号和反斜杠要转义() {
        assert_eq!(sbpl_str(r#"/a b/c"d\e"#), r#""/a b/c\"d\\e""#);
    }

    /// wrap 把命令垫到 sandbox-exec 后面，profile 用 `-p` 传。
    #[test]
    fn wrap_把命令垫进_sandbox_exec() {
        let spec = ProcessSpec {
            program: "/bin/echo".to_owned(),
            args: vec!["hi".to_owned()],
            cwd: PathBuf::from("/tmp"),
            env: vec![],
            timeout_ms: None,
        };
        let wrapped = wrap(
            &SandboxPolicy::WorkspaceWrite {
                writable: vec![],
                allow_network: true,
            },
            spec,
        );
        assert_eq!(wrapped.program, SANDBOX_EXEC);
        assert_eq!(wrapped.args[0], "-p");
        assert!(wrapped.args[1].contains("(deny file-write*)"));
        assert_eq!(wrapped.args[2], "/bin/echo");
        assert_eq!(wrapped.args[3], "hi");
    }
}
