//! Bash 命令的 AST 分析。
//!
//! `[约束]` **只允许明确白名单的 AST 节点类型。凡是不理解的结构,不假装
//! 能证明它安全,直接 Ask。**
//!
//! 这条约束的方向很重要:白名单是"哪些节点允许出现",不是"哪些节点要拦"。
//! 黑名单在这里必然失守 —— bash 的 grammar 有一百多种节点,而且会随
//! tree-sitter-bash 升级增加。漏掉一种新节点,黑名单的表现是**静默放行**。
//!
//! # 白名单基于 grammar 的实际产出
//!
//! 下面每一条都是从 tree-sitter-bash 0.25 的真实输出里读出来的,不是
//! 照着 bash 手册想出来的。几个反直觉的地方:
//!
//! - `ls &` 里的 `&` 是**匿名节点**。只遍历 named 节点的话,后台执行和
//!   普通执行的 AST 完全一样。
//! - `;` 和换行分隔的命令**不产生包装节点**,直接挂在 `program` 下面。
//!   只处理 `list` 会漏掉 `ls; rm -rf /`。
//! - `git commit -m 'msg with && inside'` 里的 `&&` 在 `raw_string` 内,
//!   AST 不会把它当成命令分隔符。这正是用 AST 而不是正则拆分的理由。
//!
//! 见 ARCHITECTURE.md §9.3

use std::collections::BTreeSet;

use riot_protocol::permission::SafetyKind;

/// 单条命令能拆出的子命令数上限。
///
/// 超过就 Ask。这个数字不是性能考虑 —— 是可读性:一条含 50 个子命令的
/// 命令行,用户在弹窗里根本审不过来,逐条列出反而制造"我看过了"的错觉。
const MAX_SUB_COMMANDS: usize = 50;

/// 命令文本长度上限。
///
/// tree-sitter 对超长输入是线性的,但下游的规则匹配和 UI 渲染不是。
const MAX_COMMAND_LEN: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Analysis {
    /// 拆成了若干条简单命令,每条都只含白名单节点。
    Simple(Vec<SubCommand>),
    /// 含有不认识或危险的结构。
    TooComplex(Complexity),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubCommand {
    /// 剥掉安全包装后的命令名。`timeout 30 npm test` → `npm`。
    pub name: String,
    pub args: Vec<String>,
    /// 用于规则匹配的文本,同样是剥掉包装后的。
    pub matchable: String,
    /// 命令前缀的环境变量赋值。
    pub assignments: Vec<(String, String)>,
    /// 参数里有未加引号的 glob 或波浪号展开。
    ///
    /// 这些在执行时会变成别的东西,只读判定必须放弃。
    pub has_unquoted_glob: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Complexity {
    pub reason: ComplexReason,
    /// 触发的具体片段,给用户看。
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComplexReason {
    /// `$()`、反引号 —— 执行时才知道内容。
    CommandSubstitution,
    /// `<()`、`>()`。
    ProcessSubstitution,
    /// `$VAR`、`${VAR}`、`$(())` —— 展开结果不可知。
    Expansion,
    /// `ls &` —— 后台执行逃出生命周期管理。
    Background,
    /// 重定向到普通路径。目标没过围栏检查,但也不是敏感目标。
    Redirect,
    /// 重定向到敏感目标（shell 启动脚本、SSH 配置、凭证……）。
    ///
    /// 和 [`ComplexReason::Redirect`] 分开是因为**只有这一种对
    /// 「全部放行」免疫**。`cargo test > out.log` 和
    /// `echo evil >> ~/.zshrc` 在语法上是同一个结构,危险程度差着量级,
    /// 混在一起就只能二选一:要么长任务寸步难行,要么放开持久化执行权。
    SensitiveRedirect(SafetyKind),
    /// 子 shell、代码块、循环、条件、函数定义。
    ControlFlow,
    /// `eval`、`source` —— 内容运行时决定。
    DynamicExecution,
    /// 危险的环境变量赋值。
    DangerousAssignment,
    /// 语法解析失败。
    ParseError,
    /// 子命令太多。
    TooManyCommands,
    /// 命令文本过长。
    TooLong,
    /// 安全包装嵌套过深。
    NestedWrappers,
    /// grammar 产出了白名单之外的节点。
    UnknownNode,
}

impl ComplexReason {
    /// 这是「确实危险」还是「静态分析看不懂」。
    ///
    /// 这个区分决定了「全部放行」管不管用，所以只能有一处定义 ——
    /// 分析阶段（谁压过谁）和决策阶段（`DecisionReason` 选哪个）必须
    /// 用同一份答案，各写一份迟早漂移，而漂移的那侧不会报错。
    ///
    /// 危险的三样都**精确**：目标明确，不是启发式猜测。其余的只是
    /// 分析器不敢断言，那在正常开发里遍地都是（`$VAR`、`$(...)`、循环）。
    pub fn is_danger(self) -> bool {
        matches!(
            self,
            ComplexReason::SensitiveRedirect(_)
                | ComplexReason::DangerousAssignment
                | ComplexReason::DynamicExecution
        )
    }
}

/// 允许出现在简单命令里的节点类型。
///
/// `[约束]` 往这里加节点前先问:这个节点的内容在**解析时**就完全确定吗?
/// 只要答案是"取决于运行时环境",就不能加。
static ALLOWED_NODES: &[&str] = &[
    "program",
    "command",
    "command_name",
    "word",
    "number",
    // 单引号字符串。内部不展开,所以整体是字面量。
    "raw_string",
    // 双引号字符串。内部**可能**有展开,但展开节点会单独出现在子节点里
    // 并被拦下,所以字符串壳本身是安全的。
    "string",
    "string_content",
    // `foo"bar"baz` 这种拼接
    "concatenation",
    // 带重定向的命令。节点本身无害 —— 危险在具体的重定向目标上,
    // 每个 `file_redirect` 子节点单独过 [`discards_output`],
    // 写文件的那些照样在这里被拦下。
    "redirected_statement",
    // `&&` `||` 连接
    "list",
    // 管道
    "pipeline",
    // `FOO=bar cmd` —— 变量名单独过黑名单
    "variable_assignment",
    "variable_name",
    // ANSI-C 字符串 `$'\n'`。是字面量,但常用于 IFS 注入,
    // 所以变量名黑名单里有 IFS。
    "ansi_c_string",
    // 转义序列 `\;`
    "escape_sequence",
    // 注释
    "comment",
];

/// 匿名节点里允许的。
///
/// `&`（后台执行）**不在**这里 —— 它是匿名的,漏掉它的话
/// `npm test &` 会被当成普通的 `npm test`。
static ALLOWED_ANON: &[&str] = &["&&", "||", "|", ";", ";;", "\n", "\"", "'", "=", "|&"];

/// 会改变动态链接、命令查找或分词行为的环境变量。
///
/// `[约束]` 这些**不剥离、不放行**。剥掉安全包装是为了让规则匹配到真实
/// 的命令,但 `LD_PRELOAD=/evil.so ls` 的危险恰恰在包装本身 ——
/// 剥掉之后规则看到的是无害的 `ls`。
static DANGEROUS_VARS: &[&str] = &[
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "DYLD_INSERT_LIBRARIES", // macOS 版 LD_PRELOAD
    "DYLD_LIBRARY_PATH",
    "DYLD_FRAMEWORK_PATH",
    "PATH",
    "IFS", // 改分词规则,能让 "safe arg" 变成两个参数
    "BASH_ENV",
    "ENV",
    "SHELLOPTS",
    "BASHOPTS",
    "PS4", // 配合 set -x 可执行任意代码
    "GLOBIGNORE",
    "PERL5LIB",
    "PERL5OPT",
    "PYTHONPATH",
    "PYTHONSTARTUP",
    "NODE_OPTIONS", // --require 能注入任意模块
    "RUBYOPT",
    "GIT_SSH",
    "GIT_SSH_COMMAND",
    "GIT_EXTERNAL_DIFF",
    "GIT_PAGER",
    "PAGER",
    "EDITOR",
    "VISUAL",
];

/// 内容在运行时才确定的命令。
static DYNAMIC_COMMANDS: &[&str] = &["eval", "source", ".", "exec", "trap", "alias", "unalias"];

/// 安全包装:剥掉之后规则匹配到的才是真正执行的命令。
///
/// `[约束]` 只剥"不改变被包装命令语义"的。`sudo` 不在这里 —— 它改变的
/// 是权限,剥掉就看不见了。
/// `[约束]` 每个包装器必须**精确**声明自己的参数形态。
///
/// 含糊的"跳过前导 flag"策略会剥错:`timeout -k 5 30 npm test` 里
/// `5` 是 `-k` 的值、`30` 才是时长,粗略跳过会把命令名认成 `30`。
/// 剥错的后果不是报错,是规则匹配到一条不存在的命令 —— 然后静默地
/// 走了错误的分支。
///
/// 形态对不上时**不剥**,保留原样让它走正常的询问流程。
static WRAPPERS: &[Wrapper] = &[
    Wrapper {
        name: "timeout",
        value_flags: &["-k", "--kill-after", "-s", "--signal"],
        positionals: 1, // DURATION
    },
    Wrapper {
        name: "nice",
        value_flags: &["-n", "--adjustment"],
        positionals: 0,
    },
    Wrapper {
        name: "nohup",
        value_flags: &[],
        positionals: 0,
    },
    Wrapper {
        name: "command",
        value_flags: &[],
        positionals: 0,
    },
    Wrapper {
        name: "builtin",
        value_flags: &[],
        positionals: 0,
    },
    Wrapper {
        name: "stdbuf",
        value_flags: &["-i", "-o", "-e", "--input", "--output", "--error"],
        positionals: 0,
    },
];

struct Wrapper {
    name: &'static str,
    /// 这些 flag 后面跟一个独立的值参数。`--flag=value` 形式不算。
    value_flags: &'static [&'static str],
    /// 包装器自己的位置参数个数。`timeout 30 cmd` 的 `30`。
    positionals: usize,
}

pub fn analyze(command: &str) -> Analysis {
    if command.len() > MAX_COMMAND_LEN {
        return too_complex(
            ComplexReason::TooLong,
            format!("命令有 {} 字节,上限 {MAX_COMMAND_LEN}", command.len()),
        );
    }

    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .is_err()
    {
        // 语言版本不匹配。这是构建期问题,但运行时遇到只能 fail-closed。
        return too_complex(ComplexReason::ParseError, "解析器初始化失败".into());
    }

    let Some(tree) = parser.parse(command, None) else {
        return too_complex(ComplexReason::ParseError, "解析器没有返回语法树".into());
    };

    let root = tree.root_node();
    if root.has_error() || root.is_missing() {
        return too_complex(
            ComplexReason::ParseError,
            "命令有语法错误,无法确定它会执行什么".into(),
        );
    }

    let src = command.as_bytes();

    // 先扫一遍全树找禁止的结构。这一步在提取子命令之前做 ——
    // 提取逻辑只处理它认识的形状,先扫描才能保证没有漏网的。
    //
    // `[约束]` "看不懂"的发现不能就地返回,必须攒着把整棵树扫完。
    // 危险结构可能藏在一个无害结构**后面**:`eval "$CMD"` 里 `$CMD` 的
    // 变量展开先被遇到,而 `eval` 才是真正的问题。就地返回的话,一个
    // 可被放行压过的判定会把一个不可放行的判定挡住 —— 危险被无害掩护。
    let uncertain = match scan_forbidden(root, src) {
        Some(c) if c.reason.is_danger() => return Analysis::TooComplex(c),
        other => other,
    };

    // 即使已经看不懂,也要把子命令挖出来 —— `eval` / `source` 这类危险
    // 是在提取阶段按命令名认出来的,跳过就漏了。
    let mut subs = Vec::new();
    match collect_commands(root, src, &mut subs) {
        Err(c) if c.reason.is_danger() => return Analysis::TooComplex(c),
        // 提取失败但不危险:优先报扫描阶段那个,它的片段更贴近原因。
        Err(c) => return Analysis::TooComplex(uncertain.unwrap_or(c)),
        Ok(()) => {}
    }

    if let Some(c) = uncertain {
        return Analysis::TooComplex(c);
    }

    if subs.len() > MAX_SUB_COMMANDS {
        return too_complex(
            ComplexReason::TooManyCommands,
            format!("拆出 {} 条子命令,上限 {MAX_SUB_COMMANDS}", subs.len()),
        );
    }

    Analysis::Simple(subs)
}

fn too_complex(reason: ComplexReason, detail: String) -> Analysis {
    Analysis::TooComplex(Complexity { reason, detail })
}

/// 遍历全树,找出不在白名单里的结构。
///
/// 遍历**包含匿名节点**。`&` 是匿名的,只看 named 节点的话
/// `npm test &` 和 `npm test` 无法区分。
///
/// `[约束]` 危险的发现立刻返回,"看不懂"的只记第一个然后**继续扫完**。
/// 两者都就地返回的话,树里靠前的无害结构会把靠后的危险结构挡住 ——
/// 而前者可以被「全部放行」压过,后者不能。`echo $HOME >> ~/.zshrc`
/// 就是这个形状:变量展开在前,写 shell 启动脚本在后。
fn scan_forbidden(root: tree_sitter::Node, src: &[u8]) -> Option<Complexity> {
    let mut stack = vec![root];
    let mut cur = root.walk();
    let mut uncertain: Option<Complexity> = None;

    while let Some(node) = stack.pop() {
        let kind = node.kind();

        if kind == "file_redirect" {
            // 丢弃输出的重定向不碰文件系统,放行且不再下探 ——
            // 里面的 fd 号和 `/dev/null` 已经在这一步验完了。
            if discards_output(node, src) {
                continue;
            }
            // 会写文件的重定向按目标分级。只有敏感目标那一档对
            // 「全部放行」免疫,普通路径归为单纯的不确定性。
            if let Some(risk) = redirect_target_risk(node, src) {
                return Some(Complexity {
                    reason: ComplexReason::SensitiveRedirect(risk),
                    detail: snippet(node, src),
                });
            }
        }

        let allowed = if node.is_named() {
            ALLOWED_NODES.contains(&kind)
        } else {
            ALLOWED_ANON.contains(&kind)
        };

        if !allowed {
            // 记下第一个"看不懂",但继续往下扫 —— 后面可能还藏着危险的。
            // 仍然下探子节点:`$(eval $X)` 的危险在展开节点的内部。
            uncertain.get_or_insert_with(|| Complexity {
                reason: classify(kind),
                detail: snippet(node, src),
            });
        }

        // 变量赋值单独查变量名
        if kind == "variable_assignment"
            && let Some(name) = node.child_by_field_name("name")
            && let Ok(text) = name.utf8_text(src)
            && DANGEROUS_VARS.contains(&text.to_ascii_uppercase().as_str())
        {
            return Some(Complexity {
                reason: ComplexReason::DangerousAssignment,
                detail: format!("`{text}=` 会改变动态链接、命令查找或分词行为"),
            });
        }

        for c in node.children(&mut cur) {
            stack.push(c);
        }
    }

    uncertain
}

/// 这个重定向是不是**丢弃输出**（而不是写文件）。
///
/// 拦重定向的理由是目标路径不过工作目录围栏 —— `ls > /etc/passwd`
/// 能绕开 Write 工具的目录限制。但有两种重定向根本碰不到文件系统:
///
/// - `2>/dev/null`:写进空设备,内容凭空消失,没有围栏可绕。
/// - `2>&1`:把一个 fd 指向另一个 fd,连 open 都不会发生。
///
/// 这两种是 shell 里最常见的写法。把它们一并拦掉的代价不只是烦:
/// 这条路径产出的是**安全发现**,对 bypass 模式免疫,而且不给"总是允许"
/// 建议 —— 用户没有任何办法让它停下来。
///
/// `[约束]` `>&` 后面跟单词是**文件重定向**不是 fd 复制。bash 里
/// `ls >&out.txt` 等价于 `ls &>out.txt`,照样写文件,而它和 `ls >&2`
/// 在语法树上共用同一个操作符节点。只能靠目标是不是纯数字来分辨。
fn discards_output(redirect: tree_sitter::Node, src: &[u8]) -> bool {
    let mut cur = redirect.walk();
    let mut is_fd_dup = false;
    let mut target = None;

    for c in redirect.children(&mut cur) {
        match c.kind() {
            // `2>` 左边那个 2
            "file_descriptor" => {}
            ">&" | "<&" => is_fd_dup = true,
            ">" | ">>" | "<" | "&>" | "&>>" => {}
            "number" | "word" => {
                // 认不出的形状一律当成危险的
                if target.replace(c).is_some() {
                    return false;
                }
            }
            // 目标是变量展开、命令替换之类 —— 运行时才知道写去哪
            _ => return false,
        }
    }

    let Some(t) = target else { return false };

    if is_fd_dup {
        return t.kind() == "number";
    }

    t.utf8_text(src).is_ok_and(|s| s == "/dev/null")
}

/// 这个重定向写向的是不是敏感目标。
///
/// `[约束]` 这是 Bash 唯一一条能看见敏感路径的通道。通用的
/// [`crate::safety::check`] 走 `Tool::target_path`,而 Bash 没有单一目标
/// 路径（返回 `None`）—— 也就是说整个路径安全检查**对 Bash 完全不生效**,
/// `echo evil >> ~/.zshrc` 里那个 `~/.zshrc` 除了这里没人看得见。
///
/// 拿掉这个函数,`~/.zshrc` 的保护就只剩"所有重定向一律 Ask"那条一刀切
/// 规则;而那条规则一旦为了长任务放宽,持久化执行权就跟着敞开了。
fn redirect_target_risk(redirect: tree_sitter::Node, src: &[u8]) -> Option<SafetyKind> {
    let mut cur = redirect.walk();
    for c in redirect.children(&mut cur) {
        if c.kind() != "word" {
            continue;
        }
        let raw = c.utf8_text(src).ok()?;
        // `~` 不展开也能判 —— is_shell_rc 之类看的是最后一段文件名。
        // 重定向就是写,`read_only = false`。
        if let Some(k) = crate::safety::write_target_risk(std::path::Path::new(raw), false) {
            return Some(k);
        }
    }
    None
}

fn classify(kind: &str) -> ComplexReason {
    match kind {
        "command_substitution" => ComplexReason::CommandSubstitution,
        "process_substitution" => ComplexReason::ProcessSubstitution,
        "expansion" | "simple_expansion" | "arithmetic_expansion" | "$" => ComplexReason::Expansion,
        "&" => ComplexReason::Background,
        "file_redirect"
        | "heredoc_redirect"
        | "redirected_statement"
        | "herestring_redirect"
        | ">"
        | ">>"
        | "<"
        | "&>" => ComplexReason::Redirect,
        "subshell"
        | "compound_statement"
        | "for_statement"
        | "while_statement"
        | "if_statement"
        | "case_statement"
        | "function_definition"
        | "do_group"
        | "negated_command"
        | "c_style_for_statement"
        | "ternary_expression" => ComplexReason::ControlFlow,
        _ => ComplexReason::UnknownNode,
    }
}

fn snippet(node: tree_sitter::Node, src: &[u8]) -> String {
    let text = node.utf8_text(src).unwrap_or("<无法解码>");
    let s: String = text.chars().take(80).collect();
    if s.len() < text.len() {
        format!("{s}…")
    } else {
        s
    }
}

/// 提取所有 `command` 节点,展平成子命令列表。
///
/// 遍历刻意不限定父节点类型。当前的白名单下,能走到这里的树里 `command`
/// 只可能挂在 `program` / `list` / `pipeline` 底下 —— 所以"只递归这三种"
/// 和"全都递归"行为完全一样。
///
/// `[约束]` 仍然要全都递归。两种写法的**失效方向**不同:宽松遍历在
/// [`scan_forbidden`] 漏掉某个容器节点时会多找到命令(多检查一遍,安全),
/// 严格遍历会直接漏掉它们(不检查,放行)。
///
/// 这个函数的正确性依赖于 `scan_forbidden` 已经拦掉了所有控制流结构。
/// 往 [`ALLOWED_NODES`] 加节点时要重新想一遍这条依赖。
fn collect_commands(
    node: tree_sitter::Node,
    src: &[u8],
    out: &mut Vec<SubCommand>,
) -> Result<(), Complexity> {
    if out.len() > MAX_SUB_COMMANDS {
        // 提前退出,不要在恶意输入上把整棵树走完
        return Ok(());
    }

    if node.kind() == "command" {
        out.push(parse_command(node, src)?);
        return Ok(());
    }

    let mut cur = node.walk();
    for child in node.named_children(&mut cur) {
        collect_commands(child, src, out)?;
    }
    Ok(())
}

fn parse_command(node: tree_sitter::Node, src: &[u8]) -> Result<SubCommand, Complexity> {
    let mut assignments = Vec::new();
    let mut name = String::new();
    let mut args: Vec<String> = Vec::new();
    let mut has_unquoted_glob = false;

    let mut cur = node.walk();
    for child in node.named_children(&mut cur) {
        match child.kind() {
            "variable_assignment" => {
                let text = child.utf8_text(src).unwrap_or_default();
                if let Some((k, v)) = text.split_once('=') {
                    assignments.push((k.to_owned(), v.to_owned()));
                }
            }
            "command_name" => {
                name = child.utf8_text(src).unwrap_or_default().to_owned();
            }
            "comment" => {}
            _ => {
                let text = child.utf8_text(src).unwrap_or_default();
                // 未加引号的 glob / 波浪号:执行时展开成别的东西。
                // `python *` 可能变成 `python evil.py`。
                if child.kind() == "word" && text.contains(['*', '?', '[', '~']) {
                    has_unquoted_glob = true;
                }
                args.push(text.to_owned());
            }
        }
    }

    if DYNAMIC_COMMANDS.contains(&name.as_str()) {
        return Err(Complexity {
            reason: ComplexReason::DynamicExecution,
            detail: format!("`{name}` 执行的内容在运行时才确定"),
        });
    }

    let (name, args) = unwrap_wrappers(name, args)?;

    let matchable = if args.is_empty() {
        name.clone()
    } else {
        format!("{name} {}", args.join(" "))
    };

    Ok(SubCommand {
        name,
        args,
        matchable,
        assignments,
        has_unquoted_glob,
    })
}

/// 剥掉安全包装,让规则匹配到真正执行的命令。
///
/// `timeout 30 npm test` → `npm test`。否则用户得为每种包装写一遍规则。
fn unwrap_wrappers(
    mut name: String,
    mut args: Vec<String>,
) -> Result<(String, Vec<String>), Complexity> {
    // 防御嵌套包装 `timeout 30 nice command ls` 造成的循环
    for _ in 0..8 {
        let Some(w) = WRAPPERS.iter().find(|w| w.name == name) else {
            return Ok((name, args));
        };

        let Some((inner, inner_args)) = strip_one(w, &args) else {
            // 形态对不上。不剥,保留原样 —— 规则匹配不到就会走询问流程,
            // 这比剥错之后匹配到错误的命令安全。
            return Ok((name, args));
        };

        if DYNAMIC_COMMANDS.contains(&inner.as_str()) {
            return Err(Complexity {
                reason: ComplexReason::DynamicExecution,
                detail: format!("包装里藏着 `{inner}`,执行内容在运行时才确定"),
            });
        }

        name = inner;
        args = inner_args;
    }

    Err(Complexity {
        reason: ComplexReason::NestedWrappers,
        detail: "包装器嵌套过深".into(),
    })
}

/// 剥掉一层包装。形态不符返回 `None`。
fn strip_one(w: &Wrapper, args: &[String]) -> Option<(String, Vec<String>)> {
    let mut i = 0;
    let mut positionals_left = w.positionals;

    while i < args.len() {
        let tok = &args[i];

        if tok == "--" {
            i += 1;
            break;
        }

        if tok.starts_with('-') && tok.len() > 1 {
            // `--flag=value` 自带值,不吃下一个
            let takes_value = !tok.contains('=') && w.value_flags.contains(&tok.as_str());
            i += if takes_value { 2 } else { 1 };
            continue;
        }

        // 非 flag。先填包装器自己的位置参数,填满了就是被包装的命令。
        if positionals_left > 0 {
            positionals_left -= 1;
            i += 1;
            continue;
        }
        break;
    }

    // 位置参数没填满(`timeout 30` 后面没命令),或者根本没有命令
    if positionals_left > 0 || i >= args.len() {
        return None;
    }

    Some((args[i].clone(), args[i + 1..].to_vec()))
}

/// 命令里出现的所有命令名,去重。给 UI 和日志用。
pub fn command_names(subs: &[SubCommand]) -> BTreeSet<&str> {
    subs.iter().map(|s| s.name.as_str()).collect()
}
