//! 远程连接的命令分发：命令名 + JSON 参数 → 调 lib.rs 里**同一个**处理函数。
//!
//! `[约束]` 这里不实现任何业务逻辑，只做参数拆包。每一条都直接调 lib.rs 里
//! `#[tauri::command]` 修饰的那个函数（宏保留了原函数，桌面和远程走的是
//! 同一份代码）。要改行为去 lib.rs / state.rs 改，两边自动一致。
//!
//! `[约束]` 这张表和 `generate_handler!`、`build.rs` 的 `COMMANDS` 必须一致，
//! `tests/acl.rs` 盯着。少一条的表现是网页版某个按钮报"未知命令"；多一条
//! 编译不过（调的函数不存在）。
//!
//! 参数名按 Tauri 的约定：Rust 的 `session_id` 在 JSON 里是 `sessionId`。
//! 前端 bridge 已经按这个约定在传，两条线共用同一份调用代码。
//!
//! 少数命令对远程观看者**语义不同**，在这里就地处理并注明理由 —— 那是
//! "这个观看者不在这台机器前"这一事实的直接后果，不是业务逻辑：
//! - `clipboard_paths`：读的是宿主机的剪贴板，和手机上按 ⌘V 的人无关，回空。
//! - `set_appearance`：改的是桌面那扇窗的原生外观，手机上切个主题不该把它
//!   一起换掉，空操作。
//! - 带观看者的六条（订阅、终端、浏览器面板）：把连接号当观看者传进去。

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tauri::ipc::{Channel, IpcResponse};
use tauri::{AppHandle, Manager};

use super::conn::ConnCtx;
use super::protocol::channel_id;
use crate::state::AppState;
use crate::term::Terminals;
use crate::{HostError, HostResult};
use riot_protocol::{UiError, ui_error};

/// 一条命令的结果。
pub enum Outcome {
    Json(Value),
    /// 对应 Tauri 的 `ipc::Response`：原样是字节，不经 JSON。
    Raw(Vec<u8>),
}

/// 参数袋。按名取一个就从袋里拿走，取错类型报一句带参数名的话。
struct Args<'c> {
    map: serde_json::Map<String, Value>,
    ctx: &'c ConnCtx,
}

impl Args<'_> {
    /// 取一个普通参数。缺的当 `null`（`Option<T>` 因此能省略），和 Tauri 一致。
    fn take<T: DeserializeOwned>(&mut self, key: &str) -> Result<T, UiError> {
        let v = self.map.remove(key).unwrap_or(Value::Null);
        serde_json::from_value(v).map_err(|e| ui_error!("host.remote.badParam", name = key; e))
    }

    /// 取一个通道参数：占位串 → 挂在这条连接上的 [`Channel`]。
    fn chan<T>(&mut self, key: &str) -> Result<Channel<T>, UiError> {
        let v = self.map.remove(key).unwrap_or(Value::Null);
        let id = channel_id(&v).ok_or_else(|| ui_error!("host.remote.badParam", name = key))?;
        Ok(self.ctx.channel(id))
    }
}

/// `HostResult<T>` → 结果帧。错误按 [`HostError::to_ui`]，和桌面那边
/// `serde::Serialize for HostError` 给前端的是同一份结构。
fn ok<T: Serialize>(r: HostResult<T>) -> Result<Outcome, UiError> {
    match r {
        Ok(v) => val(v),
        Err(e) => Err(e.to_ui()),
    }
}

/// 不会失败的返回值。
fn val<T: Serialize>(v: T) -> Result<Outcome, UiError> {
    serde_json::to_value(v)
        .map(Outcome::Json)
        .map_err(|e| ui_error!("host.remote.encode"; e))
}

/// 分发一条命令。
pub async fn dispatch(
    app: &AppHandle,
    ctx: &ConnCtx,
    cmd: &str,
    args: Value,
) -> Result<Outcome, UiError> {
    let map = match args {
        Value::Object(m) => m,
        Value::Null => serde_json::Map::new(),
        _ => return Err(ui_error!("host.remote.badParam", name = "args")),
    };
    let mut a = Args { map, ctx };
    let st = app.state::<AppState>();
    let terms = app.state::<Terminals>();
    let viewer = ctx.viewer.as_str();

    match cmd {
        // ── 会话事件与轮次 ──
        "subscribe_session" => ok(crate::subscribe_session_as(
            &st,
            viewer,
            a.take("sessionId")?,
            a.take("epoch")?,
            a.chan("onEvent")?,
        )
        .await),
        "send_turn" => ok(crate::send_turn(
            st,
            a.take("sessionId")?,
            a.take("text")?,
            a.take("images")?,
            a.take("refs")?,
            a.take("nudge")?,
        )
        .await),
        "regenerate_turn" => {
            ok(crate::regenerate_turn(st, a.take("sessionId")?, a.take("messageId")?).await)
        }
        "resend_turn" => ok(crate::resend_turn(
            st,
            a.take("sessionId")?,
            a.take("messageId")?,
            a.take("text")?,
        )
        .await),
        "edit_message" => ok(crate::edit_message(
            st,
            a.take("sessionId")?,
            a.take("messageId")?,
            a.take("text")?,
        )
        .await),
        "delete_message" => {
            ok(crate::delete_message(st, a.take("sessionId")?, a.take("messageId")?).await)
        }
        "queue_list" => ok(crate::queue_list(st, a.take("sessionId")?).await),
        "queue_remove" => {
            ok(crate::queue_remove(st, a.take("sessionId")?, a.take("entryId")?).await)
        }
        "queue_take" => ok(crate::queue_take(st, a.take("sessionId")?, a.take("entryId")?).await),
        "task_cancel" => ok(crate::task_cancel(st, a.take("sessionId")?, a.take("agentId")?).await),
        "task_history" => {
            ok(crate::task_history(st, a.take("sessionId")?, a.take("agentId")?).await)
        }
        "session_compact" => ok(crate::session_compact(st, a.take("sessionId")?).await),
        "interrupt" => ok(crate::interrupt(st, a.take("sessionId")?).await),
        "turn_nudge" => ok(crate::turn_nudge(st, a.take("sessionId")?, a.take("nudge")?).await),
        "respond_permission" => ok(crate::respond_permission(
            st,
            a.take("sessionId")?,
            a.take("askId")?,
            a.take("response")?,
        )
        .await),

        // ── 斜杠命令 / 文件 ──
        "slash_commands" => ok(crate::slash_commands(a.take("root")?).await),
        "slash_expand" => {
            ok(
                crate::slash_expand(st, a.take("sessionId")?, a.take("name")?, a.take("args")?)
                    .await,
            )
        }
        "hooks_list" => ok(crate::hooks_list(a.take("root")?).await),
        "skills_list" => ok(crate::skills_list(a.take("root")?).await),
        "search_files" => {
            ok(
                crate::search_files(st, a.take("sessionId")?, a.take("query")?, a.take("limit")?)
                    .await,
            )
        }
        "list_dir" => ok(crate::list_dir(st, a.take("sessionId")?, a.take("rel")?).await),
        "browse_dirs" => ok(crate::browse_dirs(a.take("path")?).await),
        "read_image" => ok(crate::read_image(st, a.take("path")?).await),
        "read_file_bytes" => match crate::read_file_bytes(st, a.take("path")?).await {
            Ok(resp) => match resp.body() {
                Ok(tauri::ipc::InvokeResponseBody::Raw(bytes)) => Ok(Outcome::Raw(bytes)),
                Ok(tauri::ipc::InvokeResponseBody::Json(s)) => serde_json::from_str(&s)
                    .map(Outcome::Json)
                    .map_err(|e| ui_error!("host.remote.encode"; e)),
                Err(e) => Err(ui_error!("host.remote.encode"; e)),
            },
            Err(e) => Err(e.to_ui()),
        },
        "probe_dirs" => val(crate::probe_dirs(a.take("paths")?)),
        "path_exists" => val(crate::path_exists(a.take("path")?)),
        // 宿主机的剪贴板和远端用户无关（见模块说明）。
        "clipboard_paths" => val(Vec::<String>::new()),

        // ── 定时任务 ──
        "schedule_list" => ok(crate::schedule_list(st).await),
        "schedule_create" => ok(crate::schedule_create(st, a.take("draft")?).await),
        "schedule_set_enabled" => {
            ok(crate::schedule_set_enabled(st, a.take("id")?, a.take("enabled")?).await)
        }
        "schedule_update" => ok(crate::schedule_update(st, a.take("id")?, a.take("patch")?).await),
        "schedule_delete" => ok(crate::schedule_delete(st, a.take("id")?).await),
        "schedule_run_now" => ok(crate::schedule_run_now(st, a.take("id")?).await),
        "schedule_missed" => ok(crate::schedule_missed(st).await),
        "schedule_ack_missed" => ok(crate::schedule_ack_missed(st).await),

        // ── 会话与项目 ──
        "session_changes" => ok(crate::session_changes(st, a.take("sessionId")?).await),
        "session_git_changes" => {
            ok(crate::session_git_changes(st, a.take("sessionId")?, a.take("base")?).await)
        }
        "set_permission_mode" => {
            ok(crate::set_permission_mode(st, a.take("sessionId")?, a.take("mode")?).await)
        }
        "set_session_sampling" => {
            ok(crate::set_session_sampling(st, a.take("sessionId")?, a.take("sampling")?).await)
        }
        "detect_venvs" => ok(crate::detect_venvs(st, a.take("sessionId")?).await),
        "set_session_python_venv" => {
            ok(crate::set_session_python_venv(st, a.take("sessionId")?, a.take("path")?).await)
        }
        "set_session_system_prompt" => {
            ok(crate::set_session_system_prompt(st, a.take("sessionId")?, a.take("prompt")?).await)
        }
        "set_session_thinking" => {
            ok(crate::set_session_thinking(st, a.take("sessionId")?, a.take("thinking")?).await)
        }
        "set_session_multitask" => {
            ok(crate::set_session_multitask(st, a.take("sessionId")?, a.take("on")?).await)
        }
        "add_project" => ok(crate::add_project(st, a.take("path")?).await),
        "create_session" => ok(crate::create_session(st, a.take("root")?).await),
        "list_sessions" => ok(crate::list_sessions(st).await),
        "get_history" => ok(crate::get_history(st, a.take("sessionId")?).await),
        "delete_session" => ok(crate::delete_session(st, a.take("sessionId")?).await),
        "rename_session" => {
            ok(crate::rename_session(st, a.take("sessionId")?, a.take("title")?).await)
        }
        "remove_project" => ok(crate::remove_project(st, a.take("root")?).await),

        // ── 浏览器面板 ──
        "browser_open" => ok(st
            .browser_open_for(viewer, &a.take::<String>("sessionId")?, a.chan("onFrame")?)
            .await),
        "browser_close" => ok(st
            .browser_close_for(viewer, &a.take::<String>("sessionId")?)
            .await),
        "browser_watch_tabs" => ok(st
            .browser_watch_tabs_for(viewer, &a.take::<String>("sessionId")?, a.chan("onChange")?)
            .await),
        "browser_navigate" => {
            ok(crate::browser_navigate(st, a.take("sessionId")?, a.take("url")?).await)
        }
        "browser_history" => {
            ok(crate::browser_history(st, a.take("sessionId")?, a.take("delta")?).await)
        }
        "browser_reload" => ok(crate::browser_reload(st, a.take("sessionId")?).await),
        "browser_state" => ok(crate::browser_state(st, a.take("sessionId")?).await),
        "browser_new_tab" => ok(crate::browser_new_tab(st, a.take("sessionId")?).await),
        "browser_close_tab" => {
            ok(crate::browser_close_tab(st, a.take("sessionId")?, a.take("tab")?).await)
        }
        "browser_select_tab" => {
            ok(crate::browser_select_tab(st, a.take("sessionId")?, a.take("tab")?).await)
        }
        "browser_resize" => ok(crate::browser_resize(
            st,
            a.take("sessionId")?,
            a.take("width")?,
            a.take("height")?,
            a.take("scale")?,
            a.take("viewWidth")?,
            a.take("viewHeight")?,
        )
        .await),
        "browser_input" => {
            ok(crate::browser_input(st, a.take("sessionId")?, a.take("input")?).await)
        }
        "browser_selection" => ok(crate::browser_selection(st, a.take("sessionId")?).await),
        "browser_pick" => {
            ok(crate::browser_pick(st, a.take("sessionId")?, a.take("x")?, a.take("y")?).await)
        }
        "browser_pick_hover" => {
            ok(
                crate::browser_pick_hover(st, a.take("sessionId")?, a.take("x")?, a.take("y")?)
                    .await,
            )
        }
        "browser_pick_clear" => ok(crate::browser_pick_clear(st, a.take("sessionId")?).await),
        "browser_scope_list" => ok(crate::browser_scope_list(st, a.take("sessionId")?).await),
        "browser_scope_revoke" => {
            ok(crate::browser_scope_revoke(st, a.take("sessionId")?, a.take("host")?).await)
        }

        // ── 终端面板 ──
        "term_open" => ok(terms
            .open(
                a.take("root")?,
                a.take("cols")?,
                a.take("rows")?,
                viewer,
                a.chan("onEvent")?,
            )
            .map_err(HostError::Ui)),
        "term_attach" => ok(terms
            .attach(viewer, a.take("id")?, a.chan("onEvent")?)
            .map_err(HostError::Ui)),
        "term_write" => ok(crate::term_write(terms, a.take("id")?, a.take("data")?).await),
        "term_resize" => {
            ok(crate::term_resize(terms, a.take("id")?, a.take("cols")?, a.take("rows")?).await)
        }
        "term_close" => ok(crate::term_close(terms, a.take("id")?).await),
        "term_list" => ok(crate::term_list(terms).await),
        "term_share" => ok(crate::term_share(terms, a.take("id")?, a.take("shared")?).await),
        "term_busy" => ok(crate::term_busy(terms, a.take("id")?).await),

        // ── 配置与设置页 ──
        "get_config" => ok(crate::get_config(st).await),
        "set_config" => ok(crate::set_config(app.clone(), st, a.take("config")?).await),
        "set_api_key" => ok(crate::set_api_key(st, a.take("providerId")?, a.take("key")?).await),
        "app_version" => val(crate::app_version(app.clone())),
        // 桌面窗口的外观和远端用户无关（见模块说明）。网页那头的 transport
        // 本来就不会发这条，这里兜住的是手写的调用。
        "set_appearance" => val(()),
        "check_update" => ok(crate::check_update(app.clone()).await),
        "mcp_status" => ok(crate::mcp_status(st).await),
        "mcp_restart" => ok(crate::mcp_restart(st, a.take("serverId")?).await),
        "mcp_export_json" => ok(crate::mcp_export_json(st).await),
        "mcp_import_json" => ok(crate::mcp_import_json(st, a.take("raw")?).await),
        "sandbox_status" => val(crate::sandbox_status().await),
        "sandbox_install" => ok(crate::sandbox_install().await),
        "sandbox_uninstall" => ok(crate::sandbox_uninstall().await),
        "packs_status" => ok(crate::packs_status().await),
        "packs_install" => ok(crate::packs_install(st, a.take("id")?, a.chan("onProgress")?).await),
        "packs_uninstall" => ok(crate::packs_uninstall(st, a.take("id")?).await),
        "test_connection" => {
            ok(crate::test_connection(st, a.take("providerId")?, a.take("model")?).await)
        }
        "test_search_backend" => ok(crate::test_search_backend(a.take("baseUrl")?).await),
        "list_models" => ok(crate::list_models(st, a.take("providerId")?).await),
        "remote_status" => ok(crate::remote_status(app.clone()).await),
        "remote_rotate_token" => ok(crate::remote_rotate_token(app.clone()).await),

        other => Err(ui_error!("host.remote.unknownCommand", command = other)),
    }
}
