//! 一条 Bash 命令按**字面**能看出来会碰哪些文件。
//!
//! 给检查点 / 改动栏用，不参与权限判定。Write / Edit / Delete 自己记基线，
//! Bash 不记 —— 而模型复制一个文件天然会用 `cp`，于是改动栏看不见新文件、
//! 回退也不删它。这里补的就是这一种缺口：不监听磁盘、不扫目录，只认几条
//! 最常见命令的**字面路径参数**和重定向目标。
//!
//! 放在权限 crate 是因为语法树在这里：白名单扫描、子命令提取、引号还原
//! 都是 [`super::ast`] 已经做过的活，另起一份迟早漂移。
//!
//! # 认什么、不认什么
//!
//! | 认 | 不认（整条放弃）|
//! |---|---|
//! | `cp` / `mv` 的源和目标 | 变量、命令替换、进程替换 |
//! | `touch` / `tee` / `sed -i` 的文件参数 | 未加引号的 glob / `~`（只放弃那一条子命令）|
//! | `rm` 的参数 | 控制流、函数、后台 `&` |
//! | `>` / `>>` / `&>` / `2>` 的目标 | `pushd` / `popd`、带选项或无参数的 `cd` |
//! | 字面路径的 `cd`：之后的相对路径都拼上它 | |
//!
//! `cd <目录> && cmd` 是模型最常用的写法之一（它不信任工作目录），整条放弃
//! 的话这一大类命令全漏。所以按出现顺序维护一个"当前目录"：`cd` 的参数
//! 是字面量就接上去，后面命令和重定向里的相对路径都以它为基准。`cd` 带
//! 选项、没参数（回家）、`cd -`、`pushd` / `popd` 仍然整条放弃。
//!
//! `[约束]` 宁可漏记，不能记错。记错一条基线，回退会按它去改一个不该动的
//! 文件；漏记只是回到"终端改动看不见"的老样子。所以任何一个看不懂的结构
//! 出现，整条命令返回空。
//!
//! 结果只是**候选**：`cp a dir/` 真正写的是 `dir/a`，`rm x` 里 x 可能是目录，
//! `sed` 的脚本会被当成一个文件名。这些要看磁盘，由工具层在执行前后比对
//! 决定（`riot_tools::tools::bash_effects`）—— 前后没变的候选自然落空。
//! 这里不碰文件系统。

use super::ast::{self, SubCommand};

/// 命令文本上限，和 [`ast::analyze`] 同一个量级。超过的命令多半是脚本，
/// 本来就不该按字面认。
const MAX_COMMAND_LEN: usize = 16 * 1024;

/// 一次候选的文件效应。路径是命令里的原文（已还原引号），相对路径以工作
/// 目录为基准，由调用方解析。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOp {
    /// 会写或新建这个文件：重定向目标、`touch`、`tee`、`sed -i`。
    Write(String),
    /// `cp` / `mv`。目标是已存在的目录（或以 `/` 结尾）时，真正写的是
    /// `目标/源文件名`；`moves` 为 true 时源文件随之消失。
    Transfer {
        sources: Vec<String>,
        dest: String,
        moves: bool,
    },
    /// `rm`。
    Remove(String),
}

/// 重定向相关的节点：[`ast::analyze`] 把它们当成"看不懂"，这里要认。
const REDIRECT_NODES: &[&str] = &[
    "file_redirect",
    "file_descriptor",
    ">",
    ">>",
    "&>",
    "&>>",
    ">|",
    ">&",
    "<",
    "<&",
    "<>",
    // heredoc / herestring 是读侧，正文若含展开会以 expansion 节点出现，
    // 照样被下面的白名单拦下。
    "heredoc_redirect",
    "heredoc_start",
    "heredoc_body",
    "heredoc_end",
    "heredoc_content",
    "herestring_redirect",
    "<<",
    "<<-",
    "<<<",
];

/// 一条命令最多拆多少个子命令。和 [`ast::analyze`] 同一个上限。
const MAX_SUB_COMMANDS: usize = 50;

/// 一条命令的候选文件效应。看不懂就是空。
pub fn file_effects(command: &str) -> Vec<FileOp> {
    if command.len() > MAX_COMMAND_LEN {
        return Vec::new();
    }
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .is_err()
    {
        return Vec::new();
    }
    let Some(tree) = parser.parse(command, None) else {
        return Vec::new();
    };
    let root = tree.root_node();
    if root.has_error() || root.is_missing() {
        return Vec::new();
    }
    let src = command.as_bytes();

    // 子命令和重定向按在命令文本里的位置排成一列：`cd` 只影响它后面的。
    let mut events = Vec::new();
    if !scan(root, src, &mut events) {
        return Vec::new();
    }
    if events
        .iter()
        .filter(|(_, e)| matches!(e, Event::Cmd(_)))
        .count()
        > MAX_SUB_COMMANDS
    {
        return Vec::new();
    }
    events.sort_by_key(|(pos, _)| *pos);

    let mut base: Option<String> = None;
    let mut ops = Vec::new();
    for (_, ev) in events {
        match ev {
            Event::Redirect(target) => ops.push(FileOp::Write(rebase(base.as_deref(), &target))),
            Event::Cmd(sub) => {
                let name = ast::unquote(&sub.name);
                match basename(&name) {
                    "cd" => match cd_target(&sub) {
                        Some(dir) => base = Some(rebase(base.as_deref(), &dir)),
                        None => return Vec::new(),
                    },
                    "pushd" | "popd" => return Vec::new(),
                    _ => ops.extend(
                        ops_of(&sub)
                            .into_iter()
                            .map(|op| op.rebased(base.as_deref())),
                    ),
                }
            }
        }
    }
    ops
}

enum Event {
    Cmd(SubCommand),
    Redirect(String),
}

impl FileOp {
    fn rebased(self, base: Option<&str>) -> FileOp {
        match self {
            FileOp::Write(p) => FileOp::Write(rebase(base, &p)),
            FileOp::Remove(p) => FileOp::Remove(rebase(base, &p)),
            FileOp::Transfer {
                sources,
                dest,
                moves,
            } => FileOp::Transfer {
                sources: sources.iter().map(|s| rebase(base, s)).collect(),
                dest: rebase(base, &dest),
                moves,
            },
        }
    }
}

/// 相对路径拼上当前 `cd` 到的目录；绝对路径和没 `cd` 过的原样。
fn rebase(base: Option<&str>, path: &str) -> String {
    match base {
        Some(b) if !is_absolute(path) => {
            let sep = if b.ends_with('/') || b.ends_with('\\') {
                ""
            } else {
                "/"
            };
            format!("{b}{sep}{path}")
        }
        _ => path.to_owned(),
    }
}

fn is_absolute(p: &str) -> bool {
    p.starts_with('/')
        || p.starts_with('\\')
        // `D:/proj`、`D:\proj`（Git Bash 两种都收）
        || (p.len() >= 2 && p.as_bytes()[1] == b':' && p.as_bytes()[0].is_ascii_alphabetic())
}

/// `cd` 的字面目标。带选项、没参数（回家）、`cd -`、多个参数都看不懂。
fn cd_target(sub: &SubCommand) -> Option<String> {
    if sub.has_unquoted_glob {
        return None;
    }
    let args: Vec<String> = sub.args.iter().map(|a| ast::unquote(a)).collect();
    match args.as_slice() {
        [dir] if !dir.is_empty() && !dir.starts_with('-') => Some(dir.clone()),
        _ => None,
    }
}

/// 全树白名单扫描（含匿名节点），顺手把子命令和写文件的重定向按位置
/// 收起来。返回 false = 有看不懂的结构，整条放弃。
fn scan(root: tree_sitter::Node, src: &[u8], events: &mut Vec<(usize, Event)>) -> bool {
    let mut stack = vec![root];
    let mut cur = root.walk();
    while let Some(node) = stack.pop() {
        let kind = node.kind();
        let allowed = if node.is_named() {
            ast::ALLOWED_NODES.contains(&kind) || REDIRECT_NODES.contains(&kind)
        } else {
            ast::ALLOWED_ANON.contains(&kind) || REDIRECT_NODES.contains(&kind)
        };
        if !allowed {
            return false;
        }
        match kind {
            "file_redirect" => {
                match redirect_target(node, src) {
                    Ok(Some(target)) => events.push((node.start_byte(), Event::Redirect(target))),
                    Ok(None) => {}
                    Err(()) => return false,
                }
                // 子节点已经在 redirect_target 里看过了
                continue;
            }
            "command" => {
                // `eval` / `source` 这类在这里被拒。子节点仍要压栈：参数里的
                // 展开、替换要靠白名单扫描拦下来。
                match ast::parse_command(node, src) {
                    Ok(sub) => events.push((node.start_byte(), Event::Cmd(sub))),
                    Err(_) => return false,
                }
            }
            _ => {}
        }
        for c in node.children(&mut cur) {
            stack.push(c);
        }
    }
    true
}

/// 一个重定向写向哪个文件。`Ok(None)` = 不写文件（读、fd 复制、`/dev/null`），
/// `Err` = 形状看不懂。
fn redirect_target(redirect: tree_sitter::Node, src: &[u8]) -> Result<Option<String>, ()> {
    let mut cur = redirect.walk();
    let mut op: Option<&str> = None;
    let mut target: Option<tree_sitter::Node> = None;
    for c in redirect.children(&mut cur) {
        match c.kind() {
            "file_descriptor" => {}
            k @ (">" | ">>" | "&>" | "&>>" | ">|" | ">&" | "<" | "<&" | "<>") => op = Some(k),
            "word" | "raw_string" | "string" | "concatenation" | "ansi_c_string" | "number" => {
                if target.replace(c).is_some() {
                    return Err(());
                }
            }
            _ => return Err(()),
        }
    }
    let (Some(op), Some(t)) = (op, target) else {
        return Err(());
    };
    // 读侧、fd 复制不写文件
    if matches!(op, "<" | "<&") || (op == ">&" && t.kind() == "number") {
        return Ok(None);
    }
    if op == "<>" {
        return Ok(None);
    }
    let raw = t.utf8_text(src).map_err(|_| ())?;
    let literal = ast::unquote(raw);
    if literal.is_empty() || literal.starts_with("/dev/") {
        return Ok(None);
    }
    Ok(Some(literal))
}

fn ops_of(sub: &SubCommand) -> Vec<FileOp> {
    // glob / `~` 执行时才知道展开成什么，这条子命令的目标无从谈起
    if sub.has_unquoted_glob {
        return Vec::new();
    }
    let name = ast::unquote(&sub.name);
    let args: Vec<String> = sub.args.iter().map(|a| ast::unquote(a)).collect();
    match basename(&name) {
        "cp" => transfer(&args, false),
        "mv" => transfer(&args, true),
        "touch" => split_args(&args, &["-d", "--date", "-r", "--reference", "-t"])
            .1
            .into_iter()
            .filter(|p| is_path_like(p))
            .map(FileOp::Write)
            .collect(),
        "tee" => split_args(&args, &[])
            .1
            .into_iter()
            .filter(|p| is_path_like(p))
            .map(FileOp::Write)
            .collect(),
        "rm" => split_args(&args, &[])
            .1
            .into_iter()
            .filter(|p| is_path_like(p))
            .map(FileOp::Remove)
            .collect(),
        "sed" => sed_in_place(&args),
        _ => Vec::new(),
    }
}

/// `cp` / `mv`：`-t DIR` / `--target-directory=DIR` 时全部位置参数都是源；
/// 否则最后一个是目标。递归标志不在这里判 —— 目录由工具层看磁盘跳过。
fn transfer(args: &[String], moves: bool) -> Vec<FileOp> {
    let (flags, mut pos) = split_args(args, &["-t", "--target-directory", "-S", "--suffix"]);
    let explicit_dest = flags.iter().find_map(|f| match f {
        Flag::Valued(name, v) if name == "-t" || name == "--target-directory" => Some(v.clone()),
        _ => None,
    });
    let dest = match explicit_dest {
        Some(d) => d,
        None => {
            if pos.len() < 2 {
                return Vec::new();
            }
            pos.pop().unwrap_or_default()
        }
    };
    let sources: Vec<String> = pos.into_iter().filter(|p| is_path_like(p)).collect();
    if sources.is_empty() || !is_path_like(&dest) {
        return Vec::new();
    }
    vec![FileOp::Transfer {
        sources,
        dest,
        moves,
    }]
}

/// `sed -i`：就地改写的文件。脚本是第一个位置参数（没给 `-e` / `-f` 时）。
///
/// macOS 的 `sed -i '' …` 里那个空后缀会被当成脚本、真脚本被当成文件名 ——
/// 那个"文件"前后都不存在，工具层比对时自然落空，不用在这里分辨平台。
fn sed_in_place(args: &[String]) -> Vec<FileOp> {
    let (flags, pos) = split_args(
        args,
        &["-e", "--expression", "-f", "--file", "-l", "--line-length"],
    );
    let in_place = flags.iter().any(|f| {
        let name = f.name();
        (name.starts_with("-i") && !name.starts_with("--")) || name.starts_with("--in-place")
    });
    if !in_place {
        return Vec::new();
    }
    let has_script_flag = flags.iter().any(|f| {
        matches!(f.name(), "-e" | "--expression" | "-f" | "--file")
            || f.name().starts_with("--expression=")
            || f.name().starts_with("--file=")
    });
    let files = if has_script_flag {
        pos
    } else {
        pos.into_iter().skip(1).collect()
    };
    files
        .into_iter()
        .filter(|p| is_path_like(p))
        .map(FileOp::Write)
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
enum Flag {
    /// `-f`、`--force`、`--suffix=.bak`（值内联的按整段存）。
    Plain(String),
    /// 后面跟独立值的：`-t DIR`。
    Valued(String, String),
}

impl Flag {
    fn name(&self) -> &str {
        match self {
            Flag::Plain(n) | Flag::Valued(n, _) => n,
        }
    }
}

/// 把参数分成选项和位置参数。`--` 之后全是位置参数。`value_flags` 列出的
/// 选项会吃掉下一个参数当值（`--flag=value` 的内联形式不算）。
fn split_args(args: &[String], value_flags: &[&str]) -> (Vec<Flag>, Vec<String>) {
    let mut flags = Vec::new();
    let mut pos = Vec::new();
    let mut i = 0;
    let mut only_pos = false;
    while i < args.len() {
        let a = &args[i];
        if !only_pos && a == "--" {
            only_pos = true;
            i += 1;
            continue;
        }
        if !only_pos && a.starts_with('-') && a.len() > 1 {
            if value_flags.contains(&a.as_str()) {
                match args.get(i + 1) {
                    Some(v) => flags.push(Flag::Valued(a.clone(), v.clone())),
                    None => flags.push(Flag::Plain(a.clone())),
                }
                i += 2;
            } else {
                flags.push(Flag::Plain(a.clone()));
                i += 1;
            }
            continue;
        }
        pos.push(a.clone());
        i += 1;
    }
    (flags, pos)
}

/// 排掉明显不是文件路径的位置参数：空串、`-`（标准输入）、设备文件。
fn is_path_like(p: &str) -> bool {
    !p.is_empty() && p != "-" && !p.starts_with("/dev/")
}

fn basename(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn w(p: &str) -> FileOp {
        FileOp::Write(p.to_owned())
    }
    fn rm(p: &str) -> FileOp {
        FileOp::Remove(p.to_owned())
    }
    fn cp(sources: &[&str], dest: &str) -> FileOp {
        FileOp::Transfer {
            sources: sources.iter().map(|s| (*s).to_owned()).collect(),
            dest: dest.to_owned(),
            moves: false,
        }
    }
    fn mv(sources: &[&str], dest: &str) -> FileOp {
        FileOp::Transfer {
            sources: sources.iter().map(|s| (*s).to_owned()).collect(),
            dest: dest.to_owned(),
            moves: true,
        }
    }

    /// 用户撞上的那条原样：`cp` 后面跟着一个带 glob 的 `ls`。glob 只让
    /// `ls` 那条子命令作废，`cp` 照样认 —— 否则最常见的"复制完列一下"
    /// 写法永远记不上。
    #[test]
    fn 复制后列目录_只放弃带glob的那条() {
        assert_eq!(
            file_effects("cp caiwu.txt caiwu-copy.txt && ls -la caiwu*.txt"),
            vec![cp(&["caiwu.txt"], "caiwu-copy.txt")]
        );
    }

    #[test]
    fn cp_与_mv_的源和目标() {
        assert_eq!(file_effects("cp a b"), vec![cp(&["a"], "b")]);
        assert_eq!(
            file_effects("cp -r a b dir/"),
            vec![cp(&["a", "b"], "dir/")],
            "递归标志不在这里判，目录由工具层看磁盘跳过"
        );
        assert_eq!(
            file_effects("mv 'x y.txt' z.txt"),
            vec![mv(&["x y.txt"], "z.txt")],
            "引号要还原"
        );
        assert_eq!(
            file_effects("cp -t dst a b"),
            vec![cp(&["a", "b"], "dst")],
            "-t 之后全是源"
        );
        assert_eq!(file_effects("cp a"), vec![], "少于两个位置参数不成立");
        assert_eq!(file_effects("/bin/cp a b"), vec![cp(&["a"], "b")]);
    }

    #[test]
    fn 重定向目标() {
        assert_eq!(file_effects("echo hi > out.txt"), vec![w("out.txt")]);
        assert_eq!(
            file_effects("echo hi >> 'log file.txt'"),
            vec![w("log file.txt")]
        );
        assert_eq!(file_effects("cargo test 2> err.log"), vec![w("err.log")]);
        assert_eq!(
            file_effects("cargo test > /dev/null 2>&1"),
            vec![],
            "丢弃输出和 fd 复制不写文件"
        );
        assert_eq!(file_effects("sort < in.txt"), vec![], "读侧不算");
        assert_eq!(
            file_effects("cat <<EOF > gen.txt\nhello\nEOF"),
            vec![w("gen.txt")],
            "heredoc 正文是字面量，重定向目标照认"
        );
    }

    #[test]
    fn touch_tee_rm() {
        assert_eq!(file_effects("touch a b"), vec![w("a"), w("b")]);
        assert_eq!(
            file_effects("touch -d yesterday f"),
            vec![w("f")],
            "-d 的值不是文件"
        );
        assert_eq!(file_effects("cat a | tee -a b"), vec![w("b")]);
        assert_eq!(file_effects("rm -f a b"), vec![rm("a"), rm("b")]);
        assert_eq!(
            file_effects("rm -rf build"),
            vec![rm("build")],
            "目录由工具层看磁盘跳过，这里只给候选"
        );
        assert_eq!(file_effects("rm -- -weird"), vec![rm("-weird")]);
    }

    #[test]
    fn sed_只认就地改写() {
        assert_eq!(file_effects("sed -i 's/a/b/' f"), vec![w("f")]);
        assert_eq!(file_effects("sed -i.bak 's/a/b/' f"), vec![w("f")]);
        assert_eq!(
            file_effects("sed -i -e 's/a/b/' f g"),
            vec![w("f"), w("g")],
            "给了 -e，位置参数全是文件"
        );
        assert_eq!(
            file_effects("sed -n 's/a/b/p' f"),
            vec![],
            "不带 -i 不写文件"
        );
        // macOS 形态：空后缀被当成脚本、真脚本被当成文件名。那个"文件"前后
        // 都不存在，工具层比对时落空；真正的文件仍然在
        assert_eq!(
            file_effects("sed -i '' 's/a/b/' f"),
            vec![w("s/a/b/"), w("f")]
        );
    }

    /// 模型最常用的写法之一：先 `cd` 到项目目录再干活。后面的相对路径
    /// 都要拼上它，否则这一大类命令全漏。截图里那条清理命令就是这个形状。
    #[test]
    fn 字面_cd_之后的相对路径拼上目录() {
        assert_eq!(
            file_effects("cd /Users/u/code && rm -f _a.js _b.txt && echo cleaned"),
            vec![rm("/Users/u/code/_a.js"), rm("/Users/u/code/_b.txt")]
        );
        assert_eq!(
            file_effects("cd sub && cp a b && echo x > log.txt"),
            vec![cp(&["sub/a"], "sub/b"), w("sub/log.txt")]
        );
        assert_eq!(
            file_effects("cp a b && cd sub && cp c d"),
            vec![cp(&["a"], "b"), cp(&["sub/c"], "sub/d")],
            "cd 只影响它后面的"
        );
        assert_eq!(
            file_effects("cd a && cd b && touch f"),
            vec![w("a/b/f")],
            "连续 cd 叠加"
        );
        assert_eq!(
            file_effects("cd sub && cp a /abs/b"),
            vec![cp(&["sub/a"], "/abs/b")],
            "绝对路径不拼"
        );
        assert_eq!(
            file_effects("cd 'my dir/' && rm x"),
            vec![rm("my dir/x")],
            "目录带尾斜杠不重复加"
        );
    }

    #[test]
    fn 看不懂的_cd_整条放弃() {
        for cmd in [
            "cd && rm x",
            "cd - && rm x",
            "cd -P sub && rm x",
            "cd $DIR && rm x",
            "cd ~/proj && rm x",
            "pushd sub && rm x",
        ] {
            assert_eq!(file_effects(cmd), vec![], "{cmd} 该整条放弃");
        }
    }

    /// 宁可漏记，不能记错：任何看不懂的结构都让整条命令返回空。
    #[test]
    fn 看不懂的结构整条放弃() {
        for cmd in [
            "cp $SRC b",
            "cp a $(mktemp)",
            "for f in a b; do cp $f d; done",
            "cp a b &",
            "if [ -f a ]; then cp a b; fi",
            "f() { cp a b; }; f",
            "cp a b > $LOG",
            "eval 'cp a b'",
        ] {
            assert_eq!(file_effects(cmd), vec![], "{cmd} 该整条放弃");
        }
    }

    #[test]
    fn glob_只作废那条子命令() {
        assert_eq!(file_effects("cp *.txt d/"), vec![]);
        assert_eq!(file_effects("rm ~/x"), vec![], "未加引号的 ~ 执行时才展开");
        assert_eq!(file_effects("cp a b; rm *.tmp"), vec![cp(&["a"], "b")]);
    }

    #[test]
    fn 日常命令没有文件效应() {
        for cmd in [
            "cargo build --release",
            "npm install",
            "git commit -m fix",
            "ls -la",
            "mkdir -p a/b",
            "cat a.txt",
            "grep -rn foo src",
        ] {
            assert_eq!(file_effects(cmd), vec![], "{cmd}");
        }
    }
}
