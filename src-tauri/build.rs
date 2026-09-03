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
    "regenerate_turn",
    "edit_message",
    "delete_message",
    "interrupt",
    "respond_permission",
    "set_permission_mode",
    "get_config",
    "app_version",
    "check_update",
    "set_config",
    "set_api_key",
    "add_project",
    "create_session",
    "probe_dirs",
    "path_exists",
    "list_sessions",
    "get_history",
    "delete_session",
    "rename_session",
    "remove_project",
    "test_connection",
    "test_search_backend",
    "list_models",
    "set_session_sampling",
    "detect_venvs",
    "set_session_python_venv",
    "set_session_system_prompt",
    "set_session_thinking",
    "browser_open",
    "browser_close",
    "browser_navigate",
    "browser_history",
    "browser_reload",
    "browser_state",
    "browser_new_tab",
    "browser_close_tab",
    "browser_select_tab",
    "browser_watch_tabs",
    "browser_resize",
    "browser_input",
    "browser_pick",
    "browser_pick_hover",
    "browser_pick_clear",
    "browser_scope_list",
    "browser_scope_revoke",
    "term_open",
    "term_write",
    "term_resize",
    "term_close",
    "term_list",
    "term_attach",
    "term_share",
    "term_busy",
    "read_image",
    "read_file_bytes",
    "clipboard_paths",
    "mcp_status",
    "mcp_restart",
    "mcp_export_json",
    "mcp_import_json",
    "skills_list",
    "sandbox_status",
    "sandbox_install",
    "sandbox_uninstall",
    "packs_status",
    "packs_install",
    "packs_uninstall",
    "queue_list",
    "queue_remove",
    "queue_take",
    "task_cancel",
    "task_history",
    "session_compact",
    "session_changes",
    "session_git_changes",
    "slash_commands",
    "slash_expand",
    "hooks_list",
    "search_files",
    "list_dir",
    "schedule_list",
    "schedule_set_enabled",
    "schedule_update",
    "schedule_delete",
    "schedule_run_now",
    "schedule_missed",
    "schedule_ack_missed",
];

fn main() {
    // Windows 上延迟加载 comctl32，否则测试二进制根本启动不了。
    //
    // [约束] tauri 栈（tauri-runtime-wry 的对话框、muda）导入 comctl32.dll
    // 的 TaskDialogIndirect。该符号只在 Common-Controls v6 有，而加载器只对
    // manifest 里声明了 v6 依赖的二进制解析 v6：正式 app 的 manifest 由下面
    // 的 tauri-build 嵌入，没事；但 cargo 不给测试二进制任何 manifest ——
    // 加载器退回 v5，进程死于 STATUS_ENTRYPOINT_NOT_FOUND(0xc0000139)，
    // 一个测试都跑不到。/DELAYLOAD 把解析推迟到首次调用：测试不碰 UI，
    // 永远不会解析；app 真弹对话框时 manifest 早已激活 v6，行为不变。
    //
    // [约束] 必须用无后缀的 rustc-link-arg：rustc-link-arg-tests 只覆盖
    // tests/ 下的集成测试，摸不到 lib 单元测试二进制（cargo#10937），
    // 而挂的恰恰是它。MSVC 专属语法，按 target 门控，别的平台一个字不发。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg=/DELAYLOAD:comctl32.dll");
        println!("cargo:rustc-link-arg=delayimp.lib");
    }

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
