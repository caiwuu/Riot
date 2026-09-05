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

use std::future::Future;
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

/// 排队中的一条待注入消息。
struct QueuedEntry {
    /// 条目 id，同时也是构建好的消息的 MessageId —— 前端靠它把
    /// "排队面板里的条目"和"注入后回流的消息"对上。
    id: String,
    kind: QueuedKind,
    msg: Message,
}

/// 条目的来源。决定它**什么时候**注入、排队面板看不看得见、轮次半路
/// 收场时怎么处置 —— 三件事都不一样，所以这里是三个变体而不是一个
/// `Option<TurnInput>`。
enum QueuedKind {
    /// 用户在跑轮中发的消息。等当前任务**完全跑完**才注入（Cursor 语义，
    /// 见 riot_core 的收尾 drain）；面板上可见、可撤回编辑。
    ///
    /// 带着原始输入而不只是构建好的消息：注入用后者（转述等慢活已完成），
    /// 撤回编辑用前者 —— 从消息反推输入会把图片还原成转述文字。
    Interjection(TurnInput),
    /// 后台子 agent 的完成通知（见 [`Session::deliver_task_notice`]）。
    /// 工具轮边界就注入；面板看不见、删不到。轮被中断时攒进
    /// `pending_notices` 等下一轮。
    TaskNotice,
    /// 界面按钮的带外提醒（转到后台 / 并行构建，见 [`Session::nudge`]）。
    /// 同样工具轮边界注入 —— 它是对正在进行的工作说话，等整轮跑完就
    /// 没有意义了。同理，轮被中断时直接作废，不留到下一轮。
    Nudge,
}

impl QueuedKind {
    /// 用户插话（面板可见、收尾才注入）还是带外消息（工具轮边界注入）。
    fn is_interjection(&self) -> bool {
        matches!(self, Self::Interjection(_))
    }
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

    /// 取走带外条目（通知、按钮提醒），用户插话留在队列里等收尾。
    fn take_out_of_band(&self) -> Vec<QueuedEntry> {
        let mut g = self.0.lock().expect("插话队列锁不该中毒");
        let (out, keep) = std::mem::take(&mut *g)
            .into_iter()
            .partition(|e| !e.kind.is_interjection());
        *g = keep;
        out
    }

    fn snapshot(&self) -> Vec<QueuedSummary> {
        self.0
            .lock()
            .expect("插话队列锁不该中毒")
            .iter()
            .filter_map(|e| {
                let QueuedKind::Interjection(input) = &e.kind else {
                    return None;
                };
                Some(QueuedSummary {
                    id: e.id.clone(),
                    text: input.text.clone(),
                    images: input.images.len(),
                    refs: input.refs.clone(),
                })
            })
            .collect()
    }

    /// 只删用户插话。带外条目前端看不见，也删不到 —— 它们的去处由内核定。
    fn remove(&self, id: &str) -> bool {
        let mut g = self.0.lock().expect("插话队列锁不该中毒");
        let before = g.len();
        g.retain(|e| !(e.id == id && e.kind.is_interjection()));
        g.len() < before
    }

    fn take(&self, id: &str) -> Option<TurnInput> {
        let mut g = self.0.lock().expect("插话队列锁不该中毒");
        let at = g
            .iter()
            .position(|e| e.id == id && e.kind.is_interjection())?;
        match g.remove(at).kind {
            QueuedKind::Interjection(input) => Some(input),
            _ => None,
        }
    }
}

impl riot_core::state::InputQueue for HostInputQueue {
    fn drain(&self) -> Vec<Message> {
        self.take_all().into_iter().map(|e| e.msg).collect()
    }

    fn drain_out_of_band(&self) -> Vec<Message> {
        self.take_out_of_band().into_iter().map(|e| e.msg).collect()
    }
}

/// 本轮要用的宿主能力。
///
/// `[约束]` 每轮现装，不缓存在会话上。用户中途打开搜索或改覆盖地址、给服务方
/// 勾上「支持图片」、换掉视觉兼容模型 —— 下一轮就该生效，而不是要重启。
///
/// 打成一包而不是各自当参数:它们的生命周期和取值时机完全一样，而摊平之后
/// `run_turn` 的参数列表长到要靠数位置来读。
///
/// `Clone` 是给后台子 agent 的唤醒轮用的（见 [`LastTurn`]）：那一轮不是
/// 宿主发起的，没人现装能力，只能沿用上一轮那份。
#[derive(Clone)]
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
    /// 压缩后的完整历史：续接消息 + 原样保留的尾巴。
    history: Vec<Message>,
    before_tokens: u32,
    after_tokens: u32,
}

/// 一次后台预压缩：轮刚结束时对历史发起的总结请求（见
/// [`Session::spawn_precompact`]）。
///
/// 这里只有"基于哪份历史、算到哪一步"，没有任何副作用 —— 边界落盘、
/// 记忆重注、归档写文件全部留到换入那一刻（[`Session::finish_compaction`]），
/// 由持有 `running` 的下一轮来做。后台任务只产出一个字符串。
struct Precompact {
    /// 总结基于的历史指纹（[`history_fingerprint`]）。换入时对不上就作废。
    fingerprint: (usize, String),
    /// 总结覆盖 `history[..split]`，尾巴原样保留。和指纹一起钉住，换入时
    /// 不重算 —— 重算结果理论上相同（纯函数），但两处各算一次没有意义。
    split: usize,
    cancel: CancellationToken,
    /// `None` = 总结失败（原因已进日志）。
    task: tokio::task::JoinHandle<Option<String>>,
}

impl Precompact {
    fn abandon(self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

/// 历史的指纹：条数 + 末条 id。
///
/// 用它而不是逐条比较：预压缩和换入之间历史要么原样不动、要么被上下文
/// 编辑/删除/重新生成动过 —— 那几种改动都会让条数或末条变化。同条数、
/// 同末条、中间某条被原地编辑过（`edit_message`）是唯一的漏网情形，
/// 所以那条路径上显式作废（[`Session::drop_precompact`]），不靠指纹。
fn history_fingerprint(history: &[Message]) -> (usize, String) {
    (
        history.len(),
        history
            .last()
            .map(|m| m.id().as_str().to_owned())
            .unwrap_or_default(),
    )
}

/// 一轮的数值上限，每轮从配置现取。
///
/// 打成一包而不是各自当参数，理由和 [`TurnCapabilities`] 一样:取值时机相同，
/// 而且两个字段都是 `u32` —— 摊成位置参数一旦顺序写反，编译器一声不吭，
/// 表现是"超时和轮数对调了"这种极难查的 bug。
#[derive(Clone)]
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
    /// 沙箱内额外可读的路径（手填的 allowRead），随隔离强度一起每轮现取。
    pub sandbox_allow_read: Vec<String>,
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

/// 一轮怎么开始。
///
/// 三种起点三种历史处理：用户输入要经过图片转述、`@` 展开、记忆注入才成为
/// 消息；重新生成时历史已经以提问结尾、什么都不追加；后台子 agent 的完成
/// 通知是内核合成好的消息，直接追加（见 [`Session::deliver_task_notice`]）。
pub enum TurnStart {
    User(TurnInput),
    Regenerate,
    Notices(Vec<Message>),
}

/// 上一轮用的模型端点、能力、上限。
///
/// 存它是为了**没有宿主参与**也能开一轮：后台子 agent 跑完时父会话可能空闲，
/// 通知要唤起新的一轮，而每轮的配置本来由宿主在 turn.submit 里现给。这里
/// 沿用上一轮那份 —— 唤醒轮是同一场对话的延续，用同一个模型是对的（Cursor
/// 续接子 agent 也忽略新模型、沿用旧的）。会话自己的活设置（模式、venv、
/// 追加提示词、思考策略）不在这里面，run_inner 从会话上现读，用户中途改
/// 的照常生效。
#[derive(Clone)]
struct LastTurn {
    model: riot_protocol::ModelEndpoint,
    caps: TurnCapabilities,
    limits: TurnLimits,
}

/// 自我分叉的种子：父这一轮的请求形状 + 造调度器要的零件。
///
/// 分叉出的子 agent 要和父**同 system、同工具清单**（前缀缓存命中的前提，
/// 见 `subagent` 模块文档），所以这些东西在 run_inner 装配完就存一份，
/// Task 工具收到 `resume: "self"` 时从这里取。工具清单里 Task 已经换成了
/// 深度 1 的那份（同名同形，只是不能再分叉）。
///
/// 不放 `Arc<Registry>`：父的注册表里有深度 0 的 Task 工具，而它的 deps
/// 能摸到会话 —— 存回来就是引用环。存工具列表，用时现建注册表（便宜）。
#[derive(Clone)]
struct ForkSeed {
    system: String,
    /// 父的工具清单，`task_index` 位置是一个占位的深度 1 Task。分叉时用
    /// 带上分叉 agent id（作为 parent）的那份换掉 —— 同形，只是登记时知道
    /// 自己派的子 agent 该挂在谁下面。
    tools: Vec<Arc<dyn Tool>>,
    task_index: usize,
    subagent_deps: crate::subagent::SubagentDeps,
    prompt_ctx: PromptContext,
    deferred: Option<Arc<riot_tools::tools::tool_search::DeferredPool>>,
    gate: Arc<dyn PermissionGate>,
    provider: Arc<dyn Provider>,
    model: String,
    max_turns: u32,
    thinking: riot_protocol::ThinkingPolicy,
    max_output_tokens_override: Option<u32>,
}

/// 一个会话在磁盘上的落点：transcript 通道 + 工件目录。
///
/// `store` 负责读（水合、索引重建），`log` 负责追加。分开是因为读是一次性的
/// 全量重放，写是贯穿会话生命周期的流 —— 两者的生命周期和并发语义都不同。
///
/// `artifacts_root` 是工件（截图、过大工具结果）的根目录，会话在
/// 它下面开自己的子目录（见 [`Session::artifacts_dir`]）。由宿主/manager 按
/// [`crate::config::artifacts_root`] 算好传进来，Session 自己不碰配置路径 ——
/// 于是没有持久化通道的会话（单元测试）也就没有任何路径能通到用户真实的
/// 配置目录。
pub struct SessionPersist {
    pub store: Arc<riot_store::Transcripts>,
    pub log: riot_store::SessionLog,
    pub artifacts_root: std::path::PathBuf,
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
    /// 会话级采样覆盖。字段为 None 表示继承模型/provider 那两层。
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
    /// 后台预压缩的产物（见 [`Self::spawn_precompact`]）。不落盘、不占
    /// `running`：它只是"提前算好的一份总结"，换不换入由下一轮开工时按
    /// 指纹决定。
    precompact: Mutex<Option<Precompact>>,
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
    /// 定时任务调度器（宿主能力）。宿主创建会话后挂上远程代理；
    /// 没挂上时 Schedule 工具用 `NoSchedule` 明说用不了。
    schedule: std::sync::OnceLock<Arc<dyn riot_protocol::schedule::ScheduleAccess>>,
    /// 上次注入的环境快照渲染文本 —— 差分判定的指纹。None = 模型手上
    /// 没有可信快照（新会话 / 压缩后 / 采样断供已宣告作废），下一轮发全量。
    /// 重启水合时从 transcript 恢复（见 [`Self::hydrate`]）—— 从 None 起步
    /// 的话，环境恰好变空会命中"首轮安静跳过"，历史里的旧快照就被
    /// 「没有新快照 = 没变」的契约反向背书成现状。
    env_seen: Mutex<Option<String>>,
    /// 上次宣告过的上下文用量档位（0/50/70/85）。只升不降，压缩时归零。
    env_band: Mutex<u32>,
    /// 子 agent 登记表（后台任务面板、续接的历史）。见 [`crate::tasks`]。
    tasks: Arc<crate::tasks::BackgroundTasks>,
    /// 上一轮的配置，唤醒轮沿用。见 [`LastTurn`]。
    last_turn: Mutex<Option<LastTurn>>,
    /// 本轮的分叉种子。见 [`ForkSeed`]。
    fork_seed: Mutex<Option<ForkSeed>>,
    /// 还没送进历史的完成通知：到达时会话还没跑过任何一轮（没有配置可
    /// 沿用），或者上一轮被中断、通知卡在队列里没到安全点。下一轮开工时
    /// 一并注入。std Mutex：锁内只有 Vec 操作。
    pending_notices: std::sync::Mutex<Vec<Message>>,
    /// 会话正在关闭（删会话 / 退应用）。此后到达的通知丢弃 —— 否则后台
    /// 子 agent 收尾时会往一个正在被删的 transcript 里写、唤起一轮没人看的
    /// 对话。
    closing: AtomicBool,
    /// 多任务模式（见 `prompt::multitask_reminder`）。宿主是权威，每轮现设。
    multitask: AtomicBool,
    /// 完整准则已经在历史里了：之后每轮只注短提醒。历史被动过（压缩、
    /// 回退、撤回）就清掉，下一轮重注完整版。
    multitask_announced: AtomicBool,
    /// 刚关掉多任务模式，下一轮要说一声"恢复正常"。只说一次。
    multitask_exit_pending: AtomicBool,
    /// 会话摘录写入器（见 [`crate::digest`]）。宿主创建/恢复会话后挂上；
    /// 没挂就是不写（单元测试）。
    digests: std::sync::OnceLock<Arc<crate::digest::DigestWriter>>,
    /// 会话创建时刻，水合时从 transcript 首行拿到。0 = 还不知道
    /// （没水合过 / 没有持久化通道），摘录头部退回 log 的元数据。
    created_at_ms: std::sync::atomic::AtomicU64,
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

/// 真正的用户提示。定义挪去了 [`Message::is_user_prompt`]（上下文删除
/// 按轮成对删，轮边界必须和这里同一个判定），这里留一个薄委托。
fn is_user_prompt(m: &Message) -> bool {
    m.is_user_prompt()
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
        let sink = SessionSink::default();
        Self {
            id,
            cwd,
            browser: std::sync::OnceLock::new(),
            history: Mutex::new(Vec::new()),
            running: Mutex::new(None),
            stopped_by_user: AtomicBool::new(false),
            queue: Arc::new(HostInputQueue::default()),
            tasks: Arc::new(crate::tasks::BackgroundTasks::new(sink.clone())),
            sink,
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
            precompact: Mutex::new(None),
            pending_user: Mutex::new(None),
            terminal: std::sync::OnceLock::new(),
            env: std::sync::OnceLock::new(),
            schedule: std::sync::OnceLock::new(),
            env_seen: Mutex::new(None),
            env_band: Mutex::new(0),
            last_turn: Mutex::new(None),
            fork_seed: Mutex::new(None),
            pending_notices: std::sync::Mutex::new(Vec::new()),
            closing: AtomicBool::new(false),
            multitask: AtomicBool::new(false),
            multitask_announced: AtomicBool::new(false),
            multitask_exit_pending: AtomicBool::new(false),
            digests: std::sync::OnceLock::new(),
            created_at_ms: std::sync::atomic::AtomicU64::new(0),
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
        let sink = SessionSink::default();
        Self {
            id,
            cwd,
            browser: std::sync::OnceLock::new(),
            history: Mutex::new(Vec::new()),
            running: Mutex::new(None),
            stopped_by_user: AtomicBool::new(false),
            queue: Arc::new(HostInputQueue::default()),
            tasks: Arc::new(crate::tasks::BackgroundTasks::new(sink.clone())),
            sink,
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
            precompact: Mutex::new(None),
            pending_user: Mutex::new(None),
            terminal: std::sync::OnceLock::new(),
            env: std::sync::OnceLock::new(),
            schedule: std::sync::OnceLock::new(),
            env_seen: Mutex::new(None),
            env_band: Mutex::new(0),
            last_turn: Mutex::new(None),
            fork_seed: Mutex::new(None),
            pending_notices: std::sync::Mutex::new(Vec::new()),
            closing: AtomicBool::new(false),
            multitask: AtomicBool::new(false),
            multitask_announced: AtomicBool::new(false),
            multitask_exit_pending: AtomicBool::new(false),
            digests: std::sync::OnceLock::new(),
            created_at_ms: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// 挂上摘录写入器。宿主创建/恢复会话之后调一次。
    pub fn attach_digests(&self, w: Arc<crate::digest::DigestWriter>) {
        let _ = self.digests.set(w);
    }

    /// 这个项目的摘录目录 —— 提示词里指给模型的路径。没挂写入器或用户
    /// 关掉了功能就是 None，提示词那一节整个不出现。
    fn digests_dir(&self) -> Option<std::path::PathBuf> {
        self.digests.get().and_then(|w| w.project_dir(&self.cwd))
    }

    /// 这个会话自己的摘录文件 —— 压缩续接消息里指给模型的路径。不看
    /// 「历史会话回忆」开关（压缩归档不是用户能关的）；没挂写入器（单元
    /// 测试）才是 None，续接消息的措辞退回"只有总结"。
    fn digest_path(&self) -> Option<std::path::PathBuf> {
        self.digests.get().map(|w| w.path_for(&self.cwd, &self.id))
    }

    /// 重写这个会话的摘录（见 [`crate::digest`] 的触发点清单）。
    ///
    /// 快照在写入器的项目锁里取：界面归档 + 活历史 + 此刻的标题。写失败
    /// 由写入器告警，这里不关心结果 —— 摘录是缓存。
    pub async fn refresh_digest(&self) {
        let Some(w) = self.digests.get() else { return };
        // 没水合过的会话历史是空的 —— 不先水合，改一次名字就把摘录当成
        // "历史被删空了"收掉。
        self.hydrate().await;
        w.write_with(&self.cwd, || async {
            let mut messages = self.ui_archive.lock().await.clone();
            messages.extend(self.history.lock().await.iter().cloned());
            let created = match self.created_at_ms.load(Ordering::Relaxed) {
                0 => self
                    .persist
                    .as_ref()
                    .map(|p| p.log.meta().created_at_ms)
                    .unwrap_or(0),
                t => t,
            };
            crate::digest::DigestSnapshot {
                id: self.id.clone(),
                root: self.cwd.clone(),
                title: self.title().await,
                created_at_ms: created,
                messages,
            }
        })
        .await;
    }

    /// 挂上定时任务调度器。宿主创建/恢复会话之后调一次。
    pub fn attach_schedule(&self, access: Arc<dyn riot_protocol::schedule::ScheduleAccess>) {
        let _ = self.schedule.set(access);
    }

    /// 这一轮装配给 Schedule 工具的调度能力。
    fn schedule_access(&self) -> Arc<dyn riot_protocol::schedule::ScheduleAccess> {
        self.schedule
            .get()
            .map(Arc::clone)
            .unwrap_or_else(|| Arc::new(riot_protocol::schedule::NoSchedule))
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
                if let Some(m) = &parts.meta {
                    self.created_at_ms.store(m.created_at_ms, Ordering::Relaxed);
                }
                self.restore_baselines(&parts.archived, &parts.live);
                *self.ui_archive.lock().await = parts.archived;
                if parts.live.is_empty() {
                    return;
                }
                // 环境指纹随水合恢复：上一个进程注入的快照还躺在活历史里，
                // 对模型仍然有效（契约：没有新快照就是没变）。指纹必须指向
                // 它 —— 否则重启后环境变空的那一轮会被"首轮安静跳过"吞掉，
                // 模型拿着昨天的快照当现状（截图里那次 linux.do 翻车）。
                *self.env_seen.lock().await = crate::env::last_snapshot_text(&parts.live);
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

    /// 开/关多任务模式。宿主每轮现设（和 mode 同一条规矩）。
    ///
    /// 边沿才有动作：开 → 下一轮注完整准则；关 → 下一轮说一声退出。
    /// 同值重设什么都不动，否则每轮都会重注一遍完整版。
    pub fn set_multitask(&self, on: bool) {
        let was = self.multitask.swap(on, Ordering::Relaxed);
        if on && !was {
            self.multitask_announced.store(false, Ordering::Relaxed);
            self.multitask_exit_pending.store(false, Ordering::Relaxed);
        } else if !on && was {
            self.multitask_exit_pending.store(true, Ordering::Relaxed);
        }
    }

    pub fn multitask(&self) -> bool {
        self.multitask.load(Ordering::Relaxed)
    }

    /// 历史被动过（压缩、回退、撤回）：完整准则可能不在了，下一轮重注。
    fn forget_multitask_announce(&self) {
        self.multitask_announced.store(false, Ordering::Relaxed);
    }

    /// 这一轮该附哪种多任务提醒。跟在用户正文之后，和规划模式提醒同位。
    ///
    /// 有副作用（翻 announced / exit_pending 的状态），每轮只能调一次。
    fn multitask_note(&self) -> Option<UserContent> {
        use crate::prompt::{MultitaskNote, multitask_reminder};
        if self.multitask.load(Ordering::Relaxed) {
            let announced = self.multitask_announced.swap(true, Ordering::Relaxed);
            Some(multitask_reminder(if announced {
                MultitaskNote::Short
            } else {
                MultitaskNote::Full
            }))
        } else if self.multitask_exit_pending.swap(false, Ordering::Relaxed) {
            Some(multitask_reminder(MultitaskNote::Exit))
        } else {
            None
        }
    }

    /// 界面上的按钮（转到后台 / 并行构建）→ 一条带外提醒塞进当前轮。
    ///
    /// 走队列的**带外**那一半（[`QueuedKind::Nudge`]，面板看不见），内核在
    /// 下一个安全点注入 —— 正是"这一批工具结果全部就位、模型还没开口"
    /// 的那一刻，所以最迟一次工具调用之后就生效，不必等整轮跑完。没有轮
    /// 在跑返回 false：按钮的语义是对正在进行的工作说话，闲着时没有对象。
    ///
    /// 「并行构建」顺手把会话切进多任务模式（Cursor 同款：点它就算进入
    /// Multitask）；宿主那边的开关由前端同步，下一轮 TurnConfig 传回来
    /// 是同一个值。
    pub async fn nudge(&self, nudge: riot_protocol::Nudge) -> bool {
        use riot_protocol::Nudge;
        let g = self.running.lock().await;
        if g.is_none() {
            return false;
        }
        let content = match nudge {
            Nudge::StartMultitasking => crate::prompt::nudge_start_multitasking(),
            Nudge::BuildInParallel => {
                self.set_multitask(true);
                // 完整准则跟着这条一起进去（并行构建的提醒引用了它）。
                self.multitask_announced.store(true, Ordering::Relaxed);
                crate::prompt::nudge_build_in_parallel()
            }
        };
        let mut msg_content = Vec::new();
        if matches!(nudge, Nudge::BuildInParallel) {
            msg_content.push(crate::prompt::multitask_reminder(
                crate::prompt::MultitaskNote::Full,
            ));
        }
        msg_content.push(content);
        let id = self.ids.next_id("msg");
        self.queue.push(QueuedEntry {
            id: id.clone(),
            kind: QueuedKind::Nudge,
            msg: Message::User {
                id: MessageId::from_raw(id),
                content: msg_content,
                meta: MessageMeta {
                    synthetic: true,
                    ..Default::default()
                },
            },
        });
        tracing::info!(session = %self.id.as_str(), ?nudge, "界面提醒已排队，下一批工具结果就位时注入");
        true
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

    /// 关会话 / 退应用时取消本轮，**连同全部后台子 agent**。
    ///
    /// 和 [`Self::interrupt`] 的差别：一，不算用户按停止 —— 这条路上不撤回
    /// 任何已经发出的消息，用户下次打开必须还看得见自己说过什么；二，后台
    /// 子 agent 一起停。用户按停止只停前台（"把重活移出前台"就是这个意思，
    /// 后台任务有自己的停止键）；关会话则什么都不该留下。此后到达的完成
    /// 通知丢弃（见 `closing`）。
    pub async fn abort_turn(&self) -> bool {
        self.closing.store(true, Ordering::Relaxed);
        self.tasks.cancel_all();
        self.cancel_turn(false).await
    }

    /// 停掉一个后台子 agent（面板上的停止键）。false = 没有这个任务或它
    /// 已经结束。
    pub fn cancel_task(&self, agent_id: &riot_protocol::id::AgentId) -> bool {
        self.tasks.cancel(agent_id)
    }

    /// 后台任务快照，随 session.resume 回给界面。
    pub fn tasks_snapshot(&self) -> Vec<riot_protocol::task::BackgroundTaskView> {
        self.tasks.snapshot()
    }

    /// 一个后台子 agent 跑完了，把通知送进对话。
    ///
    /// 三种去处，在 `running` 锁下判定（和 [`Self::submit`] 同一条约束：
    /// 判定和置位不在同一次锁里，两边就会各自以为对方在管）：
    /// - 有轮在跑 → 进插话队列，内核在安全点注入，模型这一轮就能看到；
    /// - 空闲且跑过至少一轮 → 用上一轮的配置**唤起新的一轮**，模型被
    ///   叫醒来处理它 —— 这就是"委派完结束回合、完成即通知"的后半段；
    /// - 空闲但从没跑过（理论上到不了：后台任务只能由某一轮开出来）或
    ///   会话在关 → 攒进 `pending_notices` / 丢弃。
    pub async fn deliver_task_notice(self: &Arc<Self>, notice: Message) {
        if self.closing.load(Ordering::Relaxed) {
            tracing::info!(session = %self.id.as_str(), "会话正在关闭，丢弃后台任务通知");
            return;
        }
        let cancel = CancellationToken::new();
        let last = {
            let mut g = self.running.lock().await;
            if g.is_some() {
                self.queue.push(QueuedEntry {
                    id: notice.id().as_str().to_owned(),
                    kind: QueuedKind::TaskNotice,
                    msg: notice,
                });
                tracing::info!(session = %self.id.as_str(), "后台任务通知已排队，安全点注入");
                return;
            }
            let Some(last) = self.last_turn.lock().await.clone() else {
                tracing::warn!(session = %self.id.as_str(), "没有可沿用的轮次配置，通知先攒着");
                self.pending_notices
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(notice);
                return;
            };
            *g = Some(cancel.clone());
            last
        };
        tracing::info!(session = %self.id.as_str(), "后台任务通知唤起新的一轮");
        let this = Arc::clone(self);
        let sink = self.sink();
        tokio::spawn(async move {
            if let Err(e) = this
                .run_locked(
                    TurnStart::Notices(vec![notice]),
                    last.model,
                    last.caps,
                    sink.clone(),
                    cancel,
                    last.limits,
                )
                .await
            {
                tracing::error!(error = %e, "唤醒轮失败");
                let _ = sink.send(AgentEvent::Done {
                    reason: riot_protocol::event::TerminalReason::Error {
                        error: riot_protocol::event::AgentError::Internal { message: e },
                    },
                });
            }
        });
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
    /// 有轮在跑就入队并返回**条目 id** —— 内核在当前任务跑完时注入
    /// （Cursor 语义，不夹进工具轮之间），事件流把它当普通消息推回来，
    /// 消息的 id 就是这个条目 id，前端靠它把排队面板的条目转成对话气泡。
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
                        kind: QueuedKind::Interjection(input),
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
                .run_locked(
                    TurnStart::User(input),
                    model,
                    caps,
                    sink.clone(),
                    cancel,
                    limits,
                )
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
    ///
    /// 标题在摘录头部和 INDEX 里都有，改了要跟上 —— 这是唯一一个不在
    /// `running` 保护下的触发点，写入器的"锁里取快照"就是为它准备的。
    pub async fn set_title(&self, title: Option<String>) {
        *self.custom_title.lock().await = title.filter(|t| !t.trim().is_empty());
        self.refresh_digest().await;
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
        self: &Arc<Self>,
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
        self.run_locked(TurnStart::User(input), model, caps, sink, cancel, limits)
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
        self.spawn_rerun(model, caps, sink, cancel, limits);
        Ok(())
    }

    /// 编辑一条用户提问并从它重新开始（Cursor 编辑气泡后发送的同款语义）：
    /// 替换文本、丢掉它之后的一切、再从它跑一轮。
    ///
    /// 和 [`Self::regenerate`] 是同一条路，只差"截到哪、改不改字"：重新
    /// 生成截到助手消息前面那条提问、不改字；这里截到被编辑的提问本身、
    /// 换掉它的文字。附件（图片、引用）原位保留 —— 用户改的是话，不是图。
    /// 忙着的时候拒绝，理由同重新生成。
    pub async fn resend_from(
        self: &Arc<Self>,
        message_id: &str,
        text: &str,
        model: riot_protocol::ModelEndpoint,
        caps: TurnCapabilities,
        sink: SessionSink,
        limits: TurnLimits,
    ) -> Result<(), String> {
        if text.trim().is_empty() {
            return Err("内容不能为空。想去掉这条消息的话，用删除。".into());
        }
        let cancel = CancellationToken::new();
        {
            let mut g = self.running.lock().await;
            if g.is_some() {
                return Err("正在跑一轮，等它结束再重新发送。".into());
            }
            *g = Some(cancel.clone());
        }
        if let Err(e) = self.truncate_to_edited_prompt(message_id, text).await {
            *self.running.lock().await = None;
            return Err(e);
        }
        self.spawn_rerun(model, caps, sink, cancel, limits);
        Ok(())
    }

    /// 历史已经以提问结尾，起一轮把它重新跑掉。regenerate / resend 共用。
    fn spawn_rerun(
        self: &Arc<Self>,
        model: riot_protocol::ModelEndpoint,
        caps: TurnCapabilities,
        sink: SessionSink,
        cancel: CancellationToken,
        limits: TurnLimits,
    ) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(e) = this
                .run_locked(
                    TurnStart::Regenerate,
                    model,
                    caps,
                    sink.clone(),
                    cancel,
                    limits,
                )
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
    }

    /// 截断活历史到指定用户提问（含）并换掉它的文字；transcript 记一条
    /// 截断加一条编辑，重启重放出来和内存一致。
    ///
    /// 只对活历史生效，理由同 [`Self::edit_message`]：归档里的消息模型已经
    /// 看不见，从那里"重新开始"会让活历史变空。
    pub async fn truncate_to_edited_prompt(
        &self,
        message_id: &str,
        text: &str,
    ) -> Result<(), String> {
        self.hydrate().await;
        let mut live = self.history.lock().await;
        let Some(at) = live.iter().position(|m| m.id().as_str() == message_id) else {
            drop(live);
            return Err(self.missing_message_error(message_id).await);
        };
        if !live[at].is_user_prompt() {
            return Err("只能从用户消息重新发送。改回复的话用「编辑」。".into());
        }
        live.truncate(at + 1);
        if !live[at].edit_text(text) {
            return Err("这条消息没有可编辑的文本。".into());
        }
        let env_seen = crate::env::last_snapshot_text(&live);
        drop(live);

        if let Some(p) = &self.persist {
            // 顺序即重放顺序：先截到这条（含），再把它的文字换掉。
            p.log.append_rewind(message_id);
            p.log.append_edit(message_id, text);
        }
        self.after_history_cut(env_seen).await;
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
        // 环境指纹恢复成"截断后模型还看得见的最后一份快照"（不变量同
        // hydrate）。往两个方向都不能错：截掉的历史带走了快照而指纹还记着
        // "已发过"，下一轮差分判定"没变化"，模型对着被截的上下文失明；
        // 反过来简单归零的话，留下的历史里若还有旧快照、下一轮环境恰好
        // 变空，"首轮安静跳过"会让旧快照被「没有新快照 = 没变」反向背书。
        let env_seen = crate::env::last_snapshot_text(&live);
        drop(live);
        drop(archived);

        if let Some(p) = &self.persist {
            p.log.append_rewind(&keep_id);
        }
        self.after_history_cut(env_seen).await;
        Ok(keep_id)
    }

    /// 历史被截短之后的善后（重新生成、编辑后重发共用）：半截流清空、
    /// 排队插话作废、挂着的询问撤掉、环境指纹与档位归位、多任务完整
    /// 准则下轮重注、后台预压缩作废。
    ///
    /// `env_seen` 是截断后模型还看得见的最后一份快照（不变量同 hydrate）。
    async fn after_history_cut(&self, env_seen: Option<String>) {
        *self.live_stream.lock().await = LiveStream::default();
        let _ = self.queue.take_all();
        self.pending_asks.clear().await;
        *self.env_seen.lock().await = env_seen;
        *self.env_band.lock().await = 0;
        self.forget_multitask_announce();
        self.drop_precompact().await;
    }

    /// 上下文编辑：把一条活历史消息的文本段替换成新文本。
    ///
    /// 只动文本（见 [`Message::edit_text`]）：思考、工具调用/结果、附件
    /// 原位保留，配对和签名都不受影响。空闲时才能做 —— 和正在写历史的
    /// 轮子并发，transcript 的追加顺序和界面都会打架。
    ///
    /// 只对活历史生效。归档（压缩前）的消息模型已经看不见，改它对上下文
    /// 没有任何效果 —— 与其静默假装成功，不如把这层告诉用户。
    pub async fn edit_message(&self, message_id: &str, text: &str) -> Result<(), String> {
        if text.trim().is_empty() {
            return Err("内容不能为空。想去掉这条消息的话，用删除。".into());
        }
        self.with_idle_lock(async {
            let mut live = self.history.lock().await;
            let Some(msg) = live.iter_mut().find(|m| m.id().as_str() == message_id) else {
                drop(live);
                return Err(self.missing_message_error(message_id).await);
            };
            if !msg.edit_text(text) {
                return Err("这条是系统提示，没有可编辑的文本。".into());
            }
            drop(live);
            if let Some(p) = &self.persist {
                p.log.append_edit(message_id, text);
            }
            // 原地编辑不改条数和末条，指纹抓不住 —— 这里必须显式作废。
            self.drop_precompact().await;
            self.refresh_digest().await;
            Ok(())
        })
        .await
    }

    /// 上下文删除：按"轮"成对删 —— 从这条消息所属的用户提问起，到
    /// 下一条提问之前，整段移除（提问、工具调用、工具结果、回复一起走）。
    ///
    /// 成对是结构的要求，不只是产品选择：只删提问会让前后两段回复贴在
    /// 一起（Anthropic 拒绝的形状），只删回复会留下悬空的工具配对。
    /// 按轮删两个坑都不存在 —— 区间以提问开头、结束在下一条提问前，
    /// 工具配对总在轮内。
    ///
    /// 例外自动成立：提问发出后模型没来得及回应（被停止/出错），这一轮
    /// 只有提问自己，删除也就只删它。
    pub async fn delete_message(&self, message_id: &str) -> Result<(), String> {
        self.with_idle_lock(async {
            let mut live = self.history.lock().await;
            let Some(at) = live.iter().position(|m| m.id().as_str() == message_id) else {
                drop(live);
                return Err(self.missing_message_error(message_id).await);
            };
            if matches!(live[at], Message::System { .. }) {
                return Err("这条是系统提示，不支持删除。".into());
            }
            // 轮的起点：目标自己是提问就是它，否则向前找最近的提问；
            // 找不到（历史以回应开头的病态形状）就从目标本身删起。
            // 起点和目标之间不会有别的提问（"最近"保证了这一点），
            // 区间因此恰好罩住目标所在的这一轮。
            let start = (0..=at)
                .rev()
                .find(|&i| live[i].is_user_prompt())
                .unwrap_or(at);
            let end = (at + 1..live.len())
                .find(|&i| live[i].is_user_prompt())
                .unwrap_or(live.len());
            let removed: Vec<String> = live
                .drain(start..end)
                .map(|m| m.id().as_str().to_owned())
                .collect();
            // 环境指纹恢复成剩余历史里的最后一份快照，理由同 rewind_to_prompt
            //（不变量同 hydrate：指纹 = 模型还看得见的那份，两个方向都不能错）。
            let env_seen = crate::env::last_snapshot_text(&live);
            drop(live);
            if let Some(p) = &self.persist {
                for id in &removed {
                    p.log.append_delete(id);
                }
            }
            *self.env_seen.lock().await = env_seen;
            *self.env_band.lock().await = 0;
            self.forget_multitask_announce();
            self.drop_precompact().await;
            self.refresh_digest().await;
            Ok(())
        })
        .await
    }

    /// 占住 `running` 跑一段短操作（上下文编辑/删除）。
    ///
    /// 和 [`Self::compact_now`] 同一个理由：改写历史不能和跑动中的轮子
    /// 并发。期间到达的插话照常排队，下一轮的收尾 drain 会捞到。
    async fn with_idle_lock<T>(
        &self,
        op: impl Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        {
            let mut g = self.running.lock().await;
            if g.is_some() {
                return Err("正在跑一轮，等它结束再修改上下文。".into());
            }
            *g = Some(CancellationToken::new());
        }
        self.hydrate().await;
        let result = op.await;
        *self.running.lock().await = None;
        result
    }

    /// 编辑/删除的目标不在活历史里时，说清它到底去了哪。
    async fn missing_message_error(&self, message_id: &str) -> String {
        if self
            .ui_archive
            .lock()
            .await
            .iter()
            .any(|m| m.id().as_str() == message_id)
        {
            "这条消息已被压缩进摘要，模型看的是摘要 —— 改它不会影响上下文。".into()
        } else {
            "这条消息已经不在当前上下文里。".into()
        }
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
    async fn finalize_partial(&self, model: &str, now_ms: u64) -> Option<Message> {
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
                // 时刻由调用方给：这条消息不走事件循环那段打戳逻辑
                //（它是本地合成的），而会话本身没有时钟。
                created_at_ms: Some(now_ms),
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
        // 环境指纹恢复成剩余历史里的最后一份快照，理由同 rewind_to_prompt
        //（被撤的那条用户消息正是捎带轮首快照的载体，撤回几乎必然动指纹）。
        let env_seen = crate::env::last_snapshot_text(&live);
        drop(live);

        if let Some(p) = &self.persist {
            p.log.append_withdraw(id.as_str());
        }
        *self.env_seen.lock().await = env_seen;
        *self.env_band.lock().await = 0;
        self.forget_multitask_announce();
        self.drop_precompact().await;
        Some(empty)
    }

    /// 跑一轮的主体。调用方必须已经把 `running` 置成本轮的令牌 ——
    /// 这里负责跑完、清 `running`、清残留插话。
    ///
    /// 起点见 [`TurnStart`]。
    async fn run_locked(
        self: &Arc<Self>,
        input: TurnStart,
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

        // `[约束]` 下面的清理必须无条件发生，`run_inner` panic 也不例外。
        //
        // 整轮跑在 `submit` 的 `tokio::spawn` 里，而 panic 是 unwind ——
        // 它只杀掉这个 task。清理被跳过的后果不是"这一轮失败"，而是
        // **会话永久卡死**:`running` 永远是 `Some`，`Done` 永远不发，
        // 此后发消息、重新生成、`/compact`、上下文编辑全被"正在跑一轮"
        // 拒掉，界面一直转圈；`interrupt()` 还会返回 true（令牌还在），
        // 让前端以为中断成功了。
        //
        // 用 catch_unwind 而不是 Drop guard:清理要 await 两把锁，
        // 而 Drop 里没法 await。
        let inner = std::panic::AssertUnwindSafe(self.run_inner(
            input,
            model,
            caps,
            sink.clone(),
            cancel,
            limits,
        ));
        let result = match futures::FutureExt::catch_unwind(inner).await {
            Ok(r) => r,
            Err(_) => {
                tracing::error!("轮次 panic，已收束成一次失败");
                Err("内部错误，这一轮没有完成。可以重试，或者换一种说法。".to_owned())
            }
        };

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
        // 三种残留三种处置：
        // - 用户插话由前端面板接管（中断后自动续、出错后停下等用户）；
        // - 后台任务的通知前端看不见，攒回来等下一轮开工时注入 —— 不在
        //   这里立刻唤起新的一轮：用户刚按了停止，多半是要改说法，这时候
        //   冒出一轮"收到通知"的对话是在和他抢话；
        // - 界面按钮的提醒**作废**。「转到后台」说的是"你手上这件事挪到
        //   后台去"，而这件事已经收场了 —— 留到下一轮，用户下次随便问句
        //   什么都会被无端分叉到后台。
        let mut notices = Vec::new();
        let (mut interjections, mut nudges) = (0usize, 0usize);
        for e in leftover {
            match e.kind {
                QueuedKind::TaskNotice => notices.push(e.msg),
                QueuedKind::Interjection(_) => interjections += 1,
                QueuedKind::Nudge => nudges += 1,
            }
        }
        if !notices.is_empty() {
            tracing::info!(
                count = notices.len(),
                "没赶上安全点的后台任务通知，留到下一轮"
            );
            self.pending_notices
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend(notices);
        }
        if interjections > 0 {
            tracing::debug!(count = interjections, "清掉没赶上收尾的插话，前端面板接管");
        }
        if nudges > 0 {
            tracing::info!(count = nudges, "轮次已收场，界面提醒作废");
        }
        // 一轮的历史定下来了，摘录跟上。放在 running 释放之后：写摘录要
        // 拿历史锁，不该让"这一轮结束了"的判定多等一次磁盘。
        self.refresh_digest().await;
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
        if let Some(note) = self.session_id_note() {
            out.push(UserContent::Attachment(note));
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

    /// 「你是哪个会话」—— 历史会话回忆开着时，首条消息（和压缩后的续接
    /// 消息）里带一句会话 id。
    ///
    /// 走消息侧而不是 system prompt：id 逐会话不同，写进 system prompt 会让
    /// 同一项目的不同会话连项目块的缓存都共享不了。模型要它做两件事：
    /// 翻 INDEX 时认出自己、引用别的会话时不把自己也引一遍。
    fn session_id_note(&self) -> Option<Attachment> {
        self.digests_dir()?;
        Some(Attachment::SystemReminder {
            text: format!(
                "This conversation is session `{}`. Its digest in the past-sessions directory \
                 is just what you already have in context — skip it when you look through \
                 earlier conversations.",
                self.id.as_str()
            ),
        })
    }

    /// 环境感知的轮首注入（docs/ENV_DESIGN.md §3）：时钟行 + 间隔警示 +
    /// 快照差分 + 告警 + 档位。
    ///
    /// 时钟行每轮必发：wire 格式里消息不带时间戳，模型对「这轮离上轮隔了
    /// 多久」零感知（真实翻过车：拿上午的浏览器快照回答下午的提问）。轮首
    /// 追加只出现在新消息里，缓存前缀（system + 工具 + 旧历史）不动，
    /// 十几个 token 买断整类时间盲。间隔超过阈值再补一句显式警示。
    ///
    /// 其余内容各自独立：快照全文只在渲染结果和上次指纹不同时注入（零变化
    /// = 只有时钟行）；档位行只在向上越档时说一次；告警由宿主去重，来了就注。
    /// 采样失败（宿主没装配 / 传输断了）不挡轮次，但**不再沉默**：手里有
    /// 指纹就宣告旧快照作废，然后清掉指纹 ——「没有新快照就是没变」的契约
    /// 会把沉默反向背书成"一切照旧"，而清指纹让恢复采样的那一轮必然全量重发。
    async fn env_prelude(
        &self,
        now_ms: u64,
        tz_offset_minutes: i32,
        last_msg_ms: Option<u64>,
        history_tokens: u32,
        compact_threshold: u32,
    ) -> Vec<UserContent> {
        let mut status = vec![crate::env::clock_line(now_ms, tz_offset_minutes)];
        if let Some(line) = last_msg_ms.and_then(|t| crate::env::gap_line(now_ms.saturating_sub(t)))
        {
            status.push(line);
        }

        let mut snapshot = None;
        let mut alerts = Vec::new();
        match self.env_probe().sample().await {
            Some(snap) => {
                let text = crate::env::render(&snap);
                let mut seen = self.env_seen.lock().await;
                // 首轮对着空环境不说话：对着空房间描述空房间是噪音。
                // 记下指纹 —— 之后第一个终端出现时，差分自然触发。
                let skip = seen.is_none() && snap.is_quiet();
                let changed = seen.as_deref() != Some(text.as_str());
                *seen = Some(text.clone());
                if changed && !skip {
                    snapshot = Some(text);
                }
                alerts = snap.alerts;
            }
            None => {
                if self.env_seen.lock().await.take().is_some() {
                    status.push(crate::env::STALE_NOTICE.to_owned());
                }
            }
        }

        // 自我状态档位（P3）。阈值为 0 说明配置坏了，跳过而不是除零。
        if compact_threshold > 0 {
            let pct = ((u64::from(history_tokens) * 100) / u64::from(compact_threshold)) as u32;
            let band = crate::env::usage_band(pct);
            let mut prev = self.env_band.lock().await;
            if band > *prev {
                *prev = band;
                status.push(crate::env::band_line(pct));
            }
        }

        let mut out = vec![UserContent::Attachment(Attachment::Environment {
            text: status.join("\n"),
        })];
        // 快照独立成条，内容逐字节等于指纹 —— 水合恢复（hydrate）靠这一点
        // 从 transcript 里原样捞回指纹，别把档位线之类的拼进来。
        if let Some(text) = snapshot {
            out.push(UserContent::Attachment(Attachment::Environment { text }));
        }
        // 条数上限宿主已经守了，这里再夹一次当保险。
        for a in alerts.iter().take(3) {
            out.push(UserContent::Attachment(Attachment::SystemReminder {
                text: crate::env::alert_text(a),
            }));
        }
        if out.len() > 1 {
            tracing::info!(parts = out.len(), alerts = alerts.len(), "注入环境快照");
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
        allow_read: &[String],
    ) -> Option<Arc<riot_runtime::ActiveSandbox>> {
        let policy = mode.policy(&self.cwd, allow_read);
        let mut slot = self.sandbox.lock().await;

        if let Some(cached) = slot.as_ref() {
            if cached.policy == policy {
                return Some(Arc::clone(&cached.active));
            }
            // 策略换了，放掉旧的、激活新的。
            //
            // 注意：Windows 那侧 Drop **不再**即时撤授权（holder 是内核 pid、
            // 多会话共享，会话级撤会连累并发会话 —— 见 riot_runtime 的
            // sandbox_win::WinSandbox 的 Drop）。所以换档后旧授权会留到内核
            // 退出（或下次启动 recover）才撤，这中间可写面是新旧两套之和。
            // 对单用户桌面有界、可接受；要即时收窄得引入进程级引用计数（未做）。
            *slot = None;
        }

        // `[约束]` 必须 `spawn_blocking`。`activate()` 从头到尾是同步阻塞:
        // Windows 上它起 `srt-win` 子进程、等 `acl grant` 把可继承 ACE 传播
        // 到工作区**每一个已存在的文件**（带 target/ 和 node_modules 的仓库
        // 是几十万个），macOS 上也要 spawn 一次冒烟。直接在 async 里调 = 占死
        // 一个 tokio 工作线程,而线程池是整个宿主共用的 —— 实测的表现不是
        // "第一条命令慢",是**连消息都读不出来**,因为 IPC 的 handler 也在
        // 同一个池子里排队。
        //
        // 挪到阻塞池之后,慢还是慢（那是另一回事,见 sandbox_win 的授权成本），
        // 但界面是活的:用户能看到进度、能去设置里把隔离关掉。
        let for_activate = policy.clone();
        let active = tokio::task::spawn_blocking(move || for_activate.activate())
            .await
            .unwrap_or_else(|e| {
                // JoinError 只可能是 panic 或取消。当成"这台机器上做不到"
                // 处理 —— 决策链会退回逐条询问,而不是当成有边界。
                tracing::warn!(error = %e, "沙箱激活任务没跑完,本轮不隔离");
                None
            })?;
        let active = Arc::new(active);
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

    /// 把一段历史压成摘要：LLM 总结 + 记忆/工作集重注 + 归档落盘 + 边界落盘。
    /// 总结失败返回 None（调用方决定要不要声张）。
    ///
    /// 这是**阻塞**的那条路：调用方等着总结回来。轮内开工前若有后台预压缩
    /// 的产物可用（[`Self::take_precompact`]），走的是不等模型的
    /// [`Self::finish_compaction`]；这里是没有产物、或产物作废时的兜底，
    /// 以及手动 `/compact`。
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
        let split = compaction_split(provider.as_ref(), history);
        // 先说一声再动手。下面那次总结是一个真实的模型调用，几十秒 ——
        // 期间界面上只有那三个点在动，和"模型正在回答"分不出来。
        self.compacting.store(true, Ordering::Relaxed);
        let _ = sink.send(AgentEvent::Compacting);
        let summary = riot_core::summarize::summarize_history(
            provider,
            model,
            &history[..split],
            shape,
            cancel,
        )
        .await;
        self.compacting.store(false, Ordering::Relaxed);
        match summary {
            Ok(s) => Some(self.finish_compaction(provider, history, split, &s).await),
            Err(e) => {
                tracing::warn!(error = %e, "历史总结失败");
                None
            }
        }
    }

    /// 拿着已经算好的总结把压缩落地：重注记忆/git/工作集、归档原文、
    /// 组续接消息、边界与尾巴落盘、更新界面归档。
    ///
    /// 不调模型 —— 总结来自 [`Self::compact_history`]（刚等回来的）或
    /// 后台预压缩（[`Self::take_precompact`]，早就算好的）。两条路的落地
    /// 逻辑必须是同一份，否则"预压缩换入的会话少了 AGENTS.md"这种 bug 只
    /// 在一条路上出现，另一条路的测试全绿。
    ///
    /// `split` 是总结覆盖的范围：`history[..split]` 被总结吞掉并归档，
    /// `history[split..]` 原样跟在续接消息之后（见
    /// [`riot_core::summarize::split_point`]）。
    ///
    /// `[约束]` 调用方必须持有 `running`：这里改写 transcript 和
    /// `ui_archive`，和跑动中的轮子并发会让两边都乱。
    async fn finish_compaction(
        &self,
        provider: &Arc<dyn Provider>,
        history: &[Message],
        split: usize,
        summary: &str,
    ) -> CompactOutcome {
        let (head, tail) = history.split_at(split);
        let before = provider.count_tokens(history);
        // 记忆重注：压缩把带着 AGENTS.md 的首条消息吞了，
        // 不重注的话项目约定从此消失（CC 的 postCompactCleanup 同款）。
        let mut memory: Vec<Attachment> = crate::memory::collect(&self.cwd)
            .into_iter()
            .map(|m| Attachment::Memory {
                path: m.path,
                content: m.content,
            })
            .collect();
        // 会话 id 同理：首条消息被吞了，模型不再知道自己是哪个会话。
        if let Some(note) = self.session_id_note() {
            memory.push(note);
        }
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
        // 尾巴里若有快照，它在续接消息之后、模型看得见，指纹指回它。
        *self.env_seen.lock().await = crate::env::last_snapshot_text(tail);
        *self.env_band.lock().await = 0;
        // 多任务准则的完整版多半被总结吞了，下一轮重注。
        self.forget_multitask_announce();
        // 原文去哪找：总结是有损的，报错原文、路径、用户原话要有地方可查。
        // 指的是这个会话的摘录（`crate::digest`）—— 它由调用方在换完历史
        // 之后重写（见 [`Self::refresh_digest`]），这里只给路径。
        let archive = self.digest_path();
        // 工作集重注：纯总结不够 —— 压缩后模型立即失去对文件
        // 内容的记忆，下一步就是把刚读过的文件再读一遍。
        let restored = restored_files(self.file_state.as_ref());
        let msg = riot_core::summarize::continuation_message(
            summary,
            memory,
            restored,
            archive.as_deref(),
            MessageId::from_raw(self.ids.next_id("msg")),
        );
        // 尾巴里 assistant 的 usage 必须抹掉。那个数描述的是压缩前那次请求
        // 的整个上下文（几十万），而 `count_tokens` 会拿历史里最后一条带
        // usage 的 assistant 打底 —— 留着它，压缩后的历史量出来还是压缩前
        // 的尺寸：`after` ≈ `before`、环境档位继续报"快满了"、下一轮开工
        // 又判定超阈值再压一次（总结的总结）。代价是界面上这一轮的费用
        // 统计少了几条，换的是压缩后每一处量尺寸的地方都对。
        let tail: Vec<Message> = tail
            .iter()
            .cloned()
            .map(|mut m| {
                m.forget_usage();
                m
            })
            .collect();
        let mut new_history = Vec::with_capacity(1 + tail.len());
        new_history.push(msg.clone());
        new_history.extend_from_slice(&tail);
        let after = provider.count_tokens(&new_history);
        // 边界必须先于续接消息落盘 —— 顺序反了，重启加载会把续接消息
        // 一起丢掉；尾巴跟在续接消息之后重新追加一遍，边界记录里的
        // keep_from 让加载器把边界前的那份从归档里摘掉（见
        // SessionLog::append_boundary）。
        if let Some(p) = &self.persist {
            p.log
                .append_boundary(before, after, tail.first().map(|m| m.id().as_str()));
            p.log.append(&msg);
            for m in &tail {
                p.log.append(m);
            }
        }
        tracing::info!(before, after, kept = tail.len(), "历史压缩完成");
        self.ui_archive.lock().await.extend(head.iter().cloned());
        CompactOutcome {
            history: new_history,
            before_tokens: before,
            after_tokens: after,
        }
    }

    /// 把主循环在 413 上做的反应式压缩落到宿主这边：内存历史、transcript、
    /// 界面归档、环境基线。见 [`RecordingCompactor`]。
    ///
    /// `compacted` 是主循环此后用的完整历史。两种形态：
    /// - 轻档（清旧工具结果）：条数不变、首条 id 还在旧历史里，只是内容
    ///   换了占位符 → 没有头，全部重写。
    /// - 重档（总结 + 尾巴）：首条是新的续接消息 → 它前面对应旧历史里
    ///   到尾巴为止的那段，归档；尾巴的首条若还在旧历史里就是 `keep_from`。
    ///
    /// 两种都用同一条 transcript 记录表达：边界（`keep_from` = 新历史里第一
    /// 条旧历史也有的消息）+ 把新历史逐条重新追加。加载器会把 `keep_from`
    /// 前的那段归档、之后的丢掉，然后读到重新追加的这份 —— 和主动压缩的
    /// 落盘方式一致（[`Self::finish_compaction`]）。
    ///
    /// `[约束]` 只能在持有 `running` 的轮内调用（事件循环里）。
    async fn absorb_reactive_compaction(
        &self,
        compacted: Vec<Message>,
        before_tokens: u32,
        after_tokens: u32,
    ) {
        let mut history = self.history.lock().await;
        // 新历史里第一条"旧历史也有"的消息：它之前的旧消息是被吞掉的头。
        let keep_at = compacted.iter().find_map(|m| {
            history
                .iter()
                .position(|h| h.id() == m.id())
                .map(|at| (at, m.id().as_str().to_owned()))
        });
        let head: Vec<Message> = match &keep_at {
            Some((at, _)) => history[..*at].to_vec(),
            None => std::mem::take(&mut *history),
        };
        *history = compacted.clone();
        drop(history);

        if let Some(p) = &self.persist {
            p.log.append_boundary(
                before_tokens,
                after_tokens,
                keep_at.as_ref().map(|(_, id)| id.as_str()),
            );
            for m in &compacted {
                p.log.append(m);
            }
        }
        if !head.is_empty() {
            // 和主动压缩同一套善后：环境指纹归零（吞掉的头里有旧快照，
            // 不归零下一轮差分判"没变化"）。
            *self.env_seen.lock().await = crate::env::last_snapshot_text(&compacted);
            *self.env_band.lock().await = 0;
            self.forget_multitask_announce();
            self.ui_archive.lock().await.extend(head);
        }
        // 续接消息（主循环的 Layered 组的）已经指着摘录路径了，文件得跟上。
        // 轻档（只清结果）也重写：摘录里的工具结果该和模型看到的一致。
        self.refresh_digest().await;
        tracing::info!(
            before = before_tokens,
            after = after_tokens,
            live = compacted.len(),
            "反应式压缩已落地到宿主历史"
        );
    }

    /// 轮刚结束时，历史已经过线的话在后台先把总结算出来。
    ///
    /// 为什么是这个时机而不是下一轮开工：
    /// - 用户不用等。开工时总结要几十秒，期间界面只有三个点在动。
    /// - 更便宜。总结请求和主循环刚发过的请求同形状、同前缀，走 provider
    ///   的前缀缓存 —— 而缓存有生命期（Anthropic 默认 5 分钟）。轮刚结束
    ///   缓存最热；等用户喝完咖啡回来再总结，~100k 的前缀全量重算。
    ///
    /// 后台任务**没有副作用**：不碰历史、不落盘、不发事件、不占 `running`
    /// （占了的话用户就发不了消息，等于没在后台）。它只产出一个字符串，
    /// 由下一轮开工时 [`Self::take_precompact`] 按指纹决定用不用。界面上
    /// 也不显示"正在压缩"—— 那个状态在前端意味着等待，而此刻没人在等。
    ///
    /// 已经有一份在跑就换掉它：新的基于更新的历史，旧的必然作废。
    async fn spawn_precompact(
        &self,
        provider: &Arc<dyn Provider>,
        model: &str,
        history: Vec<Message>,
        shape: riot_core::summarize::RequestShape,
    ) {
        let split = compaction_split(provider.as_ref(), &history);
        let fingerprint = history_fingerprint(&history);
        let cancel = CancellationToken::new();
        let task = {
            let provider = Arc::clone(provider);
            let model = model.to_owned();
            let cancel = cancel.clone();
            let session = self.id.clone();
            tokio::spawn(async move {
                match riot_core::summarize::summarize_history(
                    &provider,
                    &model,
                    &history[..split],
                    Some(&shape),
                    cancel,
                )
                .await
                {
                    Ok(s) => {
                        tracing::info!(session = %session.as_str(), "后台预压缩完成");
                        Some(s)
                    }
                    Err(e) => {
                        tracing::warn!(session = %session.as_str(), error = %e, "后台预压缩失败，下一轮开工时再压");
                        None
                    }
                }
            })
        };
        if let Some(old) = self.precompact.lock().await.replace(Precompact {
            fingerprint,
            split,
            cancel,
            task,
        }) {
            old.abandon();
        }
    }

    /// 取走一份匹配当前历史的预压缩总结：`(split, summary)`。
    ///
    /// 指纹对不上（历史被编辑/删除/截断过）就作废返回 None，调用方走阻塞
    /// 路径。还没跑完就等它 —— 它已经跑了一段，剩下的比重新来短；这段
    /// 等待和阻塞压缩一样要让界面知道（`Compacting`），否则用户面对的是
    /// 无声的空白。`cancel` 是本轮的令牌：用户按停止，等待跟着断。
    async fn take_precompact(
        &self,
        history: &[Message],
        sink: &SessionSink,
        cancel: &CancellationToken,
    ) -> Option<(usize, String)> {
        let mut pc = self.precompact.lock().await.take()?;
        if pc.fingerprint != history_fingerprint(history) {
            tracing::info!("预压缩基于的历史已变，作废");
            pc.abandon();
            return None;
        }
        if !pc.task.is_finished() {
            self.compacting.store(true, Ordering::Relaxed);
            let _ = sink.send(AgentEvent::Compacting);
        }
        let split = pc.split;
        let summary = tokio::select! {
            r = &mut pc.task => r.ok().flatten(),
            _ = cancel.cancelled() => {
                pc.abandon();
                None
            }
        };
        self.compacting.store(false, Ordering::Relaxed);
        summary.map(|s| (split, s))
    }

    /// 作废后台预压缩。历史被改写的每条路径都要调：指纹能抓住条数或末条
    /// 的变化，抓不住原地编辑（见 [`history_fingerprint`]）。
    async fn drop_precompact(&self) {
        if let Some(pc) = self.precompact.lock().await.take() {
            pc.abandon();
        }
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
            // 后台已经算好一份且对得上就直接用；否则走瘦身路径现算。
            let outcome = match self.take_precompact(&history, &sink, &cancel).await {
                Some((split, summary)) => Some(
                    self.finish_compaction(&provider, &history, split, &summary)
                        .await,
                ),
                None => {
                    self.compact_history(&provider, &model.model, &history, None, &sink, cancel)
                        .await
                }
            };
            match outcome {
                Some(o) => {
                    *self.history.lock().await = o.history;
                    Ok((o.before_tokens, o.after_tokens))
                }
                None => Err("压缩失败，历史保持原样。稍后再试。".to_owned()),
            }
        };
        if result.is_ok() {
            self.refresh_digest().await;
        }
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

    /// 工具产物（截图原图、过大工具结果）的落盘目录，会话专属。
    ///
    /// 根目录来自 [`SessionPersist::artifacts_root`]（配置目录下的
    /// `artifacts/`，见 [`crate::config::artifacts_root`]）；放配置目录下而不是
    /// 工作区:截图不是项目文件，出现在用户的 git status 里就是垃圾。没有
    /// 持久化通道的会话（单元测试）落到系统临时目录，绝不能落进用户真实
    /// 的配置目录。（压缩归档以前也在这里，现在并进了会话摘录，见
    /// [`crate::digest`]。）
    ///
    /// 目录建不出来也照常返回路径 —— 工具写不进时自行降级（消息里不带
    /// 路径），链路不断。
    fn artifacts_dir(&self) -> std::path::PathBuf {
        let root = match &self.persist {
            Some(p) => p.artifacts_root.clone(),
            None => std::env::temp_dir().join("riot-artifacts"),
        };
        let dir = root.join(self.id.as_str());
        #[allow(clippy::disallowed_methods)]
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(error = %e, dir = %dir.display(), "工件目录建不出来，截图将不落盘");
        }
        dir
    }

    /// 主动压缩：历史超阈值就先总结再开工。
    ///
    /// 反应式（413 重试）是保命；这条是"到线就处理"—— 不主动的话，
    /// 会话会一直顶着窗口上限跑，每轮都在 413 的边缘反复横跳。
    /// 调用方放在追加本轮新消息**之前**：压的是旧账，新话骑在压缩后的历史上。
    ///
    /// 上一轮结束时若已在后台把总结算好（spawn_precompact），这里直接
    /// 落地，不等模型；没有或作废了才走阻塞的 compact_history。
    /// 失败不拦路：继续用完整历史，真溢出时反应式路径兜底。
    #[allow(clippy::too_many_arguments)]
    async fn proactive_compact(
        &self,
        provider: &Arc<dyn Provider>,
        model: &str,
        history: &mut Vec<Message>,
        summary_shape: &riot_core::summarize::RequestShape,
        sink: &SessionSink,
        cancel: &CancellationToken,
        limits: &TurnLimits,
    ) {
        let history_tokens = provider.count_tokens(history);
        if history.is_empty() || history_tokens < limits.compact_threshold_tokens {
            return;
        }
        let outcome = match self.take_precompact(history, sink, cancel).await {
            Some((split, summary)) => {
                tracing::info!("换入后台预压缩的总结");
                Some(
                    self.finish_compaction(provider, history, split, &summary)
                        .await,
                )
            }
            None => {
                self.compact_history(
                    provider,
                    model,
                    history,
                    Some(summary_shape),
                    sink,
                    cancel.child_token(),
                )
                .await
            }
        };
        match outcome {
            Some(o) => {
                *history = o.history;
                *self.history.lock().await = history.clone();
                // 续接消息指着摘录，文件必须在请求发出之前是新的 ——
                // 这里在换完历史之后、主循环开跑之前同步写。
                self.refresh_digest().await;
                // 轮内原地宣布：轮子接着跑，busy 本来就该保持，
                // 而且 Compacted 后紧跟 RequestStart 的顺序有回放钉着。
                let _ = sink.send(AgentEvent::Compacted {
                    before_tokens: o.before_tokens,
                    after_tokens: o.after_tokens,
                    strategy: riot_protocol::event::CompactStrategy::FullSummary,
                });
            }
            None => tracing::warn!("主动压缩失败，本轮用完整历史"),
        }
    }

    /// 子 agent transcript 的落盘处：`sessions/subagents/<会话>/`。
    /// 混进主目录会被索引重建当成会话捞回来。None = 本会话不持久化。
    fn subagent_transcripts(&self) -> Option<Arc<riot_store::Transcripts>> {
        self.persist.as_ref().map(|p| {
            Arc::new(riot_store::Transcripts::new(
                p.store.dir().join("subagents").join(self.id.as_str()),
            ))
        })
    }

    /// 自我分叉：用本轮的种子造一个和父同形的子 agent 任务。
    ///
    /// 历史取**此刻**的活历史 —— 含把它分叉出来的那条 assistant 消息；末尾
    /// 悬空的 tool_use 由 [`crate::subagent::fork_prelude`] 补齐。调度器用
    /// 和父同一条装配路（`build_scheduler`）：同一个文件状态缓存（分叉
    /// 继承了父读过什么，改动追踪也要记在同一本账上）、同一个沙箱、同一个
    /// 权限闸。
    ///
    /// `[取舍]` 浏览器和终端面板也共享。它们是会话级独占资源，分叉和父
    /// 同时操作会打架 —— 但分叉的本意是接管实质工作、父退到协调，真正
    /// 并发驾驶浏览器的情形靠 Task 的提示词约束。给分叉 NoBrowser 会让它
    /// 面对一段"刚才浏览器里看到……"的历史却没有浏览器，更糟。
    async fn fork_job(
        &self,
        agent_id: &riot_protocol::id::AgentId,
        title: &str,
        prompt: &str,
        fork_call: &riot_protocol::id::ToolUseId,
    ) -> Result<crate::subagent::Job, String> {
        if self.closing.load(Ordering::Relaxed) {
            return Err("会话正在关闭，不能分叉。".into());
        }
        let seed = self
            .fork_seed
            .lock()
            .await
            .clone()
            .ok_or("这一轮还没装配完，暂时不能分叉；稍后再试。")?;
        let last = self
            .last_turn
            .lock()
            .await
            .clone()
            .ok_or("这个会话还没跑过完整的一轮，不能分叉。")?;

        // 分叉里的 Task 要知道自己住在哪个 agent 里（它派的子 agent 挂在
        // 这个 id 下面）。占位那份换成带真 id 的，形状不变。
        let mut tools = seed.tools.clone();
        tools[seed.task_index] = Arc::new(crate::subagent::TaskTool::nested(
            seed.subagent_deps.clone(),
            agent_id.clone(),
        ));
        // 分叉总在后台跑，权限弹窗要带归属（见 subagent::Attributed）。
        // 只改 describe，工具清单的 name / schema / prompt 和父一致。
        let registry = Registry::new(crate::subagent::Attributed::wrap_all(
            tools,
            &format!("后台任务「{title}」"),
        ))
        .map(Arc::new)
        .map_err(|e| format!("分叉的工具装配失败：{e}"))?;
        let sandbox = self
            .sandbox
            .lock()
            .await
            .as_ref()
            .map(|c| Arc::clone(&c.active));
        let python_venv = self.python_venv().await;
        let clock: Arc<dyn riot_protocol::tool::Clock> =
            Arc::new(riot_providers::watchdog::TokioClock);
        let scheduler = self.build_scheduler(
            ToolAssembly {
                registry,
                prompt_ctx: seed.prompt_ctx.clone(),
                deferred: seed.deferred.clone(),
            },
            Arc::clone(&clock),
            last.caps,
            Arc::clone(&seed.gate),
            python_venv.as_deref(),
            sandbox,
        );

        self.hydrate().await;
        let mut messages = self.history.lock().await.clone();
        let logged = messages.len();
        messages.push(crate::subagent::fork_prelude(
            &messages, agent_id, fork_call, prompt,
        ));

        let log = self.subagent_transcripts().map(|t| {
            t.open(riot_store::TranscriptMeta {
                id: SessionId::from_raw(agent_id.as_str().to_owned()),
                root: self.cwd.clone(),
                created_at_ms: clock.now_ms(),
            })
        });

        Ok(crate::subagent::Job {
            agent_id: agent_id.clone(),
            kind: crate::subagent::Kind::Fork,
            title: title.to_owned(),
            provider: seed.provider,
            model: seed.model,
            system: seed.system,
            tools: Arc::new(scheduler),
            messages,
            max_turns: seed.max_turns,
            thinking: seed.thinking,
            max_output_tokens_override: seed.max_output_tokens_override,
            log,
            logged,
            // 界面从分叉说明那条看起：前面是父会话的对话，用户正对着它。
            view_from: logged,
        })
    }

    /// 一个子 agent 的会话：视图 + 界面该看的消息 + 它派的子 agent。
    /// None = 不认识这个 id。
    pub fn task_history(&self, agent_id: &str) -> Option<crate::tasks::TaskHistory> {
        self.tasks.history(agent_id)
    }

    async fn run_inner(
        self: &Arc<Self>,
        input: TurnStart,
        model: riot_protocol::ModelEndpoint,
        mut caps: TurnCapabilities,
        sink: SessionSink,
        cancel: CancellationToken,
        limits: TurnLimits,
    ) -> Result<(), String> {
        let provider = match crate::models::provider_from_endpoint(&model) {
            Ok(p) => p,
            Err(e) => {
                // 唤醒轮起不来（配置坏了、密钥没了）不能把通知吞掉：攒回去，
                // 用户下一条消息带着它进历史。
                if let TurnStart::Notices(notices) = input {
                    self.pending_notices
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .extend(notices);
                }
                return Err(e);
            }
        };
        // 这一轮的配置留一份给唤醒轮沿用（见 LastTurn）。放在 provider 建成
        // 之后、工具装配之前：建不起 provider 的配置（缺 key）不该被沿用，
        // 而装配阶段的失败和配置无关。
        *self.last_turn.lock().await = Some(LastTurn {
            model: model.clone(),
            caps: caps.clone(),
            limits: limits.clone(),
        });
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
        let sandbox = self
            .active_sandbox(limits.sandbox, &limits.sandbox_allow_read)
            .await;
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
        let subagent_deps = crate::subagent::SubagentDeps {
            provider: Arc::clone(&provider),
            model: model.model.clone(),
            cheap: caps.subagent_cheap.clone(),
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
            flavor: crate::models::flavor_for(&model),
            max_turns: limits.max_turns,
            transcripts: self.subagent_transcripts(),
            tasks: Arc::clone(&self.tasks),
            // Weak：注册表持有工具、工具持有 deps —— 抓 Arc 就是引用环。
            host: Arc::new(SessionTaskHost {
                session: Arc::downgrade(self),
            }),
        };
        // 分叉里用的那份 Task：同名同形、深度 1。这里的 parent 是占位 ——
        // 分叉的 agent id 到分叉那一刻才有，fork_job 会换成真的。
        let forked_task: Arc<dyn Tool> = Arc::new(crate::subagent::TaskTool::nested(
            subagent_deps.clone(),
            riot_protocol::id::AgentId::from_raw("fork-placeholder"),
        ));
        let task_index = tools.len();
        let fork_subagent_deps = subagent_deps.clone();
        tools.push(Arc::new(crate::subagent::TaskTool::new(subagent_deps)));
        // 定时任务。永远注册（和浏览器/终端工具同一条惯例）：没挂宿主
        // 代理时是 NoSchedule，工具明说用不了，而不是从清单里消失 ——
        // 有条件注册会让工具列表在环境之间抖动，prompt 前缀跟着变。
        tools.push(Arc::new(riot_tools::tools::schedule::ScheduleTool::new(
            self.schedule_access(),
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

        // 分叉种子要的工具清单：同一份，只把 Task 换成深度 1 的那份。
        // 在 Registry 吃掉 `tools` 之前留一份（Arc 克隆，便宜）。
        let mut fork_tools = tools.clone();
        fork_tools[task_index] = forked_task;

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
        // 分叉种子里也要 prompt_ctx 和 deferred，装配前先留一份。
        let fork_prompt_ctx = prompt_ctx.clone();
        let fork_deferred = deferred.clone();
        let fork_gate: Arc<dyn PermissionGate> = Arc::clone(&gate) as Arc<dyn PermissionGate>;
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
        let digests_dir = self.digests_dir();
        let system = crate::prompt::system_prompt(&crate::prompt::SystemPromptInput {
            cwd: &self.cwd,
            model: &model.model,
            today: &today,
            python_venv: python_venv.as_deref(),
            extra: self.system_prompt_extra().await.as_deref(),
            has_hooks: hook_engine.has_pre_tool_use()
                || hook_engine.has_post_tool_use()
                || hook_engine.has_stop(),
            digests_dir: digests_dir.as_deref(),
            flavor: crate::models::flavor_for(&model),
        });
        // specs 取轮首快照。轮中 ToolSearch 发现新工具时主循环的 tools 会变
        //（那本来就会断缓存），总结形状不跟 —— 只影响命中率，不影响正确性。
        let summary_shape = riot_core::summarize::RequestShape {
            system: system.clone(),
            tools: tools_runner.specs(),
        };

        // 分叉种子：父这一轮的请求形状。Task 收到 resume="self" 时从这里造
        // 一个同 system、同工具清单的子 agent（见 ForkSeed / fork_job）。
        let thinking = self.thinking().await;
        *self.fork_seed.lock().await = Some(ForkSeed {
            system: system.clone(),
            tools: fork_tools,
            task_index,
            subagent_deps: fork_subagent_deps,
            prompt_ctx: fork_prompt_ctx,
            deferred: fork_deferred,
            gate: fork_gate,
            provider: Arc::clone(&provider),
            model: model.model.clone(),
            max_turns: limits
                .max_turns
                .clamp(*MAX_TURNS_RANGE.start(), *MAX_TURNS_RANGE.end()),
            thinking,
            max_output_tokens_override: model.sampling.max_output_tokens,
        });

        // 反应式压缩的产物槽：主循环压完只发事件不发历史，宿主从这里取。
        let reactive_compacted: Arc<std::sync::Mutex<Option<Vec<Message>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let deps = AgentDeps {
            provider: Arc::clone(&provider),
            // 反应式（413）路径的完整阶梯：清旧工具结果 → LLM 总结。
            // 只挂 ClearOldResults 的话，"对话本身超长"的会话一溢出就死。
            compactor: Arc::new(RecordingCompactor {
                inner: Arc::new(
                    riot_core::Layered::new(
                        Arc::clone(&provider),
                        model.model.clone(),
                        summary_shape.clone(),
                        Arc::clone(&self.ids) as Arc<dyn IdGenerator>,
                        cancel.child_token(),
                    )
                    // 反应式总结的续接消息也指向摘录；文件由
                    // absorb_reactive_compaction 在 Compacted 事件到达时重写。
                    .with_archive(self.digest_path()),
                ),
                taken: Arc::clone(&reactive_compacted),
            }),
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

        // 攒着的后台任务通知（上一轮没赶上安全点、或到得太早）跟着这一轮
        // 进历史。重新生成不夹带：那一轮是"把上一个回答重来"，历史已经截到
        // 提问，塞通知进去会改变被重来的那个问题 —— 留到再下一轮。
        let pending_notices = match &input {
            TurnStart::Regenerate => Vec::new(),
            _ => std::mem::take(
                &mut *self
                    .pending_notices
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()),
            ),
        };
        // 通知进历史的方式三处一样（攒着的、本轮的），提出来免得写三遍。
        let push_notice = |history: &mut Vec<Message>, mut m: Message, now_ms: u64| {
            m.stamp(now_ms);
            if let Some(p) = &self.persist {
                p.log.append(&m);
            }
            history.push(m);
        };

        match input {
            TurnStart::Regenerate => {
                if history.is_empty() {
                    return Err("没有可重新生成的用户消息".into());
                }
            }
            TurnStart::Notices(notices) => {
                self.proactive_compact(
                    &provider,
                    &model.model,
                    &mut history,
                    &summary_shape,
                    &sink,
                    &cancel,
                    &limits,
                )
                .await;
                let now = clock.now_ms();
                let mut all: Vec<Message> = pending_notices.into_iter().chain(notices).collect();
                // 规划模式的约束跟在最后一条通知末尾，和用户消息同一个位置逻辑。
                if let Some(Message::User { content, .. }) = all.last_mut() {
                    content.extend(crate::prompt::plan_mode_reminder(mode));
                    // 唤醒轮也要记得自己是协调者：被通知叫醒后接着综合、
                    // 启动下一批，而不是顺手自己干起来。
                    content.extend(self.multitask_note());
                }
                for m in all {
                    push_notice(&mut history, m, now);
                }
            }
            TurnStart::User(input) => {
                // 这条消息的 id 先定下来：占位版和定稿版用同一个，前端认 id。
                let user_id = MessageId::from_raw(self.ids.next_id("msg"));
                // 时刻也在这里定下来，占位版和定稿版共用。定稿要等主动压缩、
                // 图片转述、`@` 展开跑完，慢的时候十几秒 —— 各取各的时钟，
                // 界面上同一条消息会在定稿那一刻跳掉一分钟。
                let sent_at_ms = clock.now_ms();
                // 占位先立起来 —— 底下压缩和转述都是模型调用，这段时间里切走
                // 再切回来必须还看得见自己刚发的话（见 `pending_user`）。
                *self.pending_user.lock().await = Some(Message::User {
                    id: user_id.clone(),
                    content: crate::content::pending_user_content(&input),
                    meta: MessageMeta {
                        created_at_ms: Some(sent_at_ms),
                        ..Default::default()
                    },
                });

                self.proactive_compact(
                    &provider,
                    &model.model,
                    &mut history,
                    &summary_shape,
                    &sink,
                    &cancel,
                    &limits,
                )
                .await;
                // 攒着的通知排在用户这句话**前面**：它们确实先发生。
                for m in pending_notices {
                    push_notice(&mut history, m, sent_at_ms);
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
                // 间隔的参照是最后一条带时间戳的消息；老 transcript 可能一条
                // 都没有（created_at_ms 晚于它们），那就不编间隔，只报时刻。
                let last_msg_ms = history.iter().rev().find_map(|m| match m {
                    Message::User { meta, .. } | Message::Assistant { meta, .. } => {
                        meta.created_at_ms
                    }
                    Message::System { .. } => None,
                });
                prelude.extend(
                    self.env_prelude(
                        sent_at_ms,
                        clock.tz_offset_minutes(),
                        last_msg_ms,
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
                // 多任务模式的准则同位（见 prompt::multitask_reminder）。
                content.extend(self.multitask_note());
                let user_msg = Message::User {
                    id: user_id.clone(),
                    content,
                    meta: MessageMeta {
                        created_at_ms: Some(sent_at_ms),
                        ..Default::default()
                    },
                };
                // 边产生边追加（两家共识）：轮次结束才写盘的话，中途崩溃丢的是
                // 整轮对话；这里丢的最多是后台通道里还没落盘的几条。
                if let Some(p) = &self.persist {
                    p.log.append(&user_msg);
                }
                history.push(user_msg);
                submitted = Some(user_id);
            }
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
        while let Some(mut ev) = stream.next().await {
            // 打戳打在这里：内核是消息流上唯一一个既有注入时钟、又能看到
            // **全部**消息（模型产出、合成、错误）的位置。往下走它同时进
            // 前端、内存历史和磁盘，三边拿到的是同一个数。
            if let AgentEvent::Message(m) = &mut ev {
                m.stamp(clock.now_ms());
            }
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
                if let Some(m) = self.finalize_partial(&model.model, clock.now_ms()).await {
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
            if let AgentEvent::Compacted {
                before_tokens,
                after_tokens,
                ..
            } = &ev
            {
                self.compacting.store(false, Ordering::Relaxed);
                // 主循环压缩后的历史只在它自己的 state 里，宿主这边必须跟上，
                // 否则本轮后续消息叠在压缩前的全量上、下一轮再溢出一次。
                let taken = reactive_compacted
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take();
                if let Some(compacted) = taken {
                    self.absorb_reactive_compaction(compacted, *before_tokens, *after_tokens)
                        .await;
                }
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

        // ── 后台预压缩：这一轮结束后历史已经过线，趁缓存还热先把总结算好 ──
        // 被取消的轮次不做：用户按了停止多半是要改说法，或者应用在退出。
        // 阈值和开工时的判定同一个 —— 这里算好的正是下一轮开工时要等的那份。
        if !cancel.is_cancelled() {
            let history = self.history.lock().await.clone();
            if !history.is_empty()
                && provider.count_tokens(&history) >= limits.compact_threshold_tokens
            {
                self.spawn_precompact(&provider, &model.model, history, summary_shape)
                    .await;
            }
        }

        Ok(())
    }
}

/// Task 工具通向会话的那条 Weak 引用（见 [`crate::subagent::TaskHost`]）。
///
/// 会话没了（被删、内核在关）就什么都不做：后台子 agent 的收尾撞上一个
/// 已经不存在的会话，正确的反应是安静地丢弃，而不是把结果塞进别处。
struct SessionTaskHost {
    session: std::sync::Weak<Session>,
}

#[async_trait::async_trait]
impl crate::subagent::TaskHost for SessionTaskHost {
    async fn deliver(&self, notice: Message) {
        match self.session.upgrade() {
            Some(s) => s.deliver_task_notice(notice).await,
            None => tracing::info!("会话已不在，丢弃后台任务通知"),
        }
    }

    async fn fork_job(
        &self,
        agent_id: &riot_protocol::id::AgentId,
        title: &str,
        prompt: &str,
        fork_call: &riot_protocol::id::ToolUseId,
    ) -> Result<crate::subagent::Job, String> {
        match self.session.upgrade() {
            Some(s) => s.fork_job(agent_id, title, prompt, fork_call).await,
            None => Err("会话已不在，不能分叉。".into()),
        }
    }
}

/// 压缩的切分点（见 [`riot_core::summarize::split_point`]），预算取
/// [`riot_core::summarize::MAX_TAIL_TOKENS`]。提出来是为了让阻塞压缩和后台
/// 预压缩调的是**同一个**函数、同一个预算 —— 两处各写一遍，某天一处改了
/// 预算，预压缩的总结覆盖的范围就和换入时假定的不一样。
///
/// `[约束]` 量尾巴必须用 [`Provider::estimate_tokens_of`]，不能用
/// `count_tokens`。后者拿切片里最后一条 assistant 的 usage 打底，而那个数
/// 是它那次请求时**整个上下文**的大小（几十万）—— 尾巴永远"超预算"，
/// `split_point` 永远返回 `len`，最近一轮次次被总结吞掉。测试替身没有
/// usage 所以看不出来，线上就是"压缩完模型立刻失忆"。
fn compaction_split(provider: &dyn Provider, history: &[Message]) -> usize {
    riot_core::summarize::split_point(
        history,
        |m| provider.estimate_tokens_of(m),
        riot_core::summarize::MAX_TAIL_TOKENS,
    )
}

/// 反应式压缩的产物截留器。
///
/// 主循环在 413 上调 [`riot_protocol::compact::Compactor`] 改写自己的
/// `state.messages`，然后只 yield 一个 `Compacted` 事件 —— 事件里没有新
/// 历史。宿主若只翻个标志，内存历史和 transcript 仍是压缩前的全量：本轮
/// 后续消息追加在全量之上，下一轮开工再把全量发出去 → 再 413 → 再花一次
/// 总结。界面上划了"已压缩"的线，实际上什么都没变。
///
/// 这层包装把 `Compacted` 携带的新历史放进槽位，宿主在收到事件时取走并
/// 落地（[`Session::absorb_reactive_compaction`]）。事件紧跟在 `compact`
/// 返回之后 yield，槽位一定先于事件被填上。
struct RecordingCompactor {
    inner: Arc<dyn riot_protocol::compact::Compactor>,
    taken: Arc<std::sync::Mutex<Option<Vec<Message>>>>,
}

#[async_trait::async_trait]
impl riot_protocol::compact::Compactor for RecordingCompactor {
    async fn compact(
        &self,
        messages: Vec<Message>,
        budget: riot_protocol::compact::CompactBudget,
    ) -> riot_protocol::compact::CompactResult {
        let r = self.inner.compact(messages, budget).await;
        if let riot_protocol::compact::CompactResult::Compacted { messages, .. } = &r {
            *self.taken.lock().unwrap_or_else(|e| e.into_inner()) = Some(messages.clone());
        }
        r
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
            spec.env
                .iter()
                .any(|(k, v)| k == "VIRTUAL_ENV" && v == "/proj/.venv"),
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

    /// 跑一次竞速，只关心"分类器放没放行"。`_tx` 要留着 —— 提前 drop 的话
    /// rx 立刻出错返回，会被当成通道已断，测的就不是判危了。
    async fn race(
        gate: &HostGate,
        tool: &dyn Tool,
        input: &serde_json::Value,
        reason: &DecisionReason,
    ) -> Option<DecisionReason> {
        let (_tx, rx) = oneshot::channel();
        tokio::pin!(rx);
        match gate
            .classify_race(tool, input, reason, &mut rx, &CancellationToken::new())
            .await
        {
            crate::gate::RaceOutcome::Classified(r) => Some(r),
            _ => None,
        }
    }

    /// 回归：Auto 模式下用户抢答曾经让整轮 panic。
    ///
    /// 竞速的 `tokio::select!` 用 `_ = &mut *rx` **消费**掉了 oneshot 的值，
    /// 然后返回"继续等"，于是 `ask` 又对同一个 receiver poll 了一次 ——
    /// 而 tokio 的 oneshot 取空之后再 poll 是 `panic!("called after
    /// complete")`，不是返回 Err。触发条件很日常：Auto 模式 + 实现了
    /// `classifier_input` 的工具（Bash / WebFetch / 浏览器 evaluate 都有）
    /// + 用户在判危模型返回之前（约一秒的窗口）点了"允许"。
    ///
    /// 那一 panic 跑在轮次的 spawn 里，只杀掉这个 task：`running` 永远留着，
    /// `Done` 永远不发，会话此后永久卡在"忙"。
    #[tokio::test]
    async fn 用户抢在判危之前回答不会丢掉答案() {
        let s = Session::new(SessionId::from_raw("s1"), std::env::temp_dir(), None);
        let gate = gate_auto(&s).await;
        let bash = tool_named("Bash");
        let input = serde_json::json!({ "command": "ls" });

        let (tx, rx) = oneshot::channel();
        tokio::pin!(rx);
        // 判危还没跑完，用户先点了"允许"
        tx.send(PermissionResponse::Allow {
            remember: Vec::new(),
            choice: Vec::new(),
        })
        .expect("接收端还在");

        let out = gate
            .classify_race(
                bash.as_ref(),
                &input,
                &DecisionReason::Unverifiable {
                    what: "Bash".into(),
                },
                &mut rx,
                &CancellationToken::new(),
            )
            .await;

        assert!(
            matches!(
                out,
                crate::gate::RaceOutcome::Answered(PermissionResponse::Allow { .. })
            ),
            "用户的答案必须原样交回调用方 —— 丢掉它，轻则用户点了允许却收到\
             『没有得到回应』，重则调用方再 poll 一次已经取空的 oneshot 而 panic"
        );
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
        assert!(text.contains("Git repository"), "{text}");
        assert!(
            text.contains("Current branch:") || text.contains("detached"),
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
        let s = Arc::new(Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        ));
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
            sandbox_allow_read: Vec::new(),
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

    /// 压缩测试用的会话：transcript 和工件都落在一个临时目录里（用完自动
    /// 删），归档文件不会进用户真实的配置目录。返回 (会话, 临时目录)。
    fn compact_session() -> (Session, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Arc::new(riot_store::Transcripts::new(dir.path()));
        let id = SessionId::from_raw("s1");
        let log = store.open(riot_store::TranscriptMeta {
            id: id.clone(),
            root: dir.path().to_path_buf(),
            created_at_ms: 0,
        });
        let s = Session::new(
            id,
            dir.path().to_path_buf(),
            Some(SessionPersist {
                store,
                log,
                artifacts_root: dir.path().join("artifacts"),
            }),
        );
        (s, dir)
    }

    fn scripted_summary(text: &str) -> Vec<riot_protocol::provider::ProviderEvent> {
        vec![riot_protocol::provider::ProviderEvent::Message(
            hist_assistant("sum", &format!("<summary>{text}</summary>")),
        )]
    }

    fn summary_shape() -> riot_core::summarize::RequestShape {
        riot_core::summarize::RequestShape {
            system: "system".into(),
            tools: Vec::new(),
        }
    }

    fn continuation_text(m: &Message) -> String {
        let Message::User { content, meta, .. } = m else {
            panic!("续接消息是 user：{m:?}")
        };
        assert!(meta.synthetic, "续接消息要打合成标");
        content
            .iter()
            .find_map(|c| match c {
                UserContent::Text { text } => Some(text.clone()),
                _ => None,
            })
            .expect("续接消息有正文")
    }

    /// 压缩落地：最后一轮原样留在总结之后，续接消息指向本会话的摘录，
    /// 摘录里能翻到被压掉的原文。
    ///
    /// `[约束]` 落地逻辑在同一个函数里完成（[`Session::finish_compaction`]），
    /// 阻塞压缩和后台预压缩共用。这条测的是那份共用逻辑本身；摘录的重写
    /// 由调用方在换完历史之后做，测试里照着调用方的顺序来。
    #[tokio::test]
    // 测试读真文件验证落盘结果，走 std::fs 是刻意的。
    #[allow(clippy::disallowed_methods)]
    async fn 压缩落地_尾巴原样保留_原文归档可查() {
        let (s, tmp) = compact_session();
        let writer = Arc::new(crate::digest::DigestWriter::new(
            tmp.path().to_path_buf(),
            Arc::clone(&s.persist.as_ref().unwrap().store),
            Arc::new(riot_providers::watchdog::TokioClock),
        ));
        s.attach_digests(Arc::clone(&writer));
        let provider: Arc<dyn Provider> =
            Arc::new(riot_core::testing::ScriptedProvider::new(Vec::new()));
        let history = vec![
            hist_user("u1", "第一问 独特关键词甲"),
            hist_assistant("a1", "第一答"),
            hist_user("u2", "第二问"),
            hist_assistant("a2", "第二答"),
        ];
        // 历史先落盘（模拟这几轮是正常跑出来的）。
        for m in &history {
            s.persist.as_ref().unwrap().log.append(m);
        }
        let split = compaction_split(provider.as_ref(), &history);
        assert_eq!(split, 2, "从最后一条提问起留尾巴");

        let o = s
            .finish_compaction(&provider, &history, split, "九节总结")
            .await;

        // 重启水合出来的必须和内存里一样：归档 = 头，活历史 = 续接 + 尾巴。
        // 这是 store 的 keep_from 和这里的重放尾巴两半拼起来才成立的事。
        let p = s.persist.as_ref().unwrap();
        p.log.flush().await;
        let parts = p.store.load_parts(&s.id).await;
        assert_eq!(parts.archived, history[..2], "磁盘归档 = 被压掉的那段");
        assert_eq!(parts.live, o.history, "磁盘活历史 = 内存里压缩后的历史");

        assert_eq!(o.history.len(), 3, "续接 + 两条尾巴：{:?}", o.history);
        assert_eq!(o.history[1], history[2]);
        assert_eq!(o.history[2], history[3]);
        assert_eq!(
            s.ui_archive().await,
            history[..2],
            "界面归档只收被总结吞掉的那段，尾巴还活着"
        );

        // 调用方的顺序：换历史 → 重写摘录。
        *s.history.lock().await = o.history.clone();
        s.refresh_digest().await;

        let path = writer.path_for(tmp.path(), &s.id);
        assert!(
            path.starts_with(tmp.path().join("digests")),
            "归档就是会话摘录，不再有 artifacts/history.md：{}",
            path.display()
        );
        assert!(
            !tmp.path()
                .join("artifacts")
                .join("s1")
                .join("history.md")
                .exists(),
            "旧的归档文件不该再写"
        );
        let text = std::fs::read_to_string(&path).expect("摘录要落盘");
        assert!(
            text.contains("独特关键词甲"),
            "被压掉的原话要在文件里：{text}"
        );
        assert!(
            text.contains("第二问"),
            "摘录是整段对话，尾巴也在（模型看的是同一份文件）：{text}"
        );
        assert!(text.contains("## [1] 用户"), "序号从 1 起：{text}");

        let cont = continuation_text(&o.history[0]);
        assert!(cont.contains("九节总结"));
        assert!(
            cont.contains(&path.display().to_string()),
            "续接消息要给出摘录路径：{cont}"
        );

        // 第二次压缩：摘录整体重渲染，序号按整段对话连续。
        let mut later = o.history.clone();
        later.push(hist_user("u3", "第三问"));
        later.push(hist_assistant("a3", "第三答"));
        let split2 = compaction_split(provider.as_ref(), &later);
        assert_eq!(split2, 3, "尾巴是 u3/a3");
        let o2 = s
            .finish_compaction(&provider, &later, split2, "再总结")
            .await;
        *s.history.lock().await = o2.history.clone();
        s.refresh_digest().await;
        let text = std::fs::read_to_string(&path).expect("摘录");
        assert!(text.contains("独特关键词甲"), "第一段原文还在：{text}");
        assert!(
            text.contains("## [3] 用户（系统合成）"),
            "第一条续接消息排在第 3：{text}"
        );
        assert!(text.contains("第三问"), "{text}");
        assert_eq!(s.ui_archive().await.len(), 5, "2 + 3（续接、u2、a2）");
    }

    /// 和线上 provider 同口径的替身：`count_tokens` 拿最后一条带 usage 的
    /// assistant 打底，`estimate_tokens_of` 只看内容。`ScriptedProvider`
    /// 两者相同（没打底），测不出下面那条 bug。
    struct UsageAware(riot_core::testing::ScriptedProvider);

    #[async_trait::async_trait]
    impl Provider for UsageAware {
        fn stream(
            &self,
            req: riot_protocol::provider::ProviderRequest,
            cancel: CancellationToken,
        ) -> riot_protocol::provider::ProviderStream {
            self.0.stream(req, cancel)
        }
        fn count_tokens(&self, messages: &[Message]) -> u32 {
            let (from, base) = riot_protocol::provider::last_usage_checkpoint(messages);
            base + self.0.count_tokens(&messages[from..])
        }
        fn estimate_tokens_of(&self, messages: &[Message]) -> u32 {
            self.0.count_tokens(messages)
        }
    }

    fn hist_assistant_with_usage(id: &str, text: &str, context: u32) -> Message {
        Message::Assistant {
            id: MessageId::from_raw(id),
            content: vec![riot_protocol::message::AssistantContent::Text { text: text.into() }],
            usage: Some(riot_protocol::message::Usage {
                input_tokens: context,
                output_tokens: 10,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            }),
            meta: MessageMeta::default(),
        }
    }

    /// 线上历史里每条 assistant 都带着"那次请求整个上下文"的 usage。切分
    /// 和落地都不能被它带偏：尾巴照样保留，压缩后量出来是真的小。
    ///
    /// 这条曾经坏过：切分用 `count_tokens` 量尾巴，尾巴里的 assistant 一打底
    /// 就是三十万，永远超 20k 预算 → 从没留过尾巴，每次压缩模型都失忆。
    #[tokio::test]
    async fn 尾巴里的旧_usage_不影响切分和压缩后的尺寸() {
        let (s, _tmp) = compact_session();
        let provider: Arc<dyn Provider> = Arc::new(UsageAware(
            riot_core::testing::ScriptedProvider::new(Vec::new()),
        ));
        let history = vec![
            hist_user("u1", "第一问"),
            hist_assistant_with_usage("a1", "第一答", 290_000),
            hist_user("u2", "第二问"),
            hist_assistant_with_usage("a2", "第二答", 300_000),
        ];
        assert!(
            provider.count_tokens(&history) >= 300_000,
            "整份历史按打底口径量，是压缩前的真实尺寸"
        );

        let split = compaction_split(provider.as_ref(), &history);
        assert_eq!(split, 2, "尾巴（u2/a2）几十个字节，必须保留");

        let o = s
            .finish_compaction(&provider, &history, split, "总结")
            .await;
        assert!(o.before_tokens >= 300_000, "{}", o.before_tokens);
        assert!(
            o.after_tokens < 1_000,
            "压缩后的历史不能再被尾巴里的旧 usage 顶回三十万：{}",
            o.after_tokens
        );
        assert_eq!(o.history.len(), 3, "续接 + 尾巴两条");
        assert!(
            matches!(&o.history[2], Message::Assistant { usage: None, .. }),
            "尾巴里 assistant 的 usage 要抹掉：{:?}",
            o.history[2]
        );
        assert!(
            provider.count_tokens(&o.history) < 1_000,
            "此后任何地方再量这份历史，都不该被旧 usage 打底"
        );

        // 重启水合出来的也一样干净。
        let p = s.persist.as_ref().unwrap();
        p.log.flush().await;
        let parts = p.store.load_parts(&s.id).await;
        assert_eq!(parts.live, o.history);
    }

    /// 反应式压缩落地：轻档（清占位符）整份重写，重档（总结 + 尾巴）归档头、
    /// 保尾巴。内存、界面归档、重启水合三边一致。
    #[tokio::test]
    async fn 反应式压缩落地_轻档重写_重档归档头保尾巴() {
        let (s, _tmp) = compact_session();
        let history = vec![
            hist_user("u1", "第一问"),
            hist_assistant("a1", "第一答"),
            hist_tool_result("r1", "Read"),
            hist_user("u2", "第二问"),
            hist_assistant("a2", "第二答"),
        ];
        for m in &history {
            s.persist.as_ref().unwrap().log.append(m);
        }
        *s.history.lock().await = history.clone();

        // 轻档：r1 的结果被清成占位符，其余原样。
        let mut light = history.clone();
        light[2] = Message::User {
            id: MessageId::from_raw("r1"),
            content: vec![UserContent::ToolResult {
                tool_use_id: riot_protocol::id::ToolUseId::from_raw("Read"),
                content: riot_protocol::message::ToolResultContent::Cleared,
                is_error: false,
            }],
            meta: MessageMeta::default(),
        };
        s.absorb_reactive_compaction(light.clone(), 1000, 900).await;
        assert_eq!(*s.history.lock().await, light, "内存历史换成压缩后的");
        assert!(s.ui_archive().await.is_empty(), "轻档没有头，不归档");
        let p = s.persist.as_ref().unwrap();
        p.log.flush().await;
        let parts = p.store.load_parts(&s.id).await;
        assert_eq!(parts.live, light, "重启读回来的是清过的那份");
        assert!(parts.archived.is_empty());

        // 重档：总结吞掉 u1..r1，尾巴 u2/a2 原样。
        let cont = hist_user("c1", "前文总结");
        let heavy = vec![cont.clone(), light[3].clone(), light[4].clone()];
        s.absorb_reactive_compaction(heavy.clone(), 900, 100).await;
        assert_eq!(*s.history.lock().await, heavy);
        assert_eq!(s.ui_archive().await, light[..3], "被总结吞掉的头进界面归档");
        p.log.flush().await;
        let parts = p.store.load_parts(&s.id).await;
        assert_eq!(parts.live, heavy, "重启：续接 + 尾巴");
        assert_eq!(parts.archived, light[..3], "重启：头在归档里");
    }

    /// 预压缩：指纹对得上就换入（不再调模型），对不上就作废。
    #[tokio::test]
    async fn 预压缩_指纹对得上换入_对不上作废() {
        let (s, _tmp) = compact_session();
        let scripted = Arc::new(riot_core::testing::ScriptedProvider::new(vec![
            scripted_summary("后台算好的总结"),
            scripted_summary("第二份"),
        ]));
        let provider: Arc<dyn Provider> = Arc::clone(&scripted) as _;
        let history = vec![
            hist_user("u1", "第一问"),
            hist_assistant("a1", "第一答"),
            hist_user("u2", "第二问"),
            hist_assistant("a2", "第二答"),
        ];

        s.spawn_precompact(&provider, "m", history.clone(), summary_shape())
            .await;
        let got = s
            .take_precompact(&history, &test_sink(), &CancellationToken::new())
            .await
            .expect("同一份历史，预压缩该能用");
        assert_eq!(got.0, 2, "切点随总结一起带回");
        assert_eq!(got.1, "后台算好的总结");
        assert!(s.precompact.lock().await.is_none(), "取走后槽位清空");
        assert!(!s.is_compacting(), "compacting 标志不能残留");

        // 历史变了（多了一条）→ 作废。
        s.spawn_precompact(&provider, "m", history.clone(), summary_shape())
            .await;
        let mut changed = history.clone();
        changed.push(hist_user("u3", "又说了一句"));
        assert!(
            s.take_precompact(&changed, &test_sink(), &CancellationToken::new())
                .await
                .is_none(),
            "基于旧历史的总结不能换进新历史"
        );
        assert!(s.precompact.lock().await.is_none(), "作废也要清槽位");
    }

    /// 原地编辑不改条数和末条，指纹抓不住 —— 编辑路径必须显式作废。
    #[tokio::test]
    async fn 编辑上下文后预压缩作废() {
        let (s, _tmp) = compact_session();
        let provider: Arc<dyn Provider> =
            Arc::new(riot_core::testing::ScriptedProvider::new(vec![
                scripted_summary("总结"),
            ]));
        let history = vec![
            hist_user("u1", "第一问"),
            hist_assistant("a1", "第一答"),
            hist_user("u2", "第二问"),
            hist_assistant("a2", "第二答"),
        ];
        *s.history.lock().await = history.clone();
        s.spawn_precompact(&provider, "m", history.clone(), summary_shape())
            .await;
        s.edit_message("u1", "改过的第一问")
            .await
            .expect("编辑成功");
        assert!(
            s.precompact.lock().await.is_none(),
            "编辑过的历史和预压缩基于的那份不是一回事"
        );
    }

    /// 后台总结失败：换入时拿到 None，调用方退回阻塞路径；不 panic、不残留。
    #[tokio::test]
    async fn 预压缩失败时换入拿到空_退回阻塞路径() {
        let (s, _tmp) = compact_session();
        let provider: Arc<dyn Provider> =
            Arc::new(riot_core::testing::ScriptedProvider::new(vec![vec![
                riot_protocol::provider::ProviderEvent::Error(
                    riot_protocol::provider::ProviderError::Transport {
                        message: "断网".into(),
                    },
                ),
            ]]));
        let history = vec![
            hist_user("u1", "第一问"),
            hist_assistant("a1", "第一答"),
            hist_user("u2", "第二问"),
        ];
        s.spawn_precompact(&provider, "m", history.clone(), summary_shape())
            .await;
        assert!(
            s.take_precompact(&history, &test_sink(), &CancellationToken::new())
                .await
                .is_none()
        );
        assert!(!s.is_compacting());
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
            sandbox_allow_read: Vec::new(),
        }
    }

    fn queued_entry(id: &str, text: &str) -> QueuedEntry {
        QueuedEntry {
            id: id.into(),
            kind: QueuedKind::Interjection(TurnInput {
                text: text.into(),
                ..Default::default()
            }),
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
        let s = Arc::new(Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        ));
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

    fn task_view(id: &str) -> riot_protocol::task::BackgroundTaskView {
        riot_protocol::task::BackgroundTaskView {
            id: riot_protocol::id::AgentId::from_raw(id),
            title: "改 a".into(),
            kind: "general-purpose".into(),
            model: "m".into(),
            background: true,
            tool_use_id: riot_protocol::id::ToolUseId::from_raw("tu_1"),
            parent: None,
            status: riot_protocol::task::BackgroundTaskStatus::Running,
            activity: String::new(),
            tool_uses: 0,
            tokens: 0,
            started_at_ms: 0,
            finished_at_ms: None,
        }
    }

    fn start_bg_task(s: &Session, id: &str, cancel: CancellationToken) {
        s.tasks.start(
            task_view(id),
            crate::subagent::Kind::Explore,
            cancel,
            Vec::new(),
            0,
        );
    }

    fn notice(id: &str) -> Message {
        let mut view = task_view(id);
        view.status = riot_protocol::task::BackgroundTaskStatus::Completed;
        view.finished_at_ms = Some(1);
        crate::tasks::notice_message(MessageId::from_raw(id), &view, "m", "改好了", 1)
    }

    /// 父在跑：通知进队列、安全点注入 —— 但排队面板看不见它，也删不到它。
    #[tokio::test]
    async fn 后台通知在跑轮中走队列且对面板不可见() {
        let s = Arc::new(Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        ));
        *s.running.lock().await = Some(CancellationToken::new());
        s.queue.push(queued_entry("m_q1", "用户插话"));
        s.deliver_task_notice(notice("agt_1")).await;

        let snap = s.queue_snapshot();
        assert_eq!(snap.len(), 1, "面板只该看到用户的插话：{snap:?}");
        assert_eq!(snap[0].id, "m_q1");
        assert!(!s.queue_remove("agt_1"), "通知条目前端删不到");
        assert!(s.queue_take("agt_1").is_none(), "通知条目也撤不回输入框");

        use riot_core::state::InputQueue;
        let drained = s.queue.drain();
        assert_eq!(drained.len(), 2, "内核 drain 时两条都要拿到");
        assert!(
            drained.iter().any(|m| matches!(
                m, Message::User { meta, .. } if meta.task_notice.is_some()
            )),
            "通知要跟着注入"
        );
        assert!(s.pending_notices.lock().unwrap().is_empty());
    }

    /// 父空闲但还没跑过任何一轮：没有配置可沿用，通知先攒着。
    #[tokio::test]
    async fn 后台通知在无配置时先攒着() {
        let s = Arc::new(Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        ));
        s.deliver_task_notice(notice("agt_1")).await;
        assert!(s.running.lock().await.is_none(), "不该开轮");
        assert_eq!(s.pending_notices.lock().unwrap().len(), 1);
    }

    /// 父空闲且跑过一轮：通知唤起新的一轮。这里的配置缺 key，轮子立刻
    /// 失败 —— 通知不能被吞掉，要攒回去等下一轮。
    #[tokio::test]
    // 等一个 spawn 出去的轮子收场，真睡是刻意的（没有注入时钟可推）。
    #[allow(clippy::disallowed_methods)]
    async fn 后台通知唤醒轮起不来时通知攒回去() {
        let s = Arc::new(Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        ));
        *s.last_turn.lock().await = Some(LastTurn {
            model: test_model(),
            caps: test_caps(),
            limits: test_limits(),
        });
        s.deliver_task_notice(notice("agt_1")).await;
        // 唤醒轮在 spawn 里跑，等它收场。
        for _ in 0..50 {
            if s.running.lock().await.is_none() && !s.pending_notices.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(s.running.lock().await.is_none(), "失败的唤醒轮要清 running");
        assert_eq!(
            s.pending_notices.lock().unwrap().len(),
            1,
            "起不来的唤醒轮不能把通知吞掉"
        );
    }

    /// 关会话之后到达的通知丢弃，后台任务一起停。
    #[tokio::test]
    async fn 关会话时停掉后台任务且丢弃后续通知() {
        let s = Arc::new(Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        ));
        let token = CancellationToken::new();
        start_bg_task(&s, "agt_bg", token.clone());
        s.abort_turn().await;
        assert!(token.is_cancelled(), "关会话要把后台任务一起停掉");

        s.deliver_task_notice(notice("agt_bg")).await;
        assert!(
            s.pending_notices.lock().unwrap().is_empty(),
            "关闭后的通知该丢弃"
        );
        assert!(s.running.lock().await.is_none());
    }

    fn note_text(c: Option<UserContent>) -> Option<String> {
        match c {
            Some(UserContent::Attachment(Attachment::SystemReminder { text })) => Some(text),
            None => None,
            other => panic!("提醒该是 SystemReminder：{other:?}"),
        }
    }

    /// 完整准则只注一次，之后是短提醒；历史被动过重注完整版；关掉说一次退出。
    #[test]
    fn 多任务提醒_首轮完整_之后简短_关掉说一次() {
        let s = Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        );
        assert!(note_text(s.multitask_note()).is_none(), "没开就不注");

        s.set_multitask(true);
        let first = note_text(s.multitask_note()).expect("开了要注");
        assert!(first.contains("Core rules"), "首轮完整版：{first}");
        let second = note_text(s.multitask_note()).expect("之后每轮都注");
        assert!(
            second.contains("still in **multitask mode**"),
            "之后简短：{second}"
        );
        assert!(!second.contains("Core rules"));

        s.set_multitask(true);
        assert!(
            note_text(s.multitask_note())
                .unwrap()
                .contains("still in **multitask mode**"),
            "同值重设不重注完整版"
        );

        s.forget_multitask_announce();
        assert!(
            note_text(s.multitask_note())
                .unwrap()
                .contains("Core rules"),
            "历史动过要重注完整版"
        );

        s.set_multitask(false);
        let exit = note_text(s.multitask_note()).expect("关掉说一声");
        assert!(exit.contains("has left multitask mode"), "{exit}");
        assert!(note_text(s.multitask_note()).is_none(), "退出只说一次");
    }

    /// 界面按钮：有轮在跑才排得上；「并行构建」顺手打开多任务、连完整准则一起送。
    #[tokio::test]
    async fn 界面提醒只在跑轮时排队_并行构建打开多任务() {
        let s = Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        );
        assert!(
            !s.nudge(riot_protocol::Nudge::StartMultitasking).await,
            "闲着没对象"
        );

        *s.running.lock().await = Some(CancellationToken::new());
        assert!(s.nudge(riot_protocol::Nudge::StartMultitasking).await);
        assert!(s.queue_snapshot().is_empty(), "提醒不进排队面板");

        assert!(s.nudge(riot_protocol::Nudge::BuildInParallel).await);
        assert!(s.multitask(), "并行构建 = 进入多任务模式");

        use riot_core::state::InputQueue;
        // 带外通道取 —— 这就是主循环在工具结果就位时走的那条路。等收尾
        // drain 才拿到的话，「转到后台」在用户眼里就是个没反应的按钮。
        let drained = s.queue.drain_out_of_band();
        assert_eq!(drained.len(), 2);
        let texts: Vec<String> = drained.iter().map(|m| format!("{m:?}")).collect();
        assert!(texts[0].contains("转到后台") && texts[0].contains("resume=\\\"self\\\""));
        assert!(texts[1].contains("并行构建") && texts[1].contains("Core rules"));
        assert!(
            note_text(s.multitask_note())
                .unwrap()
                .contains("still in **multitask mode**"),
            "完整版已随提醒送过，下一轮只要短的"
        );
    }

    /// 带外通道只带走带外条目：用户插话留在队列里等收尾。
    ///
    /// 搞混了两种时机都会坏：插话跟着提醒中途蹦出来（惊吓），或者提醒
    /// 跟着插话等到整轮跑完（按钮失效）。
    #[tokio::test]
    async fn 带外注入不碰用户插话() {
        use riot_core::state::InputQueue;
        let s = Arc::new(Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        ));
        *s.running.lock().await = Some(CancellationToken::new());
        s.queue.push(queued_entry("m_q1", "用户插话"));
        assert!(s.nudge(riot_protocol::Nudge::StartMultitasking).await);
        s.deliver_task_notice(notice("agt_1")).await;

        let oob = s.queue.drain_out_of_band();
        assert_eq!(oob.len(), 2, "提醒和完成通知都是带外的");
        let snap = s.queue_snapshot();
        assert_eq!(snap.len(), 1, "插话还在面板上等收尾：{snap:?}");
        assert_eq!(snap[0].id, "m_q1");
        assert_eq!(s.queue.drain().len(), 1, "收尾 drain 拿到的只有插话");
    }

    /// 轮次半路收场（中断 / 出错）时的残留处置：通知留到下一轮，界面
    /// 提醒作废。
    ///
    /// 提醒留着的话，用户下次随便问句什么都会被无端分叉到后台 —— 它是对
    /// 当时那件事说的话，那件事已经收场了。
    #[tokio::test]
    async fn 轮次收场时界面提醒作废而通知留着() {
        let s = Arc::new(Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        ));
        s.queue.push(QueuedEntry {
            id: "m_nudge".into(),
            kind: QueuedKind::Nudge,
            msg: Message::User {
                id: MessageId::from_raw("m_nudge"),
                content: vec![crate::prompt::nudge_start_multitasking()],
                meta: MessageMeta {
                    synthetic: true,
                    ..Default::default()
                },
            },
        });
        s.queue.push(QueuedEntry {
            id: "agt_1".into(),
            kind: QueuedKind::TaskNotice,
            msg: notice("agt_1"),
        });

        // 缺 key，这一轮立刻失败 —— 正是"没走到 drain 点"的那类收尾。
        let input = TurnInput {
            text: "hi".into(),
            ..Default::default()
        };
        let _ = s
            .run_turn(input, test_model(), test_caps(), test_sink(), test_limits())
            .await;

        let pending = s.pending_notices.lock().unwrap();
        assert_eq!(pending.len(), 1, "完成通知要留到下一轮开工时注入");
        assert!(
            matches!(&pending[0], Message::User { meta, .. } if meta.task_notice.is_some()),
            "留下的该是通知，不是那条界面提醒：{:?}",
            pending[0]
        );
    }

    /// 用户按停止只停前台，后台任务不受影响。
    #[tokio::test]
    async fn 用户停止不带走后台任务() {
        let s = Arc::new(Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        ));
        let token = CancellationToken::new();
        start_bg_task(&s, "agt_bg", token.clone());
        *s.running.lock().await = Some(CancellationToken::new());
        assert!(s.interrupt().await);
        assert!(!token.is_cancelled(), "停止键只停前台");
        assert!(
            s.cancel_task(&riot_protocol::id::AgentId::from_raw("agt_bg")),
            "面板的停止键停得到"
        );
        assert!(token.is_cancelled());
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
                artifacts_root: dir.path().join("artifacts"),
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

    /// 编辑后重发：截到那条提问（含）、换掉它的字、附件原位保留；
    /// 重启重放出来和内存一致。只认用户提问，改回复走「编辑」那条路。
    #[tokio::test]
    async fn 编辑后重发_截到提问并换字_重放一致() {
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
                artifacts_root: dir.path().join("artifacts"),
            }),
        );
        // 第二条提问带一张图：改字不该把图弄丢。
        let with_image = Message::User {
            id: MessageId::from_raw("m2"),
            content: vec![
                UserContent::Attachment(Attachment::Image {
                    media_type: "image/png".into(),
                    data: "AAAA".into(),
                }),
                UserContent::Text {
                    text: "第二问（原）".into(),
                },
            ],
            meta: MessageMeta::default(),
        };
        for m in [
            hist_user("m1", "第一问"),
            hist_assistant("a1", "第一答"),
            with_image.clone(),
            hist_assistant("a2", "第二答"),
            hist_user("m3", "第三问"),
            hist_assistant("a3", "第三答"),
        ] {
            s.history.lock().await.push(m.clone());
            if let Some(p) = &s.persist {
                p.log.append(&m);
            }
        }
        s.flush_log().await;

        // 只认用户提问。
        assert!(
            s.truncate_to_edited_prompt("a2", "改回复").await.is_err(),
            "从助手消息重发要被拒"
        );
        assert!(s.truncate_to_edited_prompt("ghost", "x").await.is_err());

        s.truncate_to_edited_prompt("m2", "第二问（改）")
            .await
            .expect("能重发");

        let hist = s.history().await;
        assert_eq!(hist.len(), 3, "截到 m2（含）：{hist:?}");
        let Message::User { content, .. } = &hist[2] else {
            panic!("末条是提问")
        };
        assert!(
            content
                .iter()
                .any(|c| matches!(c, UserContent::Attachment(Attachment::Image { .. }))),
            "图片要原位保留：{content:?}"
        );
        assert!(
            content
                .iter()
                .any(|c| matches!(c, UserContent::Text { text } if text == "第二问（改）")),
            "文字要换成新的：{content:?}"
        );

        s.flush_log().await;
        let parts = store.load_parts(&id).await;
        assert_eq!(parts.live, hist, "重启重放出来必须和内存一致");
    }

    /// 上下文编辑改的是活历史和 transcript 两份，重启后必须还是改过的样子。
    #[tokio::test]
    async fn 编辑消息改历史也改记录() {
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
                artifacts_root: dir.path().join("artifacts"),
            }),
        );
        for m in [hist_user("m1", "第一句"), hist_assistant("a1", "答错了")] {
            s.history.lock().await.push(m.clone());
            if let Some(p) = &s.persist {
                p.log.append(&m);
            }
        }

        s.edit_message("a1", "改对了").await.expect("能编辑");

        let hist = s.history().await;
        assert_eq!(hist[1], hist_assistant("a1", "改对了"), "内存里是新文本");

        s.flush_log().await;
        let parts = store.load_parts(&id).await;
        assert_eq!(parts.live[1], hist_assistant("a1", "改对了"), "重启后也是");

        // 空文本不是编辑，指路删除。
        assert!(s.edit_message("a1", "  ").await.is_err());
        // 不存在的消息要报得出来。
        assert!(s.edit_message("ghost", "x").await.is_err());
    }

    /// 会话摘录跟着上下文变：编辑后是新文本、删除后不再出现、改名后
    /// INDEX 跟着换；首条消息里带会话 id；写入器没挂或关掉时一切照常。
    // 豁免理由：测试直接读临时目录里的摘录文件核对落盘结果。
    #[allow(clippy::disallowed_methods)]
    #[tokio::test]
    async fn 摘录跟着编辑_删除_改名走() {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Arc::new(riot_store::Transcripts::new(dir.path()));
        let id = SessionId::from_raw("s1");
        let root = dir.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let log = store.open(riot_store::TranscriptMeta {
            id: id.clone(),
            root: root.clone(),
            created_at_ms: 0,
        });
        let s = Session::new(
            id.clone(),
            root.clone(),
            Some(SessionPersist {
                store: Arc::clone(&store),
                log,
                artifacts_root: dir.path().join("artifacts"),
            }),
        );
        // 没挂写入器：提示词不指路、首条消息不带 id、刷新是空操作。
        assert!(s.digests_dir().is_none());
        assert!(s.session_id_note().is_none());
        s.refresh_digest().await;

        let writer = Arc::new(crate::digest::DigestWriter::new(
            dir.path().to_path_buf(),
            Arc::clone(&store),
            Arc::new(riot_providers::watchdog::TokioClock),
        ));
        s.attach_digests(Arc::clone(&writer));
        let digest_path = writer.project_dir(&root).expect("开着").join("s1.md");
        assert_eq!(s.digests_dir().as_deref(), digest_path.parent());
        let note = s.session_id_note().expect("开着就带 id");
        let Attachment::SystemReminder { text } = note else {
            panic!("要走 system-reminder：{note:?}")
        };
        assert!(text.contains("`s1`"), "{text}");

        for m in [
            hist_user("m1", "第一轮的提问"),
            hist_assistant("a1", "答错了"),
            hist_user("m2", "第二轮的提问"),
            hist_assistant("a2", "第二轮的回答"),
        ] {
            s.history.lock().await.push(m.clone());
            if let Some(p) = &s.persist {
                p.log.append(&m);
            }
        }
        // hydrate 会去读 transcript：先让它落盘，否则水合读到半截。
        s.flush_log().await;

        s.edit_message("a1", "改对了").await.expect("能编辑");
        let text = std::fs::read_to_string(&digest_path).expect("编辑后摘录存在");
        assert!(
            text.contains("改对了") && !text.contains("答错了"),
            "{text}"
        );
        assert!(text.contains("第二轮的提问"), "{text}");

        s.delete_message("a2").await.expect("能删除");
        let text = std::fs::read_to_string(&digest_path).unwrap();
        assert!(!text.contains("第二轮"), "删掉的一轮不能留在摘录里：{text}");
        assert!(text.contains("第一轮的提问"), "{text}");

        s.set_title(Some("我起的名".into())).await;
        let text = std::fs::read_to_string(&digest_path).unwrap();
        assert!(text.contains("title: 我起的名"), "{text}");
        let idx = std::fs::read_to_string(digest_path.parent().unwrap().join("INDEX.md")).unwrap();
        assert!(idx.contains("我起的名") && idx.contains("s1.md"), "{idx}");

        // 关掉之后：提示词不指路，刷新不写。
        writer.set_enabled(false);
        assert!(s.digests_dir().is_none());
        assert!(s.session_id_note().is_none());
    }

    /// 删除按轮成对：点回复删的是"提问 + 回复"这一轮，历史和
    /// transcript 都不留，前后两轮原样。
    #[tokio::test]
    async fn 删除回复连提问一起删() {
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
                artifacts_root: dir.path().join("artifacts"),
            }),
        );
        for m in [
            hist_user("m1", "第一句"),
            hist_assistant("a1", "不想要的回复"),
            hist_user("m2", "第二句"),
            hist_assistant("a2", "留下的回复"),
        ] {
            s.history.lock().await.push(m.clone());
            if let Some(p) = &s.persist {
                p.log.append(&m);
            }
        }

        s.delete_message("a1").await.expect("能删除");

        let hist = s.history().await;
        assert_eq!(
            hist.iter().map(|m| m.id().as_str()).collect::<Vec<_>>(),
            vec!["m2", "a2"],
            "回复连着它的提问一起删，后一轮不动"
        );

        s.flush_log().await;
        let parts = store.load_parts(&id).await;
        assert_eq!(
            parts
                .live
                .iter()
                .map(|m| m.id().as_str())
                .collect::<Vec<_>>(),
            vec!["m2", "a2"],
            "重启后也不回来：{parts:?}"
        );
    }

    /// 删除中间一轮：前后两轮贴上，形状仍是 user 开头、user/assistant
    /// 交替 —— 成对删除天然不会造出服务方拒绝的历史。
    #[tokio::test]
    async fn 删除中间一轮前后保留() {
        let s = Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        );
        s.history.lock().await.extend([
            hist_user("m1", "第一轮"),
            hist_assistant("a1", "第一轮回复"),
            hist_user("m2", "中间那轮"),
            hist_assistant("a2", "中间那轮回复"),
            hist_user("m3", "第三轮"),
            hist_assistant("a3", "第三轮回复"),
        ]);

        // 点提问和点回复删的是同一轮。
        s.delete_message("m2").await.expect("能删中间一轮");
        assert_eq!(
            s.history()
                .await
                .iter()
                .map(|m| m.id().as_str())
                .collect::<Vec<_>>(),
            vec!["m1", "a1", "m3", "a3"]
        );
    }

    /// 例外：提问发出去、模型没来得及回应就被停了 —— 这一轮只有提问，
    /// 删除也就只删它，不碰前一轮的回复。
    #[tokio::test]
    async fn 没有回应的提问只删自己() {
        let s = Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        );
        s.history.lock().await.extend([
            hist_user("m1", "第一句"),
            hist_assistant("a1", "回复"),
            hist_user("m2", "被取消的提问"),
        ]);

        s.delete_message("m2").await.expect("能删");
        assert_eq!(
            s.history()
                .await
                .iter()
                .map(|m| m.id().as_str())
                .collect::<Vec<_>>(),
            vec!["m1", "a1"],
            "只删没有回应的提问自己"
        );
    }

    /// 带工具调用的轮整轮消失：tool_use 和 tool_result 一起走，
    /// 不留悬空配对；插话开启的下一轮不受影响。
    #[tokio::test]
    async fn 删除带工具调用的轮不留悬空配对() {
        let s = Session::new(
            SessionId::from_raw("s1"),
            std::path::PathBuf::from("/tmp"),
            None,
        );
        let with_tool = Message::Assistant {
            id: MessageId::from_raw("a1"),
            content: vec![
                riot_protocol::message::AssistantContent::Text {
                    text: "我来读一下".into(),
                },
                riot_protocol::message::AssistantContent::ToolUse {
                    id: riot_protocol::id::ToolUseId::from_raw("tu1"),
                    name: "Read".into(),
                    input: serde_json::json!({}),
                },
            ],
            usage: None,
            meta: MessageMeta::default(),
        };
        s.history.lock().await.extend([
            hist_user("m1", "读一下"),
            with_tool,
            hist_tool_result("t1", "tu1"),
            hist_assistant("a2", "读完了"),
            hist_user("m2", "插话开启的下一轮"),
            hist_assistant("a3", "下一轮回复"),
        ]);

        s.delete_message("a1").await.expect("能删");
        let hist = s.history().await;
        assert_eq!(
            hist.iter().map(|m| m.id().as_str()).collect::<Vec<_>>(),
            vec!["m2", "a3"],
            "提问、工具调用、工具结果、收尾回复一轮全走"
        );
        assert!(
            hist.iter().all(|m| m.tool_use_ids().is_empty()),
            "不留悬空的 tool_use"
        );
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
                artifacts_root: dir.path().join("artifacts"),
            }),
        );

        {
            let mut live = s.live_stream.lock().await;
            live.text.push_str("先说一半");
            live.thinking.push_str("想了很久");
        }
        let msg = s
            .finalize_partial("deepseek-chat", 1_700_000_000_000)
            .await
            .expect("有半截正文就该定稿");
        match &msg {
            Message::Assistant { content, meta, .. } => {
                assert_eq!(content.len(), 1, "思考不定稿：没有签名，回喂给模型是错的");
                assert!(meta.interrupted, "界面靠它标注'已中断'");
                assert_eq!(
                    meta.created_at_ms,
                    Some(1_700_000_000_000),
                    "半截回答也是一条消息，界面要显示它的时间"
                );
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
        assert!(
            s.finalize_partial("deepseek-chat", 1_700_000_001_000)
                .await
                .is_none()
        );
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

    /// 按脚本吐快照的探针替身。脚本项为 `None` 表示这一轮采样失败
    /// （探针断了）；脚本耗尽后也一律失败。
    struct FakeEnv(std::sync::Mutex<Vec<Option<riot_protocol::env::EnvSnapshot>>>);

    impl FakeEnv {
        fn new(snaps: Vec<riot_protocol::env::EnvSnapshot>) -> Arc<Self> {
            Self::script(snaps.into_iter().map(Some).collect())
        }

        fn script(snaps: Vec<Option<riot_protocol::env::EnvSnapshot>>) -> Arc<Self> {
            Arc::new(Self(std::sync::Mutex::new(snaps)))
        }
    }

    #[async_trait::async_trait]
    impl riot_protocol::env::EnvProbe for FakeEnv {
        async fn sample(&self) -> Option<riot_protocol::env::EnvSnapshot> {
            let mut g = self.0.lock().expect("脚本锁");
            if g.is_empty() { None } else { g.remove(0) }
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

    /// 只挑快照附件（以快照头开场的那些）。轮首状态行（时钟/间隔/档位）
    /// 每轮都在，断言"快照发没发"必须把它滤掉。
    fn snap_texts(parts: &[UserContent]) -> Vec<String> {
        env_texts(parts)
            .into_iter()
            .filter(|t| t.starts_with(crate::env::SNAPSHOT_HEADER))
            .collect()
    }

    /// 差分注入的核心不变量：没变化 = 不重发快照。防的是上下文膨胀 ——
    /// 每轮复读一遍环境，长会话里会堆出几十份一样的快照。
    #[tokio::test]
    async fn 环境快照_变化才发_不变只剩时钟行() {
        let s = Session::new(SessionId::from_raw("s1"), std::env::temp_dir(), None);
        let a = env_snap(&[(3, "pnpm dev", true)]);
        s.attach_env(FakeEnv::new(vec![
            a.clone(),
            a.clone(),
            env_snap(&[(3, "pnpm dev", false)]),
        ]));

        // 第一轮：有东西，注入全量。
        let first = s.env_prelude(0, 0, None, 0, 100_000).await;
        assert_eq!(snap_texts(&first).len(), 1, "首轮该注入");
        assert!(snap_texts(&first)[0].contains("[3]"), "{first:?}");

        // 第二轮：一模一样，快照不重发，只剩轮首状态行。
        let second = s.env_prelude(0, 0, None, 0, 100_000).await;
        assert!(
            snap_texts(&second).is_empty(),
            "没变化不该重发快照：{second:?}"
        );
        assert_eq!(second.len(), 1, "只该剩一条轮首状态行：{second:?}");

        // 第三轮：服务退出了（running 翻转），差分触发。
        let third = s.env_prelude(0, 0, None, 0, 100_000).await;
        assert_eq!(snap_texts(&third).len(), 1, "状态变了该再注入");
        assert!(snap_texts(&third)[0].contains("exited"), "{third:?}");
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
            snap_texts(&s.env_prelude(0, 0, None, 0, 100_000).await).is_empty(),
            "对着空房间描述空房间是噪音"
        );
        assert!(
            !snap_texts(&s.env_prelude(0, 0, None, 0, 100_000).await).is_empty(),
            "终端出现了该说"
        );
        let gone = s.env_prelude(0, 0, None, 0, 100_000).await;
        assert!(
            snap_texts(&gone)[0].contains("No terminal in the panel is visible to you"),
            "从有到无也是变化：{gone:?}"
        );
    }

    /// 探针拿不到（宿主没装配）不发快照、不误报作废 —— 从没注入过
    /// 快照的会话谈不上"过期"。时钟行照发：感知是锦上添花，时间不是。
    #[tokio::test]
    async fn 环境快照_探针不可用_只有时钟行且不误报作废() {
        let s = Session::new(SessionId::from_raw("s1"), std::env::temp_dir(), None);
        // 不 attach：默认 NoEnvProbe。
        let parts = s.env_prelude(0, 0, None, 50_000, 100_000).await;
        assert_eq!(parts.len(), 1, "{parts:?}");
        assert!(snap_texts(&parts).is_empty());
        assert!(
            !env_texts(&parts)[0].contains("Environment sampling failed"),
            "从没有过快照就不该宣告作废：{parts:?}"
        );
    }

    /// 时钟行每轮必发：消息在 wire 上不带时间戳，这一行是模型唯一的钟。
    /// 不做差分 —— "没变化就不发"对时间没有意义。
    #[tokio::test]
    async fn 轮首时钟行_每轮都发_按时区渲染() {
        let s = Session::new(SessionId::from_raw("s1"), std::env::temp_dir(), None);
        // 2026-08-31T08:37Z，东八区 = 16:37 周一。
        let monday = 1_788_165_420_000u64;
        let p1 = s.env_prelude(monday, 480, None, 0, 100_000).await;
        assert!(
            env_texts(&p1)[0].contains("2026-08-31 (Monday) 16:37, UTC+8"),
            "{p1:?}"
        );
        // 一分钟后再来一轮，照发新时刻。
        let p2 = s.env_prelude(monday + 60_000, 480, None, 0, 100_000).await;
        assert!(env_texts(&p2)[0].contains("16:38"), "{p2:?}");
    }

    /// 间隔超过阈值要显式警示；正常节奏和没有时间戳的老历史都不说。
    #[tokio::test]
    async fn 轮首间隔_大间隔警示_小间隔与无戳沉默() {
        let s = Session::new(SessionId::from_raw("s1"), std::env::temp_dir(), None);
        let now = 1_788_165_420_000u64;

        let gapped = s
            .env_prelude(now, 0, Some(now - 5 * 3_600_000), 0, 100_000)
            .await;
        assert!(
            env_texts(&gapped)[0].contains("About 5 hours have passed"),
            "{gapped:?}"
        );

        let recent = s
            .env_prelude(now, 0, Some(now - 10 * 60_000), 0, 100_000)
            .await;
        assert!(
            !env_texts(&recent)[0].contains("have passed"),
            "十分钟是正常停顿：{recent:?}"
        );

        let unstamped = s.env_prelude(now, 0, None, 0, 100_000).await;
        assert!(
            !env_texts(&unstamped)[0].contains("have passed"),
            "老 transcript 没有时间戳，不能编一个间隔出来：{unstamped:?}"
        );
    }

    /// 采样失败不再沉默：有指纹就宣告作废（只说一次），恢复后全量重发。
    /// 否则「没有新快照就是没变」会把探针断供反向背书成"一切照旧"。
    #[tokio::test]
    async fn 环境快照_采样失败宣告作废_恢复后全量重发() {
        let s = Session::new(SessionId::from_raw("s1"), std::env::temp_dir(), None);
        let a = env_snap(&[(3, "pnpm dev", true)]);
        s.attach_env(FakeEnv::script(vec![
            Some(a.clone()),
            None,
            None,
            Some(a.clone()),
        ]));

        assert_eq!(
            snap_texts(&s.env_prelude(0, 0, None, 0, 100_000).await).len(),
            1,
            "第一轮正常注入"
        );

        let outage = s.env_prelude(0, 0, None, 0, 100_000).await;
        assert!(
            env_texts(&outage)[0].contains("Environment sampling failed"),
            "断供必须宣告旧快照作废：{outage:?}"
        );
        assert!(snap_texts(&outage).is_empty());

        let outage2 = s.env_prelude(0, 0, None, 0, 100_000).await;
        assert!(
            !env_texts(&outage2)[0].contains("Environment sampling failed"),
            "连续断供只唠叨一次：{outage2:?}"
        );

        let recovered = s.env_prelude(0, 0, None, 0, 100_000).await;
        assert_eq!(
            snap_texts(&recovered).len(),
            1,
            "恢复采样后指纹已清，必须全量重发：{recovered:?}"
        );
        assert!(snap_texts(&recovered)[0].contains("[3]"), "{recovered:?}");
    }

    /// 档位只在向上越档时说一次；压缩重置后允许再说。
    #[tokio::test]
    async fn 上下文档位_越档才说_压缩后重置() {
        let s = Session::new(SessionId::from_raw("s1"), std::env::temp_dir(), None);
        let q = quiet_snap;
        s.attach_env(FakeEnv::new(vec![q(), q(), q(), q(), q()]));

        assert!(
            !env_texts(&s.env_prelude(0, 0, None, 40_000, 100_000).await)[0]
                .contains("of the context is used"),
            "50% 以下不说话"
        );
        let at72 = s.env_prelude(0, 0, None, 72_000, 100_000).await;
        assert!(
            env_texts(&at72)[0].contains("72%"),
            "越过 70 档要报实际百分比：{at72:?}"
        );
        assert!(
            !env_texts(&s.env_prelude(0, 0, None, 73_000, 100_000).await)[0]
                .contains("of the context is used"),
            "同档内不重复唠叨"
        );

        // 模拟压缩重置（compact_history 里那两行的镜像）。
        *s.env_seen.lock().await = None;
        *s.env_band.lock().await = 0;
        let after = s.env_prelude(0, 0, None, 60_000, 100_000).await;
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

        let _ = s.env_prelude(0, 0, None, 0, 100_000).await;
        // 第二轮快照文本没变（alerts 不进指纹），但告警要出来。
        let second = s.env_prelude(0, 0, None, 0, 100_000).await;
        assert!(
            snap_texts(&second).is_empty(),
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
            reminders[0].contains("do NOT comment on it"),
            "防分心护栏：{reminders:?}"
        );
    }
}
