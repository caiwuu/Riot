//! 子 agent：Task 工具。
//!
//! # 骨架（对照 Claude Code 的 AgentTool / runAgent）
//!
//! Task 就是**再跑一遍主循环**：独立的系统提示、独立的工具集、全新的
//! 上下文（一条 user 消息），跑完取最后一条 assistant 文本回给父。
//! 它不是旁路 —— 子 agent 的每个工具调用走和父完全相同的调度器和
//! 权限闸（同一个 HostGate，弹窗、规则、模式全部一致）。
//!
//! # 两个内置类型
//!
//! - `general-purpose`：全套文件/命令/联网工具，自主完成多步任务；
//! - `explore`：**只读**侦察（Read/Grep/Glob/WebFetch/WebSearch），
//!   给"到处找找"这类任务 —— 便宜、可并行、绝不改东西。
//!
//! # 类型声明成本，不只声明工具 ⭐
//!
//! [`Kind`] 除了决定给哪些工具，还决定**这一档愿意花多少钱**：用哪个模型、
//! 最多跑几轮。这不是锦上添花 —— 只读侦察的产出是一份文字报告，却往往比
//! 主对话吃掉更多 token（几十次 Grep/Read 的结果全进它的上下文），用和主
//! 循环同一档的模型跑它是这类架构里最容易漏掉的一笔开销。
//!
//! `[约束]` 预算属于类型，不属于调用参数。模型填 `subagent_type` 是在选
//! 一个**已经定好价的档**，它没有任何办法给自己多要预算。
//!
//! # 递归与并发
//!
//! 子 agent 的注册表里**没有 Task 工具**，递归在结构上就不存在
//! （CC 的教训清单："子 agent 能再 spawn 子 agent → 无限递归"）。
//! 并发安全：多个 Task 可以同批并行 —— 这正是它的核心价值；写操作的
//! 风险由子 agent 内层的权限闸逐项把关，外壳不需要独占。
//!
//! # 结果与可观测性
//!
//! - 结果 = 最后一条有文本的 assistant 消息（CC 同款），附用量脚注；
//! - 过程**原样**套进 [`ProgressPayload::Nested`] 流回父会话的 Task 卡片：
//!   子 agent 的每条 Delta / Message / Progress / Done 都是一条嵌套事件，
//!   界面据此在卡片里画出一条完整的子时间线（思考、正文、每个工具调用
//!   的参数和输出、直播中的半截流）—— 见 [`forwards_to_parent`]；
//! - transcript 落在 `sessions/subagents/<会话>/<agent>.jsonl`，和主
//!   transcript 隔开 —— 放同一目录会被索引重建当成会话捞回来。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;

use riot_core::{AgentDeps, AgentState, run_agent};
use riot_protocol::event::{AgentEvent, ProgressPayload, TerminalReason};
use riot_protocol::id::{IdGenerator, SessionId};
use riot_protocol::message::{AssistantContent, Message, Usage};
use riot_protocol::permission::{
    DecisionReason, PermissionContext, PermissionGate, PermissionResult,
};
use riot_protocol::tool::{PromptContext, Tool, ToolContext, ToolOutcome, UiPayload};
use riot_runtime::{MemoryFileState, SystemFs, SystemProcessRunner};
use riot_tools::registry::Registry;
use riot_tools::scheduler::Scheduler;

/// 只读侦察档用的便宜模型。
///
/// None（[`SubagentDeps::cheap`] 为空）= 没配，全部类型都跟主模型。
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

/// 组装一个子 agent 轮次所需的一切。由 run_inner 从当轮快照。
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
}

#[derive(Deserialize, schemars::JsonSchema)]
struct Input {
    /// 三五个词的任务名，显示在父会话的进度里。
    description: String,
    /// 给子 agent 的完整任务描述。它看不到本对话的任何内容 ——
    /// 背景、目标、范围、已知线索都要写进来。
    prompt: String,
    /// `general-purpose`（默认，全工具）或 `explore`（只读侦察）。
    #[serde(default)]
    subagent_type: Option<String>,
}

pub struct TaskTool {
    deps: SubagentDeps,
}

impl TaskTool {
    pub fn new(deps: SubagentDeps) -> Self {
        Self { deps }
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

    fn as_str(self) -> &'static str {
        match self {
            Self::GeneralPurpose => "general-purpose",
            Self::Explore => "explore",
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
            Self::GeneralPurpose => parent,
            Self::Explore => parent.min(EXPLORE_MAX_TURNS),
        }
    }
}

/// 从工具入参里读类型。缺省和**认不出的**都算 general-purpose ——
/// 这是 fail-closed 的那一侧（会写、不便宜、不被当成只读）。真正的
/// 拒绝在 [`TaskTool::call`] 里，那里能给模型一句可用清单。
fn kind_of(input: &serde_json::Value) -> Kind {
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
        Kind::GeneralPurpose => vec![
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
        Kind::GeneralPurpose => format!(
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

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        "Task"
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        // CC AgentTool prompt 的精华：何时用/不该用/怎么写任务描述。
        "启动一个子 agent 自主完成任务。适合：需要多步探索的调研（不确定\
         东西在哪、要广撒网）、可以并行的独立子问题、一段可独立交付的实现。\
         不适合：读一个已知路径的文件（直接 Read）、找一个具体符号（直接 \
         Grep）—— 那些一步就完，包一层子 agent 只是变慢。\n\n\
         subagent_type 选 `explore`（只读侦察，便宜、可并行）或 \
         `general-purpose`（全工具，能改代码跑命令）。\n\n\
         写 prompt 时把它当成一个刚进门的同事：它**看不到**本对话的任何内容。\
         背景、目标、范围、已排除的方向、相关文件路径都要写进去；要求它汇报\
         什么形式的结果也写明。它的回复不会直接展示给用户 —— 你要自己转述\
         要点。可以在一条消息里并行发起多个 Task。"
            .into()
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let desc = input
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("子任务");
        format!("子 agent（{}）：{desc}", kind_of(input).as_str())
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
        let requested = parsed.subagent_type.as_deref().unwrap_or("general-purpose");
        let Some(kind) = Kind::parse(requested) else {
            return ToolOutcome::failed(format!(
                "没有叫「{requested}」的子 agent 类型。可用：general-purpose、explore。"
            ));
        };

        // ── 成本模型 ──────────────────────────────────────
        // 只读侦察走便宜档（配了的话）；轮数上限按档收窄。父会话的上限
        // 是天花板 —— 用户把主对话调到 8 轮，子 agent 不该偷偷跑 16 轮。
        let (provider, model) = match (kind.prefers_cheap(), self.deps.cheap.as_ref()) {
            (true, Some(c)) => (Arc::clone(&c.provider), c.model.clone()),
            _ => (Arc::clone(&self.deps.provider), self.deps.model.clone()),
        };
        let max_turns = kind.max_turns(self.deps.max_turns);

        // 模型名不用单独报：子 agent 的第一条 RequestStart 就带着它，会随
        // 嵌套事件流到卡片上 —— "便宜档到底有没有生效"用户在界面上直接
        // 看得到，不用翻日志。
        let agent_id = self.deps.ids.agent_id();

        // ── 装配子 agent 的一轮 ────────────────────────────
        let tools = tools_for(kind);
        let prompt_ctx = PromptContext {
            cwd: self.deps.cwd.clone(),
            platform: std::env::consts::OS.to_owned(),
            sandboxed: self.deps.sandbox.is_some(),
            sibling_tools: tools.iter().map(|t| t.name().to_owned()).collect(),
            today: riot_tools::tools::web::date::year_month(self.deps.clock.now_ms()),
        };
        let registry = match Registry::new(tools) {
            Ok(r) => Arc::new(r),
            Err(e) => return ToolOutcome::failed(format!("子 agent 工具装配失败：{e}")),
        };
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

        let tools_runner: Arc<dyn riot_core::state::ToolRunner> = Arc::new(scheduler);
        // 子 agent 的 system 也提前定：总结请求同形状（见 RequestShape）。
        let system = system_prompt_for(kind, &self.deps.cwd);

        let sub_session = SessionId::from_raw(agent_id.as_str().to_owned());
        let deps = AgentDeps {
            provider: Arc::clone(&provider),
            // 压缩也走这一档的模型：便宜档的历史该由便宜模型来总结，
            // 换回主模型等于在省钱的那条路上偷偷把最贵的一步加回去。
            compactor: Arc::new(riot_core::Layered::new(
                Arc::clone(&provider),
                model.clone(),
                riot_core::summarize::RequestShape {
                    system: system.clone(),
                    tools: tools_runner.specs(),
                },
                Arc::clone(&self.deps.ids),
                ctx.cancel.child_token(),
            )),
            clock: Arc::clone(&self.deps.clock),
            ids: Arc::clone(&self.deps.ids),
            tools: Arc::clone(&tools_runner),
            // 子 agent 没有"用户插话"一说 —— 插话进主 agent 的队列。
            queue: Arc::new(riot_core::state::NoQueue),
            // Stop hooks 只管主 agent 的产出。挂到子 agent 上，一次 Task
            // 会触发两层检查，反馈还会互相污染。
            stop_gate: Arc::new(riot_core::state::NoStopGate),
        };

        let user_msg = Message::User {
            id: riot_protocol::id::MessageId::from_raw(self.deps.ids.next_id("msg")),
            content: vec![riot_protocol::message::UserContent::Text {
                text: parsed.prompt.clone(),
            }],
            meta: Default::default(),
        };
        let state = AgentState::new(sub_session.clone(), model.clone())
            .with_messages(vec![user_msg.clone()])
            .with_max_turns(max_turns);
        let state = AgentState { system, ..state };

        // transcript：独立文件，和主会话隔开。
        let log = self.deps.transcripts.as_ref().map(|t| {
            let log = t.open(riot_store::TranscriptMeta {
                id: sub_session,
                root: self.deps.cwd.clone(),
                created_at_ms: self.deps.clock.now_ms(),
            });
            log.append(&user_msg);
            log
        });

        // ── 跑 ────────────────────────────────────────────
        let stream = run_agent(state, deps, ctx.cancel.child_token());
        futures::pin_mut!(stream);

        let mut last_text: Option<String> = None;
        let mut usage = Usage::default();
        let mut tool_uses = 0u32;
        let mut terminal: Option<TerminalReason> = None;

        while let Some(ev) = stream.next().await {
            // 先借着记账，再把事件整个搬进嵌套载荷 —— 消息里可能带图，
            // 不该为了留一份副本多克隆一次。
            match &ev {
                AgentEvent::Message(m) => {
                    if let Some(l) = &log {
                        l.append(m);
                    }
                    if let Message::Assistant {
                        content, usage: u, ..
                    } = m
                    {
                        if let Some(u) = u {
                            usage.merge(u);
                        }
                        tool_uses += content
                            .iter()
                            .filter(|c| matches!(c, AssistantContent::ToolUse { .. }))
                            .count() as u32;
                        if let Some(t) = assistant_text(content) {
                            last_text = Some(t);
                        }
                    }
                }
                AgentEvent::Done { reason } => terminal = Some(reason.clone()),
                _ => {}
            }
            // 过程原样套给父卡片：界面拿这条流画子时间线。合帧由宿主那层
            // 统一做（嵌套的 Delta 和主 agent 的一样按帧攒），这里不攒。
            if forwards_to_parent(&ev) {
                ctx.progress.send(ProgressPayload::Nested {
                    event: Box::new(ev),
                });
            }
        }

        if let Some(l) = &log {
            l.flush().await;
        }

        // ── 收尾 ──────────────────────────────────────────
        let footer = format!(
            "\n\n[子任务 {}：{} · {} tokens · {} 次工具调用]",
            agent_id.as_str(),
            model,
            usage.input_tokens + usage.output_tokens,
            tool_uses,
        );
        match terminal {
            Some(TerminalReason::Completed) | Some(TerminalReason::MaxTurns { .. }) => {
                match last_text {
                    Some(t) => {
                        let capped = if matches!(terminal, Some(TerminalReason::MaxTurns { .. })) {
                            format!(
                                "{t}\n\n[注意：子任务达到步数上限被停止，以上可能是未完成的结果]"
                            )
                        } else {
                            t
                        };
                        ToolOutcome::Ok {
                            ui_payload: Some(UiPayload::Plain {
                                text: format!("{} 完成{footer}", parsed.description),
                            }),
                            model_content: riot_protocol::message::ToolResultContent::text(
                                format!("{capped}{footer}"),
                            ),
                            side_messages: Vec::new(),
                        }
                    }
                    None => ToolOutcome::failed(
                        "子任务结束但没有产出任何文本结果。把任务描述写得更具体，或拆小再试。",
                    ),
                }
            }
            Some(TerminalReason::Aborted { .. }) | Some(TerminalReason::AbortedTools { .. }) => {
                ToolOutcome::Cancelled
            }
            Some(TerminalReason::Error { error }) => ToolOutcome::failed(format!(
                "子任务失败：{error:?}。可以调整任务描述重试一次；连续失败就自己动手做。"
            )),
            // 子 agent 没有 stop hooks；这个变体理论上到不了，但穷举比
            // 通配安全 —— 将来加了 hooks 这里会被编译器点名重审。
            Some(TerminalReason::StopHookPrevented { .. }) => ToolOutcome::failed(
                "子任务被 stop hook 拦下 —— 这不该发生在子 agent 上，请上报这个问题。",
            ),
            None => ToolOutcome::failed("子任务的事件流异常结束（没有终止事件）"),
        }
    }
}

/// 一条 assistant 消息里的全部文本块。没有文本（纯工具调用）返回 None，
/// 这样"最后一条**有文本**的消息"只要在流里不断覆盖就得到了。
fn assistant_text(content: &[AssistantContent]) -> Option<String> {
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

/// 子 agent 的哪些事件要套进父卡片。
///
/// 默认都转 —— 界面要的就是"子 agent 在干什么"的完整画面，缺一类就是
/// 一段空白。刻意留下的例外：
///
/// - 权限两兄弟：子 agent 和父共用一个闸，弹窗由闸直接发到父会话的
///   事件流（同一个 sink），这里再转一份就是两个弹窗；
/// - 模式切换 / 撤回提问：这两件事只有主会话会发生（子 agent 没有
///   ExitPlanMode，也没有用户在它开口前按停止），真出现了也是主会话的
///   事，不该在卡片里冒出来。
fn forwards_to_parent(ev: &AgentEvent) -> bool {
    !matches!(
        ev,
        AgentEvent::PermissionRequest { .. }
            | AgentEvent::PermissionResolved { .. }
            | AgentEvent::ModeChanged { .. }
            | AgentEvent::PromptWithdrawn { .. }
    )
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

    fn deps(provider: Arc<ScriptedProvider>) -> SubagentDeps {
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
        }
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

        let ToolOutcome::Ok { model_content, .. } = out else {
            panic!("该成功：{out:?}");
        };
        let text = format!("{model_content:?}");
        assert!(text.contains("src/main.rs:42"), "子 agent 的报告要原样回来");
        assert!(text.contains("150 tokens"), "用量脚注要在：{text}");

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

    /// 卡片上的子时间线全靠这条流。少转一类事件，界面上就是一段空白 ——
    /// 而且没有任何报错：子 agent 照常跑完、结果照常回来。
    #[tokio::test]
    async fn 子_agent_的过程原样套进父卡片的进度流() {
        let provider = Arc::new(ScriptedProvider::new(vec![vec![ProviderEvent::Message(
            assistant("报告：入口在 src/main.rs:1"),
        )]]));
        let tool = TaskTool::new(deps(Arc::clone(&provider)));
        let (c, mut rx) = ctx();

        let out = tool
            .call(
                serde_json::json!({ "description": "找入口", "prompt": "找程序入口", "subagent_type": "explore" }),
                c,
            )
            .await;
        assert!(matches!(out, ToolOutcome::Ok { .. }), "{out:?}");

        let mut nested = Vec::new();
        while let Ok((id, payload)) = rx.try_recv() {
            assert_eq!(id.as_str(), "t1", "进度要挂在发起这次 Task 的 tool_use 上");
            match payload {
                ProgressPayload::Nested { event } => nested.push(*event),
                other => panic!("子 agent 的过程只该以嵌套事件上转，收到了 {other:?}"),
            }
        }
        assert!(
            matches!(nested.first(), Some(AgentEvent::RequestStart { model, .. }) if model == "test-model"),
            "第一条该是带模型名的 RequestStart（界面靠它显示子 agent 用的哪档），实际：{:?}",
            nested.first()
        );
        assert!(
            nested.iter().any(|e| matches!(e, AgentEvent::Message(Message::Assistant { .. }))),
            "子 agent 的助手消息要到卡片上"
        );
        assert!(
            matches!(nested.last(), Some(AgentEvent::Done { reason: TerminalReason::Completed })),
            "最后一条该是 Done，界面靠它收尾，实际：{:?}",
            nested.last()
        );
    }

    /// 权限弹窗由共享的闸直接发到父会话，这里再转就是两份。
    #[test]
    fn 权限事件不上转_其余都转() {
        use riot_protocol::id::RequestId;
        use riot_protocol::permission::DecisionReason;
        assert!(!forwards_to_parent(&AgentEvent::PermissionResolved {
            request_id: RequestId::from_raw("r1"),
            reason: DecisionReason::Timeout,
        }));
        assert!(forwards_to_parent(&AgentEvent::Compacting));
        assert!(forwards_to_parent(&AgentEvent::Progress {
            tool_use_id: ToolUseId::from_raw("u1"),
            payload: ProgressPayload::Status { text: "x".into() },
        }));
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
}
