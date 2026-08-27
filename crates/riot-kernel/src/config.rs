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
    /// 接口主机（可以带前缀路径），如 `https://api.deepseek.com`。
    pub base_url: String,
    /// 接口路径，如 `/v1/chat/completions`。
    ///
    /// 空 = 按主机猜（见 `riot_providers::endpoint::api_url`）。
    ///
    /// `[取舍]` 让用户能填，而不是全靠猜。猜的规则已经踩过两次:智谱的对话在
    /// `/api/paas/v4/chat/completions`（带 `/v1` 就 404），而它的完整模型清单
    /// 偏偏在 `/api/paas/v4/v1/models`。中转和自建网关的花样只会更多，而猜错
    /// 的表现是一个 404 —— 那个报错里没有任何线索指向路径。
    ///
    /// 默认留空:大多数人不需要关心这件事，填了才走这条。
    #[serde(default)]
    pub api_path: String,
    /// 读 key 的环境变量名，同时是 `auth.json` 里的存储键。
    pub api_key_env: String,
    /// 已添加的模型（手动输入或从 `/models` 接口挑选保存）。
    #[serde(default)]
    pub models: Vec<ModelConfig>,
    /// 过载时降级到的模型。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_model: Option<String>,
    /// 这个服务方的采样参数。会话可以临时覆盖单个字段。
    #[serde(default)]
    pub sampling: Sampling,
    /// **已废弃**：视觉能力按模型记（[`ModelConfig::vision`]）。字段保留只为
    /// 读懂短暂存在过的"按服务方"格式 —— 加载时 [`normalize`] 会把它铺到这个
    /// 服务方的所有模型上然后清空。
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) vision: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// 一个模型的配置。
///
/// `[约束]` 能力和采样参数属于**模型**，不属于服务方。同一家同时有视觉模型和
/// 纯文本模型是常态 —— 智谱的 `glm-4.6v` 能看图、`glm-5.2` 不能。按服务方记
/// 的话，为了把前者配成视觉兼容模型就得给整家打开，于是和后者聊天时截图也被
/// 当成图片发出去，服务方回一句
/// `messages.content.type 参数非法，取值范围['text']` —— 而那句话完全不指向
/// "是那张截图的事"。这个坑真实踩过一次。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfig {
    /// 发给服务方的模型名。
    pub id: String,
    /// 显示名。空 = 直接显示 `id`。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// 能收图片。
    ///
    /// 默认关:多数国内的对话模型是纯文本的，猜"支持"的代价是每次带图的请求
    /// 都被服务方拒，而报错不指向这个开关。
    #[serde(default, skip_serializing_if = "is_false")]
    pub vision: bool,
    /// 上下文窗口有多大（token）。`None` = 没填，压缩阈值走全局那个数
    /// （[`AppConfig::compact_threshold_tokens`]）。
    ///
    /// `[取舍]` 让用户填**窗口**而不是直接填压缩阈值。窗口是模型文档第一页
    /// 就写着的客观数字，用户查得到也记得住；阈值要先知道"得给回复留多少、
    /// 给总结留多少、压完还要再跑一轮"才填得对 —— 那是内部机制，不该让用户
    /// 去推。填了窗口，阈值由 [`compact_threshold_for_window`] 算出来。
    ///
    /// 按模型记的理由和 [`vision`] 一样:同一家的窗口能差一个数量级，
    /// 按服务方记就得取最小的那个，于是大窗口模型白白早压好几轮。
    ///
    /// [`vision`]: ModelConfig::vision
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    /// 这个模型的采样参数。空字段继承 provider 的设置。
    #[serde(default, skip_serializing_if = "Sampling::is_empty")]
    pub sampling: Sampling,
}

impl ModelConfig {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: String::new(),
            vision: false,
            context_window: None,
            sampling: Sampling::default(),
        }
    }

    /// 界面上显示什么。
    pub fn label(&self) -> &str {
        if self.name.trim().is_empty() {
            &self.id
        } else {
            &self.name
        }
    }
}

/// `[约束]` 手写反序列化，为了同时读得懂两种形状:老配置里 `models` 是字符串
/// 数组（`["glm-5.2"]`），新配置是对象数组。
///
/// 不兼容的后果不是"少了几个开关"，而是**整份配置解析失败** —— 用户升级之后
/// 看到的是"我配的服务方、key、模型全没了"，而真正的原因只是这个字段换了形状。
impl<'de> Deserialize<'de> for ModelConfig {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Full {
            id: String,
            #[serde(default)]
            name: String,
            #[serde(default)]
            vision: bool,
            #[serde(default)]
            context_window: Option<u32>,
            #[serde(default)]
            sampling: Sampling,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            /// 老格式：只有模型名。
            Name(String),
            Full(Full),
        }

        Ok(match Raw::deserialize(d)? {
            Raw::Name(id) => Self::new(id),
            Raw::Full(f) => Self {
                id: f.id,
                name: f.name,
                vision: f.vision,
                context_window: f.context_window,
                sampling: f.sampling,
            },
        })
    }
}

impl ProviderConfig {
    /// 按模型名找配置。
    pub fn model(&self, id: &str) -> Option<&ModelConfig> {
        self.models.iter().find(|m| m.id == id)
    }
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

/// 内置 SearXNG。空地址走这里。
///
/// `[约束]` 这个常量不得出现在设置页、`config.json`、工具回传和错误文案里。
/// 用户覆盖写成自己的地址；没覆盖时界面只说"内置搜索"。
pub const BUILTIN_SEARXNG_URL: &str = "https://searxng.riotai.app";

/// 写入配置用：内置域名收成空串，避免出现在 `config.json` 和设置页。
pub fn normalize_searxng_url(raw: &str) -> String {
    let t = raw.trim().trim_end_matches('/');
    if t.is_empty() || is_builtin_searxng_url(t) {
        String::new()
    } else {
        t.to_owned()
    }
}

/// 真正发请求用的地址。空或内置域名 → 内置实例。
pub fn resolve_searxng_url(raw: &str) -> String {
    let saved = normalize_searxng_url(raw);
    if saved.is_empty() {
        BUILTIN_SEARXNG_URL.to_owned()
    } else {
        saved
    }
}

pub fn is_builtin_searxng_url(url: &str) -> bool {
    let t = url.trim().trim_end_matches('/');
    let rest = t
        .strip_prefix("https://")
        .or_else(|| t.strip_prefix("http://"))
        .unwrap_or(t);
    rest.eq_ignore_ascii_case("searxng.riotai.app")
}

/// 给用户 / 模型看的名字。内置实例不暴露域名。
pub fn searxng_error_label(url: &str) -> String {
    if is_builtin_searxng_url(url) || url.trim().is_empty() {
        "内置搜索".into()
    } else {
        url.trim().trim_end_matches('/').to_owned()
    }
}

/// 错误文案里万一带上了内置地址，换成"内置搜索"。
pub fn redact_searxng_url(msg: impl AsRef<str>) -> String {
    msg.as_ref()
        .replace(BUILTIN_SEARXNG_URL, "内置搜索")
        .replace("http://searxng.riotai.app", "内置搜索")
}

fn deserialize_searxng_url<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let s = String::deserialize(d)?;
    Ok(normalize_searxng_url(&s))
}

/// 联网能力的配置。
///
/// 抓取（WebFetch）和搜索（WebSearch）分开开关：抓取不需要任何第三方
/// 服务，配好就能用；搜索默认走内置 SearXNG，用户也可以填自己的实例覆盖。
/// "只让模型读我贴过来的链接、别自己去搜"是一种合理的用法，两个开关
/// 合并就表达不出来。
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
    /// 允许 WebSearch 搜索。关掉时工具会提示去设置里打开。
    #[serde(default = "yes")]
    pub search_enabled: bool,
    /// 用户覆盖的 SearXNG 地址。空 = 用内置实例。
    ///
    /// `[约束]` 这个地址**不过** SSRF 检查 —— 自托管实例跑在
    /// `127.0.0.1` 是最常见的覆盖方式，套上抓取工具那层内网拦截会让它
    /// 完全没法用。这不是破例：安全边界在于它是用户亲手填的一个固定
    /// 地址，模型影响不了它，而模型能影响的 `q` 参数是被 URL 编码过的。
    ///
    /// `[约束]` 内置域名读进来就收成空，禁止写回 `config.json`。
    #[serde(default, deserialize_with = "deserialize_searxng_url")]
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

/// 一个 MCP 服务器（stdio 传输：命令 + 参数 + 环境变量）。
///
/// `[约束]` `id` 进工具名（`mcp__<id>__…`）和权限规则，改了它等于换了
/// 一批工具名 —— 用户点过的"总是允许"全部失配。界面上要提示这一点。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    /// 稳定标识。只允许字母数字、`-`、`_` —— 别的字符会在工具名里被
    /// 替换成 `_`，两个只差一个点的 id 就撞名了。
    pub id: String,
    /// 显示名。空 = 显示 id。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// 启动命令，如 `npx`、`uvx`、或一个可执行文件的绝对路径。
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// 附加环境变量（API key 之类）。BTreeMap 保证序列化顺序稳定 ——
    /// config.json 是用户会看、会 diff 的文件。
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub env: std::collections::BTreeMap<String, String>,
    /// 关掉 = 进程停掉、工具消失，但配置留着。
    #[serde(default = "yes")]
    pub enabled: bool,
}

impl McpServerConfig {
    pub fn label(&self) -> &str {
        if self.name.trim().is_empty() {
            &self.id
        } else {
            &self.name
        }
    }
}

// ── MCP 的标准 JSON 配置（生态通用格式） ─────────────────────
//
// Claude Desktop / Cursor / Cline / VS Code 用的是同一个形状：
//
// ```json
// { "mcpServers": { "filesystem": { "command": "npx", "args": [...], "env": {...} } } }
// ```
//
// 每个 MCP 服务器的 README 给的就是这段。支持粘贴它，而不是逼用户把
// args 一行行拆进表单。解析在宿主做 —— 格式规则只能有一份。

/// 标准格式里的一个服务器条目。
///
/// `[约束]` 未知字段**忽略而不是报错**：各家在这个形状上各有私货
/// （Cline 的 `autoApprove`、`timeout`，VS Code 的 `envFile`……），
/// 粘过来就报错的话，用户得先手工删字段才能导入。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMcpServer {
    #[serde(default)]
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: std::collections::BTreeMap<String, String>,
    /// Cline / Roo 的停用标记。
    #[serde(default)]
    disabled: bool,
    // 远程服务器的字段。认出来是为了给一句明确的"暂不支持"，
    // 而不是"缺 command"这种不指向根因的报错。
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    server_url: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    transport: Option<String>,
}

/// 把标准 JSON 解析成服务器列表。
///
/// 认三种根形状：`{"mcpServers": {...}}`（Claude Desktop / Cursor / Cline）、
/// `{"servers": {...}}`（VS Code）、以及不带包装的裸映射 —— README 里
/// 三种都常见。
pub fn mcp_servers_from_json(raw: &str) -> Result<Vec<McpServerConfig>, ConfigError> {
    let root: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| ConfigError::Parse(format!("不是合法的 JSON：{e}")))?;

    let map = root
        .get("mcpServers")
        .or_else(|| root.get("servers"))
        .unwrap_or(&root);
    let map = map.as_object().ok_or_else(|| {
        ConfigError::Parse("形状不对。期待 {\"mcpServers\": {\"名字\": {\"command\": …}}}".into())
    })?;
    if map.is_empty() {
        return Err(ConfigError::Parse("里面一个服务器都没有".into()));
    }
    // 裸映射的误判保护：如果"服务器"的值不是对象，说明用户粘的是单个
    // 服务器的内层（{"command": "npx"}），缺了名字这一层。
    if map.values().any(|v| !v.is_object()) {
        return Err(ConfigError::Parse(
            "形状不对。每个服务器要有名字：{\"mcpServers\": {\"名字\": {\"command\": …}}}".into(),
        ));
    }

    let mut servers = Vec::with_capacity(map.len());
    let mut seen = std::collections::HashSet::new();
    for (key, value) in map {
        let raw: RawMcpServer = serde_json::from_value(value.clone())
            .map_err(|e| ConfigError::Parse(format!("「{key}」解析失败：{e}")))?;

        if raw.url.is_some()
            || raw.server_url.is_some()
            || matches!(
                raw.kind.as_deref(),
                Some("http" | "sse" | "streamable-http")
            )
            || matches!(
                raw.transport.as_deref(),
                Some("http" | "sse" | "streamable-http")
            )
        {
            return Err(ConfigError::Parse(format!(
                "「{key}」是 http/sse 远程服务器，Riot 暂时只支持 stdio（command + args）"
            )));
        }
        if raw.command.trim().is_empty() {
            return Err(ConfigError::Parse(format!("「{key}」缺 command")));
        }

        // 生态里的键可以是任意字符串（"my.server"），而 id 要进工具名，
        // 字符集受限。消毒进 id，原名进显示名 —— 不改用户看到的东西。
        let id = sanitize_mcp_id(key);
        if !seen.insert(id.clone()) {
            return Err(ConfigError::Parse(format!(
                "「{key}」和另一个服务器的 id 消毒后撞名了（{id}），改一下名字"
            )));
        }
        servers.push(McpServerConfig {
            name: if id == *key {
                String::new()
            } else {
                key.clone()
            },
            id,
            command: raw.command.trim().to_owned(),
            args: raw.args,
            env: raw.env,
            enabled: !raw.disabled,
        });
    }
    Ok(servers)
}

/// 把服务器列表导出成标准 JSON（和上面互逆）。
///
/// 只写标准字段：`name` 是 Riot 的显示名，标准格式里没有这个概念，
/// 导出就丢 —— 导入侧按 id 合并时会把它捡回来。
pub fn mcp_servers_to_json(servers: &[McpServerConfig]) -> String {
    let mut map = serde_json::Map::new();
    for s in servers {
        let mut entry = serde_json::Map::new();
        entry.insert("command".into(), s.command.clone().into());
        if !s.args.is_empty() {
            entry.insert("args".into(), s.args.clone().into());
        }
        if !s.env.is_empty() {
            entry.insert(
                "env".into(),
                serde_json::Value::Object(
                    s.env
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone().into()))
                        .collect(),
                ),
            );
        }
        if !s.enabled {
            entry.insert("disabled".into(), true.into());
        }
        map.insert(s.id.clone(), serde_json::Value::Object(entry));
    }
    let root = serde_json::json!({ "mcpServers": map });
    serde_json::to_string_pretty(&root).expect("纯数据序列化不会失败")
}

/// id 消毒：和 validate_mcp 的字符集一致，别的字符换成 `-`。
fn sanitize_mcp_id(key: &str) -> String {
    let id: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if id.trim_matches('-').is_empty() {
        "server".into()
    } else {
        id
    }
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            // 抓取默认开：它不依赖任何外部服务，而且每个域名还要过一次
            // 用户确认，再加一道默认关闭的开关只是让人多找一次设置。
            fetch_enabled: true,
            // 搜索默认开：空地址走内置实例，不用先填东西。
            search_enabled: true,
            searxng_url: String::new(),
            distill_model: String::new(),
        }
    }
}

impl WebConfig {
    /// 搜索是不是真的可用。空地址走内置，所以只看开关。
    pub fn search_ready(&self) -> bool {
        self.search_enabled
    }

    /// 真正发请求的地址。空 = 内置。
    pub fn effective_searxng_url(&self) -> String {
        resolve_searxng_url(&self.searxng_url)
    }

    /// 用户有没有覆盖内置实例。
    pub fn using_custom_searxng(&self) -> bool {
        !normalize_searxng_url(&self.searxng_url).is_empty()
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
    /// 单轮任务里模型最多自主往返多少次（一次 = 调模型 + 跑它要的工具）。
    ///
    /// 到顶就停下、等用户再说一句（不是报错，是"该歇口气了"）。可配置是
    /// 因为不同任务的步数差一个量级:随手问一句几轮就够，而浏览器自动化、
    /// 渗透这类多步任务动辄几十轮，写死一个值总有一头不合适。
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    /// 联网能力（抓取 + 搜索）。
    #[serde(default)]
    pub web: WebConfig,
    /// MCP 服务器。连接是应用级的（会话共享），工具每轮快照。
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    /// 历史估算超过这个 token 数时，轮次开始前做 LLM 总结压缩。
    ///
    /// 可配置是因为窗口大小 Riot 猜不到：各家模型 32k 到 200k+ 都有，
    /// 而配置里的模型名对不上任何一张公开价目表（中转、自建、微调名）。
    /// 默认值按 128k 窗口留 ~28k 余量取的；模型窗口更小就调低。
    /// 413 反应式压缩仍然兜底 —— 这个阈值只决定"主动压"的时机。
    #[serde(default = "default_compact_threshold_tokens")]
    pub compact_threshold_tokens: u32,
    /// 视觉兼容模型，格式 `providerId/model`。
    ///
    /// 主模型收不了图片（provider 没勾 `vision`）时，用它把图片转成文字再交给
    /// 主模型。空 = 不转，截图工具会直接说"去配一下"。
    ///
    /// `[取舍]` 转述必然有损，但可选项只有三个:什么都不给（模型会自己去 shell
    /// 里截屏，然后拿着一张截错的图分析 —— 真实发生过）、报错（截图工具在半数
    /// 配置下等于不存在）、或者给一份有损但可用的描述。第三个最不坏。
    #[serde(default)]
    pub vision_model: String,
    /// 子 agent 的便宜模型，格式 `providerId/model`。空 = 跟主模型。
    ///
    /// 只有**只读侦察**类型（`explore`）会走它 —— 那类任务是"到处翻翻然后
    /// 汇报"，产出是一份文字报告，不需要主模型的推理深度，但吃掉的 token
    /// 往往比主对话还多（几十次 Grep/Read 的结果全进它的上下文）。
    ///
    /// `[取舍]` 也可以按"便宜模型跑全部子 agent"来设计，但 `general-purpose`
    /// 会改代码，那是写操作 —— 省下的钱不值得让一个更笨的模型去动文件。
    /// 成本收缩只加在只读的那一档上。
    #[serde(default)]
    pub subagent_model: String,
    /// 命令的 OS 级隔离。
    ///
    /// 默认开。这不只是安全设置 —— 决策链里"沙箱内自动放行"那一档
    /// （`bash::decide`）要它开着才成立，关掉之后每个非只读命令又回到
    /// "要么弹窗、要么全部放行"的二选一。
    ///
    /// 平台不支持时自动降级成不隔离，`sandboxed` 也跟着回 false ——
    /// 见 [`riot_runtime::SandboxPolicy::activate`]。
    #[serde(default)]
    pub sandbox: SandboxMode,
}

/// 命令隔离的强度。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SandboxMode {
    /// 读全开、写限于工作区和构建缓存、联网照常。
    #[default]
    WorkspaceWrite,
    /// 同上，另外掐掉网络。`npm install` 之类会失败，换取"数据出不去"。
    WorkspaceWriteNoNet,
    /// 不隔离。只剩策略层拦着。
    Off,
}

impl SandboxMode {
    /// 翻译成执行器认识的策略。
    pub fn policy(self, workspace: &std::path::Path) -> riot_runtime::SandboxPolicy {
        match self {
            Self::Off => riot_runtime::SandboxPolicy::Off,
            Self::WorkspaceWrite => riot_runtime::SandboxPolicy::workspace_write(workspace),
            Self::WorkspaceWriteNoNet => {
                match riot_runtime::SandboxPolicy::workspace_write(workspace) {
                    riot_runtime::SandboxPolicy::WorkspaceWrite { writable, .. } => {
                        riot_runtime::SandboxPolicy::WorkspaceWrite {
                            writable,
                            allow_network: false,
                        }
                    }
                    other => other,
                }
            }
        }
    }
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

/// 主动压缩的默认阈值：100k token。
///
/// 给**没填窗口**的模型兜底。128k 窗口减去输出预留（~16k）和总结本身要占的
/// 空间，再留一点余量 —— 也就是把 [`compact_threshold_for_window`] 对着 128k
/// 手算了一遍。填了窗口的模型不看这个数（见 [`ResolvedModel::compact_threshold`]）。
///
/// 窗口更大的模型晚点压也无妨（压缩是省钱不是保命，保命有 413 兜底）；
/// 窗口更小的模型需要用户填窗口，或在设置里调低这个默认值。
pub const fn default_compact_threshold_tokens() -> u32 {
    100_000
}

/// 单次回复要留出的空间上限。
///
/// 窗口不是全给历史的 —— 模型还得把这一轮的回复写进去。按模型自己配的
/// `max_output_tokens` 留，但不超过这个数：留得再多也只是白扔窗口。
///
/// 没配 `max_output_tokens` 时按上限留。两种猜错的代价不对等：多留了只是
/// 早压一轮（花一次总结的钱），少留了是回复写到一半撞上下文上限 —— 那一轮
/// 的输出直接废掉，而且压缩已经跑过了，反应式重试没牌可打。
const OUTPUT_RESERVE_CAP: u32 = 20_000;

/// 阈值到窗口上限之间留的缓冲。
///
/// 阈值是**开工前**判的，判完这一轮还要继续往里塞工具结果。缓冲太小的话，
/// 压缩刚跑完的那一轮就能把窗口顶穿，而这次溢出发生在压缩之后 —— 已经没有
/// 更轻的手段可用了。
const COMPACT_BUFFER: u32 = 13_000;

/// 从上下文窗口推主动压缩的阈值：窗口 − 输出预留 − 缓冲。
///
/// `[约束]` 结果不低于窗口的一半。小窗口模型（32k 及以下）减完两笔预留会
/// 归零，那等于每轮都压 —— 压缩本身要花一次模型调用，比不压更贵，而且压完
/// 立刻又超，会在压缩和重试之间转圈。
pub fn compact_threshold_for_window(window: u32, max_output: Option<u32>) -> u32 {
    let reserve = max_output.unwrap_or(OUTPUT_RESERVE_CAP).min(OUTPUT_RESERVE_CAP);
    window
        .saturating_sub(reserve)
        .saturating_sub(COMPACT_BUFFER)
        .max(window / 2)
        .clamp(MIN_COMPACT_THRESHOLD, MAX_COMPACT_THRESHOLD)
}

/// 单轮默认最多 48 次往返。
///
/// 够一次中等复杂的任务（改代码 + 跑测试 + 修，或一串浏览器操作）在一句话
/// 里跑完，又不至于让一个跑飞的循环烧太久才被兜住。多步的浏览器/渗透任务
/// 常会吃满，用户可以在设置里调高。
pub const fn default_max_turns() -> u32 {
    48
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
            max_turns: default_max_turns(),
            web: WebConfig::default(),
            mcp_servers: Vec::new(),
            compact_threshold_tokens: default_compact_threshold_tokens(),
            vision_model: String::new(),
            subagent_model: String::new(),
            sandbox: SandboxMode::default(),
        }
    }
}

impl AppConfig {
    pub fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// 当前激活的模型能不能直接收图片。
    pub fn active_takes_images(&self) -> bool {
        self.takes_images(&self.active_provider, &self.active_model)
    }

    /// 某个服务方下的某个模型能不能收图片。
    pub fn takes_images(&self, provider_id: &str, model: &str) -> bool {
        self.provider(provider_id)
            .and_then(|p| p.model(model))
            .is_some_and(|m| m.vision)
    }

    /// 视觉兼容模型，拆成 `(providerId, model)`。
    ///
    /// 主模型自己能看图时返回 `None` —— 那条路不需要转述，多走一次辅助模型
    /// 只是白花钱，而且转述比原图差。
    pub fn vision_target(&self) -> Option<(&str, &str)> {
        if self.active_takes_images() {
            return None;
        }
        let (p, m) = self.vision_model.trim().split_once('/')?;
        (!p.is_empty() && !m.is_empty()).then_some((p, m))
    }

    /// 子 agent 的便宜模型，拆成 `(providerId, model)`。
    ///
    /// 指到主模型自己时返回 `None` —— 那样 `provider_for` 会白建一个一模一样
    /// 的客户端，还会让"这轮用的是便宜档"的提示说谎。
    pub fn subagent_target(&self) -> Option<(&str, &str)> {
        let (p, m) = self.subagent_model.trim().split_once('/')?;
        if p.is_empty() || m.is_empty() {
            return None;
        }
        (p != self.active_provider || m != self.active_model).then_some((p, m))
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
            self.validate_mcp()?;
            return Ok(());
        }
        self.provider(&self.active_provider)
            .map(|_| ())
            .ok_or_else(|| {
                ConfigError::Parse(format!("找不到 provider「{}」", self.active_provider))
            })?;
        self.validate_mcp()
    }

    /// MCP 配置的保存前校验。
    ///
    /// id 在这里管严：它进工具名和权限规则，坏 id 的失败发生在几天后
    /// 的某次权限匹配上，而不是保存的那一刻 —— 那种时差 bug 最难查。
    ///
    /// `[约束]` **不校验 command 非空**。"刚点了添加、还没填命令"是设置页
    /// 的合法中间状态，和 provider 允许暂时没选模型同一条理（见
    /// [`Self::validate`] 的注释）——拒绝保存的表现是"添加按钮点了没反应"。
    /// 空命令的服务器由 reconcile 跳过，永远不会被启动。
    fn validate_mcp(&self) -> Result<(), ConfigError> {
        let mut seen = std::collections::HashSet::new();
        for s in &self.mcp_servers {
            if s.id.trim().is_empty() {
                return Err(ConfigError::Parse("MCP 服务器的 id 不能为空".into()));
            }
            if !s
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err(ConfigError::Parse(format!(
                    "MCP 服务器 id「{}」只能用字母、数字、- 和 _（它要进工具名）",
                    s.id
                )));
            }
            if !seen.insert(s.id.as_str()) {
                return Err(ConfigError::Parse(format!(
                    "MCP 服务器 id「{}」重复了",
                    s.id
                )));
            }
        }
        Ok(())
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
    pub fn resolve_named(
        &self,
        provider_id: &str,
        model: &str,
    ) -> Result<ResolvedModel, ConfigError> {
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
        let model = model.trim();
        let mc = p.model(model);
        Ok(ResolvedModel {
            protocol: p.protocol,
            base_url: p.base_url.clone(),
            api_path: p.api_path.clone(),
            api_key_env: p.api_key_env.clone(),
            model: model.to_owned(),
            fallback_model: p.fallback_model.clone(),
            // 没配就是 None，压缩阈值那边会退回全局设置。窗口没有"服务方
            // 级默认"可继承 —— 同一家的模型窗口能差一个数量级。
            context_window: mc.and_then(|m| m.context_window),
            // 模型级参数叠在服务方之上，只盖用户在模型上动过的字段。
            //
            // 顺序是 模型 → 服务方 → 服务端默认。会话覆盖再叠在这之上（见
            // state.rs 的 send_turn）—— 越具体的赢，这条链任何一环反了都
            // 会表现为"我明明设了 temperature，它没生效"。
            sampling: mc.map_or(p.sampling, |m| m.sampling.or(p.sampling)),
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
    /// 接口路径。空 = 按主机猜。
    pub api_path: String,
    pub api_key_env: String,
    pub model: String,
    pub fallback_model: Option<String>,
    /// 这个模型的上下文窗口。`None` = 用户没填。
    pub context_window: Option<u32>,
    pub sampling: Sampling,
}

impl ResolvedModel {
    /// 这一轮主动压缩的阈值。
    ///
    /// 填了窗口就按窗口推，没填就用 `fallback`（设置页那个全局数）。老配置
    /// 里一个模型都没填窗口，于是每一个都走 `fallback` —— 行为和加这个字段
    /// 之前逐字节一致。
    pub fn compact_threshold(&self, fallback: u32) -> u32 {
        self.context_window
            .map_or(fallback, |w| {
                compact_threshold_for_window(w, self.sampling.max_output_tokens)
            })
    }

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

    /// 解析成跨进程传输的端点:协议、采样转成 protocol 类型,并把明文 key
    /// 一并解析出来。拆进程后内核拿不到 auth.json,key 必须在宿主这一侧
    /// 解析完再随 RPC 传进内核(见 riot_protocol::turn 模块文档)。
    pub fn to_endpoint(&self) -> Result<riot_protocol::ModelEndpoint, ConfigError> {
        Ok(riot_protocol::ModelEndpoint {
            protocol: match self.protocol {
                Protocol::Openai => riot_protocol::ApiProtocol::Openai,
                Protocol::Anthropic => riot_protocol::ApiProtocol::Anthropic,
            },
            base_url: self.base_url.clone(),
            api_path: self.api_path.clone(),
            api_key: self.api_key()?,
            model: self.model.clone(),
            fallback_model: self.fallback_model.clone(),
            sampling: riot_protocol::EndpointSampling {
                temperature: self.sampling.temperature,
                top_p: self.sampling.top_p,
                top_k: self.sampling.top_k,
                max_output_tokens: self.sampling.max_output_tokens,
            },
        })
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
            config_backup: RECOVERED_BACKUP.get().map(|p| p.display().to_string()),
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


/// 所有会话的浏览器 profile 都放在这个目录下，一个会话一个子目录。
///
/// `[约束]` 推导规则只能有这一份。会话装配浏览器时要按它建目录、删会话时
/// 要按它删目录 —— 两处各写一遍的话，改了一处就会留下一地删不掉的孤儿，
/// 而每个孤儿是一百多 MB。
///
/// 参数化 `config_path` 的理由同 [`load_at`]：删除路径有单元测试，而它
/// 绝不能落到用户真实的目录上。
pub fn profiles_dir(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("browser-profiles")
}

/// 可下载能力包的根目录，一个包一个子目录。
///
/// `[约束]` 推导规则只能有这一份。安装、卸载、技能扫描、PATH 注入四处都要
/// 按它定位 —— 各写各的话，改一处就会出现"设置页说没装、模型却能用"这种
/// 谁也说不清的状态，而每个包是几百 MB。
///
/// 参数化 `config_path` 的理由同 [`profiles_dir`]。
pub fn packs_dir(config_path: &Path) -> PathBuf {
    config_path.parent().unwrap_or(Path::new(".")).join("packs")
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
    let json =
        serde_json::to_string_pretty(&auth).map_err(|e| ConfigError::Parse(e.to_string()))?;
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
    // Windows 的惯例位置是 %APPDATA%。不能走下面的 HOME 分支:Windows
    // 通常没有 HOME 这个变量，掉进"当前目录"兜底的话，装好的应用会把
    // config.json 和 auth.json 写进 Program Files 或 System32。
    #[cfg(windows)]
    {
        #[allow(clippy::disallowed_methods)]
        if let Ok(x) = std::env::var("APPDATA")
            && !x.is_empty()
        {
            return Some(PathBuf::from(x));
        }
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

    // 服务方级的 vision（短暂存在过的形状）铺到它的每个模型上。
    //
    // 铺开而不是丢掉:用户勾过那个开关，丢掉等于把他的设置悄悄关了 —— 而
    // 表现是"截图突然又不给模型看了"。铺开之后如果有纯文本模型被误标，
    // 他在模型行上取消一下就行，那个位置本来就是现在该看的地方。
    for p in &mut c.providers {
        if p.vision {
            for m in &mut p.models {
                m.vision = true;
            }
            p.vision = false;
        }
        // 窗口夹在合理区间。config.json 用户能手改，而少打一个 0 的表现是
        // "每轮都在压缩"，多打一个 0 是"压缩再也不触发、直接撞 413"——
        // 两种都不指向这个字段。
        for m in &mut p.models {
            m.context_window = m
                .context_window
                .map(|w| w.clamp(MIN_CONTEXT_WINDOW, MAX_CONTEXT_WINDOW));
        }
    }

    // active 指向空/幽灵 provider 而列表非空时，吸附到第一家。
    //
    // 这个状态是能被正常操作拼出来的：添加第一家服务方时 active 留空
    // （validate 放行），之后在输入框旁的模型菜单选模型只写 active_model。
    // 主界面显示用 providers[0] 兜底，key 状态却按空 id 查 —— 表现为
    // 「key 明明已保存，横幅还说没有 API key」，发送键也一直是灰的。
    // 在加载时吸附，已经写盘的坏配置下次启动就自愈。
    if c.provider(&c.active_provider).is_none()
        && let Some(first) = c.providers.first()
    {
        let id = first.id.clone();
        let model = first.models.first().map(|m| m.id.clone());
        c.active_provider = id;
        if c.active_model.is_empty()
            && let Some(m) = model
        {
            c.active_model = m;
        }
    }

    // 夹在合理区间。config.json 是用户能手改的文件，0 会让每个弹窗
    // 瞬间超时（等于静默拒绝一切），过大的值等于回到"任务永远卡住"。
    c.ask_timeout_secs = c
        .ask_timeout_secs
        .clamp(MIN_ASK_TIMEOUT_SECS, MAX_ASK_TIMEOUT_SECS);
    // 轮数同理夹一下。0 会让任何任务一开就到顶（等于什么都做不了），
    // 过大的值让跑飞的循环烧很久才被兜住。
    c.max_turns = c.max_turns.clamp(MIN_MAX_TURNS, MAX_MAX_TURNS);
    // 压缩阈值：太低会让每轮都压（一句话就超），太高等于关掉主动压缩。
    c.compact_threshold_tokens = c
        .compact_threshold_tokens
        .clamp(MIN_COMPACT_THRESHOLD, MAX_COMPACT_THRESHOLD);
    c
}

/// 阈值下限 8k：再低连一次正经的工具输出都装不下，压缩会变成每轮必发。
const MIN_COMPACT_THRESHOLD: u32 = 8_000;
/// 上限 1M：超过现有一切模型的窗口，等于"永不主动压"。
const MAX_COMPACT_THRESHOLD: u32 = 1_000_000;

/// 窗口下限沿用阈值下限 8k —— 比这更小的窗口装不下一次正经的工具输出。
const MIN_CONTEXT_WINDOW: u32 = 8_000;
/// 上限 10M：比现有任何模型都大一个数量级，只用来兜住手滑多打的那个 0。
const MAX_CONTEXT_WINDOW: u32 = 10_000_000;

/// 弹窗至少要留 5 秒 —— 再短用户根本来不及读完就没了。
const MIN_ASK_TIMEOUT_SECS: u32 = 5;
/// 上限一小时。超过这个数和"永不超时"没有实际区别。
const MAX_ASK_TIMEOUT_SECS: u32 = 3600;
/// 至少 1 轮 —— 0 轮等于什么都做不了。
const MIN_MAX_TURNS: u32 = 1;
/// 上限 1000 轮。到这个量级还没停多半是跑飞了，兜底比放任强。
const MAX_MAX_TURNS: u32 = 1000;

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
        models: vec![ModelConfig::new(old.model.clone())],
        fallback_model: old.fallback_model,
        sampling: Sampling::default(),
        vision: false,
        api_path: String::new(),
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
        max_turns: default_max_turns(),
        // 老格式里没有联网配置，用默认值（抓取开、搜索开、空地址走内置）。
        web: WebConfig::default(),
        mcp_servers: Vec::new(),
        compact_threshold_tokens: default_compact_threshold_tokens(),
        vision_model: String::new(),
        subagent_model: String::new(),
        sandbox: SandboxMode::default(),
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
                models: vec![ModelConfig::new("m1")],
                fallback_model: None,
                sampling: Sampling::default(),
                vision: false,
                api_path: String::new(),
            }],
            active_provider: "acme".into(),
            active_model: "m1".into(),
            ..Default::default()
        }
    }

    /// 老配置里 `models` 是字符串数组，必须还能读。
    ///
    /// `[约束]` 读不懂的后果不是"少了几个开关"，而是整份配置解析失败 ——
    /// 用户升级之后看到的是"服务方、key、模型全没了"。
    #[test]
    fn 老配置的字符串模型列表能读进来() {
        let json = r#"{
            "providers": [{
                "id": "zp", "name": "智谱", "protocol": "openai",
                "baseUrl": "https://open.bigmodel.cn/api/paas/v4", "apiKeyEnv": "K",
                "models": ["glm-4.6v", "glm-5.2"]
            }],
            "activeProvider": "zp",
            "activeModel": "glm-5.2"
        }"#;
        let c: AppConfig = serde_json::from_str(json).expect("老配置要能读");
        let ids: Vec<&str> = c.providers[0]
            .models
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(ids, vec!["glm-4.6v", "glm-5.2"]);
        // 老格式里没有能力信息，一律按"不能看图"读 —— 猜"能"的代价是
        // 每次带图的请求都被服务方拒。
        assert!(c.providers[0].models.iter().all(|m| !m.vision));
    }

    /// 短暂存在过的"服务方级 vision"要铺到它的每个模型上。
    ///
    /// 丢掉的话等于把用户勾过的开关悄悄关了，表现是"截图突然又不给模型看了"。
    #[test]
    fn 服务方级视觉开关迁移到模型上() {
        let json = r#"{
            "providers": [{
                "id": "zp", "name": "智谱", "protocol": "openai",
                "baseUrl": "https://x.test", "apiKeyEnv": "K",
                "models": ["glm-4.6v", "glm-5.2"],
                "vision": true
            }],
            "activeProvider": "zp",
            "activeModel": "glm-5.2"
        }"#;
        let c = normalize(serde_json::from_str(json).expect("读配置"));
        assert!(
            c.providers[0].models.iter().all(|m| m.vision),
            "服务方那个开关该铺到每个模型上"
        );
        assert!(!c.providers[0].vision, "铺完之后要清掉，别留两个真相");
    }

    /// active 为空但列表里有服务方：加载时吸附到第一家。
    ///
    /// `[约束]` 这个状态用户拼得出来（添加第一家服务方时 active 留空，
    /// 之后只在主界面模型菜单里选模型）。放着不管的话，界面显示的是
    /// providers[0]，key 状态却按空 id 查 —— 表现为「key 已保存，横幅
    /// 还说没有 API key」。
    #[test]
    fn active_为空但有服务方时吸附到第一家() {
        let json = r#"{
            "providers": [{
                "id": "ds", "name": "deepseek", "protocol": "openai",
                "baseUrl": "https://api.deepseek.com", "apiKeyEnv": "K",
                "models": ["deepseek-v4-flash"]
            }],
            "activeProvider": "",
            "activeModel": ""
        }"#;
        let c = parse(json);
        assert_eq!(c.active_provider, "ds");
        assert_eq!(c.active_model, "deepseek-v4-flash", "模型也为空时一并吸附");
        assert!(c.validate().is_ok());
    }

    #[test]
    fn active_为空但模型已选时只吸附服务方() {
        // 正是 bug 现场的形状：横幅说没 key、模型 pill 却显示着选中的模型。
        let json = r#"{
            "providers": [{
                "id": "ds", "name": "deepseek", "protocol": "openai",
                "baseUrl": "https://api.deepseek.com", "apiKeyEnv": "K",
                "models": ["deepseek-v4-flash"]
            }],
            "activeProvider": "",
            "activeModel": "deepseek-v4-flash"
        }"#;
        let c = parse(json);
        assert_eq!(c.active_provider, "ds");
        assert_eq!(
            c.active_model, "deepseek-v4-flash",
            "用户选过的模型不能被盖掉"
        );
    }

    #[test]
    fn 没有服务方时_active_保持为空() {
        // 全新用户 / 刚删掉最后一家：空 active 是合法状态，不该被动。
        let c = parse(r#"{"providers":[],"activeProvider":"","activeModel":""}"#);
        assert!(c.active_provider.is_empty());
        assert!(c.active_model.is_empty());
    }

    /// 视觉能力按模型算，不按服务方算。
    ///
    /// `[约束]` 这条盯的就是智谱那个 400:同一家的 glm-4.6v 能看图、
    /// glm-5.2 不能，按服务方算的话后者也会被当成能看图。
    #[test]
    fn 同一家里能看图和不能看图的模型互不影响() {
        let mut c = one_provider();
        c.providers[0].models = vec![
            ModelConfig {
                vision: true,
                ..ModelConfig::new("glm-4.6v")
            },
            ModelConfig::new("glm-5.2"),
        ];

        c.active_model = "glm-4.6v".into();
        assert!(c.active_takes_images());
        c.active_model = "glm-5.2".into();
        assert!(!c.active_takes_images(), "纯文本模型不该被当成能看图");
        assert!(c.takes_images("acme", "glm-4.6v"), "另一个模型仍然能看图");
    }

    /// 采样参数的优先级:模型 → 服务方 → 服务端默认。
    #[test]
    fn 模型级采样参数盖住服务方的() {
        let mut c = one_provider();
        c.providers[0].sampling = Sampling {
            temperature: Some(0.2),
            max_output_tokens: Some(1000),
            ..Sampling::default()
        };
        c.providers[0].models = vec![ModelConfig {
            sampling: Sampling {
                temperature: Some(0.9),
                ..Sampling::default()
            },
            ..ModelConfig::new("m1")
        }];

        let r = c.resolve().expect("解析");
        assert_eq!(r.sampling.temperature, Some(0.9), "模型上动过的字段要赢");
        assert_eq!(
            r.sampling.max_output_tokens,
            Some(1000),
            "模型没动的字段要继承服务方，而不是被清掉"
        );
    }

    /// `[约束]` 没填窗口的模型必须原样走全局阈值。
    ///
    /// 这条守的是升级路径：老配置里一个模型都没填窗口，如果这里改用推导值，
    /// 所有存量用户的压缩时机会在升级当天集体变化 —— 而他们什么都没改。
    #[test]
    fn 没填窗口就走全局阈值() {
        let c = one_provider();
        let r = c.resolve().expect("解析");
        assert_eq!(r.context_window, None);
        assert_eq!(r.compact_threshold(100_000), 100_000);
        assert_eq!(r.compact_threshold(31_337), 31_337, "兜底就是原样透传");
    }

    #[test]
    fn 填了窗口就按窗口推阈值() {
        let mut c = one_provider();
        c.providers[0].models = vec![ModelConfig {
            context_window: Some(200_000),
            ..ModelConfig::new("m1")
        }];

        let r = c.resolve().expect("解析");
        // 200k − 20k 输出预留 − 13k 缓冲。全局那个 100k 不再参与。
        assert_eq!(r.compact_threshold(100_000), 167_000);
    }

    /// 模型自己声明的最大输出比上限小时，按它留 —— 省下来的都是可用窗口。
    #[test]
    fn 输出预留按模型的最大输出算() {
        let mut c = one_provider();
        c.providers[0].models = vec![ModelConfig {
            context_window: Some(200_000),
            sampling: Sampling {
                max_output_tokens: Some(4_096),
                ..Sampling::default()
            },
            ..ModelConfig::new("m1")
        }];

        let r = c.resolve().expect("解析");
        assert_eq!(r.compact_threshold(100_000), 200_000 - 4_096 - 13_000);
    }

    /// `[约束]` 小窗口模型不能被推出"每轮都压"。
    ///
    /// 32k 窗口减完两笔预留就归零了。阈值为 0 意味着每一轮开工前都要跑一次
    /// 总结（一次真实的模型调用），压完立刻又超 —— 比不压更贵。
    #[test]
    fn 小窗口不会被推成每轮都压() {
        // 32k：32000 − 20000 − 13000 < 0，要被窗口一半兜住。
        assert_eq!(compact_threshold_for_window(32_000, None), 16_000);
        // 64k：64000 − 33000 = 31000，比一半（32000）还小，同样走一半。
        assert_eq!(compact_threshold_for_window(64_000, None), 32_000);
        // 128k：减完仍高于一半，用减出来的值。
        assert_eq!(compact_threshold_for_window(128_000, None), 95_000);
        // 再小的窗口不能低于阈值下限，否则一次工具输出就触发压缩。
        assert_eq!(compact_threshold_for_window(8_000, None), MIN_COMPACT_THRESHOLD);
    }

    /// 手改 config.json 少打或多打一个 0，要在加载时被夹回来。
    ///
    /// 不夹的话，1_280（少打一个 0）会让每轮都压，而报错里没有任何东西
    /// 指向这个字段。
    #[test]
    fn 手改的离谱窗口会被夹回区间() {
        let json = r#"{
            "providers": [{
                "id": "acme", "name": "Acme", "protocol": "openai",
                "baseUrl": "https://api.acme.test", "apiKeyEnv": "K",
                "models": [
                    { "id": "tiny", "contextWindow": 12 },
                    { "id": "huge", "contextWindow": 999999999 }
                ]
            }],
            "activeProvider": "acme",
            "activeModel": "tiny"
        }"#;
        let c = parse(json);
        assert_eq!(c.providers[0].models[0].context_window, Some(MIN_CONTEXT_WINDOW));
        assert_eq!(c.providers[0].models[1].context_window, Some(MAX_CONTEXT_WINDOW));
    }

    /// 没有 `contextWindow` 字段的配置（也就是所有存量配置）要照常读，
    /// 而且读出来是 `None` 而不是 0 —— 0 会被当成"填了个 0 的窗口"。
    #[test]
    fn 老配置缺窗口字段读成未填() {
        let json = r#"{
            "providers": [{
                "id": "acme", "name": "Acme", "protocol": "openai",
                "baseUrl": "https://api.acme.test", "apiKeyEnv": "K",
                "models": [{ "id": "m1", "vision": true }]
            }],
            "activeProvider": "acme",
            "activeModel": "m1"
        }"#;
        let c = parse(json);
        assert_eq!(c.providers[0].models[0].context_window, None);
    }

    /// 没填窗口时不该往 config.json 里写 `"contextWindow": null`。
    ///
    /// 写了的话每个存量用户的配置在下一次保存时都会多出一堆 null 字段 ——
    /// 那份文件是用户会手改、会贴进 issue 的东西。
    #[test]
    fn 未填的窗口不进配置文件() {
        let json = serde_json::to_string(&one_provider()).expect("序列化");
        assert!(!json.contains("contextWindow"), "没填就不该出现这个键：{json}");
    }

    /// 主模型自己能看图时不该再走兼容模型。
    ///
    /// 走了的话每张截图都多一次调用、多一次计费，而且拿到的是有损转述 ——
    /// 明明有原图可以给。
    #[test]
    fn 能看图的模型不走视觉兼容() {
        let mut c = one_provider();
        c.vision_model = "acme/m1".into();

        assert_eq!(
            c.vision_target(),
            Some(("acme", "m1")),
            "纯文本模型该走兼容"
        );

        c.providers[0].models[0].vision = true;
        assert_eq!(c.vision_target(), None, "能看图就不该再转述一遍");
        assert!(c.active_takes_images());
    }

    #[test]
    fn 视觉兼容模型要写成_provider_斜杠_model() {
        let mut c = one_provider();
        for bad in ["", "acme", "/m1", "acme/", "  "] {
            c.vision_model = bad.into();
            assert_eq!(c.vision_target(), None, "「{bad}」不该被当成合法配置");
        }
        // 两头的空格是从输入框里带出来的，很常见。
        c.vision_model = "  acme/m1  ".into();
        assert_eq!(c.vision_target(), Some(("acme", "m1")));
    }

    /// 老配置里没有这两个字段，要能按"不收图片"读进来。
    ///
    /// `[约束]` 缺字段不能让整份配置解析失败 —— 那表现为用户升级之后
    /// "我配的东西全没了"。
    #[test]
    fn 老配置缺视觉字段也能读() {
        let json = r#"{
            "providers": [{
                "id": "acme", "name": "Acme", "protocol": "openai",
                "baseUrl": "https://api.acme.test", "apiKeyEnv": "K",
                "models": ["m1"]
            }],
            "activeProvider": "acme",
            "activeModel": "m1"
        }"#;
        let c: AppConfig = serde_json::from_str(json).expect("老配置要能读");
        assert!(!c.providers[0].vision, "缺字段按不收图片算");
        assert!(c.vision_model.is_empty());
        assert_eq!(c.vision_target(), None);
    }

    #[test]
    fn max_turns_默认_48_且越界会被夹回() {
        assert_eq!(default_max_turns(), 48);
        // 手改 config.json 把它写成 0 或天文数字，normalize 要夹回区间。
        let zero = normalize(AppConfig {
            max_turns: 0,
            ..Default::default()
        });
        assert_eq!(
            zero.max_turns, MIN_MAX_TURNS,
            "0 轮等于什么都做不了，抬到下限"
        );
        let huge = normalize(AppConfig {
            max_turns: 999_999,
            ..Default::default()
        });
        assert_eq!(huge.max_turns, MAX_MAX_TURNS, "过大要压到上限");
    }

    #[test]
    fn 老配置缺_max_turns_用默认() {
        // 升级上来的配置没有这个字段，要按默认 48 读，而不是 0。
        let json = r#"{"providers":[],"activeProvider":"","activeModel":""}"#;
        assert_eq!(parse(json).max_turns, 48);
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
        assert!(
            c.web.search_enabled,
            "搜索默认开 —— 空地址走内置实例"
        );
        assert!(c.web.search_ready());
        assert!(
            !c.web.using_custom_searxng(),
            "老配置没填地址，必须继续走内置，不能把域名写进配置"
        );
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
        assert_eq!(
            t("deepseek/deepseek-chat"),
            Some(("deepseek".into(), "deepseek-chat".into()))
        );
        // 模型名里带斜杠是常态（ollama、各家中转），只能按第一个斜杠拆
        assert_eq!(
            t("ollama/qwen2.5/7b"),
            Some(("ollama".into(), "qwen2.5/7b".into()))
        );
        assert_eq!(t(""), None);
        assert_eq!(t("没有斜杠"), None);
        assert_eq!(t("/只有模型"), None);
        assert_eq!(t("只有provider/"), None);
    }

    #[test]
    fn 内置搜索地址不进配置() {
        assert_eq!(normalize_searxng_url(""), "");
        assert_eq!(normalize_searxng_url("  "), "");
        assert_eq!(normalize_searxng_url(BUILTIN_SEARXNG_URL), "");
        assert_eq!(normalize_searxng_url("https://searxng.riotai.app/"), "");
        assert_eq!(normalize_searxng_url("http://searxng.riotai.app"), "");
        assert_eq!(
            normalize_searxng_url("http://127.0.0.1:8080/"),
            "http://127.0.0.1:8080"
        );

        assert_eq!(resolve_searxng_url(""), BUILTIN_SEARXNG_URL);
        assert_eq!(
            resolve_searxng_url("http://127.0.0.1:8080"),
            "http://127.0.0.1:8080"
        );
        assert_eq!(searxng_error_label(""), "内置搜索");
        assert_eq!(searxng_error_label(BUILTIN_SEARXNG_URL), "内置搜索");
        assert_eq!(
            searxng_error_label("http://127.0.0.1:8080"),
            "http://127.0.0.1:8080"
        );
        assert!(
            !redact_searxng_url(format!("连不上 {BUILTIN_SEARXNG_URL}"))
                .contains("searxng.riotai.app")
        );

        let leaked = parse(&format!(
            r#"{{"providers":[],"activeProvider":"","activeModel":"","projects":[],
                "web":{{"searchEnabled":true,"searxngUrl":"{BUILTIN_SEARXNG_URL}"}}}}"#
        ));
        assert_eq!(leaked.web.searxng_url, "");
        assert!(!leaked.web.using_custom_searxng());
        assert_eq!(leaked.web.effective_searxng_url(), BUILTIN_SEARXNG_URL);
    }

    fn mcp(id: &str, command: &str) -> McpServerConfig {
        McpServerConfig {
            id: id.into(),
            name: String::new(),
            command: command.into(),
            args: Vec::new(),
            env: Default::default(),
            enabled: true,
        }
    }

    #[test]
    fn mcp_配置的坏_id_在保存时就拦下() {
        // id 进工具名和权限规则，坏 id 的失败发生在几天后的权限匹配上 ——
        // 必须在保存的那一刻报。
        let ok = AppConfig {
            mcp_servers: vec![mcp("fs", "npx")],
            ..Default::default()
        };
        assert!(ok.validate().is_ok());

        let dup = AppConfig {
            mcp_servers: vec![mcp("fs", "npx"), mcp("fs", "uvx")],
            ..Default::default()
        };
        assert!(dup.validate().is_err(), "重复 id 必须拦");

        let bad_char = AppConfig {
            mcp_servers: vec![mcp("my.server", "npx")],
            ..Default::default()
        };
        let e = bad_char
            .validate()
            .expect_err("带点的 id 必须拦")
            .to_string();
        assert!(e.contains("my.server"), "报错要点名：{e}");
    }

    #[test]
    fn mcp_空命令是合法的中间状态() {
        // "刚点了添加、还没填命令"必须能保存 —— 拒绝的表现是设置页
        // "添加按钮点了没反应"（真实发生过）。空命令由 reconcile 跳过。
        let c = AppConfig {
            mcp_servers: vec![mcp("fs", "")],
            ..Default::default()
        };
        assert!(c.validate().is_ok(), "空命令不该拦保存，连接时才要求非空");
    }

    #[test]
    fn 标准_mcp_json_能解析() {
        // 这是 Claude Desktop / Cursor / Cline 的通用形状 —— 每个 MCP
        // 服务器的 README 给的就是这段，必须能整段粘贴。
        let raw = r#"{
            "mcpServers": {
                "filesystem": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
                    "env": { "LOG": "1" }
                },
                "github": { "command": "uvx", "args": ["mcp-github"], "disabled": true }
            }
        }"#;
        let servers = mcp_servers_from_json(raw).expect("标准格式必须能读");
        assert_eq!(servers.len(), 2);

        let fs = servers
            .iter()
            .find(|s| s.id == "filesystem")
            .expect("有 filesystem");
        assert_eq!(fs.command, "npx");
        assert_eq!(
            fs.args,
            vec!["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
        );
        assert_eq!(fs.env.get("LOG").map(String::as_str), Some("1"));
        assert!(fs.enabled);
        assert!(fs.name.is_empty(), "键名合法时不需要另存显示名");

        let gh = servers
            .iter()
            .find(|s| s.id == "github")
            .expect("有 github");
        assert!(!gh.enabled, "disabled: true 要变成停用");
    }

    #[test]
    fn mcp_json_认_vscode_键和裸映射() {
        let vscode = r#"{ "servers": { "fs": { "command": "npx" } } }"#;
        assert_eq!(mcp_servers_from_json(vscode).expect("servers 键").len(), 1);

        let bare = r#"{ "fs": { "command": "npx" } }"#;
        assert_eq!(mcp_servers_from_json(bare).expect("裸映射").len(), 1);
    }

    #[test]
    fn mcp_json_未知字段忽略不报错() {
        // Cline 的 autoApprove、timeout 之类的私货很常见 ——
        // 报错的话用户得先手工删字段才能导入。
        let raw = r#"{ "mcpServers": { "x": {
            "command": "npx", "autoApprove": ["a"], "timeout": 60, "envFile": ".env"
        } } }"#;
        assert!(mcp_servers_from_json(raw).is_ok());
    }

    #[test]
    fn mcp_json_远程服务器给明确的暂不支持() {
        for raw in [
            r#"{ "mcpServers": { "r": { "url": "https://x.test/mcp" } } }"#,
            r#"{ "mcpServers": { "r": { "type": "sse", "command": "x" } } }"#,
        ] {
            let e = mcp_servers_from_json(raw)
                .expect_err("远程该拒")
                .to_string();
            assert!(e.contains("stdio"), "报错要说清暂不支持什么：{e}");
        }
    }

    #[test]
    fn mcp_json_单个服务器内层给指路的报错() {
        // 用户常粘错层级：只粘了 {"command": ...}，缺名字那一层。
        let e = mcp_servers_from_json(r#"{ "command": "npx" }"#)
            .expect_err("缺名字层该拒")
            .to_string();
        assert!(e.contains("名字"), "要教用户正确形状：{e}");
    }

    #[test]
    fn mcp_json_键消毒进id_原名进显示名() {
        let raw = r#"{ "mcpServers": { "my.server v2": { "command": "npx" } } }"#;
        let servers = mcp_servers_from_json(raw).expect("能读");
        assert_eq!(servers[0].id, "my-server-v2", "id 只能用工具名允许的字符");
        assert_eq!(servers[0].name, "my.server v2", "用户看到的名字不变");
        // 消毒结果必须过得了保存校验，否则导入成功、保存失败，用户懵
        let c = AppConfig {
            mcp_servers: servers,
            ..Default::default()
        };
        assert!(c.validate().is_ok());
    }

    #[test]
    fn mcp_json_消毒撞名要报错() {
        let raw = r#"{ "mcpServers": {
            "my.server": { "command": "a" },
            "my-server": { "command": "b" }
        } }"#;
        // 两个键消毒后都是 my-server。撞名必须拒绝 —— 静默丢一个的话，
        // 用户以为两个都导入了。
        assert!(mcp_servers_from_json(raw).is_err());
    }

    #[test]
    fn mcp_json_导出导入互逆() {
        let servers = vec![
            McpServerConfig {
                id: "fs".into(),
                name: String::new(),
                command: "npx".into(),
                args: vec!["-y".into(), "pkg".into()],
                env: [("K".to_owned(), "v".to_owned())].into(),
                enabled: true,
            },
            McpServerConfig {
                id: "gh".into(),
                name: "GitHub".into(),
                command: "uvx".into(),
                args: Vec::new(),
                env: Default::default(),
                enabled: false,
            },
        ];
        let json = mcp_servers_to_json(&servers);
        assert!(json.contains("mcpServers"), "导出要用标准包装：{json}");
        assert!(
            !json.contains("GitHub"),
            "显示名不是标准字段，不该出现在导出里"
        );

        let back = mcp_servers_from_json(&json).expect("自己导出的自己要能读");
        assert_eq!(back.len(), 2);
        let fs = back.iter().find(|s| s.id == "fs").expect("fs");
        assert_eq!(fs.args, vec!["-y", "pkg"]);
        assert!(fs.enabled);
        let gh = back.iter().find(|s| s.id == "gh").expect("gh");
        assert!(!gh.enabled, "disabled 要在往返中保住");
    }

    #[test]
    fn 没有_mcp_段的老配置照常能读() {
        let c = parse(r#"{"providers":[],"activeProvider":"","activeModel":"","projects":[]}"#);
        assert!(
            c.mcp_servers.is_empty(),
            "缺字段按空列表读，不能整体解析失败"
        );
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
            c.provider("deepseek")
                .expect("有 deepseek")
                .sampling
                .max_output_tokens,
            Some(4096)
        );
        // 迁移只搬用户配过的那一家，不附赠出厂预设
        assert_eq!(
            c.providers.len(),
            1,
            "迁移不该凭空多出服务方：{:?}",
            c.providers
        );
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
        assert_eq!(p.models, vec![ModelConfig::new("qwen-max")]);
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
            vision: false,
            api_path: String::new(),
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
            vision: false,
            api_path: String::new(),
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
            vision: false,
            api_path: String::new(),
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
        let mode = std::fs::metadata(&p)
            .expect("metadata")
            .permissions()
            .mode();
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
        assert_eq!(
            load_auth(&p).get("TEST_RECOVER").map(String::as_str),
            Some("sk-x")
        );
        std::fs::remove_file(&p).ok();
    }
}
