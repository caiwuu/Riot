//! 应用配置与密钥。
//!
//! # 结构
//!
//! - [`AppConfig`] —— 持久化到 `config.json` 的全部内容：Provider 列表、
//!   当前激活的 provider+model、采样参数、项目列表。
//! - [`ProviderConfig`] —— 一个模型服务方：协议（openai/anthropic）、
//!   Base URL、密钥来源、已添加的模型列表。
//! - [`ResolvedModel`] —— 会话创建时对"当前配置"拍的快照。会话生命周期内
//!   不变，改设置只影响之后创建的会话 —— 和"会话绑定项目目录"同一条哲学。
//!
//! # 密钥
//!
//! `[约束]` API key 不进 `config.json`，也不返回给前端。
//!
//! 密钥存在**单独的** `auth.json`（0600 权限），按 `api_key_env` 为键 ——
//! 每个 provider 一份，换 provider 不会弄丢另一家的密钥。环境变量仍然
//! 可用且**优先**于 `auth.json`：显式的临时覆盖应该赢过存档。
//!
//! 不变的约束：key 绝不进日志、事件、错误消息，绝不返回给前端。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 服务方说话用的协议。决定请求格式、认证头和哪些采样参数可发送。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    /// OpenAI Chat Completions 兼容（DeepSeek、Kimi、vLLM、Ollama、各家中转）。
    Openai,
    /// Anthropic Messages。
    Anthropic,
}

/// 一个模型服务方。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    /// 稳定标识。auth.json 里的键、active_provider 的引用都靠它。
    pub id: String,
    /// 显示名。
    pub name: String,
    pub protocol: Protocol,
    pub base_url: String,
    /// 读 key 的环境变量名，同时是 `auth.json` 里的存储键。
    pub api_key_env: String,
    /// 已添加的模型（手动输入或从 `/models` 接口挑选保存）。
    #[serde(default)]
    pub models: Vec<String>,
    /// 过载时降级到的模型。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_model: Option<String>,
    /// 这个服务方的采样参数。会话可以临时覆盖单个字段。
    #[serde(default)]
    pub sampling: Sampling,
}

impl ProviderConfig {
    /// 取 key：环境变量优先，其次 `auth.json`。
    ///
    /// `[约束]` 返回值绝不能进日志、事件或错误消息。缺失时只说变量名。
    pub fn api_key(&self) -> Result<String, ConfigError> {
        self.api_key_in(&load_auth(&auth_path()))
    }

    fn api_key_in(&self, auth: &HashMap<String, String>) -> Result<String, ConfigError> {
        if let Ok(k) = std::env::var(&self.api_key_env)
            && !k.trim().is_empty()
        {
            return Ok(k.trim().to_owned());
        }
        if let Some(k) = auth.get(&self.api_key_env)
            && !k.trim().is_empty()
        {
            return Ok(k.trim().to_owned());
        }
        Err(ConfigError::MissingKey {
            var: self.api_key_env.clone(),
        })
    }

    /// key 来自哪里。前端用它决定显示"使用环境变量"还是"已保存"。
    pub fn key_source(&self) -> Option<&'static str> {
        if matches!(std::env::var(&self.api_key_env), Ok(k) if !k.trim().is_empty()) {
            return Some("env");
        }
        if matches!(load_auth(&auth_path()).get(&self.api_key_env), Some(k) if !k.trim().is_empty())
        {
            return Some("saved");
        }
        None
    }
}

/// 采样参数。`None` = 不设置：在 provider 层表示用服务端默认，
/// 在会话覆盖层表示继承 provider 的值。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sampling {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// 仅 Anthropic 协议发送。OpenAI 官方端点会拒绝未知参数。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

impl Sampling {
    /// 字段级合并：self 设置了的字段赢，没设置的用 `base` 的。
    /// 会话覆盖 provider 默认时用 —— 只改 temperature 不该把 max_tokens 也清掉。
    pub fn or(self, base: Sampling) -> Sampling {
        Sampling {
            temperature: self.temperature.or(base.temperature),
            top_p: self.top_p.or(base.top_p),
            top_k: self.top_k.or(base.top_k),
            max_output_tokens: self.max_output_tokens.or(base.max_output_tokens),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Sampling::default()
    }
}

/// 联网能力的配置。
///
/// 抓取（WebFetch）和搜索（WebSearch）分开开关：抓取不需要任何第三方
/// 服务，配好就能用；搜索要先有一个 SearXNG 实例。"只让模型读我贴过来的
/// 链接、别自己去搜"是一种合理的用法，两个开关合并就表达不出来。
///
/// 只支持 SearXNG 一种后端。Tavily / Brave / Serper 都要 key、要额度、
/// 而且在国内还要代理；SearXNG 自己起一个 docker 就能用，没有额度概念。
/// 真要加第二种后端时，这里加一个 `backend` 枚举字段，`#[serde(default)]`
/// 让老配置继续能读 —— 现在就把它加上只是给一个空抽象。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebConfig {
    /// 允许 WebFetch 抓网页。
    #[serde(default = "yes")]
    pub fetch_enabled: bool,
    /// 允许 WebSearch 搜索。关掉或没填地址时，工具会提示去设置里配。
    #[serde(default)]
    pub search_enabled: bool,
    /// SearXNG 实例地址，如 `http://127.0.0.1:8080`。
    ///
    /// `[约束]` 这个地址**不过** SSRF 检查 —— 自托管实例跑在
    /// `127.0.0.1` 是最常见的部署方式，套上抓取工具那层内网拦截会让它
    /// 完全没法用。这不是破例：安全边界在于它是用户亲手填的一个固定
    /// 地址，模型影响不了它，而模型能影响的 `q` 参数是被 URL 编码过的。
    #[serde(default)]
    pub searxng_url: String,
    /// 蒸馏网页正文用的辅助模型，格式 `providerId/model`。
    ///
    /// 空 = 不蒸馏，WebFetch 直接返回截断后的正文。这不是降级而是一个
    /// 合理选择：只配了一个贵模型的用户可能宁愿多花点上下文，也不想
    /// 每抓一个网页就多一次计费调用。
    #[serde(default)]
    pub distill_model: String,
}

fn yes() -> bool {
    true
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            // 抓取默认开：它不依赖任何外部服务，而且每个域名还要过一次
            // 用户确认，再加一道默认关闭的开关只是让人多找一次设置。
            fetch_enabled: true,
            // 搜索默认关：没填地址时开着也没用，只会让模型调一次失败一次。
            search_enabled: false,
            searxng_url: String::new(),
            distill_model: String::new(),
        }
    }
}

impl WebConfig {
    /// 搜索是不是真的可用（开关开着 + 地址填了）。
    pub fn search_ready(&self) -> bool {
        self.search_enabled && !self.searxng_url.trim().is_empty()
    }

    /// 拆出 `providerId/model`。
    pub fn distill_target(&self) -> Option<(&str, &str)> {
        let (p, m) = self.distill_model.trim().split_once('/')?;
        (!p.is_empty() && !m.is_empty()).then_some((p, m))
    }
}

/// 应用配置。整个结构持久化到 `config.json`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub providers: Vec<ProviderConfig>,
    /// 当前使用的 provider（id）与模型。
    pub active_provider: String,
    pub active_model: String,
    /// **已废弃**：采样参数属于各个 provider（[`ProviderConfig::sampling`]）。
    /// 字段保留只为读懂短暂存在过的"全局参数"格式 —— 加载时
    /// [`parse`] 会把它搬进当时激活的 provider 然后清空。
    #[serde(default, skip_serializing_if = "Sampling::is_empty")]
    pub(crate) sampling: Sampling,
    /// 最近打开过的项目目录，最近的在前。
    #[serde(default)]
    pub projects: Vec<String>,
    /// 新会话的默认权限模式。None = 每次询问。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_mode: Option<riot_protocol::permission::PermissionMode>,
    /// 权限弹窗等多久算超时（秒）。超时按拒绝处理。
    ///
    /// 可配置是因为两种用法要的值差一个量级：盯着屏幕的人希望等久点，
    /// 别手一慢就被拒；挂着跑长任务的人希望快点放弃 —— 那个弹窗根本
    /// 没人看，等待只是让整条任务停在那里。
    #[serde(default = "default_ask_timeout_secs")]
    pub ask_timeout_secs: u32,
    /// 联网能力（抓取 + 搜索）。
    #[serde(default)]
    pub web: WebConfig,
}

/// 权限弹窗默认等 60 秒。
///
/// 以前是 600 秒。那个值假设用户一定会回来，可长任务的现实是没人回来 ——
/// 一次误触发就把整轮任务钉在那儿十分钟，而结局仍然是拒绝。既然结局一样，
/// 早点拒绝、让模型换条路走，比让任务空转十分钟有用。
///
/// 60 秒对在场的用户仍然够用：弹窗是主动弹出来的，不是要人去找。
pub const fn default_ask_timeout_secs() -> u32 {
    60
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            // `[约束]` 不预置任何服务方。
            //
            // 曾经内置过 DeepSeek / Anthropic / Moonshot 三家和各自的默认
            // 模型名。问题不在"多给了几个选项",在于**那些字符串会过期**:
            // 厂商一发新版、一下线旧型号,预设就变成"看得见、选得中、
            // 一发请求报 400"的死选项,而报出来的是各家措辞不一的
            // model not found —— 用户根本猜不到该去哪儿改。
            //
            // 更麻烦的是它们看起来像"Riot 支持的模型清单",于是过期的
            // 那份数据成了错误的事实来源。空列表至少是诚实的。
            providers: Vec::new(),
            active_provider: String::new(),
            active_model: String::new(),
            sampling: Sampling::default(),
            projects: Vec::new(),
            default_mode: None,
            ask_timeout_secs: default_ask_timeout_secs(),
            web: WebConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// 配置能不能保存：active 必须指向存在的 provider。
    ///
    /// 刻意**不要求**模型非空 —— "刚添加 provider 还没配模型"是设置页的
    /// 合法中间状态。空模型在真正发请求时由 [`Self::resolve`] 拦截。
    pub fn validate(&self) -> Result<(), ConfigError> {
        // 空 active 是合法状态:一个还没配过任何服务方的新用户、或者刚把
        // 最后一家删掉的用户,都停在这里。拒绝保存的话,用户就再也删不掉
        // 最后一个服务方了 —— 那是把"没有"当成非法,而它只是"还没有"。
        if self.active_provider.is_empty() {
            return Ok(());
        }
        self.provider(&self.active_provider).map(|_| ()).ok_or_else(|| {
            ConfigError::Parse(format!("找不到 provider「{}」", self.active_provider))
        })
    }

    /// 当前激活的 provider+model 的运行时快照。每轮开始时解析一次 ——
    /// 用户在对话中途切换模型，下一轮就用新的。
    pub fn resolve(&self) -> Result<ResolvedModel, ConfigError> {
        self.resolve_named(&self.active_provider, &self.active_model)
    }

    /// 解析任意一对 provider+model。
    ///
    /// 辅助模型（网页蒸馏）走这条路 —— 它和 `active_*` 是两回事，用户
    /// 完全可以主对话用贵模型、蒸馏用便宜的本地模型。
    pub fn resolve_named(&self, provider_id: &str, model: &str) -> Result<ResolvedModel, ConfigError> {
        // 一个字都没配的时候，「找不到 provider「」」这种话等于没说。
        // 报错要能直接告诉用户下一步做什么。
        if provider_id.is_empty() {
            return Err(ConfigError::Parse(
                "还没有配置服务方，去设置里添加一个。".into(),
            ));
        }
        let p = self
            .provider(provider_id)
            .ok_or_else(|| ConfigError::Parse(format!("找不到 provider「{provider_id}」")))?;
        // 空模型名不能出宿主。发出去的结果是各家 API 五花八门的 400，
        // 用户从那种报错里看不出"其实是没选模型"。
        if model.trim().is_empty() {
            return Err(ConfigError::Parse(format!(
                "「{}」还没有选中模型。在设置里添加一个模型并点选，或在输入框的模型菜单里选择。",
                p.name
            )));
        }
        Ok(ResolvedModel {
            protocol: p.protocol,
            base_url: p.base_url.clone(),
            api_key_env: p.api_key_env.clone(),
            model: model.trim().to_owned(),
            fallback_model: p.fallback_model.clone(),
            sampling: p.sampling,
        })
    }

    /// 每个 provider 的 key 状态。前端据此渲染，key 本身不出宿主。
    pub fn key_status(&self) -> HashMap<String, String> {
        self.providers
            .iter()
            .filter_map(|p| p.key_source().map(|s| (p.id.clone(), s.to_owned())))
            .collect()
    }
}

/// 会话持有的运行配置快照。
///
/// 会话创建后改设置不影响它 —— 正在跑的轮子换模型会让"这个回答是谁
/// 生成的"变得说不清。
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedModel {
    pub protocol: Protocol,
    pub base_url: String,
    pub api_key_env: String,
    pub model: String,
    pub fallback_model: Option<String>,
    pub sampling: Sampling,
}

impl ResolvedModel {
    pub fn api_key(&self) -> Result<String, ConfigError> {
        let auth = load_auth(&auth_path());
        if let Ok(k) = std::env::var(&self.api_key_env)
            && !k.trim().is_empty()
        {
            return Ok(k.trim().to_owned());
        }
        if let Some(k) = auth.get(&self.api_key_env)
            && !k.trim().is_empty()
        {
            return Ok(k.trim().to_owned());
        }
        Err(ConfigError::MissingKey {
            var: self.api_key_env.clone(),
        })
    }

    pub fn is_anthropic(&self) -> bool {
        self.protocol == Protocol::Anthropic
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("没有找到 API key。在设置里粘贴，或设置环境变量 {var}。")]
    MissingKey { var: String },
    #[error("读配置失败：{0}")]
    Io(String),
    #[error("配置格式错误：{0}")]
    Parse(String),
}

/// 前端能看到的配置状态。**不含 key 本身**，只说每个 provider 有没有、从哪来。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigStatus {
    pub config: AppConfig,
    /// provider id → `"env"` / `"saved"`。没配 key 的 provider 不出现。
    pub key_status: HashMap<String, String>,
    pub config_path: String,
    /// 本次启动时配置读不懂，原文件被挪到了这里。正常启动是 `None`。
    ///
    /// 一定要让前端看得见。只写日志的话，桌面应用的用户根本不会去看，
    /// 他看到的就是"我配的东西全没了"，而旁边其实躺着一份完好的备份。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_backup: Option<String>,
}

impl ConfigStatus {
    pub fn of(config: AppConfig) -> Self {
        Self {
            key_status: config.key_status(),
            config_path: config_path().display().to_string(),
            config_backup: RECOVERED_BACKUP
                .get()
                .map(|p| p.display().to_string()),
            config,
        }
    }
}

/// 启动时配置读不懂的话，备份文件的位置记在这里。
///
/// 用全局而不是往 `load` 的返回值和各层签名里加参数：它是**一次性的
/// 启动事实**，读一次、全程不变，穿透五层函数只为传一个 Option 不划算。
static RECOVERED_BACKUP: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub fn config_path() -> PathBuf {
    dirs_config()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("riot")
        .join("config.json")
}

/// 密钥文件。和 `config.json` 同目录但分开存 —— 分享配置时不至于连密钥一起分享。
pub fn auth_path() -> PathBuf {
    dirs_config()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("riot")
        .join("auth.json")
}

/// 读 auth.json。格式：`{ "DEEPSEEK_API_KEY": "sk-..." }`。
/// 读不到或格式坏了都当空 —— 密钥文件损坏的正确表现是"提示重新输入"，不是崩溃。
fn load_auth(p: &Path) -> HashMap<String, String> {
    #[allow(clippy::disallowed_methods)]
    let Ok(raw) = std::fs::read_to_string(p) else {
        return HashMap::new();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

/// 保存（或删除，`key` 为空时）一个密钥。
pub fn save_key(env_name: &str, key: &str) -> Result<(), ConfigError> {
    save_key_at(&auth_path(), env_name, key)
}

fn save_key_at(p: &Path, env_name: &str, key: &str) -> Result<(), ConfigError> {
    let mut auth = load_auth(p);
    if key.trim().is_empty() {
        auth.remove(env_name);
    } else {
        auth.insert(env_name.to_owned(), key.trim().to_owned());
    }

    if let Some(d) = p.parent() {
        #[allow(clippy::disallowed_methods)]
        std::fs::create_dir_all(d).map_err(|e| ConfigError::Io(e.to_string()))?;
    }
    let json = serde_json::to_string_pretty(&auth).map_err(|e| ConfigError::Parse(e.to_string()))?;
    #[allow(clippy::disallowed_methods)]
    std::fs::write(p, json).map_err(|e| ConfigError::Io(e.to_string()))?;

    // [约束] 密钥文件必须只有本人可读。写完再 chmod 有一个先宽后严的窗口，
    // 但文件在用户自己的配置目录里，这个窗口可以接受；换成 O_CREAT 带 mode
    // 的写法要绕开 std::fs::write，为这个窗口不值得。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        #[allow(clippy::disallowed_methods)]
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| ConfigError::Io(e.to_string()))?;
    }
    Ok(())
}

fn dirs_config() -> Option<PathBuf> {
    #[allow(clippy::disallowed_methods)]
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME")
        && !x.is_empty()
    {
        return Some(PathBuf::from(x));
    }
    #[allow(clippy::disallowed_methods)]
    let home = std::env::var("HOME").ok()?;
    #[cfg(target_os = "macos")]
    return Some(PathBuf::from(home).join("Library/Application Support"));
    #[cfg(not(target_os = "macos"))]
    return Some(PathBuf::from(home).join(".config"));
}

// ── 加载 / 迁移 ──────────────────────────────────────────────

/// v1 配置（单模型结构）。只为迁移保留。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyConfig {
    provider: String,
    model: String,
    base_url: String,
    api_key_env: String,
    #[serde(default)]
    projects: Vec<String>,
    #[serde(default)]
    default_mode: Option<riot_protocol::permission::PermissionMode>,
    #[serde(default)]
    fallback_model: Option<String>,
    #[serde(default)]
    max_output_tokens: Option<u32>,
}

pub fn load() -> AppConfig {
    let (config, backup) = load_at(&config_path());
    if let Some(b) = backup {
        note_recovered(b);
    }
    config
}

/// 记下"这次启动是从损坏的配置里恢复的"，让前端能提示。
///
/// set 失败只可能是已经记过一次，后一次没有新信息。
pub fn note_recovered(backup: PathBuf) {
    let _ = RECOVERED_BACKUP.set(backup);
}

/// 从指定路径读配置。返回值第二项是备份文件的位置（没备份就是 `None`）。
///
/// 路径参数化是为了能测 —— 直接测 [`load`] 会读写开发机上真实的
/// `config.json`，那种测试跑一次就吃掉一份用户数据。
pub fn load_at(p: &Path) -> (AppConfig, Option<PathBuf>) {
    #[allow(clippy::disallowed_methods)]
    let Ok(raw) = std::fs::read_to_string(p) else {
        // 文件不存在是全新安装，不是损坏，不用备份。
        return (AppConfig::default(), None);
    };
    match try_parse(&raw) {
        Some(c) => (c, None),
        None => (AppConfig::default(), backup_unreadable(p)),
    }
}

/// 把读不懂的配置挪到旁边，再让应用用默认值继续。
///
/// `[约束]` 必须在回落到默认值**之前**把原文件挪走。
///
/// 回落本身是对的 —— 用户改坏一个逗号就打不开应用，那是最恼火的一类
/// 失败。但回落之后应用照常运行，而**下一次任何保存都会把默认值写回
/// 同一个路径**，原文件就永久没了。用户经历的是"我配的东西全没了"，
/// 且没有任何一步提示过他。
///
/// 不覆盖已有的备份。第二次损坏时，此刻的"原文件"很可能已经是上次
/// 回落出来的空默认值 —— 拿它盖掉第一次那份真正有内容的备份，等于把
/// 唯一一份能捞的数据也删了。
fn backup_unreadable(p: &Path) -> Option<PathBuf> {
    // 用序号而不是时间戳:不需要注入 Clock，而且文件名可预期、能断言。
    let bak = free_backup_path(p)?;
    #[allow(clippy::disallowed_methods)]
    match std::fs::rename(p, &bak) {
        Ok(()) => {
            tracing::warn!(backup = %bak.display(), "配置解析失败，原文件已备份，本次用默认值启动");
            Some(bak)
        }
        Err(e) => {
            // 挪不动就别硬来。留着坏文件比"既读不出来又被覆盖"强 ——
            // 至少下次启动它还在原地，用户还能自己打开看。
            tracing::error!(error = %e, "配置解析失败且备份不成，保留原文件");
            None
        }
    }
}

/// 找一个还没被占用的备份文件名：`config.json.bak`、`.1.bak`、`.2.bak`……
///
/// 全都占满时返回 `None` —— 那种情况下宁可保留坏文件不动，也不要挑一个
/// 已有备份盖掉。备份的全部意义就是"没被覆盖过的那一份"。
fn free_backup_path(p: &Path) -> Option<PathBuf> {
    const MAX_BACKUPS: u32 = 100;
    let first = p.with_extension("json.bak");
    if !first.exists() {
        return Some(first);
    }
    (1..MAX_BACKUPS)
        .map(|n| p.with_extension(format!("json.{n}.bak")))
        .find(|c| !c.exists())
}

/// 新格式优先；失败再按 v1 解析并迁移。两个都读不懂返回 `None`。
fn try_parse(raw: &str) -> Option<AppConfig> {
    // "providers" 字段是两代格式的可靠判别：v1 没有它。
    // 不能单靠 serde 试错 —— v1 的字段在 v2 里全是可选/缺失，
    // 反过来 v2 的 JSON 也可能碰巧含有 v1 要的键。
    if let Ok(c) = serde_json::from_str::<AppConfig>(raw) {
        return Some(normalize(c));
    }
    if let Ok(old) = serde_json::from_str::<LegacyConfig>(raw) {
        tracing::info!("检测到 v1 配置，迁移到 provider 列表格式");
        return Some(normalize(migrate(old)));
    }
    None
}

/// 解析，读不懂就用默认值。只给测试和不关心损坏的调用方用。
#[cfg(test)]
fn parse(raw: &str) -> AppConfig {
    try_parse(raw).unwrap_or_default()
}

/// 顶层 sampling（废弃的"全局参数"）搬进当时激活的 provider。
/// 全局值优先：它是用户显式设置的，而 provider 上此时只可能有
/// 内置默认值 —— 用户数据不能被出厂默认盖掉。
fn normalize(mut c: AppConfig) -> AppConfig {
    if !c.sampling.is_empty() {
        let global = c.sampling;
        let active = c.active_provider.clone();
        if let Some(p) = c.providers.iter_mut().find(|p| p.id == active) {
            p.sampling = global.or(p.sampling);
        }
        c.sampling = Sampling::default();
    }
    // 夹在合理区间。config.json 是用户能手改的文件，0 会让每个弹窗
    // 瞬间超时（等于静默拒绝一切），过大的值等于回到"任务永远卡住"。
    c.ask_timeout_secs = c.ask_timeout_secs.clamp(MIN_ASK_TIMEOUT_SECS, MAX_ASK_TIMEOUT_SECS);
    c
}

/// 弹窗至少要留 5 秒 —— 再短用户根本来不及读完就没了。
const MIN_ASK_TIMEOUT_SECS: u32 = 5;
/// 上限一小时。超过这个数和"永不超时"没有实际区别。
const MAX_ASK_TIMEOUT_SECS: u32 = 3600;

/// v1 → v2：旧的单模型配置变成 provider 列表里的一项并设为激活。
///
/// 只搬用户自己配过的那一家,**不附赠任何预设**。迁移的职责是不丢东西,
/// 不是趁机塞几个出厂选项 —— 那些名字会过期,见 [`AppConfig::default`]。
fn migrate(old: LegacyConfig) -> AppConfig {
    let protocol = if old.provider.eq_ignore_ascii_case("anthropic") {
        Protocol::Anthropic
    } else {
        Protocol::Openai
    };

    let providers = vec![ProviderConfig {
        id: old.provider.clone(),
        name: old.provider.clone(),
        protocol,
        base_url: old.base_url,
        api_key_env: old.api_key_env,
        models: vec![old.model.clone()],
        fallback_model: old.fallback_model,
        sampling: Sampling::default(),
    }];

    AppConfig {
        providers,
        active_provider: old.provider,
        active_model: old.model,
        sampling: Sampling {
            max_output_tokens: old.max_output_tokens,
            ..Default::default()
        },
        projects: old.projects,
        default_mode: old.default_mode,
        ask_timeout_secs: default_ask_timeout_secs(),
        // 老格式里没有联网配置，用默认值（抓取开、搜索关）。
        web: WebConfig::default(),
    }
}

pub fn save(c: &AppConfig) -> Result<(), ConfigError> {
    save_at(&config_path(), c)
}

/// 写到指定路径。
///
/// `[约束]` 任何在测试里可能被调到的写路径，都必须能指到临时目录。
///
/// 这不是洁癖。`AppState::remove_project` 原先直接调 [`save`]，而那个
/// 方法有单元测试 —— 于是**每跑一次 `cargo test`，开发机上真实的
/// `config.json` 就被测试里那份空配置覆盖一次**。表现是"应用一重启
/// 我配的东西就没了"，而实际元凶是测试，排查时根本不会往那儿想。
pub fn save_at(p: &Path, c: &AppConfig) -> Result<(), ConfigError> {
    if let Some(d) = p.parent() {
        #[allow(clippy::disallowed_methods)]
        std::fs::create_dir_all(d).map_err(|e| ConfigError::Io(e.to_string()))?;
    }
    let json = serde_json::to_string_pretty(c).map_err(|e| ConfigError::Parse(e.to_string()))?;
    #[allow(clippy::disallowed_methods)]
    std::fs::write(p, json).map_err(|e| ConfigError::Io(e.to_string()))
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;

    /// 造一个配好一家服务方的配置。测"配置好之后"的行为用它。
    fn one_provider() -> AppConfig {
        AppConfig {
            providers: vec![ProviderConfig {
                id: "acme".into(),
                name: "Acme".into(),
                protocol: Protocol::Openai,
                base_url: "https://api.acme.test".into(),
                api_key_env: "ACME_API_KEY".into(),
                models: vec!["m1".into()],
                fallback_model: None,
                sampling: Sampling::default(),
            }],
            active_provider: "acme".into(),
            active_model: "m1".into(),
            ..Default::default()
        }
    }

    #[test]
    fn 默认配置不预置任何服务方() {
        // `[约束]` 出厂不带服务方和模型名。预置过的那份数据会随厂商更新
        // 过期，变成"选得中、一发请求就 400"的死选项，而用户会把它当成
        // Riot 支持的清单。没有比过期的有更诚实。
        let c = AppConfig::default();
        assert!(c.providers.is_empty(), "不该预置服务方：{:?}", c.providers);
        assert!(c.active_provider.is_empty());
        assert!(c.active_model.is_empty());
    }

    #[test]
    fn 没配服务方时报错要说人话() {
        // 「找不到 provider「」」等于没说。用户需要知道下一步点哪儿。
        let msg = AppConfig::default().resolve().unwrap_err().to_string();
        assert!(msg.contains("设置"), "报错要指路，实际：{msg}");
        assert!(!msg.contains("「」"), "空名字不该出现在报错里：{msg}");
    }

    /// 临时配置文件路径。`load_at` 会改名，所以每个用例得用独立的一份。
    fn temp_cfg(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("riot-cfg-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).expect("建临时目录");
        d.join("config.json")
    }

    #[test]
    fn 读不懂的配置会被备份而不是被覆盖() {
        // `[约束]` 这是一条数据安全线。回落到默认值本身是对的（改坏一个
        // 逗号不该打不开应用），但回落之后应用照常跑，下一次保存就把默认
        // 值写回同一个路径 —— 用户配的东西永久消失，且全程没有任何提示。
        let p = temp_cfg("broken");
        std::fs::write(&p, "{ 这不是 JSON").expect("写坏文件");

        let (c, bak) = load_at(&p);
        assert!(c.providers.is_empty(), "读不懂就用默认值继续");

        let bak = bak.expect("必须备份");
        assert!(bak.exists(), "备份文件要真的在");
        assert!(!p.exists(), "原路径要腾空，否则下次保存还是覆盖它");
        assert_eq!(
            std::fs::read_to_string(&bak).expect("读备份"),
            "{ 这不是 JSON",
            "备份必须是原样，不能是被 serde 处理过的东西"
        );
        std::fs::remove_file(&bak).ok();
    }

    #[test]
    fn 第二次损坏不会盖掉第一次的备份() {
        // 第二次损坏时，"原文件"很可能已经是上次回落出来的空默认值。
        // 拿它盖掉第一份真正有内容的备份，等于把唯一能捞的数据也删了。
        let p = temp_cfg("twice");
        std::fs::write(&p, "第一份：有用户数据").expect("写");
        let first = load_at(&p).1.expect("第一次备份");

        std::fs::write(&p, "第二份：已经没内容了").expect("再写");
        let second = load_at(&p).1.expect("第二次备份");

        assert_ne!(first, second, "两次备份不能是同一个文件名");
        assert_eq!(
            std::fs::read_to_string(&first).expect("读第一份"),
            "第一份：有用户数据",
            "第一份备份必须原封不动"
        );
        std::fs::remove_file(&first).ok();
        std::fs::remove_file(&second).ok();
    }

    #[test]
    fn 文件不存在是全新安装不是损坏() {
        // 别给新用户平白无故留一个 .bak
        let p = temp_cfg("absent").with_file_name("nope.json");
        let (c, bak) = load_at(&p);
        assert!(c.providers.is_empty());
        assert!(bak.is_none(), "没有文件就没有东西可备份");
    }

    #[test]
    fn 能读懂的配置不会被动() {
        let p = temp_cfg("good");
        let raw = r#"{"providers":[],"activeProvider":"","activeModel":"","projects":["/w"]}"#;
        std::fs::write(&p, raw).expect("写");

        let (c, bak) = load_at(&p);
        assert_eq!(c.projects, vec!["/w"]);
        assert!(bak.is_none(), "正常配置不该产生备份");
        assert!(p.exists(), "原文件必须还在原地");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn 空配置照样能保存() {
        // 删掉最后一家服务方之后就是这个状态。校验必须放行 ——
        // 拒绝的话用户永远删不掉最后一个。
        assert!(AppConfig::default().validate().is_ok());
    }

    #[test]
    fn 配置里不含_key_字段() {
        // `[约束]` 序列化出来的东西会被同步、截图、贴进 issue
        let json = serde_json::to_string(&one_provider()).expect("序列化");
        let lower = json.to_lowercase();
        assert!(
            !lower.contains("\"apikey\""),
            "配置结构里出现了 key 字段：{json}"
        );
        assert!(lower.contains("apikeyenv"), "应该只存变量名");
    }

    #[test]
    fn 没有联网段的老配置照常能读() {
        // 每个已经在用的人升级上来都会走这条路。读不出来就是打开应用
        // 发现 provider 全没了 —— 比任何功能缺失都严重。
        let c = parse(
            r#"{"providers":[{"id":"deepseek","name":"DeepSeek","protocol":"openai",
                "baseUrl":"https://api.deepseek.com","apiKeyEnv":"DEEPSEEK_API_KEY",
                "models":["deepseek-chat"]}],
                "activeProvider":"deepseek","activeModel":"deepseek-chat","projects":[]}"#,
        );

        assert_eq!(c.active_model, "deepseek-chat");
        assert!(c.web.fetch_enabled, "抓取默认开");
        assert!(!c.web.search_enabled, "搜索默认关 —— 没填地址时开着只会调一次失败一次");
        assert!(!c.web.search_ready());
    }

    #[test]
    fn 辅助模型的provider和model要拆得开() {
        let t = |s: &str| {
            WebConfig {
                distill_model: s.into(),
                ..Default::default()
            }
            .distill_target()
            .map(|(p, m)| (p.to_owned(), m.to_owned()))
        };
        assert_eq!(t("deepseek/deepseek-chat"), Some(("deepseek".into(), "deepseek-chat".into())));
        // 模型名里带斜杠是常态（ollama、各家中转），只能按第一个斜杠拆
        assert_eq!(t("ollama/qwen2.5/7b"), Some(("ollama".into(), "qwen2.5/7b".into())));
        assert_eq!(t(""), None);
        assert_eq!(t("没有斜杠"), None);
        assert_eq!(t("/只有模型"), None);
        assert_eq!(t("只有provider/"), None);
    }

    #[test]
    fn active_指向不存在的_provider_报错() {
        let c = AppConfig {
            active_provider: "ghost".into(),
            ..Default::default()
        };
        assert!(c.resolve().is_err());
        assert!(c.validate().is_err(), "坏 provider 连保存都不该过");
    }

    #[test]
    fn 空模型名_resolve_拦截但_validate_放行() {
        // 空模型名发出去是各家措辞不一的 400，用户看不出根因。
        // 但它是"刚添加 provider 还没配模型"的合法中间状态，保存要放行。
        let c = AppConfig {
            active_model: "   ".into(),
            ..one_provider()
        };
        let e = c.resolve().expect_err("空模型必须在发请求前拦下");
        assert!(e.to_string().contains("还没有选中模型"), "要说人话：{e}");
        assert!(c.validate().is_ok(), "设置页的中间状态不该被拒绝保存");
    }

    #[test]
    fn v1_配置能迁移() {
        let v1 = r#"{
            "provider": "deepseek",
            "model": "deepseek-chat",
            "baseUrl": "https://my-proxy.example.com",
            "apiKeyEnv": "DEEPSEEK_API_KEY",
            "projects": ["/work/a"],
            "maxOutputTokens": 4096
        }"#;
        let c = parse(v1);
        assert_eq!(c.active_provider, "deepseek");
        assert_eq!(c.active_model, "deepseek-chat");
        // 用户改过的 URL 要保住
        assert_eq!(
            c.provider("deepseek").expect("有 deepseek").base_url,
            "https://my-proxy.example.com"
        );
        assert_eq!(c.projects, vec!["/work/a"]);
        // v1 里用户设的 max_output_tokens 落在激活的 provider 上
        assert_eq!(
            c.provider("deepseek").expect("有 deepseek").sampling.max_output_tokens,
            Some(4096)
        );
        // 迁移只搬用户配过的那一家，不附赠出厂预设
        assert_eq!(c.providers.len(), 1, "迁移不该凭空多出服务方：{:?}", c.providers);
    }

    #[test]
    fn v1_自定义服务方迁移成新_provider() {
        let v1 = r#"{
            "provider": "openai",
            "model": "qwen-max",
            "baseUrl": "https://dashscope.aliyuncs.com/compatible-mode",
            "apiKeyEnv": "DASHSCOPE_API_KEY"
        }"#;
        let c = parse(v1);
        assert_eq!(c.active_provider, "openai");
        assert_eq!(c.active_model, "qwen-max");
        let p = c.provider("openai").expect("迁移出的 provider");
        assert_eq!(p.protocol, Protocol::Openai);
        assert_eq!(p.models, vec!["qwen-max".to_owned()]);
    }

    #[test]
    fn 坏配置用默认值() {
        let c = parse("{oops");
        assert_eq!(c, AppConfig::default());
    }

    #[test]
    fn v2_配置直接解析不走迁移() {
        let v2 = serde_json::to_string(&AppConfig {
            active_model: "deepseek-reasoner".into(),
            ..Default::default()
        })
        .expect("序列化");
        assert_eq!(parse(&v2).active_model, "deepseek-reasoner");
    }

    #[test]
    fn 缺_key_的报错只提变量名() {
        let p = ProviderConfig {
            id: "x".into(),
            name: "x".into(),
            protocol: Protocol::Openai,
            base_url: "https://x".into(),
            api_key_env: "DEFINITELY_NOT_SET_XYZ".into(),
            models: vec![],
            fallback_model: None,
            sampling: Sampling::default(),
        };
        let e = p.api_key().expect_err("应该缺失");
        assert!(e.to_string().contains("DEFINITELY_NOT_SET_XYZ"));
    }

    #[test]
    fn 空白的_key_算缺失() {
        // 只有空格的环境变量比没设置更难排查 —— 请求会以 401 失败，
        // 而用户确信自己"已经设了"
        unsafe { std::env::set_var("RIOT_TEST_BLANK", "   ") };
        let p = ProviderConfig {
            id: "x".into(),
            name: "x".into(),
            protocol: Protocol::Openai,
            base_url: "https://x".into(),
            api_key_env: "RIOT_TEST_BLANK".into(),
            models: vec![],
            fallback_model: None,
            sampling: Sampling::default(),
        };
        assert!(p.api_key().is_err());
        unsafe { std::env::remove_var("RIOT_TEST_BLANK") };
    }

    fn temp_auth() -> PathBuf {
        std::env::temp_dir().join(format!(
            "riot-auth-test-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ))
    }

    fn provider_with_env(env: &str) -> ProviderConfig {
        ProviderConfig {
            id: "t".into(),
            name: "t".into(),
            protocol: Protocol::Openai,
            base_url: "https://x".into(),
            api_key_env: env.into(),
            models: vec![],
            fallback_model: None,
            sampling: Sampling::default(),
        }
    }

    #[test]
    fn 保存的_key_能读回来() {
        let p = temp_auth();
        save_key_at(&p, "TEST_SAVED_KEY", "  sk-saved\n").expect("保存");
        let c = provider_with_env("TEST_SAVED_KEY");
        assert_eq!(c.api_key_in(&load_auth(&p)).expect("有 key"), "sk-saved");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn 环境变量优先于存档() {
        // 显式的临时覆盖应该赢过存档，和几乎所有工具的 env > config 约定一致
        let p = temp_auth();
        save_key_at(&p, "RIOT_TEST_PRECEDENCE", "sk-from-file").expect("保存");
        unsafe { std::env::set_var("RIOT_TEST_PRECEDENCE", "sk-from-env") };
        let c = provider_with_env("RIOT_TEST_PRECEDENCE");
        assert_eq!(c.api_key_in(&load_auth(&p)).expect("有 key"), "sk-from-env");
        unsafe { std::env::remove_var("RIOT_TEST_PRECEDENCE") };
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn 空字符串删除存档的_key() {
        let p = temp_auth();
        save_key_at(&p, "TEST_DELETE_KEY", "sk-x").expect("保存");
        save_key_at(&p, "TEST_DELETE_KEY", "  ").expect("删除");
        assert!(!load_auth(&p).contains_key("TEST_DELETE_KEY"));
        std::fs::remove_file(&p).ok();
    }

    #[cfg(unix)]
    #[test]
    fn 密钥文件是_0600() {
        use std::os::unix::fs::PermissionsExt;
        let p = temp_auth();
        save_key_at(&p, "TEST_PERM_KEY", "sk-x").expect("保存");
        let mode = std::fs::metadata(&p).expect("metadata").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "auth.json 权限是 {mode:o}，应该是 600");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn 损坏的_auth_文件当作空() {
        let p = temp_auth();
        std::fs::write(&p, "{oops").expect("写坏文件");
        assert!(load_auth(&p).is_empty());
        // 还能在坏文件之上继续保存
        save_key_at(&p, "TEST_RECOVER", "sk-x").expect("覆盖保存");
        assert_eq!(load_auth(&p).get("TEST_RECOVER").map(String::as_str), Some("sk-x"));
        std::fs::remove_file(&p).ok();
    }
}
