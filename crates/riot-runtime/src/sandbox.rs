//! OS 级沙箱：把命令关进操作系统强制的边界里。
//!
//! # 为什么策略层不够
//!
//! `riot-permissions` 判断的是"这条命令看起来要干什么"。它能拆 Bash AST、
//! 认出 `rm -rf`、追踪重定向，但它读不懂 `python -c "..."` 里那段代码，
//! 也读不懂一个 npm 脚本会在 postinstall 里做什么。策略是**判断**，判断
//! 会错；沙箱是**边界**，边界由内核执行。两层正交，见 ARCHITECTURE.md §9.6。
//!
//! # 这一层换来的不只是安全
//!
//! 更直接的收益是**少打断人**。没有边界的时候只有两个极端：每个写操作都
//! 弹窗，或者「全部放行」等于裸奔。有了边界，中间那档才成立 —— 决策链里
//! 那个 `ctx.sandboxed` 分支（`bash::decide`）就是为这一刻写的：既然 OS
//! 已经挡住了文件系统，剩下那些"没规则命中、也不只读"的命令可以直接放行。
//!
//! `[约束]` **只有沙箱真的生效时才能把 `sandboxed` 置 true。** 平台不支持、
//! 后端不可用、profile / token 构造失败 —— 任何一种情况下谎报都会让策略层
//! 放行一批本该询问的命令，而且悄无声息。见 [`SandboxPolicy::activate`]。
//!
//! # 平台后端
//!
//! 这个文件是**跨平台核心**：策略、激活、[`SandboxedRunner`] 装饰器的形状
//! 三个平台共用。真正把命令关进边界的代码按平台分居后端模块：
//!
//! - macOS：[`crate::sandbox_macos`]（seatbelt / `sandbox-exec`，已落地）
//! - Windows：[`crate::sandbox_win`]（Restricted Token + Low IL，见
//!   docs/SANDBOX_WINDOWS.md；当前 M1，`supported()` 尚未放行）
//! - Linux：未排期

#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use riot_protocol::tool::{ProcessOutput, ProcessRunner, ProcessSpec};
use tokio_util::sync::CancellationToken;

/// 沙箱策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxPolicy {
    /// 不隔离。命令直接跑在宿主上，只有策略层拦着。
    Off,
    /// 读全开、写限于给定目录、网络可选。
    ///
    /// 读之所以全开：模型要读系统头文件、`node_modules`、rustup 工具链，
    /// 收紧读会让编译类命令大面积失败，而读本身不改变任何东西。真正要防的
    /// 是**写**（改配置、删文件、往启动项里塞东西）。这也是 Codex CLI
    /// `workspace-write` 的取舍。
    WorkspaceWrite {
        /// 可写目录。工作区、临时目录、构建缓存都在这里。
        writable: Vec<PathBuf>,
        /// 允许联网。
        ///
        /// 默认允许，和 Codex 相反。理由是 Riot 的联网工具（WebFetch /
        /// WebSearch）本来就走宿主、受权限管，而 Bash 断网会让
        /// `npm install`、`cargo build`（要拉 crates）、`pip install`
        /// 全部失败 —— 第一次用就撞上这个的人会直接把沙箱关掉，
        /// 那还不如给个能用的默认值。要更严的人可以关。
        allow_network: bool,
    },
}

impl SandboxPolicy {
    /// 工作区可写 + 一组常见的构建缓存。给生产装配用。
    ///
    /// `[取舍]` 放开 `~/.cargo`、`~/.npm` 这类缓存是可用性让步：不放的话
    /// 第一条 `cargo build` 就挂在"权限不足"上。代价是模型理论上能改
    /// 那些目录里的配置（比如 `~/.cargo/config.toml` 里的 build 脚本）。
    /// 用真实的失败换理论上的攻击面，这一步值得，但要写明白。
    pub fn workspace_write(workspace: &Path) -> Self {
        let mut writable = vec![workspace.to_path_buf()];
        writable.extend(temp_dirs());
        if let Some(home) = home_dir() {
            for cache in [
                ".cargo",
                ".rustup",
                ".npm",
                ".cache",
                ".pnpm-store",
                ".bun/install/cache",
                "Library/Caches",
                "go/pkg",
            ] {
                writable.push(home.join(cache));
            }
        }
        Self::WorkspaceWrite {
            writable: dedup_existing(writable),
            allow_network: true,
        }
    }

    /// 试着让这条策略生效。
    ///
    /// `None` = 这台机器上做不到（平台不支持、后端不可用）。调用方拿到
    /// `None` 时**必须**把 `PermissionContext::sandboxed` 保持 false ——
    /// 谎报会让决策链放行一批本该问用户的命令。
    pub fn activate(self) -> Option<ActiveSandbox> {
        match self {
            Self::Off => None,
            Self::WorkspaceWrite { .. } if !supported() => None,
            policy => Some(ActiveSandbox { policy }),
        }
    }
}

/// 已经确认能生效的沙箱。拿到它才有资格说"我沙箱着呢"。
#[derive(Debug, Clone)]
pub struct ActiveSandbox {
    policy: SandboxPolicy,
}

impl ActiveSandbox {
    /// 生成 macOS seatbelt profile。只在 macOS 上有意义，供该平台的
    /// 测试与诊断读取；其它平台不提供（它们不用 profile 这套机制）。
    #[cfg(target_os = "macos")]
    pub fn profile(&self) -> String {
        crate::sandbox_macos::profile(&self.policy)
    }
}

/// 这台机器支持沙箱吗。分派到平台后端。
fn supported() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::sandbox_macos::supported()
    }
    #[cfg(windows)]
    {
        crate::sandbox_win::supported()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        false
    }
}

/// 给任意执行器套上沙箱。
///
/// 装饰器而不是改 `SystemProcessRunner`：venv 那层（改 PATH）也是装饰器，
/// 两者正交、能自由组合，而"不沙箱"这条路径上一行沙箱代码都不会跑到。
pub struct SandboxedRunner {
    inner: std::sync::Arc<dyn ProcessRunner>,
    sandbox: ActiveSandbox,
}

impl SandboxedRunner {
    pub fn new(inner: std::sync::Arc<dyn ProcessRunner>, sandbox: ActiveSandbox) -> Self {
        Self { inner, sandbox }
    }
}

#[async_trait]
impl ProcessRunner for SandboxedRunner {
    async fn run(
        &self,
        spec: ProcessSpec,
        cancel: CancellationToken,
    ) -> std::io::Result<ProcessOutput> {
        #[cfg(target_os = "macos")]
        {
            // seatbelt：把命令垫进 sandbox-exec，仍由 inner 起进程。
            let wrapped = crate::sandbox_macos::wrap(&self.sandbox.policy, spec);
            self.inner.run(wrapped, cancel).await
        }
        #[cfg(not(target_os = "macos"))]
        {
            // 非 macOS 目前 activate() 返回 None（见 supported），拿不到
            // ActiveSandbox，这条透传永远跑不到 —— 存在只为能编译。
            // Windows 的 token spawn 落地后（M2）在这里换成后端调用。
            let _ = &self.sandbox;
            self.inner.run(spec, cancel).await
        }
    }
}

/// 临时目录。macOS 的 `TMPDIR` 是 `/var/folders/...`，而它本身是
/// `/private/var/folders/...` 的符号链接 —— seatbelt 按真实路径匹配，
/// 两个都要给，否则写临时文件会莫名其妙被拒。
fn temp_dirs() -> Vec<PathBuf> {
    let mut out = vec![PathBuf::from("/tmp"), PathBuf::from("/private/tmp")];
    let tmp = std::env::temp_dir();
    if let Ok(real) = tmp.canonicalize() {
        out.push(real);
    }
    out.push(tmp);
    out
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// 去重 + 丢掉不存在的。
///
/// 不存在的目录留在 profile 里不会报错，但会让它变长而且难读；真正的理由
/// 是**符号链接**：canonicalize 顺带把 `/var` → `/private/var` 这类解开，
/// 而 seatbelt 匹配的是解开之后的路径。
fn dedup_existing(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::with_capacity(paths.len());
    for p in paths {
        let real = p.canonicalize().unwrap_or(p);
        if !out.contains(&real) {
            out.push(real);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 关掉的策略拿不到_active() {
        assert!(SandboxPolicy::Off.activate().is_none());
    }

    /// 真跑一遍，验证边界确实由 OS 执行。
    ///
    /// 这条测试是这个模块存在的全部意义：profile 拼得再漂亮，只要
    /// `sandbox-exec` 没真拦住，`ctx.sandboxed` 就是在骗决策链。
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn 工作区外的写被内核拒绝() {
        use crate::proc::SystemProcessRunner;

        let work = std::env::temp_dir().join(format!("riot-sbx-{}", std::process::id()));
        std::fs::create_dir_all(&work).expect("建工作区");
        let outside = std::env::temp_dir().join(format!("riot-sbx-out-{}", std::process::id()));
        std::fs::create_dir_all(&outside).expect("建外部目录");

        let policy = SandboxPolicy::WorkspaceWrite {
            writable: vec![work.canonicalize().expect("规范化")],
            allow_network: true,
        };
        let Some(active) = policy.activate() else {
            eprintln!("这台机器没有 sandbox-exec，跳过");
            return;
        };
        let runner =
            SandboxedRunner::new(std::sync::Arc::new(SystemProcessRunner::default()), active);

        let run = |cmd: String| {
            let r = &runner;
            let cwd = work.clone();
            async move {
                r.run(
                    ProcessSpec {
                        program: "/bin/sh".to_owned(),
                        args: vec!["-c".to_owned(), cmd],
                        cwd,
                        env: Vec::new(),
                        timeout_ms: Some(10_000),
                    },
                    CancellationToken::new(),
                )
                .await
                .expect("跑得起来")
            }
        };

        let ok = run(format!("echo hi > {}/inside.txt", work.display())).await;
        assert_eq!(ok.exit_code, 0, "工作区内该写得进去：{}", ok.stderr);
        assert!(work.join("inside.txt").exists());

        let denied = run(format!("echo nope > {}/outside.txt", outside.display())).await;
        assert_ne!(denied.exit_code, 0, "工作区外必须写不进去");
        assert!(
            !outside.join("outside.txt").exists(),
            "文件真的不该被创建出来 —— 沙箱没生效的话它就在那儿"
        );

        // 读不受限：编译类命令要读系统头文件和工具链，收紧读等于什么都跑不了。
        let read = run("cat /etc/hosts > /dev/null".to_owned()).await;
        assert_eq!(read.exit_code, 0, "读该是放开的：{}", read.stderr);

        let _ = std::fs::remove_dir_all(&work);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
