//! Git / SSH 的 askpass：无 TTY 时把提问转到宿主弹窗。
//!
//! Agent 的 Bash 工具 stdin 是 `/dev/null`、没有控制终端。Git 在
//! credential helper 没吐出账号时会去开 `/dev/tty`，于是打出
//! `could not read Username for 'https://github.com': Device not configured`。
//!
//! 业内做法（VS Code Git 扩展、GitHub Desktop）是设 `GIT_ASKPASS` /
//! `SSH_ASKPASS` 指向一个小助手：git 把提示词当参数传来，助手把答案
//! 打到 stdout。助手本身再跟 IDE 说话，由 IDE 弹输入框。
//!
//! 这里同构：启动时写一份包装脚本、起一个本机 socket 服务。脚本只是
//! `exec $本二进制 --askpass`，真正的窗口在宿主进程里弹 —— 沙箱里的
//! `git` 子进程不必自己碰 Apple Events。
//!
//! 豁免理由：宿主启动路径，写临时脚本、绑本机 socket、弹系统对话框。

#![allow(clippy::disallowed_methods)]

use std::env;
use std::io::{Read, Write};
use std::path::PathBuf;

/// 写包装脚本、把 `GIT_ASKPASS` 写进当前进程、起监听线程。
///
/// `[约束]` `set_var` 必须在监听线程起来之前做完。
pub fn install() {
    let Ok(exe) = env::current_exe() else {
        tracing::warn!("拿不到本二进制路径，GIT_ASKPASS 装不上");
        return;
    };
    let Some((script, sock)) = write_wrapper() else {
        tracing::warn!("写 askpass 包装脚本失败");
        return;
    };

    unsafe {
        env::set_var("GIT_ASKPASS", &script);
        env::set_var("SSH_ASKPASS", &script);
        env::set_var("RIOT_ASKPASS_EXE", &exe);
        env::set_var("RIOT_ASKPASS_SOCK", &sock);
    }

    if !bind_and_serve(sock) {
        tracing::warn!("askpass 监听没起来，git 仍会走 GIT_TERMINAL_PROMPT=0 那条失败");
        return;
    }
    tracing::info!(script = %script.display(), "已安装 GIT_ASKPASS");
}

/// `--askpass` 入口：连回正在跑的宿主，把 git 的提示词送过去。
pub fn run_client(prompt: &str) -> i32 {
    match ask_host(prompt) {
        Some(secret) => {
            print!("{secret}");
            0
        }
        None => 1,
    }
}

fn write_wrapper() -> Option<(PathBuf, PathBuf)> {
    let dir = env::temp_dir();
    let pid = std::process::id();
    let sock = dir.join(format!("riot-askpass-{pid}.sock"));
    #[cfg(windows)]
    let script = dir.join(format!("riot-askpass-{pid}.cmd"));
    #[cfg(not(windows))]
    let script = dir.join(format!("riot-askpass-{pid}.sh"));

    #[cfg(windows)]
    {
        let body = "@echo off\r\n\"%RIOT_ASKPASS_EXE%\" --askpass %*\r\n";
        std::fs::write(&script, body).ok()?;
    }
    #[cfg(not(windows))]
    {
        let body = "#!/bin/sh\nexec \"$RIOT_ASKPASS_EXE\" --askpass \"$@\"\n";
        std::fs::write(&script, body).ok()?;
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(&script).ok()?.permissions();
        perm.set_mode(0o700);
        std::fs::set_permissions(&script, perm).ok()?;
    }
    Some((script, sock))
}

#[cfg(unix)]
fn bind_and_serve(sock: PathBuf) -> bool {
    let _ = std::fs::remove_file(&sock);
    let listener = match std::os::unix::net::UnixListener::bind(&sock) {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(error = %e, "askpass socket 绑不上");
            return false;
        }
    };
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(&sock) {
        let mut perm = meta.permissions();
        perm.set_mode(0o600);
        let _ = std::fs::set_permissions(&sock, perm);
    }
    std::thread::Builder::new()
        .name("riot-askpass".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut prompt = String::new();
                if s.read_to_string(&mut prompt).is_err() {
                    continue;
                }
                let reply = match native_prompt(prompt.trim_end()) {
                    Some(secret) => format!("ok\n{secret}"),
                    None => "cancel\n".to_owned(),
                };
                let _ = s.write_all(reply.as_bytes());
            }
        })
        .is_ok()
}

#[cfg(windows)]
fn bind_and_serve(sock: PathBuf) -> bool {
    // Windows 用 loopback TCP，端口写进同名文件，客户端按文件里的端口连。
    let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(error = %e, "askpass 端口绑不上");
            return false;
        }
    };
    let port = match listener.local_addr() {
        Ok(a) => a.port(),
        Err(_) => return false,
    };
    if std::fs::write(&sock, port.to_string()).is_err() {
        return false;
    }
    std::thread::Builder::new()
        .name("riot-askpass".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut prompt = String::new();
                if s.read_to_string(&mut prompt).is_err() {
                    continue;
                }
                let reply = match native_prompt(prompt.trim_end()) {
                    Some(secret) => format!("ok\n{secret}"),
                    None => "cancel\n".to_owned(),
                };
                let _ = s.write_all(reply.as_bytes());
            }
        })
        .is_ok()
}

#[cfg(unix)]
fn ask_host(prompt: &str) -> Option<String> {
    use std::os::unix::net::UnixStream;
    let sock = env::var_os("RIOT_ASKPASS_SOCK")?;
    let mut stream = UnixStream::connect(sock).ok()?;
    stream.write_all(prompt.as_bytes()).ok()?;
    stream.shutdown(std::net::Shutdown::Write).ok()?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply).ok()?;
    parse_reply(&reply)
}

#[cfg(windows)]
fn ask_host(prompt: &str) -> Option<String> {
    let path = env::var_os("RIOT_ASKPASS_SOCK")?;
    let port: u16 = std::fs::read_to_string(path).ok()?.trim().parse().ok()?;
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.write_all(prompt.as_bytes()).ok()?;
    stream.shutdown(std::net::Shutdown::Write).ok()?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply).ok()?;
    parse_reply(&reply)
}

fn parse_reply(reply: &str) -> Option<String> {
    let (status, rest) = reply.split_once('\n')?;
    if status.trim() != "ok" {
        return None;
    }
    Some(rest.trim_end_matches(['\r', '\n']).to_owned())
}

fn native_prompt(prompt: &str) -> Option<String> {
    if prompt.is_empty() {
        return None;
    }
    #[cfg(target_os = "macos")]
    {
        macos_dialog(prompt)
    }
    #[cfg(target_os = "linux")]
    {
        linux_dialog(prompt)
    }
    #[cfg(windows)]
    {
        windows_dialog(prompt)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        let _ = prompt;
        None
    }
}

// Windows 的 InputBox 没有密码模式，这个判断只有 mac / Linux 的
// 对话框用得上。
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn looks_secret(prompt: &str) -> bool {
    let p = prompt.to_ascii_lowercase();
    p.contains("password")
        || p.contains("passphrase")
        || p.contains("密码")
        || p.contains("口令")
}

#[cfg(target_os = "macos")]
fn macos_dialog(prompt: &str) -> Option<String> {
    let hidden = if looks_secret(prompt) {
        " with hidden answer"
    } else {
        ""
    };
    let script = format!(
        "try\n\
         set r to display dialog {q} default answer \"\"{hidden} buttons {{\"取消\", \"好\"}} default button \"好\" with title \"Riot\"\n\
         return text returned of r\n\
         on error\n\
         error number 1\n\
         end try",
        q = applescript_string(prompt),
    );
    let out = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim_end().to_owned())
}

#[cfg(target_os = "macos")]
fn applescript_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(target_os = "linux")]
fn linux_dialog(prompt: &str) -> Option<String> {
    let secret = looks_secret(prompt);
    for (bin, args) in [
        (
            "zenity",
            if secret {
                vec!["--password".into(), "--title=Riot".into(), format!("--text={prompt}")]
            } else {
                vec![
                    "--entry".into(),
                    "--title=Riot".into(),
                    format!("--text={prompt}"),
                ]
            },
        ),
        (
            "kdialog",
            if secret {
                vec!["--title".into(), "Riot".into(), "--password".into(), prompt.into()]
            } else {
                vec!["--title".into(), "Riot".into(), "--inputbox".into(), prompt.into()]
            },
        ),
    ] {
        if let Ok(out) = std::process::Command::new(bin).args(args).output()
            && out.status.success()
        {
            return Some(String::from_utf8_lossy(&out.stdout).trim_end().to_owned());
        }
    }
    None
}

#[cfg(windows)]
fn windows_dialog(prompt: &str) -> Option<String> {
    let escaped = prompt.replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName Microsoft.VisualBasic; [Microsoft.VisualBasic.Interaction]::InputBox('{escaped}','Riot')"
    );
    // CREATE_NO_WINDOW:藏的是 PowerShell 自己的黑色控制台窗，InputBox
    // 弹窗是它进程里另开的 GUI 窗口，照常显示。不藏的话每次凭据询问都是
    // "黑窗 + 弹窗"一起出现。
    use std::os::windows::process::CommandExt as _;
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim_end().to_owned();
    if s.is_empty() { None } else { Some(s) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn 密码类提示走隐藏输入() {
        assert!(looks_secret("Password for 'https://github.com':"));
        assert!(looks_secret("Enter passphrase for key '/Users/me/.ssh/id_ed25519':"));
        assert!(looks_secret("请输入密码"));
        assert!(!looks_secret("Username for 'https://github.com':"));
        assert!(!looks_secret("Are you sure you want to continue connecting (yes/no)?"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn applescript_转义引号和反斜杠() {
        assert_eq!(applescript_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }

    #[test]
    fn 应答协议_ok_带密码_cancel_是拒绝() {
        assert_eq!(parse_reply("ok\nsecret\n").as_deref(), Some("secret"));
        assert_eq!(parse_reply("ok\n").as_deref(), Some(""));
        assert_eq!(parse_reply("cancel\n"), None);
        assert_eq!(parse_reply("nope"), None);
    }
}
