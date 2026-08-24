//! 一轮任务的跨进程配置契约。
//!
//! 阶段 B 里内核是独立进程,不读 `config.json` / `auth.json`(那是宿主的
//! 职责,见 ARCHITECTURE.md §2.2 决策)。每轮所需的模型端点、采样参数、
//! 明文密钥、联网/视觉/子 agent 配置都由宿主解析好,作为 RPC 参数经这里的
//! 类型传给内核。
//!
//! `[约束]` 明文 `api_key` 只在本地进程间(stdio)传输。它不落盘、不进日志、
//! 不进事件 —— 和宿主 `auth.json` 的处理同一条线。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::permission::{PermissionMode, PermissionRule};
use crate::provider::ThinkingPolicy;

/// 说话用的协议。决定请求格式与认证头。
///
/// 和宿主 `config` 里的 `Protocol` 同构 —— 那个是配置侧(会序列化进
/// `config.json`),这个是传输侧(宿主↔内核 RPC)。分开是因为配置类型
/// 属于宿主、不该进 protocol 这个叶子 crate。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApiProtocol {
    /// OpenAI Chat Completions 兼容。
    Openai,
    /// Anthropic Messages。
    Anthropic,
}

/// 采样参数。`None` = 用端点默认。
///
/// 独立于 `riot-providers` 的 `SamplingParams`(那个不含 `max_output_tokens`,
/// 因为输出上限在主循环单独走恢复路径)—— 这里是"宿主配置的完整快照",
/// 由内核在建 Provider 和设置输出上限时各取所需。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct EndpointSampling {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

/// 一个已解析的模型端点:宿主把 provider 配置和明文 key 都填好,内核直接
/// 拿它建 Provider。
///
/// 这是 `config::ResolvedModel` 的"传输版" —— 区别在于 `api_key` 是**明文**
/// (宿主已从环境变量 / auth.json 解析出来),而不是一个待查的变量名。
/// 拆进程后内核拿不到 auth.json,key 必须在宿主这一侧解析完再传进来。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModelEndpoint {
    pub protocol: ApiProtocol,
    pub base_url: String,
    /// 接口路径,空 = 按主机猜(见 `riot_providers::endpoint`)。
    pub api_path: String,
    /// 明文密钥。见模块文档的约束。
    pub api_key: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_model: Option<String>,
    #[serde(default)]
    pub sampling: EndpointSampling,
}

impl ModelEndpoint {
    pub fn is_anthropic(&self) -> bool {
        self.protocol == ApiProtocol::Anthropic
    }
}

/// 联网能力配置(随 turn 传给内核)。
///
/// 抓取(fetch)不需要第三方服务;搜索(search)默认走内置 SearXNG,用户可覆盖;
/// 蒸馏(distill)要一个辅助模型端点。三者独立开关,和宿主 `WebConfig` 一致。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WebSetup {
    pub fetch_enabled: bool,
    pub search_enabled: bool,
    /// 用户覆盖的 SearXNG 地址。空 = 用内置实例。
    #[serde(default)]
    pub searxng_url: String,
    /// 网页正文蒸馏的辅助模型端点。None = 不蒸馏,抓取返回截断原文。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distill: Option<ModelEndpoint>,
}

/// 视觉能力配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VisionSetup {
    /// 主模型能否直接收图片。
    pub accepts_images: bool,
    /// 视觉兼容模型端点(主模型收不了图时转述)。None = 无,截图工具报未配置。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub describe: Option<ModelEndpoint>,
}

/// 命令的 OS 级隔离强度。和宿主 `config::SandboxMode` 同构。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SandboxKind {
    /// 读全开、写限于工作区和构建缓存、联网照常。
    #[default]
    WorkspaceWrite,
    /// 同上,另外掐掉网络。
    WorkspaceWriteNoNet,
    /// 不隔离,只剩策略层拦着。
    Off,
}

/// 一轮的数值上限与隔离强度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnLimits {
    /// 权限弹窗等多久算超时(秒)。超时按拒绝处理。
    pub ask_timeout_secs: u32,
    /// 单轮最多自主往返多少步。
    pub max_turns: u32,
    /// 历史超过这个 token 数就在开工前做 LLM 总结压缩。
    pub compact_threshold_tokens: u32,
    #[serde(default)]
    pub sandbox: SandboxKind,
}

/// 提交一轮所需的完整配置(`turn.submit` 的 RPC 载荷,除用户输入之外的一切)。
///
/// 宿主从 `AppConfig` + 会话设置解析出它,内核据此现装 provider、联网、视觉、
/// 子 agent、权限。**不含** MCP / Skill 工具:那些是 trait object,不能跨进程,
/// 由内核自己从 MCP hub 和技能目录装配(见 M-B4b)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TurnConfig {
    /// 主模型端点。
    pub model: ModelEndpoint,
    /// 只读侦察子 agent 的便宜档;也用于 Auto 模式的判危分类器。
    /// None = 跟主模型。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cheap_model: Option<ModelEndpoint>,
    pub web: WebSetup,
    pub vision: VisionSetup,
    pub limits: TurnLimits,
    /// 会话权限模式。
    pub mode: PermissionMode,
    /// 会话内累积的权限规则("总是允许"等)。
    #[serde(default)]
    pub rules: Vec<PermissionRule>,
    /// 会话级 Python 虚拟环境根目录。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_venv: Option<String>,
    /// 会话级追加系统提示词。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt_extra: Option<String>,
    /// 会话级思考策略。
    #[serde(default)]
    pub thinking: ThinkingPolicy,
}

/// 排队面板的一条插话摘要。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueuedSummary {
    pub id: String,
    pub text: String,
    /// 附了几张图。面板只显示个数 —— 全量 base64 回传太重。
    pub images: usize,
    /// 引用的文件路径。面板直接列出来(它们是路径,不重)。
    pub refs: Vec<String>,
}

/// 用户随消息附上的一张图。只走内容不走路径(剪贴板截图没有路径)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImageInput {
    pub media_type: String,
    pub data: String,
}

/// 用户这一轮发来的原始输入。图片转述、`@` 展开、UserPromptSubmit hook 都在
/// 内核完成 —— 所以这里只传原始三样,内核据此构造最终消息(内核有 vision /
/// mentions / hooks,宿主没有,不能在宿主构造一半)。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TurnInput {
    pub text: String,
    #[serde(default)]
    pub images: Vec<ImageInput>,
    #[serde(default)]
    pub refs: Vec<String>,
}
