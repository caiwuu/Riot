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
//! # 谁被允许让宿主弹这个窗
//!
//! `[约束]` 连上来的客户端必须先发一行口令，对不上就断开、**一个窗都不弹**。
//! 弹窗本身就是攻击目标：一个标题写着 Riot 的密码框，用户没有任何办法看出
//! 它是谁要的。口令只写在会合文件里（unix 上还套一层 0700 的私有目录），
//! 靠文件权限决定谁拿得到 —— Windows 上那条 loopback TCP 对本机任意进程
//! 都是可连的，文件权限是那里唯一的边界。
//!
//! 由此而来的一个**预期内**的结果：Windows 沙箱里的命令跑在另一个本地账户
//! 下（见 `riot_runtime::sandbox_win`），它连得上端口但读不到用户 profile 里的
//! 会合文件，所以沙箱里的 git 问不出凭据、直接失败。这不是回归 —— 沙箱的
//! 意思就是"这条命令不配拿用户的凭据"，让它有办法弹一个 Riot 密码框，
//! 恰恰是上面那个攻击的原型。
//!
//! 豁免理由：宿主启动路径，写临时脚本、绑本机 socket、弹系统对话框。

#![allow(clippy::disallowed_methods)]

use std::env;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// 一次运行的会合点。都放在一个私有目录里，删起来也是一次。
struct Rendezvous {
    /// `GIT_ASKPASS` 指向的包装脚本。
    script: PathBuf,
    /// 客户端按 `RIOT_ASKPASS_ENDPOINT` 找到的文件：第一行端点
    /// （unix 是 socket 路径，Windows 是端口），第二行口令。
    endpoint: PathBuf,
    #[cfg(unix)]
    sock: PathBuf,
}

/// 写包装脚本、把 `GIT_ASKPASS` 写进当前进程、起监听线程。
///
/// `[约束]` `set_var` 必须在监听线程起来之前做完。
pub fn install() {
    let Ok(exe) = env::current_exe() else {
        tracing::warn!("拿不到本二进制路径，GIT_ASKPASS 装不上");
        return;
    };
    let Some(rv) = write_wrapper() else {
        tracing::warn!("写 askpass 包装脚本失败");
        return;
    };

    unsafe {
        env::set_var("GIT_ASKPASS", &rv.script);
        env::set_var("SSH_ASKPASS", &rv.script);
        env::set_var("RIOT_ASKPASS_EXE", &exe);
        env::set_var("RIOT_ASKPASS_ENDPOINT", &rv.endpoint);
    }

    if !bind_and_serve(&rv, new_token()) {
        tracing::warn!("askpass 监听没起来，git 仍会走 GIT_TERMINAL_PROMPT=0 那条失败");
        return;
    }
    tracing::info!(script = %rv.script.display(), "已安装 GIT_ASKPASS");
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

/// 建私有目录、在里面原子地写出包装脚本。
fn write_wrapper() -> Option<Rendezvous> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    // 目录名带 nonce 而不是只带 pid：pid 会复用，撞上一次残留就得在"复用
    // 别人留下的目录"和"整个 askpass 装不上"之间二选一，两个都不能接受。
    let dir = env::temp_dir().join(format!(
        "riot-askpass-{}-{:x}",
        std::process::id(),
        nonce as u64
    ));
    create_private_dir(&dir).ok()?;

    #[cfg(windows)]
    let (script, body) = (
        dir.join("askpass.cmd"),
        "@echo off\r\n\"%RIOT_ASKPASS_EXE%\" --askpass %*\r\n",
    );
    #[cfg(not(windows))]
    let (script, body) = (
        dir.join("askpass.sh"),
        "#!/bin/sh\nexec \"$RIOT_ASKPASS_EXE\" --askpass \"$@\"\n",
    );
    write_private_file(&script, body, 0o700).ok()?;

    Some(Rendezvous {
        script,
        endpoint: dir.join("endpoint"),
        #[cfg(unix)]
        sock: dir.join("sock"),
    })
}

/// 建一个只有自己进得去的目录。
///
/// `[约束]` 权限必须在创建那一刻带上，且不复用已存在的目录。`/tmp` 是所有
/// 本地用户共用的：先建后 chmod 的那扇窗里，别人能进去把包装脚本换掉，而
/// 那个脚本会被 git / ssh 以当前用户身份执行。
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new().mode(0o700).create(path)
    }
    // Windows 的 %TEMP% 在用户 profile 里，ACL 默认只有本人和管理员。
    #[cfg(not(unix))]
    std::fs::create_dir(path)
}

/// 原子地创建一个私有文件并写满它。
///
/// `[约束]` `create_new` + 建文件时就给 `mode`。分成"先 write 再
/// set_permissions"的话，文件有一瞬间是 0644：共享 `/tmp` 上另一个本地
/// 用户能在这两步之间打开它并留着写句柄，之后随时改写内容。
fn write_private_file(path: &Path, body: &str, mode: u32) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    opts.open(path)?.write_all(body.as_bytes())
}

/// 会合文件：第一行端点，第二行口令。
fn write_endpoint(path: &Path, addr: &str, token: &str) -> std::io::Result<()> {
    write_private_file(path, &format!("{addr}\n{token}\n"), 0o600)
}

fn read_endpoint(path: &Path) -> Option<(String, String)> {
    let raw = std::fs::read_to_string(path).ok()?;
    let mut lines = raw.lines();
    let addr = lines.next()?.trim().to_owned();
    let token = lines.next()?.trim().to_owned();
    (!addr.is_empty() && !token.is_empty()).then_some((addr, token))
}

/// 这次运行的一次性口令。
///
/// 不引 rand / uuid：两者都不是宿主的直接依赖，为一行随机数加一个 crate
/// 不划算。`RandomState` 的键由 OS 熵源播种（std 的 HashMap 抗碰撞压的就是
/// 它），当 keyed hash 用，两轮拼出 128 位。
///
/// `[约束]` 不能换成时间戳 / pid 这类可推算的东西 —— 口令的全部意义就是
/// 连上来的进程猜不中它。
fn new_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher as _, Hasher as _};

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut out = String::with_capacity(32);
    for round in 0..2u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u128(nanos);
        h.write_u64(u64::from(std::process::id()));
        h.write_u64(round);
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

/// 请求的第一行是口令，其余是提示词。
fn format_request(token: &str, prompt: &str) -> String {
    format!("{token}\n{prompt}")
}

/// 没有第一行 = 不合协议（老客户端、端口扫描器），一律拒。
fn parse_request(raw: &str) -> Option<(&str, &str)> {
    let (token, prompt) = raw.split_once('\n')?;
    Some((token.trim_end_matches('\r'), prompt))
}

/// 常量时间比较。`==` 在第一个不同的字节上短路，连着试几万次能把口令一位
/// 一位量出来 —— 而这个监听对本机的其它进程是可连的。
fn token_ok(expected: &str, got: &str) -> bool {
    let (a, b) = (expected.as_bytes(), got.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 处理一次连接的内容，返回要写回去的应答；空串 = 什么都不回，直接断。
///
/// `[约束]` 口令没对上之前不碰提示词，更不弹窗。少了这道闸，本机任意进程
/// 连上来发一句 `Password for 'https://github.com':`，用户看到的就是一个
/// 标题为 Riot 的正常密码框，输进去的东西原样回给对方。
fn handle(raw: &str, token: &str, ask: impl FnOnce(&str) -> Option<String>) -> String {
    let Some((got, prompt)) = parse_request(raw) else {
        tracing::warn!("askpass 收到不合协议的连接，已丢弃");
        return String::new();
    };
    if !token_ok(token, got) {
        tracing::warn!("askpass 口令不匹配，已拒绝（有别的进程在冒充 git 要凭据？）");
        return String::new();
    }
    match ask(prompt.trim_end()) {
        Some(secret) => format!("ok\n{secret}"),
        None => "cancel\n".to_owned(),
    }
}

/// 一个连接读多久算够。合法客户端写完提示词就 shutdown(Write)，读到 EOF
/// 是立刻的事；不设上限的话，一个连上来不说话也不走的进程就能把这条单线程
/// 的服务占死，之后真正的凭据询问全部石沉大海。
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

#[cfg(unix)]
fn bind_and_serve(rv: &Rendezvous, token: String) -> bool {
    let listener = match std::os::unix::net::UnixListener::bind(&rv.sock) {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(error = %e, "askpass socket 绑不上");
            return false;
        }
    };
    // 不对 socket 自己 chmod:bind 之后再改有一扇窗,而且有的系统压根不看
    // socket 自身的权限位。普遍有效的是父目录 —— 它从创建那一刻就是 0700。
    if let Err(e) = write_endpoint(&rv.endpoint, &rv.sock.to_string_lossy(), &token) {
        tracing::warn!(error = %e, "askpass 会合文件写不出来");
        return false;
    }
    std::thread::Builder::new()
        .name("riot-askpass".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(s) = stream else { continue };
                let _ = s.set_read_timeout(Some(READ_TIMEOUT));
                serve_one(s, &token);
            }
        })
        .is_ok()
}

#[cfg(windows)]
fn bind_and_serve(rv: &Rendezvous, token: String) -> bool {
    // Windows 没有能靠权限位保护的 socket，退回 loopback TCP：端口谁都连得上，
    // 门槛完全落在会合文件里的那行口令上。
    let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(error = %e, "askpass 端口绑不上");
            return false;
        }
    };
    let Ok(addr) = listener.local_addr() else {
        return false;
    };
    if let Err(e) = write_endpoint(&rv.endpoint, &addr.port().to_string(), &token) {
        tracing::warn!(error = %e, "askpass 会合文件写不出来");
        return false;
    }
    std::thread::Builder::new()
        .name("riot-askpass".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(s) = stream else { continue };
                let _ = s.set_read_timeout(Some(READ_TIMEOUT));
                serve_one(s, &token);
            }
        })
        .is_ok()
}

/// 读一个请求、校验口令、（通过了才）弹窗、写回应答。
fn serve_one(mut s: impl Read + Write, token: &str) {
    let mut raw = String::new();
    if s.read_to_string(&mut raw).is_err() {
        return;
    }
    let reply = handle(&raw, token, native_prompt);
    if reply.is_empty() {
        return;
    }
    let _ = s.write_all(reply.as_bytes());
}

fn ask_host(prompt: &str) -> Option<String> {
    let path = env::var_os("RIOT_ASKPASS_ENDPOINT")?;
    let (addr, token) = read_endpoint(Path::new(&path))?;

    #[cfg(unix)]
    let mut stream = std::os::unix::net::UnixStream::connect(&addr).ok()?;
    #[cfg(windows)]
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", addr.parse::<u16>().ok()?)).ok()?;

    stream
        .write_all(format_request(&token, prompt).as_bytes())
        .ok()?;
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
    p.contains("password") || p.contains("passphrase") || p.contains("密码") || p.contains("口令")
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
                vec![
                    "--password".into(),
                    "--title=Riot".into(),
                    format!("--text={prompt}"),
                ]
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
                vec![
                    "--title".into(),
                    "Riot".into(),
                    "--password".into(),
                    prompt.into(),
                ]
            } else {
                vec![
                    "--title".into(),
                    "Riot".into(),
                    "--inputbox".into(),
                    prompt.into(),
                ]
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
        assert!(looks_secret(
            "Enter passphrase for key '/Users/me/.ssh/id_ed25519':"
        ));
        assert!(looks_secret("请输入密码"));
        assert!(!looks_secret("Username for 'https://github.com':"));
        assert!(!looks_secret(
            "Are you sure you want to continue connecting (yes/no)?"
        ));
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

    /// 端口扫描器连上来直接发提示词（老协议的形状），必须在读它之前就断掉。
    #[test]
    fn 不带口令的连接不弹窗() {
        let 弹过 = std::cell::Cell::new(false);
        let reply = handle("Password for 'https://github.com':", "真口令", |_| {
            弹过.set(true);
            Some("用户输的密码".to_owned())
        });
        assert!(reply.is_empty(), "不该给对方任何字节，实际回了 {reply:?}");
        assert!(!弹过.get(), "口令都没有就弹了窗");
    }

    /// 猜口令的那条路：猜错不能有任何可观测的副作用，更不能是一个弹窗 ——
    /// 用户没有任何办法看出那个标题为 Riot 的密码框是谁要的。
    #[test]
    fn 口令不对不弹窗() {
        let 弹过 = std::cell::Cell::new(false);
        let raw = format_request("猜的", "Password for 'https://github.com':");
        let reply = handle(&raw, "真口令", |_| {
            弹过.set(true);
            Some("用户输的密码".to_owned())
        });
        assert!(reply.is_empty(), "不该给对方任何字节，实际回了 {reply:?}");
        assert!(!弹过.get(), "口令不对却弹了窗");
    }

    /// 口令对上之后，真实客户端那条路要照常走通。
    #[test]
    fn 口令对上才把提示词交给弹窗() {
        let seen = std::cell::RefCell::new(String::new());
        let raw = format_request("真口令", "Password for 'https://github.com':\n");
        let reply = handle(&raw, "真口令", |p| {
            seen.borrow_mut().push_str(p);
            Some("hunter2".to_owned())
        });
        assert_eq!(seen.borrow().as_str(), "Password for 'https://github.com':");
        assert_eq!(parse_reply(&reply).as_deref(), Some("hunter2"));
    }

    #[test]
    fn 口令每次都不一样() {
        let a = new_token();
        assert_eq!(a.len(), 32, "128 位，写成十六进制");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, new_token(), "口令可预测的话这道闸等于没有");
    }

    /// 客户端要从会合文件里同时拿到端点和口令；缺一行就当没装上，
    /// 而不是拿着半份信息去连。
    #[test]
    fn 会合文件带端点和口令() {
        let dir = tempfile::tempdir().expect("建临时目录");
        let f = dir.path().join("endpoint");
        write_endpoint(&f, "/tmp/x.sock", "abc123").expect("写会合文件");
        assert_eq!(
            read_endpoint(&f),
            Some(("/tmp/x.sock".to_owned(), "abc123".to_owned()))
        );

        let half = dir.path().join("half");
        write_private_file(&half, "只有端点\n", 0o600).expect("写半份");
        assert_eq!(read_endpoint(&half), None);
    }

    /// 把这条闸装到真的监听上，而不只是验一个纯函数：连上来、发错口令、
    /// 对面必须一个字节都不回（也就没有弹窗）。Windows 那半边是同一段
    /// 逻辑，只是换了 listener 类型。
    #[cfg(unix)]
    #[test]
    fn 错口令的连接拿不到任何字节() {
        use std::os::unix::net::UnixStream;

        let tmp = tempfile::tempdir().expect("建临时目录");
        let rv = Rendezvous {
            script: tmp.path().join("askpass.sh"),
            endpoint: tmp.path().join("endpoint"),
            sock: tmp.path().join("sock"),
        };
        assert!(bind_and_serve(&rv, "真口令".to_owned()), "监听要起得来");

        let (addr, token) = read_endpoint(&rv.endpoint).expect("会合文件要写出来");
        assert_eq!(token, "真口令", "客户端得能从会合文件里拿到口令");

        let mut c = UnixStream::connect(&addr).expect("连得上");
        c.write_all(format_request("猜的", "Password for 'https://github.com':").as_bytes())
            .expect("写请求");
        c.shutdown(std::net::Shutdown::Write).expect("收尾");
        let mut reply = String::new();
        c.read_to_string(&mut reply).expect("读应答");
        assert!(reply.is_empty(), "口令不对却回了 {reply:?}");
    }

    /// 共享 `/tmp` 上的 TOCTOU：别人先占住这个名字时必须放弃，绝不能
    /// 往一个已经存在（可能是别人开着写句柄的）文件里写包装脚本。
    #[test]
    fn 已存在的路径不会被覆盖() {
        let dir = tempfile::tempdir().expect("建临时目录");
        let f = dir.path().join("askpass.sh");
        write_private_file(&f, "第一次", 0o700).expect("第一次该成功");
        assert!(
            write_private_file(&f, "第二次", 0o700).is_err(),
            "第二次必须失败"
        );
    }

    /// 目录和脚本在**创建那一刻**就得是私有的。先 0644 再 chmod 0700 的
    /// 写法留了一扇窗，`/tmp` 上另一个本地用户能在窗里拿到写句柄，之后随时
    /// 改写这个会被 git / ssh 以当前用户身份执行的脚本。
    #[cfg(unix)]
    #[test]
    fn 目录和脚本创建即私有() {
        use std::os::unix::fs::PermissionsExt as _;

        let tmp = tempfile::tempdir().expect("建临时目录");
        let dir = tmp.path().join("priv");
        create_private_dir(&dir).expect("建私有目录");
        assert_eq!(
            std::fs::metadata(&dir)
                .expect("读目录元信息")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert!(create_private_dir(&dir).is_err(), "不能复用已存在的目录");

        let script = dir.join("askpass.sh");
        write_private_file(&script, "#!/bin/sh\n", 0o700).expect("写脚本");
        assert_eq!(
            std::fs::metadata(&script)
                .expect("读脚本元信息")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}
