//! 子 agent：Task 工具。
//!
//! # 骨架（对照 Claude Code 的 AgentTool / runAgent）
//!
//! Task 就是**再跑一遍主循环**：独立的系统提示、独立的工具集、全新的
//! 上下文（一条 user 消息），跑完取最后一条 assistant 文本回给父。
//! 它不是旁路 —— 子 agent 的每个工具调用走和父完全相同的调度器和
//! 权限闸（同一个 HostGate，弹窗、规则、模式全部一致）。
//!
//! # 三种跑法 ⭐
//!
//! - **同步**（默认）：父的这次工具调用一直等到子 agent 跑完，结果作为
//!   tool_result 回去。适合几十秒内能回来的侦察；同一批里可以并行几个。
//! - **后台**（`run_in_background: true`）：工具调用立刻返回 agent id，
//!   子 agent 在会话上继续跑；**完成时合成一条通知消息交回父会话** ——
//!   父在跑就排队到安全点注入，父空闲就唤起新的一轮（见
//!   [`TaskHost::deliver`]）。父 agent 委派完就该结束回合，不等、不轮询。
//! - **分叉**（`resume: "self"`）：把父自己复制成一个后台子 agent ——
//!   同一份 system、同一套工具、到此为止的全部历史。父不用把上下文塞进
//!   prompt 里重述一遍；同形状的请求还能吃到 provider 的前缀缓存。
//!
//! 外加**续接**（`resume: <agent id>`）：任何跑过的子 agent（同步的、
//! 后台的、分叉的）都能带着它的全部历史再跑一段。"报告不够细，再往下
//! 挖一层"不必从头讲背景。
//!
//! # 两个内置类型
//!
//! - `general-purpose`：全套文件/命令/联网工具，自主完成多步任务；
//! - `explore`：**只读**侦察（Read/Grep/Glob/WebFetch/WebSearch），
//!   给"到处找找"这类任务 —— 便宜、可并行、绝不改东西。
//!
//! # 类型声明成本，不只声明工具
//!
//! [`Kind`] 除了决定给哪些工具，还决定**这一档愿意花多少钱**：用哪个模型、
//! 最多跑几轮。这不是锦上添花 —— 只读侦察的产出是一份文字报告，却往往比
//! 主对话吃掉更多 token（几十次 Grep/Read 的结果全进它的上下文），用和主
//! 循环同一档的模型跑它是这类架构里最容易漏掉的一笔开销。
//!
//! `[约束]` 预算属于类型，不属于调用参数。模型填 `subagent_type` 是在选
//! 一个**已经定好价的档**，它没有任何办法给自己多要预算。
//!
//! # 递归：深度计数器 ⭐
//!
//! 普通子 agent 的注册表里**没有 Task 工具**，递归在结构上就不存在
//! （CC 的教训清单："子 agent 能再 spawn 子 agent → 无限递归"）。
//!
//! 分叉是例外：它必须带 Task 工具 —— 工具清单是请求形状的一部分，少一个
//! 工具前缀缓存就全失（~100k 的上下文重算一遍），而缓存正是分叉比"重述
//! 上下文"划算的全部理由。所以分叉里的 Task 工具**同名、同 schema、同
//! 描述**，只是 [`TaskTool::depth`] 为 1：它照常能开同步子 agent（分叉
//! 内部再拆几个侦察并行是合理的），但拒绝 `resume: "self"` —— 分叉的
//! 分叉在结构上到不了。这仍然是结构性保证，不靠提示词劝。
//!
//! `[约束]` 分叉内的 Task 只能同步跑。后台任务的完成通知投递到**会话**
//! 的队列，而分叉不是会话、没有队列 —— 它开的后台任务跑完了没人会被
//! 唤醒。
//!
//! # 结果与可观测性
//!
//! - 结果 = 最后一条有文本的 assistant 消息（CC 同款），附用量脚注；
//! - 同步：过程以 Progress 事件流回父会话的工具卡片；后台：每个动静
//!   更新一次 [`riot_protocol::event::AgentEvent::BackgroundTask`]；
//! - transcript 落在 `sessions/subagents/<会话>/<agent>.jsonl`，和主
//!   transcript 隔开 —— 放同一目录会被索引重建当成会话捞回来。分叉只
//!   写它自己产生的那部分（继承的历史在父的 transcript 里已经有了）。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;

use riot_core::{AgentDeps, AgentState, run_agent};
use riot_protocol::event::{AgentEvent, OutputStream, ProgressPayload, TerminalReason};
use riot_protocol::id::{AgentId, IdGenerator, SessionId, ToolUseId};
use riot_protocol::message::{AssistantContent, Message, Usage};
use riot_protocol::permission::{
    DecisionReason, PermissionContext, PermissionGate, PermissionResult,
};
use riot_protocol::task::{BackgroundTaskStatus, BackgroundTaskView};
use riot_protocol::tool::{PromptContext, Tool, ToolContext, ToolOutcome, UiPayload};
use riot_runtime::{MemoryFileState, SystemFs, SystemProcessRunner};
use riot_tools::registry::Registry;
use riot_tools::scheduler::Scheduler;
use tokio_util::sync::CancellationToken;

use crate::tasks::BackgroundTasks;

/// 只读侦察档用的便宜模型。
///
/// None（[`SubagentDeps::cheap`] 为空）= 没配，全部类型都跟主模型。
#[derive(Clone)]
pub struct CheapModel {
    pub provider: Arc<dyn riot_protocol::provider::Provider>,
    pub model: String,
}

impl CheapModel {
    /// 从 RPC 传入的端点装便宜档(拆进程后内核走这条,不碰 AppConfig)。
    /// `None` = 没配便宜档,只读侦察跟主模型。
    pub fn from_endpoint(endpoint: Option<&riot_protocol::ModelEndpoint>) -> Option<Self> {
        let endpoint = endpoint?;
        match crate::models::provider_from_endpoint(endpoint) {
            Ok(provider) => Some(Self {
                provider,
                model: endpoint.model.clone(),
            }),
            Err(e) => {
                tracing::warn!(error = %e, "子 agent 便宜档建不出客户端，只读侦察改走主模型");
                None
            }
        }
    }

    /// 按配置装便宜档。每轮现装 —— 用户中途换掉它，下一轮生效。
    ///
    /// 没配、格式不对、指向主模型自己、provider 找不到、密钥缺失 —— 一律
    /// 返回 None，降级成"跟主模型"。
    ///
    /// `[取舍]` 这里刻意不报错。便宜档是个纯省钱的可选项，配坏了该悄悄失效：
    /// 用户加它是为了少花钱，不该因此给自己多一个"发消息没反应"的故障点。
    /// 代价是配错了不容易发现 —— 由进度行里的模型名兜住（见 [`TaskTool::call`]）。
    pub fn from_config(config: &crate::config::AppConfig) -> Option<Self> {
        let (provider_id, model) = config.subagent_target()?;
        let resolved = match config.resolve_named(provider_id, model) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "子 agent 便宜档解析不了，只读侦察改走主模型");
                return None;
            }
        };
        match crate::models::provider_for(&resolved) {
            Ok(provider) => Some(Self {
                provider,
                model: resolved.model,
            }),
            Err(e) => {
                tracing::warn!(error = %e, "子 agent 便宜档建不出客户端，只读侦察改走主模型");
                None
            }
        }
    }
}

/// 会话向 Task 工具开放的两件事：投递完成通知、造分叉。
///
/// 抽成 trait 是因为 Task 工具在 `riot-kernel` 里、会话也在 —— 但工具不该
/// 抓着 `Arc<Session>`：会话的注册表里有这个工具，抓回去就是引用环。会话
/// 侧用 `Weak` 实现它；测试给 [`NoTaskHost`]。
#[async_trait]
pub trait TaskHost: Send + Sync {
    /// 一个后台子 agent 结束了，把通知交给父会话。
    ///
    /// 父在跑：排进插话队列，内核在安全点注入；父空闲：用上一轮的配置
    /// 唤起新的一轮。会话已关闭：丢弃。
    async fn deliver(&self, notice: Message);

    /// 自我分叉：用父**此刻**的 system、工具、历史造一个 [`Job`]。
    ///
    /// `fork_call` 是把它分叉出来的那次 Task 调用的 id —— 分叉的历史里
    /// 那条 tool_use 还没有结果，要补上（见 [`fork_prelude`]）。
    /// `Err` = 这一轮不支持分叉（没装配、会话已关闭）。
    async fn fork_job(
        &self,
        agent_id: &AgentId,
        title: &str,
        prompt: &str,
        fork_call: &ToolUseId,
    ) -> Result<Job, String>;
}

/// 没有宿主：通知丢弃、不能分叉。单元测试用。
pub struct NoTaskHost;

#[async_trait]
impl TaskHost for NoTaskHost {
    async fn deliver(&self, _notice: Message) {}
    async fn fork_job(
        &self,
        _agent_id: &AgentId,
        _title: &str,
        _prompt: &str,
        _fork_call: &ToolUseId,
    ) -> Result<Job, String> {
        Err("这个环境不支持自我分叉。".into())
    }
}

/// 组装一个子 agent 轮次所需的一切。由 run_inner 从当轮快照。
#[derive(Clone)]
pub struct SubagentDeps {
    pub provider: Arc<dyn riot_protocol::provider::Provider>,
    pub model: String,
    /// 便宜档。只有 [`Kind::prefers_cheap`] 的类型会用到它。
    pub cheap: Option<CheapModel>,
    /// 和父共用同一个权限闸：弹窗、会话规则、模式全部一致。
    pub gate: Arc<dyn PermissionGate>,
    /// 父会话激活的沙箱，子 agent 的命令要套同一个。
    ///
    /// `[约束]` 和 `gate` 必须来自**同一次** activate。共用的那个闸里带着
    /// `PermissionContext::sandboxed`，子 agent 这边只要没把沙箱套上，那个
    /// 标志就成了谎报 —— 决策链按「OS 挡着文件系统」放行一批写命令，而它们
    /// 在宿主上裸跑。这正是 `riot_runtime::sandbox` 模块头 [约束] 说的那种
    /// 静默放行，只不过入口从 Bash 换成了 Task。
    pub sandbox: Option<Arc<riot_runtime::ActiveSandbox>>,
    pub web: Arc<dyn riot_protocol::web::WebAccess>,
    pub vision: Arc<dyn riot_protocol::vision::VisionAccess>,
    pub clock: Arc<dyn riot_protocol::tool::Clock>,
    pub ids: Arc<dyn IdGenerator>,
    pub cwd: PathBuf,
    pub artifacts_dir: PathBuf,
    pub max_turns: u32,
    /// 子 agent transcript 的落盘处（`sessions/subagents/<会话>/`）。
    /// None = 父会话本身不持久化（测试）。
    pub transcripts: Option<Arc<riot_store::Transcripts>>,
    /// 会话的子 agent 登记表：后台任务的面板、续接的历史都在这里。
    pub tasks: Arc<BackgroundTasks>,
    pub host: Arc<dyn TaskHost>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct Input {
    /// 三五个词的任务名，显示在界面上。续接到一个新任务时换一个；
    /// 续原任务就别改。
    description: String,
    /// 给子 agent 的任务描述。新起的子 agent 看不到本对话的任何内容 ——
    /// 背景、目标、范围、已知线索都要写进来。续接（resume）时只写增量
    /// 指令，它记得之前的一切。
    prompt: String,
    /// `general-purpose`（默认，全工具）或 `explore`（只读侦察）。
    /// resume 时忽略，沿用原来的类型。
    #[serde(default)]
    subagent_type: Option<String>,
    /// true = 后台跑：立刻返回 agent id，子 agent 完成后你会收到一条通知
    /// 消息（含它的汇报）。false（默认）= 等它跑完，结果作为本次调用的
    /// 结果返回。
    #[serde(default)]
    run_in_background: Option<bool>,
    /// 续接。填某次 Task 返回的 agent id：让那个子 agent 带着它的全部
    /// 上下文继续。填 "self"：把你自己分叉成一个后台子 agent，它继承本
    /// 对话到此为止的全部内容，独立执行 prompt 里的任务（只能后台跑）。
    #[serde(default)]
    resume: Option<String>,
}

pub struct TaskTool {
    deps: SubagentDeps,
    /// 0 = 主 agent 的 Task；1 = 分叉出的子 agent 里的 Task（见模块文档
    /// 「递归：深度计数器」）。
    depth: u8,
}

impl TaskTool {
    pub fn new(deps: SubagentDeps) -> Self {
        Self { deps, depth: 0 }
    }

    /// 分叉里用的那份：同名同形，只是不能再分叉、不能开后台。
    pub fn forked(deps: SubagentDeps) -> Self {
        Self { deps, depth: 1 }
    }
}

/// 只读侦察的轮数上限。
///
/// 侦察是"找到并汇报"，不是"做完"。一轮里能并发发十个 Grep/Read，16 轮
/// 足够把一个仓库翻遍；再往上基本是它在原地打转 —— 反复读同一批文件、
/// 换着关键词搜同一个东西 —— 而每一轮都在往上下文里堆搜索结果。
///
/// 到顶不是报错：已有结论照常回传，只附一句"被步数上限停下"。
const EXPLORE_MAX_TURNS: u32 = 16;

/// 子 agent 类型。见模块文档「类型声明成本」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    GeneralPurpose,
    Explore,
    /// 父自己的复制品。不能从 `subagent_type` 选出来，只由 `resume: "self"`
    /// 产生；工具、模型、系统提示全部跟父。
    Fork,
}

impl Kind {
    /// 未知类型返回 None —— 报错要能列出可用的，别让模型猜。
    fn parse(s: &str) -> Option<Self> {
        match s {
            "general-purpose" => Some(Self::GeneralPurpose),
            "explore" => Some(Self::Explore),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::GeneralPurpose => "general-purpose",
            Self::Explore => "explore",
            Self::Fork => "fork",
        }
    }

    /// 这一档会不会改东西。父调度器据此决定它能不能和别的只读工具同批并行。
    fn is_read_only(self) -> bool {
        matches!(self, Self::Explore)
    }

    /// 愿不愿意降到便宜模型。
    ///
    /// `[约束]` 只有只读档能降。`general-purpose` 会改文件，省下的钱不值得
    /// 让一个更笨的模型去动代码 —— 它改坏一次的代价远超那点 token 差价。
    fn prefers_cheap(self) -> bool {
        matches!(self, Self::Explore)
    }

    /// 这一档的轮数上限。父会话的上限是天花板，只能更小不能更大。
    fn max_turns(self, parent: u32) -> u32 {
        match self {
            Self::GeneralPurpose | Self::Fork => parent,
            Self::Explore => parent.min(EXPLORE_MAX_TURNS),
        }
    }
}

/// 从工具入参里读类型。缺省和**认不出的**都算 general-purpose ——
/// 这是 fail-closed 的那一侧（会写、不便宜、不被当成只读）。真正的
/// 拒绝在 [`TaskTool::call`] 里，那里能给模型一句可用清单。
///
/// 带 `resume` 的调用类型未知（跟原来的），也按会写处理。
fn kind_of(input: &serde_json::Value) -> Kind {
    if input.get("resume").and_then(|v| v.as_str()).is_some() {
        return Kind::GeneralPurpose;
    }
    input
        .get("subagent_type")
        .and_then(|v| v.as_str())
        .and_then(Kind::parse)
        .unwrap_or(Kind::GeneralPurpose)
}

/// 各类型的工具集。
///
/// `[约束]` 两个清单里都没有 Task —— 递归要在结构上不存在，不能靠
/// 提示词劝。也没有 TodoWrite（子 agent 的清单父会话看不见，白记）、
/// Browser*（浏览器是会话级独占资源，并发子 agent 抢一个面板会打架）。
///
/// `Fork` 不走这里：它的工具是父的那一套（见 [`TaskHost::fork_job`]）。
fn tools_for(kind: Kind) -> Vec<Arc<dyn Tool>> {
    use riot_tools::tools;
    let cache = Arc::new(tools::web::PageCache::default());
    match kind {
        Kind::Explore => vec![
            Arc::new(tools::Read),
            Arc::new(tools::Grep),
            Arc::new(tools::Glob),
            Arc::new(tools::WebSearch),
            Arc::new(tools::WebFetch::new(cache)),
        ],
        Kind::GeneralPurpose | Kind::Fork => vec![
            Arc::new(tools::Read),
            Arc::new(tools::Edit),
            Arc::new(tools::Write),
            Arc::new(tools::Bash),
            Arc::new(tools::Grep),
            Arc::new(tools::Glob),
            Arc::new(tools::WebSearch),
            Arc::new(tools::WebFetch::new(cache)),
        ],
    }
}

/// 子 agent 的系统提示。
///
/// 刻意**不注入** AGENTS.md：那份东西是给"在这个项目里写代码"的人看的，
/// 而侦察档只汇报。CC 源码注释说 Explore 省掉 CLAUDE.md 一项每周省
/// 5–15 G token —— 这里的省法一样，只是它从一开始就没接进来。
fn system_prompt_for(kind: Kind, cwd: &std::path::Path) -> String {
    let base = format!(
        "工作目录：{}\n平台：{}\n\n",
        cwd.display(),
        std::env::consts::OS
    );
    match kind {
        Kind::Explore => format!(
            "你是只读侦察专家，任务是快速、准确地摸清情况并汇报。\n\n{base}\
             规则：\n\
             - 只读。不修改任何文件、不执行有副作用的操作 —— 委托方是按\
               「只读侦察」放你进来的，越界的写操作绕过了他的审查。\n\
             - 并行地广撒网（Grep/Glob 可以同批多个），再对命中处精读 —— \
               串行搜索是这类任务最大的时间浪费。\n\
             - 汇报要可跳转：结论都带文件路径和行号 —— 委托方要照着你的\
               报告直接动手，少个行号他就得重找一遍。\n\
             - 你的回复会**原样**作为调查结果交回，写成一份紧凑的报告：\
               先结论，再证据，不要过程独白 —— 过程只消耗委托方的上下文，\
               不增加信息。\n\n回答用中文。",
        ),
        Kind::GeneralPurpose | Kind::Fork => format!(
            "你是自主完成任务的执行者。委托方给你一个任务，你独立做完并汇报。\n\n{base}\
             规则：\n\
             - 动手前先看清楚：改文件前 Read，找位置用 Grep —— 凭猜测改出的\
               错误，委托方比你更难发现。\n\
             - 只做任务描述里的事，不顺手扩展 —— 委托方看不到你的过程，\
               扩展出的改动他无从审查，只能连你做对的部分一起怀疑。\n\
             - 你的最后一条回复会**原样**作为任务结果交回 —— 写清楚做了什么、\
               改了哪些文件、验证结果如何；失败就如实说失败和原因，粉饰的\
               「完成」会让委托方带着错误结论继续走。\n\n回答用中文。",
        ),
    }
}

/// 给后台子 agent 的工具贴上归属：权限弹窗的那句话前面带"后台任务「x」"。
///
/// 后台任务的权限询问弹出来时，父轮次多半已经结束、用户正在聊别的 ——
/// 一句光秃秃的"运行 rm -rf build"他不知道是谁要干。只改 `describe`
/// （弹窗的 summary 取它）；name / schema / prompt 原样透传，请求形状
/// 不变（分叉的缓存命中靠这个）。
pub struct Attributed {
    inner: Arc<dyn Tool>,
    label: String,
}

impl Attributed {
    pub fn wrap_all(tools: Vec<Arc<dyn Tool>>, label: &str) -> Vec<Arc<dyn Tool>> {
        tools
            .into_iter()
            .map(|t| {
                Arc::new(Attributed {
                    inner: t,
                    label: label.to_owned(),
                }) as Arc<dyn Tool>
            })
            .collect()
    }
}

#[async_trait]
impl Tool for Attributed {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn input_schema(&self) -> schemars::Schema {
        self.inner.input_schema()
    }
    fn prompt(&self, ctx: &PromptContext) -> String {
        self.inner.prompt(ctx)
    }
    fn describe(&self, input: &serde_json::Value) -> String {
        format!("[{}] {}", self.label, self.inner.describe(input))
    }
    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        self.inner.call(input, ctx).await
    }
    fn is_read_only(&self, input: &serde_json::Value) -> bool {
        self.inner.is_read_only(input)
    }
    fn is_concurrency_safe(&self, input: &serde_json::Value) -> bool {
        self.inner.is_concurrency_safe(input)
    }
    fn is_destructive(&self, input: &serde_json::Value) -> bool {
        self.inner.is_destructive(input)
    }
    fn interrupt_behavior(&self) -> riot_protocol::tool::InterruptBehavior {
        self.inner.interrupt_behavior()
    }
    fn cascades_on_failure(&self) -> bool {
        self.inner.cascades_on_failure()
    }
    fn result_budget(&self) -> riot_protocol::tool::ResultBudget {
        self.inner.result_budget()
    }
    fn check_permissions(
        &self,
        input: &serde_json::Value,
        ctx: &PermissionContext,
    ) -> PermissionResult {
        self.inner.check_permissions(input, ctx)
    }
    async fn validate_input(
        &self,
        input: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<(), riot_protocol::tool::ValidationError> {
        self.inner.validate_input(input, ctx).await
    }
    fn classifier_input(&self, input: &serde_json::Value) -> Option<String> {
        self.inner.classifier_input(input)
    }
    fn target_path(&self, input: &serde_json::Value) -> Option<PathBuf> {
        self.inner.target_path(input)
    }
    fn should_defer(&self) -> bool {
        self.inner.should_defer()
    }
    fn user_facing_name(&self) -> &str {
        self.inner.user_facing_name()
    }
    fn aliases(&self) -> &[&'static str] {
        self.inner.aliases()
    }
}

/// 一次子 agent 运行的全部材料。同步、后台、续接、分叉四条路都先造它，
/// 再交给 [`run_job`] —— 跑法的差别只在"谁等它、结果交给谁"。
pub struct Job {
    pub agent_id: AgentId,
    pub kind: Kind,
    pub title: String,
    pub provider: Arc<dyn riot_protocol::provider::Provider>,
    pub model: String,
    pub system: String,
    pub tools: Arc<dyn riot_core::state::ToolRunner>,
    /// 起跑时的历史：新起 = 一条 prompt；续接 = 旧历史 + 增量指令；
    /// 分叉 = 父的历史 + 分叉说明。
    pub messages: Vec<Message>,
    pub max_turns: u32,
    pub thinking: riot_protocol::ThinkingPolicy,
    pub max_output_tokens_override: Option<u32>,
    /// transcript。None = 不落盘。
    pub log: Option<riot_store::SessionLog>,
    /// `messages` 里前多少条已经在 transcript 里（续接的旧历史）或不该写
    /// （分叉继承的父历史）。只追加这之后的。
    pub logged: usize,
    /// 界面看这个子 agent 的会话时从第几条开始。新起/续接 = 0（整段都是
    /// 它自己的）；分叉 = 父历史的长度（那是父会话的对话，用户正对着它）。
    pub view_from: usize,
}

/// 一次运行的收场。
pub struct JobOutcome {
    pub status: BackgroundTaskStatus,
    /// 汇报：完成 = 最后一条 assistant 文本；失败 = 原因；取消 = 空。
    pub report: String,
    /// 完整历史（起跑那份 + 这次产生的），续接用。
    pub messages: Vec<Message>,
    pub usage: Usage,
    pub tool_uses: u32,
}

/// 子 agent 跑动中冒出来的东西。
pub enum JobEvent {
    /// 一个动静：调了个工具 / 说了句话。同步路转成父卡片的进度行，
    /// 两条路都更新登记表里的活动行。
    Activity {
        line: String,
        tool_uses: u32,
        tokens: u32,
    },
    /// 一条完整消息（assistant 回复、工具结果）。进登记表，界面打开着
    /// 它的会话时靠这个追上。
    Message(Message),
}

/// 跑一个 [`Job`] 到底。
///
/// 这是四种跑法唯一共用的执行体：装 AgentDeps → run_agent → 收流 →
/// 写 transcript → 归纳收场。不碰登记表、不发通知 —— 那是调用方的事。
pub async fn run_job(
    job: Job,
    ids: Arc<dyn IdGenerator>,
    clock: Arc<dyn riot_protocol::tool::Clock>,
    cancel: CancellationToken,
    on_event: &(dyn Fn(JobEvent) + Send + Sync),
) -> JobOutcome {
    let Job {
        agent_id,
        kind: _,
        title: _,
        provider,
        model,
        system,
        tools,
        messages,
        max_turns,
        thinking,
        max_output_tokens_override,
        log,
        logged,
        view_from: _,
    } = job;

    let deps = AgentDeps {
        provider: Arc::clone(&provider),
        // 压缩也走这一档的模型：便宜档的历史该由便宜模型来总结，
        // 换回主模型等于在省钱的那条路上偷偷把最贵的一步加回去。
        compactor: Arc::new(riot_core::Layered::new(
            Arc::clone(&provider),
            model.clone(),
            riot_core::summarize::RequestShape {
                system: system.clone(),
                tools: tools.specs(),
            },
            Arc::clone(&ids),
            cancel.child_token(),
        )),
        clock: Arc::clone(&clock),
        ids: Arc::clone(&ids),
        tools: Arc::clone(&tools),
        // 子 agent 没有"用户插话"一说 —— 插话进主 agent 的队列。
        queue: Arc::new(riot_core::state::NoQueue),
        // Stop hooks 只管主 agent 的产出。挂到子 agent 上，一次 Task
        // 会触发两层检查，反馈还会互相污染。
        stop_gate: Arc::new(riot_core::state::NoStopGate),
    };

    // 起跑历史里还没落盘的那截先写（新起的 prompt；续接/分叉的旧历史跳过）。
    if let Some(l) = &log {
        for m in &messages[logged.min(messages.len())..] {
            l.append(m);
        }
    }

    let sub_session = SessionId::from_raw(agent_id.as_str().to_owned());
    let state = AgentState::new(sub_session, model.clone())
        .with_messages(messages.clone())
        .with_max_turns(max_turns);
    let state = AgentState {
        system,
        thinking,
        max_output_tokens_override,
        ..state
    };

    let stream = run_agent(state, deps, cancel.clone());
    futures::pin_mut!(stream);

    let mut collected: Vec<Message> = messages;
    let mut usage = Usage::default();
    let mut tool_uses = 0u32;
    let mut terminal: Option<TerminalReason> = None;

    while let Some(mut ev) = stream.next().await {
        match &mut ev {
            AgentEvent::Message(m) => {
                m.stamp(clock.now_ms());
                if let Some(l) = &log {
                    l.append(m);
                }
                if let Message::Assistant {
                    content, usage: u, ..
                } = &*m
                {
                    if let Some(u) = u {
                        usage.merge(u);
                    }
                    for c in content {
                        let line = match c {
                            AssistantContent::ToolUse { name, .. } => {
                                tool_uses += 1;
                                Some(format!("→ {name}"))
                            }
                            AssistantContent::Text { text } => text
                                .lines()
                                .find(|l| !l.trim().is_empty())
                                .map(|f| truncate_chars(f, 120)),
                            _ => None,
                        };
                        if let Some(line) = line {
                            on_event(JobEvent::Activity {
                                line,
                                tool_uses,
                                tokens: usage.input_tokens + usage.output_tokens,
                            });
                        }
                    }
                }
                on_event(JobEvent::Message(m.clone()));
                collected.push(m.clone());
            }
            AgentEvent::Done { reason } => terminal = Some(reason.clone()),
            // Delta/Progress/权限事件不上转：权限弹窗由共享的 gate 直接
            // 发到父会话的事件流（同一个 sink），这里转发会出现两份。
            _ => {}
        }
    }

    if let Some(l) = &log {
        l.flush().await;
    }

    let (status, report) = match terminal {
        Some(TerminalReason::Completed) => match last_assistant_text(&collected) {
            Some(t) => (BackgroundTaskStatus::Completed, t),
            None => (
                BackgroundTaskStatus::Failed,
                "子任务结束但没有产出任何文本结果。把任务描述写得更具体，或拆小再试。".into(),
            ),
        },
        Some(TerminalReason::MaxTurns { .. }) => match last_assistant_text(&collected) {
            Some(t) => (
                BackgroundTaskStatus::Completed,
                format!("{t}\n\n[注意：子任务达到步数上限被停止，以上可能是未完成的结果]"),
            ),
            None => (
                BackgroundTaskStatus::Failed,
                "子任务达到步数上限，且没有产出任何文本结果。".into(),
            ),
        },
        Some(TerminalReason::Aborted { .. }) | Some(TerminalReason::AbortedTools { .. }) => {
            (BackgroundTaskStatus::Cancelled, String::new())
        }
        Some(TerminalReason::Error { error }) => (
            BackgroundTaskStatus::Failed,
            format!("子任务失败：{error:?}。可以调整任务描述重试一次；连续失败就自己动手做。"),
        ),
        // 子 agent 没有 stop hooks；这个变体理论上到不了，但穷举比
        // 通配安全 —— 将来加了 hooks 这里会被编译器点名重审。
        Some(TerminalReason::StopHookPrevented { .. }) => (
            BackgroundTaskStatus::Failed,
            "子任务被 stop hook 拦下 —— 这不该发生在子 agent 上，请上报这个问题。".into(),
        ),
        None => (
            BackgroundTaskStatus::Failed,
            "子任务的事件流异常结束（没有终止事件）".into(),
        ),
    };

    JobOutcome {
        status,
        report,
        messages: collected,
        usage,
        tool_uses,
    }
}

/// 分叉的第一条 user 消息：补齐父历史末尾悬空的 tool_use，再接分叉说明
/// 和任务。
///
/// 父 agent 那条含 Task 调用的 assistant 消息已经在历史里，而它（和同批
/// 的兄弟调用）的结果还没有 —— 带着孤儿 tool_use 组请求会被严格校验的
/// 服务端 400。补上的结果对分叉来说也是实话：那些调用由父执行，结果
/// 不在这条线里。
pub fn fork_prelude(
    history: &[Message],
    agent_id: &AgentId,
    fork_call: &ToolUseId,
    prompt: &str,
) -> Message {
    use riot_protocol::message::{Attachment, ToolResultContent, UserContent};

    let mut content = Vec::new();
    if let Some(last @ Message::Assistant { .. }) = history.last() {
        for id in last.tool_use_ids() {
            let text = if id == fork_call {
                "这就是把你分叉出来的那次调用。你现在是那个后台子 agent。"
            } else {
                "这个调用由主 agent 执行，结果在它那边，你这条线里看不到。"
            };
            content.push(UserContent::ToolResult {
                tool_use_id: id.clone(),
                content: ToolResultContent::text(text),
                is_error: false,
            });
        }
    }
    content.push(UserContent::Attachment(Attachment::SystemReminder {
        text: format!(
            "你是从主 agent 分叉出来的后台子 agent（agent id：{}），继承了到此为止的全部\
             对话和工作区状态。从现在起你独立执行下面的任务；主 agent 只协调，不会重复\
             做你的活。规则：\n\
             - 不要再用 resume=\"self\" 分叉自己（会被拒绝）；需要拆分就开同步的子 agent。\n\
             - 不要向用户提问 —— 用户看不到你的过程，只看得到你的最后一条回复。\n\
             - 你的最后一条回复会作为汇报**原样**交回主 agent，用户也会看到：写清做了\
               什么、改了哪些文件、验证结果如何；失败就如实说。",
            agent_id.as_str()
        ),
    }));
    content.push(UserContent::Text {
        text: format!("任务：{prompt}"),
    });
    Message::User {
        id: riot_protocol::id::MessageId::from_raw(format!("{}_fork", agent_id.as_str())),
        content,
        meta: Default::default(),
    }
}

/// 一次调用解析出来的意图。
enum Plan {
    Fresh { kind: Kind },
    Resume { source: crate::tasks::ResumeSource },
    Fork,
}

impl TaskTool {
    /// 造新起 / 续接的 Job。分叉走 [`TaskHost::fork_job`]。
    fn build_job(
        &self,
        agent_id: AgentId,
        kind: Kind,
        title: String,
        mut messages: Vec<Message>,
        prompt: &str,
        background: bool,
    ) -> Result<Job, String> {
        // ── 成本模型 ──────────────────────────────────────
        // 只读侦察走便宜档（配了的话）；轮数上限按档收窄。父会话的上限
        // 是天花板 —— 用户把主对话调到 8 轮，子 agent 不该偷偷跑 16 轮。
        let (provider, model) = match (kind.prefers_cheap(), self.deps.cheap.as_ref()) {
            (true, Some(c)) => (Arc::clone(&c.provider), c.model.clone()),
            _ => (Arc::clone(&self.deps.provider), self.deps.model.clone()),
        };
        let max_turns = kind.max_turns(self.deps.max_turns);

        // 后台任务的权限弹窗要带归属：弹出来时父轮次多半已经结束，用户
        // 得知道是谁想干这件事。同步的不贴 —— 弹窗就出现在转着圈的 Task
        // 卡片旁边，归属不言自明。
        let tools = if background {
            Attributed::wrap_all(tools_for(kind), &format!("后台任务「{title}」"))
        } else {
            tools_for(kind)
        };
        let prompt_ctx = PromptContext {
            cwd: self.deps.cwd.clone(),
            platform: std::env::consts::OS.to_owned(),
            sandboxed: self.deps.sandbox.is_some(),
            sibling_tools: tools.iter().map(|t| t.name().to_owned()).collect(),
            today: riot_tools::tools::web::date::year_month(self.deps.clock.now_ms()),
        };
        let registry = Registry::new(tools)
            .map(Arc::new)
            .map_err(|e| format!("子 agent 工具装配失败：{e}"))?;
        // 子 agent 的命令套父会话那个沙箱。不套的话共用的闸里那个
        // `sandboxed: true` 就是谎报 —— 见 `SubagentDeps::sandbox`。
        let proc: Arc<dyn riot_protocol::tool::ProcessRunner> = match &self.deps.sandbox {
            Some(sb) => Arc::new(riot_runtime::SandboxedRunner::new(
                Arc::new(SystemProcessRunner::default()),
                Arc::clone(sb),
            )),
            None => Arc::new(SystemProcessRunner::default()),
        };
        let scheduler = Scheduler::new(
            registry,
            prompt_ctx,
            Arc::new(SystemFs::new()),
            proc,
            // 全新的先读后写缓存：子 agent 的"读过"和父互不作数 ——
            // 共享的话，父读过的文件子 agent 没看就能改。
            MemoryFileState::shared() as Arc<dyn riot_protocol::tool::FileStateCache>,
            Arc::clone(&self.deps.ids),
            Arc::clone(&self.deps.clock),
        )
        .with_web(Arc::clone(&self.deps.web))
        .with_vision(Arc::clone(&self.deps.vision))
        .with_gate(Arc::clone(&self.deps.gate))
        .with_artifacts_dir(self.deps.artifacts_dir.clone());

        let logged = messages.len();
        messages.push(Message::User {
            id: riot_protocol::id::MessageId::from_raw(self.deps.ids.next_id("msg")),
            content: vec![riot_protocol::message::UserContent::Text {
                text: prompt.to_owned(),
            }],
            meta: Default::default(),
        });

        let log = self.deps.transcripts.as_ref().map(|t| {
            t.open(riot_store::TranscriptMeta {
                id: SessionId::from_raw(agent_id.as_str().to_owned()),
                root: self.deps.cwd.clone(),
                created_at_ms: self.deps.clock.now_ms(),
            })
        });

        Ok(Job {
            agent_id,
            kind,
            title,
            provider,
            model,
            system: system_prompt_for(kind, &self.deps.cwd),
            tools: Arc::new(scheduler),
            messages,
            max_turns,
            thinking: riot_protocol::ThinkingPolicy::default(),
            max_output_tokens_override: None,
            log,
            logged,
            // 续接的旧历史也是它自己的，整段都给界面看。
            view_from: 0,
        })
    }
}

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        "Task"
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    /// `[约束]` 描述不能随 `depth` 变：分叉的工具清单要和父逐字节一致，
    /// 前缀缓存才命中（见模块文档）。深度差异只体现在运行时的拒绝上。
    fn prompt(&self, _ctx: &PromptContext) -> String {
        "启动一个子 agent 自主完成任务。适合：需要多步探索的调研（不确定东西在哪、\
         要广撒网）、可以并行的独立子问题、一段可独立交付的实现。不适合：读一个已知\
         路径的文件（直接 Read）、找一个具体符号（直接 Grep）—— 那些一步就完，包一层\
         子 agent 只是变慢。\n\n\
         subagent_type 选 `explore`（只读侦察，便宜、可并行）或 `general-purpose`\
         （全工具，能改代码跑命令）。\n\n\
         写 prompt 时把它当成一个刚进门的同事：它**看不到**本对话的任何内容。背景、\
         目标、范围、已排除的方向、相关文件路径都要写进去；要求它汇报什么形式的结果\
         也写明。它的回复不会直接展示给用户 —— 你要自己转述要点。可以在一条消息里\
         并行发起多个 Task。\n\n\
         run_in_background=true 把它放到后台：立刻拿到 agent id，你结束本轮回复即可，\
         它完成后会有一条通知消息（含汇报）唤醒你。适合要跑几分钟以上的实现/测试类\
         任务，以及你还有别的事要同时协调的时候。**不要等它、不要轮询、不要在前台\
         重复它正在做的活**。侦察类的短任务用同步更合适。\n\n\
         resume=<agent id> 续接一个跑过的子 agent（它记得之前的一切，prompt 只写增量\
         指令）；resume=\"self\" 把你自己分叉成后台子 agent 去执行一段实质工作 —— 它\
         继承本对话全部上下文，你不用重述背景；分叉后你在前台只做协调，不要再做同一\
         件事。不要为了并行而过度拆分：小到中等的任务一个子 agent 就够。\n\n\
         在回复里提到某个子 agent 时写成链接 `[任务名](agent:<agent id>)`，用户点它能\
         打开那个子 agent 的完整会话看过程。"
            .into()
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let desc = input
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("子任务");
        let resume = input.get("resume").and_then(|v| v.as_str());
        let bg = input
            .get("run_in_background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let what = match resume {
            Some("self") => "分叉".to_owned(),
            Some(id) => format!("续接 {id}"),
            None => kind_of(input).as_str().to_owned(),
        };
        let mode = if bg || resume == Some("self") {
            "后台"
        } else {
            "同步"
        };
        format!("子 agent（{what}·{mode}）：{desc}")
    }

    /// explore 是只读的（按输入判定）；general-purpose 会写。
    fn is_read_only(&self, input: &serde_json::Value) -> bool {
        kind_of(input).is_read_only()
    }

    /// 并行子 agent 是这个工具的核心价值。写操作的风险由子 agent 内层
    /// 的权限闸逐项把关，外壳不需要独占。
    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    /// 外壳放行：启动子任务本身没有副作用，副作用全在内层工具，而内层
    /// 每一个都过同一个权限闸。在这里多问一次，用户会在"允许启动子任务"
    /// 和"允许子任务里的写文件"上连答两遍 —— 第一遍没有信息量。
    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Preapproved {
                what: "子任务（内部操作仍逐项过权限）".into(),
            },
        }
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let parsed: Input = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutcome::failed(format!("参数不对：{e}")),
        };
        if parsed.prompt.trim().is_empty() {
            return ToolOutcome::failed(
                "prompt 是空的。子 agent 看不到本对话 —— 把背景、目标、范围写进去。",
            );
        }

        // ── 解析意图 ──────────────────────────────────────
        let plan = match parsed.resume.as_deref().map(str::trim) {
            Some("self") => {
                if self.depth > 0 {
                    return ToolOutcome::failed(
                        "你已经是分叉出来的子 agent，不能再分叉自己。需要拆分就开同步的\
                         子 agent（不填 resume，或 resume 填一个已有的 agent id）。",
                    );
                }
                Plan::Fork
            }
            Some(id) if !id.is_empty() => match self.deps.tasks.resume_source(id) {
                Ok(source) => Plan::Resume { source },
                Err(e) => return ToolOutcome::failed(e),
            },
            _ => {
                let requested = parsed.subagent_type.as_deref().unwrap_or("general-purpose");
                match Kind::parse(requested) {
                    Some(kind) => Plan::Fresh { kind },
                    None => {
                        return ToolOutcome::failed(format!(
                            "没有叫「{requested}」的子 agent 类型。可用：general-purpose、explore。"
                        ));
                    }
                }
            }
        };

        // 分叉只能后台（同步等自己的复制品毫无意义）；分叉里的 Task 只能
        // 同步（通知投不到分叉手上，见模块文档）。
        let wants_background = parsed.run_in_background.unwrap_or(false);
        let background = match plan {
            Plan::Fork => true,
            _ => wants_background && self.depth == 0,
        };
        let mut notes: Vec<&str> = Vec::new();
        if wants_background && self.depth > 0 {
            notes.push("（你是分叉出来的子 agent，这里的 Task 只能同步跑，已按同步执行）");
        }

        // ── 造 Job ─────────────────────────────────────────
        let job = match plan {
            Plan::Fresh { kind } => self.build_job(
                self.deps.ids.agent_id(),
                kind,
                parsed.description.clone(),
                Vec::new(),
                &parsed.prompt,
                background,
            ),
            Plan::Resume { source } => {
                let id = AgentId::from_raw(parsed.resume.clone().unwrap_or_default().trim());
                match source.kind {
                    // 分叉的续接：工具是父的那一套，得由会话再造一次。
                    Kind::Fork => self
                        .deps
                        .host
                        .fork_job(&id, &parsed.description, &parsed.prompt, &ctx.tool_use_id)
                        .await
                        .map(|mut job| {
                            // 分叉造出来的 messages 是"父的当前历史 + 分叉说明"；
                            // 续接要的是它自己的历史 + 增量指令。
                            let logged = source.messages.len();
                            let mut messages = source.messages;
                            messages.push(Message::User {
                                id: riot_protocol::id::MessageId::from_raw(
                                    self.deps.ids.next_id("msg"),
                                ),
                                content: vec![riot_protocol::message::UserContent::Text {
                                    text: parsed.prompt.clone(),
                                }],
                                meta: Default::default(),
                            });
                            job.messages = messages;
                            job.logged = logged;
                            job
                        }),
                    kind => self.build_job(
                        id,
                        kind,
                        parsed.description.clone(),
                        source.messages,
                        &parsed.prompt,
                        background,
                    ),
                }
            }
            Plan::Fork => {
                let id = self.deps.ids.agent_id();
                self.deps
                    .host
                    .fork_job(&id, &parsed.description, &parsed.prompt, &ctx.tool_use_id)
                    .await
            }
        };
        let job = match job {
            Ok(j) => j,
            Err(e) => return ToolOutcome::failed(e),
        };

        // ── 登记 ──────────────────────────────────────────
        let agent_id = job.agent_id.clone();
        let kind = job.kind;
        let model = job.model.clone();
        let title = job.title.clone();
        let now = self.deps.clock.now_ms();
        let view = BackgroundTaskView {
            id: agent_id.clone(),
            title: title.clone(),
            kind: kind.as_str().to_owned(),
            model: model.clone(),
            background,
            tool_use_id: ctx.tool_use_id.clone(),
            status: BackgroundTaskStatus::Running,
            activity: "启动".into(),
            tool_uses: 0,
            tokens: 0,
            started_at_ms: now,
            finished_at_ms: None,
        };
        // 后台任务有自己的令牌：父这一轮结束、被停止，都不该把它带走 ——
        // "把重活移出前台"的意思就是前台的生死和它无关。会话关闭时由
        // 登记表统一 cancel_all。同步的跟父的调用一起取消。
        let cancel = if background {
            CancellationToken::new()
        } else {
            ctx.cancel.child_token()
        };
        self.deps.tasks.start(
            view.clone(),
            kind,
            cancel.clone(),
            job.messages.clone(),
            job.view_from,
        );

        // 两条路的动静都进登记表；同步路另外转成父卡片的进度行。
        let tasks_for_events = Arc::clone(&self.deps.tasks);
        let id_for_events = agent_id.clone();
        let progress = (!background).then(|| ctx.progress.clone());
        let on_event = move |ev: JobEvent| match ev {
            JobEvent::Activity {
                line,
                tool_uses,
                tokens,
            } => {
                if let Some(p) = &progress {
                    p.send(ProgressPayload::Line {
                        stream: OutputStream::Stdout,
                        text: line.clone(),
                    });
                }
                tasks_for_events.activity(&id_for_events, line, tool_uses, tokens);
            }
            JobEvent::Message(m) => tasks_for_events.push_message(&id_for_events, m),
        };

        ctx.progress.send(ProgressPayload::Status {
            // 模型名进进度里：不显示的话，"便宜档到底有没有生效"只能去翻日志，
            // 而这正是用户配完之后第一个想确认的事。
            text: format!(
                "[{}·{}] {} {}",
                kind.as_str(),
                model,
                title,
                if background { "后台启动" } else { "启动" }
            ),
        });

        // ── 后台：spawn 走人 ───────────────────────────────
        if background {
            let deps = self.deps.clone();
            let tasks = Arc::clone(&self.deps.tasks);
            let id_for_task = agent_id.clone();
            let model_for_task = model.clone();
            tokio::spawn(async move {
                let model = model_for_task;
                let outcome = run_job(
                    job,
                    Arc::clone(&deps.ids),
                    Arc::clone(&deps.clock),
                    cancel,
                    &on_event,
                )
                .await;
                let now = deps.clock.now_ms();
                let view = tasks.finish(
                    &id_for_task,
                    outcome.status,
                    outcome.messages,
                    outcome.tool_uses,
                    outcome.usage.input_tokens + outcome.usage.output_tokens,
                    now,
                );
                tracing::info!(
                    agent = %id_for_task.as_str(),
                    status = ?outcome.status,
                    "后台子 agent 结束"
                );
                // 被停止的不通知：那是用户亲手按的，他知道；再唤起一轮
                // 让模型对着一句"被停止了"说话，只会烧一次请求。
                if outcome.status == BackgroundTaskStatus::Cancelled {
                    return;
                }
                if let Some(view) = view {
                    let notice = crate::tasks::notice_message(
                        riot_protocol::id::MessageId::from_raw(deps.ids.next_id("msg")),
                        &view,
                        &model,
                        &outcome.report,
                        now,
                    );
                    deps.host.deliver(notice).await;
                }
            });

            let mut text = format!(
                "后台子任务已启动。agent id：{}（{}·{}），标题「{}」。\n\
                 它完成后你会收到一条通知消息，里面有它的汇报。现在不要等它、不要轮询：\
                 结束本轮回复，或者去处理其它独立的事；不要在前台重复它正在做的工作。\
                 之后要给它追加指令，用 Task 的 resume 填这个 agent id。",
                agent_id.as_str(),
                kind.as_str(),
                model,
                title
            );
            for n in &notes {
                text.push('\n');
                text.push_str(n);
            }
            return ToolOutcome::Ok {
                ui_payload: Some(UiPayload::Plain {
                    text: format!("{title} 已在后台启动（{}）", agent_id.as_str()),
                }),
                model_content: riot_protocol::message::ToolResultContent::text(text),
                side_messages: Vec::new(),
            };
        }

        // ── 同步：原地等 ───────────────────────────────────
        let outcome = run_job(
            job,
            Arc::clone(&self.deps.ids),
            Arc::clone(&self.deps.clock),
            cancel,
            &on_event,
        )
        .await;
        let tokens = outcome.usage.input_tokens + outcome.usage.output_tokens;
        self.deps.tasks.finish(
            &agent_id,
            outcome.status,
            outcome.messages,
            outcome.tool_uses,
            tokens,
            self.deps.clock.now_ms(),
        );

        let footer = format!(
            "\n\n[子任务 {id}：{model} · {tokens} tokens · {} 次工具调用 · 可用 resume=\"{id}\" 续接 · \
             回复里引用它写 [{title}](agent:{id})]",
            outcome.tool_uses,
            id = agent_id.as_str(),
        );
        match outcome.status {
            BackgroundTaskStatus::Completed => {
                let mut body = outcome.report;
                for n in &notes {
                    body.push('\n');
                    body.push_str(n);
                }
                ToolOutcome::Ok {
                    ui_payload: Some(UiPayload::Plain {
                        text: format!("{title} 完成{footer}"),
                    }),
                    model_content: riot_protocol::message::ToolResultContent::text(format!(
                        "{body}{footer}"
                    )),
                    side_messages: Vec::new(),
                }
            }
            BackgroundTaskStatus::Cancelled => ToolOutcome::Cancelled,
            BackgroundTaskStatus::Failed | BackgroundTaskStatus::Running => {
                ToolOutcome::failed(outcome.report)
            }
        }
    }
}

fn last_assistant_text(messages: &[Message]) -> Option<String> {
    messages.iter().rev().find_map(|m| match m {
        Message::Assistant { content, .. } => {
            let text: String = content
                .iter()
                .filter_map(|c| match c {
                    AssistantContent::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            (!text.trim().is_empty()).then_some(text)
        }
        _ => None,
    })
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        s.chars().take(max).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use riot_core::testing::ScriptedProvider;
    use riot_protocol::id::{NanoIdGenerator, ToolUseId};
    use riot_protocol::provider::ProviderEvent;
    use riot_protocol::tool::ProgressSink;

    struct AllowAll;
    #[async_trait]
    impl PermissionGate for AllowAll {
        async fn check(
            &self,
            _tool: &dyn Tool,
            _input: &serde_json::Value,
            _id: &ToolUseId,
            _cancel: &tokio_util::sync::CancellationToken,
        ) -> riot_protocol::permission::GateOutcome {
            riot_protocol::permission::GateOutcome::Allow {
                updated_input: None,
            }
        }
    }

    /// 收通知的替身：攒起来给断言看。
    struct CollectHost(std::sync::Mutex<Vec<Message>>, tokio::sync::Notify);

    impl CollectHost {
        fn new() -> Arc<Self> {
            Arc::new(Self(
                std::sync::Mutex::new(Vec::new()),
                tokio::sync::Notify::new(),
            ))
        }
        fn taken(&self) -> Vec<Message> {
            self.0.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl TaskHost for CollectHost {
        async fn deliver(&self, notice: Message) {
            self.0.lock().unwrap().push(notice);
            self.1.notify_waiters();
            self.1.notify_one();
        }
        async fn fork_job(
            &self,
            _agent_id: &AgentId,
            _title: &str,
            _prompt: &str,
            _fork_call: &ToolUseId,
        ) -> Result<Job, String> {
            Err("测试替身不分叉".into())
        }
    }

    fn assistant(text: &str) -> Message {
        Message::Assistant {
            id: riot_protocol::id::MessageId::from_raw("a1"),
            content: vec![AssistantContent::Text { text: text.into() }],
            usage: Some(Usage {
                input_tokens: 100,
                output_tokens: 50,
                ..Default::default()
            }),
            meta: Default::default(),
        }
    }

    fn deps_with_host(provider: Arc<ScriptedProvider>, host: Arc<dyn TaskHost>) -> SubagentDeps {
        SubagentDeps {
            provider,
            model: "test-model".into(),
            cheap: None,
            gate: Arc::new(AllowAll),
            sandbox: None,
            web: Arc::new(riot_protocol::web::NoWeb),
            vision: Arc::new(riot_protocol::vision::NoVision),
            clock: Arc::new(riot_providers::watchdog::TokioClock),
            ids: Arc::new(NanoIdGenerator),
            cwd: "/tmp".into(),
            artifacts_dir: std::env::temp_dir(),
            max_turns: 8,
            transcripts: None,
            tasks: Arc::new(BackgroundTasks::new(crate::session::SessionSink::default())),
            host,
        }
    }

    fn deps(provider: Arc<ScriptedProvider>) -> SubagentDeps {
        deps_with_host(provider, Arc::new(NoTaskHost))
    }

    fn ctx() -> (
        ToolContext,
        tokio::sync::mpsc::UnboundedReceiver<(ToolUseId, ProgressPayload)>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let id = ToolUseId::from_raw("t1");
        (
            ToolContext {
                session_id: SessionId::from_raw("s1"),
                tool_use_id: id.clone(),
                cwd: "/tmp".into(),
                artifacts_dir: std::env::temp_dir(),
                cancel: tokio_util::sync::CancellationToken::new(),
                progress: ProgressSink::new(id, tx),
                file_state: MemoryFileState::shared(),
                fs: Arc::new(SystemFs::new()),
                proc: Arc::new(SystemProcessRunner::default()),
                web: Arc::new(riot_protocol::web::NoWeb),
                browser: Arc::new(riot_protocol::browser::NoBrowser),
                terminal: Arc::new(riot_protocol::terminal::NoTerminal),
                vision: Arc::new(riot_protocol::vision::NoVision),
                clock: Arc::new(riot_providers::watchdog::TokioClock),
            },
            rx,
        )
    }

    fn model_text(out: &ToolOutcome) -> String {
        match out {
            ToolOutcome::Ok { model_content, .. } => format!("{model_content:?}"),
            other => panic!("该成功：{other:?}"),
        }
    }

    #[tokio::test]
    async fn 子任务的最后一条文本回给父_带用量脚注() {
        let provider = Arc::new(ScriptedProvider::new(vec![vec![ProviderEvent::Message(
            assistant("调查完成：入口在 src/main.rs:42"),
        )]]));
        let tool = TaskTool::new(deps(Arc::clone(&provider)));
        let (c, _rx) = ctx();

        let out = tool
            .call(
                serde_json::json!({
                    "description": "找入口",
                    "prompt": "在这个仓库里找程序入口",
                    "subagent_type": "explore"
                }),
                c,
            )
            .await;

        let text = model_text(&out);
        assert!(text.contains("src/main.rs:42"), "子 agent 的报告要原样回来");
        assert!(text.contains("150 tokens"), "用量脚注要在：{text}");
        assert!(text.contains("resume="), "脚注要告诉模型能续接：{text}");

        // 子 agent 的请求不该带 Task 工具（递归在结构上不存在）
        let reqs = provider.requests();
        assert!(
            reqs[0].tools.iter().all(|t| t.name != "Task"),
            "子 agent 的工具清单里不能有 Task"
        );
        assert!(
            reqs[0].tools.iter().any(|t| t.name == "Read"),
            "explore 有只读工具"
        );
        assert!(
            reqs[0]
                .tools
                .iter()
                .all(|t| t.name != "Write" && t.name != "Bash"),
            "explore 不能有写工具"
        );
        assert!(
            reqs[0].system.contains("只读"),
            "explore 的系统提示要立只读规矩"
        );
    }

    #[tokio::test]
    async fn 未知类型报可用清单() {
        let provider = Arc::new(ScriptedProvider::new(vec![]));
        let tool = TaskTool::new(deps(provider));
        let (c, _rx) = ctx();
        let out = tool
            .call(
                serde_json::json!({ "description": "x", "prompt": "y", "subagent_type": "ninja" }),
                c,
            )
            .await;
        let ToolOutcome::Failed {
            error_for_model, ..
        } = out
        else {
            panic!("该失败")
        };
        assert!(
            error_for_model.contains("general-purpose"),
            "{error_for_model}"
        );
    }

    #[test]
    fn explore_按输入判定为只读() {
        let provider = Arc::new(ScriptedProvider::new(vec![]));
        let tool = TaskTool::new(deps(provider));
        assert!(tool.is_read_only(&serde_json::json!({ "subagent_type": "explore" })));
        assert!(!tool.is_read_only(&serde_json::json!({ "subagent_type": "general-purpose" })));
        assert!(
            !tool.is_read_only(&serde_json::json!({})),
            "缺省是 general-purpose，会写"
        );
        assert!(
            !tool.is_read_only(&serde_json::json!({ "subagent_type": "ninja" })),
            "认不出的类型要落在 fail-closed 那侧（会写），不能因为不认识就当只读并行掉"
        );
        assert!(
            !tool.is_read_only(
                &serde_json::json!({ "subagent_type": "explore", "resume": "agt_x" })
            ),
            "续接的类型跟原来的、这里不知道 —— 按会写处理"
        );
    }

    /// 便宜档存在的全部意义。走错模型不会报错、不会崩，只是账单变贵 ——
    /// 没有断言守着的话，谁在 `call` 里把 `provider` 换回 `self.deps.provider`
    /// 都不会有测试变红。
    #[tokio::test]
    async fn 只读侦察走便宜档_而_general_purpose_不走() {
        let script = || {
            Arc::new(ScriptedProvider::new(vec![vec![ProviderEvent::Message(
                assistant("报告：看完了"),
            )]]))
        };

        for (kind, cheap_used) in [("explore", true), ("general-purpose", false)] {
            let main = script();
            let cheap = script();
            let tool = TaskTool::new(SubagentDeps {
                cheap: Some(CheapModel {
                    provider: Arc::clone(&cheap) as Arc<dyn riot_protocol::provider::Provider>,
                    model: "cheap-model".into(),
                }),
                ..deps(Arc::clone(&main))
            });
            let (c, _rx) = ctx();
            let out = tool
                .call(
                    serde_json::json!({
                        "description": "看看",
                        "prompt": "看看这个仓库",
                        "subagent_type": kind
                    }),
                    c,
                )
                .await;
            assert!(
                matches!(out, ToolOutcome::Ok { .. }),
                "{kind} 该成功：{out:?}"
            );

            let (hit, miss) = if cheap_used {
                (&cheap, &main)
            } else {
                (&main, &cheap)
            };
            assert_eq!(hit.requests().len(), 1, "{kind} 该打中这一档");
            assert!(miss.requests().is_empty(), "{kind} 不该打另一档");
            assert_eq!(
                hit.requests()[0].model,
                if cheap_used {
                    "cheap-model"
                } else {
                    "test-model"
                },
                "{kind} 的请求里带的模型名不对"
            );
        }
    }

    /// 没配便宜档时不能悄悄少跑 —— 一切照旧走主模型。
    #[tokio::test]
    async fn 没配便宜档时只读侦察也走主模型() {
        let main = Arc::new(ScriptedProvider::new(vec![vec![ProviderEvent::Message(
            assistant("报告"),
        )]]));
        let tool = TaskTool::new(deps(Arc::clone(&main)));
        let (c, _rx) = ctx();
        let out = tool
            .call(
                serde_json::json!({ "description": "x", "prompt": "y", "subagent_type": "explore" }),
                c,
            )
            .await;
        assert!(matches!(out, ToolOutcome::Ok { .. }), "{out:?}");
        assert_eq!(main.requests()[0].model, "test-model");
    }

    /// 父会话的上限是天花板：用户把主对话调低，子 agent 不能反而跑更多轮。
    #[test]
    fn 轮数上限只能收窄不能放宽() {
        assert_eq!(
            Kind::Explore.max_turns(48),
            EXPLORE_MAX_TURNS,
            "侦察档要被压到上限"
        );
        assert_eq!(Kind::Explore.max_turns(4), 4, "父比上限还小时跟父");
        assert_eq!(Kind::GeneralPurpose.max_turns(48), 48, "执行档跟父");
        for parent in [1u32, 8, 16, 48, 1000] {
            assert!(
                Kind::Explore.max_turns(parent) <= parent,
                "parent={parent} 时侦察档超过了父会话的上限"
            );
        }
    }

    /// 后台：工具调用立刻返回 agent id；跑完通知交给宿主，通知里有汇报。
    #[tokio::test]
    // 等一个 spawn 出去的后台任务收场，真超时是刻意的（没有注入时钟可推）。
    #[allow(clippy::disallowed_methods)]
    async fn 后台任务立刻返回_跑完把通知交给宿主() {
        let provider = Arc::new(ScriptedProvider::new(vec![vec![ProviderEvent::Message(
            assistant("改完了：src/a.rs"),
        )]]));
        let host = CollectHost::new();
        let d = deps_with_host(
            Arc::clone(&provider),
            Arc::clone(&host) as Arc<dyn TaskHost>,
        );
        let tasks = Arc::clone(&d.tasks);
        let tool = TaskTool::new(d);
        let (c, _rx) = ctx();

        let out = tool
            .call(
                serde_json::json!({
                    "description": "改 a",
                    "prompt": "把 a.rs 改一下",
                    "run_in_background": true
                }),
                c,
            )
            .await;
        let text = model_text(&out);
        assert!(text.contains("后台子任务已启动"), "{text}");
        assert!(text.contains("不要等它"), "要告诉模型别等：{text}");
        assert_eq!(tasks.snapshot().len(), 1, "面板上要有它");

        // 等通知到
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if !host.taken().is_empty() {
                    break;
                }
                host.1.notified().await;
            }
        })
        .await
        .expect("5 秒内该收到通知");

        let notices = host.taken();
        assert_eq!(notices.len(), 1);
        let Message::User { meta, content, .. } = &notices[0] else {
            panic!("通知该是 user 消息");
        };
        let notice = meta.task_notice.as_ref().expect("要带标记");
        assert_eq!(notice.status, BackgroundTaskStatus::Completed);
        assert_eq!(notice.title, "改 a");
        assert!(
            format!("{content:?}").contains("src/a.rs"),
            "通知里要有汇报：{content:?}"
        );

        let snap = tasks.snapshot();
        assert_eq!(snap[0].status, BackgroundTaskStatus::Completed);
        assert!(
            tasks.resume_source(snap[0].id.as_str()).is_ok(),
            "跑完的后台任务能续接"
        );
    }

    /// 续接：旧历史 + 新指令一起发；请求里能看到旧的报告；类型沿用原来的。
    #[tokio::test]
    async fn 续接带着旧历史_沿用原类型() {
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![ProviderEvent::Message(assistant("第一次：找到 3 处"))],
            vec![ProviderEvent::Message(assistant("第二次：详情如下"))],
        ]));
        let tool = TaskTool::new(deps(Arc::clone(&provider)));

        let (c, _rx) = ctx();
        let out = tool
            .call(
                serde_json::json!({
                    "description": "找用法",
                    "prompt": "找 foo 的用法",
                    "subagent_type": "explore"
                }),
                c,
            )
            .await;
        let first = model_text(&out);
        let id = first
            .split("resume=\\\"")
            .nth(1)
            .and_then(|s| s.split('\\').next())
            .expect("脚注里要有 agent id")
            .to_owned();

        let (c, _rx) = ctx();
        let out = tool
            .call(
                serde_json::json!({
                    "description": "找用法",
                    "prompt": "展开说说第 2 处",
                    "resume": id,
                    "subagent_type": "general-purpose"
                }),
                c,
            )
            .await;
        let second = model_text(&out);
        assert!(second.contains("第二次"), "{second}");

        let reqs = provider.requests();
        assert_eq!(reqs.len(), 2);
        let r = &reqs[1];
        let history = format!("{:?}", r.messages);
        assert!(history.contains("找 foo 的用法"), "旧 prompt 要在");
        assert!(history.contains("第一次：找到 3 处"), "旧报告要在");
        assert!(history.contains("展开说说第 2 处"), "新指令要在");
        assert!(
            r.tools.iter().all(|t| t.name != "Write"),
            "续接沿用 explore，不能因为传了 general-purpose 就升级成会写：{:?}",
            r.tools.iter().map(|t| &t.name).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn 续接不存在的id报错() {
        let provider = Arc::new(ScriptedProvider::new(vec![]));
        let tool = TaskTool::new(deps(provider));
        let (c, _rx) = ctx();
        let out = tool
            .call(
                serde_json::json!({ "description": "x", "prompt": "y", "resume": "agt_nope" }),
                c,
            )
            .await;
        let ToolOutcome::Failed {
            error_for_model, ..
        } = out
        else {
            panic!("该失败")
        };
        assert!(error_for_model.contains("没有叫"), "{error_for_model}");
    }

    /// 深度计数器：分叉里的 Task 拒绝再分叉，后台请求被降成同步。
    #[tokio::test]
    async fn 分叉里不能再分叉_后台降成同步() {
        let provider = Arc::new(ScriptedProvider::new(vec![vec![ProviderEvent::Message(
            assistant("同步跑完"),
        )]]));
        let tool = TaskTool::forked(deps(Arc::clone(&provider)));

        let (c, _rx) = ctx();
        let out = tool
            .call(
                serde_json::json!({ "description": "x", "prompt": "y", "resume": "self" }),
                c,
            )
            .await;
        let ToolOutcome::Failed {
            error_for_model, ..
        } = out
        else {
            panic!("分叉的分叉该被拒")
        };
        assert!(error_for_model.contains("不能再分叉"), "{error_for_model}");

        let (c, _rx) = ctx();
        let out = tool
            .call(
                serde_json::json!({
                    "description": "x", "prompt": "y",
                    "subagent_type": "explore", "run_in_background": true
                }),
                c,
            )
            .await;
        let text = model_text(&out);
        assert!(text.contains("同步跑完"), "该同步等到结果：{text}");
        assert!(
            text.contains("已按同步执行"),
            "要告诉它为什么没后台：{text}"
        );
    }

    /// 主 agent 里的分叉：宿主不支持时报错，不会悄悄退化成普通子 agent。
    #[tokio::test]
    async fn 宿主不支持分叉时明说() {
        let provider = Arc::new(ScriptedProvider::new(vec![]));
        let tool = TaskTool::new(deps(provider));
        let (c, _rx) = ctx();
        let out = tool
            .call(
                serde_json::json!({ "description": "x", "prompt": "y", "resume": "self" }),
                c,
            )
            .await;
        let ToolOutcome::Failed {
            error_for_model, ..
        } = out
        else {
            panic!("该失败")
        };
        assert!(error_for_model.contains("分叉"), "{error_for_model}");
    }

    /// 归属包装只改 describe：弹窗那句话带上任务名，请求形状（name /
    /// schema / prompt）一个字都不动 —— 分叉的缓存命中靠这个。
    #[test]
    fn 归属包装只改弹窗文案不改请求形状() {
        let raw: Vec<Arc<dyn Tool>> = vec![Arc::new(riot_tools::tools::Bash)];
        let wrapped = Attributed::wrap_all(raw.clone(), "后台任务「跑测试」");
        let (a, b) = (&raw[0], &wrapped[0]);
        let ctx = PromptContext {
            cwd: "/tmp".into(),
            platform: "macos".into(),
            sandboxed: false,
            sibling_tools: vec![],
            today: "2026年9月".into(),
        };
        let input = serde_json::json!({ "command": "cargo test" });
        assert_eq!(a.name(), b.name());
        assert_eq!(a.prompt(&ctx), b.prompt(&ctx));
        assert_eq!(
            serde_json::to_string(&a.input_schema()).unwrap(),
            serde_json::to_string(&b.input_schema()).unwrap()
        );
        assert_eq!(a.is_read_only(&input), b.is_read_only(&input));
        let d = b.describe(&input);
        assert!(d.starts_with("[后台任务「跑测试」]"), "{d}");
        assert!(d.contains(&a.describe(&input)), "{d}");
    }

    /// 分叉的第一条消息要把父末尾悬空的 tool_use 全补上，否则严格校验的
    /// 服务端直接 400。
    #[test]
    fn 分叉前奏补齐悬空的工具调用() {
        let history = vec![
            Message::User {
                id: riot_protocol::id::MessageId::from_raw("u1"),
                content: vec![riot_protocol::message::UserContent::Text {
                    text: "干活".into(),
                }],
                meta: Default::default(),
            },
            Message::Assistant {
                id: riot_protocol::id::MessageId::from_raw("a1"),
                content: vec![
                    AssistantContent::ToolUse {
                        id: ToolUseId::from_raw("tu_read"),
                        name: "Read".into(),
                        input: serde_json::json!({}),
                    },
                    AssistantContent::ToolUse {
                        id: ToolUseId::from_raw("tu_fork"),
                        name: "Task".into(),
                        input: serde_json::json!({ "resume": "self" }),
                    },
                ],
                usage: None,
                meta: Default::default(),
            },
        ];
        let m = fork_prelude(
            &history,
            &AgentId::from_raw("agt_1"),
            &ToolUseId::from_raw("tu_fork"),
            "把测试跑一遍",
        );
        let ids: Vec<&ToolUseId> = m.tool_result_ids();
        assert_eq!(ids.len(), 2, "两个悬空调用都要补：{m:?}");
        let Message::User { content, .. } = &m else {
            unreachable!()
        };
        assert!(
            matches!(
                &content[0],
                riot_protocol::message::UserContent::ToolResult { .. }
            ),
            "tool_result 要排在最前"
        );
        let text = format!("{content:?}");
        assert!(text.contains("分叉出来的那次调用"));
        assert!(text.contains("把测试跑一遍"));
        assert!(text.contains("不要再用 resume"));
    }
}
