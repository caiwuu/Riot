//! 一轮任务的跨进程配置契约。
//!
//! 阶段 B 里内核是独立进程,不读 `config.json` / `auth.json`(那是宿主的
//! 职责,见 ARCHITECTURE.md §2.2 决策)。每轮所需的模型端点、采样参数、
//! 明文密钥、联网/视觉/子 agent 配置都由宿主解析好,作为 RPC 参数经这里的
//! 类型传给内核。
//!
//! `[约束]` 明文 `api_key` 只在本地进程间(stdio)传输。它不落盘、不进日志、
//! 不进事件 —— 和宿主 `auth.json` 的处理同一条线。

use std::collections::BTreeMap;

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
    /// OpenAI 兼容（Chat Completions 或 Responses，见 [`OpenaiApi`]）。
    Openai,
    /// Anthropic Messages。
    Anthropic,
}

/// OpenAI 协议下的接口形态。路径只负责拼 URL，形态决定报文。
///
/// `[约束]` 不要用路径后缀猜。官方是 `/v1/responses`，中转和 Azure 可以
/// 是任何尾巴；猜错的表现是语焉不详的 400。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OpenaiApi {
    /// `/v1/chat/completions` 那套 `messages` + SSE chunk。
    #[default]
    ChatCompletions,
    /// `/v1/responses` 那套 `input` + 命名 SSE 事件。
    Responses,
}

impl OpenaiApi {
    pub fn is_responses(self) -> bool {
        matches!(self, Self::Responses)
    }

    /// 缺省形态。配置和端点序列化时用它跳过默认值
    /// （`skip_serializing_if`），老文件 / 老宿主才读得回来。
    pub fn is_chat_completions(&self) -> bool {
        matches!(self, Self::ChatCompletions)
    }
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
    /// 已经展开过模板的额外请求头。空 = 只发协议自己的头 + 默认 User-Agent。
    ///
    /// 缺字段必须能读：老宿主发的 `ModelEndpoint` 没有这一项，新内核
    /// 不能因此整轮解析失败。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra_headers: BTreeMap<String, String>,
    /// OpenAI 下选 Chat Completions 还是 Responses。Anthropic 忽略。
    ///
    /// 缺字段必须能读：老宿主发的 `ModelEndpoint` 没有这一项，按
    /// Chat Completions 走 —— 那是缺省之前唯一的 OpenAI 形态。
    #[serde(default, skip_serializing_if = "OpenaiApi::is_chat_completions")]
    pub openai_api: OpenaiApi,
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
    /// 读全开、写限于工作区 / 临时目录 / 桌面 / 下载和构建缓存、联网照常。
    #[default]
    WorkspaceWrite,
    /// 同上,另外掐掉网络。
    WorkspaceWriteNoNet,
    /// 不隔离,只剩策略层拦着。
    Off,
}

/// 一轮的数值上限与隔离强度。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TurnLimits {
    /// 权限弹窗等多久算超时(秒)。超时按拒绝处理。
    pub ask_timeout_secs: u32,
    /// 单轮最多自主往返多少步。
    pub max_turns: u32,
    /// 历史超过这个 token 数就在开工前做 LLM 总结压缩。
    pub compact_threshold_tokens: u32,
    #[serde(default)]
    pub sandbox: SandboxKind,
    /// 沙箱内额外可读的路径,用户手填。语义对齐上游 sandbox-runtime 的
    /// `filesystem.allowRead`:Windows 上翻成 READ|EXECUTE 的 ALLOW ACE;
    /// macOS 读本就全开,用不上。per-user 工具(nvm、Scoop、conda……)在
    /// 沙箱内默认打不开是上游记档的已知限制,需要谁就填谁 —— **不**自动
    /// 扫 PATH 去授权,那会把 anaconda 这类几十万文件的树拖进每次激活。
    #[serde(default)]
    pub sandbox_allow_read: Vec<String>,
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
    /// 多任务模式（Cursor Multitask 同款）：主 agent 只协调，实质工作全部
    /// 交给后台子 agent，委派完结束回合、由完成通知叫醒。宿主是权威，
    /// 每轮现传；内核据此在消息侧注入协调者准则。
    #[serde(default)]
    pub multitask: bool,
}

/// 用户在界面上按的一个"推一把"按钮，变成一条塞给模型的带外提醒。
///
/// 对应 Cursor 的 SimulatedMsgReason：按钮不产生用户的话，只产生一条
/// system_reminder。两条投递路：
///
/// - **轮中**（`turn.nudge`）：注入到当前轮的下一个安全点 —— 这一批工具
///   结果就位、模型还没开口的那一刻。按钮说的是"你手上这件事"，等整轮
///   跑完再给模型看，功能就等于不存在。「转到后台」走这条。
/// - **开轮**（[`TurnInput::nudge`]）：随一条用户消息一起进历史，附在
///   正文之后。计划做完、回合已经结束，用户点「构建」时没有轮在跑，
///   提醒只能跟着新开的那一轮走。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Nudge {
    /// 「转到后台」：把手头的活 `resume="self"` 分叉到后台子 agent，
    /// 主 agent 立刻停下 —— 对话腾出来给用户聊别的。
    StartMultitasking,
    /// 「构建」：计划已批准，读回计划文件、落成待办、按序动手。
    BuildPlan,
    /// 「并行构建」：按计划里 todo 的依赖分层，每层一个后台子 agent，
    /// 能并行的并行；末尾的测试留给最后一个测试 agent。
    BuildInParallel,
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
    /// 这条消息是界面上哪个按钮发出来的（「构建」/「并行构建」）。
    /// 内核据此在正文之后附一条 system_reminder；正文本身只是给人看的
    /// 一句短话（"开始构建计划"），指示全在提醒里。见 [`Nudge`]。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nudge: Option<Nudge>,
    /// 这条消息是定时任务到点发的，不是用户手敲的。内核据此在正文之后附
    /// 一条 system_reminder，告诉模型是谁叫醒了它。见 [`ScheduledWake`]。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled: Option<ScheduledWake>,
}

/// 叫醒这一轮的定时任务。
///
/// 没有它的话，到点那一轮收到的是一句和用户手敲的一模一样的话：模型不知道
/// 自己是被定时任务叫醒的，也不知道任务 id —— 「盯到 CI 过就停」这种有限的
/// 盯梢，条件达成后它想把自己删掉都得先 list 再按名字对。带上 id 和重复
/// 规则，它才有办法在目标达成时收尾、在跑不下去时暂停。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledWake {
    pub task_id: String,
    pub name: String,
    pub repeat: crate::schedule::Repeat,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 宿主和内核可能不是同一个版本（升级后内核二进制先换、宿主还在跑旧的，
    /// 或反过来）：老宿主发的 `TurnInput` 没有 `scheduled`，新内核必须照常读。
    #[test]
    fn 老宿主发的_turn_input_缺_scheduled_也能读() {
        let v: TurnInput = serde_json::from_value(serde_json::json!({
            "text": "hi",
            "images": [],
            "refs": [],
        }))
        .expect("缺字段不能让整条请求解析失败");
        assert_eq!(v.scheduled, None);
        assert_eq!(v.nudge, None);
    }

    /// 老宿主发的端点没有 `extra_headers`，新内核必须照常读成空表。
    #[test]
    fn 老宿主发的_model_endpoint_缺_extra_headers_也能读() {
        let v: ModelEndpoint = serde_json::from_value(serde_json::json!({
            "protocol": "openai",
            "base_url": "https://example.com",
            "api_path": "",
            "api_key": "k",
            "model": "t"
        }))
        .expect("缺 extra_headers 不能让整轮解析失败");
        assert!(v.extra_headers.is_empty());
        assert_eq!(
            v.openai_api,
            OpenaiApi::ChatCompletions,
            "老端点没有 openai_api，必须当 Chat Completions"
        );
    }

    #[test]
    fn scheduled_wake_往返() {
        let input = TurnInput {
            text: "盯一下 CI".into(),
            scheduled: Some(ScheduledWake {
                task_id: "sch_1".into(),
                name: "盯 CI".into(),
                repeat: crate::schedule::Repeat::Every { minutes: 5 },
            }),
            ..Default::default()
        };
        let v = serde_json::to_value(&input).expect("序列化");
        assert_eq!(v["scheduled"]["taskId"], "sch_1", "{v}");
        assert_eq!(v["scheduled"]["repeat"]["kind"], "every", "{v}");
        let back: TurnInput = serde_json::from_value(v).expect("反序列化");
        assert_eq!(back, input);
    }
}
