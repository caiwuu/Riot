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

/// 输出缓冲上限。够回放一屏历史、也够模型读到失败原因；再多是 dev server
/// 刷了几万行进度条，那部分没人会看。
const BUFFER_BYTES: usize = 256 * 1024;

/// 一个终端的概况。列表和前端重挂时用。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TermSummary {
    pub id: u32,
    pub title: String,
    /// 起它的命令。模型起的服务才有；用户自己开的 shell 是 None。
    pub command: Option<String>,
    pub running: bool,
    /// 用户把这个终端交给模型看了。见 [`Terminals::set_shared`]。
    pub shared: bool,
}

/// 一个终端。
struct Term {
    /// 键盘输入往这里写。
    writer: Mutex<Box<dyn Write + Send>>,
    /// 只为 resize 留着。
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    /// 前端的事件出口。
    ///
    /// `None` 是常态而不是异常：模型起的服务不该等着用户打开面板才开始
    /// 跑。那段时间输出只进缓冲，面板打开时用 [`Terminals::attach`] 补上。
    sink: Mutex<Option<Channel<TermEvent>>>,
    /// 输出缓冲。模型靠它读，前端重新挂上来时靠它回放。
    buf: Mutex<Vec<u8>>,
    title: String,
    command: Option<String>,
    /// 进程还活着。退出后条目仍然留着（模型要读最后那几行报错），
    /// 真正的移除只发生在 [`Terminals::close`]。
    running: std::sync::atomic::AtomicBool,
    /// 用户显式把这个终端交给模型看了。
    ///
    /// `[约束]` 默认 false，且只能由用户在面板上点开 —— 模型没有任何接口
    /// 能把它置真。见 [`Terminals::set_shared`]。
    shared: std::sync::atomic::AtomicBool,
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
        self.start(root, None, "终端".to_owned(), Some(sink), cols, rows)
    }

    /// 起一条长期命令，跑在用户看得见的终端里。立刻返回 id，不等它结束。
    ///
    /// 这是模型开 dev server 的唯一正路：走 Bash 那条子进程的话，收尾时
    /// 整个进程组会被清掉，服务活不过一次调用；而用 `setsid` 逃出来的
    /// 进程谁也管不了。放在这里，用户能看见能 Ctrl-C，模型能读能停。
    pub fn spawn(
        &self,
        root: Option<String>,
        command: &str,
        title: &str,
    ) -> Result<u32, String> {
        // 尺寸随便给一个像样的：面板挂上来时会按真实宽度 resize。
        // 太窄的话服务启动那几行 banner 会折得没法看。
        self.start(
            root,
            Some(command.to_owned()),
            title.to_owned(),
            None,
            120,
            30,
        )
    }

    fn start(
        &self,
        root: Option<String>,
        command: Option<String>,
        title: String,
        sink: Option<Channel<TermEvent>>,
        cols: u16,
        rows: u16,
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
        // 带命令的走 `-c`：跑完就退，退出码留在标签上。仍然是登录 shell
        // （见 default_shell），否则 brew 装的东西一律 command not found。
        if let Some(c) = &command {
            cmd.arg("-c");
            cmd.arg(c);
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

        // 进程退出后是否留着条目。模型起的要留 —— 服务挂了它得能读到
        // 最后那几行报错，而那正是它最需要看的时候。
        let keep = command.is_some();

        let id = self.0.next.fetch_add(1, Ordering::Relaxed);
        let term = Arc::new(Term {
            writer: Mutex::new(writer),
            master: Mutex::new(pty.master),
            child: Mutex::new(child),
            sink: Mutex::new(sink),
            buf: Mutex::new(Vec::new()),
            title,
            command,
            running: std::sync::atomic::AtomicBool::new(true),
            // 默认不共享。模型起的服务不需要这个标记（它按"自己起的"放行），
            // 用户自己开的 shell 要他显式点开才给看。
            shared: std::sync::atomic::AtomicBool::new(false),
        });
        self.0
            .map
            .lock()
            .expect("终端表锁")
            .insert(id, Arc::clone(&term));

        // 读线程。PTY 读端是阻塞 I/O，不能放进 tokio —— 一个终端占一个
        // 线程，面板不会同时开几十个，这个代价可以接受。
        let inner = Arc::clone(&self.0);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            loop {
                match reader.read(&mut chunk) {
                    // EOF / 读错都表示这个 PTY 完了，没有恢复路径
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        push_capped(&mut term.buf.lock().expect("缓冲锁"), &chunk[..n]);
                        // 前端不在（面板没开）就只进缓冲。这不是错误 ——
                        // 服务照跑，等面板挂上来再回放。
                        let data = base64::engine::general_purpose::STANDARD.encode(&chunk[..n]);
                        let sink = term.sink.lock().expect("出口锁").clone();
                        if let Some(s) = sink
                            && s.send(TermEvent::Data { data }).is_err()
                        {
                            // 出口废了（窗口关了）。摘掉它，进程继续跑 ——
                            // 下次挂上来还能接着看。
                            *term.sink.lock().expect("出口锁") = None;
                        }
                    }
                }
            }
            term.running
                .store(false, std::sync::atomic::Ordering::Relaxed);
            // 收尸。close() 先到的话表里已经没有了，那边做过收尾。
            if keep {
                let _ = term.child.lock().expect("child 锁").wait();
            } else if let Some(t) = inner.map.lock().expect("终端表锁").remove(&id) {
                let _ = t.child.lock().expect("child 锁").wait();
            }
            // 前端收到后关掉对应标签。它不听了也无所谓 —— send 失败没有下文。
            if let Some(s) = term.sink.lock().expect("出口锁").as_ref() {
                let _ = s.send(TermEvent::Exit);
            }
        });

        Ok(id)
    }

    /// 把前端的出口挂到一个已经在跑的终端上，并回放已有输出。
    ///
    /// 面板重新打开、或者模型在面板没开时起了服务，都走这里。回放是
    /// 一次性的一大块 —— xterm 自己会把它渲染成正确的屏幕。
    pub fn attach(&self, id: u32, sink: Channel<TermEvent>) -> Result<(), String> {
        let t = self.get(id)?;
        let backlog = t.buf.lock().expect("缓冲锁").clone();
        if !backlog.is_empty() {
            let data = base64::engine::general_purpose::STANDARD.encode(&backlog);
            let _ = sink.send(TermEvent::Data { data });
        }
        // 进程已经退了：补一条 Exit，否则标签会一直显示"在跑"。
        if !t.running.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = sink.send(TermEvent::Exit);
        }
        *t.sink.lock().expect("出口锁") = Some(sink);
        Ok(())
    }

    /// 所有终端的概况，按 id 升序。前端重建标签栏、模型找自己的服务都用它。
    pub fn list(&self) -> Vec<TermSummary> {
        let g = self.0.map.lock().expect("终端表锁");
        let mut out: Vec<TermSummary> = g
            .iter()
            .map(|(id, t)| TermSummary {
                id: *id,
                title: t.title.clone(),
                command: t.command.clone(),
                running: t.running.load(std::sync::atomic::Ordering::Relaxed),
                shared: t.shared.load(std::sync::atomic::Ordering::Relaxed),
            })
            .collect();
        out.sort_by_key(|s| s.id);
        out
    }

    /// 读最近 `lines` 行输出，已去掉 ANSI 转义和回车覆盖。
    ///
    /// `Err` = 这个终端不在了。模型只该读自己起的那些，那条边界由
    /// 调用方（[`crate::term_access`]）把关。
    pub fn read(&self, id: u32, lines: usize) -> Result<String, String> {
        let t = self.get(id)?;
        let raw = t.buf.lock().expect("缓冲锁").clone();
        Ok(tail_lines(&plain_text(&raw), lines))
    }

    /// 这个终端是模型起的吗（以及它还在不在）。
    pub fn info(&self, id: u32) -> Option<TermSummary> {
        let g = self.0.map.lock().expect("终端表锁");
        g.get(&id).map(|t| TermSummary {
            id,
            title: t.title.clone(),
            command: t.command.clone(),
            running: t.running.load(std::sync::atomic::Ordering::Relaxed),
            shared: t.shared.load(std::sync::atomic::Ordering::Relaxed),
        })
    }

    /// 把键盘输入写进 shell。`data` 是 xterm 给的原始串（含控制序列）。
    pub fn write(&self, id: u32, data: &str) -> Result<(), String> {
        let t = self.get(id)?;
        if !t.running.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("这个终端已经结束了".to_owned());
        }
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

    /// 把一个终端交给模型看 / 收回来。
    ///
    /// `[约束]` 只有用户能调这条路（面板上的按钮 → `term_share` 命令）。
    /// 模型侧的 [`riot_protocol::terminal::TerminalAccess`] 里**没有**对应
    /// 的方法 —— 它不能给自己开权限。
    ///
    /// 为什么需要这个开关：用户自己那个 shell 里有他敲过的密码、私有仓库
    /// 地址、和这次任务无关的一切，默认不该给模型读。但"我的 dev server
    /// 报错了"是最日常的场景，让他手动复制粘贴几十行日志是白费功夫。
    /// 折中就是把决定权交给他，一次一个终端。
    pub fn set_shared(&self, id: u32, shared: bool) {
        if let Some(t) = self.0.map.lock().expect("终端表锁").get(&id) {
            t.shared.store(shared, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// 这个终端有没有被交给模型。
    pub fn is_shared(&self, id: u32) -> bool {
        self.0
            .map
            .lock()
            .expect("终端表锁")
            .get(&id)
            .is_some_and(|t| t.shared.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// 前台有没有 shell 之外的进程在跑（dev server、vim……）。
    ///
    /// 判据是 POSIX 的前台进程组：空闲的交互 shell 里 `tcgetpgrp` 给回来的
    /// 就是 shell 自己，跑着东西时是那个进程的组 —— iTerm 的"关闭前确认"
    /// 用的同一招。拿不到就当不忙：这只是提示，不是权限。
    pub fn is_busy(&self, id: u32) -> bool {
        let Some(t) = self.0.map.lock().expect("终端表锁").get(&id).cloned() else {
            return false;
        };
        if !t.running.load(std::sync::atomic::Ordering::Relaxed) {
            return false;
        }
        // 模型起的服务：整个终端就是那条命令，还活着就是在跑。
        if t.command.is_some() {
            return true;
        }
        #[cfg(unix)]
        {
            let fg = t.master.lock().expect("master 锁").process_group_leader();
            let shell = t.child.lock().expect("child 锁").process_id();
            if let (Some(fg), Some(shell)) = (fg, shell) {
                return fg != shell as i32;
            }
        }
        false
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

/// 往缓冲里塞，超上限就从头砍掉一截。
///
/// 一次砍四分之一而不是刚好砍到线上：贴着上限的话每来一个 chunk 都要
/// 搬一次几百 KB 的内存。
fn push_capped(buf: &mut Vec<u8>, bytes: &[u8]) {
    buf.extend_from_slice(bytes);
    if buf.len() > BUFFER_BYTES {
        let cut = buf.len() - BUFFER_BYTES + BUFFER_BYTES / 4;
        buf.drain(..cut.min(buf.len()));
    }
}

/// PTY 字节流 → 人和模型读得懂的纯文本。
///
/// 两件事：去掉 ANSI 转义（不去的话模型看到的是满屏 `\x1b[32m`），
/// 以及处理裸回车 —— 进度条靠 `\r` 反复重画同一行，直接留着的话
/// 一行里会堆着几十份"下载中"。按终端的实际行为，只保留最后一次。
fn plain_text(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(s.len());
    let mut line = String::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                // CSI：一直吃到 @~ 区间的终止字符。
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC：吃到 BEL 或 ESC\ 为止（设置窗口标题之类）。
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' {
                            chars.next();
                            break;
                        }
                    }
                }
                // 其余两字符序列，吃掉就好。
                _ => {}
            },
            // `\r\n` 是 PTY 的正常行尾，不是回到行首重画 —— 一律 clear
            // 的话，每一行都会在换行前被抹掉，读出来是一片空白。
            '\r' if chars.peek() == Some(&'\n') => {}
            '\r' => line.clear(),
            '\n' => {
                out.push_str(&line);
                out.push('\n');
                line.clear();
            }
            // 退格：命令行编辑会用到。
            '\u{8}' => {
                line.pop();
            }
            c if c.is_control() && c != '\t' => {}
            c => line.push(c),
        }
    }
    out.push_str(&line);
    out
}

/// 取末尾 `lines` 行。模型要的是"刚才发生了什么"，开头那些启动 banner
/// 早就没用了。
fn tail_lines(text: &str, lines: usize) -> String {
    if lines == 0 {
        return String::new();
    }
    let all: Vec<&str> = text.lines().collect();
    let from = all.len().saturating_sub(lines);
    all[from..].join("\n")
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

/// 终端相关测试共用的小工具。`term_access` 那边也要用。
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// 造一个把收到的字节攒起来的 channel。
    pub(crate) fn probe() -> (Channel<TermEvent>, Arc<Mutex<Vec<TermEvent>>>) {
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

    /// 轮询等一个条件成立。PTY 是真进程真 I/O，只能等。
    pub(crate) fn wait_until(mut done: impl FnMut() -> bool) -> bool {
        for _ in 0..200 {
            if done() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{probe, wait_until};
    use super::*;

    /// 模型起的服务：跑得起来、输出留得下、进程退了条目还在。
    ///
    /// 最后一条不是细节 —— 服务挂掉那一刻正是模型最需要读日志的时候。
    #[test]
    fn 带命令的终端留下输出且退出后仍可读() {
        let terms = Terminals::default();
        let id = terms
            .spawn(
                Some(std::env::temp_dir().display().to_string()),
                "printf 'riot-spawn-ok\\n'",
                "测试服务",
            )
            .expect("起服务");

        let seen = wait_until(|| {
            terms
                .read(id, 50)
                .map(|t| t.contains("riot-spawn-ok"))
                .unwrap_or(false)
        });
        assert!(seen, "实际读到：{:?}", terms.read(id, 50));

        let done = wait_until(|| terms.info(id).is_some_and(|i| !i.running));
        assert!(done, "printf 跑完就该退");
        assert!(
            terms.read(id, 50).expect("退了也能读").contains("riot-spawn-ok"),
            "进程退出不该带走它的日志"
        );

        terms.close(id);
        assert!(terms.read(id, 10).is_err(), "关掉之后才真正消失");
    }

    /// `\r\n` 是 PTY 的正常行尾。当成进度条的"回到行首"处理的话，
    /// 每一行都会在换行前被抹掉，模型读到的是一片空白 —— 真踩过。
    #[test]
    fn 纯文本化保留正常行尾且吃掉转义与重画() {
        let out = plain_text(b"\x1b[32mhello\x1b[0m\r\nworld\r\n");
        assert_eq!(out, "hello\nworld\n");

        // 进度条：同一行反复重画，只留最后一次。
        let bar = plain_text(b"10%\r50%\r100%\r\n");
        assert_eq!(bar, "100%\n");

        // OSC（设置窗口标题）整段吃掉，不能漏进正文。
        let osc = plain_text(b"\x1b]0;title\x07done\r\n");
        assert_eq!(osc, "done\n");
    }

    #[test]
    fn 只取末尾若干行() {
        let text = "a\nb\nc\nd\n";
        assert_eq!(tail_lines(text, 2), "c\nd");
        assert_eq!(tail_lines(text, 99), "a\nb\nc\nd");
        assert_eq!(tail_lines(text, 0), "");
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
