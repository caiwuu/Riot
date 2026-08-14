//! 底部终端面板的 PTY 管理。
//!
//! 面板里的 shell 是**用户自己**的终端 —— 和浏览器面板同理，不过权限链。
//! 模型跑命令走 Bash 工具那条路，照常受管；这里的进程和会话、围栏都没有
//! 关系，只是"在项目目录里给用户开一个真终端"。
//!
//! # 为什么在宿主层
//!
//! PTY 是纯粹的操作系统能力（见 lib.rs 顶部的职责边界）。内核不知道
//! 终端的存在 —— 它既不该知道，也用不上。
//!
//! # 生命周期
//!
//! 终端跟着应用走，不跟着会话走：切会话、关面板都不杀 shell —— 用户在
//! 里面可能挂着 dev server。真正的退出只有三条路：用户关标签（`close`）、
//! shell 自己退出（`exit` / 崩溃）、应用退出（master fd 关闭，子进程收到
//! SIGHUP，操作系统收尾）。

// 宿主层职责就是操作真实 OS：PTY 的读写是阻塞 I/O，线程和系统时钟都是真的。
#![allow(clippy::disallowed_methods)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;
use tauri::ipc::Channel;

/// 推给前端的终端事件。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TermEvent {
    /// 一段输出。base64 编码 —— 输出是字节流，chunk 的边界随时可能切在
    /// 一个 UTF-8 序列中间，按字符串传会把半个字变成替换符。xterm 自带
    /// 跨 chunk 的解码器，给它字节就好。
    Data { data: String },
    /// shell 退出了（`exit`、崩溃、或被 close 杀掉）。前端收到后关标签。
    Exit,
}

/// 一个活着的终端。
struct Term {
    /// 键盘输入往这里写。
    writer: Mutex<Box<dyn Write + Send>>,
    /// 只为 resize 留着。
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
}

/// 所有终端的注册表。`Clone` 是浅拷贝（内部是 `Arc`），和 `AppState` 同款。
#[derive(Default, Clone)]
pub struct Terminals(Arc<Inner>);

#[derive(Default)]
struct Inner {
    map: Mutex<HashMap<u32, Arc<Term>>>,
    next: AtomicU32,
}

impl Terminals {
    /// 开一个新终端：起 shell、开始把输出推给 `sink`，返回终端 id。
    ///
    /// `root` 是工作目录；不存在（或没传）就退回家目录 —— 终端还是要开，
    /// 只是位置不理想。开不出来才是错误。
    pub fn open(
        &self,
        root: Option<String>,
        cols: u16,
        rows: u16,
        sink: Channel<TermEvent>,
    ) -> Result<u32, String> {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("开不了 PTY：{e}"))?;

        let mut cmd = default_shell();
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        if let Some(dir) = root
            .filter(|r| std::path::Path::new(r).is_dir())
            .or_else(home)
        {
            cmd.cwd(dir);
        }

        let child = pty
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("shell 起不来：{e}"))?;
        // slave 端必须及时放掉。留着的话我们自己也持有从端 fd，子进程退出后
        // 读端永远等不到 EOF —— 表现是关了 shell 标签还亮着。
        drop(pty.slave);

        let mut reader = pty
            .master
            .try_clone_reader()
            .map_err(|e| format!("拿不到 PTY 读端：{e}"))?;
        let writer = pty
            .master
            .take_writer()
            .map_err(|e| format!("拿不到 PTY 写端：{e}"))?;

        let id = self.0.next.fetch_add(1, Ordering::Relaxed);
        self.0.map.lock().expect("终端表锁").insert(
            id,
            Arc::new(Term {
                writer: Mutex::new(writer),
                master: Mutex::new(pty.master),
                child: Mutex::new(child),
            }),
        );

        // 读线程。PTY 读端是阻塞 I/O，不能放进 tokio —— 一个终端占一个
        // 线程，面板不会同时开几十个，这个代价可以接受。
        let inner = Arc::clone(&self.0);
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    // EOF / 读错都表示这个 PTY 完了，没有恢复路径
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let data = base64::engine::general_purpose::STANDARD.encode(&buf[..n]);
                        if sink.send(TermEvent::Data { data }).is_err() {
                            break;
                        }
                    }
                }
            }
            // shell 退出：收尸 + 摘表。close() 先到的话这里拿到 None，
            // 收尾工作已经在那边做完了。
            if let Some(t) = inner.map.lock().expect("终端表锁").remove(&id) {
                let _ = t.child.lock().expect("child 锁").wait();
            }
            // 前端收到后关掉对应标签。它不听了也无所谓 —— send 失败没有下文。
            let _ = sink.send(TermEvent::Exit);
        });

        Ok(id)
    }

    /// 把键盘输入写进 shell。`data` 是 xterm 给的原始串（含控制序列）。
    pub fn write(&self, id: u32, data: &str) -> Result<(), String> {
        let t = self.get(id)?;
        let mut w = t.writer.lock().expect("writer 锁");
        w.write_all(data.as_bytes())
            .and_then(|()| w.flush())
            .map_err(|e| format!("写入终端失败：{e}"))
    }

    /// 面板尺寸变了。不同步的话 shell 还按旧宽度折行，vim 之类的全屏
    /// 程序会画花。
    pub fn resize(&self, id: u32, cols: u16, rows: u16) -> Result<(), String> {
        self.get(id)?
            .master
            .lock()
            .expect("master 锁")
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("调整终端尺寸失败：{e}"))
    }

    /// 关一个终端。幂等 —— shell 自己先退了、用户又点关闭，第二下不该报错。
    pub fn close(&self, id: u32) {
        if let Some(t) = self.0.map.lock().expect("终端表锁").remove(&id) {
            let mut child = t.child.lock().expect("child 锁");
            let _ = child.kill();
            // kill 之后立刻 wait，不留僵尸。读线程那边拿不到表项，不会重复收尸。
            let _ = child.wait();
        }
    }

    fn get(&self, id: u32) -> Result<Arc<Term>, String> {
        self.0
            .map
            .lock()
            .expect("终端表锁")
            .get(&id)
            .cloned()
            .ok_or_else(|| "这个终端已经关了".to_owned())
    }
}

/// 用户的默认 shell。
#[cfg(unix)]
fn default_shell() -> CommandBuilder {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_owned());
    let mut cmd = CommandBuilder::new(shell);
    // 登录 shell。GUI 进程不继承终端的 PATH，而用户的 PATH 通常写在
    // 登录期加载的 rc 里 —— macOS 的 Terminal.app 也是这么起 shell 的。
    // 不带 -l 的话，brew 装的一切命令都会"command not found"。
    cmd.arg("-l");
    cmd
}

#[cfg(windows)]
fn default_shell() -> CommandBuilder {
    CommandBuilder::new("powershell.exe")
}

fn home() -> Option<String> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个把收到的字节攒起来的 channel。
    fn probe() -> (Channel<TermEvent>, Arc<Mutex<Vec<TermEvent>>>) {
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = Arc::clone(&got);
        let ch = Channel::new(move |ev| {
            // InvokeResponseBody 只在真实 IPC 下是结构化的；测试里拿原始 JSON。
            if let tauri::ipc::InvokeResponseBody::Json(s) = ev {
                let parsed: serde_json::Value = serde_json::from_str(&s).expect("事件是 JSON");
                let ev = if parsed["kind"] == "exit" {
                    TermEvent::Exit
                } else {
                    TermEvent::Data {
                        data: parsed["data"].as_str().unwrap_or_default().to_owned(),
                    }
                };
                g.lock().expect("锁").push(ev);
            }
            Ok(())
        });
        (ch, got)
    }

    fn wait_until(mut done: impl FnMut() -> bool) -> bool {
        for _ in 0..200 {
            if done() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        false
    }

    #[test]
    fn 终端能跑命令并回显输出() {
        let terms = Terminals::default();
        let (ch, got) = probe();
        let id = terms
            .open(Some(std::env::temp_dir().display().to_string()), 80, 24, ch)
            .expect("开终端");

        terms.write(id, "printf 'riot-term-ok\\n'\r").expect("写命令");

        let seen = wait_until(|| {
            let all: Vec<u8> = got
                .lock()
                .expect("锁")
                .iter()
                .filter_map(|e| match e {
                    TermEvent::Data { data } => {
                        base64::engine::general_purpose::STANDARD.decode(data).ok()
                    }
                    TermEvent::Exit => None,
                })
                .flatten()
                .collect();
            String::from_utf8_lossy(&all).contains("riot-term-ok")
        });
        assert!(seen, "输出里应该能看到命令的回显");

        terms.close(id);
    }

    #[test]
    fn 关闭后再写会报错而不是恐慌() {
        let terms = Terminals::default();
        let (ch, _got) = probe();
        let id = terms.open(None, 80, 24, ch).expect("开终端");

        terms.close(id);
        assert!(terms.write(id, "echo hi\r").is_err(), "关掉的终端不该还能写");
        // 再关一次是无操作，不是错误
        terms.close(id);
    }

    #[test]
    fn shell_退出后自动摘表并广播_exit() {
        let terms = Terminals::default();
        let (ch, got) = probe();
        let id = terms.open(None, 80, 24, ch).expect("开终端");

        terms.write(id, "exit\r").expect("写 exit");

        let exited = wait_until(|| {
            got.lock()
                .expect("锁")
                .iter()
                .any(|e| matches!(e, TermEvent::Exit))
        });
        assert!(exited, "shell 退出后前端必须收到 Exit 事件");
        assert!(terms.write(id, "x").is_err(), "退出的终端应该已经不在表里");
    }
}
