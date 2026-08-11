//! 三份命令清单必须一致。
//!
//! 一个命令要能被前端调用，得同时出现在三个地方：
//!
//! 1. `lib.rs` 的 `generate_handler!` —— 注册处理函数；
//! 2. `build.rs` 的 `COMMANDS` —— 声明它存在，供 ACL 生成权限；
//! 3. `capabilities/default.json` —— 授予主窗口调用它的权限。
//!
//! 漏掉任何一个，**编译都不会报错**，运行时报
//! `<name> not allowed. Command not found`。
//!
//! 这个错误真实发生过一次，而且藏了很久：前端当时写的是 `void invoke(...)`，
//! 没有 catch，于是所有调用静默失败，界面只是一直转圈。等到加了错误处理
//! 才发现五个命令从头到尾就没通过。
//!
//! 所以这里用文本解析而不是什么优雅的方案 —— 优雅不是重点，"改了一处
//! 忘了另外两处时立刻红灯"才是。

use std::collections::BTreeSet;
use std::path::Path;

fn read(rel: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    // 测试读自己仓库里的源文件，不是生产 I/O —— disallowed_methods 管不到这里
    #[allow(clippy::disallowed_methods)]
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("读不到 {}：{e}", p.display()))
}

/// `build.rs` 里 `COMMANDS` 常量的内容。
fn declared() -> BTreeSet<String> {
    let src = read("build.rs");
    let body = src
        .split_once("const COMMANDS: &[&str] = &[")
        .expect("build.rs 里找不到 COMMANDS 常量")
        .1
        .split_once("];")
        .expect("COMMANDS 没有结束括号")
        .0;

    body.split(',')
        .filter_map(|s| {
            let t = s.trim().trim_matches('"').trim();
            (!t.is_empty()).then(|| t.to_owned())
        })
        .collect()
}

/// `generate_handler!` 里注册的命令。
fn registered() -> BTreeSet<String> {
    let src = read("src/lib.rs");
    let body = src
        .split_once("tauri::generate_handler![")
        .expect("lib.rs 里找不到 generate_handler")
        .1
        .split_once("])")
        .expect("generate_handler 没有结束括号")
        .0;

    body.split(',')
        .filter_map(|s| {
            let t = s.trim();
            (!t.is_empty() && !t.starts_with("//")).then(|| t.to_owned())
        })
        .collect()
}

/// capability 里放行的自定义命令（`allow-*`，不带插件前缀的那些）。
fn allowed() -> BTreeSet<String> {
    let raw = read("capabilities/default.json");
    let json: serde_json::Value = serde_json::from_str(&raw).expect("capability 不是合法 JSON");

    json["permissions"]
        .as_array()
        .expect("permissions 必须是数组")
        .iter()
        .filter_map(|v| v.as_str())
        // 带 `:` 的是插件/core 权限，不在这里管
        .filter(|s| !s.contains(':'))
        .filter_map(|s| s.strip_prefix("allow-"))
        .map(|s| s.replace('-', "_"))
        .collect()
}

#[test]
fn 注册的命令都在_build_rs_里声明了() {
    let missing: Vec<_> = registered().difference(&declared()).cloned().collect();
    assert!(
        missing.is_empty(),
        "这些命令注册了但没在 build.rs 的 COMMANDS 里声明，ACL 会拒绝它们：{missing:?}"
    );
}

#[test]
fn 声明的命令都被_capability_放行了() {
    let missing: Vec<_> = declared().difference(&allowed()).cloned().collect();
    assert!(
        missing.is_empty(),
        "这些命令没在 capabilities/default.json 里放行，前端调用会报 \
         `not allowed. Command not found`：{missing:?}"
    );
}

#[test]
fn 放行的权限都对应真实存在的命令() {
    // 反向检查。命令改名之后留下的孤儿权限不会报错，但会让人误以为
    // 某个能力还开着 —— 对着一份不准的权限清单做安全判断比没有更糟。
    let orphan: Vec<_> = allowed().difference(&declared()).cloned().collect();
    assert!(
        orphan.is_empty(),
        "capability 里放行了不存在的命令：{orphan:?}"
    );
}

#[test]
fn 声明的命令都真的注册了处理函数() {
    let ghost: Vec<_> = declared().difference(&registered()).cloned().collect();
    assert!(
        ghost.is_empty(),
        "这些命令声明并放行了，却没有处理函数：{ghost:?}"
    );
}

#[test]
fn 清单不是空的() {
    // 解析逻辑靠文本匹配，重构一旦改动了它依赖的字面量，上面四个测试会
    // 全部变成"两个空集合相等"—— 绿灯，但什么都没检查。
    assert!(!declared().is_empty(), "没解析到 build.rs 的 COMMANDS");
    assert!(!registered().is_empty(), "没解析到 generate_handler 的内容");
    assert!(!allowed().is_empty(), "没解析到 capability 的 allow-*");
}
