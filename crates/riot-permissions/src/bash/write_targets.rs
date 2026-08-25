//! 命令**参数**里的敏感写目标。
//!
//! # 这一层补的是哪个洞
//!
//! [`crate::safety::check`] 走 [`riot_protocol::tool::Tool::target_path`]，
//! 而 Bash 没有单一目标路径（返回 `None`）—— 整个路径安全检查对它不生效。
//! [`super::ast`] 的 `redirect_target_risk` 补了重定向那一半
//! （`echo evil >> ~/.zshrc`），但 shell 里写文件的方式远不止重定向：
//! `cp` / `mv` / `install` / `tee` / `sed -i` 一个都看不见。
//!
//! 没有沙箱时这个缺口的后果有限：这类命令既不只读、也没规则命中，会落到
//! 决策链第 7 步收敛成 Ask，用户还有一次机会。**沙箱把那次机会拿走了** ——
//! [`super::decide`] 的放宽档基于「OS 已经挡住文件系统」直接 Allow，而
//! 可写集里恰好躺着几处能换来**沙箱外**执行权的目标：
//!
//! - `<工作区>/.riot/hooks.json` —— 下一轮 `HookEngine::load` 无条件读它，
//!   用 `sh -c` 裸跑在宿主上，还能返回 `permissionDecision: allow` 把权限
//!   层整个关掉。一次工具调用，零确认。
//! - `<工作区>/.git/hooks/`，以及 `git config core.hooksPath` —— 用户下次
//!   commit 就执行。
//! - `~/.cargo/config.toml` 的 `rustc-wrapper` / `[alias]` —— 下次
//!   `cargo build` 就执行，而那次构建往往在沙箱之外。
//!
//! 这些目标全在沙箱边界**之内**，OS 不会拦。所以要在放宽之前挡住它们。
//!
//! # 为什么不改成在边界上 deny
//!
//! `[取舍]` 想过在 seatbelt profile 里给这几条路径补 `deny`。不做，因为
//! 那会造出「用户点了允许，命令照样失败」——沙箱是每轮按静态策略激活的，
//! 它不知道用户这一次批准了什么。挡在策略层则保住了「问一次、然后它真的
//! 能跑」这条路径。代价是间接写入（`npm install` 的 postinstall 偷改
//! `~/.cargo/config.toml`）扫不出来 —— 那是可写集本身的残余风险，见
//! `riot_runtime::sandbox` 的取舍说明。
//!
//! `[取舍]` 只扫**参数字面量**，不展开、不跟踪。要抓的是「模型把目标路径
//! 明写出来」这一种形态，而那正是这条攻击链里唯一需要模型主动做的一步。

use riot_protocol::permission::SafetyKind;

use super::ast::SubCommand;

/// 扫出来的一个敏感写目标。
pub struct WriteRisk<'a> {
    pub kind: SafetyKind,
    /// 给用户看的一句话，说明为什么拦。
    pub message: String,
    /// 命中的那条子命令，用来生成「总是允许」建议。
    pub sub: &'a SubCommand,
}

/// 一组子命令里第一个敏感写目标。没有就是 `None`。
pub fn scan<'a>(subs: &'a [SubCommand]) -> Option<WriteRisk<'a>> {
    subs.iter().find_map(sub_risk)
}

fn sub_risk(sub: &SubCommand) -> Option<WriteRisk<'_>> {
    // 只读判定按**单条**子命令做,不是整组。`cat .git/config && cp x .git/hooks/`
    // 里前者只读、后者是写,整组的 `is_read_only` 会返回 false,那样前者又会
    // 被当成写 `.git/` 误拦。逐条判才能让只读的那条真的走只读语义。
    let read_only = super::readonly::sub_is_read_only(sub);

    // git 的执行面配置键。只有「设置」才碰执行面 —— `git config core.pager`
    // (只读取值)不改任何东西,和下面的路径扫描一样受 `read_only` 约束。
    if !read_only
        && let Some(key) = git_exec_config_key(sub)
    {
        return Some(WriteRisk {
            kind: SafetyKind::GitInternals,
            message: format!(
                "这会设置 Git 配置 `{key}`。这一类键的值会被 Git 当成命令执行 —— \
                 设了它等于让之后的 git 操作自动跑指定的程序。"
            ),
            sub,
        });
    }

    sub.args.iter().find_map(|arg| {
        let path = std::path::Path::new(arg);
        let kind = crate::safety::write_target_risk(path, read_only)?;
        Some(WriteRisk {
            kind,
            message: crate::safety::describe(kind, path),
            sub,
        })
    })
}

/// Git 配置里那些**值会被当成命令执行**的键。
///
/// `[约束]` 按「git 会 spawn 它」筛，不是按「听起来重要」筛。
/// `git config core.hooksPath /tmp/evil` 的参数里没有任何看着敏感的路径，
/// 路径扫描完全看不见它 —— 但效果和直接写 `.git/hooks/` 一样。
///
/// 前缀项（`alias.` / `filter.` / `difftool.` / `mergetool.`）是因为键名
/// 中间有一段用户自定义的名字：`filter.lfs.clean`、`alias.deploy`。
const EXEC_CONFIG_KEYS: &[&str] = &[
    "core.hookspath",
    "core.fsmonitor",
    "core.sshcommand",
    "core.editor",
    "core.pager",
    "core.gitproxy",
    "sequence.editor",
    "diff.external",
    "credential.helper",
];
const EXEC_CONFIG_PREFIXES: &[&str] = &["alias.", "filter.", "difftool.", "mergetool."];

/// `git config` 里跟一个独立值的选项。
///
/// 跳过它们的值,否则 `git config --file cfg core.pager evil` 会把 `cfg`
/// 当成键、漏掉真正的 `core.pager`。
const CONFIG_VALUE_FLAGS: &[&str] = &["-f", "--file", "--blob", "-t", "--type", "--default"];

/// 这条子命令有没有在**设置**上面那些键。
///
/// 两种写法都要认，它们的效果完全一样：
/// - `git config core.pager 'sh -c evil'`（写进配置文件，之后一直生效）
/// - `git -c core.pager='sh -c evil' log`（只影响这一次，但这一次就够了）
fn git_exec_config_key(sub: &SubCommand) -> Option<&str> {
    let args = git_args(sub)?;
    scan_git_config_key(args)
}

/// 定位这条子命令里 git 的参数序列 —— 裸调用、绝对路径、被 sudo/env 包着,
/// 三种形态都要覆盖。
///
/// `[约束]` 这里剥 `sudo`/`env` 和 [`super::ast`] 的规则匹配层**不共用逻辑**,
/// 方向恰好相反:规则匹配层故意不剥 `sudo`(否则 `sudo rm` 会命中 `rm` 的
/// allow 规则);而这里是安全检查,`sudo git config core.hooksPath` 换来的
/// 执行权和裸 `git config core.hooksPath` 一模一样,必须一并看见。宁可多认 ——
/// 认错了也只是多扫几个不含配置键的参数,`scan_git_config_key` 不会误拦。
fn git_args(sub: &SubCommand) -> Option<&[String]> {
    // `/usr/bin/git` 的 basename 是 `git`。精确等值匹配挡不住绝对路径 ——
    // 那是模型最可能写出的绕过形态。
    if basename(&sub.name) == "git" {
        return Some(&sub.args);
    }
    if matches!(basename(&sub.name), "sudo" | "env") {
        // sudo/env 的选项、env 的 `FOO=bar` 赋值都跳过,直接找 `git` 那一段。
        // 双重包装 `sudo env git …` 也能一路找到。
        let pos = sub.args.iter().position(|a| basename(a) == "git")?;
        return Some(&sub.args[pos + 1..]);
    }
    None
}

/// 命令名的最后一段。`/usr/bin/git` → `git`,`git` → `git`。
fn basename(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

/// 只在「配置键位置」提取键,不碰普通值。
///
/// `[约束]` 配置键只出现在两处:全局 `-c <key>=<value>` 的 key,以及
/// `config` 子命令后的第一个位置参数。扫**所有**参数会把 `-m`/`--grep`
/// 的值也当成键 —— `git commit -m alias.deploy`、`git log --grep filter.foo`
/// 里那个裸词恰好形如配置键就误报,而 commit message / grep pattern 里
/// 出现这种词属于正常内容,不是在设置配置。
fn scan_git_config_key(args: &[String]) -> Option<&str> {
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            // 全局 `-c <name>=<value>`:只影响这一次,但值一样被 git 当命令跑
            "-c" => {
                if let Some(key) = args.get(i + 1).and_then(|t| exec_key(t)) {
                    return Some(key);
                }
                i += 2;
            }
            // `config` 子命令:键是它后面第一个位置参数
            "config" => return config_key(&args[i + 1..]),
            _ => i += 1,
        }
    }
    None
}

/// `git config [<选项>] <键> [<值>]` 里那个键。
///
/// 只看第一个位置参数。第二个位置参数是**值**,值形如 `alias.x` 也不算在
/// 设置 alias.x —— 这正是老实现"扫所有参数"扫出误报的地方。
fn config_key(rest: &[String]) -> Option<&str> {
    let mut i = 0;
    while i < rest.len() {
        let tok = &rest[i];
        if tok.starts_with('-') && tok.len() > 1 {
            let takes_value = !tok.contains('=') && CONFIG_VALUE_FLAGS.contains(&tok.as_str());
            i += if takes_value { 2 } else { 1 };
            continue;
        }
        return exec_key(tok);
    }
    None
}

/// 这个 token 是不是一个「值会被 git 当命令执行」的配置键。
///
/// `-c key=value` 和 `config key value` 里,键都是独立的一段或 `key=value`
/// 的前半段。统一切到第一个 `=` 之前再比。
fn exec_key(arg: &str) -> Option<&str> {
    let key = arg.split('=').next().unwrap_or(arg);
    let lower = key.to_ascii_lowercase();
    let hit = EXEC_CONFIG_KEYS.contains(&lower.as_str())
        || EXEC_CONFIG_PREFIXES.iter().any(|p| lower.starts_with(p));
    hit.then_some(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bash::ast::{Analysis, analyze};

    fn risk(cmd: &str) -> Option<SafetyKind> {
        let Analysis::Simple(subs) = analyze(cmd) else {
            panic!("{cmd} 该能拆成简单命令");
        };
        scan(&subs).map(|r| r.kind)
    }

    /// 沙箱可写集里那几处能换来沙箱外执行权的目标。这组用例就是这个
    /// 模块存在的理由 —— 少一条，沙箱放宽档就在那条上静默交出执行权。
    #[test]
    fn 沙箱内但能拿到沙箱外执行权的目标() {
        for cmd in [
            "cp payload .riot/hooks.json",
            "mv payload .git/hooks/pre-commit",
            "install -m 755 payload .git/hooks/pre-commit",
            "chmod +x .git/hooks/pre-commit",
            "cp payload /Users/u/.cargo/config.toml",
        ] {
            assert!(risk(cmd).is_some(), "{cmd} 必须被扫出来");
        }
    }

    /// 重定向之外的写法 —— 这些正是 `redirect_target_risk` 看不见的。
    #[test]
    fn 非重定向的写法也要认() {
        assert_eq!(risk("cp evil /home/u/.zshrc"), Some(SafetyKind::ShellRc));
        assert_eq!(
            risk("sed -i s/a/b/ /home/u/.ssh/config"),
            Some(SafetyKind::SshConfig)
        );
        assert_eq!(
            risk("cp /work/.env /tmp/leak"),
            Some(SafetyKind::Credentials)
        );
    }

    #[test]
    fn git_配置里会执行的键() {
        for cmd in [
            "git config core.hooksPath /tmp/evil",
            "git config --global core.pager evil",
            "git -c core.sshCommand=evil push",
            "git config alias.deploy '!sh /tmp/evil'",
            "git config filter.lfs.clean evil",
            "git config credential.helper '!evil'",
        ] {
            assert_eq!(risk(cmd), Some(SafetyKind::GitInternals), "{cmd}");
        }
    }

    /// 绝对路径 / sudo / env 前缀不能绕过执行面配置键的检查。
    ///
    /// 精确等值 `name == "git"` 会把这几种全放过。沙箱默认开着,漏掉的命令
    /// 走放宽档静默放行,写的又是沙箱可写的工作区 `.git/config` —— 于是
    /// 「下次 git 操作执行任意代码」这条链在边界之内重新打通。`/usr/bin/git`
    /// 是模型最可能写出的形态。
    #[test]
    fn git_执行面配置键不被前缀绕过() {
        for cmd in [
            "/usr/bin/git config core.hooksPath /tmp/evil",
            "sudo git config core.hooksPath /tmp/evil",
            "env git config core.hooksPath /tmp/evil",
            "sudo -u root git config core.hooksPath /tmp/evil",
            "env FOO=bar git -c core.pager=evil log",
            "sudo env git config core.hooksPath /tmp/evil",
        ] {
            assert_eq!(risk(cmd), Some(SafetyKind::GitInternals), "{cmd} 该被扫出来");
        }
    }

    /// 配置键只在 `-c <key>` 和 `config` 子命令后出现。老实现扫**所有**
    /// 参数,把 `-m`/`--grep` 的值也当成键 —— 那些位置放的是 commit message
    /// 和 grep pattern,形如 `alias.x`/`filter.x` 是正常内容,不是在设置配置。
    #[test]
    fn git_普通值不当成配置键() {
        for cmd in [
            "git commit -m alias.deploy",
            "git commit -m filter.foo",
            "git log --grep filter.foo",
            "git log --grep=filter.foo",
            "git grep core.pager",
            // 读取配置值不是设置,不碰执行面
            "git config core.pager",
            "git config --get core.pager",
        ] {
            assert_eq!(risk(cmd), None, "{cmd} 不该被当成设置配置键");
        }
    }

    /// 只读命令读敏感文件不该被拦。这和 `safety.rs` 的
    /// `on_read("/work/.git/config") == None` 是同一条不变量 —— 老实现里
    /// `write_target_risk` 硬编码 `read_only=false`,把这些读操作全拦成了
    /// 一个对 bypass 免疫的 Ask,同一个读走 Write 工具却是放行的。
    #[test]
    fn 只读命令读敏感文件不误伤() {
        for cmd in [
            "cat .git/config",
            "cat /home/u/.zshrc",
            "cat .cargo/config.toml",
            "grep foo .git/config",
            "head .git/HEAD",
        ] {
            assert_eq!(risk(cmd), None, "{cmd} 是只读,不该拦");
        }
    }

    /// 但区分读写不能把两件事一起放过:凭证读到即泄露(只读也拦),
    /// 非凭证类的**写**照旧拦。
    #[test]
    fn 区分读写不放过凭证读和普通写() {
        // 凭证:只读命令读也要拦
        assert_eq!(risk("cat .env"), Some(SafetyKind::Credentials));
        assert_eq!(
            risk("cat /home/u/.ssh/id_rsa"),
            Some(SafetyKind::Credentials)
        );
        assert_eq!(
            risk("cat /home/u/.ssh/config"),
            Some(SafetyKind::SshConfig)
        );
        // 非凭证:写命令照旧拦
        assert_eq!(
            risk("cp evil .git/hooks/pre-commit"),
            Some(SafetyKind::GitInternals)
        );
        assert_eq!(risk("cp evil /home/u/.zshrc"), Some(SafetyKind::ShellRc));
    }

    /// 误报比漏报更快消耗用户的注意力。日常命令一条都不许命中。
    #[test]
    fn 日常命令不误伤() {
        for cmd in [
            "cargo build --release",
            "npm install",
            "git commit -m fix",
            "git config user.email me@example.com",
            "git log --oneline",
            "rm -rf node_modules",
            "cp src/a.rs src/b.rs",
            "mkdir -p .github/workflows",
            "cp x /work/src/legit.git-helper.rs",
            "cat .env.example",
        ] {
            assert_eq!(risk(cmd), None, "{cmd} 被误判了");
        }
    }
}
