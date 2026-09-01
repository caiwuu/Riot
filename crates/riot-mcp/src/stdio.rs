//! stdio 传输：把 MCP 服务器作为子进程拉起来。
//!
//! 豁免理由：这里的职责就是操作真实进程。MCP 服务器是长驻子进程，
//! 不走 `ProcessRunner`（那个抽象是"跑一条命令等它结束"，形状不对）。
#![allow(clippy::disallowed_methods)]

use std::process::Stdio;
use std::time::Duration;

use process_wrap::tokio::{ChildWrapper, CommandWrap};

use crate::hub::ServerSpec;

/// SIGTERM 到 SIGKILL 之间的宽限期。和 riot-runtime 的命令执行器一致。
const KILL_GRACE: Duration = Duration::from_millis(500);

/// 允许传给 MCP 服务器进程的宿主环境变量。
///
/// `[约束]` 白名单，不是黑名单。MCP 服务器是第三方代码（`npx <package>`
/// 是标准形态），继承整个 env 意味着它能读到宿主的一切 —— 而 API key
/// 恰恰可以来自环境变量。黑名单挡不住这个：密钥的变量名由用户和各家
/// SDK 决定，列不全。
///
/// 收进来的每一个都要能说出理由：少了它某类服务器起不来。
#[cfg(unix)]
const INHERITED: &[&str] = &[
    // 找得到解释器和工具链。没有它连 node / python 都找不到。
    "PATH",
    // npm/pip/uv 的缓存和配置都在 ~ 下；缺了会退化成每次现下载，
    // 或者干脆报"权限不足"（它会去写 / 下面）。
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    // 语言环境。缺了 Python 会按 ASCII 解码，中文路径直接抛异常。
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    // 临时目录。缺了写到 /tmp，沙箱环境下可能不可写。
    "TMPDIR",
    // `[取舍]` 代理放行。公司网里 `npx -y` 冷启动要现下包，没有代理就是
    // 卡到超时，而报错("没有响应")完全不指向"下不动包"。代理地址不是
    // 本应用的密钥；用户若把凭证塞进代理 URL，那份凭证本来就发给了
    // 每一个联网的进程。
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
];

/// Windows 的必需集完全是另一套。
///
/// `[约束]` 少了 `SYSTEMROOT` 连 winsock 都初始化不了（表象是所有网络
/// 调用直接失败），少了 `PATHEXT` 则 `npx` / `npm` 这类 `.cmd` 启动器
/// 找不到 —— 都是"清干净之后服务器整个起不来"的级别。
#[cfg(windows)]
const INHERITED: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "PROGRAMW6432",
    "HOMEDRIVE",
    "HOMEPATH",
    "USERNAME",
    "COMPUTERNAME",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "OS",
    // 理由同 unix 侧。
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
];

/// 环境变量名的比较口径。Windows 上是大小写无关的（`Path` 和 `PATH`
/// 是同一个变量），按大小写敏感比会同时传进两份，取哪份看运气。
fn env_key_eq(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// 算出子进程真正拿到的环境：白名单里的宿主变量 + `spec.env` 的显式配置。
///
/// 是纯函数而不是直接操作 `Command`，这样"密钥有没有漏过去"能直接
/// 摆进测试里，不用起进程。
fn child_env<I>(host: I, spec: &[(String, String)]) -> Vec<(String, std::ffi::OsString)>
where
    I: IntoIterator<Item = (String, std::ffi::OsString)>,
{
    let mut out: Vec<(String, std::ffi::OsString)> = host
        .into_iter()
        .filter(|(k, _)| INHERITED.iter().any(|allowed| env_key_eq(allowed, k)))
        .collect();

    // 用户在这个服务器上显式配的覆盖宿主的同名值 —— 他配 `PATH` 就是
    // 想换一个 PATH，而不是想要两份。
    for (k, v) in spec {
        match out.iter_mut().find(|(existing, _)| env_key_eq(existing, k)) {
            Some(slot) => slot.1 = v.into(),
            None => out.push((k.clone(), v.into())),
        }
    }
    out
}

/// 单条 stderr 日志行的上限。比 JSON-RPC 帧小得多 —— 这一路只进日志，
/// 超过几十 KB 的"一行"本来也没人读得下去。
const MAX_LOG_LINE: usize = 64 * 1024;

#[cfg(unix)]
const SIGTERM: i32 = 15;

pub(crate) struct SpawnedServer {
    pub child: Box<dyn ChildWrapper>,
    pub stdout: tokio::process::ChildStdout,
    pub stdin: tokio::process::ChildStdin,
}

/// 把 `CREATE_SUSPENDED | CREATE_NO_WINDOW` 焊在 spawn 前的最后一笔，
/// 否则打包后的 GUI 主程序每连一个 MCP 服务器就弹一个黑色控制台窗
/// （dev 下继承终端的控制台，看不出来）。
///
/// `[约束]` 必须注册在 JobObject **之后**、不能用直接 creation_flags 或
/// 库自带的 CreationFlags 包装器 —— 前者被 JobObject 整个改写，后者
/// 在 process-wrap 9.1.0 里因为 spawn 时包装器表被 mem::take 拿空而
/// 永远读不到。完整分析见 riot-runtime 的命令执行器。
#[cfg(windows)]
#[derive(Debug)]
struct NoWindow;

#[cfg(windows)]
impl process_wrap::tokio::CommandWrapper for NoWindow {
    fn pre_spawn(
        &mut self,
        command: &mut tokio::process::Command,
        _core: &CommandWrap,
    ) -> std::io::Result<()> {
        use windows::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};
        command.creation_flags((CREATE_NO_WINDOW | CREATE_SUSPENDED).0);
        Ok(())
    }
}

/// 拉起服务器进程。stderr 由后台任务转进日志 —— MCP 规范允许服务器往
/// stderr 打日志，不接的话管道写满 64KB 后服务器会整个卡住。
pub(crate) fn spawn_server(spec: &ServerSpec) -> std::io::Result<SpawnedServer> {
    let mut cmd = tokio::process::Command::new(&spec.command);
    cmd.args(&spec.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // 兜底：句柄被 drop 时至少杀掉直接子进程。整组清理走 terminate。
        .kill_on_drop(true)
        // `[约束]` 先清空再按白名单填。追加式的 `env(k, v)` 会让子进程
        // 继承宿主的全部环境，包括从环境变量读来的 API key。
        .env_clear();

    // 名字不是合法 UTF-8 的变量直接跳过（白名单里全是 ASCII 名，不可能
    // 命中）。值保持 OsString —— PATH 里出现非 UTF-8 分量在真实机器上
    // 是有的，按 UTF-8 过滤会把它整条丢掉，服务器随即找不到解释器。
    let host = std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v)));
    for (k, v) in child_env(host, &spec.env) {
        cmd.env(k, v);
    }

    // `[约束]` 必须包进程组 / Job Object。`npx foo` 这种启动器会再 spawn
    // 真正的服务器进程 —— 只杀 npx 的话，真身被 init 收养，一直活到关机。
    // 和 riot-runtime 的命令执行器同一条规矩（ARCHITECTURE.md §2.3）。
    let mut wrap = CommandWrap::from(cmd);
    #[cfg(unix)]
    wrap.wrap(process_wrap::tokio::ProcessGroup::leader());
    #[cfg(windows)]
    {
        wrap.wrap(process_wrap::tokio::JobObject);
        wrap.wrap(NoWindow);
    }

    let mut child = wrap.spawn()?;
    let stdout = child.stdout().take().expect("stdout 已设为 piped");
    let stdin = child.stdin().take().expect("stdin 已设为 piped");
    let stderr = child.stderr().take().expect("stderr 已设为 piped");

    let server_id = spec.id.clone();
    tokio::spawn(async move {
        let mut reader = tokio::io::BufReader::new(stderr);
        let mut buf = Vec::new();
        // `[约束]` 按字节读行、有损解码，不能用 `lines()`：它一遇到非 UTF-8
        // 字节就返回 Err，循环退出、stderr 读端被 drop —— 服务器下一次写
        // stderr 直接撞上断掉的管道，Python 会带着打不出来的 traceback
        // 整个退出（真实案例：FastMCP 的启动横幅经 Windows ANSI 码页编码，
        // 横幅打到一半进程就死，表象是"握手前连接断开"）。排水必须活到 EOF。
        //
        // `[约束]` 同样必须带上限。日志流是最容易失控的一路（死循环里
        // 打日志、把二进制数据打到 stderr），而它的内容只进 tracing，
        // 丢一行的代价远小于把宿主吃到 OOM。
        loop {
            match crate::lines::read_line_capped(&mut reader, &mut buf, MAX_LOG_LINE).await {
                crate::lines::ReadLine::Eof => break,
                crate::lines::ReadLine::TooLong => {
                    tracing::debug!(server = %server_id, limit = MAX_LOG_LINE, "stderr 有一行超过上限，已丢弃");
                }
                crate::lines::ReadLine::Line => {
                    let line = String::from_utf8_lossy(&buf);
                    tracing::debug!(server = %server_id, "{}", line.trim_end());
                }
            }
        }
    });

    Ok(SpawnedServer {
        child,
        stdout,
        stdin,
    })
}

/// 停掉整个进程组：先 SIGTERM 给它收尾的机会，再无条件 SIGKILL。
///
/// `[约束]` 不能只在异常路径杀。服务器正常退出 ≠ 它 spawn 的子进程也
/// 退出了 —— 那些被收养的孤儿比"卡死"隐蔽得多。
pub(crate) async fn terminate(mut child: Box<dyn ChildWrapper>) {
    #[cfg(unix)]
    {
        if child.signal(SIGTERM).is_ok() {
            let _ = tokio::time::timeout(KILL_GRACE, child.wait()).await;
        }
    }
    if let Err(e) = child.start_kill() {
        tracing::debug!(error = %e, "清理 MCP 服务器进程组");
    }
    let _ = tokio::time::timeout(KILL_GRACE, child.wait()).await;
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    fn host(pairs: &[(&str, &str)]) -> Vec<(String, OsString)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), OsString::from(*v)))
            .collect()
    }

    fn get<'a>(env: &'a [(String, OsString)], key: &str) -> Option<&'a OsString> {
        env.iter().find(|(k, _)| env_key_eq(k, key)).map(|(_, v)| v)
    }

    #[test]
    fn 宿主的_api_key_不进子进程环境() {
        // 第三方 MCP 服务器读到的是这份环境。宿主的密钥在里面的话，
        // 一个"只是列文件"的服务器就能把用户的 key 发走，而全程没有
        // 任何可见迹象。
        let env = child_env(
            host(&[
                ("ANTHROPIC_API_KEY", "sk-ant-绝密"),
                ("OPENAI_API_KEY", "sk-绝密"),
                ("AWS_SECRET_ACCESS_KEY", "绝密"),
                ("GITHUB_TOKEN", "ghp_绝密"),
                ("PATH", "/usr/bin"),
            ]),
            &[],
        );

        for leaked in [
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "AWS_SECRET_ACCESS_KEY",
            "GITHUB_TOKEN",
        ] {
            assert!(get(&env, leaked).is_none(), "{leaked} 漏进了子进程");
        }
        assert_eq!(
            get(&env, "PATH").map(OsString::as_os_str),
            Some("/usr/bin".as_ref()),
            "PATH 必须留着，否则连解释器都找不到"
        );
    }

    #[test]
    fn 用户显式配的变量能传进去也能覆盖白名单() {
        // spec.env 是用户自己写的，那是他明确要给这个服务器的东西。
        let env = child_env(
            host(&[("PATH", "/usr/bin"), ("SECRET", "绝密")]),
            &[
                ("MY_SERVER_TOKEN".into(), "配给这个服务器的".into()),
                ("PATH".into(), "/opt/custom/bin".into()),
            ],
        );

        assert_eq!(
            get(&env, "MY_SERVER_TOKEN").map(OsString::as_os_str),
            Some("配给这个服务器的".as_ref())
        );
        assert_eq!(
            get(&env, "PATH").map(OsString::as_os_str),
            Some("/opt/custom/bin".as_ref()),
            "配了就是想换一个，不是想要两份"
        );
        assert_eq!(
            env.iter().filter(|(k, _)| env_key_eq(k, "PATH")).count(),
            1,
            "同名变量传两份的话，取哪份看运气"
        );
        assert!(get(&env, "SECRET").is_none());
    }

    /// Windows 的必需集和 unix 完全不同，漏一个就是"服务器整个起不来"。
    /// 这条断言在 macOS / Linux 上编不进来，改动 `INHERITED` 时要靠
    /// Windows 上的 CI 兜住。
    #[cfg(windows)]
    #[test]
    fn windows_的必需变量不能被清掉() {
        let env = child_env(
            host(&[
                ("Path", "C:\\Windows\\System32"),
                ("SystemRoot", "C:\\Windows"),
                ("PATHEXT", ".COM;.EXE;.CMD"),
                ("TEMP", "C:\\Temp"),
                ("ANTHROPIC_API_KEY", "sk-ant-绝密"),
            ]),
            &[],
        );

        // 少了 SystemRoot 连 winsock 都初始化不了；少了 PATHEXT 找不到
        // npx / npm 这类 .cmd 启动器。
        for needed in ["Path", "SystemRoot", "PATHEXT", "TEMP"] {
            assert!(get(&env, needed).is_some(), "{needed} 被清掉了");
        }
        assert!(get(&env, "ANTHROPIC_API_KEY").is_none());
    }

    /// Windows 上 `Path` 和 `PATH` 是同一个变量。按大小写敏感比的话，
    /// 用户配的 `PATH` 会和宿主的 `Path` 同时传进去。
    #[test]
    fn 变量名比较跟着平台的口径走() {
        assert_eq!(env_key_eq("Path", "PATH"), cfg!(windows));
        assert!(env_key_eq("PATH", "PATH"));
    }
}
