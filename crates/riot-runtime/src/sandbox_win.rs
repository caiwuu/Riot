//! Windows 沙箱后端：专用本地账户 + 附加 ACE。
//!
//! 底层实现是 vendored 的 `srt-win`（Apache-2.0，见 vendor/srt-win/NOTICE.md）。
//! 这个文件只做**编排**：查装机状态、给可写目录授权、把命令改写成一次
//! `srt-win exec`、会话结束时回收授权。
//!
//! # 为什么不再用 Low IL
//!
//! 上一版是「受限令牌 + Low 完整性级别 + 给可写目录打 Low 标签」。它被咬过
//! 两次：给 `~/.rustup` 打标签让宿主机的 cargo 全废（标签是**对象属性**，
//! 对全机所有进程生效），以及 Low 进程连不上 Docker 的 named pipe（MIC 的
//! no-write-up 拦住了对 Medium 完整性对象的写）。
//!
//! 换的是隔离轴,不是实现细节:
//!
//! | | 旧 | 新 |
//! |---|---|---|
//! | 主体 | 同一用户,降到 Low IL | **另一个本地账户**,Medium IL |
//! | 客体 | 给目录打 Low 标签(影响所有人) | 给沙箱 SID 加**附加 ACE**(只影响它) |
//!
//! 附加 ACE 从不重写路径原有的安全描述符,所以宿主用户的访问一点不变——
//! 上面那两个事故的根因就此消失。Anthropic 和 OpenAI 的实现独立收敛到了
//! 同一个架构,而且都显式避开 Low IL(srt-win 的 `token.rs`:Medium IL,
//! "so Schannel / LSA / registry edge cases that fire at Low IL don't apply")。
//!
//! # 只用它的一半
//!
//! `srt-win` 的完整方案还含一层 WFP 出网栅栏,我们**不装**:它会拦掉沙箱
//! 账户的全部外连、只放行 loopback 上的代理端口段,而 Riot 没有代理层,
//! 装了沙箱内就彻底断网。裁剪靠不调用 `wfp` 子命令,不靠改它的代码。
//! 代价是 NoNet 档在 Windows 仍然诚实降级(见 [`activate`])。
//!
//! # 为什么起子进程而不是在进程内调
//!
//! `srt-win` 是库 + 二进制两用的,`run_from_args` 可以直接链接进来。但
//! `exec` 的 broker 半边会**把子进程的 stdout/stderr 泵到自己的 stdio**
//! （见它的 `logon.rs`）—— 在进程内调,沙箱命令的输出就直接串进内核自己的
//! 标准输出了,没法按命令捕获。所以走子进程。
//!
//! 而且这样和 macOS 同构:两边都是「改写 `ProcessSpec`,交给 `inner` 跑」
//! （见 [`wrap`]），管道、超时、取消、输出封顶全部复用 `proc.rs` 那一套。
//! 上一版为此手写了 490 行 `CreateProcessAsUserW` + 管道泵,现在一行不用。

#![allow(clippy::disallowed_methods)]
// 这个模块在所有平台编译（见 lib.rs 的理由），但只有 Windows 会调用
// `activate` / `recover_orphans` 那条链。在 mac 上它们没有调用方，而
// 门控掉就等于把纯逻辑那部分也一起门控没了 —— 那才是这里最该被测的东西。
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use riot_protocol::tool::ProcessSpec;

/// 已激活的 Windows 沙箱。
///
/// 拿到它意味着:`srt-win` 装过了(账户 + 凭证在位),而且本会话的可写目录
/// 已经拿到了沙箱账户的 ALLOW ACE。`Drop` 时按 holder pid 一次性回收。
pub(crate) struct WinSandbox {
    /// 怎么调 `srt-win`。
    srt: SrtWin,
    /// 沙箱账户的 SID。`acl` 的每个子命令都要带,exec 不用。
    sid: String,
    /// 本会话授权过的路径。只用于 `Drop` 的日志——真正的账在 srt-win 自己
    /// 的状态库里,按 holder pid 记,`acl revoke` 一次清干净。
    granted: Vec<PathBuf>,
    /// 会话专属 temp 子目录。见 [`session_temp_env`]。
    session_temp: PathBuf,
}

impl WinSandbox {
    /// 给 cargo 敏感面打附加 DENY ACE。
    ///
    /// 见 [`crate::sandbox::escape_surfaces`]：这几处在可写区**之内**，但写了
    /// 就换到沙箱**之外**的执行权。清单怎么筛见 [`stamp_targets`]。
    fn stamp_protected(&self) -> std::io::Result<()> {
        let paths = stamp_targets(&self.granted);
        if paths.is_empty() {
            return Ok(());
        }
        let pid = std::process::id().to_string();
        run_srt(
            &self.srt,
            &[
                "acl",
                "stamp",
                "--holder-pid",
                &pid,
                "--sandbox-user-sid",
                &self.sid,
            ],
            Some(&stamp_payload(&paths)),
        )
        .map(drop)
    }

    /// 真起一条命令，确认这套东西在这台机器 / 这个工作区上跑得通。
    ///
    /// 用会话 temp 当工作目录而不是工作区：这一步要验的是「沙箱能不能起
    /// 进程」，而会话 temp 是我们自己建的、必然已授权。工作区那侧的可写性
    /// 由决策链之后的真实命令去验 —— 在这里多验一次只会让激活更慢。
    ///
    /// `[取舍]` 代价是每次会话激活多一次两跳启动（seclogon + 建桌面），
    /// 比 macOS 那次 `/usr/bin/true` 贵得多。但它只在**会话第一次激活**时
    /// 付，而换到的是「沙箱要么真能用、要么老实说不能用」。
    fn smoke(&self) -> std::io::Result<()> {
        let (program, prefix) = self.srt.argv_prefix();
        let spec = ProcessSpec {
            program: "cmd".to_owned(),
            args: vec!["/c".to_owned(), "exit 0".to_owned()],
            cwd: self.session_temp.clone(),
            env: Vec::new(),
            timeout_ms: None,
            sandbox_exempt: false,
        };
        let args = exec_args(&prefix, &self.session_temp, &spec);
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        run_srt_in(&program, &argv, None, Some(&self.session_temp)).map(drop)
    }

    /// 把一条命令改写成「在沙箱里跑这条命令」。
    pub(crate) fn wrap(&self, spec: ProcessSpec) -> ProcessSpec {
        let (program, prefix) = self.srt.argv_prefix();
        ProcessSpec {
            program,
            args: exec_args(&prefix, &self.session_temp, &spec),
            // env 不留在 spec 上:它已经被翻成 `--env` 了。留着的话
            // `inner` 会把它设到 **broker**（srt-win 自己）的环境上,而不是
            // 沙箱子进程的——那是两个不同的进程。
            env: Vec::new(),
            ..spec
        }
    }
}

impl Drop for WinSandbox {
    fn drop(&mut self) {
        let srt = self.srt.clone();
        let sid = std::mem::take(&mut self.sid);
        let n = self.granted.len();
        let temp = std::mem::take(&mut self.session_temp);
        let cleanup = move || {
            // 按 holder pid 回收本会话写下的 ACE。srt-win 内部按路径引用
            // 计数,归零才真撤——同机另一个会话正用着同一个工作区时,它的
            // ACE 不会被我们连坐撤掉。
            //
            // `[约束]` 两笔账要分开撤:grant 的 ALLOW 走 `revoke`,stamp 的
            // DENY 走 `restore`。只撤一边会留下另一边,而留下 DENY 尤其糟 ——
            // 下一次会话的 grant 压不过它(DENY 在 DACL 求值里优先),表现是
            // 沙箱内的 cargo 莫名其妙写不了 `.cargo\bin`。
            let pid = std::process::id().to_string();
            for (what, sub) in [("授权", "revoke"), ("敏感面 DENY", "restore")] {
                match run_srt(
                    &srt,
                    &[
                        "acl",
                        sub,
                        "--holder-pid",
                        &pid,
                        "--sandbox-user-sid",
                        &sid,
                        "--json",
                    ],
                    None,
                ) {
                    Ok(_) => tracing::debug!(paths = n, kind = what, "沙箱 ACE 已回收"),
                    // 撤不掉不 panic:srt-win 的状态库在下一次 acl 操作时会跑
                    // 崩溃恢复(它的 state_db 模块头写明"crash-recovery runs
                    // unconditionally at every acquire"),孤儿 ACE 由那条路兜底。
                    Err(e) => {
                        tracing::warn!(error = %e, kind = what, "回收沙箱 ACE 失败,留给下次崩溃恢复");
                    }
                }
            }
            let _ = std::fs::remove_dir_all(&temp);
        };
        // 回收要起子进程、还要遍历目录树改 ACL,`~/.cargo` 那种十万文件的树
        // 能走上几秒。Drop 常发生在 tokio 工作线程上,同步做就是把 runtime
        // 堵在那儿。
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => drop(rt.spawn_blocking(cleanup)),
            Err(_) => cleanup(),
        }
    }
}

/// 尝试激活 Windows 沙箱。
///
/// `None` = 这台机器上做不到。三种情况:找不到 `srt-win`、没装过(用户还没
/// 跑提权安装)、或授权失败。任一种都必须返回 `None` —— 决策链据
/// `sandboxed` 放宽,绝不能在"其实没边界"时谎报成 true。
pub(crate) fn activate(policy: &crate::sandbox::SandboxPolicy) -> Option<WinSandbox> {
    use crate::sandbox::SandboxPolicy;

    let SandboxPolicy::WorkspaceWrite {
        writable,
        allow_network,
    } = policy
    else {
        return None; // Off 不该走到这里
    };

    // `[约束]` NoNet 档在 Windows 继续诚实降级。断网要靠 srt-win 的 WFP 那
    // 一半,而那一半我们刻意不装(见模块头)。装了它会连正常联网一起掐,
    // 因为 Riot 没有代理层给它放行。返回 None → 逐条询问,慢但不撒谎。
    if !allow_network {
        tracing::warn!("WorkspaceWriteNoNet 在 Windows 暂不隔离网络,本轮不激活沙箱(逐条询问)");
        return None;
    }

    let srt = SrtWin::locate()?;

    let sid = match sandbox_user_sid(&srt) {
        Ok(Some(sid)) => sid,
        Ok(None) => {
            // 装机是一次性的提权动作,不能在这里偷偷做——建本地账户 + 写
            // HKLM 需要 UAC,而这里可能跑在后台会话里。给一句能照做的话。
            //
            // `[约束]` 两步,第二步不能省。`install` 会把账户**和 WFP 出网
            // 栅栏**一起装上,而那道栅栏会拦掉沙箱账户的全部外连、只放行
            // loopback 上的代理端口段——Riot 没有代理层,留着它沙箱内就
            // 彻底断网(`npm install` / `cargo build` 全死),而策略层还以为
            // allow_network 是 true。`wfp uninstall` 只摘过滤器,账户和凭证
            // 都留着。
            tracing::warn!(
                "Windows 沙箱尚未安装,本轮不隔离。以管理员身份依次跑:\
                 `srt-win install` 然后 `srt-win wfp uninstall`\
                 (后者摘掉出网栅栏,只留专用账户 —— 见 docs/SANDBOX_WINDOWS.md §4)"
            );
            return None;
        }
        Err(e) => {
            tracing::warn!(error = %e, "查 Windows 沙箱装机状态失败,本轮不隔离");
            return None;
        }
    };

    // 会话专属 temp。沙箱账户有自己的 %TEMP%,但 Riot 在 Windows 上跑的是
    // Git Bash,MSYS 那套**先看 TMPDIR**;而且宿主侧有时要能读到命令留下的
    // 临时产物。建在真实用户的 %TEMP% 下、单独授权给沙箱账户,比让它写进
    // 另一个用户的 profile 更好收拾。
    let session_temp = match make_session_temp() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "建会话 temp 子目录失败,本轮不隔离");
            return None;
        }
    };

    let mut granted = writable.clone();
    granted.push(session_temp.clone());

    // 分步计时。`acl grant` 会把可继承 ACE 传播到**已有子对象**，而
    // `~/.cargo\registry` 动辄几万文件 —— CI 上量到整个 activate 17.8s。
    // 光一个总数说不清是 grant 还是 stamp 贵，而这两条的优化方向完全不同。
    let t_grant = std::time::Instant::now();
    if let Err(e) = grant(&srt, &sid, &granted) {
        tracing::warn!(error = %e, "给沙箱账户授权可写目录失败,本轮不隔离");
        let _ = std::fs::remove_dir_all(&session_temp);
        return None;
    }
    tracing::info!(
        elapsed_ms = t_grant.elapsed().as_millis() as u64,
        paths = granted.len(),
        "acl grant 完成"
    );

    // 先把它造出来：后面每一步失败都靠 `Drop` 回收（撤 grant/stamp、删会话
    // temp），不再各自写一遍清理。
    let sandbox = WinSandbox {
        srt,
        sid,
        granted,
        session_temp,
    };

    // `[约束]` 敏感面的 DENY 必须跟在 grant 后面，而且失败要退回不隔离。
    //
    // `.cargo` 整树在 grant 列表里（构建要写 registry 和锁），而 `acl grant`
    // 写的是 `(OI)(CI)` 可继承 ALLOW —— 它会继承进 `.cargo\bin`，于是沙箱
    // 账户能顶掉那里的 `cargo.exe` / `rustc.exe`，用户下一次在**沙箱外**
    // 构建就执行了它。DENY 在 DACL 求值里排在 ALLOW 之前，压得住。
    //
    // 盖不上就不能说自己沙箱着 —— 那等于一边报 sandboxed=true、一边把
    // 沙箱外的执行权敞着。
    let t_stamp = std::time::Instant::now();
    if let Err(e) = sandbox.stamp_protected() {
        tracing::warn!(error = %e, "给 cargo 敏感面打 DENY 失败,本轮不隔离");
        return None;
    }
    tracing::info!(
        elapsed_ms = t_stamp.elapsed().as_millis() as u64,
        "acl stamp 完成"
    );

    // `[约束]` 装机状态对**不代表跑得起来**。真起一条命令确认一遍，失败就
    // 退回不隔离 —— 和 macOS 的 `profile_accepted` 同一个理由：不冒烟的话
    // `sandboxed` 已经报成 true、决策链按"OS 挡着"放行了一批命令，然后每
    // 一条都失败。方向是安全的，但用户看到的是应用坏了。
    //
    // 已知会走到这里的一类是 `mapped_drive_cwd`：工作区在映射盘 / 网络盘上
    // 时，seclogon 为沙箱账户建的登录会话里没有那个映射，
    // `CreateProcessWithLogonW` 直接失败（srt-win 退 16，stderr 一行 JSON）。
    // 没有这道冒烟的话，症状是**每条命令**都吐那段 JSON —— 而模型读不懂它。
    if let Err(e) = sandbox.smoke() {
        tracing::warn!(error = %e, "Windows 沙箱冒烟没过,本轮不隔离(决策链回到逐条询问)");
        return None; // Drop 会回收刚授权的 ACE、删掉会话 temp
    }

    Some(sandbox)
}

/// 启动时回收上次进程残留的孤儿 ACE。**在任何会话激活之前调一次。**
///
/// 上一版这里要自己拿跨进程独占锁(标签清单只有一个写者)。现在不用了:
/// srt-win 的状态库自带文件锁和崩溃恢复,`acl recover` 只是显式触发一次。
/// 不带 `--force` —— 那个会无视 holder 存活情况横扫,同机双开时会踩到
/// 另一个内核进程正在用的授权。
pub(crate) fn recover_orphans() {
    let Some(srt) = SrtWin::locate() else { return };
    match run_srt(&srt, &["acl", "recover", "--json"], None) {
        Ok(out) => tracing::debug!(result = %out.trim(), "沙箱孤儿授权回收完毕"),
        Err(e) => tracing::info!(error = %e, "沙箱孤儿授权回收跳过"),
    }
}

// ─────────────────────────────────────────────────────────────
// 纯逻辑:命令行拼装与状态解析
//
// 这一段刻意不碰任何 Windows API,于是能在 mac 上真跑测试。Windows 侧
// 剩下的只有「起子进程」那一下,而那部分的行为由 CI 的真机冒烟兜底。
// ─────────────────────────────────────────────────────────────

/// 怎么调 `srt-win`。
#[derive(Clone, Debug, PartialEq, Eq)]
enum SrtWin {
    /// 独立的 `srt-win.exe`。`RIOT_SRT_WIN` 指定,开发和 CI 用
    /// （`cargo build -p srt-win` 的产物）。
    Exe(PathBuf),
    /// multicall:就是内核自己这个可执行文件,靠 `argv[1]` 分发。
    /// 上游为此导出了 `SRT_WIN_DISPATCH_ARG1` 和 `run_from_args`。
    SelfExe(PathBuf),
}

/// 上游约定的 multicall 分发标记（`srt_win::SRT_WIN_DISPATCH_ARG1`）。
///
/// 这里写字面量而不是引用那个常量:这个模块在 mac 上也要编译,而
/// `srt-win` 整个 crate 是 `#![cfg(windows)]`,非 Windows 上取不到符号。
/// 有测试钉住两者一致(仅 Windows 编译时生效)。
const SRT_WIN_DISPATCH_ARG1: &str = "--srt-win";

impl SrtWin {
    fn locate() -> Option<Self> {
        if let Some(p) = std::env::var_os("RIOT_SRT_WIN") {
            let p = PathBuf::from(p);
            if p.is_file() {
                return Some(Self::Exe(p));
            }
            tracing::warn!(path = %p.display(), "RIOT_SRT_WIN 指向的文件不存在,回退到 multicall");
        }
        match std::env::current_exe() {
            Ok(p) => Some(Self::SelfExe(p)),
            Err(e) => {
                tracing::warn!(error = %e, "取不到当前可执行文件路径,Windows 沙箱不可用");
                None
            }
        }
    }

    /// `(程序, 前置参数)`。multicall 要多一个 `--srt-win`。
    fn argv_prefix(&self) -> (String, Vec<String>) {
        match self {
            Self::Exe(p) => (p.to_string_lossy().into_owned(), Vec::new()),
            Self::SelfExe(p) => (
                p.to_string_lossy().into_owned(),
                vec![SRT_WIN_DISPATCH_ARG1.to_owned()],
            ),
        }
    }
}

/// 一条 `srt-win exec` 的完整参数表。
///
/// `[约束]` `--` 之后必须是「程序 + 它的参数」,而且 `--` 不能省:命令行里
/// 常有 `-c`、`--release` 这类东西,不终止 srt-win 自己的选项解析就会被它
/// 当成自己的参数吃掉。
fn exec_args(prefix: &[String], session_temp: &Path, spec: &ProcessSpec) -> Vec<String> {
    let mut args = prefix.to_vec();
    args.push("exec".to_owned());
    // broker 的进度行、per-exec deny 摘要会混进沙箱命令的 stderr,而那份
    // stderr 是要给模型看的。
    args.push("--quiet".to_owned());

    // 顺序即优先级（同名时 srt-win 取后者）：broker 的 PATH 打底，会话 temp
    // 覆盖它，调用方自己设的 env 最大。
    for (k, v) in broker_env()
        .into_iter()
        .chain(session_temp_env(session_temp))
        .chain(spec.env.iter().cloned())
    {
        args.push("--env".to_owned());
        args.push(format!("{k}={v}"));
    }

    args.push("--".to_owned());
    args.push(resolve_program(&spec.program));
    args.extend(spec.args.iter().cloned());
    args
}

/// 把程序名解析成绝对路径。
///
/// `[约束]` 不能把裸名字交给 srt-win。它最终走
/// `CreateProcessAsUserW(lpApplicationName = <程序>, …)`，而 `lpApplicationName`
/// **非 NULL 时 Windows 不做 PATH 搜索、也不自动补 `.exe`** —— 传 `cmd` 会以
/// `The system cannot find the file specified (0x80070002)` 收场。
///
/// 这不是只影响测试：`diagnostics` 传的是 `cargo` / `npx` 这类裸名，
/// `tools::bash::shell_program` 找不到 Git Bash 时也兜底成裸 `bash`。不解析的话
/// 沙箱一开，这些命令全部起不来。
fn resolve_program(program: &str) -> String {
    resolve_program_in(
        program,
        std::env::var_os("PATH").as_deref().unwrap_or_default(),
        &pathext(),
    )
}

/// [`resolve_program`] 的纯逻辑部分：PATH 和扩展名列表由调用方给。
///
/// 拆出来是为了能测 —— 直接读环境变量的版本在并行测试里改不得
/// （`set_var` 是进程级的）。
fn resolve_program_in(program: &str, path: &std::ffi::OsStr, exts: &[String]) -> String {
    // 已经带路径分隔符（绝对或相对）：调用方指名道姓了，别替它改。
    if program.contains('/') || program.contains('\\') {
        return program.to_owned();
    }
    for dir in std::env::split_paths(path) {
        for ext in exts {
            let cand = dir.join(format!("{program}{ext}"));
            if cand.is_file() {
                return cand.to_string_lossy().into_owned();
            }
        }
    }
    // 找不到就原样交出去。srt-win 报的 "cannot find the file specified" 比
    // 我们在这里编一个错误更准确，而且路径完全一致（都是没找到）。
    program.to_owned()
}

/// 要试的扩展名，空串在最前（`foo` 本身也可能就是可执行文件）。
fn pathext() -> Vec<String> {
    #[allow(unused_mut)] // 非 Windows 上永远只有那个空串
    let mut v = vec![String::new()];
    #[cfg(windows)]
    if let Some(pe) = std::env::var_os("PATHEXT") {
        v.extend(
            pe.to_string_lossy()
                .split(';')
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
        );
    }
    v
}

/// 沙箱子进程要继承的 broker 侧环境。
///
/// 目前只有 `PATH`。沙箱账户以 `LOGON_WITH_PROFILE` 登录，拿到的是**它自己**
/// 的 profile 环境，`PATH` 只有机器级那部分 —— 而工具解析要按调用方（真实
/// 用户）的 PATH 来算，否则沙箱里 `where npm` 和外面不是一个答案。上游的
/// TS 侧也是这么传的（logon.rs 模块头：profile-scoped 的变量留沙箱账户的，
/// 工具解析用的 PATH 用 broker 的）。
///
/// `[取舍]` 这只解决「找得到」，不解决「打得开」。用户 profile 下装的工具
/// （nvm 的 node、Scoop 的包）沙箱账户仍然没有读权限 —— 那是上游记档的已知
/// 限制，见 vendor/srt-win/NOTICE.md。
fn broker_env() -> Vec<(String, String)> {
    std::env::var_os("PATH")
        .map(|p| vec![("PATH".to_owned(), p.to_string_lossy().into_owned())])
        .unwrap_or_default()
}

/// 指向会话 temp 的三个变量。
///
/// `[约束]` `TMPDIR` 不能漏。Bash 工具在 Windows 上跑的是 Git for Windows
/// 的 bash（见 `tools::bash::shell_program`），而 MSYS 那套**先看 TMPDIR**:
/// 不设的话 `mktemp`、编译器的中间文件会落到沙箱账户够不着的地方。
///
/// 排在 `spec.env` **前面**:同名时后写的赢,会话真要覆盖 TMP 也随它。
fn session_temp_env(session_temp: &Path) -> Vec<(String, String)> {
    let t = session_temp.to_string_lossy().into_owned();
    ["TMP", "TEMP", "TMPDIR"]
        .into_iter()
        .map(|k| (k.to_owned(), t.clone()))
        .collect()
}

/// `acl grant` 的 stdin 载荷。
///
/// 只给 `write`:读不受限（沙箱账户对系统目录本来就有读权限,而工作区的读
/// 由 `MODIFY` 里的 `FILE_GENERIC_READ` 带出来）。收紧读会让编译类命令
/// 大面积失败,和 macOS 那侧的取舍一致。
/// 从逃逸面清单里筛出这次真要打 DENY 的目标，翻成 `acl stamp` 的路径串。
///
/// `[约束]` 只给**授权过的**路径打。DENY 存在的意义就是压住 `acl grant` 那条
/// 可继承 ALLOW，没 grant 的地方沙箱账户本来就够不着。这不是省事：srt-win 的
/// `apply_aces` 会把 ACE 传播到已有子对象，给 `~/.rustup\toolchains` 那种几万
/// 文件的树白打一遍，激活要多花好几秒。按 `granted` 现算而不是写死平台差异，
/// 是为了让「缓存表里加一项，它的敏感面就自动受保护」成立 —— 上次这套东西
/// 出问题，正是因为我把这层耦合记在脑子里而不是代码里。
///
/// `[约束]` 两边都规范化再比。`escape_surfaces` 的路径来自 `canonicalize`，
/// 在 Windows 上带 `\\?\` 扩展长度前缀；`granted` 只有走
/// `SandboxPolicy::workspace_write`（内部的 `dedup_existing`）才是同一种形式。
/// 形式不一致时 `starts_with` 全假，筛出空表 —— 而空表是**静默**的：一条
/// DENY 都不打，激活照样成功，洞就这么重新开了。不靠调用方保证。
fn stamp_targets(granted: &[PathBuf]) -> Vec<String> {
    let granted: Vec<PathBuf> = granted
        .iter()
        .map(|g| g.canonicalize().unwrap_or_else(|_| g.clone()))
        .collect();
    crate::sandbox::escape_surfaces()
        .iter()
        .filter(|p| granted.iter().any(|g| p.path.starts_with(g)))
        // 不给预建的、又还不存在的,只能放过 —— srt-win 对缺失的 deny 目标
        // 一律建 placeholder,没有"只在存在时才打"的模式。
        .filter(|p| p.plant || p.path.exists())
        .map(ProtectedPathExt::stamp_target)
        .collect()
}

/// 把 [`crate::sandbox::ProtectedPath`] 翻成 `acl stamp` 认的目标串。
trait ProtectedPathExt {
    fn stamp_target(&self) -> String;
}

impl ProtectedPathExt for crate::sandbox::ProtectedPath {
    /// 目录带尾 `\`，文件不带。
    ///
    /// srt-win 的 `create_placeholder_chain` 用尾分隔符区分二者，缺省建
    /// **文件** —— `.cargo\bin` 要是落成文件，rustup 的 shim 目录就没了，
    /// 而且它明说 placeholder 是永久的（restore 只摘 ACE 不删），坏了得用户
    /// 手工收拾。
    fn stamp_target(&self) -> String {
        let s = self.path.to_string_lossy();
        if self.is_dir {
            format!("{}\\", s.trim_end_matches('\\'))
        } else {
            s.into_owned()
        }
    }
}

/// `acl stamp` 的 stdin 载荷。
///
/// 只 deny 写。读不拦：`.cargo\bin` 里的 shim 本来就要能被读（沙箱内的构建
/// 要跑它们），拦读等于让沙箱里的 cargo 直接不可用。要防的是**改**。
fn stamp_payload(paths: &[String]) -> String {
    serde_json::json!({ "denyRead": [], "denyWrite": paths }).to_string()
}

fn grant_payload(paths: &[PathBuf]) -> String {
    let write: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    serde_json::json!({ "read": [], "write": write }).to_string()
}

/// 从 `srt-win user status` 的 JSON 里取沙箱账户 SID。
///
/// `None` = 没装过。判据是 `marker_user_sid` 有值**且** `cred_present`：
/// 前者说明装机标记在,后者说明凭证还在——少了凭证,`exec` 的两跳启动第一
/// 跳就起不来（它要读密码去 `CreateProcessWithLogonW`）。
fn sid_from_user_status(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    if v.get("cred_present")?.as_bool() != Some(true) {
        return None;
    }
    let sid = v.get("marker_user_sid")?.as_str()?;
    (!sid.is_empty()).then(|| sid.to_owned())
}

// ─────────────────────────────────────────────────────────────
// 起子进程那一下
// ─────────────────────────────────────────────────────────────

fn sandbox_user_sid(srt: &SrtWin) -> std::io::Result<Option<String>> {
    Ok(sid_from_user_status(&run_srt(
        srt,
        &["user", "status"],
        None,
    )?))
}

fn grant(srt: &SrtWin, sid: &str, paths: &[PathBuf]) -> std::io::Result<()> {
    let pid = std::process::id().to_string();
    run_srt(
        srt,
        &[
            "acl",
            "grant",
            "--holder-pid",
            &pid,
            "--sandbox-user-sid",
            sid,
        ],
        Some(&grant_payload(paths)),
    )
    .map(drop)
}

/// 同步跑一次 `srt-win <args>`,返回 stdout。
///
/// 用 `std::process` 而不是 tokio:调用点（`activate` / `Drop` 的
/// `spawn_blocking`）本来就是同步上下文,而且这些都是毫秒级的小命令。
/// 真正要捕获输出、管超时和取消的是沙箱内的用户命令,那条走 `wrap` +
/// `inner`,复用 `proc.rs`。
fn run_srt(srt: &SrtWin, args: &[&str], stdin: Option<&str>) -> std::io::Result<String> {
    let (program, prefix) = srt.argv_prefix();
    let full: Vec<&str> = prefix
        .iter()
        .map(String::as_str)
        .chain(args.iter().copied())
        .collect();
    run_srt_in(&program, &full, stdin, None)
}

/// [`run_srt`] 的底座：参数已经拼全，可以指定工作目录。
///
/// 冒烟要用它 —— `srt-win exec` 没有 `--cwd`，沙箱子进程的工作目录就是
/// broker 的工作目录，所以只能在这里设。
fn run_srt_in(
    program: &str,
    args: &[&str],
    stdin: Option<&str>,
    cwd: Option<&Path>,
) -> std::io::Result<String> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let mut cmd = Command::new(program);
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    cmd.args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    no_window(&mut cmd);

    let mut child = cmd.spawn()?;
    if let Some(payload) = stdin {
        child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("srt-win stdin 拿不到"))?
            .write_all(payload.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(std::io::Error::other(format!(
            "srt-win {} 退出码 {:?}: {}",
            args.join(" "),
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// 不弹控制台窗口。GUI 宿主起子进程时不加这个会闪黑框。
#[cfg(windows)]
fn no_window(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}
#[cfg(not(windows))]
fn no_window(_cmd: &mut std::process::Command) {}

/// 建一个会话专属的 temp 子目录 `<%TEMP%>/riot-sbx-<pid>-<纳秒>`。
///
/// 名字带 pid + 纳秒,避免同机多会话/多次激活撞名。
fn make_session_temp() -> std::io::Result<PathBuf> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("riot-sbx-{}-{}", std::process::id(), nonce));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn spec(program: &str, args: &[&str]) -> ProcessSpec {
        ProcessSpec {
            program: program.to_owned(),
            args: args.iter().map(|s| (*s).to_owned()).collect(),
            cwd: PathBuf::from("/work"),
            env: Vec::new(),
            timeout_ms: None,
            sandbox_exempt: false,
        }
    }

    /// multicall 要多一个 `--srt-win`,独立 exe 不要——两者的参数表除此
    /// 之外必须一模一样。
    #[test]
    fn multicall_与独立_exe_只差分发标记() {
        let temp = PathBuf::from("/t");
        let s = spec("bash", &["-c", "echo hi"]);

        let (prog, prefix) = SrtWin::Exe(PathBuf::from("/bin/srt-win.exe")).argv_prefix();
        assert_eq!(prog, "/bin/srt-win.exe");
        let exe_args = exec_args(&prefix, &temp, &s);

        let (prog, prefix) = SrtWin::SelfExe(PathBuf::from("/bin/riot.exe")).argv_prefix();
        assert_eq!(prog, "/bin/riot.exe");
        let self_args = exec_args(&prefix, &temp, &s);

        assert_eq!(self_args[0], SRT_WIN_DISPATCH_ARG1);
        assert_eq!(&self_args[1..], &exe_args[..]);
    }

    /// `--` 必须在,而且用户命令要原样落在它后面。
    ///
    /// 少了它,`bash -c ...` 的 `-c` 会被 srt-win 自己的选项解析吃掉 ——
    /// 表现是所有带短选项的命令都莫名其妙报参数错误。
    #[test]
    fn 用户命令在双横线之后原样传递() {
        let args = exec_args(&[], Path::new("/t"), &spec("bash", &["-c", "ls -la"]));
        let dd = args.iter().position(|a| a == "--").expect("要有 --");
        // 程序名会被 resolve_program 解析成绝对路径（mac 上 /bin/bash，
        // Windows 上 C:\Program Files\Git\bin\bash.EXE），所以只断言它指向
        // bash；后面的参数必须一字不改。
        //
        // 大小写要放平：返回的是**候选路径**的拼写，扩展名来自 PATHEXT
        // （那里是 `.EXE`），不是磁盘上的真实拼写。Windows 路径大小写不敏感，
        // 这个差异对 CreateProcessAsUserW 无所谓 —— 只对断言有所谓。
        let prog = args[dd + 1].to_ascii_lowercase();
        assert!(
            prog.ends_with("bash") || prog.ends_with("bash.exe"),
            "第一个位置该是 bash：{:?}",
            args[dd + 1]
        );
        assert_eq!(&args[dd + 2..], &["-c".to_owned(), "ls -la".to_owned()]);
        assert!(args[..dd].contains(&"exec".to_owned()));
        assert!(args[..dd].contains(&"--quiet".to_owned()));
    }

    /// TMP/TEMP/TMPDIR 三个都要给。TMPDIR 尤其不能漏 —— Git Bash 的 MSYS
    /// 先看它。
    #[test]
    fn 三个_temp_变量都指到会话目录() {
        let args = exec_args(&[], Path::new("/session/tmp"), &spec("cmd", &[]));
        for k in ["TMP", "TEMP", "TMPDIR"] {
            assert!(
                args.contains(&format!("{k}=/session/tmp")),
                "{k} 没指到会话目录:{args:?}"
            );
        }
    }

    /// 会话自己设的环境变量排在 temp 之后 —— 同名时它赢。
    #[test]
    fn 会话环境变量覆盖默认的_temp() {
        let mut s = spec("cmd", &[]);
        s.env = vec![
            ("TMP".into(), "/custom".into()),
            ("FOO".into(), "bar".into()),
        ];
        let args = exec_args(&[], Path::new("/session/tmp"), &s);

        let tmps: Vec<usize> = args
            .iter()
            .enumerate()
            .filter(|(_, a)| a.starts_with("TMP="))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(tmps.len(), 2, "两条 TMP 都该在,由 srt-win 取后者");
        assert_eq!(args[tmps[1]], "TMP=/custom");
        assert!(args.contains(&"FOO=bar".to_owned()));
    }

    /// 值里带 `=` 的环境变量不能被拆坏（`GIT_CONFIG_PARAMETERS` 之类）。
    #[test]
    fn 环境变量的值里可以有等号() {
        let mut s = spec("cmd", &[]);
        s.env = vec![("K".into(), "a=b=c".into())];
        let args = exec_args(&[], Path::new("/t"), &s);
        assert!(args.contains(&"K=a=b=c".to_owned()));
    }

    /// 裸程序名必须解析成绝对路径。
    ///
    /// srt-win 最终走 `CreateProcessAsUserW(lpApplicationName = <程序>, …)`，
    /// 而它非 NULL 时 Windows **不做 PATH 搜索、也不补 .exe** —— 传 `cmd`
    /// 直接 0x80070002。这条测试就是为那次真机失败写的。
    #[test]
    fn 裸程序名按_path_解析成绝对路径() {
        let dir = tempfile::tempdir().expect("临时目录");
        // 扩展名的大小写和下面搜的那个保持一致：Windows 的文件系统大小写
        // 不敏感，返回的是**候选路径**的拼写而不是磁盘上的真名，两边不一致
        // 只会测出这个无关紧要的差异。
        let exe = dir.path().join("mytool.BAT");
        std::fs::write(&exe, "").expect("造一个假可执行文件");

        let path = std::ffi::OsString::from(dir.path());
        let exts = vec![String::new(), ".BAT".to_owned()];
        assert_eq!(
            resolve_program_in("mytool", &path, &exts),
            exe.to_string_lossy(),
            "该按 PATHEXT 补上扩展名并给出绝对路径"
        );
    }

    /// 已经带路径的原样透传 —— 调用方指名道姓了，别替它改。
    #[test]
    fn 带路径的程序名不动() {
        let empty = std::ffi::OsString::new();
        let exts = vec![String::new()];
        for p in [
            r"C:\Windows\System32\cmd.exe",
            "/usr/bin/env",
            r".\local.exe",
        ] {
            assert_eq!(resolve_program_in(p, &empty, &exts), p);
        }
    }

    /// 找不到就原样交出去：srt-win 报的「找不到文件」比我们编一个更准确。
    #[test]
    fn 找不到的程序名原样交出去() {
        let empty = std::ffi::OsString::new();
        assert_eq!(
            resolve_program_in("definitely-not-here", &empty, &[String::new()]),
            "definitely-not-here"
        );
    }

    /// broker 的 PATH 要打底传进去，且排在最前 —— 同名时后写的赢，会话
    /// 和调用方都能覆盖它。
    #[test]
    fn broker_的_path_打底且可被覆盖() {
        let mut s = spec("cmd", &[]);
        s.env = vec![("PATH".into(), "/only-this".into())];
        let args = exec_args(&[], Path::new("/t"), &s);

        let last = args
            .iter()
            .rfind(|a| a.starts_with("PATH="))
            .map(String::as_str);
        assert_eq!(
            last,
            Some("PATH=/only-this"),
            "调用方设的 PATH 必须排在最后：{args:?}"
        );
    }

    #[test]
    fn grant_载荷只给写不给读() {
        let p = grant_payload(&[PathBuf::from("/a"), PathBuf::from("/b")]);
        let v: serde_json::Value = serde_json::from_str(&p).expect("合法 JSON");
        assert_eq!(v["read"].as_array().expect("read 是数组").len(), 0);
        assert_eq!(v["write"][0], "/a");
        assert_eq!(v["write"][1], "/b");
    }

    /// 授权了 `~/.cargo`，就必须筛出它的敏感面。
    ///
    /// 这条钉的是整个 DENY 机制**有没有生效**，而不是它生效得对不对。筛出
    /// 空表是静默的：`stamp_protected` 直接返回 Ok，激活成功，`sandboxed`
    /// 报 true，而 `.cargo\bin` 在可继承 ALLOW 下大敞着。CI 上真出过一次
    /// 苗头 —— `canonicalize` 在 Windows 带 `\\?\` 前缀，两边形式不一致
    /// `starts_with` 就全假。
    #[test]
    fn 授权了缓存树就要筛出它的敏感面() {
        let Some(cargo) = crate::sandbox::home_dir().map(|h| h.join(".cargo")) else {
            eprintln!("拿不到 home，跳过");
            return;
        };
        if !cargo.join("bin").is_dir() {
            eprintln!("这台机器没有 ~/.cargo/bin，跳过");
            return;
        }
        // 故意传**没规范化**的路径：调用方不一定走
        // `SandboxPolicy::workspace_write`，筛选不能依赖它先帮忙规范化。
        let targets = stamp_targets(std::slice::from_ref(&cargo));
        assert!(
            targets.iter().any(|t| t.contains("bin")),
            "授权了 {} 却没筛出 bin —— DENY 一条都不会打，洞是敞开的：{targets:?}",
            cargo.display()
        );
    }

    /// 没授权的地方不打 —— 没有 ALLOW 要压，白打一遍还要在几万文件的树上
    /// 传播 ACE。
    #[test]
    fn 没授权的路径不打_deny() {
        let dir = tempfile::tempdir().expect("临时目录");
        assert!(
            stamp_targets(&[dir.path().to_path_buf()]).is_empty(),
            "一个不相干的目录不该牵出任何 DENY 目标"
        );
    }

    #[test]
    fn stamp_载荷只拦写不拦读() {
        let p = stamp_payload(&["/a".into(), "/b".into()]);
        let v: serde_json::Value = serde_json::from_str(&p).expect("合法 JSON");
        // 拦读会让沙箱内的 cargo 跑不了 `.cargo\bin` 里的 shim。
        assert_eq!(v["denyRead"].as_array().expect("denyRead 是数组").len(), 0);
        assert_eq!(v["denyWrite"][0], "/a");
        assert_eq!(v["denyWrite"][1], "/b");
    }

    /// srt-win 用尾分隔符区分「建目录」和「建文件」,缺省建文件。`.cargo\bin`
    /// 落成文件 = 永久占掉 rustup 的 shim 目录(placeholder 不会被 restore
    /// 删掉),所以这条编码进类型的东西必须钉住。
    #[test]
    fn 目录型敏感面带尾分隔符而文件型不带() {
        use crate::sandbox::ProtectedPath;
        let dir = ProtectedPath {
            path: PathBuf::from("C:\\u\\.cargo\\bin"),
            is_dir: true,
            plant: true,
        };
        let file = ProtectedPath {
            path: PathBuf::from("C:\\u\\.cargo\\config.toml"),
            is_dir: false,
            plant: true,
        };
        assert_eq!(dir.stamp_target(), "C:\\u\\.cargo\\bin\\");
        assert_eq!(file.stamp_target(), "C:\\u\\.cargo\\config.toml");
    }

    /// 已经带尾分隔符的不能变成两条。
    #[test]
    fn 目录型尾分隔符不重复叠加() {
        use crate::sandbox::ProtectedPath;
        let d = ProtectedPath {
            path: PathBuf::from("C:\\u\\.cargo\\bin\\"),
            is_dir: true,
            plant: true,
        };
        assert_eq!(d.stamp_target(), "C:\\u\\.cargo\\bin\\");
    }

    /// 装过的判据是「标记在 **且** 凭证在」。少了凭证,exec 的两跳启动第一
    /// 跳就起不来——那时候再失败就太晚了,决策链已经按沙箱着放宽过了。
    #[test]
    fn 装机判据要标记和凭证同时在() {
        let sid = "S-1-5-21-1-2-3-1001";
        assert_eq!(
            sid_from_user_status(&format!(
                r#"{{"cred_present":true,"marker_user_sid":"{sid}"}}"#
            )),
            Some(sid.to_owned())
        );
        for bad in [
            r#"{"cred_present":false,"marker_user_sid":"S-1-5-21-1"}"#,
            r#"{"cred_present":true,"marker_user_sid":null}"#,
            r#"{"cred_present":true,"marker_user_sid":""}"#,
            r#"{"cred_present":true}"#,
            r#"{}"#,
            r#"不是 JSON"#,
        ] {
            assert_eq!(sid_from_user_status(bad), None, "{bad} 该判成没装");
        }
    }

    /// 改写之后 env 必须清空:它已经翻成 `--env` 了。留着的话 `inner` 会把
    /// 它设到 srt-win（broker）自己的环境上,而沙箱子进程是另一个进程,
    /// 根本看不到——表现是「设了环境变量但命令里读不到」。
    #[test]
    fn 改写后不再把_env_留在_spec_上() {
        let sb = WinSandbox {
            srt: SrtWin::Exe(PathBuf::from("/bin/srt-win.exe")),
            sid: "S-1-5-21-1".to_owned(),
            granted: vec![],
            session_temp: PathBuf::from("/t"),
        };
        let mut s = spec("bash", &["-c", "echo"]);
        s.env = vec![("FOO".into(), "bar".into())];
        s.timeout_ms = Some(1234);
        let w = sb.wrap(s);

        assert!(w.env.is_empty(), "env 要清空");
        assert!(
            w.args.contains(&"FOO=bar".to_owned()),
            "但要出现在 --env 里"
        );
        assert_eq!(w.timeout_ms, Some(1234), "其余字段原样保留");
        assert_eq!(w.cwd, PathBuf::from("/work"));
        // 别把自己也 exempt 了 —— 那样 SandboxedRunner 会直接透传,沙箱白套。
        assert!(!w.sandbox_exempt);
        std::mem::forget(sb); // 别在测试里触发 Drop 去起真进程
    }

    /// 分发标记必须和上游常量一致。写死字面量是因为 mac 上取不到那个符号
    /// （srt-win 整个 crate 是 #![cfg(windows)]）,所以在 Windows 上钉住。
    #[cfg(windows)]
    #[test]
    fn 分发标记与上游常量一致() {
        assert_eq!(SRT_WIN_DISPATCH_ARG1, srt_win::SRT_WIN_DISPATCH_ARG1);
    }
}
