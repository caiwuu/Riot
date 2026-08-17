//! 会话：把内核零件组装成能跑的东西。
//!
//! # 为什么内核在宿主进程内跑
//!
//! 架构文档里内核最终是独立进程（M4）。现在还不是 —— 阶段 A 它是一个
//! library，直接在 Tauri 的 tokio runtime 上跑。
//!
//! 这不是偷懒，是顺序问题：进程边界要解决的是崩溃隔离和资源限制，而在
//! 主循环的正确性还没被真实模型验证过之前，那层边界只会让每一次调试
//! 多一跳。等这里稳定了再拆，拆的时候 `AgentDeps` 的形状不用变 ——
//! 它本来就是按"能被替换"设计的。
//!
//! # 历史从事件流重建
//!
//! `run_agent` 只吐事件，不返回终态。会话历史是把 `AgentEvent::Message`
//! 攒起来得到的。这样宿主和 UI 看到的是同一份东西 —— 如果它们各自维护
//! 一份，两者的分歧只会在几十轮之后以"模型突然失忆"的形式暴露出来。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::ipc::Channel;
use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;

use riot_core::{AgentDeps, AgentState, run_agent};
use riot_permissions::RuleSet;
use riot_protocol::event::AgentEvent;
use riot_protocol::id::{IdGenerator, MessageId, NanoIdGenerator, RequestId, SessionId};
use riot_protocol::message::{Attachment, Message, MessageMeta, UserContent};
use riot_protocol::permission::{
    AskPreview, DecisionReason, GateOutcome, PermissionAsk, PermissionContext, PermissionGate,
    PermissionMode, PermissionModeState, PermissionResponse, PermissionResult, PermissionRule,
    SafetyVerdict,
};
use riot_protocol::provider::Provider;
use riot_protocol::tool::{FileStateCache, PromptContext, Tool};
use riot_providers::anthropic::request::SystemSection;
use riot_providers::{
    AnthropicConfig, AnthropicProvider, OpenAiConfig, OpenAiProvider, ReqwestTransport,
};
use riot_runtime::{MemoryFileState, SystemFs, SystemProcessRunner};
use riot_tools::registry::Registry;
use riot_tools::scheduler::Scheduler;

use crate::config::{ResolvedModel, Sampling};

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

/// 判危通过之后，等这么久再自动放行。
///
/// 存在的理由是防误触：弹窗不该在用户手指正落下的那一刻消失，把这次点击
/// 漏给底下的界面。它挡不住"看到弹窗、想两秒才点"—— 那时早放行了；挡的是
/// 判危结果和点击几乎同时到达的那一小段。
const CLASSIFY_GRACE: Duration = Duration::from_millis(200);

/// 用户随消息附上的一张图。
///
/// 只走内容不走路径:图片可能压根没有路径（从剪贴板粘的截图），而有路径的
/// 那些也要读成 base64 才能进请求 —— 统一成内容，下游少一条分支。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInput {
    /// MIME 类型，如 `image/png`。
    pub media_type: String,
    /// base64 编码的图片数据。
    pub data: String,
}

/// 读回来的一张图。字段名和 [`ImageInput`] 对齐 —— 前端读完直接原样发回来。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageOutput {
    pub media_type: String,
    pub data: String,
    /// 文件名。界面上给附件条做标签用。
    pub name: String,
}

/// 磁盘上的图片文件读进来的上限（原始字节）。
///
/// base64 之后会涨三分之一，所以这个数要比单图上限小一截。
const MAX_IMAGE_FILE: u64 = 3_500_000;

/// 读一个图片文件。
///
/// `[约束]` 类型按**扩展名**判断，而且只认这几种。不认的一律拒绝 ——
/// 把一个 PDF 当 image/png 发出去，服务方要么 400、要么解出一张坏图，
/// 而报错完全不会指向"类型判错了"。
pub async fn read_image(path: &str) -> Result<ImageOutput, String> {
    let p = std::path::Path::new(path);
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let media_type = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        other => {
            return Err(format!(
                "不支持 .{other} —— 只能附 png / jpg / gif / webp。\
                 其它文件可以用「附加文件」，模型会自己去读。"
            ));
        }
    };

    // 豁免理由：这是宿主层，读的是用户亲手选的那个文件，注入 FileSystem
    // 抽象在这里没有意义（见 clippy.toml 的说明）。
    #[allow(clippy::disallowed_methods)]
    let meta = tokio::fs::metadata(p)
        .await
        .map_err(|e| format!("读不到 {path}：{e}"))?;
    if meta.len() > MAX_IMAGE_FILE {
        return Err(format!(
            "这张图有 {} MB，太大了（上限约 {} MB）。裁剪或缩小之后再附。",
            meta.len() / 1_000_000,
            MAX_IMAGE_FILE / 1_000_000,
        ));
    }

    #[allow(clippy::disallowed_methods)]
    let bytes = tokio::fs::read(p)
        .await
        .map_err(|e| format!("读不到 {path}：{e}"))?;

    use base64::Engine as _;
    Ok(ImageOutput {
        media_type: media_type.to_owned(),
        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
        name: p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("image")
            .to_owned(),
    })
}

/// 单张图的上限（base64 后的长度）。
///
/// 各家服务方对单张图有自己的限制（Anthropic 是 5MB），超了是一个 400。
/// 在这里拦住，用户能立刻知道是哪张图太大 —— 而模型那边报回来的错只会说
/// "请求无效"。前端会先按长边缩一遍，走到这条的多半是超大截图。
const MAX_IMAGE_B64: usize = 5_000_000;

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
#[derive(Clone, Default)]
pub struct SessionSink(Arc<std::sync::Mutex<Option<Channel<AgentEvent>>>>);

impl SessionSink {
    /// 换上前端最新的那个 channel。
    pub fn attach(&self, ch: Channel<AgentEvent>) {
        *self.0.lock().expect("事件出口锁不该中毒") = Some(ch);
    }

    /// 发一个事件。`Err` = 这个会话此刻没有出口（前端从没订阅过，
    /// 或者 channel 已经废了）。
    pub fn send(&self, ev: AgentEvent) -> Result<(), SinkClosed> {
        let g = self.0.lock().expect("事件出口锁不该中毒");
        match g.as_ref() {
            Some(ch) => ch.send(ev).map_err(|_| SinkClosed),
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

/// 给前端排队面板的一条摘要。
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct QueuedSummary {
    pub id: String,
    pub text: String,
    /// 附了几张图。面板只显示个数 —— 全量 base64 回传太重。
    pub images: usize,
    /// 引用的文件路径。面板直接列出来（它们是路径，不重）。
    pub refs: Vec<String>,
}

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
/// `[约束]` 每轮现装，不缓存在会话上。用户中途填上 SearXNG 地址、给服务方
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
    browser: Option<Arc<crate::browser::access::HostBrowser>>,
    history: Mutex<Vec<Message>>,
    /// 当前这一轮的取消令牌。没有正在跑的轮次时是 None。
    running: Mutex<Option<CancellationToken>>,
    /// 跑轮中用户插话的队列。入队与否的判定必须在 `running` 锁下做
    /// （见 [`Self::try_enqueue`]），否则消息会卡在一个没人 drain 的队列里。
    queue: Arc<HostInputQueue>,
    /// 事件出口。前端每次订阅都会换掉里面的 channel，跑着的轮子跟着换。
    sink: SessionSink,
    pending_asks: Arc<PendingAsks>,
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
    /// 模型这一侧的终端面板。宿主创建会话后挂上（见 [`Self::attach_terminals`]）。
    ///
    /// 会话级而不是每轮现建：`owned` 集合记着模型起过哪些服务，每轮重建
    /// 的话上一轮起的 dev server 这一轮就不认了。
    /// 没挂上时是 `NoTerminal` —— 忘了装配的表现是工具明说"用不了"，
    /// 不是悄悄退回那条会把服务杀掉的老路。
    terminal: std::sync::OnceLock<Arc<crate::term_access::HostTerminal>>,
}

/// 标题截断规则：去空白、取前 40 个字符。
///
/// 提出来共享是因为三处要用同一条规则（自动标题、索引重建、历史推导）——
/// 各写一遍的话，重建出来的标题和原来的差一个字符宽度都算 bug。
pub(crate) fn title_excerpt(text: &str) -> Option<String> {
    let t = text.trim();
    (!t.is_empty()).then(|| t.chars().take(40).collect())
}

#[derive(Default)]
pub struct PendingAsks {
    map: Mutex<HashMap<String, oneshot::Sender<PermissionResponse>>>,
}

impl PendingAsks {
    async fn insert(&self, id: String, tx: oneshot::Sender<PermissionResponse>) {
        self.map.lock().await.insert(id, tx);
    }

    pub async fn resolve(&self, id: &str, response: PermissionResponse) -> bool {
        match self.map.lock().await.remove(id) {
            // 接收端已经走了（超时或取消）。不是错误 —— 用户在超时之后
            // 才点了按钮，这时候什么都不该发生。
            Some(tx) => tx.send(response).is_ok(),
            None => false,
        }
    }

    async fn forget(&self, id: &str) {
        self.map.lock().await.remove(id);
    }
}

/// 把用户这一轮的输入拼成消息内容。
///
/// `[约束]` 图片排在文字前面。两家服务方的文档都建议这个顺序，实测差别在
/// "先看图再读问题"和"读完问题回头找图"之间 —— 后者更容易答偏。
///
/// `[约束]` 模型收不了图片时必须**在这里**转成文字，不能把图片原样塞进历史。
/// 塞进去的话，OpenAI 那条路会把它发成一条模型看不懂的 image_url（400），
/// Anthropic 那条路会被服务方拒 —— 而两种失败都发生在用户已经按下发送之后。
async fn user_content(
    input: TurnInput,
    vision: &dyn riot_protocol::vision::VisionAccess,
    mentions: MentionCtx<'_>,
) -> Vec<UserContent> {
    let mut content = Vec::with_capacity(input.images.len() + 1);

    // `[约束]` 转述和各类说明用 SystemReminder 附件，不用 Text。
    //
    // 这些话是宿主替模型补的上下文，不是用户说的:混进 Text 的话，
    // 前端重建历史时会把整段转述当成用户气泡显示出来（实时路径看不到
    // 这个问题 —— 乐观回显只显示用户真正打的字，切回会话才暴露）。
    // 模型侧则两条路都读得到，SystemReminder 还多了"这是带外提示"的语义。
    for (i, img) in input.images.into_iter().enumerate() {
        if img.data.len() > MAX_IMAGE_B64 {
            content.push(UserContent::Attachment(Attachment::SystemReminder {
                text: format!(
                    "用户附了第 {} 张图，但它有 {} KB，超过单张上限，没有发给你。\
                     可以请用户裁剪或缩小之后再发。",
                    i + 1,
                    img.data.len() / 1024,
                ),
            }));
            continue;
        }

        if vision.accepts_images() {
            content.push(UserContent::Attachment(Attachment::Image {
                media_type: img.media_type,
                data: img.data,
            }));
            continue;
        }

        // 走视觉兼容。失败也要留一句话 —— 静默丢掉的话，用户明明附了图，
        // 模型却完全不知道有这回事，然后答得像用户什么都没给。
        let described = vision
            .describe(riot_protocol::vision::DescribeRequest {
                media_type: img.media_type,
                data: img.data,
                focus: "用户附上这张图是想让你看懂它的内容:上面的文字、界面元素、\
                        数据、以及任何看起来是报错的地方"
                    .to_owned(),
            })
            .await;
        content.push(UserContent::Attachment(Attachment::SystemReminder {
            text: match described {
                Ok(desc) => format!("用户附的第 {} 张图：\n{desc}", i + 1),
                Err(e) => format!("用户附了第 {} 张图，但没能转成文字：{e}", i + 1),
            },
        }));
    }

    // 空文本也要留个位置:用户可能只丢了一张图什么都没说，而空的 user 消息
    // 会被一部分服务方拒。
    let text_for_mentions = input.text.clone();
    content.push(UserContent::Text {
        text: if input.text.trim().is_empty() {
            "看这张图。".to_owned()
        } else {
            input.text
        },
    });
    // `@路径` 引用：用户点名的文件连内容一起带上，排在正文之后
    //（先读问题再看材料 —— 和图片相反，图片是"看着图听问题"）。
    // 两路来源：正文里手打的 @，和界面上选中的块。
    let refs = crate::mentions::merge(
        crate::mentions::parse(&text_for_mentions, mentions.cwd),
        crate::mentions::from_paths(&input.refs, mentions.cwd),
    );
    if !refs.is_empty() {
        tracing::info!(count = refs.len(), "展开 @ 文件引用");
        content.extend(
            crate::mentions::expand(&refs, mentions.file_state)
                .into_iter()
                .map(UserContent::Attachment),
        );
    }

    // UserPromptSubmit hook 的补充上下文排在最后 —— 它是对这条消息的
    // 注解，不是消息本身。
    for ctx in input.extra_context {
        content.push(UserContent::Attachment(Attachment::SystemReminder {
            text: format!("UserPromptSubmit hook 的补充上下文：\n{ctx}"),
        }));
    }
    content
}

/// `@` 引用展开要用的东西：解析相对路径的基准 + 工作集登记。
///
/// `file_state` 为 None 时不登记（测试）。登记之后模型能直接 Edit
/// 引用过的文件，不用先 Read 一遍。
#[derive(Clone, Copy)]
struct MentionCtx<'a> {
    cwd: &'a std::path::Path,
    file_state: Option<&'a dyn riot_protocol::tool::FileStateCache>,
}

/// 给一个会话装配浏览器能力。
///
/// 没打包浏览器时装 `NoBrowser` —— 工具会明确说"用不了"，而不是让宿主
/// 在启动时就失败。浏览器是可选能力，缺了它聊天和文件操作照常。
fn make_browser(id: &SessionId) -> Option<Arc<crate::browser::access::HostBrowser>> {
    let app = crate::browser::access::locate_app().or_else(|| {
        tracing::info!("没找到打包好的浏览器，Browser* 工具和面板都不可用");
        None
    })?;
    let profile =
        crate::config::profiles_dir(&crate::config::config_path()).join(id.as_str());
    Some(crate::browser::access::HostBrowser::new(app, profile))
}

impl Session {
    /// 给工具用的浏览器能力。没打包时是 `NoBrowser`，工具会明说用不了。
    fn browser(&self) -> Arc<dyn riot_protocol::browser::BrowserAccess> {
        match &self.browser {
            Some(b) => Arc::clone(b) as Arc<dyn riot_protocol::browser::BrowserAccess>,
            None => Arc::new(riot_protocol::browser::NoBrowser),
        }
    }

    /// 给面板用的浏览器。`None` = 这个构建没带浏览器。
    pub fn panel_browser(&self) -> Option<Arc<crate::browser::access::HostBrowser>> {
        self.browser.clone()
    }

    pub fn new(id: SessionId, cwd: std::path::PathBuf, persist: Option<SessionPersist>) -> Self {
        let browser = make_browser(&id);
        Self {
            id,
            cwd,
            browser,
            history: Mutex::new(Vec::new()),
            running: Mutex::new(None),
            queue: Arc::new(HostInputQueue::default()),
            sink: SessionSink::default(),
            pending_asks: Arc::new(PendingAsks::default()),
            sampling_override: Mutex::new(Sampling::default()),
            python_venv: Mutex::new(None),
            system_prompt_extra: Mutex::new(None),
            thinking_override: Mutex::new(riot_protocol::ThinkingPolicy::default()),
            rules: Arc::new(Mutex::new(Vec::new())),
            mode: Arc::new(Mutex::new(PermissionMode::Default)),
            custom_title: Mutex::new(None),
            auto_title: Mutex::new(None),
            persist,
            discovered_tools: Arc::new(std::sync::RwLock::new(std::collections::HashSet::new())),
            hydrated: tokio::sync::OnceCell::new(),
            file_state: MemoryFileState::shared(),
            ids: Arc::new(NanoIdGenerator),
            compacting: AtomicBool::new(false),
            ui_archive: Mutex::new(Vec::new()),
            terminal: std::sync::OnceLock::new(),
        }
    }

    /// 从索引恢复一个会话。历史**不在这里加载** —— 惰性水合，见 [`Self::hydrate`]。
    ///
    /// `[约束]` 权限规则（"总是允许"、渗透 scope）刻意**不恢复**，回到空。
    /// 那些授权是对着一个活着的会话给出的；跨越重启把它们静默续上，等于
    /// 用户某天的一次点击变成了永久放行 —— Claude Code 的会话级授权同样
    /// 死于会话结束。
    pub fn restored(
        meta: &crate::persist::PersistedSession,
        cwd: std::path::PathBuf,
        persist: Option<SessionPersist>,
    ) -> Self {
        let id = SessionId::from_raw(meta.id.clone());
        let browser = make_browser(&id);
        Self {
            id,
            cwd,
            browser,
            history: Mutex::new(Vec::new()),
            running: Mutex::new(None),
            queue: Arc::new(HostInputQueue::default()),
            sink: SessionSink::default(),
            pending_asks: Arc::new(PendingAsks::default()),
            sampling_override: Mutex::new(meta.sampling),
            python_venv: Mutex::new(meta.python_venv.clone()),
            system_prompt_extra: Mutex::new(meta.system_prompt.clone()),
            thinking_override: Mutex::new(meta.thinking),
            rules: Arc::new(Mutex::new(Vec::new())),
            mode: Arc::new(Mutex::new(meta.mode)),
            custom_title: Mutex::new(meta.custom_title.clone()),
            auto_title: Mutex::new(meta.auto_title.clone()),
            persist,
            discovered_tools: Arc::new(std::sync::RwLock::new(std::collections::HashSet::new())),
            hydrated: tokio::sync::OnceCell::new(),
            file_state: MemoryFileState::shared(),
            ids: Arc::new(NanoIdGenerator),
            compacting: AtomicBool::new(false),
            ui_archive: Mutex::new(Vec::new()),
            terminal: std::sync::OnceLock::new(),
        }
    }

    /// 挂上终端面板。宿主创建/恢复会话之后调一次。
    pub fn attach_terminals(&self, terms: crate::term::Terminals) {
        let _ = self.terminal.set(Arc::new(crate::term_access::HostTerminal::new(
            terms,
            self.cwd.clone(),
        )));
    }

    /// 这一轮装配给工具的终端能力。
    fn terminal(&self) -> Arc<dyn riot_protocol::terminal::TerminalAccess> {
        match self.terminal.get() {
            Some(t) => Arc::clone(t) as Arc<dyn riot_protocol::terminal::TerminalAccess>,
            None => Arc::new(riot_protocol::terminal::NoTerminal),
        }
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

    /// 中断本轮。返回是否真的有轮子在跑。
    ///
    /// `false` 给前端一个明确信号：宿主已经闲着，该把残留的停止键收掉。
    /// 只记日志的话，界面还转圈，用户连点停止也毫无反应。
    pub async fn interrupt(&self) -> bool {
        // 这条日志是"按了停止没反应"唯一能自证的地方：要么没到这里
        //（前端/命令层断了），要么到了但没有正在跑的轮子（界面 busy
        // 是假的），要么取消发出去了而下游没理它。三种病因三种药。
        match self.running.lock().await.as_ref() {
            Some(t) => {
                tracing::info!(session = %self.id.as_str(), "中断：向本轮发出取消");
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
        model: ResolvedModel,
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
            let content = user_content(
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
                    self.queue.push(QueuedEntry { id: id.clone(), input, msg });
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
                .run_locked(input, model, caps, sink.clone(), cancel, limits)
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
    pub async fn history(&self) -> Vec<Message> {
        self.hydrate().await;
        self.history.lock().await.clone()
    }

    /// 挂上前端最新的事件出口。跑着的轮子会立刻改用它。
    pub fn attach_sink(&self, ch: Channel<AgentEvent>) {
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

    fn baselines_path(&self) -> Option<std::path::PathBuf> {
        self.persist
            .as_ref()
            .map(|p| crate::changes::baselines_path(p.store.dir(), self.id.as_str()))
    }

    /// 重启后把改动基线装回内存。有 sidecar 用 sidecar；老会话没有就
    /// 从对话里的 Read / Write / Edit 推。推出来的当场落盘，下次不用再走。
    fn restore_baselines(&self, archived: &[Message], live: &[Message]) {
        let Some(path) = self.baselines_path() else { return };
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
        model: ResolvedModel,
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
        self.run_locked(input, model, caps, sink, cancel, limits).await
    }

    /// 跑一轮的主体。调用方必须已经把 `running` 置成本轮的令牌 ——
    /// 这里负责跑完、清 `running`、清残留插话。
    async fn run_locked(
        &self,
        input: TurnInput,
        model: ResolvedModel,
        caps: TurnCapabilities,
        sink: SessionSink,
        cancel: CancellationToken,
        limits: TurnLimits,
    ) -> Result<(), String> {
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
        if !leftover.is_empty() {
            tracing::debug!(count = leftover.len(), "清掉没赶上安全点的插话，前端面板接管");
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
        sandbox: Option<riot_runtime::ActiveSandbox>,
    ) -> Scheduler {
        // venv 每轮现装（和 caps 一个道理）：用户中途在会话设置里换环境，
        // 下一轮就生效。
        let proc: Arc<dyn riot_protocol::tool::ProcessRunner> = match python_venv {
            Some(v) => Arc::new(VenvRunner::new(v)),
            None => Arc::new(SystemProcessRunner::default()),
        };
        // 沙箱套在最外层：它改写的是"跑什么"（前面垫一个 sandbox-exec），
        // venv 改的是环境变量，两件事互不干涉。
        let proc: Arc<dyn riot_protocol::tool::ProcessRunner> = match sandbox {
            Some(sb) => Arc::new(riot_runtime::SandboxedRunner::new(proc, sb)),
            None => proc,
        };
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

    /// 把一段历史压成摘要：LLM 总结 + 记忆/工作集重注 + 边界落盘 + 事件。
    /// 返回 `Some(新历史)`；总结失败返回 None（调用方决定要不要声张）。
    async fn compact_history(
        &self,
        provider: &Arc<dyn Provider>,
        model: &str,
        history: &[Message],
        sink: &SessionSink,
        cancel: CancellationToken,
    ) -> Option<Vec<Message>> {
        let before = provider.count_tokens(history);
        // 先说一声再动手。下面那次总结是一个真实的模型调用，几十秒 ——
        // 期间界面上只有那三个点在动，和"模型正在回答"分不出来。
        self.compacting.store(true, Ordering::Relaxed);
        let _ = sink.send(AgentEvent::Compacting);
        let summary =
            match riot_core::summarize::summarize_history(provider, model, history, cancel).await {
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
            .map(|m| Attachment::Memory { path: m.path, content: m.content })
            .collect();
        // git 快照同理，而且重注的这份是**新的** —— 压缩前的那几十轮里
        // 分支和工作区多半已经变了，照抄旧快照比不给还糟。
        if let Some(info) = crate::git::probe(&self.cwd).await {
            memory.push(Attachment::Environment {
                text: crate::git::describe(&info),
            });
        }
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
        let _ = sink.send(AgentEvent::Compacted {
            before_tokens: before,
            after_tokens: after,
            strategy: riot_protocol::event::CompactStrategy::FullSummary,
        });
        Some(vec![msg])
    }

    /// 手动压缩（`/compact`）。空闲时才能做 —— 压缩改写历史，
    /// 不能和跑动中的轮子并发。
    pub async fn compact_now(
        &self,
        model: ResolvedModel,
        sink: SessionSink,
    ) -> Result<(), String> {
        let provider = provider_for(&model)?;
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
        let history = self.history.lock().await.clone();
        let result = if history.is_empty() {
            Err("还没有对话内容，没什么可压缩的。".to_owned())
        } else {
            match self
                .compact_history(&provider, &model.model, &history, &sink, cancel)
                .await
            {
                Some(new_history) => {
                    *self.history.lock().await = new_history;
                    Ok(())
                }
                None => Err("压缩失败，历史保持原样。稍后再试。".to_owned()),
            }
        };
        *self.running.lock().await = None;
        result
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
        input: TurnInput,
        model: ResolvedModel,
        mut caps: TurnCapabilities,
        sink: SessionSink,
        cancel: CancellationToken,
        limits: TurnLimits,
    ) -> Result<(), String> {
        let provider = provider_for(&model)?;
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
        let sandbox = limits.sandbox.policy(&self.cwd).activate();
        if sandbox.is_none() && limits.sandbox != crate::config::SandboxMode::Off {
            // 说一声。静默降级的话，用户以为自己开着沙箱。
            tracing::warn!(
                session = %self.id.as_str(),
                "这台机器上沙箱起不来（需要 macOS 的 sandbox-exec），本轮不隔离"
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

        let make_ctx = |sibling_tools: Vec<String>| PromptContext {
            cwd: self.cwd.clone(),
            platform: std::env::consts::OS.to_owned(),
            // 全部正式名。工具的 prompt 靠它写清分工（"有 X 就别用我"）。
            sibling_tools,
            // 模型对"今天"没有概念，它的年份停在训练截止那天。不注入的
            // 话它搜"最新版本"会带上一个两年前的年份，然后拿着过期结果
            // 言之凿凿。见 tools::web::date。
            today: riot_tools::tools::web::date::year_month(clock.now_ms()),
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
            ToolAssembly { registry, prompt_ctx, deferred },
            clock.clone(),
            caps,
            gate,
            python_venv.as_deref(),
            sandbox,
        );
        // PostToolUse hooks：只在真配了的时候装 —— enabled() 为 false 时
        // 调度器连 hook 参数（input 克隆）都不准备。
        let scheduler = if hook_engine.has_post_tool_use() {
            scheduler.with_hooks(Arc::new(crate::hooks::HookToolHooks(Arc::clone(&hook_engine))))
        } else {
            scheduler
        };

        let deps = AgentDeps {
            provider: Arc::clone(&provider),
            // 反应式（413）路径的完整阶梯：清旧工具结果 → LLM 总结。
            // 只挂 ClearOldResults 的话，"对话本身超长"的会话一溢出就死。
            compactor: Arc::new(riot_core::Layered::new(
                Arc::clone(&provider),
                model.model.clone(),
                Arc::clone(&self.ids) as Arc<dyn IdGenerator>,
                cancel.child_token(),
            )),
            clock: Arc::clone(&clock),
            ids: Arc::clone(&self.ids) as Arc<dyn IdGenerator>,
            tools: Arc::new(scheduler),
            queue: Arc::clone(&self.queue) as Arc<dyn riot_core::state::InputQueue>,
            // Stop hooks 接在这里（每轮现装，配置中途改了下一轮生效）。
            // 没配置任何 Stop hook 时给 NoStopGate —— 收尾零开销。
            stop_gate: stop_gate.clone(),
        };

        let mut history = self.history.lock().await.clone();

        // ── 主动压缩：历史超阈值就先总结再开工 ────────────────────
        // 反应式（413 重试）是保命；这条是"到线就处理"—— 不主动的话，
        // 会话会一直顶着窗口上限跑，每轮都在 413 的边缘反复横跳。
        // 放在追加本轮用户消息**之前**：压的是旧账，新话骑在压缩后的历史上。
        let history_tokens = provider.count_tokens(&history);
        if !history.is_empty() && history_tokens >= limits.compact_threshold_tokens {
            match self
                .compact_history(&provider, &model.model, &history, &sink, cancel.child_token())
                .await
            {
                Some(compacted) => {
                    history = compacted;
                    *self.history.lock().await = history.clone();
                }
                // 失败不拦路：继续用完整历史，真溢出时反应式路径兜底。
                None => tracing::warn!("主动压缩失败，本轮用完整历史"),
            }
        }
        let mut content = user_content(input, vision.as_ref(), self.mention_ctx()).await;
        // 记忆注入：会话的**第一条**用户消息前置 AGENTS.md（全局 + 项目）。
        // 只注入一次 —— 它随消息进历史和 transcript，往后每轮自然带着；
        // 每轮都注的话，同一份内容会在上下文里堆出 N 份。
        if history.is_empty() {
            let mut prelude = self.first_message_prelude().await;
            if !prelude.is_empty() {
                prelude.append(&mut content);
                content = prelude;
            }
        }
        let user_msg = Message::User {
            id: MessageId::from_raw(self.ids.next_id("msg")),
            content,
            meta: MessageMeta::default(),
        };
        // 边产生边追加（两家共识）：轮次结束才写盘的话，中途崩溃丢的是
        // 整轮对话；这里丢的最多是后台通道里还没落盘的几条。
        if let Some(p) = &self.persist {
            p.log.append(&user_msg);
        }
        history.push(user_msg);

        let state = AgentState::new(self.id.clone(), model.model.clone())
            .with_messages(history)
            // 再夹一次。配置加载时已经夹过（config::normalize），但这里是唯一
            // 真正用到它的地方，最后一道防线 —— 和 ask_timeout 同样的处理。
            .with_max_turns(limits.max_turns.clamp(*MAX_TURNS_RANGE.start(), *MAX_TURNS_RANGE.end()));

        let system = system_prompt(
            &self.cwd,
            python_venv.as_deref(),
            self.system_prompt_extra().await.as_deref(),
            mode,
            hook_engine.has_pre_tool_use()
                || hook_engine.has_post_tool_use()
                || hook_engine.has_stop(),
        );
        let state = AgentState {
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

        let stream = run_agent(state, deps, cancel.clone());
        futures::pin_mut!(stream);

        use futures::StreamExt;
        while let Some(ev) = stream.next().await {
            // 每轮怎么收场都记一笔。"按了停止没反应"的排查里，这条能
            // 区分"内核没收到取消"和"收到了但界面没更新"。
            if let AgentEvent::Done { reason } = &ev {
                tracing::info!(session = %self.id.as_str(), ?reason, "本轮结束");
                self.compacting.store(false, Ordering::Relaxed);
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
/// 最多 5 个、单文件 ~5k token、总量 ~25k token，字符按 4:1 折算）。
fn restored_files(file_state: &dyn riot_protocol::tool::FileStateCache) -> Vec<Attachment> {
    const MAX_FILES: usize = 5;
    const MAX_CHARS_PER_FILE: usize = 20_000;
    const MAX_TOTAL_CHARS: usize = 100_000;

    let mut total = 0usize;
    let mut out = Vec::new();
    for (path, st) in file_state.recent(MAX_FILES) {
        let mut content = st.content;
        if content.chars().count() > MAX_CHARS_PER_FILE {
            content = content.chars().take(MAX_CHARS_PER_FILE).collect();
            content.push_str("\n\n[文件超长已截断，需要完整内容用 Read 重读]");
        }
        if total + content.len() > MAX_TOTAL_CHARS {
            break;
        }
        total += content.len();
        out.push(Attachment::RestoredFile { path, content });
    }
    out
}

/// 给工具子进程注入 Python 虚拟环境的 ProcessRunner。
///
/// `ProcessSpec.env` 的语义是"覆盖这几个、其余继承"（见
/// `SystemProcessRunner`），所以 PATH 必须在这里拼完整：宿主当前的
/// PATH 前面接上 `<venv>/bin`。只补 spec 里没有的变量 —— 工具自己
/// 显式设置的同名 env 优先。
struct VenvRunner {
    inner: SystemProcessRunner,
    env: Vec<(String, String)>,
}

impl VenvRunner {
    fn new(venv: &str) -> Self {
        let bin = std::path::Path::new(venv).join(if cfg!(windows) { "Scripts" } else { "bin" });
        let sep = if cfg!(windows) { ';' } else { ':' };
        let path = match std::env::var("PATH") {
            Ok(p) => format!("{}{sep}{p}", bin.display()),
            Err(_) => bin.display().to_string(),
        };
        Self {
            inner: SystemProcessRunner::default(),
            env: vec![
                ("VIRTUAL_ENV".to_owned(), venv.to_owned()),
                ("PATH".to_owned(), path),
            ],
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
        for (k, v) in &self.env {
            if !spec.env.iter().any(|(ek, _)| ek == k) {
                spec.env.push((k.clone(), v.clone()));
            }
        }
        self.inner.run(spec, cancel).await
    }
}

/// 按配置构建 provider。会话和"测试连接"共用 —— 两处各写一遍的话，
/// 测试通过而正式请求失败（或反过来）这种事迟早发生。
pub fn provider_for(model: &ResolvedModel) -> Result<Arc<dyn Provider>, String> {
    let key = model.api_key().map_err(|e| e.to_string())?;
    let transport = Arc::new(ReqwestTransport::new().map_err(|e| e.to_string())?);
    let clock = Arc::new(riot_providers::watchdog::TokioClock);

    let sampling = riot_providers::SamplingParams {
        temperature: model.sampling.temperature,
        top_p: model.sampling.top_p,
        top_k: model.sampling.top_k,
    };

    if model.is_anthropic() {
        return Ok(Arc::new(AnthropicProvider::new(
            transport,
            clock,
            Vec::new(),
            AnthropicConfig {
                base_url: model.base_url.clone(),
                api_path: model.api_path.clone(),
                api_key: key,
                fallback_model: model.fallback_model.clone(),
                sampling,
                ..Default::default()
            },
        )));
    }

    Ok(Arc::new(OpenAiProvider::new(
        transport,
        clock,
        Vec::<SystemSection>::new(),
        OpenAiConfig {
            base_url: model.base_url.clone(),
            api_path: model.api_path.clone(),
            api_key: key,
            fallback_model: model.fallback_model.clone(),
            sampling,
            ..Default::default()
        },
    )))
}

/// 拉取服务方的可用模型列表（`GET /v1/models`，两个协议的响应
/// 恰好都是 `{"data":[{"id":...}]}`）。
///
/// 独立于 Provider trait：列模型是配置期操作，不该走流式管线。
pub async fn list_models(p: &crate::config::ProviderConfig) -> Result<Vec<String>, String> {
    let key = p.api_key().map_err(|e| e.to_string())?;
    let client = reqwest::Client::new();

    let mut ids: Vec<String> = Vec::new();
    let mut first_error: Option<String> = None;

    for url in model_list_urls(&p.base_url, &p.api_path) {
        let req = match p.protocol {
            crate::config::Protocol::Openai => client.get(&url).bearer_auth(key.clone()),
            crate::config::Protocol::Anthropic => client
                .get(&url)
                .header("x-api-key", key.clone())
                .header("anthropic-version", "2023-06-01"),
        };
        match fetch_models(req).await {
            Ok(found) => ids.extend(found),
            Err(e) => {
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }
    }

    if ids.is_empty() {
        // 一个都没问到才算失败。报第一条错 —— 它来自最规范的那个路径，
        // 而后面那个只是补充。
        return Err(first_error.unwrap_or_else(|| "服务方没有返回任何模型".to_owned()));
    }

    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// 模型清单可能在哪几个地址上。
///
/// `[约束]` 要问不止一个路径。各家的"清单"和"对话"不一定在同一层:智谱的
/// `/api/paas/v4/models` 只列 8 个模型，而 `/api/paas/v4/v1/models` 列 14 个
/// （视觉模型全在后者里），两个都通 —— 而对话**必须**走不带 `/v1` 的那个，
/// 带上就 404。
///
/// 只问一个的后果是"能用的模型在列表里看不见":用户在设置里找不到
/// `glm-4.6v`，而它明明能对话。实测过这两个路径的返回。
///
/// `[取舍]` 合并两份清单，代价是可能列出对话端点不认的模型。那个由模型弹窗
/// 里的「测试模型」兜底 —— 一次点击就能确认，比"看不见"好排查得多。
fn model_list_urls(base: &str, api_path: &str) -> Vec<String> {
    let root = base.trim().trim_end_matches('/');
    let mut urls = Vec::new();

    // 用户配了对话路径的话，清单大概率和它同一层:把接口那一段换成 models。
    // `/v1/chat/completions` → `/v1/models`。
    //
    // `[约束]` 要按**已知的接口尾巴**剥，不能只剥最后一段。OpenAI 的尾巴是
    // 两段（`chat/completions`），只剥一段会拼出 `/v1/chat/models`。
    if let Some(prefix) = strip_endpoint_tail(api_path)
        && !prefix.is_empty()
    {
        urls.push(format!("{root}/{prefix}/models"));
    }

    urls.push(riot_providers::endpoint::api_url(base, "v1", "models"));
    // 再试一次在同一个根上多接一层 `v1`（智谱那种把 OpenAI 兼容清单挂在
    // `<根>/v1/models` 的布局）。
    urls.push(format!("{root}/v1/models"));

    urls.dedup();
    // 去重要按值，不只是相邻 —— 上面三条在常见配置下会两两相同。
    let mut seen = std::collections::HashSet::new();
    urls.retain(|u| seen.insert(u.clone()));
    urls
}

/// 把对话路径末尾那个接口名剥掉，留下它所在的那一层。
///
/// 认得出的尾巴优先（两个协议各一个）；都不匹配时退回"去掉最后一段"，
/// 那对自定义网关是个合理的猜测。
fn strip_endpoint_tail(api_path: &str) -> Option<&str> {
    let p = api_path.trim().trim_start_matches('/').trim_end_matches('/');
    if p.is_empty() {
        return None;
    }
    for tail in ["chat/completions", "messages", "completions"] {
        if let Some(rest) = p.strip_suffix(tail) {
            return Some(rest.trim_end_matches('/'));
        }
    }
    p.rsplit_once('/').map(|(head, _)| head)
}

/// 发一次清单请求。
async fn fetch_models(req: reqwest::RequestBuilder) -> Result<Vec<String>, String> {
    // 等外部服务，真实时钟
    #[allow(clippy::disallowed_methods)]
    let resp = req
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("请求失败：{e}"))?;

    let status = resp.status();
    let body = resp.text().await.map_err(|e| format!("读响应失败：{e}"))?;
    if !status.is_success() {
        // 错误体里常有有用的说明（key 无效、路径不对），截断后带给用户
        let hint: String = body.chars().take(200).collect();
        return Err(format!("HTTP {status}：{hint}"));
    }

    #[derive(serde::Deserialize)]
    struct ModelEntry {
        id: String,
    }
    #[derive(serde::Deserialize)]
    struct ModelList {
        data: Vec<ModelEntry>,
    }

    let list: ModelList =
        serde_json::from_str(&body).map_err(|e| format!("响应不是模型列表：{e}"))?;
    Ok(list.data.into_iter().map(|m| m.id).collect())
}

/// 用当前配置发一个最小请求，验证 base URL、key、模型名这条链路通不通。
///
/// 这是设置页"测试连接"按钮的后端。没有它的话，配置错误的表现是
/// "发消息后转圈很久然后报一长串"—— 用户分不清是网络、key 还是模型名的锅。
pub async fn test_connection(model: &ResolvedModel) -> Result<String, String> {
    use riot_protocol::provider::{ProviderEvent, ProviderRequest};

    let provider = provider_for(model)?;
    let req = ProviderRequest {
        model: model.model.clone(),
        messages: vec![Message::User {
            id: MessageId::from_raw("msg_conn_test"),
            content: vec![UserContent::Text { text: "ping".into() }],
            meta: MessageMeta::default(),
        }],
        system: String::new(),
        tools: Vec::new(),
        // 要的是"链路通"，不是回答质量 —— 别让用户为一次握手付整段生成的钱
        max_output_tokens: Some(16),
        thinking: Default::default(),
    };

    let cancel = CancellationToken::new();
    let mut stream = provider.stream(req, cancel.clone());

    use futures::StreamExt;
    // 等的是外部服务，真实时钟。30 秒等不来第一个事件就是链路有问题。
    #[allow(clippy::disallowed_methods)]
    let verdict = tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(ev) = stream.next().await {
            match ev {
                ProviderEvent::Message(_) | ProviderEvent::Usage(_) => {
                    return Ok(());
                }
                ProviderEvent::Error(e) => return Err(format!("{e}")),
                _ => {}
            }
        }
        Err("连接中断，没有收到任何响应".to_owned())
    })
    .await;

    cancel.cancel();
    match verdict {
        Ok(Ok(())) => Ok(format!("连接正常：{} @ {}", model.model, model.base_url)),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("30 秒内没有响应。检查 base URL 和网络。".to_owned()),
    }
}

/// 一次询问的全部内容，来自 [`PermissionResult::Ask`]。
///
/// 三个字段捆在一起传是因为它们同源:都由决策链在同一处算出。拆成
/// 三个参数散着传，就给了调用点"只带一部分、剩下的现编"的机会 ——
/// `reason` 曾经就是这么被写死成 `Mode` 的。
struct AskSpec {
    message: String,
    suggestions: Vec<riot_protocol::permission::PermissionUpdate>,
    reason: DecisionReason,
}

/// 宿主侧的权限闸。
///
/// 决策链算出 allow/ask/deny，这里负责 ask 那一支 —— 弹窗、等待、超时。
struct HostGate {
    sink: SessionSink,
    pending: Arc<PendingAsks>,
    ids: Arc<dyn IdGenerator>,
    ctx: PermissionContext,
    /// 和 Session.rules 是同一份。"总是允许"写进这里，同一轮内的
    /// 下一次调用立即生效。
    rules_live: Arc<Mutex<Vec<PermissionRule>>>,
    /// 和 Session.mode 是同一份。批准计划把模式切到执行档之后，
    /// 同一轮的下一个工具调用就要按新模式判定。
    mode_live: Arc<Mutex<PermissionMode>>,
    cwd: std::path::PathBuf,
    /// 等用户回应的上限，来自配置。见 [`ASK_TIMEOUT_RANGE`]。
    ask_timeout: Duration,
    /// PreToolUse hooks。deny 一票否决、ask 强制询问、allow 只把
    /// "要问"升级成"放行" —— 内置决策链的 Deny 不可被 hook 压过。
    hooks: Arc<crate::hooks::HookEngine>,
    /// Auto 模式的判危分类器。没配便宜档模型时是
    /// [`riot_protocol::permission::NoClassifier`]（永远 Hold），
    /// Auto 模式于是退化成 Default —— 不会静默放行。
    classifier: Arc<dyn riot_protocol::permission::SafetyClassifier>,
}

#[async_trait::async_trait]
impl PermissionGate for HostGate {
    async fn check(
        &self,
        tool: &dyn Tool,
        input: &serde_json::Value,
        tool_use_id: &riot_protocol::id::ToolUseId,
        cancel: &CancellationToken,
    ) -> GateOutcome {
        // ── PreToolUse hooks 先跑 ────────────────────────────────
        // 聚合规则照 CC：deny > ask > allow。deny 直接拒（不再走决策链，
        // 理由发回模型让它换个做法）；ask / allow 记下来和决策链的结果
        // 合成。
        //
        // `[约束]` hook 的 allow **只能免掉例行询问**（Consent /
        // Unverifiable / 模式引起的那些），压不过三样东西：决策链的
        // Deny、安全检查（SafetyCheck，对 bypass 都免疫）、以及用户
        // 自己写的 ask 规则。否则一行 hooks.json 就等于把整套安全
        // 检查关掉 —— 而 hooks.json 是项目目录里的文件，clone 别人的
        // 仓库就可能带一个。
        let mut hook_allow = false;
        let mut hook_ask: Option<String> = None;
        let mut rewritten: Option<serde_json::Value> = None;
        if self.hooks.has_pre_tool_use() {
            for o in self
                .hooks
                .pre_tool_use(tool.name(), input, tool_use_id.as_str())
                .await
            {
                match o {
                    crate::hooks::Outcome::Block { reason } => {
                        return GateOutcome::Deny {
                            message: format!("PreToolUse hook 拒绝了这次调用：{reason}"),
                        };
                    }
                    crate::hooks::Outcome::Ask { reason } => hook_ask = Some(reason),
                    crate::hooks::Outcome::Allow => hook_allow = true,
                    crate::hooks::Outcome::Rewrite { input } => rewritten = Some(input),
                    crate::hooks::Outcome::Context { .. } => {}
                }
            }
        }
        // 改写后的输入从这里开始就是"这次调用"本身：判定、弹窗预览、
        // 最终执行都用它。判定看旧输入而执行跑新输入 = 按 A 授权执行 B。
        let input: &serde_json::Value = rewritten.as_ref().unwrap_or(input);

        // 每次都从共享状态取最新规则和模式，不用构建时的快照 —— 快照
        // 意味着"总是允许"和"批准计划切模式"都要到下一轮才生效。
        let rules = RuleSet::new(self.rules_live.lock().await.clone());
        let mut ctx = self.ctx.clone();
        ctx.mode = PermissionModeState(Some(*self.mode_live.lock().await));

        let decided = riot_permissions::decide(tool, input, &ctx, &rules);

        // hook 要求强制询问：除非决策链本来就要拒，一律改成问用户。
        let outcome = if let Some(reason) = hook_ask.filter(|_| !matches!(decided, PermissionResult::Deny { .. })) {
            let spec = AskSpec {
                message: format!("PreToolUse hook 要求确认：{reason}"),
                suggestions: vec![],
                reason: DecisionReason::Hook { name: "PreToolUse".into() },
            };
            self.ask(tool, input, tool_use_id, cancel, spec).await
        } else {
            match decided {
                PermissionResult::Allow { updated_input, .. } => {
                    GateOutcome::Allow { updated_input }
                }

                PermissionResult::Deny { message, .. } => GateOutcome::Deny { message },

                // Passthrough 到这里说明决策链没能定性。收敛成询问，不是放行 ——
                // 「不知道该不该」和「可以」是两回事。
                PermissionResult::Passthrough if hook_allow => {
                    GateOutcome::Allow { updated_input: None }
                }
                PermissionResult::Passthrough => {
                    let spec = AskSpec {
                        message: "需要确认这次调用".into(),
                        suggestions: vec![],
                        reason: DecisionReason::Unverifiable {
                            what: tool.name().to_owned(),
                        },
                    };
                    self.ask(tool, input, tool_use_id, cancel, spec).await
                }

                PermissionResult::Ask {
                    message,
                    suggestions,
                    reason,
                } => {
                    if hook_allow && hook_may_skip_ask(&reason) {
                        GateOutcome::Allow { updated_input: None }
                    } else {
                        let spec = AskSpec {
                            message,
                            suggestions,
                            reason,
                        };
                        self.ask(tool, input, tool_use_id, cancel, spec).await
                    }
                }
            }
        };

        // hook 的改写要跟到执行那一步。权限层自己也可能改写（给命令补
        // 安全 flag），那份更靠后、基于改写后的输入算出来的，优先。
        match (outcome, rewritten) {
            (GateOutcome::Allow { updated_input: None }, Some(r)) => {
                GateOutcome::Allow { updated_input: Some(r) }
            }
            (other, _) => other,
        }
    }
}

/// PreToolUse hook 的 allow 能不能免掉这次询问。
///
/// 能：例行询问（陌生域名的同意、静态分析看不懂的命令、模式引起的确认）
/// —— 这正是 hook 存在的意义，"我这个项目里这类操作没问题"。
///
/// 不能：安全检查（写 SSH 配置、凭证文件、命令注入……对 bypass 都免疫，
/// 更不该被一个脚本压过）和用户自己写的 ask 规则（那是用户明确要求
/// "这个必须问我"，脚本无权替他改主意）。
fn hook_may_skip_ask(reason: &DecisionReason) -> bool {
    !matches!(
        reason,
        DecisionReason::SafetyCheck { .. } | DecisionReason::Rule { .. }
    )
}

/// 把用户选中的选项写进工具输入，交给 `AskUserQuestion` 读。
///
/// 走 `updated_input` 而不是另开一条通道：权限层本来就有改写输入的权力
/// （给命令补安全 flag 用的就是它），提问的答案是同一件事的另一种用法。
///
/// 返回 None = 不改输入。空选择必须走这条路：普通的"允许一次"也经过这里，
/// 给每个工具都塞一个空的 `__chosen` 字段会让工具入参多出一个没人要的键。
fn inject_choice(
    input: &serde_json::Value,
    choice: Vec<String>,
) -> Option<serde_json::Value> {
    if choice.is_empty() {
        return None;
    }
    let mut v = input.clone();
    // 非对象的输入没法插字段。走到这里说明工具入参不成形，validate_input
    // 会在后面把它拦下 —— 这里静默不改，不要 panic。
    let obj = v.as_object_mut()?;
    obj.insert(
        riot_tools::tools::ask::CHOSEN_KEY.to_owned(),
        serde_json::Value::Array(choice.into_iter().map(serde_json::Value::String).collect()),
    );
    Some(v)
}

/// 落实"总是允许"里的 AddRule 建议。SetMode 在 [`HostGate::remember`]
/// 处理（要碰会话的 mode_live 和事件通道）；AddWorkingDirectory 仍然
/// 明确不支持 —— 扩围栏牵动的状态面更大，明确不支持好过半支持。
fn apply_remember(
    rules: &mut Vec<PermissionRule>,
    updates: Vec<riot_protocol::permission::PermissionUpdate>,
) {
    for u in updates {
        if let riot_protocol::permission::PermissionUpdate::AddRule {
            tool,
            pattern,
            decision,
            ..
        } = u
        {
            let rule = PermissionRule {
                tool,
                pattern,
                decision,
                source: riot_protocol::permission::RuleSource::Session,
            };
            if !rules.contains(&rule) {
                rules.push(rule);
            }
        }
    }
}

impl HostGate {
    async fn remember(&self, updates: Vec<riot_protocol::permission::PermissionUpdate>) {
        if updates.is_empty() {
            return;
        }
        // 模式切换先落。批准计划的场景里，模型的**下一个**工具调用就要
        // 按新模式判定 —— check() 每次都从 mode_live 现读，这里写完
        // 立即可见。
        for u in &updates {
            if let riot_protocol::permission::PermissionUpdate::SetMode { mode, .. } = u {
                *self.mode_live.lock().await = *mode;
                tracing::info!(mode = ?mode, "权限模式已切换（用户批准计划时选择）");
                // 告诉界面。不发的话 composer 还显示「规划模式」，而宿主
                // 已经按新档放行 —— 显示得比实际更严是最坏的一种错。
                let _ = self.sink.send(AgentEvent::ModeChanged { mode: *mode });
            }
        }
        apply_remember(&mut *self.rules_live.lock().await, updates);
    }

    // 等用户回应用的是真实时钟。禁用列表针对的是内核逻辑 —— 那里的时间
    // 必须可控才能做黄金回放；这里等的是人，回放里根本走不到。
    #[allow(clippy::disallowed_methods)]
    async fn ask(
        &self,
        tool: &dyn Tool,
        input: &serde_json::Value,
        tool_use_id: &riot_protocol::id::ToolUseId,
        cancel: &CancellationToken,
        // `[约束]` `reason` 必须原样来自决策链，不能在这里现编。
        // 曾经这里写死成 `Mode`，于是所有弹窗都自称"由权限模式决定"，
        // 用户看到的解释和实际原因无关：明明是写 `~/.zshrc` 触发的安全
        // 检查，弹窗说的却是模式。那种解释比没有解释更糟 —— 它把人引向
        // 去改模式设置，而改了也没用。
        spec: AskSpec,
    ) -> GateOutcome {
        let request_id = self.ids.next_id("ask");
        // 判危要看这个理由（它是安全边界的判据），而下面它会被 move 进
        // PermissionAsk —— 先留一份。
        let reason = spec.reason.clone();
        let (tx, rx) = oneshot::channel();
        self.pending.insert(request_id.clone(), tx).await;

        let ask = PermissionAsk {
            tool_use_id: tool_use_id.clone(),
            tool_name: tool.name().to_owned(),
            summary: if spec.message.trim().is_empty() {
                tool.describe(input)
            } else {
                spec.message
            },
            preview: preview_of(tool, input, &self.cwd),
            suggestions: spec.suggestions,
            reason: spec.reason,
        };

        let sent = self.sink.send(AgentEvent::PermissionRequest {
            request_id: RequestId::from_raw(request_id.clone()),
            detail: Box::new(ask),
        });

        if sent.is_err() {
            self.pending.forget(&request_id).await;
            return GateOutcome::Deny {
                message: "无法向用户请求授权（界面已断开），本次操作未执行".into(),
            };
        }

        // 计划批准不吃普通询问的超时：计划是要读的文档，几页纸读一刻钟
        // 很正常，而普通超时默认才 60 秒 —— 读到一半计划被"超时拒绝"，
        // 模型退回规划模式重新提交，用户刚读的白读。上限一小时兜底
        //（人真的走了不能让轮次永远挂着）。
        let timeout = if tool.name() == "ExitPlanMode" {
            Duration::from_secs(3600)
        } else {
            self.ask_timeout
        };

        // 这里等的是**用户**，用真实时钟而不是注入的 Clock。黄金回放里
        // 走不到这条路径（那些用例不弹窗），注入只会多一层没人用的间接。
        //
        // Auto 模式下弹窗和判危并行跑，先有结果的算（见 classify_race）。
        tokio::pin!(rx);
        if let Some(verdict) = self.classify_race(tool, input, &reason, &mut rx, cancel).await {
            self.pending.forget(&request_id).await;
            // 告诉界面这个弹窗作废了，理由是分类器 —— 不发的话它挂在那里，
            // 用户点"允许"毫无反应（操作早就放行并跑完了）。
            self.resolved(&request_id, verdict);
            return GateOutcome::Allow { updated_input: None };
        }

        let answer = tokio::select! {
            r = tokio::time::timeout(timeout, &mut rx) => r,
            _ = cancel.cancelled() => {
                self.pending.forget(&request_id).await;
                self.resolved(&request_id, DecisionReason::UserChoice { remembered: false });
                return GateOutcome::Deny { message: "用户已中断，本次操作未执行".into() };
            }
        };

        match answer {
            Ok(Ok(PermissionResponse::Allow { remember, choice })) => {
                self.remember(remember).await;
                GateOutcome::Allow {
                    updated_input: inject_choice(input, choice),
                }
            }
            Ok(Ok(PermissionResponse::Deny { message })) => GateOutcome::Deny {
                message: match message.as_deref().map(str::trim) {
                    Some(m) if !m.is_empty() => format!("用户拒绝了这次操作：{m}"),
                    _ => "用户拒绝了这次操作。换一种方式，或者问清楚再动手。".to_owned(),
                },
            },
            Ok(Err(_)) => GateOutcome::Deny {
                message: "授权请求没有得到回应，本次操作未执行".into(),
            },
            Err(_) => {
                self.pending.forget(&request_id).await;
                // 告诉界面这个弹窗已经作废。不发的话它会一直挂在那里，
                // 用户点"允许"也不会有任何反应 —— 操作早就被拒绝了。
                self.resolved(&request_id, DecisionReason::Timeout);
                // `[约束]` 超时按拒绝处理。见 ASK_TIMEOUT_RANGE 的注释。
                GateOutcome::Deny {
                    message: format!(
                        "等待授权超过 {} 秒，本次操作未执行。如果仍然需要，请重新提出。",
                        timeout.as_secs()
                    ),
                }
            }
        }
    }

    /// Auto 模式：判危与弹窗竞速。
    ///
    /// 返回 `Some(reason)` = 分类器判它安全，自动放行；`None` = 继续等用户
    /// （不是 Auto 模式、这类询问不许它判、判不准、或者用户先答了）。
    ///
    /// # 三道闸
    ///
    /// 1. **模式**：只有 [`PermissionMode::Auto`]。
    /// 2. **理由**：只有 `yields_to_bypass()` 为真的询问。安全检查和用户
    ///    亲手写的 ask 规则对它免疫 —— 和 bypass 模式共用同一个谓词，
    ///    不是另立一套。**这是整个 Auto 模式的安全边界。**
    /// 3. **工具**：只有覆盖了 `classifier_input()` 的工具。没覆盖的返回
    ///    None，等于"这个工具不打算被自动判"，照常问人。
    ///
    /// # 宽限期
    ///
    /// 拿到 Safe 之后不立刻放行，先等 [`CLASSIFY_GRACE`]。这段时间里用户
    /// 的答案仍然优先 —— 弹窗不会在他手指正落下时消失，把点击漏给底下的
    /// 界面。它挡不住"用户看到弹窗、想了两秒才点"（那时早放行了），挡的是
    /// 判危结果和点击几乎同时到达的那一小段。
    #[allow(clippy::disallowed_methods)]
    async fn classify_race(
        &self,
        tool: &dyn Tool,
        input: &serde_json::Value,
        reason: &DecisionReason,
        rx: &mut std::pin::Pin<&mut oneshot::Receiver<PermissionResponse>>,
        cancel: &CancellationToken,
    ) -> Option<DecisionReason> {
        if *self.mode_live.lock().await != PermissionMode::Auto {
            return None;
        }
        // 这一行是安全边界。改成 `true` 会让 Auto 模式能自动放行写 SSH
        // 密钥和 shell 启动脚本 —— 而全套测试里只有守着它的那几个会红。
        if !reason.yields_to_bypass() {
            return None;
        }
        let what = tool.classifier_input(input)?;

        let verdict = tokio::select! {
            v = self.classifier.judge(tool.name(), &what) => v,
            // 用户先答了：判危白跑，让下面的正常流程去收他的答案。
            _ = &mut *rx => return None,
            _ = cancel.cancelled() => return None,
        };

        let SafetyVerdict::Safe { confidence } = verdict else {
            return None;
        };

        // 宽限期。用户在这段时间里答了就算他的。
        tokio::select! {
            _ = &mut *rx => return None,
            _ = tokio::time::sleep(CLASSIFY_GRACE) => {}
        }

        tracing::info!(
            tool = tool.name(),
            confidence,
            "判危通过，自动放行（Auto 模式）"
        );
        Some(DecisionReason::Classifier { confidence })
    }

    /// 通知界面某个权限请求已经作废。发送失败无所谓 —— 那说明界面已经断开。
    fn resolved(&self, request_id: &str, reason: DecisionReason) {
        let _ = self.sink.send(AgentEvent::PermissionResolved {
            request_id: RequestId::from_raw(request_id.to_owned()),
            reason,
        });
    }
}

fn preview_of(tool: &dyn Tool, input: &serde_json::Value, cwd: &std::path::Path) -> AskPreview {
    match tool.name() {
        "Bash" => AskPreview::Command {
            command: input
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned(),
            cwd: cwd.to_path_buf(),
        },
        "Write" => {
            let content = input.get("content").and_then(|v| v.as_str()).unwrap_or_default();
            // 前 40 行够看清"要写个什么东西"，又不至于把整个文件铺进弹窗。
            const MAX_LINES: usize = 40;
            let total = content.lines().count();
            let truncated = total > MAX_LINES;
            let preview = content
                .lines()
                .take(MAX_LINES)
                .collect::<Vec<_>>()
                .join("\n");
            AskPreview::FileWrite {
                path: tool.target_path(input).unwrap_or_default(),
                bytes: content.len() as u64,
                preview,
                lines: total as u64,
                truncated,
            }
        }
        "Edit" => AskPreview::FileEdit {
            path: tool.target_path(input).unwrap_or_default(),
            diff: format!(
                "- {}\n+ {}",
                input
                    .get("old_string")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default(),
                input
                    .get("new_string")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default(),
            ),
        },
        // 模型主动提的问题：把选项原样交给界面渲染成对话里的选项卡。
        // 拆不出来（参数不成形）就退回普通描述 —— validate_input 会在
        // 权限之后把它拦下，这里不该因为参数坏了就崩。
        "AskUserQuestion" => riot_tools::tools::ask::preview_parts(input).map_or_else(
            || AskPreview::Plain { text: tool.describe(input) },
            |(question, options, allow_multiple)| AskPreview::Choice {
                question,
                options,
                allow_multiple,
            },
        ),
        // 计划批准卡显示计划**原文** —— 摘要等于让用户盲签一份实施方案。
        "ExitPlanMode" => AskPreview::Plain {
            text: input
                .get("plan")
                .and_then(|v| v.as_str())
                .unwrap_or("（计划为空 —— 这不该发生，拒绝并让模型重新提交）")
                .to_owned(),
        },
        _ => AskPreview::Plain {
            text: tool.describe(input),
        },
    }
}

fn system_prompt(
    cwd: &std::path::Path,
    python_venv: Option<&str>,
    extra: Option<&str>,
    mode: PermissionMode,
    has_hooks: bool,
) -> String {
    let mut p = format!(
        "你是 Riot——跑在用户机器上的全能智能体，Codex 的叛逆版。\n\
         编码只是你的一部分能力；你还负责调研、浏览、自动化、排查、验证，\
         以及把事情真正做完，而不是只给建议。\n\
         \n\
         工作目录：{}\n\
         平台：{}\n\
         \n\
         你能做的事包括但不限于：\n\
         - 读改代码、搜文件、跑命令、验证结果\n\
         - 用内置浏览器操作页面、核对前端与线上效果\n\
         - 联网检索与抓取，把外部信息消化进当前任务\n\
         - 跨工具串起来把目标落地，而不是停在「你可以这样」\n\
         \n\
         行为准则（每条都带着理由，理由是让你能推断没写到的情况）：\n\
         - 先搞清楚再动手。改代码前用 Read / Grep 看过相关位置，碰外部系统前\
           先确认现状 —— 基于猜测的修改错了之后，用户得先理解你改了什么\
           才能撤销，比从头做还慢。\n\
         - 一次只做被要求的事。顺手重构、顺手加注释、顺手改格式，会让 diff 里\
           混进无关改动 —— review 的人分不清哪些是任务本身、哪些是顺手，\
           只能整体不信任。\n\
         - 写代码要像周围的代码。命名、注释密度、错误处理方式都跟着现有风格走 —— \
           风格突变会让后来的维护者以为这里有特殊原因，白花时间考古。\n\
         - 自主性按后果分档。可逆的操作（改文件、跑测试、装依赖）直接做完再汇报，\
           停下来问「要继续吗」只是让用户干等；破坏性操作（删数据、覆盖未提交的\
           改动、对外发布）和真正的需求歧义才停下来确认 —— 这两类猜错了没法撤销。\n\
         - 工具失败时先读错误信息再动作，不要换个参数重试同一件事 —— \
           错误没消化，重试只是把同一堵墙撞第二遍。\n\
         - 多步任务用 TodoWrite 拆解和跟踪：做完一项立刻标记完成，不要攒一批再改 —— \
           清单是用户看进度的窗口，攒着改等于窗口失真。\n\
         - 说「做完了」之前先验证：能编译的编译，能跑的跑一遍 —— \
           没验证过的「完成」是把调试成本转嫁给用户。测试没过就如实报告，\
           不要粉饰成完成。\n\
         - 不要擅自提交。`git commit` 只在用户明确要求时做 —— 他多半想先\
           看看改了什么；同理不要擅自 push、切分支、stash、reset。\n\
         - 自我介绍时不要把自己缩成「编程助手」；你是全能智能体。\n\
         \n\
         引用仓库里**已有**的代码时，代码块的语言位置写成 `起始行:结束行:路径`：\n\
         \n\
         ```12:14:src/main.rs\n\
         fn main() {{\n\
             run();\n\
         }}\n\
         ```\n\
         \n\
         界面会把它渲染成带路径标题、点一下能打开文件的块。\
         路径按工作目录的相对路径写，行号照文件里的实际行号。\
         你**新写的**代码不要用这个格式 —— 那是普通代码块（写语言名，如 ```rust），\
         两者在界面上是不同的东西：前者是「去看这里」，后者是「这是我建议加的」。\n\
         \n\
         流程图、时序图、状态图用 mermaid 围栏直接写在回复里：\n\
         \n\
         ```mermaid\n\
         flowchart LR\n\
             A --> B\n\
         ```\n\
         \n\
         界面会把它画成图。不要为了给人看图去写 HTML、引 mermaid.js、再打开浏览器 —— \
         浏览器是用来核对自己改过的页面，不是当画板。\n\
         \n\
         回答用中文。代码和标识符保持原文。",
        cwd.display(),
        std::env::consts::OS,
    );
    // 不告诉模型的话，它多半会自己 source activate 或者另建一个 venv ——
    // 前者没必要，后者直接绕开了用户指定的环境。
    if let Some(venv) = python_venv {
        p.push_str(&format!(
            "\n\nPython 虚拟环境：{venv}\n\
             已注入 PATH 和 VIRTUAL_ENV，python / pip 直接就是这个环境的，\
             不要 source activate，也不要另建虚拟环境。"
        ));
    }
    // 只在真配了 hooks 时说。没配的用户读到"检查脚本"只会困惑，
    // 而且这段话每轮都在上下文里占位置。
    if has_hooks {
        p.push_str(
            "\n\n这个项目配了检查脚本（hooks）：工具调用前后、以及你想收尾时，\
             用户写的脚本会检查一遍。它们的反馈以 system-reminder 出现，\
             **当成用户本人的意见对待** —— 被拦下时不要重试同一个动作，\
             而是按反馈调整做法；说「测试没过」就去修，不要绕过检查。",
        );
    }
    if let Some(extra) = extra {
        p.push_str(&format!("\n\n用户为这个会话补充的指令：\n{extra}"));
    }
    // 规划模式的段落放在**最后**：它是本轮最强的行为约束，离对话越近
    // 权重越高。措辞对照 Claude Code 的 plan_mode 注入（"MUST NOT ...
    // supercedes any other instructions"），那句硬约束是整个模式的地基。
    if mode == PermissionMode::Plan {
        p.push_str(
            "\n\n当前处于规划模式：用户还不希望你动手。禁止一切修改 —— \
             编辑文件、执行会产生副作用的命令、改配置、提交，全部不行；\
             这条约束压过你收到的其它所有指令。\n\
             现在该做的：\n\
             1. 用只读工具（Read / Grep / Glob / WebSearch / WebFetch）把现状摸清楚；\n\
             2. 想清楚方案：动哪些文件、什么顺序、怎么验证、有什么权衡；\n\
             3. 计划成熟后，调用 ExitPlanMode 工具提交计划全文（Markdown），等待用户批准。\n\
             不要用普通回复问「这个计划可以吗？」「要开始吗？」—— \
             提交计划是征求批准的唯一方式，批准后规划模式自动退出。",
        );
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 图片能看的模型:图片原样进消息，而且排在文字前面。
    #[tokio::test]
    async fn 能看图时图片在文字前面() {
        struct Direct;
        #[async_trait::async_trait]
        impl riot_protocol::vision::VisionAccess for Direct {
            fn accepts_images(&self) -> bool {
                true
            }
            async fn describe(
                &self,
                _r: riot_protocol::vision::DescribeRequest,
            ) -> Result<String, riot_protocol::vision::VisionError> {
                panic!("能看图就不该来转述")
            }
        }

        let content = user_content(
            TurnInput {
                text: "这里为什么错位".into(),
                images: vec![ImageInput {
                    media_type: "image/png".into(),
                    data: "AAAA".into(),
                }],
                ..Default::default()
            },
            &Direct,
            no_mentions(),
        )
        .await;

        assert!(
            matches!(
                content.first(),
                Some(UserContent::Attachment(Attachment::Image { data, .. })) if data == "AAAA"
            ),
            "图片该排在最前：{content:?}"
        );
        assert!(matches!(
            content.last(),
            Some(UserContent::Text { text }) if text == "这里为什么错位"
        ));
    }

    /// 看不了图的模型:图片转成文字，**不能**把图片留在消息里。
    ///
    /// `[约束]` 留在里面的话，OpenAI 那条路会发出一条模型看不懂的 image_url，
    /// Anthropic 那条会被服务方拒 —— 而两种失败都发生在用户已经点了发送之后。
    #[tokio::test]
    async fn 看不了图时转成文字() {
        struct Compat;
        #[async_trait::async_trait]
        impl riot_protocol::vision::VisionAccess for Compat {
            fn accepts_images(&self) -> bool {
                false
            }
            async fn describe(
                &self,
                _r: riot_protocol::vision::DescribeRequest,
            ) -> Result<String, riot_protocol::vision::VisionError> {
                Ok("图里是一个两栏布局".into())
            }
        }

        let content = user_content(
            TurnInput {
                text: String::new(),
                images: vec![ImageInput {
                    media_type: "image/png".into(),
                    data: "AAAA".into(),
                }],
                ..Default::default()
            },
            &Compat,
            no_mentions(),
        )
        .await;

        assert!(
            !content
                .iter()
                .any(|c| matches!(c, UserContent::Attachment(Attachment::Image { .. }))),
            "不能把图片留在消息里：{content:?}"
        );
        // 转述进 SystemReminder 而不是 Text:它是宿主补的上下文，不是用户
        // 说的话。混进 Text 的话，前端重建历史时会把整段转述当成用户气泡。
        assert!(
            content.iter().any(|c| matches!(
                c,
                UserContent::Attachment(Attachment::SystemReminder { text })
                    if text.contains("两栏")
            )),
            "转述要以 SystemReminder 附件带上：{content:?}"
        );
        // 只丢了图什么都没说时，也得有一句话 —— 空 user 消息会被一部分
        // 服务方拒。
        assert!(
            content.iter().any(|c| matches!(
                c, UserContent::Text { text } if text.contains("看这张图")
            )),
            "空文本要补一句：{content:?}"
        );
    }

    /// 超大图不发出去，但要告诉模型"有这么回事"。
    #[tokio::test]
    async fn 超大图被拦下并留一句说明() {
        struct Direct;
        #[async_trait::async_trait]
        impl riot_protocol::vision::VisionAccess for Direct {
            fn accepts_images(&self) -> bool {
                true
            }
            async fn describe(
                &self,
                _r: riot_protocol::vision::DescribeRequest,
            ) -> Result<String, riot_protocol::vision::VisionError> {
                unreachable!()
            }
        }

        let content = user_content(
            TurnInput {
                text: "看图".into(),
                images: vec![ImageInput {
                    media_type: "image/png".into(),
                    data: "x".repeat(MAX_IMAGE_B64 + 1),
                }],
                ..Default::default()
            },
            &Direct,
            no_mentions(),
        )
        .await;

        assert!(
            !content
                .iter()
                .any(|c| matches!(c, UserContent::Attachment(Attachment::Image { .. }))),
            "超限的图不该发出去"
        );
        assert!(
            content.iter().any(|c| matches!(
                c,
                UserContent::Attachment(Attachment::SystemReminder { text })
                    if text.contains("超过单张上限")
            )),
            "要留一句说明，否则模型以为用户什么都没给：{content:?}"
        );
    }

    /// 拉模型清单要问两个路径。
    ///
    /// `[约束]` 这条盯的是"能用的模型在列表里看不见"。智谱把 OpenAI 兼容的
    /// 清单挂在 `<根>/v1/models`（14 个，视觉模型全在里面），而它自己的
    /// `<根>/models` 只有 8 个 —— 对话却必须走不带 `/v1` 的根。只问一个路径，
    /// 用户就永远找不到 `glm-4.6v`，而那个模型明明能对话。
    #[test]
    fn 模型清单问两个路径() {
        let urls = model_list_urls("https://open.bigmodel.cn/api/paas/v4", "");
        assert_eq!(
            urls,
            vec![
                "https://open.bigmodel.cn/api/paas/v4/models".to_owned(),
                "https://open.bigmodel.cn/api/paas/v4/v1/models".to_owned(),
            ]
        );

        // 只有主机名时两条会撞成同一个地址，那就只问一次 —— 同一个请求发两遍
        // 只是白等一次超时。
        assert_eq!(
            model_list_urls("https://api.deepseek.com", ""),
            vec!["https://api.deepseek.com/v1/models".to_owned()]
        );
        // 尾斜杠不该产生双斜杠，有些网关把 `//` 当成另一个路径。
        assert_eq!(
            model_list_urls("https://api.deepseek.com/", ""),
            vec!["https://api.deepseek.com/v1/models".to_owned()]
        );
    }

    /// 用户配了对话路径时，清单先按同一层去问。
    ///
    /// `[约束]` 自建网关常常把两个接口挂在同一个前缀下（`/openai/v1/...`），
    /// 而那个前缀我们猜不出来。不跟着用户配的路径走的话，他明明能对话，
    /// 「从 API 获取」却一直失败。
    #[test]
    fn 配了路径时清单跟着同一层去问() {
        let urls = model_list_urls("https://gw.test", "/openai/v1/chat/completions");
        assert_eq!(urls[0], "https://gw.test/openai/v1/models");
        // 后面两条兜底照旧留着 —— 有些网关的清单确实不在那一层。
        assert!(urls.contains(&"https://gw.test/v1/models".to_owned()));

        // 路径只有一段时没有"上一层"，跳过它别拼出 `//models`。
        let urls = model_list_urls("https://gw.test/api", "/completions");
        assert!(
            urls.iter().all(|u| !u.contains("//models")),
            "不该拼出双斜杠：{urls:?}"
        );
    }

    /// 只认几种图片扩展名。
    ///
    /// `[约束]` 把 PDF 当 image/png 发出去，服务方要么 400、要么解出一张
    /// 坏图，而报错完全不指向"类型判错了"。
    #[tokio::test]
    async fn 不认识的扩展名直接拒() {
        let e = read_image("/tmp/whatever.pdf").await.expect_err("该拒");
        assert!(e.contains("png"), "报错要说清能附什么：{e}");
        // 文件压根不存在也是这个结论 —— 扩展名先判，省一次磁盘访问。
        assert!(!e.contains("读不到"), "不该先去读盘：{e}");
    }

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
        let clamp = |v: u32| {
            u64::from(v).clamp(*ASK_TIMEOUT_RANGE.start(), *ASK_TIMEOUT_RANGE.end())
        };
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
        let AskPreview::Choice { question, options, allow_multiple } = p else {
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
        assert!(text.contains("磁盘"), "模型要收到用户点的那个 label：{text}");
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

    #[tokio::test]
    async fn 回应不存在的请求不会崩() {
        // 用户在超时之后才点按钮，这时候什么都不该发生
        let p = PendingAsks::default();
        assert!(
            !p.resolve("nope", PermissionResponse::Allow { remember: vec![], choice: vec![] })
                .await
        );
    }

    #[tokio::test]
    async fn 回应之后请求就被摘掉了() {
        let p = PendingAsks::default();
        let (tx, rx) = oneshot::channel();
        p.insert("a1".into(), tx).await;

        assert!(
            p.resolve("a1", PermissionResponse::Allow { remember: vec![], choice: vec![] })
                .await
        );
        assert!(rx.await.is_ok());
        // 第二次应该找不到 —— 否则重复点击会让同一个操作跑两遍
        assert!(
            !p.resolve("a1", PermissionResponse::Allow { remember: vec![], choice: vec![] })
                .await
        );
    }

    #[test]
    fn 系统提示里带上工作目录() {
        // 没有它模型会用相对路径乱猜
        let p = system_prompt(
            std::path::Path::new("/tmp/proj"),
            None,
            None,
            PermissionMode::Default,
            false,
        );
        assert!(p.contains("/tmp/proj"));
        assert!(!p.contains("规划模式"), "默认模式不该带规划段落");
    }

    /// 代码引用的格式约定必须在提示词里，而且要说清和普通代码块的区别。
    ///
    /// 只在前端实现渲染是没用的：模型不知道有这个格式就永远不会产出它，
    /// 那段渲染代码等于死代码。而不说清区别的话，它会把新写的代码也标上
    /// 行号和路径 —— 用户点开发现文件里根本不是那样。
    #[test]
    fn 提示词里有代码引用的格式约定() {
        let p = system_prompt(
            std::path::Path::new("/tmp/proj"),
            None,
            None,
            PermissionMode::Default,
            false,
        );
        assert!(p.contains("起始行:结束行:路径"), "要给出格式");
        assert!(p.contains("```12:14:src/main.rs"), "要给一个具体例子");
        assert!(p.contains("新写的"), "要说清新代码不用这个格式");
    }

    /// mermaid 围栏能画成图这件事必须写进提示词。
    ///
    /// 只在前端接渲染、不告诉模型的话，它会写一个 HTML 再打开浏览器
    /// 「测效果」—— 用户要的是对话里的图，不是多出来的测试页。
    #[test]
    fn 提示词里有_mermaid_围栏会画成图() {
        let p = system_prompt(
            std::path::Path::new("/tmp/proj"),
            None,
            None,
            PermissionMode::Default,
            false,
        );
        assert!(p.contains("```mermaid"), "要给出围栏写法");
        assert!(p.contains("不要为了给人看图"), "要禁止借浏览器当画板");
    }

    #[test]
    fn 会话设置会附加进系统提示() {
        // venv 不进提示词的话，模型会自己 source activate 或另建环境；
        // 追加提示词必须是**追加** —— 替换掉内置提示词等于丢了 cwd。
        let p = system_prompt(
            std::path::Path::new("/tmp/proj"),
            Some("/tmp/proj/.venv"),
            Some("测试要跑 pytest -x"),
            PermissionMode::Default,
            false,
        );
        assert!(p.contains("/tmp/proj"), "内置部分必须还在");
        assert!(p.contains("/tmp/proj/.venv"));
        assert!(p.contains("pytest -x"));
    }

    /// 自主性必须按后果分档，不能只写一句「不确定就问」。
    ///
    /// 裸的「不确定就问」会让模型向保守面倒：改个文件也停下来问「要继续吗」，
    /// 用户干等。拆成可逆/破坏性两档后，模型能推断没列举到的操作该归哪档。
    #[test]
    fn 自主性按后果分档() {
        let p = system_prompt(
            std::path::Path::new("/tmp/proj"),
            None,
            None,
            PermissionMode::Default,
            false,
        );
        assert!(p.contains("可逆"), "可逆操作要直接做完");
        assert!(p.contains("破坏性"), "破坏性操作才停下来确认");
    }

    /// 「做完了」之前必须验证，且不许粉饰失败。
    ///
    /// 不写这条的话，模型倾向于改完就宣布完成 —— 编译错误留给用户发现，
    /// 等于把调试成本转嫁出去；测试失败时还可能措辞含糊地带过。
    #[test]
    fn 声称完成前要先验证() {
        let p = system_prompt(
            std::path::Path::new("/tmp/proj"),
            None,
            None,
            PermissionMode::Default,
            false,
        );
        assert!(p.contains("先验证"), "要求完成前验证");
        assert!(p.contains("如实报告"), "失败不许粉饰");
    }

    #[test]
    fn 配了_hooks_才说怎么对待检查反馈() {
        // 不说的话模型会把 hook 的"测试没过"当成一次偶然失败去重试同一
        // 个动作；而没配 hooks 的用户读到这段只会困惑，还每轮占上下文。
        let path = std::path::Path::new("/tmp/proj");
        let with = system_prompt(path, None, None, PermissionMode::Default, true);
        assert!(with.contains("hooks"), "配了就要说明反馈怎么对待");
        let without = system_prompt(path, None, None, PermissionMode::Default, false);
        assert!(!without.contains("hooks"), "没配就别占上下文");
    }

    #[test]
    fn 规划模式的段落押最后且指路出口() {
        // 不注入的话模型不知道自己在规划模式：它会正常动手，然后每个
        // 写操作都被拒，看起来像权限系统坏了。段落必须指路 ExitPlanMode ——
        // 否则计划写完了模型不知道怎么提交，用户只能干等。
        let p = system_prompt(
            std::path::Path::new("/tmp/proj"),
            None,
            Some("补充指令"),
            PermissionMode::Plan,
            false,
        );
        assert!(p.contains("规划模式"));
        assert!(p.contains("ExitPlanMode"), "必须指路出口工具");
        assert!(
            p.rfind("规划模式").expect("有") > p.rfind("补充指令").expect("有"),
            "规划段落要押最后 —— 它是本轮最强的约束，离对话越近权重越高"
        );
    }

    #[test]
    fn venv_runner_拼出完整的环境() {
        // PATH 必须完整拼出来：ProcessSpec.env 是"覆盖这几个、其余继承"，
        // 只放 <venv>/bin 的话子进程连 bash 都找不到。
        let r = VenvRunner::new("/tmp/venv");
        let path = r
            .env
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.clone())
            .expect("有 PATH");
        assert!(path.starts_with("/tmp/venv/bin"), "venv 的 bin 要排最前：{path}");
        assert!(
            r.env.iter().any(|(k, v)| k == "VIRTUAL_ENV" && v == "/tmp/venv"),
            "要带 VIRTUAL_ENV"
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
        let s = SessionSink::default();
        s.attach(Channel::new(|_| Ok(())));
        s
    }

    /// 不解析 @ 引用的上下文（图片相关的用例不碰文件）。cwd 指一个
    /// 不存在的目录，万一测试文本里出现 @ 也读不到东西。
    fn no_mentions() -> MentionCtx<'static> {
        MentionCtx { cwd: std::path::Path::new("/nonexistent-mentions"), file_state: None }
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
            &DecisionReason::Consent { what: "跑一条命令".into() },
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
            write.classifier_input(&serde_json::json!({ "path": "a.txt" })).is_none(),
            "前提变了：Write 现在交了判定文本，这个用例要重写"
        );
        assert!(
            race(
                &gate,
                write.as_ref(),
                &serde_json::json!({ "path": "a.txt", "content": "x" }),
                &DecisionReason::Consent { what: "写文件".into() },
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
        let s = Session::new(SessionId::from_raw("s1"), std::path::PathBuf::from("/tmp"), None);
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
                assert!(message.contains("这个目录不许读"), "理由要带给模型：{message}")
            }
            GateOutcome::Allow { .. } => panic!("hook 说了不行，不能放行"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pretooluse_的_allow_把要问变成放行() {
        // Read 在默认模式下本来是允许的，换个真会问的：Bash 带变量展开
        // （Unverifiable）。hook 说 allow 就不该再弹窗。
        let s = Session::new(SessionId::from_raw("s1"), std::path::PathBuf::from("/tmp"), None);
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
        assert!(matches!(outcome, GateOutcome::Allow { .. }), "hook 放行后不该再问");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hook_的_allow_压不过安全检查() {
        // 安全边界优先于用户脚本。反过来就意味着一行 hooks.json 能把整套
        // 安全检查关掉 —— 而 hooks.json 躺在项目目录里，clone 别人的仓库
        // 就可能带一个。这里用"写 SSH 私钥"这种对 bypass 都免疫的操作。
        let s = Session::new(SessionId::from_raw("s1"), std::path::PathBuf::from("/tmp"), None);
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
        let s = Session::new(SessionId::from_raw("s1"), std::path::PathBuf::from("/tmp"), None);
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
        assert!(hook_may_skip_ask(&DecisionReason::Consent { what: "example.com".into() }));
        assert!(hook_may_skip_ask(&DecisionReason::Unverifiable { what: "Bash".into() }));
        assert!(hook_may_skip_ask(&DecisionReason::Mode { mode: PermissionMode::Default }));
        assert!(!hook_may_skip_ask(&DecisionReason::SafetyCheck {
            safety: riot_protocol::permission::SafetyKind::SshConfig
        }));
        assert!(!hook_may_skip_ask(&DecisionReason::Rule {
            source: riot_protocol::permission::RuleSource::Session,
            pattern: "Bash(rm *)".into(),
        }));
    }

    #[tokio::test]
    async fn 装配好的调度器带齐权限闸围栏和联网() {
        // 这三样每漏一个都编译得过、跑得起来，只是行为悄悄降级：
        // 漏权限闸 = 所有操作不再询问；漏围栏 = 什么文件都写不了；
        // 漏联网 = WebFetch/WebSearch 一律说"未配置"。
        let s = Session::new(SessionId::from_raw("s1"), std::path::PathBuf::from("/tmp"), None);

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
                registry: Arc::new(
                    Registry::new(riot_tools::tools::builtin()).expect("注册表"),
                ),
                prompt_ctx: PromptContext {
                    cwd: s.cwd.clone(),
                    platform: "test".into(),
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
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
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
            !s.first_message_prelude().await.iter().any(|c| matches!(
                c,
                UserContent::Attachment(Attachment::Environment { .. })
            )),
            "非 git 目录不该有环境快照"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn 同一会话不允许并发两轮() {
        let s = Session::new(SessionId::from_raw("s1"), std::path::PathBuf::from("/tmp"), None);
        let model = ResolvedModel {
            protocol: crate::config::Protocol::Openai,
            base_url: "https://api.deepseek.com".into(),
            api_path: String::new(),
            api_key_env: "RIOT_NOT_SET".into(),
            model: "deepseek-chat".into(),
            fallback_model: None,
            sampling: Sampling::default(),
        };
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
        let input = TurnInput { text: "hi".into(), ..Default::default() };
        let limits = TurnLimits {
            ask_timeout_secs: 60,
            max_turns: 48,
            compact_threshold_tokens: 100_000,
            sandbox: crate::config::SandboxMode::Off,
        };
        let _ = s.run_turn(input, model, caps, ch, limits).await;
        assert!(s.running.lock().await.is_none(), "失败路径没有清理 running");
    }

    fn test_model() -> ResolvedModel {
        ResolvedModel {
            protocol: crate::config::Protocol::Openai,
            base_url: "https://api.deepseek.com".into(),
            api_path: String::new(),
            api_key_env: "RIOT_NOT_SET".into(),
            model: "deepseek-chat".into(),
            fallback_model: None,
            sampling: Sampling::default(),
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
            input: TurnInput { text: text.into(), ..Default::default() },
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
        let s = Session::new(SessionId::from_raw("s1"), std::path::PathBuf::from("/tmp"), None);
        assert!(s.history().await.is_empty());
        assert!(!s.is_running().await, "还没开轮");

        // 模拟轮子跑起来：置位 running + 逐条追加（run_locked 的两件事）。
        *s.running.lock().await = Some(CancellationToken::new());
        s.history.lock().await.push(Message::User {
            id: MessageId::from_raw("m1"),
            content: vec![UserContent::Text { text: "打开这个文件".into() }],
            meta: MessageMeta::default(),
        });

        assert_eq!(s.history().await.len(), 1, "跑到一半也要读得到");
        assert!(s.is_running().await, "切回来时要能看出还在跑，否则停止键没了");
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

        let input = TurnInput { text: "插一句".into(), ..Default::default() };
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
        let s = Session::new(SessionId::from_raw("s1"), std::path::PathBuf::from("/tmp"), None);
        s.queue.push(queued_entry("m_q1", "还有这个也改一下"));

        // 缺 key，这一轮立刻失败 —— 正是"没走到 drain 点"的那类收尾。
        let input = TurnInput { text: "hi".into(), ..Default::default() };
        let ch = test_sink();
        let _ = s.run_turn(input, test_model(), test_caps(), ch, test_limits()).await;

        assert!(s.queue_snapshot().is_empty(), "残留插话该被清空");
        assert!(s.running.lock().await.is_none(), "running 该清干净");
    }

    #[tokio::test]
    async fn 第一句话定下自动标题且只定一次() {
        // 自动标题是缓存的，不再每次从历史推导 —— 历史是惰性水合的，
        // 启动画侧边栏时它还没加载。
        let s = Session::new(SessionId::from_raw("s1"), std::path::PathBuf::from("/tmp"), None);
        assert_eq!(s.title().await, None, "还没说过话");

        assert!(s.note_first_prompt("  你好，世界  ").await, "第一句要触发索引落盘");
        assert_eq!(s.title().await.as_deref(), Some("你好，世界"));

        assert!(!s.note_first_prompt("第二句").await, "标题只定一次");
        assert_eq!(s.title().await.as_deref(), Some("你好，世界"));

        // 手动标题优先，清除后回退到自动标题
        s.set_title(Some("手动名".into())).await;
        assert_eq!(s.title().await.as_deref(), Some("手动名"));
        s.set_title(None).await;
        assert_eq!(s.title().await.as_deref(), Some("你好，世界"));
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
}
