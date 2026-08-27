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
            // 按 holder pid 回收本会话的全部 ALLOW ACE。srt-win 内部按路径
            // 引用计数,归零才真撤——同机另一个会话正用着同一个工作区时,
            // 它的 ACE 不会被我们连坐撤掉。
            match run_srt(
                &srt,
                &[
                    "acl",
                    "revoke",
                    "--holder-pid",
                    &std::process::id().to_string(),
                    "--sandbox-user-sid",
                    &sid,
                    "--json",
                ],
                None,
            ) {
                Ok(_) => tracing::debug!(paths = n, "沙箱授权已回收"),
                // 撤不掉不 panic:srt-win 的状态库在下一次 acl 操作时会跑
                // 崩溃恢复(它的 state_db 模块头写明"crash-recovery runs
                // unconditionally at every acquire"),孤儿 ACE 由那条路兜底。
                Err(e) => tracing::warn!(error = %e, "回收沙箱授权失败,留给下次崩溃恢复"),
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

    if let Err(e) = grant(&srt, &sid, &granted) {
        tracing::warn!(error = %e, "给沙箱账户授权可写目录失败,本轮不隔离");
        let _ = std::fs::remove_dir_all(&session_temp);
        return None;
    }

    Some(WinSandbox {
        srt,
        sid,
        granted,
        session_temp,
    })
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

    for (k, v) in session_temp_env(session_temp)
        .into_iter()
        .chain(spec.env.iter().cloned())
    {
        args.push("--env".to_owned());
        args.push(format!("{k}={v}"));
    }

    args.push("--".to_owned());
    args.push(spec.program.clone());
    args.extend(spec.args.iter().cloned());
    args
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
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let (program, prefix) = srt.argv_prefix();
    let mut cmd = Command::new(program);
    cmd.args(&prefix)
        .args(args)
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
        assert_eq!(
            &args[dd + 1..],
            &["bash".to_owned(), "-c".to_owned(), "ls -la".to_owned()]
        );
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

    #[test]
    fn grant_载荷只给写不给读() {
        let p = grant_payload(&[PathBuf::from("/a"), PathBuf::from("/b")]);
        let v: serde_json::Value = serde_json::from_str(&p).expect("合法 JSON");
        assert_eq!(v["read"].as_array().expect("read 是数组").len(), 0);
        assert_eq!(v["write"][0], "/a");
        assert_eq!(v["write"][1], "/b");
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
