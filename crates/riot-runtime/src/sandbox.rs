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
//! # 它关的是进程，不是意图
//!
//! `[约束]` 边界只管得住**被关住的那个进程自己**动手。一条命令完全可以
//! 不碰边界，而是把活外包给一个沙箱外的 daemon 去干 —— `docker` 是最典型
//! 的，写盘是 VM 干的，seatbelt 从头到尾没看见。这一类只能按**通道**挡：
//! macOS 的 profile 禁掉 unix socket 外连（`sandbox_macos` 的
//! `unix_socket_section`），配套的出口是 `ProcessSpec::sandbox_exempt`。
//! 完整取舍见 ARCHITECTURE.md §9.6.2。
//!
//! # 平台后端
//!
//! 这个文件是**跨平台核心**：策略、激活、[`SandboxedRunner`] 装饰器的形状
//! 三个平台共用。真正把命令关进边界的代码按平台分居后端模块：
//!
//! - macOS：[`crate::sandbox_macos`]（seatbelt / `sandbox-exec`，已落地）
//! - Windows：[`crate::sandbox_win`]（专用本地账户 + 附加 ACE，编排
//!   vendored 的 srt-win，已落地，见 docs/SANDBOX_WINDOWS.md）
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
    /// [`escape_surfaces`]）：macOS 翻成 profile 末尾的 deny 子句，Windows
    /// 翻成 `acl stamp` 的 DENY ACE。排除清单是有限且稳定的
    /// （config/bin/凭证/工具链），写路径清单是开放且会长的 —— 排除式站在
    /// 稳定的那一边。
    ///
    /// `[约束]` **表里加一项，就要问它的敏感面在不在排除清单里。**两者是
    /// 一对：放宽的每一寸都可能盖住一条通往沙箱外的执行路径。Windows 那侧
    /// 曾经按「不 grant 就够不着」省掉过排除面 —— 而 `.cargo` 恰恰是 grant
    /// 的，`(OI)(CI)` 的 ALLOW 一路继承进 `.cargo\bin`，边界自己开了门。
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
        // —— 那是所有用户共用的目录，给沙箱账户加一条可继承的 ALLOW ACE
        // 等于让它能改别人的临时文件。Windows 的会话专属 temp 子目录由
        // sandbox_win::activate 现建现授权、退出即删。
        #[cfg(not(windows))]
        writable.extend(temp_dirs());

        // 相对主目录的构建缓存，按平台各一张表 —— 路径约定不同（Unix 系
        // 工具直接在 ~ 下建点目录，Windows 的 npm/pip/pnpm 走
        // %LOCALAPPDATA%）。表可以放心列宽：dedup_existing 只保留真实存在
        // 的目录。
        // `.cargo` 和 `.rustup` 整树进表，但它们的敏感面（bin、config、凭证、
        // 工具链 —— 见 escape_surfaces）由 deny 压掉：写它们换到的是**沙箱
        // 外**的执行权，放行等于边界自己开门。
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
        // 这张表比 Unix 那张窄两项，两项各有各的理由，而且都**不是**
        // Low IL 时代那个理由了（那时给 `~/.rustup` 打 Low 标签会让宿主
        // 自己的 cargo 全废 —— 2026-08-25 的真实事故；换成专用账户 + 附加
        // ACE 之后，授权只对沙箱账户生效，宿主一点不受影响）。
        //
        // 缺 `.rustup`：整棵树里真正值钱的是 `toolchains`，而它在
        // `escape_surfaces` 的 deny 清单上（沙箱换掉 `toolchains/*/bin/rustc`
        // = 用户下次在沙箱外构建就执行它）。剩下的 `downloads`/`tmp` 单独
        // 放开换不到什么 —— 装工具链照样卡在 `toolchains` 上。所以不进表，
        // 沙箱内装工具链走升级到沙箱外那条路。
        //
        // 缺 pnpm 的 **根目录**：`%LOCALAPPDATA%\pnpm` 就是全局 bin
        // （`pnpm.exe` 在用户 PATH 上），和 `.cargo\bin` 同一类。但它下面的
        // `store` 是纯内容寻址的包缓存，单独进表 —— `pnpm install` 要写它，
        // 而写它换不到 PATH 上的执行权。代价是 `pnpm add -g` 仍然失败。
        //
        // `.cargo` 整树进表（构建要写 registry 和 .package-cache 锁），敏感面
        // 由 `acl stamp` 的 DENY ACE 压掉，见 `escape_surfaces`。
        #[cfg(windows)]
        const HOME_CACHES: &[&str] = &[
            ".cargo",
            ".bun/install/cache",
            "go/pkg",
            "AppData/Local/npm-cache",
            "AppData/Local/pip/Cache",
            "AppData/Local/pnpm-cache",
            "AppData/Local/pnpm/store",
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
    /// `None` = 这台机器上做不到（平台不支持、后端不可用、Windows 上
    /// `srt-win` 没装或授权失败）。调用方拿到 `None` 时**必须**把
    /// `PermissionContext::sandboxed` 保持 false —— 谎报会让决策链放行
    /// 一批本该问用户的命令。
    pub fn activate(self) -> Option<ActiveSandbox> {
        if matches!(self, Self::Off) {
            return None;
        }
        #[cfg(target_os = "macos")]
        {
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
            crate::sandbox_win::activate(&self).map(|win| ActiveSandbox { win })
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = self;
            None
        }
    }
}

/// 已经确认能生效的沙箱。拿到它才有资格说"我沙箱着呢"。
///
/// 后端按平台不同：macOS 持策略（每次 spawn 拼 seatbelt profile 垫
/// argv），Windows 持沙箱账户的 SID 与本会话的授权（每次 spawn 垫成一次
/// `srt-win exec`，Drop 时回收 ACE）。其它平台 `activate` 恒返回 None，
/// 构造不出它。
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
/// venv / 能力包在它外面。两个平台都是「改写 spec 再交给 `inner`」，而
/// 改写会把 `spec.env` 翻走（Windows 翻成 `--env`）—— 装在最外层的话，
/// 里面那些改环境变量的装饰器一个都跑不到，表现是「一开沙箱（默认开），
/// 会话设的 Python venv 和能力包就静默失效」。装在最里层则外层改完 env 的
/// spec 原样落到这里。
pub struct SandboxedRunner {
    // 三个平台都读它：macOS 垫 sandbox-exec、Windows 垫 srt-win，都交给它
    // 起进程；其它平台直接透传。
    inner: std::sync::Arc<dyn ProcessRunner>,
    /// `Arc` 而不是独占：沙箱按**会话**激活一次、跨轮复用。Windows 上
    /// 激活要给可写目录写可继承的 ACE，而 `SetNamedSecurityInfoW` 会把它
    /// 传播到已有子对象 —— `~/.cargo` 的 registry 缓存动辄十万文件，每轮
    /// 授权一次回收一次是实打实的卡顿。见 `session.rs` 的沙箱缓存。
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
        // 明确要求不隔离的命令（`docker` 这类把活外包给沙箱外 daemon 的，
        // 见 `riot_permissions::bash::delegation`）。对它们而言沙箱从来
        // 不是边界，只是一层让命令失败的摩擦 —— 关住客户端进程拦不住
        // daemon 写盘。放它们回宿主，换来的是决策层一次真实的确认。
        //
        // `[约束]` 打这个标记的地方必须同时把 `PermissionContext::sandboxed`
        // 抹成 false。这里不复查（这一层看不到权限上下文），靠 `bash` 工具
        // 把两处判定读同一个函数来保证。
        if spec.sandbox_exempt {
            return self.inner.run(spec, cancel).await;
        }
        #[cfg(target_os = "macos")]
        let out = {
            // seatbelt：把命令垫进 sandbox-exec，仍由 inner 起进程。
            let wrapped = crate::sandbox_macos::wrap(&self.sandbox.policy, spec);
            self.inner.run(wrapped, cancel).await
        };
        #[cfg(windows)]
        let out = {
            // 和 macOS 同构：改写成一次 `srt-win exec`，仍由 inner 起进程。
            // 上一版在这里用受限令牌自己 spawn，管道/超时/取消全是手写的；
            // 换成账户 + ACE 模型之后那 490 行没有存在理由了。
            let wrapped = self.sandbox.win.wrap(spec);
            self.inner.run(wrapped, cancel).await
        };
        #[cfg(not(any(target_os = "macos", windows)))]
        let out = {
            // 其它平台 activate 恒返回 None，构造不出 ActiveSandbox，这条
            // 跑不到 —— 存在只为能编译。
            let _ = &self.sandbox;
            self.inner.run(spec, cancel).await
        };
        Ok(annotate_denial(out?))
    }
}

/// 命令失败得像是被沙箱拒的，就在输出末尾说一声。
///
/// # 为什么非说不可
///
/// 模型看不到沙箱。它拿到的只有一句 `Operation not permitted`，而那句话
/// 在没有沙箱的机器上通常意味着"路径写错了"或"该加 sudo" —— 两个方向都是
/// 错的。Cursor 公开过这个失败模式：agent 会把同一条命令原样再跑一遍，
/// 直到轮次耗尽；他们把沙箱约束回显给模型之后，离线 eval 明显变好。
/// Claude Code 做的是同一件事（把 violation 详情追加到失败输出里）。
///
/// `[取舍]` 判据是**启发式**的，宁可偶尔多说一句。`Permission denied` 在
/// 非沙箱原因下也会出现（跑一个没有执行位的文件），那时这句提示是噪音 ——
/// 但代价只是模型多试一次、拿到同样的错误；反过来漏掉的代价是它在原地
/// 打转直到轮次耗尽。所以措辞是"可能"，并且明说要先判断目标是不是真在
/// 边界之外。
fn annotate_denial(mut out: ProcessOutput) -> ProcessOutput {
    // 超时不是拒绝。成功也不用解释。
    if out.exit_code == 0 || out.timed_out || !looks_denied(&out.stderr) {
        return out;
    }
    if !out.stderr.is_empty() && !out.stderr.ends_with('\n') {
        out.stderr.push('\n');
    }
    out.stderr.push_str(DENIAL_HINT);
    out
}

/// 追加给模型看的那句话。
///
/// 提到具体参数名（`sandbox: false`）而不是泛泛说"可以申请出沙箱"：
/// 模型要的是下一步怎么做，而不是知道有个东西存在。
const DENIAL_HINT: &str = "\n[riot:sandbox] 这条命令跑在 OS 沙箱里，上面的失败可能来自沙箱边界\
（可写范围限于工作区和构建缓存；连接 unix socket 和 Apple Events 一律拒绝，\
所以 docker 这类要跟本机 daemon 通信的工具在沙箱内必然失败）。\
如果目标确实在边界之外而这次操作是必要的，用 `sandbox: false` 重跑一次 —— \
那会在沙箱外执行并请求用户确认。如果失败与边界无关（路径写错、依赖缺失、\
测试真的挂了），照常修就行，别用这个参数。\n";

/// stderr 看起来像不像内核拒绝。
///
/// 三个平台的说法不一样：macOS 的 seatbelt 给 `EPERM`，Windows 的 MIC 给
/// `ERROR_ACCESS_DENIED`。Rust 侧的 `std::io::Error` 又会渲染成 `os error N`。
fn looks_denied(stderr: &str) -> bool {
    const SIGNS: &[&str] = &[
        "operation not permitted",
        "permission denied",
        "access is denied",
        "os error 1)",
        "os error 5)",
        "read-only file system",
    ];
    let lower = stderr.to_ascii_lowercase();
    SIGNS.iter().any(|s| lower.contains(s))
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
/// 执行权的路径。翻成 profile 末尾的 deny 子句
/// （`sandbox_macos::profile`）。返回 `(cargo home, 敏感子路径)`；
/// 机器上没有 `~/.cargo` 就是 `None`。
///
/// `[约束]` 清单按「写它能换到什么」筛，不是按「听起来重要」筛：
/// - `bin`：用户 PATH 上的 cargo/rustc（rustup shim 全家）。可写 = 顶一个
///   假 `cargo` 等用户在沙箱外执行。
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
/// 两个平台都要：macOS 翻成 profile 末尾的 deny 子句
/// （`sandbox_macos::profile`），Windows 翻成 `srt-win acl stamp` 的附加
/// DENY ACE（`sandbox_win::stamp`）。
///
/// `[约束]` Windows 这边**一定要有**。`.cargo` 整树在 grant 列表里，而
/// `acl grant` 写的是 `(OI)(CI)` 可继承 ALLOW —— 它会一路继承进
/// `.cargo\bin`，于是沙箱账户能顶掉那里的 `cargo.exe` / `rustc.exe`
/// （rustup shim 全家，就在用户 PATH 上），用户下一次在**沙箱外**构建就
/// 执行了它。DENY 在 Windows 的 DACL 求值里排在 ALLOW 之前，压得住。
///
/// 这一条曾经漏过：从 Low IL 换到账户模型时，我按「不 grant 就够不着」把
/// Windows 这半删了 —— 而 `.cargo` 恰恰是 grant 的。
pub(crate) fn escape_surfaces() -> Vec<ProtectedPath> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Ok(cargo) = home.join(".cargo").canonicalize() {
        out.push(ProtectedPath::dir(cargo.join("bin")));
        out.push(ProtectedPath::file(cargo.join("env")));
        // cargo 的配置有「现代名」和「无扩展名的老名」两份，两份都在就**用
        // 老的**并警告一句（实测 `warning: both … exist. Using …`）。所以两
        // 份都要 deny：只钉现代名，沙箱建一个老名的就把它盖过去了。
        //
        // `[约束]` 但老名**不能预建**。Windows 侧缺失的 deny 目标由 srt-win
        // 建空 placeholder，而一个空的 `config` 会把用户真实的 `config.toml`
        // 整个屏蔽掉（`credentials` 那对更糟，registry 凭证直接失效），而且
        // placeholder 是永久的。预建的伤害大过它堵的洞。
        for (legacy, modern) in [
            ("config", "config.toml"),
            ("credentials", "credentials.toml"),
        ] {
            let legacy = cargo.join(legacy);
            // 现代名只在老名不存在时才预建 —— 老名在的话，凭空多出一个
            // `config.toml` 只会让 cargo 每次都警告一句，配置还是走老名。
            let plant_modern = !legacy.exists();
            out.push(ProtectedPath::file(legacy).no_plant());
            out.push(ProtectedPath::file(cargo.join(modern)).plant_if(plant_modern));
        }
    }
    // `~/.rustup/toolchains` 整棵。`.cargo/bin` 里的 shim 只是 rustup 代理，
    // 真正被执行的 rustc / cargo 二进制在这里 —— 只钉 `.cargo/bin` 而放开
    // 这棵，等于锁了门开着窗：`~/.rustup` 在 macOS 的可写表里（见
    // `HOME_CACHES`），沙箱换掉 `toolchains/stable-*/bin/rustc`，用户下次在
    // 沙箱外构建就执行了它。
    //
    // `[取舍]` 整棵 deny 会让沙箱内的 `rustup toolchain install` 和
    // `rust-toolchain.toml` 自动装工具链失败（那要写 `toolchains/<名>/`）。
    // 收窄到 `*/bin` 修不了：`lib` 下的 `.dylib` / `.so` 同样由 rustc 加载
    // 执行。所以按「写它能换到什么」的判据整棵进表，装工具链走升级到沙箱外
    // 的那条路。
    if let Ok(rustup) = home.join(".rustup").canonicalize() {
        out.push(ProtectedPath::dir(rustup.join("toolchains")));
    }
    out
}

/// [`escape_surfaces`] 清单里的一条。
///
/// `is_dir` 是**约定的**类型而不是现场 stat 出来的：Windows 要给不存在的
/// 路径也打上 DENY —— ACE 只能写在真实存在的对象上，而"不存在"本身就是
/// 缺口（沙箱账户可以在可写的 `.cargo` 里**创建** `config.toml`，内容照样被
/// 沙箱外的 cargo 读走）。预建交给 srt-win，但建目录还是建文件得我们说。
///
/// macOS 用不上它：seatbelt 的 deny 按路径匹配、不要求对象存在。
pub(crate) struct ProtectedPath {
    pub path: PathBuf,
    pub is_dir: bool,
    /// 不存在时，可不可以建一个空的占位再打 DENY。
    ///
    /// `false` 的含义是「宁可留着这个洞」—— 见 [`escape_surfaces`] 里 cargo
    /// 老配置名那段。Windows 专用；macOS 的 deny 不要求对象存在。
    pub plant: bool,
}

impl ProtectedPath {
    fn dir(path: PathBuf) -> Self {
        Self {
            path,
            is_dir: true,
            plant: true,
        }
    }
    fn file(path: PathBuf) -> Self {
        Self {
            path,
            is_dir: false,
            plant: true,
        }
    }
    fn no_plant(self) -> Self {
        Self {
            plant: false,
            ..self
        }
    }
    fn plant_if(self, plant: bool) -> Self {
        Self { plant, ..self }
    }
}

/// 用户主目录。Windows 的约定是 `USERPROFILE`；`HOME` 是 Unix 的，
/// GUI 启动的 Windows 进程环境里通常没有它（Git Bash 会设，但从宿主
/// 起的内核继承不到）—— 读错变量的后果是缓存目录一条都进不了
/// writable，沙箱下第一条 `cargo build` 就死在"写不了缓存"上。
pub(crate) fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    return std::env::var_os("USERPROFILE").map(PathBuf::from);
    #[cfg(not(windows))]
    std::env::var_os("HOME").map(PathBuf::from)
}

/// 去重 + 丢掉不存在的（canonicalize 失败即视为不存在）。
///
/// 丢不存在的不是洁癖：Windows 给目录写 ACE（SetNamedSecurityInfoW）对
/// 不存在的路径直接失败，而 `acl grant` 是全有或全无 —— 一条不存在的缓存
/// 路径就能让整个沙箱永远激活不了。macOS 无所谓（profile 里多几条不存在的
/// 路径不报错），但两个平台走同一条装配，按严的那个来。canonicalize 还
/// 顺带把符号链接解开（`/var` → `/private/var`），seatbelt 匹配的是解开
/// 之后的路径。
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

/// 启动时回收上次进程残留的沙箱授权。非 Windows 空操作（seatbelt 不留
/// 持久状态）。
///
/// `[约束]` 在**任何会话激活之前**调一次。跨进程互斥由 `srt-win` 自己的
/// 状态库负责（文件锁 + 每次 acl 操作都跑崩溃恢复），这里只是显式触发。
pub fn recover_orphan_sandbox_state() {
    #[cfg(windows)]
    crate::sandbox_win::recover_orphans();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 关掉的策略拿不到_active() {
        assert!(SandboxPolicy::Off.activate().is_none());
    }

    fn out(exit_code: i32, stderr: &str) -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: stderr.to_owned(),
            exit_code,
            timed_out: false,
            duration_ms: 1,
        }
    }

    /// 被拒的失败要带上提示。模型看不到沙箱，只看到一句
    /// `Operation not permitted` —— 那句话在没有沙箱的机器上通常意味着
    /// "路径写错了"或"该加 sudo"，两个方向都会让它原地打转。
    #[test]
    fn 疑似被沙箱拒的失败要带提示() {
        for stderr in [
            "bash: /Users/u/x.txt: Operation not permitted",
            "permission denied while trying to connect to the docker API",
            "Access is denied.",
            "failed to write: Read-only file system",
        ] {
            let got = annotate_denial(out(1, stderr));
            assert!(
                got.stderr.contains("[riot:sandbox]"),
                "{stderr:?} 该带提示：{}",
                got.stderr
            );
            assert!(got.stderr.starts_with(stderr), "原始 stderr 不能被改掉");
        }
    }

    /// 反面三条。加提示的门槛要低到能覆盖真实拒绝，但不能低到每个失败
    /// 都喊一句"可能是沙箱" —— 那样模型会拿 `sandbox: false` 去修一个
    /// 编译错误，白打断用户一次。
    #[test]
    fn 普通失败不加提示() {
        for (code, stderr) in [
            (0, "Operation not permitted"), // 成功了就没什么好解释的
            (1, "error[E0308]: mismatched types"),
            (1, "2 tests failed"),
            (101, ""),
        ] {
            let got = annotate_denial(out(code, stderr));
            assert!(
                !got.stderr.contains("[riot:sandbox]"),
                "({code}, {stderr:?}) 不该带提示：{}",
                got.stderr
            );
        }
    }

    #[test]
    fn 超时不算被拒() {
        let mut o = out(1, "Operation not permitted");
        o.timed_out = true;
        assert!(!annotate_denial(o).stderr.contains("[riot:sandbox]"));
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

    /// 钉住 cargo 敏感面的内容：少一条，对应的注入路径就静默重开。
    #[test]
    fn cargo_敏感面清单覆盖执行权路径() {
        let Some(home) = home_dir() else {
            eprintln!("拿不到 home，跳过");
            return;
        };
        let protected = escape_surfaces();
        if protected.is_empty() {
            eprintln!("这台机器既没有 ~/.cargo 也没有 ~/.rustup，跳过");
            return;
        }
        let names: Vec<_> = protected
            .iter()
            .filter_map(|p| p.path.file_name().and_then(|n| n.to_str()))
            .collect();
        if home.join(".cargo").exists() {
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
        }
        // `.cargo/bin` 里的 rustup shim 只是代理，真正被执行的二进制在
        // `~/.rustup/toolchains` 下 —— 漏了它等于锁门开窗。
        if home.join(".rustup").exists() {
            assert!(
                names.contains(&"toolchains"),
                "清单少了 rustup 工具链：{names:?}"
            );
        }
        // 比之前先规范化。`escape_surfaces` 的路径来自 `canonicalize`，而
        // Windows 上那会带 `\\?\` 扩展长度前缀 —— 拿没规范化的 home 去比
        // 一律不匹配。
        let home = home.canonicalize().unwrap_or(home);
        for p in &protected {
            assert!(
                p.path.starts_with(&home),
                "deny 必须落在 home 之内，否则会误伤别的路径：{} 不在 {} 之下",
                p.path.display(),
                home.display()
            );
        }
    }

    /// cargo 的老配置名一律不预建。
    ///
    /// 预建的是空文件，而 cargo 在两份都在时**用老的**（实测会警告一句
    /// `both … exist. Using …`）—— 于是一个空的 `config` 会把用户真实的
    /// `config.toml` 整个屏蔽掉，`credentials` 那对更糟。Windows 的
    /// placeholder 还是永久的，摘 ACE 不删文件。宁可留着那个洞。
    #[test]
    fn cargo_老配置名不许预建() {
        let protected = escape_surfaces();
        if protected.is_empty() {
            eprintln!("这台机器上没有相关目录，跳过");
            return;
        }
        for p in &protected {
            let name = p.path.file_name().unwrap_or_default();
            if name == "config" || name == "credentials" {
                assert!(
                    !p.plant,
                    "{} 不能预建：空文件会盖掉用户真实的 .toml 配置",
                    p.path.display()
                );
            }
        }
    }

    /// 目录型和文件型不能记混。
    ///
    /// Windows 侧 srt-win 会给缺失的 deny 目标建 placeholder，缺省建**文件**，
    /// 而且 placeholder 是永久的（`acl restore` 只摘 ACE 不删）。把 `bin` 或
    /// `toolchains` 声明成文件 = 在还没装 rustup 的机器上永久占掉那个位置，
    /// 之后 rustup 自己也建不出目录，用户得手工收拾。
    #[test]
    fn 目录型敏感面要如实标记() {
        let protected = escape_surfaces();
        if protected.is_empty() {
            eprintln!("这台机器上没有相关目录，跳过");
            return;
        }
        for p in &protected {
            let name = p.path.file_name().unwrap_or_default();
            let expect_dir = name == "bin" || name == "toolchains";
            assert_eq!(
                p.is_dir,
                expect_dir,
                "{} 的类型声明错了：建错类型的 placeholder 是不可逆的",
                p.path.display()
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
        let Some(active) = policy.activate() else {
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
                        sandbox_exempt: false,
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

    /// 逃逸面真的被内核挡住，而它所在的缓存树真的还写得进。
    ///
    /// 两半必须一起验。只验 deny，测试在「`.cargo` 压根没放开」的情况下
    /// 也会通过 —— 那时候什么都没证明；只验 allow，排除面漏了也发现不了。
    /// 一起验才说明「先放宽、再挖洞」这套顺序在真内核上成立。
    ///
    /// 探针文件名带 pid 且是点开头：万一沙箱没生效，落在用户真实
    /// `~/.cargo/bin` 里的东西不会被 PATH 捡起来执行，收尾也会删掉。
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn 逃逸面在放开的缓存树里依然写不进() {
        use crate::proc::SystemProcessRunner;

        let Some(home) = home_dir() else {
            eprintln!("拿不到 home，跳过");
            return;
        };
        let (cargo, rustup) = (home.join(".cargo"), home.join(".rustup"));
        if !cargo.join("bin").is_dir() {
            eprintln!("这台机器没有 ~/.cargo/bin，跳过");
            return;
        }

        let work = std::env::temp_dir().join(format!("riot-esc-{}", std::process::id()));
        std::fs::create_dir_all(&work).expect("建工作区");
        let Some(active) = SandboxPolicy::workspace_write(&work).activate() else {
            eprintln!("这台机器没有 sandbox-exec，跳过");
            return;
        };
        let runner = SandboxedRunner::new(
            std::sync::Arc::new(SystemProcessRunner::default()),
            std::sync::Arc::new(active),
        );
        let run = |cmd: String| {
            let (r, cwd) = (&runner, work.clone());
            async move {
                r.run(
                    ProcessSpec {
                        program: "/bin/sh".to_owned(),
                        args: vec!["-c".to_owned(), cmd],
                        cwd,
                        env: Vec::new(),
                        timeout_ms: Some(10_000),
                        sandbox_exempt: false,
                    },
                    CancellationToken::new(),
                )
                .await
                .expect("跑得起来")
            }
        };

        let tag = format!(".riot-probe-{}", std::process::id());
        // 每棵缓存树都配一对：树本身可写（控制组）、树里的逃逸面不可写
        // （实验组）。控制组不过的话，实验组通过什么都不说明 —— 那可能只是
        // 整棵树压根没放开。
        let mut cases = vec![
            (cargo.join(&tag), true),
            (cargo.join("bin").join(&tag), false),
            (cargo.join("config.toml"), false),
        ];
        if rustup.join("toolchains").is_dir() {
            cases.push((rustup.join(&tag), true));
            cases.push((rustup.join("toolchains").join(&tag), false));
        }

        for (probe, writable) in cases {
            let existed = probe.exists();
            // 追加而不是截断：清单里有 `config.toml` 这种可能已存在且用户
            // 在用的文件，deny 万一失效也不能把它清空。
            let r = run(format!("echo nope >> {}", probe.display())).await;
            if writable {
                assert_eq!(
                    r.exit_code,
                    0,
                    "{} 该是放开的,否则下面的 deny 断言证明不了任何事：{}",
                    probe.display(),
                    r.stderr
                );
            } else {
                assert_ne!(
                    r.exit_code,
                    0,
                    "{} 必须写不进 —— 写得进就等于能顶掉用户在沙箱外执行的 rustc",
                    probe.display()
                );
            }
            if !existed {
                let born = probe.exists();
                let _ = std::fs::remove_file(&probe);
                assert_eq!(born, writable, "文件在不在，要和退出码说的一致");
            }
        }

        let _ = std::fs::remove_dir_all(&work);
    }

    /// unix socket 外连真的被内核拒绝，而 `sandbox_exempt` 真的能绕开。
    ///
    /// 这两件事必须一起测。只测前者，`docker` 就只是"在沙箱里失败"，用户
    /// 没有出路、只会把沙箱关掉；只测后者，豁免就成了一个谁都能走的后门。
    /// 探针用 `nc -U` 连一个测试自己建的 socket，先跑一遍不沙箱的控制组 ——
    /// 没有它，一个用法写错的 `nc` 会让这条测试"通过"而什么都没验到。
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn unix_socket_外连被拒而豁免的命令放行() {
        use crate::proc::SystemProcessRunner;
        use std::os::unix::net::UnixListener;

        let dir = std::env::temp_dir().join(format!("riot-uds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建目录");
        let sock = dir.join("probe.sock");
        let _ = std::fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock).expect("建 socket");
        std::thread::spawn(move || while listener.accept().is_ok() {});

        let policy = SandboxPolicy::WorkspaceWrite {
            writable: vec![dir.canonicalize().expect("规范化")],
            allow_network: true,
        };
        let Some(active) = policy.activate() else {
            eprintln!("这台机器没有 sandbox-exec，跳过");
            return;
        };
        let base: std::sync::Arc<dyn ProcessRunner> =
            std::sync::Arc::new(SystemProcessRunner::default());
        let sandboxed = SandboxedRunner::new(base.clone(), std::sync::Arc::new(active));

        let spec = || ProcessSpec {
            program: "/usr/bin/nc".to_owned(),
            args: vec![
                "-U".to_owned(),
                sock.display().to_string(),
                "-w".to_owned(),
                "2".to_owned(),
            ],
            cwd: dir.clone(),
            env: Vec::new(),
            timeout_ms: Some(10_000),
            sandbox_exempt: false,
        };

        let control = base
            .run(spec(), CancellationToken::new())
            .await
            .expect("跑得起来");
        assert_eq!(control.exit_code, 0, "控制组该连得上：{}", control.stderr);

        let denied = sandboxed
            .run(spec(), CancellationToken::new())
            .await
            .expect("跑得起来");
        assert_ne!(
            denied.exit_code, 0,
            "沙箱内必须连不上 unix socket —— 那是把活外包给沙箱外 daemon 的通道"
        );

        let exempt = ProcessSpec {
            sandbox_exempt: true,
            ..spec()
        };
        let out = sandboxed
            .run(exempt, CancellationToken::new())
            .await
            .expect("跑得起来");
        assert_eq!(
            out.exit_code, 0,
            "标了 sandbox_exempt 的命令要落到宿主执行器上：{}",
            out.stderr
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Windows：走**完整生产装配**（activate → SandboxedRunner → run）
    /// 验证边界。`sandbox_win` 的单测只覆盖命令行拼装那半（纯逻辑，mac 上
    /// 也跑）；这条覆盖真正落地的那半：装机检查、`acl grant`、`srt-win exec`
    /// 起进程、Drop 回收。
    ///
    /// `[约束]` 它要求这台机器**已经装过** `srt-win`（管理员跑一次
    /// `srt-win install`）。没装时 activate 返回 None 是**正确行为**，所以
    /// 这里跳过而不是失败 —— 在没装的机器上把它判红，只会让人把测试注掉。
    /// 真机保证由 CI 的 win-sandbox-smoke 提供。
    #[cfg(windows)]
    #[tokio::test]
    async fn windows_经装配路径的沙箱边界() {
        use crate::proc::SystemProcessRunner;

        let base = std::env::temp_dir().join(format!("riot-sbx-integ-{}", std::process::id()));
        let work = base.join("work");
        let outside = base.join("outside");
        std::fs::create_dir_all(&work).expect("建工作区");
        std::fs::create_dir_all(&outside).expect("建外部目录");

        // 用生产构造器而不是手写 `WorkspaceWrite{…}`：这条测试的意义就是
        // "完整生产装配"，而缓存表（`~/.cargo` 那些）和逃逸面 DENY 是一对，
        // 手写一个只有工作区的策略就把这对关系整个绕过去了。
        let policy = SandboxPolicy::workspace_write(&work);
        // `[约束]` 装 subscriber，否则 `activate` 失败的原因（全走
        // `tracing::warn!`）会被静默丢掉 —— 而这条测试只在 CI 的 Windows 上
        // 跑，日志是唯一的断案材料。`try_init` 而不是 `init`：同进程里别的
        // 测试可能已经装过。
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .with_max_level(tracing::Level::DEBUG)
            .try_init();

        // 计时是有用的信号，不只是好奇：`acl grant` 会把可继承 ACE 传播到
        // 已有子对象，而 `~/.cargo\registry` 动辄几万文件。这个数字直接决定
        // 「每会话激活一次」这个设计能不能接受（见 SandboxedRunner 的
        // `sandbox: Arc<…>` 注释）。回收要再付一次同样的钱。
        let t0 = std::time::Instant::now();
        let activated = policy.activate();
        eprintln!("[timing] activate 耗时 {:?}", t0.elapsed());
        // `[约束]` CI 里必须失败得响亮。`activate` 返回 None 有两个原因：
        // 没装（开发机的常态，该跳过），或者装了但冒烟没过（真问题）。这两个
        // 在返回值上分不开，而默认跳过会让 e2e job 在什么都没验的情况下变绿 ——
        // 那比红更糟。CI 设 RIOT_SANDBOX_TEST_REQUIRE=1 把跳过变成失败。
        let Some(active) = activated else {
            let msg = "沙箱没激活成功。要么没装（管理员跑 `srt-win install` \
                       然后 `srt-win wfp uninstall`），要么装了但冒烟没过 —— \
                       后者会在 warn 日志里给出原因（比如工作区在映射盘上）。";
            assert!(
                std::env::var_os("RIOT_SANDBOX_TEST_REQUIRE").is_none(),
                "{msg}"
            );
            eprintln!("{msg} 跳过。");
            let _ = std::fs::remove_dir_all(&base);
            return;
        };
        let runner = SandboxedRunner::new(
            std::sync::Arc::new(SystemProcessRunner::default()),
            std::sync::Arc::new(active),
        );

        // `[约束]` 整条命令作为**一个** `cmd /c` 参数传，不要拆成
        // `["/c", "echo", "hi", ">", path]`。拆开的写法依赖「谁来把 argv 重新
        // 拼成命令行」：旧的 Low IL 后端手写 `build_command_line`、把参数裸拼
        // 进去，于是 `>` 还是重定向；换成 srt-win 之后 argv 由它自己按
        // CreateProcess 的规矩组装，`>` 就成了 echo 的字面参数 —— 命令照样
        // 退出 0，文件却没建出来。这个测试为此红过一次。
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
                        sandbox_exempt: false,
                    },
                    CancellationToken::new(),
                )
                .await
                .expect("跑得起来")
            }
        };
        // 热身探针：一条**不依赖任何授权**的命令。先把它的结果打出来（不
        // 断言），好把「沙箱里能不能跑命令」和「授权对不对」分开。
        //
        // 首跑失败时看到的是 exit=124（超时）+ stdout/stderr 全空，那形态
        // 分不出是 exec 压根没起来、还是起来了但写不进去。这一条能分。
        let warm = exec("echo probe-ok").await;
        eprintln!(
            "[probe] exit={} timed_out={} stdout={:?} stderr={:?}",
            warm.exit_code, warm.timed_out, warm.stdout, warm.stderr
        );
        assert!(
            !warm.timed_out,
            "沙箱内连 `echo` 都没跑起来（超时）。这与授权无关 —— \
             要么 srt-win exec 在这个调用环境下起不来（stdin/job object/桌面），\
             要么参数拼错了。stdout={:?} stderr={:?}",
            warm.stdout, warm.stderr
        );
        assert_eq!(
            warm.exit_code, 0,
            "沙箱内跑 `echo` 就失败了：stdout={:?} stderr={:?}",
            warm.stdout, warm.stderr
        );

        // 重定向目标加引号：临时目录路径里可能有空格。
        let write_probe = |p: &std::path::Path| format!("echo hi > \"{}\"", p.display());

        let inside = work.join("in.txt");
        let ok = exec(&write_probe(&inside)).await;
        assert_eq!(
            ok.exit_code, 0,
            "工作区内该写得进：stdout={:?} stderr={:?}",
            ok.stdout, ok.stderr
        );
        assert!(
            inside.exists(),
            "命令退出 0 但文件没建出来 —— 多半是重定向被当成了字面参数：stdout={:?}",
            ok.stdout
        );

        // ── 工作区之外写不进 ────────────────────────────────────────
        //
        // `[约束]` 判据是「**真实用户的文件**碰不到」，不是「任何没授权的
        // 路径都碰不到」。这两者在旧的 Low IL 模型下是一回事（没打标签的
        // 一律拒），在账户 + ACE 模型下不是：附加 ACE 只**增加**权限，不
        // 移除已有的。沙箱账户是 `BUILTIN\Users` 成员，凡是对 Users 开放
        // 写的位置（`C:\Windows\Temp`、松散 ACL 的数据盘目录……）它照样
        // 写得进 —— 那些地方从来就没被任何人保护过。
        //
        // 用真实用户的 profile 当靶子：那是这套设计真正要挡住的东西，也是
        // 唯一一个「挡不住就等于沙箱没用」的判据。取舍与残余风险记在
        // docs/SANDBOX_WINDOWS.md。
        let icacls = |p: &std::path::Path| {
            std::process::Command::new("icacls")
                .arg(p)
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                .unwrap_or_else(|e| format!("(icacls 跑不起来：{e})"))
        };

        let home = std::path::PathBuf::from(
            std::env::var_os("USERPROFILE").expect("Windows 上必然有 USERPROFILE"),
        );
        let home_file = home.join(format!("riot-sbx-escape-{}.txt", std::process::id()));
        let denied = exec(&write_probe(&home_file)).await;
        let leaked = home_file.exists();
        let _ = std::fs::remove_file(&home_file);
        assert!(
            denied.exit_code != 0 && !leaked,
            "沙箱写进了真实用户的主目录 —— 这套隔离没起作用。\n\
             exit={} stdout={:?}\n{} 的 ACL:\n{}",
            denied.exit_code,
            denied.stdout,
            home.display(),
            icacls(&home)
        );

        // 同一个临时树里的兄弟目录。这一条**只诊断不断言**：它写不写得进
        // 取决于那棵树原本的 ACL（CI runner 上的 RUNNER_TEMP 对 Users 开放
        // 写），而那不是沙箱能决定的事。打出来是为了让残余风险有据可查。
        let out_file = outside.join("out.txt");
        let sibling = exec(&write_probe(&out_file)).await;
        eprintln!(
            "[diag] 未授权兄弟目录 exit={} 文件存在={} ACL:\n{}",
            sibling.exit_code,
            out_file.exists(),
            icacls(&outside)
        );
        let _ = std::fs::remove_file(&out_file);

        // ── 授权的树里，逃逸面依然写不进 ─────────────────────────────
        //
        // `[约束]` 这条只有真机说了算。整套 DENY 建立在「显式 DENY ACE 在
        // DACL 求值里排在显式 ALLOW 之前」上 —— mac 上的单测只能验命令行拼
        // 得对，压不压得住得 Windows 自己回答。压不住的话 `~/.cargo\bin` 在
        // `acl grant` 的可继承 ALLOW 下是可写的，沙箱能顶掉用户 PATH 上的
        // `cargo.exe`。
        //
        // 配对断言：`.cargo` 本身要写得进（否则下面那半证明不了任何事 ——
        // 可能只是整棵树压根没授权），`.cargo\bin` 要写不进。
        if let Some(cargo) = home_dir().map(|h| h.join(".cargo"))
            && cargo.join("bin").is_dir()
        {
            let tag = format!("riot-esc-{}.txt", std::process::id());
            for (probe, writable) in [
                (cargo.join(&tag), true),
                (cargo.join("bin").join(&tag), false),
            ] {
                let r = exec(&write_probe(&probe)).await;
                let born = probe.exists();
                let _ = std::fs::remove_file(&probe);
                assert_eq!(
                    (r.exit_code == 0) && born,
                    writable,
                    "{} 的可写性不对（该 writable={writable}）：exit={} stdout={:?} \
                     文件存在={born}\nACL:\n{}",
                    probe.display(),
                    r.exit_code,
                    r.stdout,
                    icacls(&probe)
                );
            }
        }

        // 整套设计成立与否的单点判据：沙箱里的命令**是另一个用户在跑**。
        //
        // 上面那两条写入断言只能证明「有个边界」；这一条证明边界是**身份**
        // 而不是完整性级别。两跳启动（broker → CreateProcessWithLogonW →
        // runner → 受限令牌）任何一环没接上，whoami 都会回落成宿主用户，
        // 而那时文件系统断言可能仍然是绿的（工作区本来就能写）。
        let who = exec("whoami").await;
        assert_eq!(who.exit_code, 0, "whoami 该跑得起来：{}", who.stderr);
        let host_who = std::process::Command::new("whoami")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_lowercase())
            .expect("宿主 whoami");
        let sandbox_who = who.stdout.trim().to_lowercase();
        assert!(
            !sandbox_who.is_empty() && sandbox_who != host_who,
            "沙箱内该是另一个账户：沙箱={sandbox_who:?} 宿主={host_who:?}"
        );

        // TMP/TEMP/TMPDIR 都被重写到会话专属子目录，而且那里真能写。
        // 沙箱账户对宿主的全局 %TEMP% 没有授权，不重写的话临时文件全失败。
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

        // 注册表持久化（Run 键、文件关联）这条路要堵住。
        //
        // 换成账户模型之后判据变了：沙箱账户有**自己的** HKCU hive，所以这条
        // `reg add` 多半会成功 —— 但写进去的是它的 hive。旧版靠 Low IL 让它
        // 直接失败，现在靠身份隔离。要钉的是后者：真实用户的 HKCU 里不能
        // 出现这个键。只断言 exit_code != 0 的话，这个测试在新模型下会红，
        // 而红的原因和安全性无关。
        const PROBE_KEY: &str = r"HKCU\Software\RiotSandboxProbe";
        let _ = exec(&format!("reg add {PROBE_KEY} /v x /d 1 /f")).await;
        let leaked = std::process::Command::new("reg")
            .args(["query", PROBE_KEY])
            .output()
            .is_ok_and(|o| o.status.success());
        assert!(!leaked, "沙箱内的注册表写不该落到真实用户的 HKCU 上");

        drop(runner); // 触发 Drop：回收 ACE + 删会话 temp
        let _ = std::fs::remove_dir_all(&base);
    }
}
