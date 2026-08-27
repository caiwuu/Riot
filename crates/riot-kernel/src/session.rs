//! 会话：把内核零件组装成能跑的东西。
//!
//! # 职责边界
//!
//! 这里只做**装配与运行**：每轮把工具、权限闸、provider、能力包串成
//! 一个能跑的轮子，并维护会话状态（历史、队列、挂起的询问）。
//! 单一职责的零件在各自的模块里：
//!
//! - [`crate::prompt`] —— 系统提示词与规划模式提醒
//! - [`crate::content`] —— 用户输入 → 消息内容（图片/引用/占位）
//! - [`crate::gate`] —— 权限闸（弹窗、等待、判危竞速）
//! - [`crate::models`] —— 模型端点 → Provider 实例、清单与连通性探测
//!
//! 内核以 `riot-kernel` 二进制独立运行（阶段 B，见 ARCHITECTURE §2.2），
//! 宿主通过 stdio JSON-RPC 驱动；测试与内嵌场景则直接调这里的类型。
//!
//! # 历史从事件流重建
//!
//! `run_agent` 只吐事件，不返回终态。会话历史是把 `AgentEvent::Message`
//! 攒起来得到的。这样宿主和 UI 看到的是同一份东西 —— 如果它们各自维护
//! 一份，两者的分歧只会在几十轮之后以"模型突然失忆"的形式暴露出来。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use riot_core::{AgentDeps, AgentState, run_agent};
use riot_protocol::event::{AgentEvent, StreamDelta};
use riot_protocol::id::{IdGenerator, MessageId, NanoIdGenerator, SessionId};
use riot_protocol::message::{Attachment, Message, MessageMeta, UserContent};
use riot_protocol::permission::{
    PermissionContext, PermissionGate, PermissionMode, PermissionModeState, PermissionRule,
};
use riot_protocol::provider::Provider;
use riot_protocol::tool::{FileStateCache, PromptContext, Tool};
use riot_runtime::{MemoryFileState, SystemFs, SystemProcessRunner};
use riot_tools::registry::Registry;
use riot_tools::scheduler::Scheduler;

use crate::config::Sampling;
use crate::content::{ImageInput, MentionCtx};
use crate::gate::{HostGate, PendingAsks};

/// 等用户回应权限请求的上限区间（秒）。实际值由用户在设置里定，
/// 由 [`crate::config::normalize`] 夹进这个区间。
///
/// `[约束]` 超时按**拒绝**处理，不是允许。用户离开了键盘，而模型想删个
/// 目录 —— 那种时候唯一安全的默认是不做。这条不随可配置化改变：
/// 用户能调的是"等多久",不是"等不到时算同意"。
const ASK_TIMEOUT_RANGE: std::ops::RangeInclusive<u64> = 5..=3600;

/// 单轮最大往返数的合理区间。见 [`crate::config::default_max_turns`]。
/// 至少 1（0 等于什么都做不了），上限 1000（再多多半是跑飞了）。
const MAX_TURNS_RANGE: std::ops::RangeInclusive<u32> = 1..=1000;

/// 用户这一轮发过来的东西。
///
/// 文字和附件打成一包:它们一起来、一起进同一条消息，摊平成两个参数只会让
/// 调用点长到要靠数位置来读（clippy 也会在 8 个参数上叫）。
#[derive(Clone, Default)]
pub struct TurnInput {
    pub text: String,
    /// 附的图。没附就是空的。
    pub images: Vec<ImageInput>,
    /// 用户在输入框里选中的文件引用（界面上的那些块）。
    ///
    /// 和正文里手打的 `@路径` 是两条路、同一个去处：界面选中的不该
    /// 在正文里留下 `@xxx` 字样（那是实现细节漏给用户看），所以它们
    /// 单独走这个字段。两边合并后去重。
    pub refs: Vec<String>,
    /// UserPromptSubmit hook 附加的上下文段落，包成 system-reminder
    /// 跟在这条消息里（模型可见，界面不当用户的话显示）。
    pub extra_context: Vec<String>,
}

/// 会话的事件出口。
///
/// `[约束]` 轮子持有这个中转，**不能**持有某一个具体的 `Channel`。
/// 前端每订阅一次就换一个 channel（切走再切回、开发模式下 StrictMode
/// 的双挂载），而一轮可能横跨好几次订阅。抓着开轮那一刻的 channel 不放
/// 的话，用户切回来之后看到的是一个永远停在原地的界面：轮子还在跑、
/// 事件全发给了一个没人听的旧 channel，连结束都收不到。
///
/// std Mutex 而不是 tokio：锁里只有一次 clone，没有 await。
/// 会话事件的出口抽象。
///
/// 内核通过它把 [`AgentEvent`] 推出去,不关心接收端是宿主的 tauri
/// `Channel`(阶段 A 内嵌)还是 stdout 上的 RPC 通知(阶段 B 拆进程)。
/// `[约束]` 定义在内核侧且**不依赖 tauri** —— 这是内核能脱离宿主编译、
/// 进而作为独立进程运行的前提(见 ARCHITECTURE.md §2.2)。
pub trait EventSink: Send + Sync {
    fn send(&self, event: AgentEvent) -> Result<(), SinkClosed>;
}

#[derive(Clone, Default)]
pub struct SessionSink(Arc<std::sync::Mutex<Option<Arc<dyn EventSink>>>>);

impl SessionSink {
    /// 换上前端最新的那个 channel。
    pub fn attach(&self, ch: Arc<dyn EventSink>) {
        *self.0.lock().expect("事件出口锁不该中毒") = Some(ch);
    }

    /// 发一个事件。`Err` = 这个会话此刻没有出口（前端从没订阅过，
    /// 或者 channel 已经废了）。
    pub fn send(&self, ev: AgentEvent) -> Result<(), SinkClosed> {
        let g = self.0.lock().expect("事件出口锁不该中毒");
        match g.as_ref() {
            Some(ch) => ch.send(ev),
            None => Err(SinkClosed),
        }
    }
}

/// 事件发不出去：界面那头没了。调用方据此中止本轮 —— 没人听的时候
/// 继续跑只是白烧额度。
#[derive(Debug)]
pub struct SinkClosed;

/// 排队中的一条插话。
///
/// 同时留着原始输入和构建好的消息：内核注入用后者（转述等慢活已完成），
/// 用户撤回编辑用前者 —— 从构建好的消息反推原始输入会把图片还原成转述文字。
struct QueuedEntry {
    /// 条目 id，同时也是构建好的消息的 MessageId —— 前端靠它把
    /// "排队面板里的条目"和"注入后回流的消息"对上。
    id: String,
    input: TurnInput,
    msg: Message,
}

/// 给前端排队面板的一条摘要。形状在 protocol(跨进程走 queue.list)。
pub use riot_protocol::QueuedSummary;

/// 撤回一条排队插话时还给前端的原始输入。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedInputOut {
    pub text: String,
    pub images: Vec<ImageInput>,
    pub refs: Vec<String>,
}

/// 跑轮中插话的宿主队列（内核 [`riot_core::state::InputQueue`] 的实现）。
///
/// 消息在**入队时**就构建完毕 —— 图片转述是要调 LLM 的慢活，不能拖到
/// 内核的同步 drain 里做。std Mutex 而不是 tokio：drain 是同步契约，
/// 锁内只有 Vec 操作。
#[derive(Default)]
pub struct HostInputQueue(std::sync::Mutex<Vec<QueuedEntry>>);

impl HostInputQueue {
    fn push(&self, entry: QueuedEntry) {
        self.0.lock().expect("插话队列锁不该中毒").push(entry);
    }

    fn take_all(&self) -> Vec<QueuedEntry> {
        std::mem::take(&mut *self.0.lock().expect("插话队列锁不该中毒"))
    }

    fn snapshot(&self) -> Vec<QueuedSummary> {
        self.0
            .lock()
            .expect("插话队列锁不该中毒")
            .iter()
            .map(|e| QueuedSummary {
                id: e.id.clone(),
                text: e.input.text.clone(),
                images: e.input.images.len(),
                refs: e.input.refs.clone(),
            })
            .collect()
    }

    fn remove(&self, id: &str) -> bool {
        let mut g = self.0.lock().expect("插话队列锁不该中毒");
        let before = g.len();
        g.retain(|e| e.id != id);
        g.len() < before
    }

    fn take(&self, id: &str) -> Option<TurnInput> {
        let mut g = self.0.lock().expect("插话队列锁不该中毒");
        let at = g.iter().position(|e| e.id == id)?;
        Some(g.remove(at).input)
    }
}

impl riot_core::state::InputQueue for HostInputQueue {
    fn drain(&self) -> Vec<Message> {
        self.take_all().into_iter().map(|e| e.msg).collect()
    }
}

/// 本轮要用的宿主能力。
///
/// `[约束]` 每轮现装，不缓存在会话上。用户中途打开搜索或改覆盖地址、给服务方
/// 勾上「支持图片」、换掉视觉兼容模型 —— 下一轮就该生效，而不是要重启。
///
/// 打成一包而不是各自当参数:它们的生命周期和取值时机完全一样，而摊平之后
/// `run_turn` 的参数列表长到要靠数位置来读。
pub struct TurnCapabilities {
    pub web: Arc<dyn riot_protocol::web::WebAccess>,
    pub vision: Arc<dyn riot_protocol::vision::VisionAccess>,
    /// 只读侦察档的便宜模型。None = 没配，子 agent 全跟主模型。
    pub subagent_cheap: Option<crate::subagent::CheapModel>,
    /// Auto 模式的判危分类器。每轮现装 —— 换了便宜档模型下一轮生效。
    pub classifier: Arc<dyn riot_protocol::permission::SafetyClassifier>,
    /// 本轮追加的外部工具（MCP、Skill）。每轮现装，和 web/vision 同一条
    /// 规矩：配置或 SKILL.md 中途改了，下一轮生效。
    pub extra_tools: Vec<Arc<dyn Tool>>,
}

/// 一次压缩的产物。事件由调用方按各自的时机发（见
/// [`Session::compact_history`] 上的约束），所以规模数字要一起带出来。
struct CompactOutcome {
    /// 压缩后的完整历史（一条续接消息）。
    history: Vec<Message>,
    before_tokens: u32,
    after_tokens: u32,
}

/// 一轮的数值上限，每轮从配置现取。
///
/// 打成一包而不是各自当参数，理由和 [`TurnCapabilities`] 一样:取值时机相同，
/// 而且两个字段都是 `u32` —— 摊成位置参数一旦顺序写反，编译器一声不吭，
/// 表现是"超时和轮数对调了"这种极难查的 bug。
#[derive(Clone, Copy)]
pub struct TurnLimits {
    /// 权限弹窗等多久算超时（秒）。见 [`ASK_TIMEOUT_RANGE`]。
    pub ask_timeout_secs: u32,
    /// 单轮最多自主往返多少步。见 [`MAX_TURNS_RANGE`]。
    pub max_turns: u32,
    /// 历史超过这个 token 数就在开工前做 LLM 总结压缩。
    /// 见 [`crate::config::default_compact_threshold_tokens`]。
    pub compact_threshold_tokens: u32,
    /// 命令的 OS 级隔离强度。每轮现取 —— 用户在设置里改完，下一轮生效。
    pub sandbox: crate::config::SandboxMode,
}

/// 会话缓存住的沙箱：激活它的策略 + 激活结果。
///
/// 带着策略一起存，是为了能判断"用户是不是换了档" —— 只按
/// `Option::is_some` 判断的话，用户在设置里从 `WorkspaceWrite` 切到
/// `Off`，这个会话会一直用着上一档的边界，直到重启。
struct CachedSandbox {
    policy: riot_runtime::SandboxPolicy,
    active: Arc<riot_runtime::ActiveSandbox>,
}

/// 本轮工具装配的产物：注册表、prompt 上下文、延迟加载池。
///
/// 三者由 run_inner 的同一段装配逻辑产出、时机相同，捆在一起传 ——
/// 摊平的话 build_scheduler 的参数列表长到要靠数位置来读
/// （clippy 也在第 8 个参数上叫）。
struct ToolAssembly {
    registry: Arc<Registry>,
    prompt_ctx: PromptContext,
    /// 延迟加载池。None = 本轮不启用（候选不足阈值）。
    deferred: Option<Arc<riot_tools::tools::tool_search::DeferredPool>>,
}

/// 一个会话的持久化通道。
///
/// `store` 负责读（水合、索引重建），`log` 负责追加。分开是因为读是一次性的
/// 全量重放，写是贯穿会话生命周期的流 —— 两者的生命周期和并发语义都不同。
pub struct SessionPersist {
    pub store: Arc<riot_store::Transcripts>,
    pub log: riot_store::SessionLog,
}

/// 恢复一个会话时需要的可变设置。
///
/// 宿主从自己的会话索引里读出这些值来构造它 —— 内核不依赖宿主的索引
/// 类型 `crate::persist`(拆进程后那是宿主私有的 UI 元数据,见 ARCHITECTURE.md
/// §2.2 决策:侧边栏/标题/模式等 UI 状态由宿主维护)。
#[derive(Debug, Clone)]
pub struct SessionSettings {
    pub id: String,
    pub sampling: crate::config::Sampling,
    pub mode: riot_protocol::permission::PermissionMode,
    pub python_venv: Option<String>,
    pub system_prompt: Option<String>,
    pub thinking: riot_protocol::ThinkingPolicy,
    pub custom_title: Option<String>,
    pub auto_title: Option<String>,
}

pub struct Session {
    pub id: SessionId,
    pub cwd: std::path::PathBuf,
    /// 这个会话的浏览器。惰性启动 —— 大多数会话不碰它，不该为它们
    /// 付六个进程几百 MB 的常驻代价。见 browser::access。
    ///
    /// 每个会话一个独立 profile:同一个数据目录不能有两个 Chromium 实例，
    /// 共用的话第二个会话一用浏览器就报"不可用"。
    ///
    /// 存具体类型而不是 `dyn BrowserAccess`:面板要用的 screencast 和输入
    /// 转发是**界面**的需求，不该塞进给工具用的那个 trait。两者的读者
    /// 不同，混在一起会让工具层看到一堆它永远不该调的方法。
    /// `None` = 没打包浏览器。
    browser: std::sync::OnceLock<Arc<dyn riot_protocol::browser::BrowserAccess>>,
    history: Mutex<Vec<Message>>,
    /// 当前这一轮的取消令牌。没有正在跑的轮次时是 None。
    running: Mutex<Option<CancellationToken>>,
    /// 本轮的取消是**用户按的停止**，不是关会话/退应用顺手带走的。
    ///
    /// 两者的收场不一样：用户按停止而模型还没开口时，那句话要退回输入框
    /// （见 [`AgentEvent::PromptWithdrawn`]）；关应用则什么都不该动 ——
    /// 用户下次打开必须还看得见自己发过什么。每轮起跑时清零。
    stopped_by_user: AtomicBool,
    /// 跑轮中用户插话的队列。入队与否的判定必须在 `running` 锁下做
    /// （见 [`Self::try_enqueue`]），否则消息会卡在一个没人 drain 的队列里。
    queue: Arc<HostInputQueue>,
    /// 事件出口。前端每次订阅都会换掉里面的 channel，跑着的轮子跟着换。
    sink: SessionSink,
    pending_asks: Arc<PendingAsks>,
    /// 进行中的半截流（见 [`LiveStream`]）。随 session.resume 快照回给界面。
    live_stream: Mutex<LiveStream>,
    /// 会话级采样覆盖。字段为 None 表示继承 provider 的设置。
    /// 模型本身不存这里 —— 每轮由宿主按当前激活配置解析传入，
    /// 用户在对话中途切换模型，下一轮立即生效。
    sampling_override: Mutex<Sampling>,
    /// 会话级 Python 虚拟环境（venv 根目录）。
    ///
    /// 设置后，工具子进程带上 `VIRTUAL_ENV` 且 `<venv>/bin` 排在 PATH
    /// 最前 —— Bash 里的 python / pip 直接落在这个环境里，不需要
    /// `source activate`。下一轮生效。
    python_venv: Mutex<Option<String>>,
    /// 用户为这个会话追加的系统提示词。
    ///
    /// **追加**在内置提示词之后，不替换它 —— 内置提示词里有工作目录和
    /// 安全准则，替换掉的代价是模型连 cwd 都不知道。下一轮生效。
    system_prompt_extra: Mutex<Option<String>>,
    /// 会话级思考策略。默认不干预；下一轮生效。
    thinking_override: Mutex<riot_protocol::ThinkingPolicy>,
    /// 会话内累积的权限规则（用户点了"总是允许"）。
    ///
    /// `Arc` 是刻意的：HostGate 持有同一份，规则在**同一轮内**立即生效。
    /// 拿快照的话，用户点了"总是允许 npm run *"，十秒后模型跑
    /// `npm run build` 还会弹窗 —— 用户会认为按钮坏了。
    rules: Arc<Mutex<Vec<PermissionRule>>>,
    /// 权限模式。`Arc` 的理由和 `rules` 完全一样：批准计划（ExitPlanMode）
    /// 会在**轮次中间**把模式切到执行档，同一轮的下一个工具调用就要按
    /// 新模式判定 —— 快照做不到这一点。
    mode: Arc<Mutex<PermissionMode>>,
    /// 已激活的 OS 沙箱，跨轮复用。见 [`Self::active_sandbox`]。
    sandbox: Mutex<Option<CachedSandbox>>,
    /// 用户手动改过的标题。None 时回退到自动标题。
    custom_title: Mutex<Option<String>>,
    /// 自动标题：第一句用户输入的截断。
    ///
    /// 缓存而不是每次从历史推导 —— 历史是惰性水合的，启动画侧边栏时它还
    /// 没加载，从历史推导的结果是恢复的会话全部显示成"无标题"。
    auto_title: Mutex<Option<String>>,
    /// 持久化通道。None = 不落盘（部分单元测试）。
    persist: Option<SessionPersist>,
    /// 延迟加载工具的"已发现"集合（见 riot-tools 的 tool_search）。
    ///
    /// 会话级而不是轮级：模型这一轮用 ToolSearch 加载过的工具，下一轮
    /// 不该要求它再加载一遍 —— 每轮的池共享这一份集合。
    /// 刻意**不持久化**：重启后模型再多跳一次 ToolSearch 就能找回，
    /// 而持久化它得在 transcript 里扫标记，脆得不值。
    discovered_tools: Arc<std::sync::RwLock<std::collections::HashSet<String>>>,
    /// 历史只从磁盘加载一次。用 OnceCell 而不是 bool：两个入口
    /// （切回会话拉历史、发消息开轮）可能并发首次触发，加载必须恰好一次，
    /// 否则后完成的那次会把先完成的覆盖掉。
    hydrated: tokio::sync::OnceCell<()>,
    file_state: Arc<MemoryFileState>,
    ids: Arc<NanoIdGenerator>,
    /// 此刻是否在压缩。不落盘 —— 和 `running` 一样是活状态，切回会话时
    /// 跟历史一起回给前端，否则三个点还在、"正在压缩上下文"几个字没了。
    compacting: AtomicBool,
    /// 压缩边界之前的消息，只给界面画。模型看的是 `history`（活的那截）。
    ui_archive: Mutex<Vec<Message>>,
    /// 这一轮还没成形的用户消息，只给界面看：不进 `history`、不落盘。
    ///
    /// 用户消息要等主动压缩、图片转述、`@` 展开全跑完才定稿，前两样都是
    /// 模型调用，慢的时候十几秒起步。而 `running` 在这之前就置位了 ——
    /// 这段时间里切走再切回来，前端靠 [`Self::history`] 重建界面，看到的
    /// 是一个转圈的"正在生成"和一片空白，自己刚发出去的话不见了（真实
    /// 反馈）。和 `live_stream` 同一个路子：还没进历史但界面得看得见的
    /// 东西，挂在会话上随快照一起回去。
    pending_user: Mutex<Option<Message>>,
    /// 模型这一侧的终端面板。宿主创建会话后挂上（见 [`Self::attach_terminal`]）。
    ///
    /// 没挂上时是 `NoTerminal` —— 忘了装配的表现是工具明说"用不了"，
    /// 不是悄悄退回那条会把服务杀掉的老路。（"模型起过哪些服务"记在宿主
    /// 的 Terminals 注册表条目上，不在这条代理里 —— docs/ENV_DESIGN.md §6。）
    terminal: std::sync::OnceLock<Arc<dyn riot_protocol::terminal::TerminalAccess>>,
    /// 环境探针（docs/ENV_DESIGN.md）。宿主创建会话后挂上；没挂上就是
    /// "没有感知"，轮次照常跑。
    env: std::sync::OnceLock<Arc<dyn riot_protocol::env::EnvProbe>>,
    /// 上次注入的环境快照渲染文本 —— 差分判定的指纹。None = 还没注入过
    /// （新会话 / 重启水合 / 压缩后），下一轮发全量。
    env_seen: Mutex<Option<String>>,
    /// 上次宣告过的上下文用量档位（0/50/70/85）。只升不降，压缩时归零。
    env_band: Mutex<u32>,
}

/// 标题截断规则：去空白、取前 40 个字符。
///
/// 提出来共享是因为三处要用同一条规则（自动标题、索引重建、历史推导）——
/// 各写一遍的话，重建出来的标题和原来的差一个字符宽度都算 bug。
pub fn title_excerpt(text: &str) -> Option<String> {
    let t = text.trim();
    (!t.is_empty()).then(|| t.chars().take(40).collect())
}

/// 进行中的半截流：正在流式生成、还没落成完整消息的正文和思考。
///
/// 历史只收完整消息（见模块文档），流式增量不进 transcript —— 于是
/// 「切走再切回」的界面从历史里恢复不出正在生成的这一段：思考块的字数
/// 从 0 重数、正文缺头，直到消息完成才自愈。这份缓冲随 session.resume
/// 快照整体带回，界面拿它接着显示。
#[derive(Default)]
pub struct LiveStream {
    pub text: String,
    pub thinking: String,
}

/// 把一条事件折进半截流缓冲。
///
/// 清空点和前端 applyMessage 一致：助手消息完成（整段内容已经在消息里，
/// 缓冲的使命结束）和轮子结束。工具增量刻意不进这里 —— 完整参数在
/// Message 里，卡片摘要缺一小会儿会在消息到达时自愈，为它再攒一份
/// 每个 tool_use_id 的 JSON 不值。
fn fold_live(live: &mut LiveStream, ev: &AgentEvent) {
    match ev {
        AgentEvent::Delta(StreamDelta::Text { text, .. }) => live.text.push_str(text),
        AgentEvent::Delta(StreamDelta::Thinking { text, .. }) => live.thinking.push_str(text),
        AgentEvent::Message(Message::Assistant { .. }) | AgentEvent::Done { .. } => {
            live.text.clear();
            live.thinking.clear();
        }
        _ => {}
    }
}

/// 真正的用户提示：有正文、附图或 `@` 文件。工具结果不算。
fn is_user_prompt(m: &Message) -> bool {
    match m {
        Message::User { content, .. } => content.iter().any(|c| match c {
            UserContent::Text { text } => !text.trim().is_empty(),
            UserContent::Attachment(
                Attachment::Image { .. }
                | Attachment::DescribedImage { .. }
                | Attachment::UserFile { .. },
            ) => true,
            _ => false,
        }),
        _ => false,
    }
}

/// 重新生成的截断点：指定助手消息前面最近一条用户提示的下标。
fn cut_at_user_prompt(history: &[Message], assistant_id: &str) -> Option<usize> {
    let ast = history
        .iter()
        .position(|m| matches!(m, Message::Assistant { .. }) && m.id().as_str() == assistant_id)?;
    history[..ast].iter().rposition(is_user_prompt)
}

impl Session {
    /// 给工具用的浏览器能力。没打包时是 `NoBrowser`，工具会明说用不了。
    fn browser(&self) -> Arc<dyn riot_protocol::browser::BrowserAccess> {
        self.browser
            .get()
            .map(Arc::clone)
            .unwrap_or_else(|| Arc::new(riot_protocol::browser::NoBrowser))
    }

    /// 给面板用的浏览器。`None` = 这个构建没带浏览器。
    pub fn attach_browser(&self, browser: Arc<dyn riot_protocol::browser::BrowserAccess>) {
        let _ = self.browser.set(browser);
    }

    pub fn new(id: SessionId, cwd: std::path::PathBuf, persist: Option<SessionPersist>) -> Self {
        Self {
            id,
            cwd,
            browser: std::sync::OnceLock::new(),
            history: Mutex::new(Vec::new()),
            running: Mutex::new(None),
            stopped_by_user: AtomicBool::new(false),
            queue: Arc::new(HostInputQueue::default()),
            sink: SessionSink::default(),
            pending_asks: Arc::new(PendingAsks::default()),
            live_stream: Mutex::new(LiveStream::default()),
            sampling_override: Mutex::new(Sampling::default()),
            python_venv: Mutex::new(None),
            system_prompt_extra: Mutex::new(None),
            thinking_override: Mutex::new(riot_protocol::ThinkingPolicy::default()),
            rules: Arc::new(Mutex::new(Vec::new())),
            mode: Arc::new(Mutex::new(PermissionMode::Default)),
            sandbox: Mutex::new(None),
            custom_title: Mutex::new(None),
            auto_title: Mutex::new(None),
            persist,
            discovered_tools: Arc::new(std::sync::RwLock::new(std::collections::HashSet::new())),
            hydrated: tokio::sync::OnceCell::new(),
            file_state: MemoryFileState::shared(),
            ids: Arc::new(NanoIdGenerator),
            compacting: AtomicBool::new(false),
            ui_archive: Mutex::new(Vec::new()),
            pending_user: Mutex::new(None),
            terminal: std::sync::OnceLock::new(),
            env: std::sync::OnceLock::new(),
            env_seen: Mutex::new(None),
            env_band: Mutex::new(0),
        }
    }

    /// 从索引恢复一个会话。历史**不在这里加载** —— 惰性水合，见 [`Self::hydrate`]。
    ///
    /// `[约束]` 权限规则（"总是允许"、渗透 scope）刻意**不恢复**，回到空。
    /// 那些授权是对着一个活着的会话给出的；跨越重启把它们静默续上，等于
    /// 用户某天的一次点击变成了永久放行 —— Claude Code 的会话级授权同样
    /// 死于会话结束。
    pub fn restored(
        settings: &SessionSettings,
        cwd: std::path::PathBuf,
        persist: Option<SessionPersist>,
    ) -> Self {
        let id = SessionId::from_raw(settings.id.clone());
        Self {
            id,
            cwd,
            browser: std::sync::OnceLock::new(),
            history: Mutex::new(Vec::new()),
            running: Mutex::new(None),
            stopped_by_user: AtomicBool::new(false),
            queue: Arc::new(HostInputQueue::default()),
            sink: SessionSink::default(),
            pending_asks: Arc::new(PendingAsks::default()),
            live_stream: Mutex::new(LiveStream::default()),
            sampling_override: Mutex::new(settings.sampling),
            python_venv: Mutex::new(settings.python_venv.clone()),
            system_prompt_extra: Mutex::new(settings.system_prompt.clone()),
            thinking_override: Mutex::new(settings.thinking),
            rules: Arc::new(Mutex::new(Vec::new())),
            mode: Arc::new(Mutex::new(settings.mode)),
            sandbox: Mutex::new(None),
            custom_title: Mutex::new(settings.custom_title.clone()),
            auto_title: Mutex::new(settings.auto_title.clone()),
            persist,
            discovered_tools: Arc::new(std::sync::RwLock::new(std::collections::HashSet::new())),
            hydrated: tokio::sync::OnceCell::new(),
            file_state: MemoryFileState::shared(),
            ids: Arc::new(NanoIdGenerator),
            compacting: AtomicBool::new(false),
            ui_archive: Mutex::new(Vec::new()),
            pending_user: Mutex::new(None),
            terminal: std::sync::OnceLock::new(),
            env: std::sync::OnceLock::new(),
            env_seen: Mutex::new(None),
            env_band: Mutex::new(0),
        }
    }

    /// 挂上终端面板。宿主创建/恢复会话之后调一次。
    pub fn attach_terminal(&self, terminal: Arc<dyn riot_protocol::terminal::TerminalAccess>) {
        let _ = self.terminal.set(terminal);
    }

    /// 这一轮装配给工具的终端能力。
    fn terminal(&self) -> Arc<dyn riot_protocol::terminal::TerminalAccess> {
        self.terminal
            .get()
            .map(Arc::clone)
            .unwrap_or_else(|| Arc::new(riot_protocol::terminal::NoTerminal))
    }

    /// 挂上环境探针。宿主创建/恢复会话之后调一次。
    pub fn attach_env(&self, probe: Arc<dyn riot_protocol::env::EnvProbe>) {
        let _ = self.env.set(probe);
    }

    fn env_probe(&self) -> Arc<dyn riot_protocol::env::EnvProbe> {
        self.env
            .get()
            .map(Arc::clone)
            .unwrap_or_else(|| Arc::new(riot_protocol::env::NoEnvProbe))
    }

    /// 确保历史已从磁盘加载。恰好一次；没有持久化通道时是空操作。
    ///
    /// 必须发生在**任何**读或写历史的路径之前：跑轮次、拉历史、判断
    /// 第一句话。漏一处的表现是"切回会话少了前半段"或"模型失忆"。
    async fn hydrate(&self) {
        self.hydrated
            .get_or_init(|| async {
                let Some(p) = &self.persist else { return };
                let parts = p.store.load_parts(&self.id).await;
                self.restore_baselines(&parts.archived, &parts.live);
                *self.ui_archive.lock().await = parts.archived;
                if parts.live.is_empty() {
                    return;
                }
                // 自愈：老索引可能没存自动标题，从水合出来的历史找回。
                {
                    let mut auto = self.auto_title.lock().await;
                    if auto.is_none() {
                        *auto = parts.live.iter().find_map(|m| match m {
                            Message::User { content, .. } => content.iter().find_map(|c| match c {
                                UserContent::Text { text } => title_excerpt(text),
                                _ => None,
                            }),
                            _ => None,
                        });
                    }
                }
                *self.history.lock().await = parts.live;
            })
            .await;
    }

    /// 记下会话的第一句话作为自动标题。返回是否发生了变化（要不要重写索引）。
    pub async fn note_first_prompt(&self, text: &str) -> bool {
        // 先水合：恢复的会话在这里第一次被碰历史。不水合就判断的话，
        // 一个有历史但索引丢了标题的会话会把**新**消息当成第一句。
        self.hydrate().await;
        let mut auto = self.auto_title.lock().await;
        if auto.is_some() {
            return false;
        }
        match title_excerpt(text) {
            Some(t) => {
                *auto = Some(t);
                true
            }
            None => false,
        }
    }

    /// 等待所有已提交的追加落盘。退出钩子用。
    pub async fn flush_log(&self) {
        if let Some(p) = &self.persist {
            p.log.flush().await;
        }
    }

    /// 落盘并关闭 transcript 文件句柄。删除会话前必须调用 ——
    /// Windows 删不掉还开着的文件。
    pub async fn close_log(&self) {
        if let Some(p) = &self.persist {
            p.log.shutdown().await;
        }
    }

    pub fn pending_asks(&self) -> Arc<PendingAsks> {
        Arc::clone(&self.pending_asks)
    }

    /// 进行中的半截流快照：（正文, 思考）。空闲时两段都是空串。
    pub async fn live_stream(&self) -> (String, String) {
        let g = self.live_stream.lock().await;
        (g.text.clone(), g.thinking.clone())
    }

    pub async fn set_mode(&self, m: PermissionMode) {
        *self.mode.lock().await = m;
    }

    pub async fn mode(&self) -> PermissionMode {
        *self.mode.lock().await
    }

    /// 本会话已授权的渗透 scope（host 列表）。
    ///
    /// scope 表达为伪工具 `Pentest` 的 `scope:<host>` allow 规则（见
    /// riot-tools 的 browser scope 判定）。这里把它们从会话规则里挑出来、
    /// 剥掉前缀，给前端展示。
    pub async fn scope_hosts(&self) -> Vec<String> {
        self.rules
            .lock()
            .await
            .iter()
            .filter(|r| r.tool == "Pentest")
            .filter_map(|r| r.pattern.as_deref())
            .filter_map(|p| p.strip_prefix("scope:"))
            .map(ToOwned::to_owned)
            .collect()
    }

    /// 撤销一个渗透 scope 授权。撤销后该目标的侵入性动作会重新要求授权
    /// （`HostGate` 和这里共享同一份规则 Arc，下次判定立刻生效）。
    pub async fn revoke_scope(&self, host: &str) {
        let want = format!("scope:{host}");
        self.rules
            .lock()
            .await
            .retain(|r| !(r.tool == "Pentest" && r.pattern.as_deref() == Some(want.as_str())));
    }

    pub async fn set_sampling(&self, s: Sampling) {
        *self.sampling_override.lock().await = s;
    }

    pub async fn sampling(&self) -> Sampling {
        *self.sampling_override.lock().await
    }

    /// 设置 Python 虚拟环境。None 表示清除，回到宿主默认环境。
    pub async fn set_python_venv(&self, venv: Option<String>) {
        *self.python_venv.lock().await = venv.filter(|v| !v.trim().is_empty());
    }

    pub async fn python_venv(&self) -> Option<String> {
        self.python_venv.lock().await.clone()
    }

    /// 设置会话级追加提示词。None 或空白表示清除，只用内置提示词。
    pub async fn set_system_prompt(&self, prompt: Option<String>) {
        *self.system_prompt_extra.lock().await = prompt.filter(|p| !p.trim().is_empty());
    }

    /// 设置会话级思考策略。下一轮生效。
    pub async fn set_thinking(&self, policy: riot_protocol::ThinkingPolicy) {
        *self.thinking_override.lock().await = policy;
    }

    pub async fn thinking(&self) -> riot_protocol::ThinkingPolicy {
        *self.thinking_override.lock().await
    }

    pub async fn system_prompt_extra(&self) -> Option<String> {
        self.system_prompt_extra.lock().await.clone()
    }

    /// 用户按了停止。返回是否真的有轮子在跑。
    ///
    /// `false` 给前端一个明确信号：宿主已经闲着，该把残留的停止键收掉。
    /// 只记日志的话，界面还转圈，用户连点停止也毫无反应。
    pub async fn interrupt(&self) -> bool {
        self.cancel_turn(true).await
    }

    /// 关会话 / 退应用时取消本轮。
    ///
    /// 和 [`Self::interrupt`] 的唯一差别是**不算用户按停止**：这条路上
    /// 不撤回任何已经发出的消息 —— 用户下次打开必须还看得见自己说过什么。
    pub async fn abort_turn(&self) -> bool {
        self.cancel_turn(false).await
    }

    async fn cancel_turn(&self, by_user: bool) -> bool {
        // 这条日志是"按了停止没反应"唯一能自证的地方：要么没到这里
        //（前端/命令层断了），要么到了但没有正在跑的轮子（界面 busy
        // 是假的），要么取消发出去了而下游没理它。三种病因三种药。
        match self.running.lock().await.as_ref() {
            Some(t) => {
                tracing::info!(session = %self.id.as_str(), by_user, "中断：向本轮发出取消");
                // 置位在 cancel 之前：轮子看到取消之后马上就会读这个标志，
                // 反过来写会让"刚好那一瞬"的停止被当成关应用。
                if by_user {
                    self.stopped_by_user.store(true, Ordering::Relaxed);
                }
                t.cancel();
                true
            }
            None => {
                tracing::warn!(
                    session = %self.id.as_str(),
                    "中断：这个会话没有正在跑的轮次（界面显示的 busy 可能是残留）"
                );
                false
            }
        }
    }

    /// 提交一轮输入：没有轮在跑就把轮子 spawn 到后台并返回 `None`；
    /// 有轮在跑就入队并返回**条目 id** —— 内核在安全点（工具结果全部
    /// 就位后）注入，事件流把它当普通消息推回来，消息的 id 就是这个
    /// 条目 id，前端靠它把排队面板的条目转成对话气泡。
    ///
    /// `[约束]` "排队还是开轮"的判定和 `running` 的置位在**同一次锁**下。
    /// 分开做的话，两次几乎同时的发送会一个开轮、一个报"上一轮还在
    /// 进行中" —— 用户快速连发两条消息就会随机丢一条。
    ///
    /// 竞态窗口：构建消息内容（图片转述是要调 LLM 的慢活，在锁外做）
    /// 期间轮子可能恰好结束。二次检查兜住它：还在跑就入队；不跑了就
    /// 拿原始输入回去抢开轮。入队和 [`Self::run_locked`] 收尾的残留
    /// 清理都以 `running` 锁定序，消息不会滑进一个没人 drain 的队列。
    pub async fn submit(
        self: &Arc<Self>,
        input: TurnInput,
        model: riot_protocol::ModelEndpoint,
        caps: TurnCapabilities,
        sink: SessionSink,
        limits: TurnLimits,
    ) -> Option<String> {
        let cancel = CancellationToken::new();
        loop {
            {
                let mut g = self.running.lock().await;
                if g.is_none() {
                    *g = Some(cancel.clone());
                    break;
                }
            }
            let id = self.ids.next_id("msg");
            let content = crate::content::user_content(
                input.clone(),
                caps.vision.as_ref(),
                self.mention_ctx(),
            )
            .await;
            let msg = Message::User {
                id: MessageId::from_raw(id.clone()),
                content,
                meta: MessageMeta::default(),
            };
            {
                let g = self.running.lock().await;
                if g.is_some() {
                    self.queue.push(QueuedEntry {
                        id: id.clone(),
                        input,
                        msg,
                    });
                    return Some(id);
                }
            }
            // 构建内容期间轮子恰好结束 —— 回去抢开轮。
        }

        // 抢到跑者：轮子丢后台，命令立刻返回 —— 整轮可能要几分钟，
        // 阻塞在这里用户就按不了停止键了。
        let this = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(e) = this
                .run_locked(Some(input), model, caps, sink.clone(), cancel, limits)
                .await
            {
                tracing::error!(error = %e, "本轮失败");
                // 把失败也送到 UI。静默失败的表现是"发了消息没反应"，
                // 那种情况用户只会以为程序坏了。
                let _ = sink.send(AgentEvent::Done {
                    reason: riot_protocol::event::TerminalReason::Error {
                        error: riot_protocol::event::AgentError::Internal { message: e },
                    },
                });
            }
        });
        None
    }

    /// `@` 引用展开的上下文：相对路径以项目根为准，读到的文件登记进
    /// 工作集（模型可以直接改，不用再 Read 一遍）。
    fn mention_ctx(&self) -> MentionCtx<'_> {
        MentionCtx {
            cwd: &self.cwd,
            file_state: Some(self.file_state.as_ref() as &dyn riot_protocol::tool::FileStateCache),
        }
    }

    /// 排队面板要显示的插话清单。
    pub fn queue_snapshot(&self) -> Vec<QueuedSummary> {
        self.queue.snapshot()
    }

    /// 删掉一条排队插话。返回是否真的删到了（条目可能已被注入）。
    pub fn queue_remove(&self, id: &str) -> bool {
        self.queue.remove(id)
    }

    /// 撤回一条排队插话，还给调用方原始输入（放回输入框编辑用）。
    /// `None` = 条目已经不在了（被注入或被删）。
    pub fn queue_take(&self, id: &str) -> Option<TurnInput> {
        self.queue.take(id)
    }

    /// 历史快照。切回一个会话时前端用它重建对话流。
    ///
    /// 末尾可能多出一条还没定稿的用户消息（见 `pending_user`）——
    /// 界面要的是"用户发了什么"，不是"模型收到了什么"。
    pub async fn history(&self) -> Vec<Message> {
        self.hydrate().await;
        let mut out = self.history.lock().await.clone();
        if let Some(m) = self.pending_user.lock().await.clone() {
            out.push(m);
        }
        out
    }

    /// 挂上前端最新的事件出口。跑着的轮子会立刻改用它。
    pub fn attach_sink(&self, ch: Arc<dyn EventSink>) {
        self.sink.attach(ch);
    }

    /// 事件出口的句柄（轮子持有它，不持有具体 channel）。
    pub fn sink(&self) -> SessionSink {
        self.sink.clone()
    }

    /// 此刻有没有轮子在跑。
    ///
    /// 前端切回一个会话时要靠它决定显示停止键还是发送键 —— 界面状态
    /// 随组件卸载丢了，而轮子还在后台跑着。
    pub async fn is_running(&self) -> bool {
        self.running.lock().await.is_some()
    }

    /// 此刻是否在压缩上下文。切回会话时跟 `is_running` 一起回给前端。
    pub fn is_compacting(&self) -> bool {
        self.compacting.load(Ordering::Relaxed)
    }

    /// 压缩边界之前的消息，只给界面画分割线上面的记录。
    pub async fn ui_archive(&self) -> Vec<Message> {
        self.hydrate().await;
        self.ui_archive.lock().await.clone()
    }

    /// 本会话经工具改过的文件，及各自改动前的内容。改动视图用。
    pub async fn changes(&self) -> Vec<crate::changes::FileChange> {
        // 基线在水合时从磁盘（或对话记录）装回来。不先水合的话，
        // 重启后只打开改动面板会看到空的 —— 历史还没加载，基线也没有。
        self.hydrate().await;
        crate::changes::collect(&self.cwd, self.file_state.baselines()).await
    }

    /// 工作区相对所选基线的差异(Git 面板)。跟对话历史无关,
    /// 不用水合 —— 只是以会话的项目目录为根跑 git。
    pub async fn git_changes(&self, base: Option<&str>) -> riot_protocol::GitChanges {
        crate::git_changes::collect(&self.cwd, base).await
    }

    fn baselines_path(&self) -> Option<std::path::PathBuf> {
        self.persist
            .as_ref()
            .map(|p| crate::changes::baselines_path(p.store.dir(), self.id.as_str()))
    }

    /// 重启后把改动基线装回内存。有 sidecar 用 sidecar；老会话没有就
    /// 从对话里的 Read / Write / Edit 推。推出来的当场落盘，下次不用再走。
    fn restore_baselines(&self, archived: &[Message], live: &[Message]) {
        let Some(path) = self.baselines_path() else {
            return;
        };
        let loaded = crate::changes::load_baselines(&path);
        if !loaded.is_empty() {
            for (p, b) in loaded {
                self.file_state.note_baseline(p, b);
            }
            return;
        }
        if archived.is_empty() && live.is_empty() {
            return;
        }
        let mut all = Vec::with_capacity(archived.len() + live.len());
        all.extend_from_slice(archived);
        all.extend_from_slice(live);
        let recovered = crate::changes::reconstruct_baselines(&self.cwd, &all);
        if recovered.is_empty() {
            return;
        }
        for (p, b) in recovered {
            self.file_state.note_baseline(p, b);
        }
        if let Err(e) = crate::changes::save_baselines(&path, &self.file_state.baselines()) {
            tracing::warn!(error = %e, "恢复的基线没写上盘");
        }
    }

    /// 手动设置标题。None 或空串表示清除，回退到自动标题。
    pub async fn set_title(&self, title: Option<String>) {
        *self.custom_title.lock().await = title.filter(|t| !t.trim().is_empty());
    }

    /// 手动标题本身（不合并自动标题）。索引落盘用 —— 索引要分开存两者，
    /// 否则"清除手动名回退到第一条消息"在重启之后就失效了。
    pub async fn custom_title(&self) -> Option<String> {
        self.custom_title.lock().await.clone()
    }

    /// 自动标题本身。索引落盘用。
    pub async fn auto_title(&self) -> Option<String> {
        self.auto_title.lock().await.clone()
    }

    /// 会话标题：手动改过的优先，其次自动标题（第一句用户输入的截断）。
    /// 都没有就是 None（还没说过话）。
    pub async fn title(&self) -> Option<String> {
        if let Some(t) = self.custom_title.lock().await.clone() {
            return Some(t);
        }
        self.auto_title.lock().await.clone()
    }

    /// 跑一轮。事件边产生边推给 `sink`，返回时这一轮已经结束。
    ///
    /// `model` 是宿主对"此刻激活配置"的解析结果（含会话覆盖合并后的
    /// 采样参数）。每轮传入而不是创建时锁死 —— 换模型下一轮就生效。
    pub async fn run_turn(
        &self,
        input: TurnInput,
        model: riot_protocol::ModelEndpoint,
        caps: TurnCapabilities,
        sink: SessionSink,
        limits: TurnLimits,
    ) -> Result<(), String> {
        let cancel = CancellationToken::new();
        {
            let mut g = self.running.lock().await;
            if g.is_some() {
                return Err("上一轮还在进行中".into());
            }
            *g = Some(cancel.clone());
        }
        self.run_locked(Some(input), model, caps, sink, cancel, limits)
            .await
    }

    /// 丢掉指定助手消息及其后的一切，从它前面那条用户提示再跑一轮。
    ///
    /// 不重复插入用户消息：历史已经以那条提示结尾。忙着的时候拒绝 ——
    /// 截断和正在写的 transcript 打架，界面也会同时出现新旧两段。
    pub async fn regenerate(
        self: &Arc<Self>,
        assistant_id: &str,
        model: riot_protocol::ModelEndpoint,
        caps: TurnCapabilities,
        sink: SessionSink,
        limits: TurnLimits,
    ) -> Result<(), String> {
        let cancel = CancellationToken::new();
        {
            let mut g = self.running.lock().await;
            if g.is_some() {
                return Err("正在跑一轮，等它结束再重新生成。".into());
            }
            *g = Some(cancel.clone());
        }
        if let Err(e) = self.rewind_to_prompt(assistant_id).await {
            *self.running.lock().await = None;
            return Err(e);
        }
        let this = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(e) = this
                .run_locked(None, model, caps, sink.clone(), cancel, limits)
                .await
            {
                tracing::error!(error = %e, "重新生成失败");
                let _ = sink.send(AgentEvent::Done {
                    reason: riot_protocol::event::TerminalReason::Error {
                        error: riot_protocol::event::AgentError::Internal { message: e },
                    },
                });
            }
        });
        Ok(())
    }

    /// 截断内存历史（以及 transcript）到指定助手消息前面那条用户提示。
    ///
    /// 归档里的旧回复也能点：截回那条就把压缩后的活历史一并丢掉。
    pub async fn rewind_to_prompt(&self, assistant_id: &str) -> Result<String, String> {
        self.hydrate().await;
        let mut live = self.history.lock().await;
        let mut archived = self.ui_archive.lock().await;

        let mut all = Vec::with_capacity(archived.len() + live.len());
        all.extend_from_slice(&archived);
        all.extend_from_slice(&live);

        let keep = cut_at_user_prompt(&all, assistant_id).ok_or_else(|| {
            if all.iter().any(|m| m.id().as_str() == assistant_id) {
                "找不到这条回复前面的用户消息，没法重新生成。".to_owned()
            } else {
                "这条消息已经不在当前上下文里。".to_owned()
            }
        })?;
        let keep_id = all[keep].id().as_str().to_owned();
        let archive_len = archived.len();
        if keep < archive_len {
            archived.truncate(keep + 1);
            live.clear();
        } else {
            live.truncate(keep - archive_len + 1);
        }
        drop(live);
        drop(archived);

        if let Some(p) = &self.persist {
            p.log.append_rewind(&keep_id);
        }
        *self.live_stream.lock().await = LiveStream::default();
        let _ = self.queue.take_all();
        self.pending_asks.clear().await;
        // 环境指纹归零：截掉的历史可能带走了最近那份快照，指纹还记着
        // "已发过"的话，下一轮差分判定"没变化"，模型对着被截的上下文失明。
        // 多发一份全量最多几十 token，方向和压缩重置一致。
        *self.env_seen.lock().await = None;
        *self.env_band.lock().await = 0;
        Ok(keep_id)
    }

    /// 把半截流里已经吐出来的正文定稿成一条助手消息（历史 + transcript）。
    ///
    /// 用户按停止常常是"够了，别说了"，不是"当你没说过" —— 而取消时
    /// provider 直接结束流，不会再有定稿消息：不接这一手的话，屏幕上
    /// 读了一半的回答会在 `Done` 到达的瞬间整段消失。
    ///
    /// 思考不定稿。它没有签名，回喂给模型是错的（见 INV-9 与降级剥离
    /// 签名那条规矩），而单独留一段没有结论的推理对用户也没有价值。
    ///
    /// 返回 `None` = 没有半截正文可留（模型还没开口，或这一轮正常收尾时
    /// 缓冲已经被 [`fold_live`] 清空了）。
    async fn finalize_partial(&self, model: &str) -> Option<Message> {
        let text = {
            let mut live = self.live_stream.lock().await;
            live.thinking.clear();
            std::mem::take(&mut live.text)
        };
        if text.trim().is_empty() {
            return None;
        }
        let msg = Message::Assistant {
            id: MessageId::from_raw(self.ids.next_id("msg")),
            content: vec![riot_protocol::message::AssistantContent::Text { text }],
            // 用量随被取消的那次请求一起丢了。报 0 好过报一个编出来的数。
            usage: Default::default(),
            meta: MessageMeta {
                interrupted: true,
                model_origin: Some(model.to_owned()),
                ..Default::default()
            },
        };
        // 这条消息不经过事件循环那段 `AgentEvent::Message` 的追加逻辑
        //（它是本地合成的，不来自 stream），所以历史和 transcript 在
        // 这里自己写。
        self.history.lock().await.push(msg.clone());
        if let Some(p) = &self.persist {
            p.log.append(&msg);
        }
        Some(msg)
    }

    /// 撤回本轮那条用户消息：内存历史和 transcript 都不再有它。
    ///
    /// 只在它还是历史末尾时动手。`None` = 没撤（末尾已经不是它了，说明
    /// 这一轮其实产出过东西，撤掉会在上下文里留下一个悬空的回答）。
    /// `Some(true)` = 撤完这个会话一条消息都不剩。
    async fn withdraw_prompt(&self, id: &MessageId) -> Option<bool> {
        let mut live = self.history.lock().await;
        if live.last().map(|m| m.id()) != Some(id) {
            tracing::warn!(session = %self.id.as_str(), "撤回落空：历史末尾已经不是这条提问");
            return None;
        }
        live.pop();
        let empty = live.is_empty() && self.ui_archive.lock().await.is_empty();
        drop(live);

        if let Some(p) = &self.persist {
            p.log.append_withdraw(id.as_str());
        }
        // 环境指纹归零，理由同 rewind_to_prompt：被撤的那条消息可能捎带了
        // 最近一份环境快照，指纹还记着"已发过"的话，下一轮差分判定
        // "没变化"，模型对着一段自己从没见过的上下文失明。
        *self.env_seen.lock().await = None;
        *self.env_band.lock().await = 0;
        Some(empty)
    }

    /// 跑一轮的主体。调用方必须已经把 `running` 置成本轮的令牌 ——
    /// 这里负责跑完、清 `running`、清残留插话。
    ///
    /// `input` 为 `None` 表示重新生成：历史已经以用户提示结尾，不再追加。
    async fn run_locked(
        &self,
        input: Option<TurnInput>,
        model: riot_protocol::ModelEndpoint,
        caps: TurnCapabilities,
        sink: SessionSink,
        cancel: CancellationToken,
        limits: TurnLimits,
    ) -> Result<(), String> {
        // 上一轮的停止不能算到这一轮头上。清在这里而不是置 `running` 那几处：
        // 那是三个入口，漏一个的表现是"上次按过停止，这次没按也把话撤了"。
        self.stopped_by_user.store(false, Ordering::Relaxed);
        // 水合在 running 置位之后：并发的第二轮已经被挡在排队那条路上，
        // 这里的加载不会和另一轮的写历史交错。
        self.hydrate().await;
        let result = self
            .run_inner(input, model, caps, sink.clone(), cancel, limits)
            .await;

        // 残留插话：这一轮被中断/出错，没走到内核的 drain 点。宿主侧
        // 静默清掉 —— 前端的排队面板留着这些条目，由它决定接力重发
        // （中断后自动续，出错后停下等用户）。清空必须和 running 置空在
        // **同一次锁**里：分开的话，一次恰好挤进缝隙的入队会既被这里
        // 清掉、又让前端以为它还排着 —— 消息就真丢了。
        let leftover = {
            let mut g = self.running.lock().await;
            *g = None;
            self.queue.take_all()
        };
        // 占位在正常路径上定稿时就撤了；这里兜住半路失败的那条（比如缺
        // key，消息根本没进历史也没落盘）。留着的话，界面上会挂着一条
        // 永远等不到回复、重启之后又消失的用户消息。
        *self.pending_user.lock().await = None;
        if !leftover.is_empty() {
            tracing::debug!(
                count = leftover.len(),
                "清掉没赶上安全点的插话，前端面板接管"
            );
        }
        result
    }

    /// 会话第一条用户消息前面垫的东西：项目约定 + git 快照。
    ///
    /// 只在第一条注入 —— 它随消息进历史和 transcript，往后每轮自然带着；
    /// 每轮都注的话，同一份内容会在上下文里堆出 N 份。压缩会把这条消息
    /// 吞掉，那时由 [`Self::compact_history`] 重注一份新的。
    ///
    /// 提成函数是为了能测：漏了不会报错，只会让模型少知道一些，而
    /// "它为什么不知道自己在哪个分支"从日志里看不出来。
    async fn first_message_prelude(&self) -> Vec<UserContent> {
        let mut out: Vec<UserContent> = crate::memory::collect(&self.cwd)
            .into_iter()
            .map(|m| {
                UserContent::Attachment(Attachment::Memory {
                    path: m.path,
                    content: m.content,
                })
            })
            .collect();
        if !out.is_empty() {
            tracing::info!(count = out.len(), "注入记忆文件");
        }

        // git 快照放这里而不是 system prompt：分支和工作区脏不脏是会变的，
        // 而 system prompt 是 prompt cache 的前缀 —— 把变化的东西写进去，
        // 每切一次分支就让整个缓存作废。
        if let Some(info) = crate::git::probe(&self.cwd).await {
            tracing::info!(branch = %info.branch, dirty = info.dirty, "注入 git 快照");
            out.push(UserContent::Attachment(Attachment::Environment {
                text: crate::git::describe(&info),
            }));
        }
        out
    }

    /// 环境感知的轮首注入（docs/ENV_DESIGN.md §3）：快照差分 + 告警 + 档位。
    ///
    /// 三种内容各自独立：快照全文只在渲染结果和上次指纹不同时注入（零变化
    /// = 零 token）；档位行只在向上越档时说一次；告警由宿主去重，来了就注。
    /// 采样失败（宿主没装配 / 传输断了）就什么都不注 —— 感知是锦上添花，
    /// 不该挡住轮次。
    async fn env_prelude(&self, history_tokens: u32, compact_threshold: u32) -> Vec<UserContent> {
        let Some(snap) = self.env_probe().sample().await else {
            return Vec::new();
        };
        let mut out = Vec::new();

        let text = crate::env::render(&snap);
        let mut body: Option<String> = {
            let mut seen = self.env_seen.lock().await;
            // 首轮对着空环境不说话：对着空房间描述空房间是噪音。
            // 记下指纹 —— 之后第一个终端出现时，差分自然触发。
            let skip = seen.is_none() && snap.is_quiet();
            let changed = seen.as_deref() != Some(text.as_str());
            *seen = Some(text.clone());
            (changed && !skip).then_some(text)
        };

        // 自我状态档位（P3）。阈值为 0 说明配置坏了，跳过而不是除零。
        if compact_threshold > 0 {
            let pct = ((u64::from(history_tokens) * 100) / u64::from(compact_threshold)) as u32;
            let band = crate::env::usage_band(pct);
            let mut prev = self.env_band.lock().await;
            if band > *prev {
                *prev = band;
                let line = crate::env::band_line(pct);
                body = Some(match body {
                    Some(b) => format!("{b}\n{line}"),
                    None => line,
                });
            }
        }

        if let Some(text) = body {
            out.push(UserContent::Attachment(Attachment::Environment { text }));
        }
        // 条数上限宿主已经守了，这里再夹一次当保险。
        for a in snap.alerts.iter().take(3) {
            out.push(UserContent::Attachment(Attachment::SystemReminder {
                text: crate::env::alert_text(a),
            }));
        }
        if !out.is_empty() {
            tracing::info!(
                parts = out.len(),
                alerts = snap.alerts.len(),
                "注入环境快照"
            );
        }
        out
    }

    /// 这个会话的 OS 沙箱，跨轮复用。`None` = 这台机器上做不到，或者
    /// 用户关了它。
    ///
    /// `[约束]` 不能每轮 activate 一次。Windows 上激活要给可写目录打 Low
    /// 标签，而 `SetNamedSecurityInfoW` 会把可继承 ACE **传播到已有子对象**
    /// —— `~/.cargo` 的 registry 缓存动辄十万个文件，每轮打一次撤一次是
    /// 实打实的卡顿，还在用户文件上反复重写安全描述符。macOS 那侧也有一笔：
    /// 激活时的 profile 冒烟要 spawn 一次进程。
    ///
    /// 策略变了才重新激活（用户在设置里换档）。比较的是 [`SandboxPolicy`]
    /// 本身而不是 [`crate::config::SandboxMode`]：可写集是按 cwd 和"哪些
    /// 缓存目录真实存在"算出来的，装完 rustup 之后同一个 Mode 也该重打标签。
    async fn active_sandbox(
        &self,
        mode: crate::config::SandboxMode,
    ) -> Option<Arc<riot_runtime::ActiveSandbox>> {
        let policy = mode.policy(&self.cwd);
        let mut slot = self.sandbox.lock().await;

        if let Some(cached) = slot.as_ref() {
            if cached.policy == policy {
                return Some(Arc::clone(&cached.active));
            }
            // 策略换了。先把旧的放掉（Drop 会归还标签引用）再激活新的 ——
            // 反过来的话两套标签会同时挂着，中间那段时间的可写面是两者之和。
            *slot = None;
        }

        // Low 标签清单放配置目录，全局一份（标签是全机器状态，孤儿回收
        // 统一）。macOS 忽略这个 setup。now_ms 走真实时钟：它只进清单做
        // 诊断，不参与任何黄金回放。
        let ledger_path = crate::config::sandbox_ledger_path();
        #[allow(clippy::disallowed_methods)]
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let active = Arc::new(
            policy
                .clone()
                .activate(riot_runtime::SandboxSetup {
                    ledger_path,
                    now_ms,
                })?,
        );
        *slot = Some(CachedSandbox {
            policy,
            active: Arc::clone(&active),
        });
        Some(active)
    }

    /// 装配这一轮的工具调度器。
    ///
    /// 单独提出来是为了能被测到。`with_*` 系列每漏一个都是静默降级 ——
    /// 漏 `with_gate` 是所有操作不再询问，漏 `with_web` 是联网工具一律
    /// 报"未配置"。两者都编译得过，都要跑起来才发现。
    fn build_scheduler(
        &self,
        tools: ToolAssembly,
        clock: Arc<dyn riot_protocol::tool::Clock>,
        caps: TurnCapabilities,
        gate: Arc<dyn PermissionGate>,
        python_venv: Option<&str>,
        sandbox: Option<Arc<riot_runtime::ActiveSandbox>>,
    ) -> Scheduler {
        // venv 和能力包都每轮现装（和 caps 一个道理）：用户中途在设置里换
        // 环境、或者刚装完文档能力包，下一轮就生效，不用重启。
        let proc = process_chain(
            Arc::new(SystemProcessRunner::default()),
            sandbox,
            python_venv,
            crate::packs::doc_runtime().as_ref(),
        );
        let file_state: Arc<dyn FileStateCache> = match self.baselines_path() {
            Some(p) => Arc::new(crate::changes::PersistingBaselines::new(
                Arc::clone(&self.file_state),
                p,
            )),
            None => Arc::clone(&self.file_state) as Arc<dyn FileStateCache>,
        };
        let scheduler = Scheduler::new(
            tools.registry,
            tools.prompt_ctx,
            Arc::new(SystemFs::new()),
            proc,
            file_state,
            Arc::clone(&self.ids) as Arc<dyn IdGenerator>,
            clock,
        )
        .with_web(caps.web)
        .with_browser(self.browser())
        .with_terminal(self.terminal())
        .with_vision(caps.vision)
        .with_gate(gate)
        .with_artifacts_dir(self.artifacts_dir());
        match tools.deferred {
            Some(pool) => scheduler.with_deferred(pool),
            None => scheduler,
        }
    }

    /// 把一段历史压成摘要：LLM 总结 + 记忆/工作集重注 + 边界落盘。
    /// 总结失败返回 None（调用方决定要不要声张）。
    ///
    /// `shape` 是本轮主循环请求的形状（system + tools）：轮内的主动压缩传
    /// 得出来 —— 总结请求同形状才能吃前缀缓存；手动 /compact 在空闲时跑、
    /// 没有轮次装配，传 None 走瘦身路径（慢一点、贵一点，但不常用）。
    ///
    /// `[约束]` `Compacted` 事件由**调用方**发，这里只发 `Compacting`。
    /// 手动 `/compact` 必须先把 `running` 放掉再宣布完成 —— 顺序反了的话，
    /// 前端收到事件去拉快照，看到的还是 busy=true（`running` 被压缩占着，
    /// 见 [`Self::compact_now`]），状态行从"正在压缩上下文"变成一个假的
    /// "正在生成"，挂到下一个看门狗周期才消失（真实发生过，12 秒）。
    /// 轮内压缩则相反，要求原地立发。两种时机只有调用方分得清。
    async fn compact_history(
        &self,
        provider: &Arc<dyn Provider>,
        model: &str,
        history: &[Message],
        shape: Option<&riot_core::summarize::RequestShape>,
        sink: &SessionSink,
        cancel: CancellationToken,
    ) -> Option<CompactOutcome> {
        let before = provider.count_tokens(history);
        // 先说一声再动手。下面那次总结是一个真实的模型调用，几十秒 ——
        // 期间界面上只有那三个点在动，和"模型正在回答"分不出来。
        self.compacting.store(true, Ordering::Relaxed);
        let _ = sink.send(AgentEvent::Compacting);
        let summary = match riot_core::summarize::summarize_history(
            provider, model, history, shape, cancel,
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "历史总结失败");
                self.compacting.store(false, Ordering::Relaxed);
                return None;
            }
        };
        // 记忆重注：压缩把带着 AGENTS.md 的首条消息吞了，
        // 不重注的话项目约定从此消失（CC 的 postCompactCleanup 同款）。
        let mut memory: Vec<Attachment> = crate::memory::collect(&self.cwd)
            .into_iter()
            .map(|m| Attachment::Memory {
                path: m.path,
                content: m.content,
            })
            .collect();
        // git 快照同理，而且重注的这份是**新的** —— 压缩前的那几十轮里
        // 分支和工作区多半已经变了，照抄旧快照比不给还糟。
        if let Some(info) = crate::git::probe(&self.cwd).await {
            memory.push(Attachment::Environment {
                text: crate::git::describe(&info),
            });
        }
        // 环境指纹与档位归零：压缩吞掉了带着旧快照的消息，不归零的话
        // 下一轮差分判定"没变化"，模型从此失明（docs/ENV_DESIGN.md §3.2）。
        // 和记忆/git 重注放同一个函数里 —— 漏一起漏，测试一起钉。
        *self.env_seen.lock().await = None;
        *self.env_band.lock().await = 0;
        // 工作集重注：纯总结不够 —— 压缩后模型立即失去对文件
        // 内容的记忆，下一步就是把刚读过的文件再读一遍。
        let restored = restored_files(self.file_state.as_ref());
        let msg = riot_core::summarize::continuation_message(
            &summary,
            memory,
            restored,
            MessageId::from_raw(self.ids.next_id("msg")),
        );
        let after = provider.count_tokens(std::slice::from_ref(&msg));
        // 边界必须先于续接消息落盘 —— 顺序反了，重启加载会把
        // 续接消息一起丢掉（见 SessionLog::append_boundary）。
        if let Some(p) = &self.persist {
            p.log.append_boundary(before, after);
            p.log.append(&msg);
        }
        tracing::info!(before, after, "历史压缩完成");
        self.ui_archive.lock().await.extend(history.iter().cloned());
        self.compacting.store(false, Ordering::Relaxed);
        Some(CompactOutcome {
            history: vec![msg],
            before_tokens: before,
            after_tokens: after,
        })
    }

    /// 手动压缩（`/compact`）。空闲时才能做 —— 压缩改写历史，
    /// 不能和跑动中的轮子并发。
    pub async fn compact_now(
        &self,
        model: riot_protocol::ModelEndpoint,
        sink: SessionSink,
    ) -> Result<(), String> {
        let provider = crate::models::provider_from_endpoint(&model)?;
        let cancel = CancellationToken::new();
        // 占住 running：期间的插话照常排队，下一轮的收尾 drain 会捞到。
        {
            let mut g = self.running.lock().await;
            if g.is_some() {
                return Err("正在跑一轮，等它结束再压缩。".into());
            }
            *g = Some(cancel.clone());
        }
        self.hydrate().await;
        let mut history = self.history.lock().await.clone();
        // 和 run() 同一道自愈：历史里若有悬空 tool_use（上次中断/崩溃留下），
        // 总结请求发给严格校验的服务端同样 400，用户看到的只是一句
        // 莫名其妙的"压缩失败"。
        riot_core::repair::repair_tool_pairing(&mut history);
        let result = if history.is_empty() {
            Err("还没有对话内容，没什么可压缩的。".to_owned())
        } else {
            match self
                .compact_history(&provider, &model.model, &history, None, &sink, cancel)
                .await
            {
                Some(o) => {
                    *self.history.lock().await = o.history;
                    Ok((o.before_tokens, o.after_tokens))
                }
                None => Err("压缩失败，历史保持原样。稍后再试。".to_owned()),
            }
        };
        *self.running.lock().await = None;
        // `[约束]` 宣布完成必须在 running 释放**之后**。前端收到 Compacted
        // 会去拉快照对齐状态 —— 此刻快照必须已经是空闲，否则它把 busy=true
        // 吸进去，没有下一个事件会来清（手动压缩不是轮次，没有 Done），
        // 只能等 12 秒的看门狗兜底。
        result.map(|(before_tokens, after_tokens)| {
            let _ = sink.send(AgentEvent::Compacted {
                before_tokens,
                after_tokens,
                strategy: riot_protocol::event::CompactStrategy::FullSummary,
            });
        })
    }

    /// 工具产物（截图原图等）的落盘目录，会话专属。
    ///
    /// 放配置目录下而不是工作区:截图不是项目文件，出现在用户的 git
    /// status 里就是垃圾。目录建不出来也照常返回路径 —— 工具写不进时
    /// 自行降级（消息里不带路径），链路不断。
    fn artifacts_dir(&self) -> std::path::PathBuf {
        let dir = crate::config::config_path()
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("artifacts")
            .join(self.id.as_str());
        #[allow(clippy::disallowed_methods)]
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(error = %e, dir = %dir.display(), "工件目录建不出来，截图将不落盘");
        }
        dir
    }

    async fn run_inner(
        &self,
        input: Option<TurnInput>,
        model: riot_protocol::ModelEndpoint,
        mut caps: TurnCapabilities,
        sink: SessionSink,
        cancel: CancellationToken,
        limits: TurnLimits,
    ) -> Result<(), String> {
        let provider = crate::models::provider_from_endpoint(&model)?;
        let clock: Arc<dyn riot_protocol::tool::Clock> =
            Arc::new(riot_providers::watchdog::TokioClock);

        // Hooks 引擎每轮现装（和 caps 一条规矩：hooks.json 中途改了，
        // 下一轮生效）。没配置时各检查点都是零开销的空实现。
        let hook_engine = Arc::new(crate::hooks::HookEngine::load(&self.cwd, self.id.as_str()));

        // 沙箱先算出来：它同时决定"命令怎么跑"（下面的 build_scheduler）
        // 和"策略层敢不敢放松"（紧接着的 PermissionContext）。
        //
        // `[约束]` 两处必须来自**同一次** activate。分别判断的话，一边以为
        // 有边界、另一边其实没套上 —— 那正好是最坏的组合：决策链按"OS 挡着"
        // 放行了命令，而实际上什么都没挡。
        let sandbox = self.active_sandbox(limits.sandbox).await;
        if sandbox.is_none() && limits.sandbox != crate::config::SandboxMode::Off {
            // 说一声。静默降级的话，用户以为自己开着沙箱。
            //
            // 不写死原因：降级的现场按平台各不相同（macOS 缺
            // `sandbox-exec` 或 profile 没通过；Windows 是打标签失败、
            // 建令牌失败，或 NoNet 档的诚实降级）。真正的原因由
            // `riot_runtime` 那侧带上下文打日志，这里只负责让用户知道
            // "你以为开着的那层现在没开"。
            tracing::warn!(
                session = %self.id.as_str(),
                mode = ?limits.sandbox,
                "这台机器上沙箱没能激活，本轮不隔离（决策链回到逐条询问）"
            );
        }

        // 权限闸先装：工具装配就要用它（Task 子 agent 与父共用同一个闸）。
        let mode = *self.mode.lock().await;
        let gate = Arc::new(HostGate {
            sink: sink.clone(),
            pending: Arc::clone(&self.pending_asks),
            ids: Arc::clone(&self.ids) as Arc<dyn IdGenerator>,
            ctx: PermissionContext {
                mode: PermissionModeState(Some(mode)),
                rules: self.rules.lock().await.clone(),
                sandboxed: sandbox.is_some(),
                can_prompt_user: true,
            },
            rules_live: Arc::clone(&self.rules),
            mode_live: Arc::clone(&self.mode),
            cwd: self.cwd.clone(),
            // 再夹一次。配置加载时已经夹过，但这里是唯一真正用到它的
            // 地方 —— 上游多一条没走 normalize 的路，这里就是最后一道。
            ask_timeout: Duration::from_secs(
                u64::from(limits.ask_timeout_secs)
                    .clamp(*ASK_TIMEOUT_RANGE.start(), *ASK_TIMEOUT_RANGE.end()),
            ),
            hooks: Arc::clone(&hook_engine),
            classifier: Arc::clone(&caps.classifier),
        });

        // 本轮的工具 = 内置 + 子 agent + 外部（MCP、Skill）。
        //
        // `[约束]` 撞名的外部工具**跳过并告警**，不能 panic —— 内置工具重名
        // 是代码错误（下面的 expect 管它），外部工具重名是**用户配置**引起
        // 的（两个 MCP 服务器的 id 消毒后相同），配置错误不能把应用带崩。
        let mut tools = riot_tools::tools::builtin();
        tools.push(Arc::new(crate::subagent::TaskTool::new(
            crate::subagent::SubagentDeps {
                provider: Arc::clone(&provider),
                model: model.model.clone(),
                cheap: caps.subagent_cheap.take(),
                gate: Arc::clone(&gate) as Arc<dyn PermissionGate>,
                // 和上面那个 gate 来自同一次 activate。缺了它，子 agent 的
                // 命令在宿主上裸跑而闸里写着 sandboxed: true。
                sandbox: sandbox.clone(),
                web: Arc::clone(&caps.web),
                vision: Arc::clone(&caps.vision),
                clock: Arc::clone(&clock),
                ids: Arc::clone(&self.ids) as Arc<dyn IdGenerator>,
                cwd: self.cwd.clone(),
                artifacts_dir: self.artifacts_dir(),
                max_turns: limits.max_turns,
                // 子 agent transcript 放 subagents/ 子目录 —— 混进主目录
                // 会被索引重建当成会话捞回来。
                transcripts: self.persist.as_ref().map(|p| {
                    Arc::new(riot_store::Transcripts::new(
                        p.store.dir().join("subagents").join(self.id.as_str()),
                    ))
                }),
            },
        )));
        let mut names: std::collections::HashSet<String> = tools
            .iter()
            .flat_map(|t| {
                std::iter::once(t.name().to_owned())
                    .chain(t.aliases().iter().map(|a| (*a).to_owned()))
            })
            .collect();
        for t in std::mem::take(&mut caps.extra_tools) {
            if names.insert(t.name().to_owned()) {
                tools.push(t);
            } else {
                tracing::warn!(tool = %t.name(), "外部工具与已有工具重名，本轮跳过它");
            }
        }

        // 模型对"今天"没有概念，它的年份停在训练截止那天。不注入的话它
        // 搜"最新版本"会带上一个两年前的年份，然后拿着过期结果言之凿凿；
        // 聊天里问「最近」「今年」也一样。系统提示词和工具 prompt 共用
        // 这一份，粒度（只到月，为缓存）见 tools::web::date。
        let today = riot_tools::tools::web::date::year_month(clock.now_ms());
        let sandboxed = sandbox.is_some();
        let make_ctx = |sibling_tools: Vec<String>| PromptContext {
            cwd: self.cwd.clone(),
            platform: std::env::consts::OS.to_owned(),
            // 和上面 PermissionContext 里那个是同一次 activate 的结果。
            // 工具描述照它变（Bash 会讲清边界和出沙箱的办法）。
            sandboxed,
            // 全部正式名。工具的 prompt 靠它写清分工（"有 X 就别用我"）。
            sibling_tools,
            today: today.clone(),
        };

        // 规划模式的出口工具只在规划模式注册：其它模式下它没有意义，
        // 挂在清单里只会引诱模型误调。本轮批准后模式虽已切换，工具要到
        // 下一轮才消失 —— 再调一次也只是无害地重复"已批准"。
        if mode == PermissionMode::Plan && names.insert("ExitPlanMode".into()) {
            tools.push(Arc::new(riot_tools::tools::plan::ExitPlanMode));
        }

        // 工具目录瘦身：延迟候选（MCP 工具）的定义总量超过阈值才启用 ——
        // 只有几个工具时，省下的上下文抵不过多一跳 ToolSearch 的往返。
        // 已发现集合是会话级的，这一轮加载过的下一轮不用再加载。
        let render_ctx = make_ctx(tools.iter().map(|t| t.name().to_owned()).collect());
        let pool = riot_tools::tools::tool_search::DeferredPool::new(
            &tools,
            &render_ctx,
            Arc::clone(&self.discovered_tools),
        );
        let deferred = if !pool.is_empty()
            && pool.total_chars() >= riot_tools::tools::tool_search::DEFER_THRESHOLD_CHARS
        {
            let pool = Arc::new(pool);
            tools.push(Arc::new(riot_tools::tools::tool_search::ToolSearch::new(
                Arc::clone(&pool),
            )));
            tracing::info!(
                chars = pool.total_chars(),
                "延迟工具定义超过阈值，启用 ToolSearch"
            );
            Some(pool)
        } else {
            None
        };

        let prompt_ctx = make_ctx(tools.iter().map(|t| t.name().to_owned()).collect());

        // 注册失败说明内置工具有重名或别名冲突 —— 那是代码错误，不是
        // 运行时状况（外部工具的撞名已经在上面被摘掉了）。
        let registry = Arc::new(Registry::new(tools).expect("内置工具注册表有冲突"));

        // Stop hooks：没配置时给 NoStopGate —— 收尾零开销。
        let stop_gate: Arc<dyn riot_core::state::StopGate> = if hook_engine.has_stop() {
            Arc::new(crate::hooks::HookStopGate(Arc::clone(&hook_engine)))
        } else {
            Arc::new(riot_core::state::NoStopGate)
        };

        // 附件要用同一份图片能力（模型收不了图时靠它转成文字），而 caps
        // 马上要被 scheduler 拿走 —— 先留一份。
        let vision = Arc::clone(&caps.vision);
        let python_venv = self.python_venv().await;
        let scheduler = self.build_scheduler(
            ToolAssembly {
                registry,
                prompt_ctx,
                deferred,
            },
            clock.clone(),
            caps,
            gate,
            python_venv.as_deref(),
            sandbox,
        );
        // PostToolUse hooks：只在真配了的时候装 —— enabled() 为 false 时
        // 调度器连 hook 参数（input 克隆）都不准备。
        let scheduler = if hook_engine.has_post_tool_use() {
            scheduler.with_hooks(Arc::new(crate::hooks::HookToolHooks(Arc::clone(
                &hook_engine,
            ))))
        } else {
            scheduler
        };

        let tools_runner: Arc<dyn riot_core::state::ToolRunner> = Arc::new(scheduler);

        // system prompt 在这里就定下来，而不是 run_agent 前夕 —— 主动/反应式
        // 压缩的总结请求要用**同一份**：同形状（system + tools 逐字节一致）
        // 的总结请求才能吃到 provider 的前缀缓存，~100k 的输入走 cache_read。
        let system = crate::prompt::system_prompt(
            &self.cwd,
            &today,
            python_venv.as_deref(),
            self.system_prompt_extra().await.as_deref(),
            hook_engine.has_pre_tool_use()
                || hook_engine.has_post_tool_use()
                || hook_engine.has_stop(),
        );
        // specs 取轮首快照。轮中 ToolSearch 发现新工具时主循环的 tools 会变
        //（那本来就会断缓存），总结形状不跟 —— 只影响命中率，不影响正确性。
        let summary_shape = riot_core::summarize::RequestShape {
            system: system.clone(),
            tools: tools_runner.specs(),
        };

        let deps = AgentDeps {
            provider: Arc::clone(&provider),
            // 反应式（413）路径的完整阶梯：清旧工具结果 → LLM 总结。
            // 只挂 ClearOldResults 的话，"对话本身超长"的会话一溢出就死。
            compactor: Arc::new(riot_core::Layered::new(
                Arc::clone(&provider),
                model.model.clone(),
                summary_shape.clone(),
                Arc::clone(&self.ids) as Arc<dyn IdGenerator>,
                cancel.child_token(),
            )),
            clock: Arc::clone(&clock),
            ids: Arc::clone(&self.ids) as Arc<dyn IdGenerator>,
            tools: Arc::clone(&tools_runner),
            queue: Arc::clone(&self.queue) as Arc<dyn riot_core::state::InputQueue>,
            // Stop hooks 接在这里（每轮现装，配置中途改了下一轮生效）。
            // 没配置任何 Stop hook 时给 NoStopGate —— 收尾零开销。
            stop_gate: stop_gate.clone(),
        };

        let mut history = self.history.lock().await.clone();

        // 自愈：历史里可能残留悬空的 tool_use —— 流中途断开时被持久化的
        // 半截 assistant 消息、工具执行途中崩溃/强杀、或旧版本留下的脏
        // transcript。带着孤儿 tool_calls 组请求，严格校验的服务端
        //（DeepSeek 等）会对**每一次**请求 400，会话永久废掉；宽松的
        // 服务端（智谱等）能跑，于是表现成"换个模型就好了"。就地补上
        // 错误结果。transcript 不回写：修复是幂等且确定的，每次组请求
        // 前重修一遍，重启后加载的脏历史同样在这里被治好。
        let repaired = riot_core::repair::repair_tool_pairing(&mut history);
        if repaired > 0 {
            tracing::warn!(
                session = %self.id.as_str(),
                repaired,
                "历史里有 {repaired} 个悬空的 tool_use（上次中断/崩溃留下），已就地补上错误结果"
            );
        }

        // 本轮追加的那条用户消息。模型一个字都没给出就被停止时，它要被
        // 撤回（见 AgentEvent::PromptWithdrawn）。重新生成这一路没有它。
        let mut submitted: Option<MessageId> = None;

        if let Some(input) = input {
            // 这条消息的 id 先定下来：占位版和定稿版用同一个，前端认 id。
            let user_id = MessageId::from_raw(self.ids.next_id("msg"));
            // 占位先立起来 —— 底下压缩和转述都是模型调用，这段时间里切走
            // 再切回来必须还看得见自己刚发的话（见 `pending_user`）。
            *self.pending_user.lock().await = Some(Message::User {
                id: user_id.clone(),
                content: crate::content::pending_user_content(&input),
                meta: MessageMeta::default(),
            });

            // ── 主动压缩：历史超阈值就先总结再开工 ────────────────────
            // 反应式（413 重试）是保命；这条是"到线就处理"—— 不主动的话，
            // 会话会一直顶着窗口上限跑，每轮都在 413 的边缘反复横跳。
            // 放在追加本轮用户消息**之前**：压的是旧账，新话骑在压缩后的历史上。
            let history_tokens = provider.count_tokens(&history);
            if !history.is_empty() && history_tokens >= limits.compact_threshold_tokens {
                match self
                    .compact_history(
                        &provider,
                        &model.model,
                        &history,
                        Some(&summary_shape),
                        &sink,
                        cancel.child_token(),
                    )
                    .await
                {
                    Some(o) => {
                        history = o.history;
                        *self.history.lock().await = history.clone();
                        // 轮内原地宣布：轮子接着跑，busy 本来就该保持，
                        // 而且 Compacted 后紧跟 RequestStart 的顺序有回放钉着。
                        let _ = sink.send(AgentEvent::Compacted {
                            before_tokens: o.before_tokens,
                            after_tokens: o.after_tokens,
                            strategy: riot_protocol::event::CompactStrategy::FullSummary,
                        });
                    }
                    // 失败不拦路：继续用完整历史，真溢出时反应式路径兜底。
                    None => tracing::warn!("主动压缩失败，本轮用完整历史"),
                }
            }
            let mut content =
                crate::content::user_content(input, vision.as_ref(), self.mention_ctx()).await;
            // 记忆注入：会话的**第一条**用户消息前置 AGENTS.md（全局 + 项目）。
            // 只注入一次 —— 它随消息进历史和 transcript，往后每轮自然带着；
            // 每轮都注的话，同一份内容会在上下文里堆出 N 份。
            let mut prelude = if history.is_empty() {
                self.first_message_prelude().await
            } else {
                Vec::new()
            };
            // 环境感知：轮首采样、差分注入（docs/ENV_DESIGN.md）。顺序放在
            // 记忆之后、用户正文之前 —— 身份和约定先于状态，状态先于问题。
            // token 数取压缩之后的历史：档位说的是本轮真实的余量。
            prelude.extend(
                self.env_prelude(
                    provider.count_tokens(&history),
                    limits.compact_threshold_tokens,
                )
                .await,
            );
            if !prelude.is_empty() {
                prelude.append(&mut content);
                content = prelude;
            }
            // 规划模式的约束跟在消息**末尾**（用户正文之后）：它是对本轮
            // 状态的注解，不是消息本身，和 extra_context 同一个位置逻辑。
            // 为什么不进 system prompt，见 plan_mode_reminder 的取舍注释。
            content.extend(crate::prompt::plan_mode_reminder(mode));
            let user_msg = Message::User {
                id: user_id.clone(),
                content,
                meta: MessageMeta::default(),
            };
            // 边产生边追加（两家共识）：轮次结束才写盘的话，中途崩溃丢的是
            // 整轮对话；这里丢的最多是后台通道里还没落盘的几条。
            if let Some(p) = &self.persist {
                p.log.append(&user_msg);
            }
            history.push(user_msg);
            submitted = Some(user_id);
        } else if history.is_empty() {
            return Err("没有可重新生成的用户消息".into());
        }

        let state = AgentState::new(self.id.clone(), model.model.clone())
            .with_messages(history)
            // 再夹一次。配置加载时已经夹过（config::normalize），但这里是唯一
            // 真正用到它的地方，最后一道防线 —— 和 ask_timeout 同样的处理。
            .with_max_turns(
                limits
                    .max_turns
                    .clamp(*MAX_TURNS_RANGE.start(), *MAX_TURNS_RANGE.end()),
            );

        let state = AgentState {
            // 构建提前到了工具装配之后（见 summary_shape），这里只是接住。
            system,
            max_output_tokens_override: model.sampling.max_output_tokens,
            thinking: self.thinking().await,
            ..state
        };

        // `[约束]` 内存历史必须**边跑边更新**，不能等整轮结束再覆盖。
        //
        // 前端切走一个会话时会丢掉它的界面状态，切回来靠 history() 重建。
        // 而这一轮的消息在轮子结束前只存在于事件流里 —— 攒到最后才写的
        // 话，跑到一半切走再切回来，看到的是这一轮开始前的样子（新会话
        // 就是一片空白，用户以为聊天记录没了）。落盘那边本来就是逐条追加
        // 的，内存这边没有理由不一致。
        *self.history.lock().await = state.messages.clone();
        // 定稿版已经进历史，占位撤掉 —— 留着的话历史末尾会多出一条重复的
        // 用户消息。撤在写历史**之后**：反过来的话中间那一瞬两边都没有。
        *self.pending_user.lock().await = None;
        // 半截流从零开始。正常情况下上一轮的 Done 已经清过，这里兜住
        // 通道断开提前 break 的那条路 —— 不清的话残留会拼进这一轮。
        *self.live_stream.lock().await = LiveStream::default();

        let stream = run_agent(state, deps, cancel.clone());
        futures::pin_mut!(stream);

        // 这一轮有没有留下东西。决定被停止时那句提问是撤回还是留下。
        let mut produced = false;

        use futures::StreamExt;
        while let Some(ev) = stream.next().await {
            if leaves_a_trace(&ev) {
                produced = true;
            }
            // 每轮怎么收场都记一笔。"按了停止没反应"的排查里，这条能
            // 区分"内核没收到取消"和"收到了但界面没更新"。
            if let AgentEvent::Done { reason } = &ev {
                tracing::info!(session = %self.id.as_str(), ?reason, "本轮结束");
                self.compacting.store(false, Ordering::Relaxed);
                // 已经说出口的半截话先定稿。排在撤回判定**之前**：它一旦
                // 落地，这一轮就算有产出，提问不能再撤（撤了那半截回答
                // 就悬空了）。
                if let Some(m) = self.finalize_partial(&model.model).await {
                    produced = true;
                    let _ = sink.send(AgentEvent::Message(m));
                }
                // 用户按了停止，而这一轮什么都没留下 —— 那句提问从没被
                // 回答过，留在历史里只会在下一轮原样再发一次。撤回它，
                // 界面把原文放回输入框。
                //
                // 排在 Done **之前**发：Done 是流的最后一个事件（INV-4），
                // 而且前端收到 Done 就会去接力排队的插话。
                if !produced
                    && self.stopped_by_user.load(Ordering::Relaxed)
                    && let Some(id) = submitted.take()
                    && let Some(empty) = self.withdraw_prompt(&id).await
                {
                    let _ = sink.send(AgentEvent::PromptWithdrawn {
                        message_id: id,
                        session_empty: empty,
                    });
                }
            }
            if let AgentEvent::Compacting = &ev {
                self.compacting.store(true, Ordering::Relaxed);
            }
            if let AgentEvent::Compacted { .. } = &ev {
                self.compacting.store(false, Ordering::Relaxed);
            }
            if let AgentEvent::Message(m) = &ev {
                // 磁盘和内存在同一处追加 —— 两边各攒各的迟早分叉
                //（"重启后少半段"）。
                self.history.lock().await.push(m.clone());
                if let Some(p) = &self.persist {
                    p.log.append(m);
                }
            }
            // 半截流缓冲：切回会话的快照要能带上正在生成的正文/思考，
            // 否则界面只能从 0 重新攒（历史只收完整消息）。
            fold_live(&mut *self.live_stream.lock().await, &ev);
            // 发送失败说明前端窗口没了。继续跑完只会白烧 API 额度。
            if sink.send(ev).is_err() {
                tracing::warn!("事件通道已断开，中止本轮");
                cancel.cancel();
                break;
            }
        }

        Ok(())
    }
}

/// 压缩后重注入的工作集：最近读过的文件（预算对齐 Claude Code：
/// 最多 5 个、单文件 ~5k token、总量 ~25k token）。
///
/// 预算统一按**字节**算 —— 和 `estimate_tokens` 的 4 字节/token 同一口径。
/// 以前单文件按字符数、总量按字节数：中文内容下 20k 字符 ≈ 60k 字节，
/// 一个文件就吃掉大半总预算，"最多 5 个"实际只进得去一两个。
fn restored_files(file_state: &dyn riot_protocol::tool::FileStateCache) -> Vec<Attachment> {
    const MAX_FILES: usize = 5;
    const MAX_BYTES_PER_FILE: usize = 20_000;
    const MAX_TOTAL_BYTES: usize = 100_000;

    let mut total = 0usize;
    let mut out = Vec::new();
    for (path, st) in file_state.recent(MAX_FILES) {
        let mut content = st.content;
        if content.len() > MAX_BYTES_PER_FILE {
            // 截在字符边界上：中间劈开一个多字节字符，truncate 直接 panic。
            let mut cut = MAX_BYTES_PER_FILE;
            while !content.is_char_boundary(cut) {
                cut -= 1;
            }
            content.truncate(cut);
            content.push_str("\n\n[文件超长已截断，需要完整内容用 Read 重读]");
        }
        if total + content.len() > MAX_TOTAL_BYTES {
            break;
        }
        total += content.len();
        out.push(Attachment::RestoredFile { path, content });
    }
    out
}

/// 往 `spec` 的 PATH 前面接上几个目录。
///
/// `ProcessSpec.env` 的语义是"覆盖这几个、其余继承"（见
/// `SystemProcessRunner`），所以 PATH 必须拼完整：已经有人设过就在那份前面
/// 接，没有就从宿主当前的 PATH 接。
///
/// `[约束]` 必须是"接"而不是"没有才设"。venv 和能力包两层装饰器都要改
/// PATH，谁先跑取决于装配顺序 —— 用"没有才设"的话，先跑的那层会把后跑的
/// 那层整个吞掉，表现是设了 venv 就找不到 soffice（或者反过来）。
/// 把 `base`（真正起进程的那个）一层层包成本轮要用的执行器。
///
/// 单独提出来是为了**顺序能被测到**。这条链上每一层的位置都有过一次
/// 真实的 bug，而三种错法全部编译得过、也全部静默：
///
/// - 沙箱装在最外层 → Windows 上 `WinSandbox::run` 用受限令牌自己调
///   `CreateProcessAsUserW`，**根本不会调 inner**，于是下面两层改环境
///   变量的装饰器一个都跑不到。表现是"一开沙箱（默认开），会话设的 venv
///   和能力包静默失效，python 拿到的是系统那个"。所以沙箱贴最里层 ——
///   它替换的是"最终由谁起进程"，不是"跑什么"。
/// - venv 装在能力包外层 → [`prepend_path`] 是往**队首**插，而外层先跑，
///   所以后跑的排在前面。venv 在外的话能力包的目录会盖在用户显式选的
///   venv 前面，一句 `python3 manage.py` 就拿到包里那份、找不到项目依赖。
///
/// 结论：`能力包 → venv → 沙箱 → base`，从外到里。
fn process_chain(
    base: Arc<dyn riot_protocol::tool::ProcessRunner>,
    sandbox: Option<Arc<riot_runtime::ActiveSandbox>>,
    python_venv: Option<&str>,
    pack: Option<&crate::packs::InstalledPack>,
) -> Arc<dyn riot_protocol::tool::ProcessRunner> {
    let proc: Arc<dyn riot_protocol::tool::ProcessRunner> = match sandbox {
        Some(sb) => Arc::new(riot_runtime::SandboxedRunner::new(base, sb)),
        None => base,
    };
    let proc: Arc<dyn riot_protocol::tool::ProcessRunner> = match python_venv {
        Some(v) => Arc::new(VenvRunner::new(v, proc)),
        None => proc,
    };
    match pack {
        Some(p) => Arc::new(DocPackRunner::new(p, proc)),
        None => proc,
    }
}

fn prepend_path(spec: &mut riot_protocol::tool::ProcessSpec, dirs: &[std::path::PathBuf]) {
    if dirs.is_empty() {
        return;
    }
    let sep = if cfg!(windows) { ';' } else { ':' };
    let mut head = dirs
        .iter()
        .map(|d| d.display().to_string())
        .collect::<Vec<_>>()
        .join(&sep.to_string());

    match spec.env.iter_mut().find(|(k, _)| k == "PATH") {
        Some((_, existing)) => {
            head.push(sep);
            head.push_str(existing);
            *existing = head;
        }
        None => {
            if let Ok(host) = std::env::var("PATH") {
                head.push(sep);
                head.push_str(&host);
            }
            spec.env.push(("PATH".to_owned(), head));
        }
    }
}

/// 给工具子进程注入 Python 虚拟环境的 ProcessRunner。
struct VenvRunner {
    inner: Arc<dyn riot_protocol::tool::ProcessRunner>,
    bin: std::path::PathBuf,
    venv: String,
}

impl VenvRunner {
    fn new(venv: &str, inner: Arc<dyn riot_protocol::tool::ProcessRunner>) -> Self {
        Self {
            inner,
            bin: std::path::Path::new(venv).join(if cfg!(windows) { "Scripts" } else { "bin" }),
            venv: venv.to_owned(),
        }
    }
}

#[async_trait::async_trait]
impl riot_protocol::tool::ProcessRunner for VenvRunner {
    async fn run(
        &self,
        mut spec: riot_protocol::tool::ProcessSpec,
        cancel: CancellationToken,
    ) -> std::io::Result<riot_protocol::tool::ProcessOutput> {
        // 工具自己显式设的同名变量优先。
        if !spec.env.iter().any(|(k, _)| k == "VIRTUAL_ENV") {
            spec.env.push(("VIRTUAL_ENV".to_owned(), self.venv.clone()));
        }
        prepend_path(&mut spec, std::slice::from_ref(&self.bin));
        self.inner.run(spec, cancel).await
    }
}

/// 给工具子进程接上已安装的能力包（目前是文档运行时）。
///
/// 做成装饰器而不是在 `SystemProcessRunner` 里判断：和 venv、沙箱是同一类
/// 正交关注点，而"没装能力包"这条路径上一行相关代码都不会跑到。
///
/// `[约束]` 进 PATH 的只有 `pack.json` 里 `pathPrepend` 声明的那些目录 ——
/// 文档包故意把 `python3` 和 `node` 排除在外。放进去的话，用户给会话设了
/// venv 时一句 `python3 manage.py` 会拿到包里那份、找不到项目依赖；为了
/// 文档功能弄坏用户原本的 Python 工作流是不划算的。skill 正文里已经改成
/// 显式写 `$RUNTIME_BIN_DIR/python3`。
struct DocPackRunner {
    inner: Arc<dyn riot_protocol::tool::ProcessRunner>,
    env: Vec<(String, String)>,
    path_dirs: Vec<std::path::PathBuf>,
}

impl DocPackRunner {
    fn new(
        pack: &crate::packs::InstalledPack,
        inner: Arc<dyn riot_protocol::tool::ProcessRunner>,
    ) -> Self {
        Self {
            inner,
            env: pack.env(),
            path_dirs: pack.path_dirs(),
        }
    }
}

#[async_trait::async_trait]
impl riot_protocol::tool::ProcessRunner for DocPackRunner {
    async fn run(
        &self,
        mut spec: riot_protocol::tool::ProcessSpec,
        cancel: CancellationToken,
    ) -> std::io::Result<riot_protocol::tool::ProcessOutput> {
        for (k, v) in &self.env {
            if !spec.env.iter().any(|(ek, _)| ek == k) {
                spec.env.push((k.clone(), v.clone()));
            }
        }
        prepend_path(&mut spec, &self.path_dirs);
        self.inner.run(spec, cancel).await
    }
}

/// 这个事件算不算"这一轮在对话里留下了东西"（决定按停止时撤不撤提问）。
///
/// `[约束]` 只有 [`AgentEvent::Message`] 算，Delta 不算 —— 包括思考和
/// 已经吐出来的半截正文。理由是它们**哪里都没留下**：取消时 provider
/// 直接 return，不会有定稿消息，历史和 transcript 里一个字都没有；界面
/// 收到 Done 也把 streaming/thinking 清空。拿一个转瞬即逝的东西当"有
/// 产出"的凭据，用户按下停止看到的就是：思考没了，而自己那句提问孤零零
/// 留在对话里等一个永远不会来的回答。
///
/// 反过来，只要有一条 Message 落了地（哪怕是被取消的工具的结果），
/// 提问就必须留下 —— 撤了会在上下文里留下一个悬空的回答。
fn leaves_a_trace(ev: &AgentEvent) -> bool {
    matches!(ev, AgentEvent::Message(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::{apply_remember, hook_may_skip_ask, inject_choice, preview_of};
    use riot_protocol::id::RequestId;
    use riot_protocol::permission::{
        AskPreview, DecisionReason, GateOutcome, PermissionAsk, PermissionResponse, SafetyVerdict,
    };
    use tokio::sync::oneshot;

    #[test]
    fn 超时区间足够长但不是无限() {
        // 太短会在用户离开一会儿时误拒；无限会让会话永远结束不了
        assert!(*ASK_TIMEOUT_RANGE.start() >= 5);
        assert!(*ASK_TIMEOUT_RANGE.end() <= 3600);
    }

    #[test]
    fn 配置里的超时值会被夹进可用区间() {
        // config.json 用户能手改。0 会让每个弹窗瞬间超时 —— 那等于把
        // 「每次询问」悄悄变成「一律拒绝」，而界面上什么都看不出来。
        let clamp =
            |v: u32| u64::from(v).clamp(*ASK_TIMEOUT_RANGE.start(), *ASK_TIMEOUT_RANGE.end());
        assert_eq!(clamp(0), 5, "0 秒必须被抬到下限");
        assert_eq!(clamp(60), 60);
        assert_eq!(clamp(u32::MAX), 3600, "过大的值必须被压到上限");
    }

    #[test]
    fn 默认超时是一分钟() {
        // `[约束]` 这个默认值是为**长任务**定的，不是为盯屏幕的人定的。
        // 以前是 600 秒：一次误触发就把整轮任务钉住十分钟，而结局仍然
        // 是拒绝。既然结局一样，早点拒绝、让模型换条路走更有用。
        assert_eq!(crate::config::default_ask_timeout_secs(), 60);
    }

    #[test]
    fn 总是允许会落成会话级规则() {
        use riot_protocol::permission::{PermissionUpdate, RuleDecision, UpdateScope};

        let mut rules = Vec::new();
        let add = PermissionUpdate::AddRule {
            tool: "Bash".into(),
            pattern: Some("npm run *".into()),
            decision: RuleDecision::Allow,
            scope: UpdateScope::Session,
        };

        apply_remember(&mut rules, vec![add.clone()]);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].tool, "Bash");
        assert_eq!(
            rules[0].source,
            riot_protocol::permission::RuleSource::Session
        );

        // 同一条建议再点一次不会堆出重复规则
        apply_remember(&mut rules, vec![add]);
        assert_eq!(rules.len(), 1);

        // 改模式的建议被忽略 —— 明确不支持，而不是半支持
        apply_remember(
            &mut rules,
            vec![PermissionUpdate::SetMode {
                mode: PermissionMode::AcceptEdits,
                scope: UpdateScope::Session,
            }],
        );
        assert_eq!(rules.len(), 1);
    }

    /// 提问的卡片要拿到真的选项，而不是一句摘要。
    ///
    /// 退回 Plain 的话界面上是一行 describe 文本、没有按钮 —— 用户只能
    /// 点"跳过"，而模型在等一个选择。
    #[test]
    fn 提问工具的预览是带选项的_choice() {
        let input = serde_json::json!({
            "question": "缓存放哪？",
            "options": [
                { "id": "mem", "label": "内存" },
                { "id": "disk", "label": "磁盘（推荐）" }
            ],
            "allow_multiple": false
        });
        let p = preview_of(
            &riot_tools::tools::ask::AskUserQuestion,
            &input,
            std::path::Path::new("/tmp"),
        );
        let AskPreview::Choice {
            question,
            options,
            allow_multiple,
        } = p
        else {
            panic!("该是 Choice：{p:?}");
        };
        assert_eq!(question, "缓存放哪？");
        assert_eq!(options.len(), 2);
        assert_eq!(options[1].id, "disk");
        assert!(!allow_multiple);
    }

    /// 用户的选择必须真的走到工具手里 —— 这是整条链路的命门。
    ///
    /// 断言跨过宿主与工具的边界：宿主写进输入，工具读出来给模型。中间
    /// 任何一环把键名写错，两边各自的单测都还是绿的。
    #[tokio::test]
    async fn 用户的选择经输入改写送到工具() {
        let input = serde_json::json!({
            "question": "缓存放哪？",
            "options": [
                { "id": "mem", "label": "内存" },
                { "id": "disk", "label": "磁盘" }
            ]
        });

        let updated = inject_choice(&input, vec!["disk".into()]).expect("有选择就该改写输入");
        assert_eq!(
            updated.get("question").and_then(|v| v.as_str()),
            Some("缓存放哪？"),
            "改写不能弄丢原有字段"
        );

        let out = riot_tools::tools::ask::AskUserQuestion
            .call(updated, tool_ctx())
            .await;
        let riot_protocol::tool::ToolOutcome::Ok { model_content, .. } = out else {
            panic!("工具该拿到选择并成功：{out:?}");
        };
        let text = format!("{model_content:?}");
        assert!(
            text.contains("磁盘"),
            "模型要收到用户点的那个 label：{text}"
        );
    }

    /// 「其他」走同一条 `__chosen` 通道，编码前缀不能漏给模型。
    #[tokio::test]
    async fn 自己填写的其他经输入改写送到工具() {
        let input = serde_json::json!({
            "question": "缓存放哪？",
            "options": [
                { "id": "mem", "label": "内存" },
                { "id": "disk", "label": "磁盘" }
            ]
        });
        let updated = inject_choice(
            &input,
            vec![format!("{}用 sqlite", riot_tools::tools::ask::OTHER_PREFIX)],
        )
        .expect("有填写就该改写输入");
        let out = riot_tools::tools::ask::AskUserQuestion
            .call(updated, tool_ctx())
            .await;
        let riot_protocol::tool::ToolOutcome::Ok { model_content, .. } = out else {
            panic!("工具该拿到填写并成功：{out:?}");
        };
        let text = format!("{model_content:?}");
        assert!(text.contains("自己填写：用 sqlite"), "{text}");
        assert!(
            !text.contains(riot_tools::tools::ask::OTHER_PREFIX),
            "编码前缀不该漏给模型：{text}"
        );
    }

    /// 普通的"允许一次"不该给工具入参塞一个空字段。
    #[test]
    fn 没有选择时不改写输入() {
        let input = serde_json::json!({ "command": "ls" });
        assert!(inject_choice(&input, vec![]).is_none(), "空选择不该改写");
        // 参数不成形时静默不改，交给 validate_input 去报错。
        assert!(inject_choice(&serde_json::json!("字符串"), vec!["x".into()]).is_none());
    }

    /// 测试用的最小询问详情。
    fn ask_detail(tool: &str) -> PermissionAsk {
        PermissionAsk {
            tool_use_id: riot_protocol::id::ToolUseId::from_raw("t1"),
            tool_name: tool.to_owned(),
            summary: format!("运行 {tool}"),
            preview: AskPreview::Plain {
                text: String::new(),
            },
            suggestions: vec![],
            reason: DecisionReason::UserChoice { remembered: false },
        }
    }

    /// 半截流缓冲：增量累加，助手消息完成或轮子结束即清空。
    ///
    /// 历史只收完整消息 —— 这份缓冲是「切走再切回」时恢复正在生成内容的
    /// 唯一来源，思考块字数从 0 重数就是它缺位的症状。清空点必须和前端
    /// applyMessage 一致：消息完成时整段内容已在消息里，缓冲再留着就会
    /// 在下一段流里拼出重复。
    #[test]
    fn 半截流_增量累加_消息完成即清空() {
        use riot_protocol::event::TerminalReason;

        let mut live = LiveStream::default();
        let mid = MessageId::from_raw("m1");
        let delta = |text: &str| StreamDelta::Thinking {
            message_id: mid.clone(),
            text: text.into(),
        };
        fold_live(&mut live, &AgentEvent::Delta(delta("先看")));
        fold_live(&mut live, &AgentEvent::Delta(delta("内核")));
        fold_live(
            &mut live,
            &AgentEvent::Delta(StreamDelta::Text {
                message_id: mid.clone(),
                text: "好的".into(),
            }),
        );
        assert_eq!(live.thinking, "先看内核");
        assert_eq!(live.text, "好的");

        // 工具增量不进缓冲 —— 完整参数在 Message 里。
        fold_live(
            &mut live,
            &AgentEvent::Delta(StreamDelta::ToolInput {
                tool_use_id: riot_protocol::id::ToolUseId::from_raw("t1"),
                partial_json: "{\"a\":".into(),
            }),
        );
        assert_eq!(live.text, "好的", "工具参数不该混进正文");

        fold_live(
            &mut live,
            &AgentEvent::Message(Message::Assistant {
                id: mid,
                content: vec![],
                usage: Default::default(),
                meta: MessageMeta::default(),
            }),
        );
        assert!(
            live.text.is_empty() && live.thinking.is_empty(),
            "完整消息已带全部内容，缓冲留着会在下一段流里拼出重复"
        );

        fold_live(
            &mut live,
            &AgentEvent::Delta(StreamDelta::Text {
                message_id: MessageId::from_raw("m2"),
                text: "下一段".into(),
            }),
        );
        fold_live(
            &mut live,
            &AgentEvent::Done {
                reason: TerminalReason::Completed,
            },
        );
        assert!(live.text.is_empty(), "轮子结束也要清空");
    }

    #[tokio::test]
    async fn 回应不存在的请求不会崩() {
        // 用户在超时之后才点按钮，这时候什么都不该发生
        let p = PendingAsks::default();
        assert!(
            !p.resolve(
                "nope",
                PermissionResponse::Allow {
                    remember: vec![],
                    choice: vec![]
                }
            )
            .await
        );
    }

    /// 挂着的询问要能进会话快照：按到达顺序，解决后消失。
    ///
    /// `permission_request` 事件只发一次。界面切走再切回、或睡眠唤醒后
    /// 换通道，弹窗全靠 session.resume 里的这份快照重建 —— 快照缺了它，
    /// 那次询问只能等到超时被拒，而用户从头到尾看不见任何东西。
    #[tokio::test]
    async fn 挂着的询问进快照_按到达顺序_解决后消失() {
        let p = PendingAsks::default();
        let (tx1, _rx1) = oneshot::channel();
        let (tx2, _rx2) = oneshot::channel();
        // id 故意让字典序和到达顺序相反 —— 快照排序靠到达序号，不是名字。
        p.insert("ask-10".into(), tx1, ask_detail("Bash")).await;
        p.insert("ask-2".into(), tx2, ask_detail("Write")).await;

        let snap = p.snapshot().await;
        assert_eq!(snap.len(), 2);
        assert_eq!(
            snap[0].request_id,
            RequestId::from_raw("ask-10".to_owned()),
            "按到达顺序"
        );
        assert_eq!(snap[0].detail.tool_name, "Bash");
        assert_eq!(snap[1].request_id, RequestId::from_raw("ask-2".to_owned()));

        assert!(
            p.resolve(
                "ask-10",
                PermissionResponse::Allow {
                    remember: vec![],
                    choice: vec![]
                }
            )
            .await
        );
        let snap = p.snapshot().await;
        assert_eq!(snap.len(), 1, "解决掉的不该再出现在快照里");
        assert_eq!(snap[0].detail.tool_name, "Write");
    }

    #[tokio::test]
    async fn 回应之后请求就被摘掉了() {
        let p = PendingAsks::default();
        let (tx, rx) = oneshot::channel();
        p.insert("a1".into(), tx, ask_detail("Bash")).await;

        assert!(
            p.resolve(
                "a1",
                PermissionResponse::Allow {
                    remember: vec![],
                    choice: vec![]
                }
            )
            .await
        );
        assert!(rx.await.is_ok());
        // 第二次应该找不到 —— 否则重复点击会让同一个操作跑两遍
        assert!(
            !p.resolve(
                "a1",
                PermissionResponse::Allow {
                    remember: vec![],
                    choice: vec![]
                }
            )
            .await
        );
    }

    fn empty_spec() -> riot_protocol::tool::ProcessSpec {
        riot_protocol::tool::ProcessSpec {
            program: "true".to_owned(),
            args: vec![],
            cwd: std::path::PathBuf::from("/tmp"),
            env: vec![],
            timeout_ms: None,
            sandbox_exempt: false,
        }
    }

    fn path_of(spec: &riot_protocol::tool::ProcessSpec) -> String {
        spec.env
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.clone())
            .expect("有 PATH")
    }

    #[test]
    fn prepend_path_从宿主环境拼出完整的_path() {
        // ProcessSpec.env 是"覆盖这几个、其余继承"，只放新目录的话
        // 子进程连 bash 都找不到。
        let mut spec = empty_spec();
        prepend_path(&mut spec, &[std::path::PathBuf::from("/tmp/venv/bin")]);
        let path = path_of(&spec);
        assert!(path.starts_with("/tmp/venv/bin"), "新目录要排最前：{path}");
        assert!(
            path.len() > "/tmp/venv/bin".len(),
            "后面要接着宿主原有的 PATH：{path}"
        );
    }

    /// venv 和能力包两层都要改 PATH。用"没有才设"的语义时，先跑的那层会把
    /// 后跑的整个吞掉 —— 表现是设了 venv 就找不到 soffice。
    #[test]
    fn 两层装饰器的_path_叠加而不是互相覆盖() {
        let mut spec = empty_spec();
        prepend_path(&mut spec, &[std::path::PathBuf::from("/packs/doc/path")]);
        prepend_path(&mut spec, &[std::path::PathBuf::from("/proj/.venv/bin")]);

        let path = path_of(&spec);
        let venv = path.find("/proj/.venv/bin").expect("venv 要在");
        let pack = path.find("/packs/doc/path").expect("能力包要在");
        assert!(
            venv < pack,
            "用户显式选的 venv 要排在能力包前面，实际：{path}"
        );
    }

    /// 录下最终落到"真正起进程"那一层的 spec。
    #[derive(Default)]
    struct RecordingRunner(std::sync::Mutex<Option<riot_protocol::tool::ProcessSpec>>);

    #[async_trait::async_trait]
    impl riot_protocol::tool::ProcessRunner for RecordingRunner {
        async fn run(
            &self,
            spec: riot_protocol::tool::ProcessSpec,
            _cancel: CancellationToken,
        ) -> std::io::Result<riot_protocol::tool::ProcessOutput> {
            *self.0.lock().expect("录制锁") = Some(spec);
            Ok(riot_protocol::tool::ProcessOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
                timed_out: false,
                duration_ms: 0,
            })
        }
    }

    fn doc_pack() -> crate::packs::InstalledPack {
        crate::packs::InstalledPack {
            root: std::path::PathBuf::from("/packs/doc-runtime"),
            manifest: crate::packs::PackManifest {
                name: "doc-runtime".into(),
                version: "0.1.0".into(),
                platform: "darwin-arm64".into(),
                source_runtime: None,
                env: [("RUNTIME_NODE".to_owned(), "bin/node".to_owned())]
                    .into_iter()
                    .collect(),
                path_prepend: vec!["path".to_owned()],
                mcp_servers: vec![],
                skills: vec![],
            },
        }
    }

    /// `[约束]` 装配顺序：能力包 → venv → 沙箱 → 真正起进程的那层。
    ///
    /// 这条用例钉的是 [`process_chain`] 文档里那两个错法。上面
    /// `两层装饰器的_path_叠加而不是互相覆盖` 只验了 `prepend_path` 本身
    /// 叠加得对 —— 而真出过的 bug 是**装配顺序反了**（venv 在能力包外层），
    /// 那种情况下 prepend_path 每一步都正确，结果照样是错的。所以要走
    /// 真正的链条，看最里层收到什么。
    #[tokio::test]
    async fn 执行器链条的顺序_venv_压过能力包() {
        let rec = Arc::new(RecordingRunner::default());
        let chain = process_chain(
            Arc::clone(&rec) as Arc<dyn riot_protocol::tool::ProcessRunner>,
            None,
            Some("/proj/.venv"),
            Some(&doc_pack()),
        );

        chain
            .run(empty_spec(), CancellationToken::new())
            .await
            .expect("跑得起来");

        let spec = rec.0.lock().expect("录制锁").clone().expect("录到了");
        let path = path_of(&spec);
        let venv = path.find("/proj/.venv/bin").expect("venv 要在 PATH 里");
        let pack = path.find("/packs/doc-runtime/path").expect("能力包要在");
        assert!(venv < pack, "用户显式选的 venv 要排在能力包前面：{path}");
        assert!(
            spec.env.iter().any(|(k, v)| k == "VIRTUAL_ENV" && v == "/proj/.venv"),
            "VIRTUAL_ENV 要落到最里层：{:?}",
            spec.env
        );
        assert!(
            spec.env.iter().any(|(k, _)| k == "RUNTIME_NODE"),
            "能力包的变量也要落到最里层：{:?}",
            spec.env
        );
    }

    #[test]
    fn 能力包注入_runtime_变量并把工具目录放进_path() {
        let pack = doc_pack();
        let runner = DocPackRunner::new(&pack, Arc::new(SystemProcessRunner::default()));

        assert_eq!(
            runner.env,
            vec![(
                "RUNTIME_NODE".to_owned(),
                "/packs/doc-runtime/bin/node".to_owned()
            )],
            "RUNTIME_* 要解析成绝对路径 —— skill 自带脚本直接读它们"
        );
        assert_eq!(
            runner.path_dirs,
            vec![std::path::PathBuf::from("/packs/doc-runtime/path")],
            "进 PATH 的只能是 path/，bin/ 里的 python3 会盖掉用户的解释器"
        );
    }

    /// 一个不碰 fs / 进程 / 网络的工具上下文。给 AskUserQuestion 这类
    /// 纯对话工具用 —— 它只把用户的选择转成一句话。
    fn tool_ctx() -> riot_protocol::tool::ToolContext {
        let id = riot_protocol::id::ToolUseId::from_raw("t1");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        riot_protocol::tool::ToolContext {
            session_id: SessionId::from_raw("s1"),
            tool_use_id: id.clone(),
            cwd: "/tmp".into(),
            artifacts_dir: std::env::temp_dir(),
            cancel: CancellationToken::new(),
            progress: riot_protocol::tool::ProgressSink::new(id, tx),
            file_state: MemoryFileState::shared(),
            fs: Arc::new(SystemFs::new()),
            proc: Arc::new(SystemProcessRunner::default()),
            web: Arc::new(riot_protocol::web::NoWeb),
            browser: Arc::new(riot_protocol::browser::NoBrowser),
            terminal: Arc::new(riot_protocol::terminal::NoTerminal),
            vision: Arc::new(riot_protocol::vision::NoVision),
            clock: Arc::new(riot_providers::watchdog::TokioClock),
        }
    }

    /// 一个丢弃全部事件的出口。
    fn test_sink() -> SessionSink {
        struct Discard;
        impl EventSink for Discard {
            fn send(&self, _event: AgentEvent) -> Result<(), SinkClosed> {
                Ok(())
            }
        }
        let s = SessionSink::default();
        s.attach(Arc::new(Discard));
        s
    }

    /// 造一个装了指定 hooks 的权限闸。
    fn gate_with_hooks(s: &Session, hooks: serde_json::Value) -> HostGate {
        HostGate {
            sink: test_sink(),
            pending: Arc::clone(&s.pending_asks),
            ids: Arc::clone(&s.ids) as Arc<dyn IdGenerator>,
            ctx: PermissionContext {
                mode: PermissionModeState(Some(PermissionMode::Default)),
                rules: Vec::new(),
                sandboxed: false,
                can_prompt_user: true,
            },
            rules_live: Arc::clone(&s.rules),
            mode_live: Arc::clone(&s.mode),
            cwd: s.cwd.clone(),
            // 短超时：hook 要真卡住，测试该很快失败而不是挂十分钟。
            ask_timeout: Duration::from_secs(2),
            hooks: Arc::new(crate::hooks::HookEngine::from_config_json(hooks, &s.cwd)),
            classifier: Arc::new(riot_protocol::permission::NoClassifier),
        }
    }

    /// 一个把什么都判成安全的分类器。用它来试探边界:凡是它也放不过去的，
    /// 就是被别的机制挡住的。
    struct AlwaysSafe;

    #[async_trait::async_trait]
    impl riot_protocol::permission::SafetyClassifier for AlwaysSafe {
        async fn judge(&self, _tool: &str, _what: &str) -> SafetyVerdict {
            SafetyVerdict::Safe { confidence: 1.0 }
        }
    }

    /// Auto 模式 + 有求必应的分类器。
    async fn gate_auto(s: &Session) -> HostGate {
        s.set_mode(PermissionMode::Auto).await;
        HostGate {
            classifier: Arc::new(AlwaysSafe),
            ..gate_with_hooks(s, serde_json::json!({}))
        }
    }

    /// 跑一次竞速。`_tx` 要留着 —— 提前 drop 的话 rx 立刻出错返回，
    /// 会被当成"用户已经答了"，测的就不是判危了。
    async fn race(
        gate: &HostGate,
        tool: &dyn Tool,
        input: &serde_json::Value,
        reason: &DecisionReason,
    ) -> Option<DecisionReason> {
        let (_tx, rx) = oneshot::channel();
        tokio::pin!(rx);
        gate.classify_race(tool, input, reason, &mut rx, &CancellationToken::new())
            .await
    }

    /// **Auto 模式的安全边界。**
    ///
    /// 分类器的权力不能超过 bypass 模式：安全检查（写 SSH 密钥、shell 启动
    /// 脚本）和用户亲手写下的 ask 规则，它一律碰不到。把 classify_race 里
    /// 那句 `yields_to_bypass()` 判断删掉，只有这个用例会红。
    #[tokio::test]
    async fn 分类器压不过安全检查和用户规则() {
        let s = Session::new(SessionId::from_raw("s1"), std::env::temp_dir(), None);
        let gate = gate_auto(&s).await;
        let bash = tool_named("Bash");
        let input = serde_json::json!({ "command": "echo hi >> ~/.zshrc" });

        for reason in [
            DecisionReason::SafetyCheck {
                safety: riot_protocol::permission::SafetyKind::ShellRc,
            },
            DecisionReason::SafetyCheck {
                safety: riot_protocol::permission::SafetyKind::SshConfig,
            },
            DecisionReason::Rule {
                source: riot_protocol::permission::RuleSource::User,
                pattern: "rm *".into(),
            },
        ] {
            assert!(
                race(&gate, bash.as_ref(), &input, &reason).await.is_none(),
                "{reason:?} 触发的询问被分类器放行了 —— 这是 Auto 模式的安全边界"
            );
        }
    }

    /// 例行同意请求可以被判危放行 —— 这是 Auto 模式存在的意义。
    /// 一个什么都放不过的 Auto 模式等于 Default，白加一档。
    #[tokio::test]
    async fn 例行询问可以被判危放行() {
        let s = Session::new(SessionId::from_raw("s1"), std::env::temp_dir(), None);
        let gate = gate_auto(&s).await;
        let bash = tool_named("Bash");

        let got = race(
            &gate,
            bash.as_ref(),
            &serde_json::json!({ "command": "cargo check" }),
            &DecisionReason::Consent {
                what: "跑一条命令".into(),
            },
        )
        .await;

        assert!(
            matches!(got, Some(DecisionReason::Classifier { .. })),
            "该被自动放行，且理由要记成 Classifier（日志和界面要解释得清是谁批的）：{got:?}"
        );
    }

    /// 只有 Auto 模式才问分类器。别的模式下它装了也不该被咨询 ——
    /// 用户选 Default 就是要自己看每一个。
    #[tokio::test]
    async fn 非_auto_模式不咨询分类器() {
        let s = Session::new(SessionId::from_raw("s1"), std::env::temp_dir(), None);
        let gate = gate_auto(&s).await;
        let bash = tool_named("Bash");
        let input = serde_json::json!({ "command": "cargo check" });
        let consent = DecisionReason::Consent { what: "x".into() };

        for mode in [
            PermissionMode::Default,
            PermissionMode::AcceptEdits,
            PermissionMode::Plan,
        ] {
            s.set_mode(mode).await;
            assert!(
                race(&gate, bash.as_ref(), &input, &consent).await.is_none(),
                "{mode:?} 下不该咨询分类器"
            );
        }
    }

    /// 没覆盖 classifier_input 的工具等于"不参与自动判定"，照常问人。
    ///
    /// 这条是 fail-closed 的那一侧：新加的工具默认不被自动放行，要放行
    /// 得由工具作者显式交出判定文本。
    #[tokio::test]
    async fn 没给判定文本的工具不被自动放行() {
        let s = Session::new(SessionId::from_raw("s1"), std::env::temp_dir(), None);
        let gate = gate_auto(&s).await;
        let write = tool_named("Write");
        assert!(
            write
                .classifier_input(&serde_json::json!({ "path": "a.txt" }))
                .is_none(),
            "前提变了：Write 现在交了判定文本，这个用例要重写"
        );
        assert!(
            race(
                &gate,
                write.as_ref(),
                &serde_json::json!({ "path": "a.txt", "content": "x" }),
                &DecisionReason::Consent {
                    what: "写文件".into()
                },
            )
            .await
            .is_none(),
            "工具没交判定文本就不该被自动放行"
        );
    }

    fn tool_named(name: &str) -> Arc<dyn Tool> {
        riot_tools::tools::builtin()
            .into_iter()
            .find(|t| t.name() == name)
            .expect("内置工具")
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pretooluse_的_deny_直接拒掉工具() {
        // 这是 hooks 最重要的一条：脚本说不行，工具就不该跑 ——
        // 而且理由要发回模型（tool_result），它才知道换个做法。
        let s = Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        );
        let gate = gate_with_hooks(
            &s,
            serde_json::json!({
                "PreToolUse": [{"matcher": "Read", "hooks": [
                    {"type": "command", "command": "echo '这个目录不许读' >&2; exit 2"}
                ]}]
            }),
        );

        let outcome = gate
            .check(
                tool_named("Read").as_ref(),
                &serde_json::json!({ "path": "/tmp/x" }),
                &riot_protocol::id::ToolUseId::from_raw("tu1"),
                &CancellationToken::new(),
            )
            .await;

        match outcome {
            GateOutcome::Deny { message } => {
                assert!(
                    message.contains("这个目录不许读"),
                    "理由要带给模型：{message}"
                )
            }
            GateOutcome::Allow { .. } => panic!("hook 说了不行，不能放行"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pretooluse_的_allow_把要问变成放行() {
        // Read 在默认模式下本来是允许的，换个真会问的：Bash 带变量展开
        // （Unverifiable）。hook 说 allow 就不该再弹窗。
        let s = Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        );
        let gate = gate_with_hooks(
            &s,
            serde_json::json!({
                "PreToolUse": [{"hooks": [
                    {"type": "command", "command": "echo '{\"hookSpecificOutput\":{\"permissionDecision\":\"allow\"}}'"}
                ]}]
            }),
        );

        // 没有 pending ask 的接收端，真弹窗的话这里会等到超时（2 秒）
        // 然后按拒绝返回 —— 所以 Allow 就证明它没走询问那条路。
        let outcome = gate
            .check(
                tool_named("Bash").as_ref(),
                &serde_json::json!({ "command": "echo $HOME" }),
                &riot_protocol::id::ToolUseId::from_raw("tu1"),
                &CancellationToken::new(),
            )
            .await;
        assert!(
            matches!(outcome, GateOutcome::Allow { .. }),
            "hook 放行后不该再问"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hook_的_allow_压不过安全检查() {
        // 安全边界优先于用户脚本。反过来就意味着一行 hooks.json 能把整套
        // 安全检查关掉 —— 而 hooks.json 躺在项目目录里，clone 别人的仓库
        // 就可能带一个。这里用"写 SSH 私钥"这种对 bypass 都免疫的操作。
        let s = Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        );
        let gate = gate_with_hooks(
            &s,
            serde_json::json!({
                "PreToolUse": [{"hooks": [
                    {"type": "command", "command": "echo '{\"decision\":\"approve\"}'"}
                ]}]
            }),
        );

        // 没有弹窗接收端，走询问那条路就会等满 2 秒然后按拒绝返回 ——
        // Deny 即证明它没被 hook 放行。
        let home = std::env::var("HOME").expect("HOME");
        let outcome = gate
            .check(
                tool_named("Write").as_ref(),
                &serde_json::json!({ "path": format!("{home}/.ssh/id_rsa"), "content": "x" }),
                &riot_protocol::id::ToolUseId::from_raw("tu1"),
                &CancellationToken::new(),
            )
            .await;
        assert!(
            matches!(outcome, GateOutcome::Deny { .. }),
            "hook 的 allow 不能免掉安全检查"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hook_改写的输入会跟到执行() {
        // updatedInput 的用途是"把命令补成安全的版本"。改写必须在判定
        // **之前**生效并跟到执行 —— 判定看旧的、执行跑新的，就是按 A
        // 授权执行 B。
        let s = Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        );
        let gate = gate_with_hooks(
            &s,
            serde_json::json!({
                "PreToolUse": [{"matcher": "Read", "hooks": [{
                    "type": "command",
                    "command": "echo '{\"hookSpecificOutput\":{\"updatedInput\":{\"path\":\"/tmp/safe.txt\"}}}'"
                }]}]
            }),
        );

        let outcome = gate
            .check(
                tool_named("Read").as_ref(),
                &serde_json::json!({ "path": "/tmp/original.txt" }),
                &riot_protocol::id::ToolUseId::from_raw("tu1"),
                &CancellationToken::new(),
            )
            .await;

        match outcome {
            GateOutcome::Allow { updated_input } => assert_eq!(
                updated_input,
                Some(serde_json::json!({ "path": "/tmp/safe.txt" })),
                "改写没跟到执行"
            ),
            GateOutcome::Deny { message } => panic!("不该拒：{message}"),
        }
    }

    #[test]
    fn hook_能免掉的询问只有例行那几类() {
        // 这条是上面那个集成测试的纯函数版：加新的 DecisionReason 变体时，
        // 编译器不会提醒你想清楚"hook 能不能压过它"，这里替它记着。
        assert!(hook_may_skip_ask(&DecisionReason::Consent {
            what: "example.com".into()
        }));
        assert!(hook_may_skip_ask(&DecisionReason::Unverifiable {
            what: "Bash".into()
        }));
        assert!(hook_may_skip_ask(&DecisionReason::Mode {
            mode: PermissionMode::Default
        }));
        assert!(!hook_may_skip_ask(&DecisionReason::SafetyCheck {
            safety: riot_protocol::permission::SafetyKind::SshConfig
        }));
        assert!(!hook_may_skip_ask(&DecisionReason::Rule {
            source: riot_protocol::permission::RuleSource::Session,
            pattern: "Bash(rm *)".into(),
        }));
        // 提问不是信任问题：hook 的 allow 回答不了选择题，跳过卡片
        // 只会让 AskUserQuestion 拿着空选择必然失败。
        assert!(!hook_may_skip_ask(&DecisionReason::UserChoice {
            remembered: false
        }));
    }

    #[tokio::test]
    async fn 装配好的调度器带齐权限闸围栏和联网() {
        // 这三样每漏一个都编译得过、跑得起来，只是行为悄悄降级：
        // 漏权限闸 = 所有操作不再询问；漏围栏 = 什么文件都写不了；
        // 漏联网 = WebFetch/WebSearch 一律说"未配置"。
        let s = Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        );

        let gate = Arc::new(HostGate {
            sink: test_sink(),
            pending: Arc::clone(&s.pending_asks),
            ids: Arc::clone(&s.ids) as Arc<dyn IdGenerator>,
            ctx: PermissionContext {
                mode: PermissionModeState(Some(PermissionMode::Default)),
                rules: Vec::new(),
                sandboxed: false,
                can_prompt_user: true,
            },
            rules_live: Arc::clone(&s.rules),
            mode_live: Arc::clone(&s.mode),
            cwd: s.cwd.clone(),
            ask_timeout: Duration::from_secs(60),
            hooks: Arc::new(crate::hooks::HookEngine::empty()),
            classifier: Arc::new(riot_protocol::permission::NoClassifier),
        });

        let scheduler = s.build_scheduler(
            ToolAssembly {
                registry: Arc::new(Registry::new(riot_tools::tools::builtin()).expect("注册表")),
                prompt_ctx: PromptContext {
                    cwd: s.cwd.clone(),
                    platform: "test".into(),
                    sandboxed: false,
                    sibling_tools: Vec::new(),
                    today: "2026年8月".into(),
                },
                deferred: None,
            },
            Arc::new(riot_providers::watchdog::TokioClock),
            TurnCapabilities {
                web: Arc::new(riot_protocol::web::NoWeb),
                vision: Arc::new(riot_protocol::vision::NoVision),
                subagent_cheap: None,
                classifier: Arc::new(riot_protocol::permission::NoClassifier),
                extra_tools: Vec::new(),
            },
            gate,
            None,
            None,
        );

        assert!(scheduler.has_gate(), "没装权限闸，所有操作都会静默放行");
        assert!(scheduler.has_web(), "没装联网能力，联网工具会一律报未配置");
    }

    /// 模型开口之前就该知道自己在哪个分支。
    ///
    /// 不给的话它有两条路，都不好：先花一整轮跑 `git status`，或者干脆
    /// 不查 —— 后者更常见，表现是它在一个有未提交改动的工作区里
    /// checkout，或者若无其事地往 main 上提交。
    #[tokio::test]
    async fn 首条消息带上_git_快照() {
        // CARGO_MANIFEST_DIR 是 crates/riot-kernel;仓库根在上两级。
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("仓库根")
            .to_path_buf();
        let s = Session::new(SessionId::from_raw("s1"), repo, None);

        let env_text = s
            .first_message_prelude()
            .await
            .into_iter()
            .find_map(|c| match c {
                UserContent::Attachment(Attachment::Environment { text }) => Some(text),
                _ => None,
            });

        let Some(text) = env_text else {
            // 源码树不是 git 仓库（打包场景）时没得注，这条不算失败。
            eprintln!("这里不是 git 仓库，跳过");
            return;
        };
        assert!(text.contains("Git 仓库"), "{text}");
        assert!(
            text.contains("当前分支：") || text.contains("detached"),
            "总得说清在不在分支上：{text}"
        );
    }

    /// 不是 git 仓库时不该硬塞一段空快照进去。
    #[tokio::test]
    async fn 非仓库目录不注入_git() {
        let dir = std::env::temp_dir().join(format!("riot-nogit-sess-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建目录");
        let s = Session::new(SessionId::from_raw("s1"), dir.clone(), None);

        assert!(
            !s.first_message_prelude()
                .await
                .iter()
                .any(|c| matches!(c, UserContent::Attachment(Attachment::Environment { .. }))),
            "非 git 目录不该有环境快照"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn 同一会话不允许并发两轮() {
        let s = Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        );
        let model = test_model();
        // 第一轮会因为缺 key 立刻失败，但它必须把 running 清干净，
        // 否则会话就卡死了 —— 用户看到的是"发消息没反应"
        let ch = test_sink();
        let caps = TurnCapabilities {
            web: Arc::new(riot_protocol::web::NoWeb),
            vision: Arc::new(riot_protocol::vision::NoVision),
            subagent_cheap: None,
            classifier: Arc::new(riot_protocol::permission::NoClassifier),
            extra_tools: Vec::new(),
        };
        let input = TurnInput {
            text: "hi".into(),
            ..Default::default()
        };
        let limits = TurnLimits {
            ask_timeout_secs: 60,
            max_turns: 48,
            compact_threshold_tokens: 100_000,
            sandbox: crate::config::SandboxMode::Off,
        };
        let _ = s.run_turn(input, model, caps, ch, limits).await;
        assert!(s.running.lock().await.is_none(), "失败路径没有清理 running");
        // 这一轮连历史都没写进去（缺 key，provider 都没建起来）。占位留着
        // 的话，界面上会挂一条永远等不到回复、重启之后又消失的用户消息。
        assert!(
            s.pending_user.lock().await.is_none(),
            "失败路径没有清理准备中的用户消息"
        );
    }

    /// `/compact` 失败（空历史）也要把 running 放掉。
    ///
    /// `[约束]` 手动压缩借 `running` 挡并发轮次，于是它的每条退出路径都欠
    /// 一次释放。漏掉的表现和轮子失败不清 running 一样：会话从此永远"忙"，
    /// 发消息、再压缩全被拒，只能重启。
    #[tokio::test]
    async fn 手动压缩失败也要释放_running() {
        let s = Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        );
        let r = s.compact_now(test_model(), test_sink()).await;
        assert!(r.is_err(), "空历史没什么可压缩的");
        assert!(
            s.running.lock().await.is_none(),
            "失败路径没有释放 running，会话卡死"
        );
        assert!(!s.is_compacting(), "compacting 标志不能残留");
    }

    /// 还没定稿的用户消息也要出现在历史快照里。
    ///
    /// `[约束]` 用户消息要等主动压缩和图片转述（都是模型调用）跑完才进
    /// `history`，而 `running` 早就置位了。这段时间里切走再切回来，前端
    /// 拿到的快照必须已经带着它 —— 否则界面上只剩一个转圈的"正在生成"，
    /// 用户刚发出去的话不见了，等模型答完再切一次才又冒出来（真实反馈）。
    #[tokio::test]
    async fn 准备中的用户消息也进历史快照() {
        let s = Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        );
        *s.pending_user.lock().await = Some(hist_user("m1", "在准备"));

        let snap = s.history().await;
        assert_eq!(snap.len(), 1, "快照要带上准备中的那条：{snap:?}");
        assert!(
            s.history.lock().await.is_empty(),
            "它只给界面看，不能混进模型读的历史"
        );
    }

    fn test_model() -> riot_protocol::ModelEndpoint {
        riot_protocol::ModelEndpoint {
            protocol: riot_protocol::ApiProtocol::Openai,
            base_url: "https://api.deepseek.com".into(),
            api_path: String::new(),
            // 空 key:让 provider_from_endpoint 立即失败,不真打网络。这些测试
            // 验的是"轮子失败后清干净 running / 不并发",不关心失败的具体原因。
            api_key: String::new(),
            model: "deepseek-chat".into(),
            fallback_model: None,
            sampling: riot_protocol::EndpointSampling::default(),
        }
    }

    fn test_caps() -> TurnCapabilities {
        TurnCapabilities {
            web: Arc::new(riot_protocol::web::NoWeb),
            vision: Arc::new(riot_protocol::vision::NoVision),
            subagent_cheap: None,
            classifier: Arc::new(riot_protocol::permission::NoClassifier),
            extra_tools: Vec::new(),
        }
    }

    fn test_limits() -> TurnLimits {
        TurnLimits {
            ask_timeout_secs: 60,
            max_turns: 48,
            compact_threshold_tokens: 100_000,
            // 测试跑真命令，沙箱会把临时目录外的写拦掉 —— 那是另一组
            // 用例的事（riot-runtime::sandbox），这里只测调度和权限。
            sandbox: crate::config::SandboxMode::Off,
        }
    }

    fn queued_entry(id: &str, text: &str) -> QueuedEntry {
        QueuedEntry {
            id: id.into(),
            input: TurnInput {
                text: text.into(),
                ..Default::default()
            },
            msg: Message::User {
                id: MessageId::from_raw(id),
                content: vec![UserContent::Text { text: text.into() }],
                meta: MessageMeta::default(),
            },
        }
    }

    #[tokio::test]
    async fn 跑轮期间的历史随时可读() {
        // 用户切走会话再切回来，界面靠 history() 重建。这一轮的消息要是
        // 攒到结束才写进内存，切回来看到的就是这轮开始前的样子 ——
        // 新会话上表现为"聊天记录全没了"。
        let s = Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        );
        assert!(s.history().await.is_empty());
        assert!(!s.is_running().await, "还没开轮");

        // 模拟轮子跑起来：置位 running + 逐条追加（run_locked 的两件事）。
        *s.running.lock().await = Some(CancellationToken::new());
        s.history.lock().await.push(Message::User {
            id: MessageId::from_raw("m1"),
            content: vec![UserContent::Text {
                text: "打开这个文件".into(),
            }],
            meta: MessageMeta::default(),
        });

        assert_eq!(s.history().await.len(), 1, "跑到一半也要读得到");
        assert!(
            s.is_running().await,
            "切回来时要能看出还在跑，否则停止键没了"
        );
    }

    #[tokio::test]
    async fn 忙时提交入队并返回条目id() {
        // 这是排队消息的宿主侧入口：上一轮还在跑时 submit 必须入队返回
        // 条目 id，既不报"上一轮还在进行中"，也不动 running。
        let s = Arc::new(Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        ));
        *s.running.lock().await = Some(CancellationToken::new());

        let input = TurnInput {
            text: "插一句".into(),
            ..Default::default()
        };
        let id = s
            .submit(input, test_model(), test_caps(), test_sink(), test_limits())
            .await
            .expect("忙时提交该入队并返回条目 id");

        assert!(s.running.lock().await.is_some(), "排队不该动别人的 running");
        let panel = s.queue_snapshot();
        assert_eq!(panel.len(), 1, "面板该看到这条插话");
        assert_eq!(panel[0].id, id);
        assert_eq!(panel[0].text, "插一句");

        // 撤回拿到的是原始输入 —— 编辑要还原的是用户打的字，不是转述。
        let took = s.queue_take(&id).expect("撤回");
        assert_eq!(took.text, "插一句");
        assert!(s.queue_snapshot().is_empty(), "撤回后面板该空了");
        assert!(!s.queue_remove(&id), "已撤回的条目删不到第二次");
    }

    #[tokio::test]
    async fn 收尾时静默清空残留插话() {
        // 中断/出错的轮次没走到内核的 drain 点，队列里可能剩着插话。
        // 宿主只负责清空 —— 排队面板的镜像还在前端手里,由它决定接力
        // 重发还是留给用户处置,宿主再喊一嗓子只会出现两条提示。
        let s = Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        );
        s.queue.push(queued_entry("m_q1", "还有这个也改一下"));

        // 缺 key，这一轮立刻失败 —— 正是"没走到 drain 点"的那类收尾。
        let input = TurnInput {
            text: "hi".into(),
            ..Default::default()
        };
        let ch = test_sink();
        let _ = s
            .run_turn(input, test_model(), test_caps(), ch, test_limits())
            .await;

        assert!(s.queue_snapshot().is_empty(), "残留插话该被清空");
        assert!(s.running.lock().await.is_none(), "running 该清干净");
    }

    #[tokio::test]
    async fn 第一句话定下自动标题且只定一次() {
        // 自动标题是缓存的，不再每次从历史推导 —— 历史是惰性水合的，
        // 启动画侧边栏时它还没加载。
        let s = Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        );
        assert_eq!(s.title().await, None, "还没说过话");

        assert!(
            s.note_first_prompt("  你好，世界  ").await,
            "第一句要触发索引落盘"
        );
        assert_eq!(s.title().await.as_deref(), Some("你好，世界"));

        assert!(!s.note_first_prompt("第二句").await, "标题只定一次");
        assert_eq!(s.title().await.as_deref(), Some("你好，世界"));

        // 手动标题优先，清除后回退到自动标题
        s.set_title(Some("手动名".into())).await;
        assert_eq!(s.title().await.as_deref(), Some("手动名"));
        s.set_title(None).await;
        assert_eq!(s.title().await.as_deref(), Some("你好，世界"));
    }

    fn hist_user(id: &str, text: &str) -> Message {
        Message::User {
            id: MessageId::from_raw(id),
            content: vec![UserContent::Text { text: text.into() }],
            meta: MessageMeta::default(),
        }
    }

    fn hist_assistant(id: &str, text: &str) -> Message {
        Message::Assistant {
            id: MessageId::from_raw(id),
            content: vec![riot_protocol::message::AssistantContent::Text { text: text.into() }],
            usage: None,
            meta: MessageMeta::default(),
        }
    }

    fn hist_tool_result(id: &str, tool: &str) -> Message {
        Message::User {
            id: MessageId::from_raw(id),
            content: vec![UserContent::ToolResult {
                tool_use_id: riot_protocol::id::ToolUseId::from_raw(tool),
                content: riot_protocol::message::ToolResultContent::text("ok"),
                is_error: false,
            }],
            meta: MessageMeta::default(),
        }
    }

    #[test]
    fn 重新生成截在最近一条用户提示() {
        let hist = [
            hist_user("m1", "第一句"),
            hist_assistant("a1", "旧答"),
            hist_user("m2", "第二句"),
            hist_assistant("a2", "工具"),
            hist_tool_result("t1", "tu1"),
            hist_assistant("a3", "要重来的"),
        ];
        assert_eq!(cut_at_user_prompt(&hist, "a3"), Some(2));
        assert_eq!(cut_at_user_prompt(&hist, "a1"), Some(0));
        assert_eq!(cut_at_user_prompt(&hist, "missing"), None);
    }

    #[test]
    fn 工具结果不算用户提示() {
        let hist = [
            hist_user("m1", "做这个"),
            hist_assistant("a1", "先读"),
            hist_tool_result("t1", "tu1"),
            hist_assistant("a2", "答"),
        ];
        assert_eq!(
            cut_at_user_prompt(&hist, "a2"),
            Some(0),
            "中间的 tool_result 不能当成重新生成的起点"
        );
    }

    #[tokio::test]
    async fn 重新生成丢掉助手消息之后的历史() {
        let s = Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        );
        {
            let mut h = s.history.lock().await;
            h.push(hist_user("m1", "第一句"));
            h.push(hist_assistant("a1", "旧答"));
            h.push(hist_user("m2", "第二句"));
            h.push(hist_assistant("a2", "要丢掉"));
        }
        let keep = s.rewind_to_prompt("a2").await.expect("能截断");
        assert_eq!(keep, "m2");
        let hist = s.history().await;
        assert_eq!(hist.len(), 3);
        assert_eq!(hist[2].id().as_str(), "m2");
        assert!(s.queue_snapshot().is_empty());
    }

    #[tokio::test]
    async fn 重新生成可以截回压缩前的归档() {
        let s = Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        );
        s.ui_archive
            .lock()
            .await
            .extend([hist_user("m1", "压缩前"), hist_assistant("a1", "旧答")]);
        s.history
            .lock()
            .await
            .extend([hist_user("m2", "压缩后"), hist_assistant("a2", "新答")]);
        s.rewind_to_prompt("a1").await.expect("能截回归档");
        assert_eq!(s.ui_archive().await.len(), 1);
        assert!(s.history().await.is_empty(), "截回归档后活历史应清空");
    }

    /// 模型还没开口就被停止：那句提问从历史和 transcript 里一并消失。
    ///
    /// 只从内存里删不够 —— 重启水合会把它读回来，用户会看到一条自己
    /// 明明取消过、还带回了输入框的消息又躺在对话里。
    #[tokio::test]
    async fn 撤回的提问历史和记录里都不留() {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Arc::new(riot_store::Transcripts::new(dir.path()));
        let id = SessionId::from_raw("s1");
        let log = store.open(riot_store::TranscriptMeta {
            id: id.clone(),
            root: dir.path().to_path_buf(),
            created_at_ms: 0,
        });
        let s = Session::new(
            id.clone(),
            dir.path().to_path_buf(),
            Some(SessionPersist {
                store: Arc::clone(&store),
                log,
            }),
        );

        let msg = hist_user("m1", "发出去又后悔了");
        s.history.lock().await.push(msg.clone());
        if let Some(p) = &s.persist {
            p.log.append(&msg);
        }

        assert_eq!(
            s.withdraw_prompt(&MessageId::from_raw("m1")).await,
            Some(true),
            "撤完这个会话就空了"
        );
        assert!(s.history().await.is_empty());

        s.flush_log().await;
        let parts = store.load_parts(&id).await;
        assert!(parts.live.is_empty(), "重启后不该再读回来：{parts:?}");
    }

    /// 已经说出口的半截回答要留下，而且要留得住（重启还在）。
    ///
    /// 按停止常常是"够了，别说了"—— 用户读到一半的东西不该在 Done
    /// 到达的那一瞬间整段消失。
    #[tokio::test]
    async fn 被打断的半截回答定稿进历史() {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Arc::new(riot_store::Transcripts::new(dir.path()));
        let id = SessionId::from_raw("s1");
        let log = store.open(riot_store::TranscriptMeta {
            id: id.clone(),
            root: dir.path().to_path_buf(),
            created_at_ms: 0,
        });
        let s = Session::new(
            id.clone(),
            dir.path().to_path_buf(),
            Some(SessionPersist {
                store: Arc::clone(&store),
                log,
            }),
        );

        {
            let mut live = s.live_stream.lock().await;
            live.text.push_str("先说一半");
            live.thinking.push_str("想了很久");
        }
        let msg = s
            .finalize_partial("deepseek-chat")
            .await
            .expect("有半截正文就该定稿");
        match &msg {
            Message::Assistant { content, meta, .. } => {
                assert_eq!(content.len(), 1, "思考不定稿：没有签名，回喂给模型是错的");
                assert!(meta.interrupted, "界面靠它标注'已中断'");
            }
            other => panic!("该是一条助手消息：{other:?}"),
        }
        assert_eq!(s.history().await.len(), 1);
        assert!(
            s.live_stream.lock().await.text.is_empty(),
            "缓冲要清空，否则下一轮会把这段再定稿一遍"
        );

        s.flush_log().await;
        let parts = store.load_parts(&id).await;
        assert_eq!(parts.live.len(), 1, "重启后还得在：{parts:?}");

        // 没说过话的那一轮不该凭空长出一条空消息。
        assert!(s.finalize_partial("deepseek-chat").await.is_none());
    }

    /// 思考不算产出：模型转了几秒圈就被停，那句提问照样回输入框。
    ///
    /// 这条用例来自一次实测 —— 之前只要模型开始思考就算"有产出"，用户
    /// 按停止之后思考被丢弃（不落盘），提问却留在了对话里：屏幕上是一条
    /// 没人应答的消息，而输入框是空的。
    #[test]
    fn 思考和半截正文都不算产出() {
        use riot_protocol::event::StreamDelta;

        let m = MessageId::from_raw("m1");
        assert!(!leaves_a_trace(&AgentEvent::Delta(StreamDelta::Thinking {
            message_id: m.clone(),
            text: "让我想想".into(),
        })));
        assert!(
            !leaves_a_trace(&AgentEvent::Delta(StreamDelta::Text {
                message_id: m.clone(),
                text: "好的".into(),
            })),
            "半截正文同样不落盘，取消之后界面也会清掉"
        );
        assert!(leaves_a_trace(&AgentEvent::Message(hist_assistant(
            "a1",
            "答完了"
        ))));
    }

    /// 模型已经答过话就不能撤：撤了会在上下文里留下一个悬空的回答。
    #[tokio::test]
    async fn 产出过的那一轮不撤回提问() {
        let s = Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        );
        s.history
            .lock()
            .await
            .extend([hist_user("m1", "问题"), hist_assistant("a1", "半句答")]);

        assert_eq!(
            s.withdraw_prompt(&MessageId::from_raw("m1")).await,
            None,
            "末尾已经不是那条提问了"
        );
        assert_eq!(s.history().await.len(), 2, "什么都不该动");
    }

    /// 关会话 / 退应用的取消不是"用户按了停止"。
    ///
    /// 混为一谈的话，退出时正等着首字的那一轮会把用户的提问撤掉 ——
    /// 下次打开，他发过的话没了，而且没有任何地方能解释去哪了。
    #[tokio::test]
    async fn 关会话的取消不算用户按停止() {
        let s = Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        );
        *s.running.lock().await = Some(CancellationToken::new());
        assert!(s.abort_turn().await);
        assert!(!s.stopped_by_user.load(Ordering::Relaxed));

        assert!(s.interrupt().await);
        assert!(s.stopped_by_user.load(Ordering::Relaxed));

        // 没有轮子在跑时按停止不留痕 —— 否则这个标志会跨到下一轮去。
        let idle = Session::new(
            SessionId::from_raw("s2"),
            std::path::PathBuf::from("/tmp"),
            None,
        );
        assert!(!idle.interrupt().await);
        assert!(!idle.stopped_by_user.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn 忙时拒绝重新生成() {
        let s = Arc::new(Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        ));
        *s.running.lock().await = Some(CancellationToken::new());
        let err = s
            .regenerate("a1", test_model(), test_caps(), test_sink(), test_limits())
            .await
            .expect_err("忙着不该开重新生成");
        assert!(err.contains("正在跑"), "{err}");
    }

    #[test]
    fn 标题截断规则() {
        assert_eq!(title_excerpt("   "), None, "空白不算标题");
        assert_eq!(title_excerpt(" 你好 ").as_deref(), Some("你好"));
        let long: String = "字".repeat(50);
        assert_eq!(title_excerpt(&long).map(|t| t.chars().count()), Some(40));
    }

    #[test]
    fn bash_的预览是命令本身() {
        let tools = riot_tools::tools::builtin();
        let bash = tools
            .iter()
            .find(|t| t.name() == "Bash")
            .expect("有 Bash 工具");

        let p = preview_of(
            bash.as_ref(),
            &serde_json::json!({ "command": "rm -rf build" }),
            std::path::Path::new("/w"),
        );
        match p {
            AskPreview::Command { command, .. } => assert_eq!(command, "rm -rf build"),
            other => panic!("弹窗必须显示完整命令，否则用户是在盲签：{other:?}"),
        }
    }

    // ── 环境感知（docs/ENV_DESIGN.md）───────────────────────────────

    /// 按脚本吐快照的探针替身。
    struct FakeEnv(std::sync::Mutex<Vec<riot_protocol::env::EnvSnapshot>>);

    impl FakeEnv {
        fn new(snaps: Vec<riot_protocol::env::EnvSnapshot>) -> Arc<Self> {
            Arc::new(Self(std::sync::Mutex::new(snaps)))
        }
    }

    #[async_trait::async_trait]
    impl riot_protocol::env::EnvProbe for FakeEnv {
        async fn sample(&self) -> Option<riot_protocol::env::EnvSnapshot> {
            let mut g = self.0.lock().expect("脚本锁");
            if g.is_empty() {
                None
            } else {
                Some(g.remove(0))
            }
        }
    }

    fn env_snap(terms: &[(u32, &str, bool)]) -> riot_protocol::env::EnvSnapshot {
        riot_protocol::env::EnvSnapshot {
            mine: terms
                .iter()
                .map(|(id, cmd, running)| riot_protocol::terminal::TerminalInfo {
                    id: *id,
                    title: format!("t{id}"),
                    command: Some((*cmd).to_owned()),
                    running: *running,
                    shared: false,
                })
                .collect(),
            shared: vec![],
            unshared_count: 0,
            browser: None,
            alerts: vec![],
        }
    }

    fn quiet_snap() -> riot_protocol::env::EnvSnapshot {
        env_snap(&[])
    }

    fn env_texts(parts: &[UserContent]) -> Vec<String> {
        parts
            .iter()
            .filter_map(|c| match c {
                UserContent::Attachment(Attachment::Environment { text }) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// 差分注入的核心不变量：没变化 = 零注入。防的是上下文膨胀 ——
    /// 每轮复读一遍环境，长会话里会堆出几十份一样的快照。
    #[tokio::test]
    async fn 环境快照_变化才注入_不变零注入() {
        let s = Session::new(SessionId::from_raw("s1"), std::env::temp_dir(), None);
        let a = env_snap(&[(3, "pnpm dev", true)]);
        s.attach_env(FakeEnv::new(vec![
            a.clone(),
            a.clone(),
            env_snap(&[(3, "pnpm dev", false)]),
        ]));

        // 第一轮：有东西，注入全量。
        let first = s.env_prelude(0, 100_000).await;
        assert_eq!(env_texts(&first).len(), 1, "首轮该注入");
        assert!(env_texts(&first)[0].contains("[3]"), "{first:?}");

        // 第二轮：一模一样，零注入。
        let second = s.env_prelude(0, 100_000).await;
        assert!(second.is_empty(), "没变化不该注入：{second:?}");

        // 第三轮：服务退出了（running 翻转），差分触发。
        let third = s.env_prelude(0, 100_000).await;
        assert_eq!(env_texts(&third).len(), 1, "状态变了该再注入");
        assert!(env_texts(&third)[0].contains("已退出"), "{third:?}");
    }

    /// 首轮对着空环境不说话；但从有到无是变化，要说。
    #[tokio::test]
    async fn 环境快照_首轮安静跳过_从有到无要说() {
        let s = Session::new(SessionId::from_raw("s1"), std::env::temp_dir(), None);
        s.attach_env(FakeEnv::new(vec![
            quiet_snap(),
            env_snap(&[(3, "pnpm dev", true)]),
            quiet_snap(),
        ]));

        assert!(
            s.env_prelude(0, 100_000).await.is_empty(),
            "对着空房间描述空房间是噪音"
        );
        assert!(
            !s.env_prelude(0, 100_000).await.is_empty(),
            "终端出现了该说"
        );
        let gone = s.env_prelude(0, 100_000).await;
        assert!(
            env_texts(&gone)[0].contains("没有你能看的终端"),
            "从有到无也是变化：{gone:?}"
        );
    }

    /// 探针拿不到（宿主没装配）就什么都不注 —— 感知是锦上添花。
    #[tokio::test]
    async fn 环境快照_探针不可用则零注入() {
        let s = Session::new(SessionId::from_raw("s1"), std::env::temp_dir(), None);
        // 不 attach：默认 NoEnvProbe。
        assert!(s.env_prelude(50_000, 100_000).await.is_empty());
    }

    /// 档位只在向上越档时说一次；压缩重置后允许再说。
    #[tokio::test]
    async fn 上下文档位_越档才说_压缩后重置() {
        let s = Session::new(SessionId::from_raw("s1"), std::env::temp_dir(), None);
        let q = quiet_snap;
        s.attach_env(FakeEnv::new(vec![q(), q(), q(), q(), q()]));

        assert!(
            s.env_prelude(40_000, 100_000).await.is_empty(),
            "50% 以下不说话"
        );
        let at72 = s.env_prelude(72_000, 100_000).await;
        assert!(
            env_texts(&at72)[0].contains("72%"),
            "越过 70 档要报实际百分比：{at72:?}"
        );
        assert!(
            s.env_prelude(73_000, 100_000).await.is_empty(),
            "同档内不重复唠叨"
        );

        // 模拟压缩重置（compact_history 里那两行的镜像）。
        *s.env_seen.lock().await = None;
        *s.env_band.lock().await = 0;
        let after = s.env_prelude(60_000, 100_000).await;
        assert!(
            env_texts(&after)[0].contains("60%"),
            "压缩归零后再次越档要能说：{after:?}"
        );
    }

    /// 告警走 system-reminder、带防分心护栏，且不受快照指纹影响。
    #[tokio::test]
    async fn 环境告警_独立于快照差分() {
        let s = Session::new(SessionId::from_raw("s1"), std::env::temp_dir(), None);
        let base = env_snap(&[(3, "pnpm dev", true)]);
        let mut alerted = base.clone();
        alerted.alerts.push(riot_protocol::env::EnvAlert {
            terminal_id: 3,
            title: "t3".into(),
            excerpt: "Error: EADDRINUSE".into(),
        });
        s.attach_env(FakeEnv::new(vec![base, alerted]));

        let _ = s.env_prelude(0, 100_000).await;
        // 第二轮快照文本没变（alerts 不进指纹），但告警要出来。
        let second = s.env_prelude(0, 100_000).await;
        assert!(
            env_texts(&second).is_empty(),
            "快照没变不该重发：{second:?}"
        );
        let reminders: Vec<&String> = second
            .iter()
            .filter_map(|c| match c {
                UserContent::Attachment(Attachment::SystemReminder { text }) => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(reminders.len(), 1, "{second:?}");
        assert!(reminders[0].contains("EADDRINUSE"), "{reminders:?}");
        assert!(
            reminders[0].contains("不必评论"),
            "防分心护栏：{reminders:?}"
        );
    }
}
