/// 暴露给前端的命令。
///
/// `[约束]` 这里列出的每一个都必须同时出现在 `capabilities/default.json`
/// 的 `permissions` 里（形式是 `allow-<kebab-case>`），否则前端调用会被
/// ACL 拒绝，报 `<name> not allowed. Command not found`。
///
/// 两处都要改是刻意的：声明"存在"和授予"可用"是两件事。代价是加命令时
/// 容易漏掉一处 —— 所以 `src-tauri/tests/acl.rs` 盯着这两份清单是否一致。
const COMMANDS: &[&str] = &[
    "subscribe_session",
    "send_turn",
    "interrupt",
    "respond_permission",
    "set_permission_mode",
    "get_config",
    "set_config",
    "set_api_key",
    "add_project",
    "create_session",
    "list_sessions",
    "get_history",
    "delete_session",
    "rename_session",
    "remove_project",
    "test_connection",
    "test_search_backend",
    "list_models",
    "set_session_sampling",
    "browser_open",
    "browser_close",
    "browser_navigate",
    "browser_input",
];

fn main() {
    // 显式声明命令白名单。
    //
    // [约束] 不这么做的话，注册在 invoke_handler 里的自定义命令**默认对所有
    // window/webview 开放**，不受 capability 约束。对一个能执行任意命令的
    // agent 应用，这意味着将来加一个 OAuth window 或 devtools window，
    // 它自动拥有全部命令权限。
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("tauri-build 失败");
}
