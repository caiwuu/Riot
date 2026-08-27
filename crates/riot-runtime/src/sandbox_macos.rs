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

/// DNS。macOS 的解析走 mDNSResponder 的 unix socket —— 它落在下面那条
/// unix socket 总禁令的范围里，不单独放回来的话整个沙箱直接没网
/// （实测：`curl https://example.com` 报 "Could not resolve host"，
/// 而 `allow_network` 明明是 true）。这是这一段最容易踩空的地方。
const MDNS_SOCKET: &str = "/private/var/run/mDNSResponder";

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

    // cargo 敏感面：可写区之内、写了等于换取沙箱外执行权的路径
    // （config 的 rustc-wrapper、PATH 上的 bin……见 cargo_protected）。
    // 排在 allow 之后 —— SBPL 后写覆盖先写，这段必须压住上面的放行。
    // deny 按路径匹配，不要求文件存在：连"创建一个新的 config.toml"
    // 一并挡住（实测），所以 macOS 不需要 Windows 那样的预建。
    if let Some((_, protected)) = crate::sandbox::cargo_protected() {
        p.push_str(&deny_section(protected.iter().map(|pp| pp.path.as_path())));
    }

    p.push_str(&unix_socket_section());

    // Apple Events：和 unix socket 同一类外包通道，只是接收方从 daemon
    // 换成**另一个 App**。`osascript -e 'tell application "Finder" to …'`
    // 让 Finder 去写文件，Finder 不在沙箱里；`do shell script … with
    // administrator privileges` 更是直接经 Authorization Services 提权。
    //
    // `[约束]` 按命令名挡不住这一类。`osascript` 只是最显眼的入口，Python
    // 的 appscript、任何脚本里嵌的一句 `osascript` 都走同一条 mach 通道，
    // 而那些是**间接**调用，命令分析器看不见。OS 这层管的就是间接那半 ——
    // 和 cargo 敏感面的分工一模一样。
    //
    // Claude Code 默认也关着（`allowAppleEvents: false`），它的文档把打开
    // 的后果写得很直白：removes code-execution isolation。
    p.push_str("(deny appleevent-send)\n");

    // 排在上面几段**之后**：断网档要连 DNS 和 ssh-agent 一起断掉，
    // 那两条放行不能把它捅漏。
    if !allow_network {
        p.push_str("(deny network*)\n");
    }
    p
}

/// 禁止连接 unix domain socket。
///
/// # 这一段挡的是什么
///
/// 文件系统的边界只管得住**被包住的那个进程自己**写盘。unix socket 是它
/// 绕过这一点的通道：把活外包给一个沙箱外的 daemon，由那个 daemon 以完整
/// 权限去写。`docker` 是最典型的 —— 实测在完整生产 profile 下
/// `docker run -v $HOME:/h alpine sh -c 'echo x > /h/f'` 成功把文件写进了
/// 主目录，而同一个沙箱里 `echo x > $HOME/f` 是 "Operation not permitted"。
/// 写是 VM 里的 daemon 干的，seatbelt 根本看不见。
///
/// 同类的还有 ssh-agent（以你的身份认证到任何主机）、已在跑的 tmux server
/// （在沙箱外执行命令）、本地数据库的 `COPY TO '/path'`。**按 socket 挡是
/// 唯一收敛的做法** —— 按命令名挡要跟 `-v` / `--mount` / compose YAML 三套
/// 语法赛跑，而漏一条的表现是静默放行。
///
/// `[约束]` 只挡 `network-outbound`（connect），不挡 bind。沙箱内的进程自己
/// 建一个 socket 不构成外包 —— 挡了只会让本地起服务的测试莫名其妙失败。
///
/// `[取舍]` ssh-agent 放回来了，尽管它确实是一条外包通道。理由是代价不对等：
/// 挡住它，用私钥在 agent 里的人（大多数）连 `git push` 都跑不了，而
/// **Riot 没有「沙箱外重试」机制**，用户唯一的出路是把沙箱整个关掉 ——
/// 那正是模块头那句「第一次用就撞上这个的人会直接把沙箱关掉」。而放回来
/// 并没有新增能力：今天的沙箱本来就没挡它。这条记为残余风险。
fn unix_socket_section() -> String {
    let mut s = String::from("(deny network-outbound (subpath \"/\"))\n");
    s.push_str(&format!(
        "(allow network-outbound (literal {}))\n",
        sbpl_str(MDNS_SOCKET)
    ));
    // 路径每次登录都变（`/private/tmp/com.apple.launchd.XXXX/Listeners`），
    // 只能现读现拼。没有这个变量就不放 —— 没有 agent 也就无从谈起。
    if let Some(sock) = std::env::var_os("SSH_AUTH_SOCK") {
        s.push_str(&format!(
            "(allow network-outbound (literal {}))\n",
            sbpl_str(&sock.to_string_lossy())
        ));
    }
    s
}

/// 敏感面的 deny 段。`subpath` 对文件路径同样生效（匹配它自身，实测），
/// 不用按类型分 `literal`/`subpath` 两种子句。
fn deny_section<'a>(paths: impl Iterator<Item = &'a Path>) -> String {
    let mut s = String::from("(deny file-write*\n");
    for p in paths {
        s.push_str("  (subpath ");
        s.push_str(&sbpl_str(&p.to_string_lossy()));
        s.push_str(")\n");
    }
    s.push_str(")\n");
    s
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
        // 比的是断网那条整句，不是 "deny network" 前缀 —— unix socket 段里
        // 的 `(deny network-outbound …)` 也含那个前缀，用前缀会把两件事混掉。
        assert!(!p.contains("(deny network*)"), "默认不断网");
    }

    #[test]
    fn 断网策略写进_profile() {
        let p = profile(&SandboxPolicy::WorkspaceWrite {
            writable: vec![],
            allow_network: false,
        });
        assert!(p.contains("(deny network*)"));
    }

    /// unix socket 外连要挡住，DNS 要放回来，而且顺序不能反 ——
    /// SBPL 后写覆盖先写，allow 写在 deny 前面的话 DNS 会被一起掐掉，
    /// 表现是"沙箱一开就没网"，而 `allow_network` 明明是 true。
    #[test]
    fn unix_socket_外连被挡住而_dns_放回来() {
        let p = profile(&SandboxPolicy::WorkspaceWrite {
            writable: vec![],
            allow_network: true,
        });

        let deny = p
            .find("(deny network-outbound (subpath \"/\"))")
            .expect("要挡住 unix socket 外连");
        let dns = p
            .find(&format!(
                "(allow network-outbound (literal {}))",
                sbpl_str(MDNS_SOCKET)
            ))
            .expect("DNS 的 socket 要放回来");
        assert!(dns > deny, "DNS 的 allow 必须写在 deny 之后：{p}");
        assert!(
            !p.contains("(deny network*)"),
            "只掐 unix socket，TCP 不受影响"
        );
    }

    /// Apple Events 是 unix socket 之外的另一条外包通道：让别的 App 去做事，
    /// 而那些 App 不在沙箱里。按命令名挡不住间接调用，只能在这层挡。
    #[test]
    fn apple_events_默认关掉() {
        for allow_network in [true, false] {
            let p = profile(&SandboxPolicy::WorkspaceWrite {
                writable: vec![],
                allow_network,
            });
            assert!(
                p.contains("(deny appleevent-send)"),
                "allow_network={allow_network} 时也要关掉 Apple Events：{p}"
            );
        }
    }

    /// 断网档要把上面那两条放行一并盖掉。`(deny network*)` 排在最后，
    /// 顺序反了的话"断网"会漏出 DNS 和 ssh-agent 两个洞。
    #[test]
    fn 断网档压在_unix_socket_段之后() {
        let p = profile(&SandboxPolicy::WorkspaceWrite {
            writable: vec![],
            allow_network: false,
        });
        let dns = p.find("(allow network-outbound").expect("有放行段");
        let off = p.find("(deny network*)").expect("有断网段");
        assert!(off > dns, "断网必须压在放行之后：{p}");
    }

    /// cargo 敏感面的 deny 段。真机行为（build 正常、写 config/bin 被拒、
    /// **创建不存在的文件也被拒**）已人工验证；这里钉住 profile 的形状。
    #[test]
    fn cargo_敏感面的_deny_段在_allow_之后() {
        let s = deny_section(
            [
                Path::new("/h/.cargo/bin"),
                Path::new("/h/.cargo/config.toml"),
            ]
            .into_iter(),
        );
        assert_eq!(
            s,
            "(deny file-write*\n  (subpath \"/h/.cargo/bin\")\n  (subpath \"/h/.cargo/config.toml\")\n)\n"
        );

        // 有 ~/.cargo 的机器上（开发机与 CI 都是 Rust 环境），deny 段要
        // 真进 profile，且排在 allow 段之后 —— SBPL 后写覆盖先写，顺序
        // 反了 allow 会把 deny 盖掉，敏感面静默重新可写。
        if let Some((cargo, _)) = crate::sandbox::cargo_protected() {
            let dir = tempfile::tempdir().expect("临时目录");
            let p = profile(&policy_for(dir.path()));
            let allow_pos = p.find("(allow file-write*\n").expect("有 allow 段");
            let deny_pos = p.rfind("(deny file-write*\n").expect("有敏感面 deny 段");
            assert!(deny_pos > allow_pos, "deny 必须写在 allow 之后：{p}");
            assert!(
                p.contains(&format!(
                    "(subpath {})",
                    sbpl_str(&cargo.join("config.toml").to_string_lossy())
                )),
                "config.toml 要在 deny 段里：{p}"
            );
        }
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
            sandbox_exempt: false,
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
