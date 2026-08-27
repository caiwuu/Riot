//! 只读判定。
//!
//! `[约束]` 判定依据是"命令名在白名单 **且** 所有 flag 都在该命令的安全
//! flag 白名单里",不是"不在危险名单里就算只读"。
//!
//! 黑名单在这里必然失守:只读命令的集合是开放的(每个人的机器上都有不同
//! 的工具),而"某个看起来无害的命令带上某个 flag 就能写文件"的组合太多。
//! `find` 是最好的例子 —— 它是标准的只读工具,直到你给它 `-exec`。
//!
//! 判成只读的后果是**跳过用户确认直接执行**,所以这里的错误方向必须是
//! "把只读的判成非只读"(多问一次),不能是反过来。
//!
//! 见 ARCHITECTURE.md §9.4

use super::ast::SubCommand;

/// 只读命令白名单。
///
/// `[约束]` 往这里加命令前先问三件事:
///
/// 1. 它有没有任何 flag 能写文件或执行子命令?有的话必须配 flag 白名单。
/// 2. 它会不会泄露凭证?`env` / `printenv` 因此**故意不在**这里 ——
///    它们不改任何东西,但会把 API key 打进对话历史。
/// 3. 它是不是分页器或编辑器?那些会挂住终端等输入。
static READ_ONLY: &[(&str, Flags)] = &[
    ("ls", Flags::Any),
    ("pwd", Flags::Any),
    ("echo", Flags::Any),
    ("cat", Flags::Any),
    ("head", Flags::Any),
    ("tail", Flags::Deny(&["-f", "--follow", "-F"])), // -f 永不返回
    ("wc", Flags::Any),
    ("file", Flags::Any),
    ("stat", Flags::Any),
    ("basename", Flags::Any),
    ("dirname", Flags::Any),
    ("realpath", Flags::Any),
    ("which", Flags::Any),
    ("type", Flags::Any),
    ("date", Flags::Deny(&["-s", "--set"])),
    ("uname", Flags::Any),
    ("whoami", Flags::Any),
    ("hostname", Flags::Deny(&["-b", "--boot"])),
    ("df", Flags::Any),
    ("du", Flags::Any),
    ("tree", Flags::Any),
    ("diff", Flags::Any),
    ("cmp", Flags::Any),
    ("sort", Flags::Deny(&["-o", "--output"])),
    ("uniq", Flags::Any),
    ("cut", Flags::Any),
    ("tr", Flags::Any),
    ("nl", Flags::Any),
    ("column", Flags::Any),
    ("jq", Flags::Deny(&["-i", "--in-place"])),
    // grep 家族:-r 只读,但某些实现的 --include 配合别的 flag 有坑,
    // 主要拦的是 GNU grep 的 -Z/--null 之类不影响写入的,先只拦明确写的
    ("grep", Flags::Any),
    ("egrep", Flags::Any),
    ("fgrep", Flags::Any),
    ("rg", Flags::Any),
    ("ag", Flags::Any),
    // find 默认只读,直到给它执行类 flag
    (
        "find",
        Flags::Deny(&[
            "-exec", "-execdir", "-ok", "-okdir", "-delete", "-fprintf", "-fprint", "-fls",
        ]),
    ),
    ("fd", Flags::Deny(&["-x", "--exec", "-X", "--exec-batch"])),
    // sed / awk 能写文件
    ("sed", Flags::Deny(&["-i", "--in-place"])),
    ("awk", Flags::Any),
    ("gawk", Flags::Any),
    // git 的只读子命令单独判定
    ("git", Flags::GitSubcommand),
    // docker 同理。它进这张表是因为 `bash::delegation` 把 docker 整族移出了
    // 沙箱 —— 移出之后就要走正常权限流,而 `docker ps` 这种纯查询命令如果
    // 每次都弹窗,只会训练用户无脑点"允许"(见 chain::mode_default 的注释)。
    ("docker", Flags::DockerSubcommand),
    ("podman", Flags::DockerSubcommand),
];

/// git 的只读子命令。
///
/// `[约束]` 白名单而不是黑名单。git 有一百多个子命令,还能通过
/// `git-foo` 可执行文件扩展 —— 黑名单挡不住 `git my-custom-deploy`。
static GIT_READ_ONLY: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "branch",
    "tag",
    "remote",
    "config",
    "blame",
    "describe",
    "rev-parse",
    "rev-list",
    "ls-files",
    "ls-tree",
    "ls-remote",
    "cat-file",
    "shortlog",
    "reflog",
    "whatchanged",
    "grep",
    "count-objects",
    "check-ignore",
    "var",
    "help",
];

/// git 子命令里能写东西的 flag。
///
/// `git config --global x y` 写用户配置,`git branch -d` 删分支。
static GIT_WRITE_FLAGS: &[&str] = &[
    "-d",
    "-D",
    "--delete",
    "--unset",
    "--unset-all",
    "--add",
    "--replace-all",
    "--edit",
    "-e",
    "--set-upstream-to",
    "-m",
    "-M",
    "--move",
    "--prune",
    "--force",
    "-f",
];

/// docker / podman 的只读子命令。
///
/// `[约束]` 和 git 一样是白名单。docker 的子命令能改的东西比文件系统大得多
/// (起容器、删镜像、改 context),黑名单漏一条就是静默放行。
///
/// 故意不收的:
///
/// - `stats` / `events`:不给 flag 就永不返回,和 `tail -f` 同类。
/// - `compose` / `volume` / `network` / `context`:它们的读写之分在**第二层**
///   子命令上(`volume ls` 只读、`volume rm` 不是)。这张表只判第一层,收进来
///   等于把 `volume rm` 也判成只读。
static DOCKER_READ_ONLY: &[&str] = &[
    "version", "info", "ps", "images", "inspect", "logs", "port", "top", "history", "diff",
    "search",
];

/// docker 全局 flag 里会改变"对哪个 daemon 说话"的那些。
///
/// 它们后面跟一个值,不跳过的话 `docker -H tcp://x ps` 的 `tcp://x` 会被当成
/// 子命令。直接放弃只读判定更简单,也更安全 —— 指向别的 daemon 这件事本身
/// 就值得问一次。
static DOCKER_HOST_FLAGS: &[&str] = &["-H", "--host", "-c", "--context", "--config"];

enum Flags {
    /// 任何 flag 都不会让它写东西。
    Any,
    /// 这些 flag 出现就不是只读。
    Deny(&'static [&'static str]),
    /// git 走单独逻辑。
    GitSubcommand,
    /// docker / podman 走单独逻辑。
    DockerSubcommand,
}

/// 判断一组子命令整体是不是只读。
///
/// 任何一条不是只读,整体就不是。
pub fn is_read_only(subs: &[SubCommand]) -> bool {
    !subs.is_empty() && subs.iter().all(sub_is_read_only)
}

pub(crate) fn sub_is_read_only(sub: &SubCommand) -> bool {
    // 未加引号的 glob 或波浪号:执行时展开成什么不知道。
    // `python *` 可能变成 `python evil.py`,`cat ~/*` 可能读到围栏外。
    if sub.has_unquoted_glob {
        return false;
    }

    // 命令前缀的环境变量赋值。危险变量在 AST 层已经拦掉了,
    // 剩下的虽然无害,但"只读"意味着跳过确认,这里从严。
    if !sub.assignments.is_empty() {
        return false;
    }

    let Some((_, flags)) = READ_ONLY.iter().find(|(n, _)| *n == sub.name) else {
        return false;
    };

    match flags {
        Flags::Any => true,
        Flags::Deny(deny) => !sub.args.iter().any(|a| flag_matches(a, deny)),
        Flags::GitSubcommand => git_is_read_only(&sub.args),
        Flags::DockerSubcommand => docker_is_read_only(&sub.args),
    }
}

fn docker_is_read_only(args: &[String]) -> bool {
    let mut it = args.iter();
    let sub = loop {
        let Some(a) = it.next() else {
            // 光一个 `docker`,打印用法,无害
            return true;
        };
        let head = a.split('=').next().unwrap_or(a);
        if DOCKER_HOST_FLAGS.contains(&head) {
            return false;
        }
        if !a.starts_with('-') {
            break a;
        }
    };

    if !DOCKER_READ_ONLY.contains(&sub.as_str()) {
        return false;
    }

    // `docker logs -f` 跟着容器输出跑,永不返回 —— 和 `tail -f` 一样,
    // 判成只读就等于让调度器静默起一个卡死的命令。
    !it.any(|a| flag_matches(a, &["-f", "--follow"]))
}

/// flag 匹配要认 `--flag=value` 形式。
///
/// 只比字符串相等的话,`sed --in-place=.bak` 会被判成只读。
fn flag_matches(arg: &str, deny: &[&str]) -> bool {
    let head = arg.split('=').next().unwrap_or(arg);
    if deny.contains(&head) {
        return true;
    }

    // 合并的短 flag:`sort -no out.txt` 里的 `-o`。
    // 只对单横线开头、不含等号的短 flag 做拆分。
    if head.starts_with('-') && !head.starts_with("--") && head.len() > 2 {
        return head[1..]
            .chars()
            .any(|c| deny.contains(&format!("-{c}").as_str()));
    }

    false
}

fn git_is_read_only(args: &[String]) -> bool {
    // 跳过 git 自己的全局 flag,找子命令。
    //
    // `-c` 和 `--exec-path` 后面跟值,而且 `-c core.pager=evil` 能改行为,
    // 所以遇到就放弃只读判定。
    let mut it = args.iter();
    let sub = loop {
        let Some(a) = it.next() else {
            // 光一个 `git`,列出用法,无害
            return true;
        };
        if a == "-c" || a == "--exec-path" || a.starts_with("--exec-path=") {
            return false;
        }
        if !a.starts_with('-') {
            break a;
        }
    };

    if !GIT_READ_ONLY.contains(&sub.as_str()) {
        return false;
    }

    // `git config --global user.email x` 会写配置文件
    if sub == "config" {
        let rest: Vec<&String> = it.clone().collect();
        // 只有查询形式是只读:`git config x` / `git config --get x`
        let writes = rest.iter().any(|a| {
            let head = a.split('=').next().unwrap_or(a);
            GIT_WRITE_FLAGS.contains(&head)
        });
        // `git config key value` —— 两个非 flag 参数就是写入
        let positional = rest.iter().filter(|a| !a.starts_with('-')).count();
        return !writes && positional <= 1;
    }

    !it.any(|a| {
        let head = a.split('=').next().unwrap_or(a);
        GIT_WRITE_FLAGS.contains(&head)
    })
}
