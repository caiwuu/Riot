//! 把活外包给沙箱外 daemon 的命令。
//!
//! # 为什么要单独认出这一类
//!
//! OS 沙箱关住的是**进程**，不是**意图**。`docker run -v $HOME:/h …` 里那次
//! 写盘是 VM 里的 daemon 干的，seatbelt 和 Low IL 都看不见 —— 实测在完整
//! 生产 profile 下它成功写进了主目录，而同一个沙箱里 `echo x > $HOME/f`
//! 是 "Operation not permitted"。
//!
//! 边界那一侧已经按 socket 收口了（`riot_runtime` 的 `sandbox_macos` 禁掉
//! unix socket 外连和 Apple Events），于是这类命令在沙箱里会直接失败。
//!
//! # 这张表只是省一次往返
//!
//! 出沙箱的**通用**机制不在这里，而是 `Bash` 的 `sandbox: false`：命令失败
//! 时 `SandboxedRunner` 会把「可能是沙箱拦的」回显给模型（`annotate_denial`），
//! 模型判断之后带这个参数重跑一次，用户点头才执行。那条路覆盖所有情况，
//! 包括我们没想到的。
//!
//! 这张表存在的唯一理由是：有些命令**每一次**都会撞上边界，让它们先失败
//! 一次再重试是纯粹的浪费。所以收进来的门槛是两条同时成立：
//!
//! 1. 在沙箱内 **100% 必然失败**（必须连本机 daemon 的 unix socket）；
//! 2. 高频到不值得每次付一次失败往返。
//!
//! 这是个**封闭**判据，不是「听起来像外包」。`osascript`、`launchctl`、
//! `tmux`、`colima` 都符合第 1 条但不符合第 2 条 —— 它们走通用那条路，多一次
//! 往返完全可以接受，而少一条表项就少一分静默出圈的机会。
//!
//! # 命中之后仍然要问
//!
//! `[约束]` 命中这张表**不等于**跳过确认。`riot_tools` 的 `bash` 把两条路
//! 汇到同一个出口：兜底档（没有规则命中、也不是只读）一律升级成对「全部
//! 放行」免疫的 `SandboxEscape` 询问。所以表项写错的代价是「多问一次」，
//! 不是「静默放行」—— 这也是这张表能存在的前提。
//!
//! 即便如此，仍然不做 `sudo` / `env` 前缀剥离：剥离要猜哪个参数才是真正的
//! 命令（`sudo -u x docker ps` 的第一个非 flag 参数是 `x`），而猜错换来的
//! 收益只是省一次往返，不值得。`sudo docker …` 留在沙箱里失败即可。
//!
//! # 为什么整条命令要一起干净
//!
//! 豁免的粒度是**整条命令**（一个 `bash -c`），不是单条子命令。于是
//! `docker run x && rm -rf /tmp/y` 一旦豁免，那个 `rm` 也在宿主上裸跑了。
//! 所以只在**每一条**子命令都是外包命令或只读命令时才豁免；混进任何别的
//! 写命令就整条留在沙箱里。模型收到 docker 的失败后自己拆开重试即可。

use super::ast::{Analysis, SubCommand, analyze};

/// 每次都会撞上边界、而且高频到不值得先失败一次的命令。
///
/// 只有容器运行时的客户端符合这两条。它们的每一条子命令都要先连
/// `docker.sock` 才能干活，在开发流程里又出现得极频繁。
///
/// 符合第 1 条但**故意不收**的（走 `sandbox: false` 那条通用路）：
///
/// - `osascript` / `open`：Apple Events 已经在 profile 里 deny 了，失败同样
///   确定，但它们在写代码的流程里出现得很少。
/// - `launchctl` / `crontab`：把执行安排到未来，那次执行必然在沙箱外。低频。
/// - `tmux` / `screen`：命令交给早就在跑的 server 执行，那个 server 从来没
///   进过沙箱。低频，而且长期服务本来就该走 `background: true`。
/// - `colima` / `limactl` / `orb`：管 VM 生命周期的，一个项目里跑一两次。
///
/// 不符合第 1 条、因而两条路都不该收的：
///
/// - `ssh`：绝大多数用法是连远端主机，在沙箱内跑得通（走 TCP，而
///   ssh-agent 的 socket 是放行的）。`ssh localhost` 那条窄路仍然开着，
///   记为残余风险。
/// - `kubectl`：对着远端集群，够不到本机文件系统。
/// - `psql` / `mysql`：走 TCP 时沙箱内正常，走 unix socket 时才失败 ——
///   不确定，交给通用路按实际失败处理。
static DELEGATES: &[&str] = &[
    "docker",
    "docker-compose",
    "podman",
    "podman-compose",
    "nerdctl",
];

/// 这条命令该不该被移出沙箱。
///
/// `[约束]` 调用方拿到 `true` 之后**必须同时**做两件事：把
/// `ProcessSpec::sandbox_exempt` 置 true，以及把
/// `PermissionContext::sandboxed` 抹成 false。只做前者 = 命令在宿主裸跑
/// 而决策链以为 OS 挡着；只做后者 = 白问一次然后照样在沙箱里失败。
pub fn escapes_sandbox(command: &str) -> bool {
    match analyze(command) {
        Analysis::Simple(subs) => subs_escape(&subs),
        // 看不懂结构就不豁免。这里的错误方向必须是"留在沙箱里"——
        // `$(...)` 里藏着什么只有运行时知道，而豁免是不可逆的放权。
        Analysis::TooComplex(_) => false,
    }
}

fn subs_escape(subs: &[SubCommand]) -> bool {
    !subs.is_empty()
        && subs.iter().any(is_delegate)
        && subs
            .iter()
            .all(|s| is_delegate(s) || super::readonly::sub_is_read_only(s))
}

fn is_delegate(sub: &SubCommand) -> bool {
    // 未加引号的 glob 展开成什么不知道，别拿它换沙箱外的执行权。
    if sub.has_unquoted_glob {
        return false;
    }
    DELEGATES.contains(&basename(&sub.name))
}

/// 命令名的最后一段。`/usr/local/bin/docker` → `docker`。
///
/// 绝对路径要认：那是模型很自然会写出的形态，而精确等值匹配会把它漏掉 ——
/// 漏了的后果是 docker 留在沙箱里失败，方向安全但没必要。
fn basename(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 容器运行时被认出来() {
        for cmd in [
            "docker run --rm alpine echo hi",
            "docker compose up -d",
            "/usr/local/bin/docker ps -a",
            "podman run alpine true",
            "nerdctl ps",
        ] {
            assert!(escapes_sandbox(cmd), "{cmd} 该被移出沙箱");
        }
    }

    /// 低频的外包命令**故意**不在表里：它们走 `sandbox: false` 那条通用路，
    /// 多付一次失败往返换表更小。表越小，写错一条的机会越少。
    #[test]
    fn 低频外包命令留给通用路() {
        for cmd in [
            "osascript -e 'tell application \"Finder\" to activate'",
            "tmux new-session -d 'sleep 1'",
            "launchctl list",
            "colima start",
        ] {
            assert!(!escapes_sandbox(cmd), "{cmd} 该走通用路，不该进表");
        }
    }

    #[test]
    fn 普通命令不受影响() {
        for cmd in [
            "cargo build",
            "rm -rf build",
            "npm install",
            // 名字里带 docker 但不是在跑 docker
            "grep docker Dockerfile",
            "cat docker-compose.yml",
        ] {
            assert!(!escapes_sandbox(cmd), "{cmd} 不该被移出沙箱");
        }
    }

    /// 豁免的粒度是整条命令，所以混进别的写命令就整条不豁免 ——
    /// 否则那个 `rm` 会跟着一起在宿主上裸跑。
    #[test]
    fn 混进写命令就整条留在沙箱里() {
        assert!(!escapes_sandbox("docker run alpine true && rm -rf /tmp/x"));
        assert!(!escapes_sandbox("rm -rf build && docker build ."));
        // 反面：搭只读命令可以，它们本来就不改任何东西
        assert!(escapes_sandbox("ls && docker ps"));
    }

    /// 看不懂的结构一律不豁免。豁免是放权，放权不能建立在猜测上。
    #[test]
    fn 结构看不懂就不豁免() {
        for cmd in [
            "docker run $(cat cmd.txt)",
            "eval docker ps",
            "for i in 1 2; do docker ps; done",
        ] {
            assert!(!escapes_sandbox(cmd), "{cmd} 该留在沙箱里");
        }
    }

    /// `sudo` / `env` 前缀不剥。剥离要猜哪个参数是真命令，猜错就是
    /// 多豁免一条 —— 而这张表的错误方向必须是宁可漏。
    #[test]
    fn 不剥离_sudo_env_前缀() {
        assert!(!escapes_sandbox("sudo docker ps"));
        assert!(!escapes_sandbox("env FOO=bar docker ps"));
        // 真正要防的是这个：把 docker 当成普通参数的命令不能蒙混过关
        assert!(!escapes_sandbox("sudo grep docker /etc/hosts"));
    }
}
