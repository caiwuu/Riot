//! Tauri 宿主。
//!
//! 职责边界：窗口、内核进程监管、PTY、文件系统访问、密钥。
//! **不包含任何 agent 逻辑** —— 那些在 `riot-core` 里，宿主只是搬运工。
//!
//! 判断一段代码该不该放这里：它是否需要操作系统能力？需要就放这里，
//! 否则放内核。这条界线不清晰的话，agent 逻辑会慢慢渗进宿主，
//! 然后就没法脱离 Tauri 做黄金回放了。

pub mod browser;
pub mod config;
pub mod fence;
pub mod kernel;
pub mod session;
pub mod state;
pub mod web;

use tauri::Manager;
use tauri::ipc::Channel;

use riot_protocol::event::AgentEvent;
use state::AppState;

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error(transparent)]
    Kernel(#[from] kernel::KernelError),
    #[error(transparent)]
    Fence(#[from] fence::FenceError),
    #[error("会话不存在。先用 create_session 建一个（每个会话绑定一个项目目录）。")]
    NoSession,
    #[error("这个会话还没有订阅事件流")]
    NoSink,
    #[error(transparent)]
    Config(#[from] config::ConfigError),
    #[error("{0}")]
    Provider(String),
}

// Tauri 要求错误类型可序列化。thiserror 不给 Serialize，手写一层。
impl serde::Serialize for HostError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

type HostResult<T> = Result<T, HostError>;

/// 订阅会话事件流。
///
/// `Channel` 实现了 `Clone` 且是 `Send + Sync`，所以能存进 `State` 让后台任务
/// 长期持有 —— 不必局限在这次调用内。这正是 token 流需要的模式。
#[tauri::command]
async fn subscribe_session(
    state: tauri::State<'_, AppState>,
    session_id: String,
    epoch: u64,
    on_event: Channel<AgentEvent>,
) -> HostResult<()> {
    if !state.attach_sink(session_id.clone(), epoch, on_event).await {
        // 不当错误报。前端那次订阅本来就已经被自己弃用了，
        // 弹一条"订阅失败"只会把用户引向一个不存在的问题。
        tracing::debug!(session_id, epoch, "忽略过期的订阅");
    }
    Ok(())
}

#[tauri::command]
async fn send_turn(
    state: tauri::State<'_, AppState>,
    session_id: String,
    text: String,
) -> HostResult<String> {
    state.send_turn(&session_id, &text).await
}

#[tauri::command]
async fn interrupt(state: tauri::State<'_, AppState>, session_id: String) -> HostResult<()> {
    state.interrupt(&session_id).await
}

#[tauri::command]
async fn respond_permission(
    state: tauri::State<'_, AppState>,
    session_id: String,
    ask_id: String,
    response: riot_protocol::permission::PermissionResponse,
) -> HostResult<()> {
    state
        .respond_permission(&session_id, &ask_id, response)
        .await
}

#[tauri::command]
async fn set_permission_mode(
    state: tauri::State<'_, AppState>,
    session_id: String,
    mode: riot_protocol::permission::PermissionMode,
) -> HostResult<()> {
    state.set_mode(&session_id, mode).await
}

/// 会话级采样覆盖。空字段继承 provider 的设置；下一轮生效。
#[tauri::command]
async fn set_session_sampling(
    state: tauri::State<'_, AppState>,
    session_id: String,
    sampling: config::Sampling,
) -> HostResult<()> {
    state.set_sampling(&session_id, sampling).await
}

/// 当前模型配置与"密钥在不在、从哪来"。
///
/// `[约束]` 只回 `has_api_key` / `key_source`，不回 key 本身。前端不需要它，
/// 而一旦返回过，它就会出现在 devtools、日志和错误上报里。
#[tauri::command]
async fn get_config(state: tauri::State<'_, AppState>) -> HostResult<config::ConfigStatus> {
    Ok(config::ConfigStatus::of(state.config().await))
}

#[tauri::command]
async fn set_config(
    state: tauri::State<'_, AppState>,
    config: config::AppConfig,
) -> HostResult<config::ConfigStatus> {
    // 保存前校验：active 指向不存在的 provider 在这里报错，而不是
    // 等到发消息时才发现设置页写坏了配置。模型允许暂空（新 provider
    // 还没配模型的中间状态），resolve() 在发请求时拦。
    config.validate()?;
    config::save(&config)?;
    state.set_config(config.clone()).await;
    Ok(config::ConfigStatus::of(config))
}

/// 保存某个 provider 的 API key 到 auth.json（0600）。空字符串表示删除。
///
/// `[约束]` key 参数不能出现在任何日志或错误消息里 —— 这个函数体内
/// 不允许有 tracing 调用。
#[tauri::command]
async fn set_api_key(
    state: tauri::State<'_, AppState>,
    provider_id: String,
    key: String,
) -> HostResult<config::ConfigStatus> {
    let config = state.config().await;
    let p = config
        .provider(&provider_id)
        .ok_or_else(|| HostError::Provider(format!("找不到 provider「{provider_id}」")))?;
    config::save_key(&p.api_key_env, &key)?;
    Ok(config::ConfigStatus::of(config))
}

/// 把一个目录加进项目列表（验证它存在、可 canonicalize），返回规范化的根。
///
/// 只是登记，不创建会话 —— 项目和会话是两层：项目是侧边栏的分组，
/// 会话才绑定围栏。
#[tauri::command]
async fn add_project(state: tauri::State<'_, AppState>, path: String) -> HostResult<String> {
    let f = fence::Fence::new(&path)?;
    let root = f.root().display().to_string();

    let mut config = state.config().await;
    config.projects.retain(|p| p != &root);
    config.projects.insert(0, root.clone());
    // 上限防的是列表无限膨胀，不是内存 —— 是"侧边栏滚不到底"这种 UI 债
    config.projects.truncate(20);
    // 记不住就算了，不影响本次使用 —— 但要留下痕迹，不然"为什么有时候
    // 记得有时候不记得"会变成一个没法排查的玄学问题。
    if let Err(e) = config::save(&config) {
        tracing::warn!(error = %e, "项目列表没能写进配置");
    }
    state.set_config(config).await;
    Ok(root)
}

/// 在某个项目下开一个新会话。会话从创建起就绑定这个目录，永不改变。
#[tauri::command]
async fn create_session(
    state: tauri::State<'_, AppState>,
    root: String,
) -> HostResult<state::SessionInfo> {
    state.create_session(&root).await
}

/// 所有活着的会话。前端启动或刷新后用它对齐侧边栏。
#[tauri::command]
async fn list_sessions(state: tauri::State<'_, AppState>) -> HostResult<Vec<state::SessionInfo>> {
    Ok(state.list_sessions().await)
}

/// 一个会话的完整历史。切回这个会话时前端用它重建对话流。
#[tauri::command]
async fn get_history(
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> HostResult<Vec<riot_protocol::message::Message>> {
    state.history(&session_id).await
}

/// 删除会话（中断正在跑的轮子）。幂等。
#[tauri::command]
async fn delete_session(state: tauri::State<'_, AppState>, session_id: String) -> HostResult<()> {
    state.delete_session(&session_id).await;
    Ok(())
}

/// 重命名会话。空标题清除手动名，回退到第一条消息。
#[tauri::command]
async fn rename_session(
    state: tauri::State<'_, AppState>,
    session_id: String,
    title: String,
) -> HostResult<()> {
    state.rename_session(&session_id, &title).await
}

/// 把项目从列表移除并关闭它下面的会话。**不删磁盘上的目录。**
/// 返回被关闭的会话 id，前端拿它清理界面。
#[tauri::command]
async fn remove_project(
    state: tauri::State<'_, AppState>,
    root: String,
) -> HostResult<Vec<String>> {
    Ok(state.remove_project(&root).await)
}

/// 发一个最小请求，验证 base URL / key / 模型名的链路。
///
/// `provider_id` / `model` 不传就用当前激活的 —— 设置页测的是
/// "正在编辑的那个"，不一定是激活的。
#[tauri::command]
async fn test_connection(
    state: tauri::State<'_, AppState>,
    provider_id: Option<String>,
    model: Option<String>,
) -> HostResult<String> {
    let config = state.config().await;
    let probe = config::AppConfig {
        active_provider: provider_id.unwrap_or(config.active_provider.clone()),
        active_model: model.unwrap_or(config.active_model.clone()),
        ..config
    };
    let resolved = probe.resolve()?;
    session::test_connection(&resolved)
        .await
        .map_err(HostError::Provider)
}

/// 测 SearXNG 地址通不通。设置页的「测试」按钮走这里。
///
/// 传的是**正在编辑的**地址而不是已保存的配置 —— 用户的期待是"填完点
/// 测试"，要求先保存再测会让他在两个按钮之间来回跑。
#[tauri::command]
async fn test_search_backend(base_url: String) -> HostResult<String> {
    web::test_searxng(&base_url).await.map_err(HostError::Provider)
}

/// 拉取某个 provider 的可用模型列表（`GET /v1/models`）。
#[tauri::command]
async fn list_models(
    state: tauri::State<'_, AppState>,
    provider_id: String,
) -> HostResult<Vec<String>> {
    let config = state.config().await;
    let p = config
        .provider(&provider_id)
        .ok_or_else(|| HostError::Provider(format!("找不到 provider「{provider_id}」")))?;
    session::list_models(p).await.map_err(HostError::Provider)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "riot=debug,warn".into()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        // [约束] invoke_handler 只能调用一次。调多次只有最后一次生效，
        // 而且是静默失败 —— 前面注册的命令全部变成 "command not found"。
        .invoke_handler(tauri::generate_handler![
            subscribe_session,
            send_turn,
            interrupt,
            respond_permission,
            set_permission_mode,
            set_session_sampling,
            get_config,
            set_config,
            set_api_key,
            add_project,
            create_session,
            list_sessions,
            get_history,
            delete_session,
            rename_session,
            remove_project,
            test_connection,
            test_search_backend,
            list_models,
        ])
        .build(tauri::generate_context!())
        .expect("Tauri 初始化失败")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                // [约束] 这里必须开新的 tokio runtime，不能复用 tauri::async_runtime。
                // 复用它在 Windows 上经常失败或被整个跳过（clash-verge-rev 的实践结论）。
                //
                // 更重要的是：这个钩子在两条路径上根本不会跑 —— tauri dev 停止时
                // 对 cargo 发 SIGKILL，NSIS 升级时用 TerminateProcess。所以这里是
                // 「尽力而为」，真正的保障是 supervisor 里的 Job Object / 进程组。
                let state = app.state::<AppState>().inner().clone();
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("退出清理的 runtime");
                    rt.block_on(state.shutdown());
                })
                .join()
                .ok();
            }
        });
}
