//! 工具替身。
//!
//! `[约束]` 替身**不该比真实实现更宽容**。上一轮在 provider 层踩过这个坑：
//! `ScriptedTransport` 在脚本耗尽时返回可重试的传输错误，于是 provider 一直
//! 空转到次数上限，而"重试了几次"这个断言就永远测不准。
//!
//! 所以这里的 [`FakeTool`] 严格遵守 [`Tool`] 的契约：fail-closed 默认值
//! 照搬、`call` 永不 panic、取消一定响应。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use riot_protocol::event::ProgressPayload;
use riot_protocol::tool::{
    InterruptBehavior, PromptContext, ResultBudget, Tool, ToolContext, ToolOutcome,
};

/// 可配置行为的假工具。
pub struct FakeTool {
    name: &'static str,
    aliases: Vec<&'static str>,
    safety: Safety,
    behavior: Behavior,
    cascades: bool,
    /// 跑了几次。用来断言"级联真的阻止了执行"。
    runs: Arc<AtomicUsize>,
}

#[derive(Clone, Copy)]
enum Safety {
    AlwaysSafe,
    NeverSafe,
    /// 按 `input.command` 判定：以 ls / cat / git status 开头算只读。
    ByCommand,
    /// 判定时 panic。测 fail-closed。
    Panics,
}

#[derive(Clone)]
enum Behavior {
    Ok(String),
    Fail(String),
    /// 先睡一会再成功。用来制造"慢工具先入队"的保序场景。
    SlowOk {
        text: String,
        delay: Duration,
    },
    /// 吐几条进度再成功。
    Progress {
        lines: Vec<String>,
        text: String,
    },
    /// 永不返回，只能被取消。
    Hangs,
}

impl FakeTool {
    fn base(name: &'static str, safety: Safety) -> Self {
        Self {
            name,
            aliases: Vec::new(),
            safety,
            behavior: Behavior::Ok(format!("{name} 完成")),
            cascades: false,
            runs: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// 只读工具，总是可并行。
    pub fn read_only(name: &'static str) -> Self {
        Self::base(name, Safety::AlwaysSafe)
    }

    /// 写工具，从不并行。
    pub fn writer(name: &'static str) -> Self {
        Self::base(name, Safety::NeverSafe)
    }

    /// 按输入判定的工具（模拟 Bash）。
    pub fn conditional(name: &'static str) -> Self {
        Self::base(name, Safety::ByCommand)
    }

    /// 判定函数会 panic 的工具。
    pub fn panicking(name: &'static str) -> Self {
        Self::base(name, Safety::Panics)
    }

    pub fn with_aliases(mut self, aliases: &[&'static str]) -> Self {
        self.aliases = aliases.to_vec();
        self
    }

    pub fn failing(mut self, msg: impl Into<String>) -> Self {
        self.behavior = Behavior::Fail(msg.into());
        self
    }

    pub fn slow(mut self, ms: u64) -> Self {
        self.behavior = Behavior::SlowOk {
            text: format!("{} 完成", self.name),
            delay: Duration::from_millis(ms),
        };
        self
    }

    pub fn with_progress(mut self, lines: &[&str]) -> Self {
        self.behavior = Behavior::Progress {
            lines: lines.iter().map(|s| (*s).to_owned()).collect(),
            text: format!("{} 完成", self.name),
        };
        self
    }

    pub fn hanging(mut self) -> Self {
        self.behavior = Behavior::Hangs;
        self
    }

    /// 失败时级联取消兄弟（模拟 Bash）。
    pub fn cascading(mut self) -> Self {
        self.cascades = true;
        self
    }

    /// 共享的执行计数器。测"级联真的阻止了执行"要用。
    pub fn counter(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.runs)
    }
}

#[async_trait]
impl Tool for FakeTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::json_schema!({ "type": "object" })
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        format!("{} 的说明", self.name)
    }

    fn describe(&self, _input: &serde_json::Value) -> String {
        format!("运行 {}", self.name)
    }

    async fn call(&self, _input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        self.runs.fetch_add(1, Ordering::SeqCst);

        match &self.behavior {
            Behavior::Ok(text) => ToolOutcome::ok_text(text.clone()),

            Behavior::Fail(msg) => ToolOutcome::failed(msg.clone()),

            Behavior::SlowOk { text, delay } => {
                // 取消必须能打断等待，否则中断要等最慢的工具跑完
                tokio::select! {
                    _ = wall_sleep(*delay) => ToolOutcome::ok_text(text.clone()),
                    _ = ctx.cancel.cancelled() => ToolOutcome::Cancelled,
                }
            }

            Behavior::Progress { lines, text } => {
                for line in lines {
                    ctx.progress
                        .send(ProgressPayload::Status { text: line.clone() });
                }
                ToolOutcome::ok_text(text.clone())
            }

            Behavior::Hangs => {
                ctx.cancel.cancelled().await;
                ToolOutcome::Cancelled
            }
        }
    }

    fn is_read_only(&self, input: &serde_json::Value) -> bool {
        self.is_concurrency_safe(input)
    }

    fn is_concurrency_safe(&self, input: &serde_json::Value) -> bool {
        match self.safety {
            Safety::AlwaysSafe => true,
            Safety::NeverSafe => false,
            Safety::ByCommand => {
                let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
                ["ls", "cat", "git status", "pwd"]
                    .iter()
                    .any(|p| cmd.trim_start().starts_with(p))
            }
            Safety::Panics => panic!("这个工具的判定函数会炸"),
        }
    }

    fn interrupt_behavior(&self) -> InterruptBehavior {
        InterruptBehavior::Cancel
    }

    fn result_budget(&self) -> ResultBudget {
        ResultBudget::Unlimited
    }

    fn cascades_on_failure(&self) -> bool {
        self.cascades
    }

    fn aliases(&self) -> &[&'static str] {
        &self.aliases
    }
}

// ────────────────────────────────────────────────────────────
// 基础设施替身
//
// 这些一律**报错**而不是返回空值。调度器的测试不该碰文件系统和进程 ——
// 真碰到了说明有人在调度层加了不该有的逻辑，那时候报错比静默返回空
// 更容易发现。
// ────────────────────────────────────────────────────────────

use std::path::{Path, PathBuf};

use riot_protocol::IdGenerator;
use riot_protocol::tool::{
    FileMeta, FileState, FileStateCache, FileSystem, ProcessOutput, ProcessRunner, ProcessSpec,
};
use tokio_util::sync::CancellationToken;

/// 真实的时间流逝。
///
/// 豁免理由：这个替身的作用就是制造"某个工具比另一个慢"的时序，
/// 好验证结果保序。用注入的 Clock 反而测不到 —— mock 时钟的 sleep
/// 立即返回，所有工具会同时完成，保序断言就永远是绿的。
///
/// 测试跑在 `start_paused = true` 下，tokio 的时间轮会自动推进，
/// 所以 800ms 的等待在几微秒内跑完，不依赖真实挂钟。
#[allow(clippy::disallowed_methods)]
async fn wall_sleep(d: Duration) {
    tokio::time::sleep(d).await;
}

fn refuse<T>(what: &str) -> std::io::Result<T> {
    Err(std::io::Error::other(format!(
        "调度器测试不该{what} —— 碰到这里说明调度层混进了不属于它的逻辑"
    )))
}

pub struct NullFs;

#[async_trait]
impl FileSystem for NullFs {
    async fn read(&self, _path: &Path) -> std::io::Result<Vec<u8>> {
        refuse("读文件")
    }
    async fn write(&self, _path: &Path, _data: &[u8]) -> std::io::Result<()> {
        refuse("写文件")
    }
    async fn metadata(&self, _path: &Path) -> std::io::Result<FileMeta> {
        refuse("读元数据")
    }
    async fn read_dir(&self, _path: &Path) -> std::io::Result<Vec<PathBuf>> {
        refuse("列目录")
    }
    async fn canonicalize(&self, _path: &Path) -> std::io::Result<PathBuf> {
        refuse("解析路径")
    }
}

pub struct NullProc;

#[async_trait]
impl ProcessRunner for NullProc {
    async fn run(
        &self,
        _spec: ProcessSpec,
        _cancel: CancellationToken,
    ) -> std::io::Result<ProcessOutput> {
        refuse("起进程")
    }
}

/// 什么都不记的文件状态缓存。
#[derive(Default)]
pub struct NullFileState;

impl FileStateCache for NullFileState {
    fn get(&self, _path: &Path) -> Option<FileState> {
        None
    }
    fn put(&self, _path: PathBuf, _state: FileState) {}
    fn invalidate(&self, _path: &Path) {}
    fn recent(&self, _limit: usize) -> Vec<(PathBuf, FileState)> {
        Vec::new()
    }
}

/// 手动推进的时钟。
///
/// 缓存过期这类行为必须能在测试里精确控制 —— 靠 sleep 去等 15 分钟的
/// TTL 不现实，靠"反正没到期"的隐含假设又测不出边界。
pub struct FixedClock(std::sync::atomic::AtomicU64);

impl Default for FixedClock {
    fn default() -> Self {
        Self::new(0)
    }
}

impl FixedClock {
    pub fn new(start_ms: u64) -> Self {
        Self(std::sync::atomic::AtomicU64::new(start_ms))
    }

    pub fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

#[async_trait]
impl riot_protocol::tool::Clock for FixedClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    /// 不真的睡。测试里睡觉只会让 CI 变慢，测不出任何东西。
    async fn sleep_ms(&self, ms: u64) {
        self.advance(ms);
    }
}

/// 按脚本应答的联网替身。
///
/// `[约束]` 脚本耗尽时返回**不可重试**的错误，理由同本文件顶部：替身
/// 比真实实现宽容会让"发了几次请求"这类断言永远测不准。
#[derive(Default)]
pub struct FakeWeb {
    /// url → 响应。
    pages: std::sync::Mutex<std::collections::HashMap<String, riot_protocol::web::WebResponse>>,
    hits: std::sync::Mutex<Vec<riot_protocol::web::SearchHit>>,
    /// None = 未配置辅助模型，走降级路径。
    distilled: std::sync::Mutex<Option<String>>,
    /// 实际请求过的 URL，按顺序。用来断言重定向跳了几跳、缓存有没有生效。
    requested: std::sync::Mutex<Vec<String>>,
}

impl FakeWeb {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一个 200 响应。
    pub fn page(self, url: &str, content_type: &str, body: &str) -> Self {
        self.put(
            url,
            riot_protocol::web::WebResponse {
                status: 200,
                status_text: "OK".into(),
                content_type: content_type.into(),
                body: body.as_bytes().to_vec(),
                location: None,
            },
        );
        self
    }

    /// 登记一个重定向。`location` 要是绝对地址 —— 真实实现负责解析相对地址。
    pub fn redirect(self, url: &str, status: u16, location: &str) -> Self {
        self.put(
            url,
            riot_protocol::web::WebResponse {
                status,
                status_text: "Redirect".into(),
                content_type: String::new(),
                body: Vec::new(),
                location: Some(location.to_owned()),
            },
        );
        self
    }

    pub fn status(self, url: &str, code: u16, body: &str) -> Self {
        self.put(
            url,
            riot_protocol::web::WebResponse {
                status: code,
                status_text: "Error".into(),
                content_type: "text/plain".into(),
                body: body.as_bytes().to_vec(),
                location: None,
            },
        );
        self
    }

    pub fn search_hits(self, hits: Vec<riot_protocol::web::SearchHit>) -> Self {
        *self.hits.lock().expect("hits poisoned") = hits;
        self
    }

    pub fn with_distiller(self, out: &str) -> Self {
        *self.distilled.lock().expect("distilled poisoned") = Some(out.to_owned());
        self
    }

    /// 实际发出去的请求，按顺序。
    pub fn requested(&self) -> Vec<String> {
        self.requested.lock().expect("requested poisoned").clone()
    }

    fn put(&self, url: &str, resp: riot_protocol::web::WebResponse) {
        self.pages
            .lock()
            .expect("pages poisoned")
            .insert(url.to_owned(), resp);
    }
}

#[async_trait]
impl riot_protocol::web::WebAccess for FakeWeb {
    async fn get(
        &self,
        req: riot_protocol::web::WebRequest,
        _cancel: &CancellationToken,
    ) -> Result<riot_protocol::web::WebResponse, riot_protocol::web::WebError> {
        self.requested
            .lock()
            .expect("requested poisoned")
            .push(req.url.clone());

        self.pages
            .lock()
            .expect("pages poisoned")
            .get(&req.url)
            .cloned()
            .ok_or(riot_protocol::web::WebError::Status {
                code: 404,
                body: format!("替身里没有登记 {}", req.url),
            })
    }

    async fn search(
        &self,
        _query: riot_protocol::web::SearchQuery,
        _cancel: &CancellationToken,
    ) -> Result<Vec<riot_protocol::web::SearchHit>, riot_protocol::web::WebError> {
        Ok(self.hits.lock().expect("hits poisoned").clone())
    }

    async fn distill(
        &self,
        _req: riot_protocol::web::DistillRequest,
        _cancel: &CancellationToken,
    ) -> Result<String, riot_protocol::web::WebError> {
        self.distilled
            .lock()
            .expect("distilled poisoned")
            .clone()
            .ok_or(riot_protocol::web::WebError::NotConfigured {
                what: "辅助模型".to_owned(),
            })
    }
}

/// 递增的 id 生成器。
#[derive(Default)]
pub struct SeqIds(AtomicUsize);

impl IdGenerator for SeqIds {
    fn next_id(&self, prefix: &str) -> String {
        let n = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        format!("{prefix}_{n}")
    }
}

/// 用给定工具搭一个调度器。
pub fn test_scheduler(tools: Vec<Arc<dyn Tool>>) -> crate::scheduler::Scheduler {
    crate::scheduler::Scheduler::new(
        Arc::new(crate::registry::Registry::new(tools).expect("注册表构造成功")),
        PromptContext {
            cwd: "/tmp".into(),
            platform: "test".into(),
            sibling_tools: Vec::new(),
            today: "2026年8月".into(),
        },
        Arc::new(NullFs),
        Arc::new(NullProc),
        Arc::new(NullFileState),
        Arc::new(SeqIds::default()),
        Arc::new(FixedClock::default()),
    )
}

/// 浏览器替身:能截图，交互按配置应答。
///
/// 没配置的能力一律报"用不了" —— 替身比真实实现宽容会让用例测不到
/// 该测的东西（见本文件顶部）。
#[derive(Default)]
pub struct FakeBrowser {
    /// 截图返回的 base64。
    pub shot: String,
    /// 交互（click/type/key/scroll）的应答。
    /// `None` = 报"不可用"；`Some(Err(msg))` = Target 错误（编号失效那类）。
    pub interact: Option<Result<String, String>>,
    /// 交互调用的记录，如 `click 3`、`type 3 "你好" submit=true`。
    /// 用例拿它断言参数原样到达了宿主。
    pub calls: std::sync::Mutex<Vec<String>>,
}

impl FakeBrowser {
    fn interaction(&self, what: String) -> Result<String, riot_protocol::browser::InteractError> {
        self.calls.lock().expect("calls poisoned").push(what);
        match &self.interact {
            Some(Ok(msg)) => Ok(msg.clone()),
            Some(Err(t)) => Err(riot_protocol::browser::InteractError::Target(t.clone())),
            None => Err(riot_protocol::browser::InteractError::Unavailable(
                riot_protocol::browser::BrowserUnavailable("替身没有交互能力".into()),
            )),
        }
    }
}

#[async_trait::async_trait]
impl riot_protocol::browser::BrowserAccess for FakeBrowser {
    async fn navigate(&self, url: &str) -> Result<(), riot_protocol::browser::BrowserUnavailable> {
        self.calls
            .lock()
            .expect("calls poisoned")
            .push(format!("navigate {url}"));
        Err(riot_protocol::browser::BrowserUnavailable(
            "替身不导航".into(),
        ))
    }
    async fn screenshot(&self) -> Result<String, riot_protocol::browser::BrowserUnavailable> {
        Ok(self.shot.clone())
    }
    async fn snapshot(&self) -> Result<String, riot_protocol::browser::BrowserUnavailable> {
        Err(riot_protocol::browser::BrowserUnavailable(
            "替身没有快照".into(),
        ))
    }
    /// 和 [`Self::snapshot`] 一致地报不可用。
    ///
    /// `[约束]` 替身不能比真实实现更宽容。这里返回一个空的 `MarkedView`
    /// 会让"没有快照能力"变成"快照是空的"，而 BrowserView 对这两种情况
    /// 的处理完全不同（前者失败，后者说"页面上没有可识别的结构"）——
    /// 那样用例就在替身造出来的第三种世界里跑。
    async fn snapshot_marked(
        &self,
    ) -> Result<riot_protocol::browser::MarkedView, riot_protocol::browser::BrowserUnavailable>
    {
        Err(riot_protocol::browser::BrowserUnavailable(
            "替身没有快照".into(),
        ))
    }
    async fn console(&self) -> Result<Vec<String>, riot_protocol::browser::BrowserUnavailable> {
        Err(riot_protocol::browser::BrowserUnavailable(
            "替身没有 console".into(),
        ))
    }
    async fn current_url(&self) -> String {
        String::new()
    }
    async fn click(
        &self,
        target: riot_protocol::browser::Target,
    ) -> Result<String, riot_protocol::browser::InteractError> {
        self.interaction(format!("click {}", fake_target(&target)))
    }
    async fn type_text(
        &self,
        target: riot_protocol::browser::Target,
        text: &str,
        submit: bool,
    ) -> Result<String, riot_protocol::browser::InteractError> {
        self.interaction(format!(
            "type {} {text:?} submit={submit}",
            fake_target(&target)
        ))
    }
    async fn press_key(&self, key: &str) -> Result<String, riot_protocol::browser::InteractError> {
        self.interaction(format!("key {key}"))
    }
    async fn scroll(&self, delta_y: f64) -> Result<String, riot_protocol::browser::InteractError> {
        self.interaction(format!("scroll {delta_y}"))
    }
    async fn wait_for(
        &self,
        cond: riot_protocol::browser::WaitCondition,
        timeout_ms: u64,
    ) -> Result<String, riot_protocol::browser::InteractError> {
        self.interaction(format!("wait {cond:?} {timeout_ms}"))
    }
    async fn act(
        &self,
        action: riot_protocol::browser::Action,
    ) -> Result<String, riot_protocol::browser::InteractError> {
        self.interaction(format!("act {action:?}"))
    }
    async fn browse(
        &self,
        nav: riot_protocol::browser::Nav,
    ) -> Result<String, riot_protocol::browser::InteractError> {
        self.interaction(format!("browse {nav:?}"))
    }
    async fn evaluate(&self, expr: &str) -> Result<String, riot_protocol::browser::InteractError> {
        self.interaction(format!("eval {expr}"))
    }
    async fn upload(
        &self,
        target: riot_protocol::browser::Target,
        paths: Vec<String>,
    ) -> Result<String, riot_protocol::browser::InteractError> {
        self.interaction(format!(
            "upload {} {}",
            fake_target(&target),
            paths.join(",")
        ))
    }
    async fn cookies(&self) -> Result<String, riot_protocol::browser::InteractError> {
        self.interaction("cookies".to_owned())
    }
    async fn network(
        &self,
        query: riot_protocol::browser::NetQuery,
    ) -> Result<String, riot_protocol::browser::InteractError> {
        self.interaction(format!("network {query:?}"))
    }
    async fn replay(
        &self,
        url: &str,
        method: &str,
        _headers: serde_json::Value,
        body: Option<String>,
    ) -> Result<String, riot_protocol::browser::InteractError> {
        self.interaction(format!("replay {method} {url} body={}", body.is_some()))
    }
    async fn intercept(
        &self,
        op: riot_protocol::browser::InterceptOp,
    ) -> Result<String, riot_protocol::browser::InteractError> {
        self.interaction(format!("intercept {op:?}"))
    }
}

/// 把定位目标压成一行，供用例断言参数原样到达。
fn fake_target(t: &riot_protocol::browser::Target) -> String {
    match t {
        riot_protocol::browser::Target::Ref(n) => format!("ref:{n}"),
        riot_protocol::browser::Target::Selector(s) => format!("sel:{s}"),
        riot_protocol::browser::Target::Text(s) => format!("text:{s}"),
    }
}

/// 图片能力的替身。
pub enum FakeVision {
    /// 主模型自己能看图。
    Direct,
    /// 主模型看不了，由兼容模型转述成这段文字。
    Describe(String),
    /// 看不了，也没配兼容模型。
    None,
}

#[async_trait::async_trait]
impl riot_protocol::vision::VisionAccess for FakeVision {
    fn accepts_images(&self) -> bool {
        matches!(self, Self::Direct)
    }

    async fn describe(
        &self,
        _req: riot_protocol::vision::DescribeRequest,
    ) -> Result<String, riot_protocol::vision::VisionError> {
        match self {
            Self::Describe(text) => Ok(text.clone()),
            // Direct 时调用方不该走到这条路 —— 走到了就是分支写错了，
            // 替身要让那种错误当场可见，而不是悄悄给一段文字。
            Self::Direct => Err(riot_protocol::vision::VisionError::Failed {
                message: "模型本来就能看图，不该来转述".into(),
            }),
            Self::None => Err(riot_protocol::vision::VisionError::NotConfigured),
        }
    }
}
