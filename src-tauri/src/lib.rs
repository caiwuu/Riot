//! Tauri 宿主。
//!
//! 职责边界：窗口、内核进程监管、PTY、文件系统访问、密钥。
//! **不包含任何 agent 逻辑** —— 那些在 `riot-core` 里，宿主只是搬运工。
//!
//! 判断一段代码该不该放这里：它是否需要操作系统能力？需要就放这里，
//! 否则放内核。这条界线不清晰的话，agent 逻辑会慢慢渗进宿主，
//! 然后就没法脱离 Tauri 做黄金回放了。

pub mod browser;
pub mod fence;
// 阶段 B:内核逻辑搬进 riot-kernel crate,这里 re-export 维持 `crate::changes`
// 等旧路径,宿主其它模块无需改动(见 ARCHITECTURE.md §2.2)。
// 留在宿主的是需要 OS/tauri 能力的部分:browser、term、term_access、fence、
// persist、gui_env、askpass、kernel(进程监管)。
pub use riot_kernel::{
    changes, classifier, config, content, git, hooks, memory, mentions, models, session, skills,
    slash, subagent, vision, web,
};
mod askpass;
mod gui_env;
pub use askpass::run_client as run_askpass;
pub use gui_env::print_process_env;

pub mod env_probe;
pub mod kernel;
pub mod packs;
pub mod pasteboard;
pub mod persist;
pub mod state;
pub mod term;
pub mod term_access;
pub mod update;
#[cfg(any(windows, target_os = "macos"))]
mod vibrancy;

use tauri::Manager;
use tauri::ipc::Channel;

// 命令清单里要写光名字（ACL 测试按名字比对三份清单），所以这里 use 进来。
use pasteboard::clipboard_paths;

use riot_protocol::event::AgentEvent;
use state::AppState;

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error(transparent)]
    Kernel(#[from] kernel::KernelError),
    #[error(transparent)]
    Fence(#[from] fence::FenceError),
    /// 工作区根已经不在磁盘上。前端当成可恢复错误，不要整页炸掉。
    #[error("项目目录不存在：{0}")]
    MissingProject(String),
    #[error("浏览器不可用：{0}")]
    Browser(riot_protocol::browser::BrowserUnavailable),
    #[error("会话不存在。先用 create_session 建一个（每个会话绑定一个项目目录）。")]
    NoSession,
    #[error("这个会话还没有订阅事件流")]
    NoSink,
    #[error(transparent)]
    Config(#[from] config::ConfigError),
    #[error("{0}")]
    Provider(String),
    #[error("{0}")]
    Term(String),
    /// UserPromptSubmit hook 拦下了这条消息。
    #[error("{0}")]
    Hook(String),
    #[error("{0}")]
    Pack(String),
    #[error("{0}")]
    Update(String),
    /// 沙箱的提权安装。文案已经是给用户看的（含"你取消了权限确认"这种
    /// 非故障结局），前端直接显示即可。
    #[error("{0}")]
    Sandbox(String),
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

/// 返回 `Some(条目 id)` 表示上一轮还在跑、消息进了插话队列；
/// `None` 表示直接开轮。前端排队面板靠这个 id 跟踪条目。
#[tauri::command]
async fn send_turn(
    state: tauri::State<'_, AppState>,
    session_id: String,
    text: String,
    // 用户附的图。没附就是空数组。
    images: Vec<riot_protocol::ImageInput>,
    // 输入框里选中的文件引用（界面上的那些块），项目内相对路径。
    // Option 是为了兼容不带这个字段的调用（缺参数会被 Tauri 拒成一条
    // 看不懂的反序列化错误）。
    refs: Option<Vec<String>>,
) -> HostResult<Option<String>> {
    state
        .send_turn(&session_id, &text, images, refs.unwrap_or_default())
        .await
}

/// 丢掉指定助手消息及其后的一切，从它前面那条用户提示再跑一轮。
#[tauri::command]
async fn regenerate_turn(
    state: tauri::State<'_, AppState>,
    session_id: String,
    message_id: String,
) -> HostResult<()> {
    state.regenerate_turn(&session_id, &message_id).await
}

/// 手动压缩会话历史（`/compact`）。空闲时才能做；完成发 Compacted 事件。
#[tauri::command]
async fn session_compact(state: tauri::State<'_, AppState>, session_id: String) -> HostResult<()> {
    state.compact_session(&session_id).await
}

/// 可用的斜杠命令（内置 + 项目 + 全局）。`root` 为 null 时只列内置和全局。
#[tauri::command]
async fn slash_commands(root: Option<String>) -> HostResult<Vec<slash::SlashCommand>> {
    Ok(slash::discover(root.as_ref().map(std::path::Path::new)))
}

/// 配置里的 hooks 清单（含解析失败的文件）。给设置页看。
#[tauri::command]
async fn hooks_list(root: Option<String>) -> HostResult<Vec<hooks::HookInfo>> {
    Ok(hooks::list(root.as_ref().map(std::path::Path::new)))
}

/// 展开一条自定义命令：`/name args` → 发给模型的 prompt。
/// null = 没这条命令，或它是内置命令（前端按 name 执行）。
#[tauri::command]
async fn slash_expand(
    state: tauri::State<'_, AppState>,
    session_id: String,
    name: String,
    args: String,
) -> HostResult<Option<String>> {
    state.slash_expand(&session_id, &name, &args).await
}

/// `@` 补全菜单的文件搜索：返回项目内相对路径。
#[tauri::command]
async fn search_files(
    state: tauri::State<'_, AppState>,
    session_id: String,
    query: String,
) -> HostResult<Vec<String>> {
    state.search_files(&session_id, &query).await
}

/// 排队面板：当前排着的插话清单。
#[tauri::command]
async fn queue_list(
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> HostResult<Vec<session::QueuedSummary>> {
    state.queue_list(&session_id).await
}

/// 删掉一条排队插话。false = 条目已经不在（被注入或早被删了）。
#[tauri::command]
async fn queue_remove(
    state: tauri::State<'_, AppState>,
    session_id: String,
    entry_id: String,
) -> HostResult<bool> {
    state.queue_remove(&session_id, &entry_id).await
}

/// 撤回一条排队插话，还给前端原始输入（放回输入框编辑）。
#[tauri::command]
async fn queue_take(
    state: tauri::State<'_, AppState>,
    session_id: String,
    entry_id: String,
) -> HostResult<Option<riot_protocol::TurnInput>> {
    state.queue_take(&session_id, &entry_id).await
}

#[tauri::command]
async fn interrupt(state: tauri::State<'_, AppState>, session_id: String) -> HostResult<bool> {
    state.interrupt(&session_id).await
}

/// 本会话改了哪些文件、哪些行。只含经 Edit / Write 落下的改动 ——
/// 用户自己在编辑器里的改动不算，那正是这个视图和 `git diff` 的区别。
#[tauri::command]
async fn session_changes(
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> HostResult<Vec<changes::FileChange>> {
    state.changes(&session_id).await
}

/// 工作区相对所选基线的差异（侧边抽屉的 Git 面板）。
/// `base` 空 = 当前分支 / HEAD。只换对比对象，不 checkout。
#[tauri::command]
async fn session_git_changes(
    state: tauri::State<'_, AppState>,
    session_id: String,
    base: Option<String>,
) -> HostResult<riot_protocol::GitChanges> {
    state.git_changes(&session_id, base.as_deref()).await
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

/// 本会话已授权的渗透 scope（host 列表），给前端管理面板看。
#[tauri::command]
async fn browser_scope_list(
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> HostResult<Vec<String>> {
    state.scope_hosts(&session_id).await
}

/// 撤销一个渗透 scope 授权。之后对该目标的侵入性动作会重新要求授权。
#[tauri::command]
async fn browser_scope_revoke(
    state: tauri::State<'_, AppState>,
    session_id: String,
    host: String,
) -> HostResult<()> {
    state.revoke_scope(&session_id, &host).await
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

/// 探测会话根目录下的常见虚拟环境（.venv / venv）。
///
/// 系统的目录选择框默认隐藏点开头的目录，用户看不到 `.venv` ——
/// 探测出来给前端做一键填入。
#[tauri::command]
async fn detect_venvs(
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> HostResult<Vec<String>> {
    state.detect_venvs(&session_id).await
}

/// 会话的 Python 虚拟环境。空字符串清除；宿主会验证目录像不像一个 venv。
#[tauri::command]
async fn set_session_python_venv(
    state: tauri::State<'_, AppState>,
    session_id: String,
    path: String,
) -> HostResult<()> {
    state.set_python_venv(&session_id, &path).await
}

/// 会话级追加的系统提示词。空字符串清除；下一轮生效。
#[tauri::command]
async fn set_session_system_prompt(
    state: tauri::State<'_, AppState>,
    session_id: String,
    prompt: String,
) -> HostResult<()> {
    state.set_system_prompt(&session_id, &prompt).await
}

/// 会话级思考策略。默认不干预（不发思考参数）；下一轮生效。
#[tauri::command]
async fn set_session_thinking(
    state: tauri::State<'_, AppState>,
    session_id: String,
    thinking: riot_protocol::ThinkingPolicy,
) -> HostResult<()> {
    state.set_thinking(&session_id, thinking).await
}

/// 当前模型配置与"密钥在不在、从哪来"。
///
/// `[约束]` 只回 `has_api_key` / `key_source`，不回 key 本身。前端不需要它，
/// 而一旦返回过，它就会出现在 devtools、日志和错误上报里。
#[tauri::command]
async fn get_config(state: tauri::State<'_, AppState>) -> HostResult<config::ConfigStatus> {
    Ok(config::ConfigStatus::of(state.config().await))
}

/// 这台机器上 OS 级隔离**现在**能不能用。
///
/// 配置里那个「命令隔离」开关只是意图。Windows 上没跑过提权安装时，
/// 每轮激活都会静默失败、命令照常裸跑（决策链退回逐条询问，方向安全），
/// 而界面上看不出任何区别 —— 用户以为开着隔离，还得多点一堆确认框却不
/// 知道为什么。设置页要能把这两者分开显示，就得先能查到真实状态。
///
/// 只查不改，随时可调。
#[tauri::command]
async fn sandbox_status() -> riot_runtime::SandboxStatus {
    riot_runtime::sandbox::status()
}

/// 跑那次一次性的提权安装。**Windows 上会弹两次 UAC。**
///
/// `[约束]` 走 `spawn_blocking`。这个调用里等的是**用户去点系统对话框**，
/// 时长完全不可控；占着 tokio 的工作线程会把别的命令一起拖住。
#[tauri::command]
async fn sandbox_install() -> HostResult<()> {
    tokio::task::spawn_blocking(riot_runtime::sandbox::install)
        .await
        .map_err(|e| HostError::Sandbox(format!("安装任务没跑完：{e}")))?
        .map_err(HostError::Sandbox)
}

/// 卸载命令隔离（删掉沙箱专用账户与凭证）。**Windows 上会弹一次 UAC。**
///
/// `[约束]` 和安装同一条：走 `spawn_blocking`，等的是用户点系统对话框。
#[tauri::command]
async fn sandbox_uninstall() -> HostResult<()> {
    tokio::task::spawn_blocking(riot_runtime::sandbox::uninstall)
        .await
        .map_err(|e| HostError::Sandbox(format!("卸载任务没跑完：{e}")))?
        .map_err(HostError::Sandbox)
}

/// 和 `tauri.conf.json` 的 version 同一份，设置 → 关于用来显示。
#[tauri::command]
fn app_version(app: tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

/// 对照 GitHub 上最新正式 Release。没网、还没发过版都不该让调用方当成崩溃。
#[tauri::command]
async fn check_update(app: tauri::AppHandle) -> HostResult<update::UpdateInfo> {
    crate::update::check(&app.package_info().version.to_string()).await
}

#[tauri::command]
async fn set_config(
    state: tauri::State<'_, AppState>,
    mut config: config::AppConfig,
) -> HostResult<config::ConfigStatus> {
    // 保存前校验：active 指向不存在的 provider 在这里报错，而不是
    // 等到发消息时才发现设置页写坏了配置。模型允许暂空（新 provider
    // 还没配模型的中间状态），resolve() 在发请求时拦。
    config.validate()?;
    // 内置域名不写进 config.json，设置页也看不到。
    config.web.searxng_url = config::normalize_searxng_url(&config.web.searxng_url);
    config::save(&config)?;
    state.set_config(config.clone()).await;
    // MCP 连接对齐新配置。reconcile 只 diff + 起连接任务，不等握手，
    // 不会拖慢保存按钮；进度由设置页轮询 mcp_status 看。
    state.reconcile_mcp().await;
    Ok(config::ConfigStatus::of(config))
}

/// MCP 服务器的连接状态（设置页轮询它显示状态点和工具数）。
#[tauri::command]
async fn mcp_status(
    state: tauri::State<'_, AppState>,
) -> HostResult<Vec<riot_protocol::rpc::McpServerStatus>> {
    Ok(state.mcp_statuses().await)
}

/// 手动重连一个 MCP 服务器（设置页的「重连」按钮）。
#[tauri::command]
async fn mcp_restart(state: tauri::State<'_, AppState>, server_id: String) -> HostResult<()> {
    state.mcp_restart(&server_id).await
}

/// 当前可用的技能清单（含解析失败的，带原因）。
/// `root` 传当前会话的项目根；不传只列全局技能。
#[tauri::command]
async fn skills_list(root: Option<String>) -> HostResult<Vec<skills::SkillInfo>> {
    Ok(skills::list(root.as_deref().map(std::path::Path::new)))
}

/// 当前 MCP 服务器的标准 JSON（`{"mcpServers": {...}}`，生态通用格式）。
#[tauri::command]
async fn mcp_export_json(state: tauri::State<'_, AppState>) -> HostResult<String> {
    Ok(config::mcp_servers_to_json(
        &state.config().await.mcp_servers,
    ))
}

/// 用标准 JSON **整体替换** MCP 服务器配置。
///
/// 语义是替换不是追加：JSON 视图显示的就是全部，保存回来的也该是
/// 全部 —— 追加语义下删除一个服务器要去表单里点，两个视图就打架了。
/// 显示名不属于标准格式，按 id 从旧配置捡回来。
#[tauri::command]
async fn mcp_import_json(
    state: tauri::State<'_, AppState>,
    raw: String,
) -> HostResult<config::ConfigStatus> {
    let mut servers = config::mcp_servers_from_json(&raw)?;
    let mut config = state.config().await;
    for s in &mut servers {
        if s.name.is_empty()
            && let Some(old) = config.mcp_servers.iter().find(|o| o.id == s.id)
        {
            s.name = old.name.clone();
        }
    }
    config.mcp_servers = servers;
    config.validate()?;
    config::save(&config)?;
    state.set_config(config.clone()).await;
    state.reconcile_mcp().await;
    Ok(config::ConfigStatus::of(config))
}

// ── 能力包 ────────────────────────────────────────────

/// 能力包清单：装了什么、有什么可装。设置页轮询它。
#[tauri::command]
async fn packs_status() -> HostResult<Vec<packs::PackStatus>> {
    Ok(packs::status().await)
}

/// 下载并安装一个能力包，进度通过 `on_progress` 推给前端。
///
/// 装完立刻接线，不要求重启：MCP 服务器现连，技能目录每轮重扫，PATH 注入
/// 每轮现装。让用户重启一次应用才能用刚下好的东西，是很容易被当成"没装上"的。
#[tauri::command]
async fn packs_install(
    state: tauri::State<'_, AppState>,
    id: String,
    on_progress: Channel<packs::PackProgress>,
) -> HostResult<()> {
    let sink = on_progress.clone();
    let result = packs::install(&id, move |p| {
        // 前端不听了就没必要嚷嚷，但安装本身要继续走完。
        let _ = sink.send(p);
    })
    .await;

    match result {
        Ok(_) => {
            sync_packs_into_config(&state).await;
            Ok(())
        }
        Err(e) => {
            // 失败也要推一条终态。只靠命令的 Err 返回的话，进度条会永远停在
            // 最后一个百分比上，用户不知道是卡住了还是失败了。
            let _ = on_progress.send(packs::PackProgress::Failed {
                error: e.to_string(),
            });
            Err(HostError::Pack(e.to_string()))
        }
    }
}

/// 卸载一个能力包，连带摘掉它注册的 MCP 服务器。
#[tauri::command]
async fn packs_uninstall(state: tauri::State<'_, AppState>, id: String) -> HostResult<()> {
    packs::uninstall(&id).map_err(|e| HostError::Pack(e.to_string()))?;
    sync_packs_into_config(&state).await;
    Ok(())
}

/// 把"当前装了哪些包"落进配置并重连 MCP。装、卸、启动三处共用。
async fn sync_packs_into_config(state: &AppState) {
    let mut config = state.config().await;
    if !packs::sync_mcp(&mut config) {
        return;
    }
    // 存不下也要让本次生效 —— 配置文件写不进去是另一个问题，不该顺带
    // 把刚装好的能力包也变成不可用。
    if let Err(e) = config::save(&config) {
        tracing::warn!(error = %e, "能力包的 MCP 配置没能落盘");
    }
    state.set_config(config).await;
    state.reconcile_mcp().await;
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

// ── 内置浏览器面板 ────────────────────────────────────
//
// `[约束]` 面板发起的操作**不过权限链**。
//
// 权限系统管的是"模型能不能做这件事"。面板上的点击、输入、地址栏跳转都是
// **用户自己**在操作 —— 为用户的鼠标弹一个"是否允许点击"的窗，既没有意义
// 也会立刻把人逼去开「全部放行」。模型走 Browser* 工具，那条照常受管。

/// 打开面板:开始把画面推给前端，并回一份标签栏状态。
///
/// `[约束]` 必须把状态一起回。不回的话前端只能等下一次定时同步 —— 而浏览器
/// 起来本身就要一秒，再叠一次轮询间隔，用户看到的是"开了面板、空着两秒、
/// 才冒出一个标签页"。
///
/// `[取舍]` 帧走**二进制**通道，不走 JSON。格式是 8 字节小端头（宽、高，
/// 各 u32，CSS 像素）+ JPEG 字节，前端按这个拆。JSON 的路径每帧要把几百 KB
/// 的 base64 序列化一遍、在 JS 主线程上 `JSON.parse` 一遍 —— 界面主线程
/// 同时还要处理输入和渲染，滚动时正是它们在互相挤兑。Raw 通道在 Tauri
/// 里走 fetch 取回，JS 拿到的直接是 ArrayBuffer，两次解析都省掉。
#[tauri::command]
async fn browser_open(
    state: tauri::State<'_, AppState>,
    session_id: String,
    on_frame: Channel<tauri::ipc::InvokeResponseBody>,
) -> HostResult<browser::access::PanelState> {
    let b = state.panel_browser(&session_id).await?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<browser::access::Frame>();

    // 帧从 tokio 通道转到 Tauri 的 Channel。中间这一跳是必要的:
    // Channel 不是 Clone 到处传的东西，而帧的产生方在另一个任务里。
    tokio::spawn(async move {
        while let Some(mut f) = rx.recv().await {
            // 只推最新的一帧。通道是无界的，前端一旦消化得慢，积压的
            // 每一帧都还要过一遍 webview —— 追着播放旧帧只会让画面越来
            // 越落后于手上的操作。滚动要的是"跟手"，不是"一帧不落"。
            while let Ok(newer) = rx.try_recv() {
                f = newer;
            }
            let mut buf = Vec::with_capacity(8 + f.data.len());
            buf.extend_from_slice(&f.width.to_le_bytes());
            buf.extend_from_slice(&f.height.to_le_bytes());
            buf.extend_from_slice(&f.data);
            if on_frame
                .send(tauri::ipc::InvokeResponseBody::Raw(buf))
                .is_err()
            {
                break; // 前端不听了
            }
        }
    });

    b.start_screencast(tx).await.map_err(HostError::Browser)?;
    b.state().await.map_err(HostError::Browser)
}

/// 关闭面板。停止编码 —— 没人看的时候继续推是白烧 CPU 和电。
#[tauri::command]
async fn browser_close(state: tauri::State<'_, AppState>, session_id: String) -> HostResult<()> {
    // 会话已经没了也算成功:用户关窗口时两件事同时发生，报错没有意义。
    if let Ok(b) = state.panel_browser(&session_id).await {
        b.stop_screencast().await;
    }
    Ok(())
}

/// 地址栏跳转。用户自己输的，不问权限。
#[tauri::command]
async fn browser_navigate(
    state: tauri::State<'_, AppState>,
    session_id: String,
    url: String,
) -> HostResult<()> {
    use riot_protocol::browser::BrowserAccess as _;
    let b = state.panel_browser(&session_id).await?;
    b.navigate(&url).await.map_err(HostError::Browser)
}

/// 工具栏的前进后退。`delta` 为 -1 后退、+1 前进。回来的是走完之后的状态。
///
/// 合成一条命令而不是 back / forward 两条:两边的实现只差一个符号，
/// 而每多一条命令就要同步 build.rs、capabilities 和 ACL 用例三处。
#[tauri::command]
async fn browser_history(
    state: tauri::State<'_, AppState>,
    session_id: String,
    delta: i32,
) -> HostResult<browser::access::TabInfo> {
    let b = state.panel_browser(&session_id).await?;
    b.go(delta).await.map_err(HostError::Browser)
}

/// 刷新当前页面。
#[tauri::command]
async fn browser_reload(state: tauri::State<'_, AppState>, session_id: String) -> HostResult<()> {
    let b = state.panel_browser(&session_id).await?;
    b.reload().await.map_err(HostError::Browser)
}

/// 标签栏 + 工具栏的状态。面板定期问，用来跟上页面自己发起的跳转。
#[tauri::command]
async fn browser_state(
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> HostResult<browser::access::PanelState> {
    let b = state.panel_browser(&session_id).await?;
    b.state().await.map_err(HostError::Browser)
}

/// 新开一个标签页并切过去。
#[tauri::command]
async fn browser_new_tab(
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> HostResult<browser::access::PanelState> {
    let b = state.panel_browser(&session_id).await?;
    b.open_tab().await.map_err(HostError::Browser)
}

/// 关一个标签页。关掉最后一个时会补一个新的空白页。
#[tauri::command]
async fn browser_close_tab(
    state: tauri::State<'_, AppState>,
    session_id: String,
    tab: u32,
) -> HostResult<browser::access::PanelState> {
    let b = state.panel_browser(&session_id).await?;
    b.close_tab(tab).await.map_err(HostError::Browser)
}

/// 切到某个标签页。画面和工具栏都跟着它 —— 模型的浏览器工具也一样，
/// 见 `BrowserAccess for HostBrowser` 的说明。
#[tauri::command]
async fn browser_select_tab(
    state: tauri::State<'_, AppState>,
    session_id: String,
    tab: u32,
) -> HostResult<browser::access::PanelState> {
    let b = state.panel_browser(&session_id).await?;
    b.select_tab(tab).await.map_err(HostError::Browser)
}

/// 面板尺寸变了。视口跟着变 —— 比例对不上时画面周围会留出黑边。
///
/// `width`/`height` 是页面排版视口（Web 模式是 1280）。`view_width` /
/// `view_height` 是画面区实际占的 CSS 像素，推流按它封顶。
/// `scale` 是面板所在屏幕的像素密度。它决定同一块地方用多少物理像素去画，
/// 不改变页面的排版尺寸。
#[tauri::command]
async fn browser_resize(
    state: tauri::State<'_, AppState>,
    session_id: String,
    width: i32,
    height: i32,
    scale: f32,
    view_width: i32,
    view_height: i32,
) -> HostResult<()> {
    let b = state.panel_browser(&session_id).await?;
    b.resize_view(width, height, scale, view_width, view_height)
        .await
        .map_err(HostError::Browser)
}

/// 把面板上的输入打到页面里。
#[tauri::command]
async fn browser_input(
    state: tauri::State<'_, AppState>,
    session_id: String,
    input: browser::access::Input,
) -> HostResult<()> {
    let b = state.panel_browser(&session_id).await?;
    b.send_input(input).await.map_err(HostError::Browser)
}

// ── 底部终端面板 ──────────────────────────────────────
//
// 和浏览器面板同一条原则：面板里的操作是**用户自己**在敲，不过权限链。
// 终端跟应用走、不跟会话走 —— 里面可能挂着 dev server，切个会话就把它
// 杀掉是不可接受的。

/// 开一个终端（在 `root` 目录起用户的默认 shell），输出通过 `on_event` 推给前端。
///
/// `root` 不存在或没传就退回家目录 —— 终端还是要开，只是位置不理想。
#[tauri::command]
async fn term_open(
    terms: tauri::State<'_, term::Terminals>,
    root: Option<String>,
    cols: u16,
    rows: u16,
    on_event: Channel<term::TermEvent>,
) -> HostResult<u32> {
    terms
        .open(root, cols, rows, on_event)
        .map_err(HostError::Term)
}

/// 把键盘输入写进 shell。`data` 是 xterm 给的原始串（含控制序列）。
#[tauri::command]
async fn term_write(
    terms: tauri::State<'_, term::Terminals>,
    id: u32,
    data: String,
) -> HostResult<()> {
    terms.write(id, &data).map_err(HostError::Term)
}

/// 面板里的终端尺寸变了，PTY 跟着变 —— 不同步的话 shell 按旧宽度折行。
#[tauri::command]
async fn term_resize(
    terms: tauri::State<'_, term::Terminals>,
    id: u32,
    cols: u16,
    rows: u16,
) -> HostResult<()> {
    terms.resize(id, cols, rows).map_err(HostError::Term)
}

/// 关一个终端（杀掉 shell）。幂等。
#[tauri::command]
async fn term_close(terms: tauri::State<'_, term::Terminals>, id: u32) -> HostResult<()> {
    terms.close(id);
    Ok(())
}

/// 现有的终端。面板重建标签栏、以及发现模型起了新服务，都靠它。
#[tauri::command]
async fn term_list(terms: tauri::State<'_, term::Terminals>) -> HostResult<Vec<term::TermSummary>> {
    Ok(terms.list())
}

/// 把出口挂到一个已经在跑的终端上，并回放它已有的输出。
///
/// 模型起的服务在面板打开之前就在跑了 —— 没有这条，那些输出永远到不了
/// 用户眼前。
#[tauri::command]
async fn term_attach(
    terms: tauri::State<'_, term::Terminals>,
    id: u32,
    on_event: Channel<term::TermEvent>,
) -> HostResult<()> {
    terms.attach(id, on_event).map_err(HostError::Term)
}

/// 把一个终端交给模型看 / 收回来。
///
/// `[约束]` 这条命令只有面板会调 —— 模型侧的 `TerminalAccess` 里没有对应
/// 方法，它不能给自己开权限。共享只给读，停仍然只认模型自己起的终端，
/// 理由见 `term_access` 的模块文档。
#[tauri::command]
async fn term_share(
    terms: tauri::State<'_, term::Terminals>,
    id: u32,
    shared: bool,
) -> HostResult<()> {
    terms.set_shared(id, shared);
    Ok(())
}

/// 这个终端的前台有没有正在跑的进程。关标签前的确认用 ——
/// 一键杀掉正忙的 dev server 不该是无声的。
#[tauri::command]
async fn term_busy(terms: tauri::State<'_, term::Terminals>, id: u32) -> HostResult<bool> {
    Ok(terms.is_busy(id))
}

/// 读一个图片文件，回 base64 和 MIME 类型。
///
/// 给拖进来的图和"选图片"按钮用。前端拿不到磁盘内容:webview 的
/// `File` 对象只有拖放数据里那份，而 Tauri 的拖放事件给的是**路径**。
///
/// `[约束]` 必须限大小。一张手机拍的照片十几 MB，读进来再 base64 变二十
/// 多 MB，光是 IPC 那一跳就能让界面卡住一两秒 —— 而它最终还是会被服务方
/// 的单图上限拒掉。在这里拦住，用户立刻知道是哪张图的问题。
#[tauri::command]
async fn read_image(path: String) -> HostResult<content::ImageOutput> {
    content::read_image(&path)
        .await
        .map_err(HostError::Provider)
}

/// 文件预览一次能读进来的上限。
///
/// 预览是在 webview 里渲染的：字节要过一次 IPC、再在 JS 侧解析成
/// DOM / canvas，太大的文件两头都扛不住。超限让用户走系统应用打开，
/// 那条路是流式的。
const MAX_PREVIEW_FILE: u64 = 128 * 1024 * 1024;

/// 读一个文件的原始字节，给应用内预览（Office / PDF / 图片等）用。
///
/// 返回 `tauri::ipc::Response`，走 Tauri 的原始二进制通道 —— 不经
/// JSON 也不做 base64，几十 MB 的文档不会因为编码把界面卡住。
///
/// 只在用户点击界面上已展示的路径时调用，语义与 `read_image` 一致：
/// 读的是用户自己机器上、自己点开的文件。
#[tauri::command]
async fn read_file_bytes(path: String) -> HostResult<tauri::ipc::Response> {
    let meta = tokio::fs::metadata(&path)
        .await
        .map_err(|e| HostError::Provider(format!("读不到文件：{e}")))?;
    if !meta.is_file() {
        return Err(HostError::Provider("这个路径不是文件".into()));
    }
    if meta.len() > MAX_PREVIEW_FILE {
        return Err(HostError::Provider(format!(
            "文件太大（{} MB），应用内预览最多 {} MB。请用系统应用打开。",
            meta.len() / (1024 * 1024),
            MAX_PREVIEW_FILE / (1024 * 1024),
        )));
    }
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| HostError::Provider(format!("读文件失败：{e}")))?;
    Ok(tauri::ipc::Response::new(bytes))
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

/// 哪些路径现在不是目录。只看、不改配置。
///
/// 侧栏和欢迎页用来标失效项目：目录是用户在访达里删的，配置里的
/// 列表不会自己消失。每次点开再 canonicalize 失败再整页报错，太晚了。
#[tauri::command]
fn probe_dirs(paths: Vec<String>) -> Vec<String> {
    fence::missing_dirs(paths)
}

/// 这个路径在磁盘上存在吗。打开本地文件前的前置检查。
///
/// opener 插件是分离式启动（spawn 完不等结果），目标不存在也报成功，
/// 用户看到的就是"点了没反应"。先问一句存在与否，界面才有机会说"打不开"。
#[tauri::command]
fn path_exists(path: String) -> bool {
    std::path::Path::new(&path).exists()
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
) -> HostResult<state::HistoryOut> {
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
    // 覆盖 active 到"正在编辑的那个"再解析。用可变赋值而不是结构体更新
    // 语法:config 里有 pub(crate) 的废弃字段,配置类型搬进 riot-kernel 之后,
    // 跨 crate 的 `..config` 访问不到它们。
    let mut probe = state.config().await;
    if let Some(p) = provider_id {
        probe.active_provider = p;
    }
    if let Some(m) = model {
        probe.active_model = m;
    }
    let resolved = probe.resolve()?;
    models::test_connection(&resolved)
        .await
        .map_err(HostError::Provider)
}

/// 测搜索后端通不通。设置页的「测试」按钮走这里。
///
/// 传的是**正在编辑的**地址而不是已保存的配置 —— 用户的期待是"填完点
/// 测试"，要求先保存再测会让他在两个按钮之间来回跑。空地址测内置实例。
#[tauri::command]
async fn test_search_backend(base_url: String) -> HostResult<String> {
    web::test_searxng(&base_url)
        .await
        .map_err(HostError::Provider)
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
    models::list_models(p).await.map_err(HostError::Provider)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "riot=debug,warn".into()),
        )
        .init();

    // Dock / 访达启动的 .app 继承不到终端环境。必须在 restore 和任何子进程
    // 之前补上（set_var 不是线程安全的）：先吸入登录 shell 的 PATH /
    // SSH_AUTH_SOCK / gh token，再挂上 GIT_ASKPASS。
    gui_env::inherit_login_env();
    askpass::install();

    // dev 和安装版共用 identifier「dev.riot.app」，Windows 上 WebView2 的用户
    // 数据目录因此都是 %LOCALAPPDATA%\dev.riot.app\EBWebView。这个目录被先
    // 启动的实例占住后，后启动的那个创建 webview 会报 0x8007139F
    // （ERROR_INVALID_STATE），窗口永远出不来 —— 装过正式版再跑
    // `pnpm tauri dev` 必踩。debug 构建改用自己的目录，两边即可并存。
    // 环境变量的优先级高于 wry 传给 CreateCoreWebView2Environment 的参数；
    // 用户自己 export 过就尊重用户的。
    #[cfg(all(windows, debug_assertions))]
    if std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").is_none()
        && let Some(base) = std::env::var_os("LOCALAPPDATA")
    {
        let dir = std::path::Path::new(&base)
            .join("dev.riot.app")
            .join("EBWebView-dev");
        // 豁免理由：宿主启动路径，建的是自己的 webview profile 目录。
        #[allow(clippy::disallowed_methods)]
        let _ = std::fs::create_dir_all(&dir);
        // 此刻只有主线程（见 inherit_login_env 的约束），set_var 安全。
        unsafe { std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &dir) };
    }

    // 全局技能目录启动时就建好。设置页的「打开目录」按钮 reveal 的是它 ——
    // 目录不存在时系统的 reveal 静默失败，按钮看起来就是坏的（用户没写过
    // 技能之前恰恰是最需要这个按钮的时候）。建不出来只记日志：技能扫描
    // 对缺目录本来就容忍。
    // 豁免理由：宿主启动路径，操作自己的配置目录。
    #[allow(clippy::disallowed_methods)]
    if let Err(e) = std::fs::create_dir_all(skills::global_dir()) {
        tracing::warn!(error = %e, "全局技能目录建不出来，设置页的「打开目录」将无效");
    }

    // restore 而不是空表：会话和历史从上次的磁盘状态恢复。
    // 必须发生在任何命令可达之前 —— 前端启动第一件事就是 list_sessions。
    let state = AppState::restore();
    // 同一份终端句柄两边共享：前端命令用 managed 的那份，会话用 state 里
    // 那份。各拿各的话，模型起的服务会跑在一个用户永远看不到的面板里。
    let terminals = state.terminals();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .manage(state)
        .manage(terminals)
        // [约束] invoke_handler 只能调用一次。调多次只有最后一次生效，
        // 而且是静默失败 —— 前面注册的命令全部变成 "command not found"。
        .invoke_handler(tauri::generate_handler![
            subscribe_session,
            send_turn,
            regenerate_turn,
            queue_list,
            queue_remove,
            queue_take,
            session_compact,
            slash_commands,
            slash_expand,
            hooks_list,
            search_files,
            interrupt,
            session_changes,
            session_git_changes,
            respond_permission,
            set_permission_mode,
            set_session_sampling,
            detect_venvs,
            set_session_python_venv,
            set_session_system_prompt,
            set_session_thinking,
            browser_open,
            browser_close,
            browser_navigate,
            browser_history,
            browser_reload,
            browser_state,
            browser_new_tab,
            browser_close_tab,
            browser_select_tab,
            browser_resize,
            browser_input,
            browser_scope_list,
            browser_scope_revoke,
            term_open,
            term_write,
            term_resize,
            term_close,
            term_list,
            term_attach,
            term_share,
            term_busy,
            read_image,
            read_file_bytes,
            clipboard_paths,
            get_config,
            app_version,
            check_update,
            set_config,
            set_api_key,
            mcp_status,
            mcp_restart,
            mcp_export_json,
            mcp_import_json,
            skills_list,
            sandbox_status,
            sandbox_install,
            sandbox_uninstall,
            packs_status,
            packs_install,
            packs_uninstall,
            add_project,
            create_session,
            probe_dirs,
            path_exists,
            list_sessions,
            get_history,
            delete_session,
            rename_session,
            remove_project,
            test_connection,
            test_search_backend,
            list_models,
        ])
        // 启动时把 MCP 连接对齐配置。放 setup 里而不是 restore：
        // spawn 连接任务要求 runtime 已经起来，restore 跑在那之前。
        .setup(|app| {
            // 只有一套深色配色。系统浅色时不钉住：macOS 的 sidebar 材质会变成
            // 浅灰，Windows 的 mica 会垫一层白雾。配置里 theme: Dark 在建窗时
            // 先钉上；这里再钉一次，覆盖菜单和以后新开的窗。
            app.set_theme(Some(tauri::Theme::Dark));
            // 侧栏材质的平台补钉，见 vibrancy 模块。放在最前面：越早越不容易
            // 被用户看见中间态。
            #[cfg(any(windows, target_os = "macos"))]
            if let Some(main) = app.get_webview_window("main") {
                vibrancy::apply(&main);
            }

            // 接上内核事件里宿主要消费的那几件(busy / mode 回流)。
            app.state::<AppState>().inner().spawn_host_bridge();
            let state = app.state::<AppState>().inner().clone();
            tauri::async_runtime::spawn(async move {
                // 先按已装的能力包对齐 MCP 配置，再连。分两步的话，包里的
                // 服务器要等到用户下次进设置页才会起来。用户手工删过配置、
                // 或者直接拷贝了一份包目录进来，都靠这一步兜住。
                sync_packs_into_config(&state).await;
                state.reconcile_mcp().await;
            });
            // 顺手收掉没人认领的浏览器 profile。同样放 setup：它要在会话表
            // 恢复完之后才能判断谁是孤儿，而且删目录要 spawn_blocking。
            // 不 await —— 启动路径上不该等着删几个 GB 的缓存。
            let state = app.state::<AppState>().inner().clone();
            tauri::async_runtime::spawn(async move {
                state.gc_browser_profiles().await;
            });
            Ok(())
        })
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
