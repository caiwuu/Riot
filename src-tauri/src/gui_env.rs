//! GUI 进程的环境补齐。
//!
//! macOS / Linux 从 Dock / 访达 / `.desktop` 启动的应用，拿不到用户在
//! `.zprofile` / `.zshrc` 里 export 的东西。PATH 只剩
//! `/usr/bin:/bin:/usr/sbin:/sbin`，`SSH_AUTH_SOCK` 要么没有、要么指向
//! 空的系统 agent，`gh` / `nvm` / Homebrew 全不在。
//!
//! `pnpm tauri dev` 从已经 export 过的终端起，宿主继承了那份环境，所以
//! 开发时 `git push`、MCP 的 `npx` 看起来一切正常。打成 `.app` 之后同一
//! 条命令就会变成 `Device not configured` 或 `os error 2`。
//!
//! 业内做法（VS Code `resolveShellEnv`、Zed、JetBrains）是启动时跑一次
//! 用户的登录+交互 shell，把算出来的环境写进当前进程。Bash 工具仍然用
//! `bash -c`、不加 `-l`/`-i` —— 吸入的是**变量**，不是 alias / 函数，
//! 模型看到的命令和终端里跑的是同一套凭证。
//!
//! 终端面板已经用登录 shell（`zsh -l`，见 `term.rs`）绕过了。MCP / Bash /
//! hooks 走的是宿主进程环境，必须在任何子进程起来之前补一次。
//!
//! 豁免理由：宿主启动路径，操作真实 OS 环境和登录 shell。

#![allow(clippy::disallowed_methods)]

use std::collections::HashSet;
use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

/// 等交互式登录 shell 吐出环境的上限。VS Code 默认 10s；`.zshrc` 里
/// 要是有 `compinit` 或网络请求，宁可放弃交互式、退回纯登录 shell，
/// 也不能让窗口一直不出来。
#[cfg(unix)]
const INTERACTIVE_ENV_TIMEOUT: Duration = Duration::from_secs(8);

/// 纯登录 shell（只读 `.zprofile`）更快，也更不容易挂。
#[cfg(unix)]
const LOGIN_ENV_TIMEOUT: Duration = Duration::from_secs(2);

/// 把登录 shell 的环境写进当前进程。
///
/// `[约束]` 必须在任何线程起来之前调用。`env::set_var` 不是线程安全的。
pub fn inherit_login_env() {
    if let Some(vars) = read_shell_env() {
        apply_shell_env(&vars);
        tracing::info!(n = vars.len(), "已吸入登录 shell 的环境");
    }
    fallback_ssh_auth_sock();
    // 登录 shell 没写进 PATH 的常见目录再补一层。nvm 默认只在 `.zshrc`，
    // 交互式吸入成功的话这里是空操作（去重）。
    let current = env::var_os("PATH").unwrap_or_default();
    let extras = env::join_paths(well_known_bins()).unwrap_or_default();
    let merged = merge_paths([&current, &extras]);
    if !merged.is_empty() && merged != current {
        unsafe { env::set_var("PATH", &merged) };
    }
}

/// 给 `--print-env` 用：把当前进程环境打成一段 JSON。
///
/// 登录 shell 里再 exec 一次本二进制，避免解析 `export -p`（zshrc 往
/// stdout 打字会把解析撑破，Zed 为此换过一次实现）。
pub fn print_process_env() {
    let map: std::collections::BTreeMap<String, String> = env::vars().collect();
    match serde_json::to_string(&map) {
        Ok(s) => print!("{s}"),
        Err(e) => eprintln!("print-env 序列化失败: {e}"),
    }
}

fn apply_shell_env(vars: &[(String, String)]) {
    for (k, v) in vars {
        if !keep_imported(k) {
            continue;
        }
        unsafe { env::set_var(k, v) };
    }
}

/// 进程自身的状态、我们稍后要覆盖的 askpass 变量，不能从 shell 原样抄进来。
///
/// `XDG_RUNTIME_DIR` 是 VS Code 的已知坑（#22593）：抄过来会让 GUI 进程
/// 的运行时目录指到另一处。`TMPDIR` 留给 launchd 给这个 .app 的那份。
fn keep_imported(key: &str) -> bool {
    !matches!(
        key,
        "PWD"
            | "OLDPWD"
            | "SHLVL"
            | "_"
            | "PPID"
            | "TMPDIR"
            | "XDG_RUNTIME_DIR"
            | "TERM"
            | "TERMINFO"
            | "TERM_PROGRAM"
            | "TERM_PROGRAM_VERSION"
            | "COLORTERM"
            | "COMMAND_MODE"
            | "ITERM_SESSION_ID"
            | "ITERM_PROFILE"
            | "GIT_ASKPASS"
            | "SSH_ASKPASS"
            | "SSH_ASKPASS_REQUIRE"
            | "GIT_TERMINAL_PROMPT"
            | "RIOT_ASKPASS_SOCK"
            | "RIOT_ASKPASS_EXE"
            | "RIOT_RESOLVING_ENVIRONMENT"
            | "ELECTRON_RUN_AS_NODE"
            | "ELECTRON_NO_ATTACH_CONSOLE"
    )
}

/// 登录 shell 没给出 `SSH_AUTH_SOCK` 时，问 launchd 要系统 agent 的插座。
///
/// Dock 启动的应用经常是这个状态：钥匙在系统 agent 里，但 GUI 进程的
/// 环境里没有这根变量。`launchctl getenv` 是 macOS 上比猜路径更稳的问法。
fn fallback_ssh_auth_sock() {
    if env::var_os("SSH_AUTH_SOCK").is_some() {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("launchctl");
        cmd.args(["getenv", "SSH_AUTH_SOCK"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let Some(out) = run_with_timeout(cmd, Duration::from_millis(400)) else {
            return;
        };
        if !out.status.success() {
            return;
        }
        let sock = String::from_utf8_lossy(&out.stdout);
        let sock = sock.trim();
        if !sock.is_empty() {
            unsafe { env::set_var("SSH_AUTH_SOCK", sock) };
            tracing::info!("已从 launchctl 补上 SSH_AUTH_SOCK");
        }
    }
}

fn read_shell_env() -> Option<Vec<(String, String)>> {
    #[cfg(windows)]
    {
        // 和 VS Code 一样：Windows 的 GUI 进程从用户会话继承环境，不必再
        // 跑一遍 shell。乱跑还会把 `ComSpec` 之类搞乱。
        return None;
    }
    #[cfg(unix)]
    {
        read_unix_shell_env(true).or_else(|| read_unix_shell_env(false))
    }
}

#[cfg(unix)]
fn read_unix_shell_env(interactive: bool) -> Option<Vec<(String, String)>> {
    let exe = env::current_exe().ok()?;
    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_owned());
    let mark = env_mark();
    let quoted_exe = posix_single_quote(&exe.to_string_lossy());
    let command = format!("printf %s '{mark}'; {quoted_exe} --print-env; printf %s '{mark}'");

    let mut cmd = std::process::Command::new(&shell);
    if interactive {
        cmd.arg("-i");
    }
    cmd.args(["-l", "-c", &command])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .env("TERM", "dumb")
        .env("RIOT_RESOLVING_ENVIRONMENT", "1");

    let timeout = if interactive {
        INTERACTIVE_ENV_TIMEOUT
    } else {
        LOGIN_ENV_TIMEOUT
    };
    let output = run_with_timeout(cmd, timeout)?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let json = extract_marked(&raw, &mark)?;
    parse_env_json(json)
}

fn env_mark() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("__RIOT_ENV_{:x}_{:x}__", std::process::id(), nanos)
}

fn posix_single_quote(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

fn extract_marked<'a>(raw: &'a str, mark: &str) -> Option<&'a str> {
    let start = raw.find(mark)? + mark.len();
    let rest = raw.get(start..)?;
    let end = rest.rfind(mark)?;
    Some(rest.get(..end)?.trim())
}

fn parse_env_json(s: &str) -> Option<Vec<(String, String)>> {
    let map: std::collections::BTreeMap<String, String> = serde_json::from_str(s).ok()?;
    Some(map.into_iter().collect())
}

#[cfg(unix)]
fn run_with_timeout(mut cmd: std::process::Command, timeout: Duration) -> Option<std::process::Output> {
    use std::io::Read;
    // 不另起线程：`set_var` 之前进程里必须只有主线程。超时就杀，
    // 避免 `.zshrc` 里的网络请求把启动卡住。
    let mut child = cmd.spawn().ok()?;
    let ticks = (timeout.as_millis() / 20).max(1);
    for _ in 0..ticks {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = Vec::new();
                if let Some(mut r) = child.stdout.take() {
                    let _ = r.read_to_end(&mut stdout);
                }
                return Some(std::process::Output {
                    status,
                    stdout,
                    stderr: Vec::new(),
                });
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return None,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

/// 登录 shell 没写进 PATH、但目录确实在的常见位置。
///
/// nvm 默认只在 `.zshrc` 里，纯 `-l` 拿不到；Homebrew 有人只写在 `.zshrc`。
/// 目录不存在就跳过，不会把 PATH 弄脏。
fn well_known_bins() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/opt/homebrew/sbin"),
        PathBuf::from("/usr/local/bin"),
    ];
    if let Some(home) = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")) {
        let home = PathBuf::from(home);
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".cargo/bin"));
        dirs.push(home.join(".volta/bin"));
        if let Some(nvm) = nvm_default_bin(&home) {
            dirs.push(nvm);
        }
    }
    dirs.into_iter().filter(|p| p.is_dir()).collect()
}

/// `[约束]` 不追 alias 链。`alias/default` 常见值是 `lts/*`，再跳
/// `alias/lts/*`，而那可能指向一个**没装**的版本（nvm 只在 `nvm install`
/// 时更新本地 LTS 表，之后表里的"最新 LTS"就和磁盘脱节了）。顺着别名找
/// 会在真机上空手而归 —— 直接扫 `versions/node/` 里实际存在的，取最高版。
fn nvm_default_bin(home: &Path) -> Option<PathBuf> {
    let versions = home.join(".nvm/versions/node");
    let mut best: Option<(Vec<u64>, PathBuf)> = None;
    for entry in std::fs::read_dir(&versions).ok()? {
        let entry = entry.ok()?;
        let bin = entry.path().join("bin");
        if !bin.is_dir() {
            continue;
        }
        let Some(key) = version_key(&entry.file_name().to_string_lossy()) else {
            continue;
        };
        if best.as_ref().is_none_or(|(k, _)| key > *k) {
            best = Some((key, bin));
        }
    }
    best.map(|(_, bin)| bin)
}

/// `v22.17.1` → `[22, 17, 1]`，用于挑最高版本。解析不了的目录名跳过。
fn version_key(name: &str) -> Option<Vec<u64>> {
    let nums: Vec<u64> = name
        .trim_start_matches('v')
        .split('.')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    (!nums.is_empty()).then_some(nums)
}

/// 按出现顺序合并多段 PATH，去重。前面的优先。
fn merge_paths<I, S>(chunks: I) -> OsString
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for chunk in chunks {
        for p in env::split_paths(chunk.as_ref()) {
            if p.as_os_str().is_empty() || !seen.insert(p.clone()) {
                continue;
            }
            out.push(p);
        }
    }
    env::join_paths(out).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 合并去重且前面的优先() {
        let merged = merge_paths(["/opt/homebrew/bin:/usr/bin", "/usr/bin:/bin", "/bin:/sbin"]);
        let parts: Vec<_> = env::split_paths(&merged).collect();
        assert_eq!(
            parts,
            vec![
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
                PathBuf::from("/sbin"),
            ]
        );
    }

    #[test]
    fn 空段被丢掉() {
        let merged = merge_paths(["", "/usr/bin", ""]);
        let parts: Vec<_> = env::split_paths(&merged).collect();
        assert_eq!(parts, vec![PathBuf::from("/usr/bin")]);
    }

    #[test]
    fn nvm_default_目录不存在就当没有() {
        let tmp = std::env::temp_dir().join(format!("riot-nvm-miss-{}", std::process::id()));
        assert!(nvm_default_bin(&tmp).is_none());
    }

    #[test]
    fn nvm_扫描装了的版本_取最高() {
        // 真机上栽过的坑：alias/default → lts/* → 一个没装的版本。
        // 所以这里刻意不建 alias —— 结果必须只看磁盘上有什么。
        let home = std::env::temp_dir().join(format!("riot-nvm-hit-{}", std::process::id()));
        let nvm = home.join(".nvm");
        for v in ["v18.20.8", "v22.17.1"] {
            std::fs::create_dir_all(nvm.join("versions/node").join(v).join("bin"))
                .expect("建 nvm 版本目录");
        }
        // 一个解析不了的目录名，不该让整个扫描失败。
        std::fs::create_dir_all(nvm.join("versions/node/.DS_Store-not-a-version/bin")).ok();
        let got = nvm_default_bin(&home).expect("该找到");
        assert_eq!(got, nvm.join("versions/node/v22.17.1/bin"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn 版本号比较是数字序不是字典序() {
        // 字典序会认为 v9 > v22。
        assert!(version_key("v22.17.1") > version_key("v9.9.9"));
        assert!(version_key("not-a-version").is_none());
    }

    #[test]
    fn 标记之间抽出_json_即使_zshrc_往_stdout_打过字() {
        let mark = "__RIOT_ENV_test__";
        let raw = format!("compinit ok\n{mark}{{\"SSH_AUTH_SOCK\":\"/tmp/s\",\"PATH\":\"/bin\"}}{mark}\n");
        let json = extract_marked(&raw, mark).expect("有标记");
        let vars = parse_env_json(json).expect("是 JSON");
        assert!(vars.iter().any(|(k, v)| k == "SSH_AUTH_SOCK" && v == "/tmp/s"));
    }

    #[test]
    fn 吸入时丢掉进程状态_留下凭证相关() {
        assert!(keep_imported("SSH_AUTH_SOCK"));
        assert!(keep_imported("PATH"));
        assert!(keep_imported("GH_TOKEN"));
        assert!(keep_imported("GITHUB_TOKEN"));
        assert!(keep_imported("LANG"));
        assert!(!keep_imported("PWD"));
        assert!(!keep_imported("GIT_ASKPASS"));
        assert!(!keep_imported("TMPDIR"));
        assert!(!keep_imported("XDG_RUNTIME_DIR"));
    }

    #[test]
    fn posix_单引号能包住带引号的路径() {
        assert_eq!(posix_single_quote("/Applications/Riot.app/Contents/MacOS/Riot"), "'/Applications/Riot.app/Contents/MacOS/Riot'");
        assert_eq!(posix_single_quote("/tmp/it's here"), "'/tmp/it'\\''s here'");
    }
}
