//! GUI 进程的环境补齐。
//!
//! macOS 从 Dock / 访达启动的应用，PATH 只有 `/usr/bin:/bin:/usr/sbin:/sbin`。
//! 用户装的 `npx`、`uvx`、`node`、`brew` 都不在里面。
//!
//! `pnpm tauri dev` 从已经 export 过的终端起，宿主继承了那份 PATH，所以
//! 开发时 MCP 看起来一切正常。打成 `.app` 之后同一份配置就会报
//! `No such file or directory (os error 2)`。
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
use std::process::{Command, Stdio};
#[cfg(unix)]
use std::time::Duration;

/// 等登录 shell 吐出 PATH 的上限。`.zprofile` 里要是有网络请求，宁可
/// 用常见目录兜底，也不能让窗口一直不出来。
#[cfg(unix)]
const LOGIN_PATH_TIMEOUT: Duration = Duration::from_secs(2);

/// 把登录 shell 的 PATH 和常见用户目录写进当前进程。
///
/// `[约束]` 必须在任何线程起来之前调用。`env::set_var` 不是线程安全的。
pub fn inherit_login_path() {
    let current = env::var_os("PATH").unwrap_or_default();
    let login = read_login_path().unwrap_or_default();
    let extras = env::join_paths(well_known_bins()).unwrap_or_default();
    let merged = merge_paths([&login, &extras, &current]);
    if merged.is_empty() || merged == current {
        return;
    }
    // SAFETY: 只在 `run()` 入口、restore / tokio 之前调用一次。
    unsafe { env::set_var("PATH", &merged) };
    tracing::info!("已补上登录 shell 的 PATH，MCP 和工具子进程都能找到 npx / uvx");
}

/// 跑一次登录 shell，把它算出来的 PATH 拿回来。
///
/// 只用 `-l`、不加 `-i`：`.zshrc` 里常有 `compinit`、提示符、甚至
/// `read`，交互式启动会挂死。这和 Terminal.app / `term.rs` 同一层。
fn read_login_path() -> Option<OsString> {
    #[cfg(windows)]
    {
        return None;
    }
    #[cfg(unix)]
    {
        let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_owned());
        let mut cmd = Command::new(shell);
        cmd.args(["-l", "-c", "printf %s \"$PATH\""])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env("TERM", "dumb");
        let output = run_with_timeout(cmd, LOGIN_PATH_TIMEOUT)?;
        if !output.status.success() {
            return None;
        }
        let path = String::from_utf8_lossy(&output.stdout);
        let path = path.trim();
        (!path.is_empty()).then(|| OsString::from(path))
    }
}

#[cfg(unix)]
fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Option<std::process::Output> {
    use std::io::Read;
    // 不另起线程：`set_var` 之前进程里必须只有主线程。超时就杀，
    // 避免 `.zprofile` 里的网络请求把启动卡住。
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
/// nvm 默认只在 `.zshrc` 里，`-l` 拿不到；Homebrew 有人只写在 `.zshrc`。
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
}
