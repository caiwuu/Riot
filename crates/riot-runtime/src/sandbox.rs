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
//! - Windows：[`crate::sandbox_win`]（Restricted Token + Low IL，已落地，
//!   见 docs/SANDBOX_WINDOWS.md）
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
    /// 第一条 `cargo build` 就挂在"权限不足"上。**整表收窄成"只列 cargo
    /// 真实要写的子路径"是修不了的**：cargo 的锁文件家族一直在长
    /// （`.package-cache`、`.package-cache-mutate`、`.global-cache`……），
    /// 猜漏一条的表现就是"构建莫名其妙失败"，而那正是用户直接关掉沙箱的
    /// 理由。所以方向反过来 —— 表保持放宽，把**敏感面**排除掉（见
    /// [`cargo_protected`]）：macOS 翻成 profile 末尾的 deny 子句，Windows
    /// 翻成打标签前的保护洞。排除清单是有限且稳定的（config/bin/凭证），
    /// 写路径清单是开放且会长的 —— 排除式站在稳定的那一边。
    ///
    /// `[约束]` 边界之内仍留着通往边界之外的路：**工作区里**的
    /// `.git/hooks/` 和 `.riot/hooks.json` —— 写了它们，下一次提交 / 对话
    /// 轮就在沙箱外执行任意代码。它们不能进 OS 排除面：装 husky、写
    /// hooks 是高频合法操作，OS 级 deny 会造出「用户点了允许、命令照样
    /// 失败」。这几类只能靠决策链挡明写
    /// （`riot_permissions::bash::write_targets`），间接写入是接受的残余
    /// 风险。cargo 敏感面则相反 —— 让 agent 改全局 cargo 配置的合法场景
    /// 近乎不存在，批准后沙箱内失败的报错清晰（带路径的权限错误），
    /// 用户有出路（自己改/关沙箱），所以值得用 OS 挡住**间接写**这条
    /// 没有任何确认机会的静默路径。
    pub fn workspace_write(workspace: &Path) -> Self {
        let mut writable = vec![workspace.to_path_buf()];
        // temp 的处理平台不同：macOS/Unix 直接放开全局 temp（seatbelt 的
        // 授权只对被包进程生效，不影响别人）；Windows **不**放全局 %TEMP%
        // —— 那里的 Low 标签是对象属性，会让全机所有 Low 进程都能写它
        // （§2）。Windows 的会话专属 temp 子目录由 sandbox_win::activate
        // 现建现打标签、退出即删。
        #[cfg(not(windows))]
        writable.extend(temp_dirs());

        // 相对主目录的构建缓存，按平台各一张表。差异有两层：一是路径约定
        // 不同（Unix 系工具直接在 ~ 下建点目录，Windows 的 npm/pip/pnpm 走
        // %LOCALAPPDATA%）；二是**授权模型不同** —— seatbelt 的授权只对被
        // 包的那个进程生效，放宽 `.rustup` 对宿主机零影响；Windows 的 Low
        // 标签是对象属性，对所有进程生效（见下方 Windows 表的约束）。
        // 表可以放心列宽：dedup_existing 只保留真实存在的目录。
        // `.cargo` 整树进表，但它的敏感面（bin、config、凭证 —— 见
        // cargo_protected）在 macOS 由 profile 末尾的 deny 子句压掉：
        // 写它们换到的是**沙箱外**的执行权，放行等于边界自己开门。
        #[cfg(not(windows))]
        const HOME_CACHES: &[&str] = &[
            ".cargo",
            ".rustup",
            ".npm",
            ".cache",
            ".pnpm-store",
            ".bun/install/cache",
            "Library/Caches",
            "go/pkg",
        ];
        // `[约束]` Windows 的表**不收含用户 PATH 可执行文件的目录**。Low
        // 标签是对象属性：从带 Low 标签的 exe 启动的进程，令牌会被降到
        // Low，之后写任何默认完整性的位置都被 MIC 拒绝 —— 也就是说给
        // `~/.rustup` 打标签的瞬间，用户自己终端里的 cargo/rustc 就全废了
        // （2026-08-25 真实事故：沙箱一激活，宿主机 cargo 全局 os error 5，
        // 残留后重启、重装 rustup 都救不回来）。这和 §2 里 temp 不打全局
        // 标签是同一条原则，只是后果从"扩大攻击面"升级成"启动即降权"。
        //
        // 逐条说明：
        // - `.rustup` 不进表：正常构建对它只读，读不受 MIC 限制。代价是
        //   沙箱内 rust-toolchain.toml 触发的 rustup 自动装工具链会失败，
        //   报错清晰、模型会转告用户 —— 比全机工具链静默降权好接受。
        // - `pnpm`（%LOCALAPPDATA%\pnpm）不进表：它的根目录就是全局 bin
        //   （pnpm.exe 在 PATH 上）。代价是沙箱内 `pnpm add -g` 和写
        //   store（默认在 pnpm\store）会失败 —— 已知限制，等文件级豁免
        //   机制再收编。
        // - `.cargo` 必须进表（构建要写 registry 和 .package-cache 锁），
        //   它的敏感面（bin、config、凭证、env，见 cargo_protected）由
        //   WinLabeler 打"保护洞"豁免，见 sandbox_win::label。
        #[cfg(windows)]
        const HOME_CACHES: &[&str] = &[
            ".cargo",
            ".bun/install/cache",
            "go/pkg",
            "AppData/Local/npm-cache",
            "AppData/Local/pip/Cache",
            "AppData/Local/pnpm-cache",
        ];
        if let Some(home) = home_dir() {
            for cache in HOME_CACHES {
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
    /// `None` = 这台机器上做不到（平台不支持、后端不可用、Windows 打标签
    /// 或建令牌失败）。调用方拿到 `None` 时**必须**把
    /// `PermissionContext::sandboxed` 保持 false —— 谎报会让决策链放行
    /// 一批本该问用户的命令。
    ///
    /// `setup` 携带 Windows 激活需要的东西（标签清单路径、当前时间）；
    /// macOS 用不上（seatbelt 不打持久标签），忽略。
    pub fn activate(self, setup: SandboxSetup) -> Option<ActiveSandbox> {
        if matches!(self, Self::Off) {
            return None;
        }
        #[cfg(target_os = "macos")]
        {
            let _ = setup;
            if !crate::sandbox_macos::supported() {
                return None;
            }
            // `[约束]` 光有 sandbox-exec 还不够，profile 得真能被它接受。
            // 一个带换行或怪字符的工作区路径就能让整份 SBPL 解析失败，而
            // 那时候 `sandboxed` 已经报成 true —— 决策链按"OS 挡着"放行了
            // 一批命令，然后每一条都死在 "failed to parse"。方向是安全的
            // （什么都跑不了），但用户看到的是应用坏了。冒烟一次，几毫秒，
            // 而且只在会话第一次激活时付。
            if !crate::sandbox_macos::profile_accepted(&self) {
                tracing::warn!("seatbelt profile 没被 sandbox-exec 接受，本轮不隔离");
                return None;
            }
            Some(ActiveSandbox { policy: self })
        }
        #[cfg(windows)]
        {
            crate::sandbox_win::activate(&self, setup.ledger_path, setup.now_ms)
                .map(|win| ActiveSandbox { win })
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = (self, setup);
            None
        }
    }
}

/// Windows 激活需要、macOS 忽略的上下文。
///
/// 打成一包传，而不是给 `activate` 加两个 macOS 用不上的参数 —— 平台
/// 差异藏在这个结构里，调用点两边一致。
pub struct SandboxSetup {
    /// Low 标签清单的落盘位置（`<config>/sandbox-labels.json`）。
    pub ledger_path: std::path::PathBuf,
    /// 打标时间（纪元毫秒），进清单做诊断。
    pub now_ms: u64,
}

/// 已经确认能生效的沙箱。拿到它才有资格说"我沙箱着呢"。
///
/// 后端按平台不同：macOS 持策略（每次 spawn 拼 seatbelt profile 垫
/// argv），Windows 持一枚受限令牌 + 标签守卫（每次 spawn 用令牌起进程，
/// Drop 时回滚标签）。其它平台 `activate` 恒返回 None，构造不出它。
pub struct ActiveSandbox {
    #[cfg(target_os = "macos")]
    policy: SandboxPolicy,
    #[cfg(windows)]
    win: crate::sandbox_win::WinSandbox,
}

#[cfg(target_os = "macos")]
impl ActiveSandbox {
    /// 生成 macOS seatbelt profile，供该平台的测试与诊断读取。
    pub fn profile(&self) -> String {
        crate::sandbox_macos::profile(&self.policy)
    }
}

/// 给任意执行器套上沙箱。
///
/// 装饰器而不是改 `SystemProcessRunner`：venv 那层（改 PATH）也是装饰器，
/// 两者正交、能自由组合，而"不沙箱"这条路径上一行沙箱代码都不会跑到。
///
/// `[约束]` 这一层必须装在执行器链条的**最里层**（紧贴真正起进程的那个），
/// venv / 能力包在它外面。理由是平台不对称：macOS 只改 argv，装哪一层都
/// 一样；**Windows 换掉的是"谁来起进程"** —— 它用受限令牌自己调
/// `CreateProcessAsUserW`，压根不会调 `inner`。装在最外层的话，里面那些
/// 改环境变量的装饰器一个都跑不到，表现是「Windows 上一开沙箱（默认开），
/// 会话设的 Python venv 和能力包就静默失效」。装在最里层则两个平台一致：
/// 外层改完 env 的 spec 原样落到这里。
pub struct SandboxedRunner {
    // Windows 用令牌自己起进程（WinSandbox::run），不装饰 inner —— 于是
    // inner 在这个平台无人读。macOS（垫 argv 交 inner 跑）和其它平台
    // （透传）都读它。按平台豁免，而不是删字段（删了 macOS 就没法跑了）。
    #[cfg_attr(windows, allow(dead_code))]
    inner: std::sync::Arc<dyn ProcessRunner>,
    /// `Arc` 而不是独占：沙箱按**会话**激活一次、跨轮复用。Windows 上
    /// 激活要给可写目录打 Low 标签，而 `SetNamedSecurityInfoW` 会把可继承
    /// ACE 传播到已有子对象 —— `~/.cargo` 的 registry 缓存动辄十万文件，
    /// 每轮打一次撤一次是实打实的卡顿。见 `session.rs` 的沙箱缓存。
    sandbox: std::sync::Arc<ActiveSandbox>,
}

impl SandboxedRunner {
    pub fn new(
        inner: std::sync::Arc<dyn ProcessRunner>,
        sandbox: std::sync::Arc<ActiveSandbox>,
    ) -> Self {
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
        #[cfg(windows)]
        {
            // Windows 不装饰 argv，而是用受限令牌自己起进程（inner 用不上）。
            self.sandbox.win.run(spec, cancel).await
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            // 其它平台 activate 恒返回 None，构造不出 ActiveSandbox，这条
            // 跑不到 —— 存在只为能编译。
            let _ = &self.sandbox;
            self.inner.run(spec, cancel).await
        }
    }
}

/// 临时目录。macOS 的 `TMPDIR` 是 `/var/folders/...`，而它本身是
/// `/private/var/folders/...` 的符号链接 —— seatbelt 按真实路径匹配，
/// 两个都要给，否则写临时文件会莫名其妙被拒。
///
/// 只非 Windows 用：Windows 不放全局 temp（见 workspace_write）。
#[cfg(not(windows))]
fn temp_dirs() -> Vec<PathBuf> {
    let mut out = vec![PathBuf::from("/tmp"), PathBuf::from("/private/tmp")];
    let tmp = std::env::temp_dir();
    if let Ok(real) = tmp.canonicalize() {
        out.push(real);
    }
    out.push(tmp);
    out
}

/// `~/.cargo` 边界内的**敏感面**：可写区之内、但写它等于换取**沙箱外**
/// 执行权的路径。macOS 翻成 profile 末尾的 deny 子句
/// （`sandbox_macos::profile`），Windows 翻成打标签前的保护洞
/// （`sandbox_win` 的 `WinLabeler`）。返回 `(cargo home, 敏感子路径)`；
/// 机器上没有 `~/.cargo` 就是 `None`。
///
/// `[约束]` 清单按「写它能换到什么」筛，不是按「听起来重要」筛：
/// - `bin`：用户 PATH 上的 cargo/rustc（rustup shim 全家）。可写 = 顶一个
///   假 `cargo` 等用户在沙箱外执行；Windows 上还叠加「Low 标签启动即降权」
///   （见 `sandbox_win::label`）。
/// - `config.toml` / `config`（旧名，cargo 至今兼容读）：
///   `build.rustc-wrapper`、`[target].runner`、`[source]` 源替换 ——
///   写一行，用户下次在沙箱外 `cargo build` 就执行任意代码。
/// - `credentials.toml` / `credentials`（旧名）：发布凭证。
/// - `env`：rustup 生成、被 shell rc `source`，写它等于写 shell rc。
///
/// `[取舍]` `registry/` 不排除：构建要往里下载解压，排除它等于沙箱内装
/// 不了新依赖，主用例直接废掉。代价是 **src 缓存投毒**（改解压后的源码，
/// 等用户沙箱外构建时执行）仍然开着 —— cargo 对解压产物不做完整性校验，
/// 这条在下游堵不干净，记档见 docs/SANDBOX_WINDOWS.md §2 残余风险。
/// `.crates.toml`/`.crates2.json`（install 记账）同理不收：写它们骗不来
/// 执行权。
///
/// 只在有沙箱后端的平台编译（macOS 的 profile、Windows 的洞）——
/// 其它平台没有调用方，留着就是死代码告警。
#[cfg(any(target_os = "macos", windows))]
pub(crate) fn cargo_protected() -> Option<(PathBuf, Vec<ProtectedPath>)> {
    // canonicalize 对齐 dedup_existing 的形态 —— Windows 的洞靠
    // 「父目录路径逐字节相等」匹配（`WinLabeler::holes_of`），一边规范化
    // 一边不规范化的话，洞永远匹配不上且没有任何报错。
    let cargo = home_dir()?.join(".cargo").canonicalize().ok()?;
    let protected = vec![
        ProtectedPath::dir(cargo.join("bin")),
        ProtectedPath::file(cargo.join("config.toml")),
        ProtectedPath::file(cargo.join("config")),
        ProtectedPath::file(cargo.join("credentials.toml")),
        ProtectedPath::file(cargo.join("credentials")),
        ProtectedPath::file(cargo.join("env")),
    ];
    Some((cargo, protected))
}

/// [`cargo_protected`] 清单里的一条。
///
/// `is_dir` 是**约定的**类型而不是现场 stat 出来的：Windows 侧对不存在的
/// 路径要先预建再打洞（洞对不存在的对象没处打标，而"不存在"本身就是
/// 缺口 —— 沙箱进程可以在 Low 的 `.cargo` 里创建 `config.toml`，内容照样
/// 被沙箱外的 cargo 读走），预建时必须知道建空目录还是空文件。
#[cfg(any(target_os = "macos", windows))]
pub(crate) struct ProtectedPath {
    pub path: PathBuf,
    /// macOS 用不上（deny 按路径匹配、不要求存在，subpath 通吃文件和目录），
    /// 只有 Windows 的预建读它。
    #[cfg_attr(not(windows), allow(dead_code))]
    pub is_dir: bool,
}

#[cfg(any(target_os = "macos", windows))]
impl ProtectedPath {
    fn dir(path: PathBuf) -> Self {
        Self { path, is_dir: true }
    }
    fn file(path: PathBuf) -> Self {
        Self { path, is_dir: false }
    }
}

/// 用户主目录。Windows 的约定是 `USERPROFILE`；`HOME` 是 Unix 的，
/// GUI 启动的 Windows 进程环境里通常没有它（Git Bash 会设，但从宿主
/// 起的内核继承不到）—— 读错变量的后果是缓存目录一条都进不了
/// writable，沙箱下第一条 `cargo build` 就死在"写不了缓存"上。
fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    return std::env::var_os("USERPROFILE").map(PathBuf::from);
    #[cfg(not(windows))]
    std::env::var_os("HOME").map(PathBuf::from)
}

/// 去重 + 丢掉不存在的（canonicalize 失败即视为不存在）。
///
/// 丢不存在的不是洁癖：Windows 给目录打 Low 标签
/// （SetNamedSecurityInfoW）对不存在的路径直接失败，而授权是全有或
/// 全无 —— 一条不存在的缓存路径就能让整个沙箱永远激活不了。macOS
/// 无所谓（profile 里多几条不存在的路径不报错），但两个平台走同一条
/// 装配，按严的那个来。canonicalize 还顺带把符号链接解开
/// （`/var` → `/private/var`），seatbelt 匹配的是解开之后的路径。
fn dedup_existing(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::with_capacity(paths.len());
    for p in paths {
        let Ok(real) = p.canonicalize() else { continue };
        if !out.contains(&real) {
            out.push(real);
        }
    }
    out
}

/// 启动时回收上次进程残留的沙箱标签。非 Windows 空操作（seatbelt 不留
/// 持久状态）。
///
/// `[约束]` 在**任何会话激活之前**调一次 —— 此刻本进程没有活跃引用，
/// 撤残留标签是安全的。跨进程互斥（同机双开内核）由
/// `sandbox_win::recover_orphans` 的独占锁保证。
pub fn recover_orphan_labels(ledger_path: &Path) {
    #[cfg(windows)]
    crate::sandbox_win::recover_orphans(ledger_path);
    #[cfg(not(windows))]
    let _ = ledger_path;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用的激活上下文。macOS 忽略它，Windows 会用 ledger_path ——
    /// 指向临时目录，测试不污染真实配置。
    fn test_setup() -> SandboxSetup {
        SandboxSetup {
            ledger_path: std::env::temp_dir().join(format!(
                "riot-sbx-labels-{}.json",
                std::process::id()
            )),
            now_ms: 0,
        }
    }

    #[test]
    fn 关掉的策略拿不到_active() {
        assert!(SandboxPolicy::Off.activate(test_setup()).is_none());
    }

    /// 不存在的路径必须被丢掉 —— Windows 上给不存在的目录打标签直接
    /// 失败，授权全有或全无，留着它 = 沙箱永远激活不了。
    #[test]
    fn dedup_existing_丢掉不存在的并去重() {
        let base = std::env::temp_dir().join(format!("riot-dedup-{}", std::process::id()));
        std::fs::create_dir_all(&base).expect("建目录");
        let missing = base.join("并不存在的子目录");

        let out = dedup_existing(vec![base.clone(), missing.clone(), base.clone()]);

        assert_eq!(out.len(), 1, "重复的合一条、不存在的丢掉：{out:?}");
        assert!(!out.contains(&missing));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 敏感面清单是两平台共用的单一来源（macOS 的 deny 段 / Windows 的
    /// 保护洞）。钉住内容：少一条，对应的注入路径就静默重开。
    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn cargo_敏感面清单覆盖执行权路径() {
        let Some((cargo, protected)) = cargo_protected() else {
            eprintln!("这台机器没有 ~/.cargo，跳过");
            return;
        };
        let names: Vec<_> = protected
            .iter()
            .filter_map(|p| p.path.file_name().and_then(|n| n.to_str()))
            .collect();
        for required in [
            "bin",
            "config.toml",
            "config",
            "credentials.toml",
            "credentials",
            "env",
        ] {
            assert!(names.contains(&required), "清单少了 {required}：{names:?}");
        }
        for p in &protected {
            // bin 是目录、其余是文件 —— Windows 预建靠这个约定。
            let is_bin = p.path.file_name().is_some_and(|n| n == "bin");
            assert_eq!(p.is_dir, is_bin, "{} 的类型约定错了", p.path.display());
            assert!(
                p.path.starts_with(&cargo),
                "洞必须在 .cargo 之内，否则 Windows 的 holes_of 匹配不上"
            );
        }
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
        let Some(active) = policy.activate(test_setup()) else {
            eprintln!("这台机器没有 sandbox-exec，跳过");
            return;
        };
        let runner = SandboxedRunner::new(
            std::sync::Arc::new(SystemProcessRunner::default()),
            std::sync::Arc::new(active),
        );

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

    /// Windows：走**完整生产装配**（activate → SandboxedRunner → run）
    /// 验证边界。sandbox_win 的 e2e 是手动串底层机制；这条多覆盖 activate
    /// 的装配（建会话 temp、NoNet 检查、SandboxedRunner 的令牌分派、
    /// TMP 注入、Drop 回滚）—— 也就是 session.rs 真正会走的那条路。
    #[cfg(windows)]
    #[tokio::test]
    async fn windows_经装配路径的沙箱边界() {
        use crate::proc::SystemProcessRunner;

        let base = std::env::temp_dir().join(format!("riot-sbx-integ-{}", std::process::id()));
        let work = base.join("work");
        let outside = base.join("outside");
        std::fs::create_dir_all(&work).expect("建工作区");
        std::fs::create_dir_all(&outside).expect("建外部目录");

        let policy = SandboxPolicy::WorkspaceWrite {
            writable: vec![work.clone()],
            allow_network: true,
        };
        let setup = SandboxSetup {
            ledger_path: base.join("labels.json"),
            now_ms: 0,
        };
        let Some(active) = policy.activate(setup) else {
            panic!("Windows 上 WorkspaceWrite 该激活成功");
        };
        let runner = SandboxedRunner::new(
            std::sync::Arc::new(SystemProcessRunner::default()),
            std::sync::Arc::new(active),
        );

        let run = |target: std::path::PathBuf| {
            let r = &runner;
            let cwd = base.clone();
            async move {
                r.run(
                    ProcessSpec {
                        program: "cmd".to_owned(),
                        args: vec![
                            "/c".to_owned(),
                            "echo".to_owned(),
                            "hi".to_owned(),
                            ">".to_owned(),
                            target.display().to_string(),
                        ],
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

        let inside = work.join("in.txt");
        let ok = run(inside.clone()).await;
        assert_eq!(ok.exit_code, 0, "工作区内该写得进：{}", ok.stderr);
        assert!(inside.exists());

        let out_file = outside.join("out.txt");
        let denied = run(out_file.clone()).await;
        assert_ne!(denied.exit_code, 0, "工作区外必须写不进");
        assert!(!out_file.exists(), "文件不该被创建");

        // 一条自定义命令的小工具，下面几个断言各用各的。
        let exec = |cmd: &str| {
            let r = &runner;
            let cwd = base.clone();
            let cmd = cmd.to_owned();
            async move {
                r.run(
                    ProcessSpec {
                        program: "cmd".to_owned(),
                        args: vec!["/c".to_owned(), cmd],
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

        // 文档 §6 用例 3：TMP/TEMP/TMPDIR 都被重写到会话专属子目录，而且
        // 那里真能写。全局 %TEMP% 没打标签，不重写的话所有临时文件都失败。
        // `^|` 转义：args 对 cmd 是裸拼进命令行的，`|` 不转义就是管道，
        // %TEMP% 展开出来的路径会被当成管道下游的命令去执行。
        let tmp = exec("echo %TMP%^|%TEMP%^|%TMPDIR%").await;
        assert_eq!(tmp.exit_code, 0, "stderr={}", tmp.stderr);
        for (i, seen) in tmp.stdout.trim().split('|').enumerate() {
            assert!(
                seen.contains("riot-sbx-"),
                "第 {i} 个 temp 变量没指到会话子目录：{seen:?}"
            );
        }
        let tmp_write = exec("echo hi > %TMP%\\probe.txt && type %TMP%\\probe.txt").await;
        assert_eq!(
            tmp_write.exit_code, 0,
            "会话 temp 必须可写：{}",
            tmp_write.stderr
        );

        // 文档 §6 用例 2：HKCU 写被拒。这是 Low IL 比 macOS 档多出来的一层
        // 持久化防线（Run 键、文件关联），要有测试钉住。
        let reg = exec("reg add HKCU\\Software\\RiotSandboxProbe /v x /d 1 /f").await;
        assert_ne!(reg.exit_code, 0, "低完整性进程不该写得动 HKCU");

        drop(runner); // 触发 Drop：回滚标签 + 删会话 temp
        let _ = std::fs::remove_dir_all(&base);
    }
}
