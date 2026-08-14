//! 并发执行与结果保序。
//!
//! # 三条约束，破坏任何一条都是静默的
//!
//! **一、结果顺序必须等于调用顺序。**用 `FuturesOrdered` 而不是
//! `FuturesUnordered`。后者按完成顺序产出，于是 transcript 的消息顺序
//! 取决于调度时序 —— 黄金回放会随机失败，而大家会以为是"测试不稳定"。
//! `clippy.toml` 里禁了 `FuturesUnordered` 来兜这个。
//!
//! **二、每个 `tool_use` 恰好一个 `tool_result`。**包括被取消的、
//! 未注册的、panic 的。缺一个下次请求就是 400，而错误信息不会告诉你
//! 是哪个 id 缺了。
//!
//! **三、进度可以插队，结果不行。**进度事件是给 UI 看的实时反馈，
//! 延后就失去意义；结果是 transcript 的一部分，顺序即语义。
//!
//! 见 ARCHITECTURE.md §7.3、§7.4

use std::sync::Arc;

use async_stream::stream;
use futures::StreamExt;
use futures::stream::FuturesOrdered;
use riot_protocol::IdGenerator;
use riot_protocol::id::{MessageId, ToolUseId};
use riot_protocol::message::{Message, MessageMeta, ToolResultContent, UserContent};
use riot_protocol::permission::PermissionGate;
use riot_protocol::provider::ToolSpec;
use riot_protocol::runner::{
    BatchContext, BatchEvent, BatchOutcome, BatchStream, ToolCall, ToolRunner,
};
use riot_protocol::tool::{
    FileStateCache, FileSystem, ProcessRunner, ProgressSink, PromptContext, Tool, ToolContext,
    ToolOutcome,
};
use tokio_util::sync::CancellationToken;

use crate::partition::{DEFAULT_MAX_CONCURRENCY, partition};
use crate::registry::Registry;

/// 执行一批工具调用。
pub struct Scheduler {
    registry: Arc<Registry>,
    max_concurrency: usize,
    prompt_ctx: PromptContext,
    fs: Arc<dyn FileSystem>,
    proc: Arc<dyn ProcessRunner>,
    file_state: Arc<dyn FileStateCache>,
    ids: Arc<dyn IdGenerator>,
    clock: Arc<dyn riot_protocol::tool::Clock>,
    /// 联网能力。默认 [`riot_protocol::web::NoWeb`]（一律拒绝）。
    ///
    /// 默认值是"没网"而不是某个兜底客户端：忘了装配的表现应该是
    /// WebFetch 明确报"联网未启用"，而不是它悄悄用上了一条没人审过的出口。
    web: Arc<dyn riot_protocol::web::WebAccess>,
    /// 注入的浏览器能力。默认 NoBrowser —— 没装配就明说用不了。
    browser: Arc<dyn riot_protocol::browser::BrowserAccess>,
    /// 图片能力。默认 [`riot_protocol::vision::NoVision`]（模型不收图片、
    /// 也没有兼容模型）。
    vision: Arc<dyn riot_protocol::vision::VisionAccess>,
    /// `web` 是宿主装的还是默认的。只给 [`Self::has_web`] 用 ——
    /// trait object 之间没法比较"是不是同一个默认值"。
    web_injected: bool,
    /// 执行前的权限闸。
    ///
    /// `[约束]` `None` 表示**不检查**，只在测试里可以这样 —— 那些用例
    /// 验证的是调度行为（顺序、配对、级联），权限会把它们变成两件事。
    /// 生产路径必须调 [`Scheduler::with_gate`]，`session.rs` 里有一个
    /// 测试盯着这一点。
    gate: Option<Arc<dyn PermissionGate>>,
    /// 工具产物（截图原图等）的落盘目录。宿主按会话装配；默认指向
    /// 系统临时目录，写不进时工具自行降级。
    artifacts_dir: std::path::PathBuf,
    /// 延迟加载状态。None = 不启用，全部工具直接可见。
    ///
    /// 启用时未被发现的延迟工具既不进 [`ToolRunner::specs`]，也不可
    /// 直接调用 —— 模型没见过 schema，编出来的参数不可信。
    deferred: Option<Arc<crate::tools::tool_search::DeferredPool>>,
    /// PostToolUse 检查点（用户配置的 hooks）。默认 NoToolHooks —— 零开销。
    hooks: Arc<dyn riot_protocol::hook::ToolHooks>,
}

impl Scheduler {
    pub fn new(
        registry: Arc<Registry>,
        prompt_ctx: PromptContext,
        fs: Arc<dyn FileSystem>,
        proc: Arc<dyn ProcessRunner>,
        file_state: Arc<dyn FileStateCache>,
        ids: Arc<dyn IdGenerator>,
        clock: Arc<dyn riot_protocol::tool::Clock>,
    ) -> Self {
        Self {
            registry,
            max_concurrency: DEFAULT_MAX_CONCURRENCY,
            prompt_ctx,
            fs,
            proc,
            file_state,
            ids,
            clock,
            web: Arc::new(riot_protocol::web::NoWeb),
            browser: Arc::new(riot_protocol::browser::NoBrowser),
            vision: Arc::new(riot_protocol::vision::NoVision),
            web_injected: false,
            gate: None,
            artifacts_dir: std::env::temp_dir().join("riot-artifacts"),
            deferred: None,
            hooks: Arc::new(riot_protocol::hook::NoToolHooks),
        }
    }

    /// 装上 PostToolUse 检查点（用户配置的 hooks）。
    pub fn with_hooks(mut self, hooks: Arc<dyn riot_protocol::hook::ToolHooks>) -> Self {
        self.hooks = hooks;
        self
    }

    /// 启用延迟加载。池由装配方（session）在超过阈值时构建。
    pub fn with_deferred(
        mut self,
        pool: Arc<crate::tools::tool_search::DeferredPool>,
    ) -> Self {
        self.deferred = Some(pool);
        self
    }

    pub fn with_artifacts_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.artifacts_dir = dir;
        self
    }

    pub fn with_gate(mut self, gate: Arc<dyn PermissionGate>) -> Self {
        self.gate = Some(gate);
        self
    }

    pub fn with_browser(
        mut self,
        browser: Arc<dyn riot_protocol::browser::BrowserAccess>,
    ) -> Self {
        self.browser = browser;
        self
    }

    pub fn with_vision(
        mut self,
        vision: Arc<dyn riot_protocol::vision::VisionAccess>,
    ) -> Self {
        self.vision = vision;
        self
    }

    pub fn with_web(mut self, web: Arc<dyn riot_protocol::web::WebAccess>) -> Self {
        self.web = web;
        self.web_injected = true;
        self
    }

    pub fn has_gate(&self) -> bool {
        self.gate.is_some()
    }

    /// 宿主有没有装过联网能力。
    ///
    /// 供装配测试用。默认的 [`riot_protocol::web::NoWeb`] 会让联网
    /// 工具一律报"未配置"，那是个跑起来才看得见的静默降级。
    pub fn has_web(&self) -> bool {
        self.web_injected
    }

    pub fn with_max_concurrency(mut self, n: usize) -> Self {
        self.max_concurrency = n;
        self
    }
}

impl ToolRunner for Scheduler {
    fn specs(&self) -> Vec<ToolSpec> {
        let specs = self.registry.specs(&self.prompt_ctx);
        let Some(pool) = &self.deferred else {
            return specs;
        };
        // 未发现的延迟工具不进请求。specs 每轮请求都会重算（agent loop
        // 每次组请求都调它），所以本轮中途 ToolSearch 发现的工具，
        // 下一次请求就带上完整定义。
        specs.into_iter().filter(|s| !pool.is_hidden(&s.name)).collect()
    }

    fn run_batch(&self, calls: Vec<ToolCall>, ctx: BatchContext) -> BatchStream {
        let registry = Arc::clone(&self.registry);
        let max = self.max_concurrency;
        let fs = Arc::clone(&self.fs);
        let proc = Arc::clone(&self.proc);
        let file_state = Arc::clone(&self.file_state);
        let clock = Arc::clone(&self.clock);
        let web = Arc::clone(&self.web);
        let browser = Arc::clone(&self.browser);
        let vision = Arc::clone(&self.vision);
        let ids = Arc::clone(&self.ids);
        let cwd = self.prompt_ctx.cwd.clone();
        let gate = self.gate.clone();
        let artifacts_dir = self.artifacts_dir.clone();
        let deferred = self.deferred.clone();
        let hooks = Arc::clone(&self.hooks);

        Box::pin(stream! {
            let total = calls.len();
            let batches = partition(calls, &registry, max);

            // 进度通道。工具在自己的 task 里往这里塞，我们在主循环里排空。
            let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();

            let mut results: Vec<UserContent> = Vec::with_capacity(total);
            let mut side_messages: Vec<Message> = Vec::new();
            let mut cancelled = 0usize;

            'batches: for batch in batches {
                // 上一批级联取消或用户中断之后，剩下的批次直接补结果。
                // 不能 break —— 那样后面的 tool_use 就没有结果了。
                if ctx.cancel.is_cancelled() {
                    for call in batch.calls() {
                        results.push(cancelled_result(&call.id));
                        cancelled += 1;
                    }
                    continue 'batches;
                }

                // 兄弟级联用的令牌：它是 ctx.cancel 的子令牌，所以父级中断
                // 会传下来，而级联取消不会影响父级和后续批次的判断。
                let sibling = ctx.cancel.child_token();

                let mut pending = FuturesOrdered::new();
                for call in batch.calls() {
                    pending.push_back(run_one(
                        call.clone(),
                        Arc::clone(&registry),
                        ToolDeps {
                            fs: Arc::clone(&fs),
                            proc: Arc::clone(&proc),
                            file_state: Arc::clone(&file_state),
                            clock: Arc::clone(&clock),
                            web: Arc::clone(&web),
                            browser: Arc::clone(&browser),
                            vision: Arc::clone(&vision),
                            cwd: cwd.clone(),
                            artifacts_dir: artifacts_dir.clone(),
                            deferred: deferred.clone(),
                            hooks: Arc::clone(&hooks),
                        },
                        ctx.session_id.clone(),
                        sibling.child_token(),
                        progress_tx.clone(),
                        gate.clone(),
                    ));
                }

                // 收结果。用 select 同时排空进度通道 —— 否则 Bash 的实时输出
                // 会攒到整批结束才吐出去，UI 上就是"卡住然后一次刷完"。
                loop {
                    tokio::select! {
                        biased;

                        Some((id, payload)) = progress_rx.recv() => {
                            yield BatchEvent::Progress { tool_use_id: id, payload };
                        }

                        item = pending.next() => {
                            let Some(done) = item else { break };

                            if done.cascaded {
                                cancelled += 1;
                            }
                            // 只有真正的失败才级联。被取消的工具不该再去
                            // 取消别人 —— 那会让一次中断变成连锁反应，
                            // 而用户只按了一次停止。
                            if done.outcome.is_error() && done.cascades && !sibling.is_cancelled() {
                                tracing::info!(tool = %done.name, "工具失败，取消同批兄弟");
                                sibling.cancel();
                            }

                            let (content, sides) = split_outcome(done.outcome);
                            results.push(UserContent::ToolResult {
                                tool_use_id: done.id,
                                content,
                                is_error: done.is_error,
                            });
                            side_messages.extend(sides);
                            // hook 反馈以带外提示进对话：它是自动化检查说的话，
                            // 不是用户说的 —— synthetic + system-reminder 双标记。
                            for text in done.hook_feedback {
                                side_messages.push(Message::User {
                                    id: MessageId::from_raw(ids.next_id("msg")),
                                    content: vec![UserContent::Attachment(
                                        riot_protocol::message::Attachment::SystemReminder { text },
                                    )],
                                    meta: MessageMeta { synthetic: true, ..Default::default() },
                                });
                            }
                        }
                    }
                }

                // 排空剩余进度。工具已经结束了，但通道里可能还压着几条。
                while let Ok((id, payload)) = progress_rx.try_recv() {
                    yield BatchEvent::Progress { tool_use_id: id, payload };
                }
            }

            debug_assert_eq!(
                results.len(),
                total,
                "每个 tool_use 必须恰好一个 tool_result，缺一个下次请求就是 400"
            );

            yield BatchEvent::Done(BatchOutcome {
                results: Message::User {
                    id: MessageId::from_raw(ids.next_id("msg")),
                    content: results,
                    meta: MessageMeta::default(),
                },
                side_messages,
                cancelled,
            });
        })
    }
}

struct ToolDeps {
    fs: Arc<dyn FileSystem>,
    proc: Arc<dyn ProcessRunner>,
    file_state: Arc<dyn FileStateCache>,
    clock: Arc<dyn riot_protocol::tool::Clock>,
    web: Arc<dyn riot_protocol::web::WebAccess>,
    browser: Arc<dyn riot_protocol::browser::BrowserAccess>,
    vision: Arc<dyn riot_protocol::vision::VisionAccess>,
    cwd: std::path::PathBuf,
    artifacts_dir: std::path::PathBuf,
    /// 延迟加载状态。None = 不启用。
    deferred: Option<Arc<crate::tools::tool_search::DeferredPool>>,
    hooks: Arc<dyn riot_protocol::hook::ToolHooks>,
}

struct Done {
    id: ToolUseId,
    name: String,
    outcome: ToolOutcome,
    is_error: bool,
    /// 这个工具失败时要不要连累兄弟。
    cascades: bool,
    /// 它是被取消/级联掉的，没有真正执行。
    cascaded: bool,
    /// PostToolUse hooks 的反馈段落。收集侧包成 system-reminder。
    hook_feedback: Vec<String>,
}

async fn run_one(
    call: ToolCall,
    registry: Arc<Registry>,
    deps: ToolDeps,
    session_id: riot_protocol::id::SessionId,
    cancel: CancellationToken,
    progress_tx: tokio::sync::mpsc::UnboundedSender<(
        ToolUseId,
        riot_protocol::event::ProgressPayload,
    )>,
    gate: Option<Arc<dyn PermissionGate>>,
) -> Done {
    let Some(tool) = registry.get(&call.name).cloned() else {
        // 未注册。给模型一条能纠错的消息，而不是原始错误 ——
        // "找不到 Reed 工具"它看不懂，"用 Read"它能改。
        let available = registry
            .iter()
            .map(|t| t.name())
            .collect::<Vec<_>>()
            .join("、");
        return Done {
            id: call.id,
            name: call.name.clone(),
            outcome: ToolOutcome::failed(format!(
                "没有名为 `{}` 的工具。可用的工具：{available}",
                call.name
            )),
            is_error: true,
            cascades: false,
            cascaded: false,
            hook_feedback: Vec::new(),
        };
    };

    // 还没加载的延迟工具不执行（fail-closed）：模型只见过名字没见过
    // schema，编出来的参数不可信。放在权限闸**之前** —— 为一个不该
    // 执行的调用弹授权窗，只会把用户也拖进这个错误。
    if let Some(pool) = &deps.deferred
        && pool.is_hidden(&call.name)
    {
        return Done {
            id: call.id,
            name: call.name.clone(),
            outcome: ToolOutcome::failed(format!(
                "`{}` 还没加载，参数定义未知。先调用 ToolSearch（query 用 \
                 \"select:{}\"）取回它的定义，下一步再用它。",
                call.name, call.name
            )),
            is_error: true,
            cascades: false,
            cascaded: false,
            hook_feedback: Vec::new(),
        };
    }

    let cascades = tool.cascades_on_failure();

    // 兄弟已经被取消了：不执行，直接补结果。
    // 这一步必须在 spawn 之前 —— 否则"级联"只是让工具跑完后丢弃结果，
    // 副作用照样发生了。
    if cancel.is_cancelled() {
        return Done {
            id: call.id,
            name: call.name,
            outcome: ToolOutcome::failed("同批次的其它工具失败，已跳过"),
            is_error: true,
            cascades: false,
            cascaded: true,
            hook_feedback: Vec::new(),
        };
    }

    // 权限闸。放在这里而不是工具内部，是因为拒绝必须**在副作用之前**发生 ——
    // 工具自己检查的话，"检查"和"动手"之间的每一行代码都是可能出错的地方。
    let mut input = call.input;
    if let Some(g) = &gate {
        match g.check(tool.as_ref(), &input, &call.id, &cancel).await {
            riot_protocol::permission::GateOutcome::Allow { updated_input } => {
                if let Some(v) = updated_input {
                    input = v;
                }
            }
            riot_protocol::permission::GateOutcome::Deny { message } => {
                return Done {
                    id: call.id,
                    name: call.name,
                    outcome: ToolOutcome::failed(message),
                    is_error: true,
                    // 被拒不连累兄弟。用户拒绝一次写文件，不该把同批的
                    // 读取也一起废掉 —— 那些结果模型还用得上。
                    cascades: false,
                    cascaded: false,
                    hook_feedback: Vec::new(),
                };
            }
        }
    }

    // 闸后重查取消。等用户回答弹窗期间他可能按了停止，那时再去执行
    // 就是"我明明点了停止它还是改了文件"。
    if cancel.is_cancelled() {
        return Done {
            id: call.id,
            name: call.name,
            outcome: ToolOutcome::Cancelled,
            is_error: true,
            cascades: false,
            cascaded: true,
            hook_feedback: Vec::new(),
        };
    }

    let ctx = ToolContext {
        session_id,
        tool_use_id: call.id.clone(),
        cwd: deps.cwd,
        artifacts_dir: deps.artifacts_dir,
        cancel: cancel.clone(),
        progress: ProgressSink::new(call.id.clone(), progress_tx),
        file_state: deps.file_state,
        fs: deps.fs,
        proc: deps.proc,
        web: deps.web,
        browser: deps.browser,
        vision: deps.vision,
        clock: deps.clock,
    };

    // hook 要看 input，而 execute 拿走了它 —— 只在真装了 hooks 时才付
    // 这份克隆（Write 的 input 可能是整个文件）。
    let hook_input = deps.hooks.enabled().then(|| input.clone());

    let outcome = execute_guarded(tool, input, ctx, &call.name).await;
    let is_error = !matches!(outcome, ToolOutcome::Ok { .. });

    // PostToolUse hooks：执行完了让用户配置的检查点看一眼。
    // 取消的调用不问 —— 没执行过的东西没什么可检查的。
    let hook_feedback = match (&outcome, hook_input) {
        (ToolOutcome::Cancelled, _) | (_, None) => Vec::new(),
        (_, Some(hin)) => {
            deps.hooks
                .post_tool_use(&call.name, &hin, &outcome_preview(&outcome), is_error)
                .await
        }
    };

    Done {
        id: call.id,
        name: call.name,
        outcome,
        is_error,
        cascades,
        cascaded: false,
        hook_feedback,
    }
}

/// 给 hook 看的结果预览。图片给占位符 —— hook 是 shell 脚本，
/// 喂 base64 只会撑爆它的 stdin。
fn outcome_preview(outcome: &ToolOutcome) -> String {
    match outcome {
        ToolOutcome::Ok { model_content, .. } => match model_content {
            ToolResultContent::Text { text } => text.clone(),
            ToolResultContent::Spilled { preview, .. } => preview.clone(),
            ToolResultContent::Cleared => String::new(),
            ToolResultContent::Image { .. } | ToolResultContent::DescribedImage { .. } => {
                "(图片结果)".into()
            }
        },
        ToolOutcome::Failed { error_for_model, .. } => error_for_model.clone(),
        ToolOutcome::Cancelled => String::new(),
    }
}

/// 执行工具，把 panic 变成普通的失败结果。
///
/// 工具是第三方可扩展的（MCP）。一个 panic 不该拖垮整个批次 ——
/// 更要命的是，panic 会让这个 tool_use 没有结果，下次请求直接 400。
async fn execute_guarded(
    tool: Arc<dyn Tool>,
    input: serde_json::Value,
    ctx: ToolContext,
    name: &str,
) -> ToolOutcome {
    let fut = std::panic::AssertUnwindSafe(tool.call(input, ctx));

    match futures::FutureExt::catch_unwind(fut).await {
        Ok(outcome) => outcome,
        Err(_) => {
            tracing::error!(tool = %name, "工具 panic");
            ToolOutcome::failed(format!(
                "工具 `{name}` 内部错误，这次调用没有完成。换一种方式或换个工具。"
            ))
        }
    }
}

fn split_outcome(outcome: ToolOutcome) -> (ToolResultContent, Vec<Message>) {
    match outcome {
        ToolOutcome::Ok {
            model_content,
            side_messages,
            ..
        } => (model_content, side_messages),
        ToolOutcome::Failed {
            error_for_model, ..
        } => (ToolResultContent::text(error_for_model), Vec::new()),
        // 取消也要有结果，否则 tool_use 成了孤儿
        ToolOutcome::Cancelled => (ToolResultContent::text("已取消，此工具未完成"), Vec::new()),
    }
}

fn cancelled_result(id: &ToolUseId) -> UserContent {
    UserContent::ToolResult {
        tool_use_id: id.clone(),
        content: ToolResultContent::text("用户中断，此工具未执行"),
        is_error: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeTool;
    use riot_protocol::id::SessionId;
    use pretty_assertions::assert_eq;
    use std::sync::atomic::Ordering;

    fn scheduler(tools: Vec<Arc<dyn Tool>>) -> Scheduler {
        crate::testing::test_scheduler(tools)
    }

    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            id: ToolUseId::from_raw(id),
            name: name.into(),
            input: serde_json::json!({}),
        }
    }

    fn ctx(cancel: CancellationToken) -> BatchContext {
        BatchContext {
            session_id: SessionId::from_raw("s1"),
            cancel,
        }
    }

    async fn run(s: &Scheduler, calls: Vec<ToolCall>) -> Vec<BatchEvent> {
        s.run_batch(calls, ctx(CancellationToken::new()))
            .collect()
            .await
    }

    fn outcome(events: &[BatchEvent]) -> &BatchOutcome {
        let dones: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                BatchEvent::Done(o) => Some(o),
                _ => None,
            })
            .collect();
        assert_eq!(dones.len(), 1, "流必须以恰好一个 Done 结束");
        dones[0]
    }

    /// 结果里的 (tool_use_id, 文本) 序列。
    fn result_pairs(o: &BatchOutcome) -> Vec<(String, String)> {
        match &o.results {
            Message::User { content, .. } => content
                .iter()
                .map(|c| match c {
                    UserContent::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } => {
                        let text = match content {
                            ToolResultContent::Text { text } => text.clone(),
                            other => format!("{other:?}"),
                        };
                        (tool_use_id.as_str().to_owned(), text)
                    }
                    other => panic!("结果里混进了非 tool_result：{other:?}"),
                })
                .collect(),
            other => panic!("结果消息必须是 User：{other:?}"),
        }
    }

    // ── 测试 ──────────────────────────────────────────

    #[tokio::test]
    async fn posttooluse_的反馈作为带外提示进对话() {
        // hook 说了话却没人转达，等于这个功能不存在 —— 而它编译得过、
        // 跑得起来，只是模型永远收不到检查结果。
        struct Feedback;
        #[async_trait::async_trait]
        impl riot_protocol::hook::ToolHooks for Feedback {
            async fn post_tool_use(
                &self,
                tool: &str,
                _input: &serde_json::Value,
                _output_preview: &str,
                _is_error: bool,
            ) -> Vec<String> {
                vec![format!("{tool} 跑完了，记得跑 fmt")]
            }
        }

        let s = scheduler(vec![Arc::new(FakeTool::read_only("Echo"))]).with_hooks(Arc::new(Feedback));
        let events = run(&s, vec![call("t1", "Echo")]).await;
        let o = outcome(&events);

        // 反馈**不能**混进 tool_result（那会篡改工具输出），要独立成一条
        // synthetic 的 system-reminder 消息。
        assert_eq!(result_pairs(o).len(), 1, "工具结果照常只有一条");
        let side = o.side_messages.first().expect("该有一条 hook 反馈消息");
        match side {
            Message::User { content, meta, .. } => {
                assert!(meta.synthetic, "hook 说的话不是用户说的，必须标 synthetic");
                let ok = content.iter().any(|c| matches!(
                    c,
                    UserContent::Attachment(riot_protocol::message::Attachment::SystemReminder { text })
                        if text.contains("记得跑 fmt")
                ));
                assert!(ok, "反馈内容丢了：{content:?}");
            }
            other => panic!("hook 反馈该是 User 消息：{other:?}"),
        }
    }

    #[tokio::test]
    async fn 没装_hooks_时不产生额外消息() {
        // 默认路径的回归护栏：绝大多数用户没有 hooks，一条多余的
        // system-reminder 就是每轮都付的上下文税。
        let s = scheduler(vec![Arc::new(FakeTool::read_only("Echo"))]);
        let events = run(&s, vec![call("t1", "Echo")]).await;
        assert!(outcome(&events).side_messages.is_empty());
    }

    #[tokio::test]
    async fn 未加载的延迟工具被拦_发现后放行() {
        struct DeferredFake;
        #[async_trait::async_trait]
        impl Tool for DeferredFake {
            fn name(&self) -> &str {
                "mcp__x__y"
            }
            fn input_schema(&self) -> schemars::Schema {
                schemars::json_schema!({ "type": "object" })
            }
            fn prompt(&self, _: &PromptContext) -> String {
                "外部工具".into()
            }
            fn describe(&self, _: &serde_json::Value) -> String {
                "d".into()
            }
            fn should_defer(&self) -> bool {
                true
            }
            async fn call(&self, _: serde_json::Value, _: ToolContext) -> ToolOutcome {
                ToolOutcome::ok_text("ran")
            }
        }

        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(DeferredFake)];
        let discovered = Arc::new(std::sync::RwLock::new(std::collections::HashSet::new()));
        let pctx = PromptContext {
            cwd: "/w".into(),
            platform: "t".into(),
            sibling_tools: Vec::new(),
            today: "2026年8月".into(),
        };
        let pool = Arc::new(crate::tools::tool_search::DeferredPool::new(
            &tools,
            &pctx,
            Arc::clone(&discovered),
        ));
        let s = scheduler(tools).with_deferred(Arc::clone(&pool));

        // 未发现：不进 specs（这正是省上下文的地方）
        assert!(
            s.specs().iter().all(|sp| sp.name != "mcp__x__y"),
            "未发现的延迟工具不该进请求"
        );

        // 未发现：直接调用被拦，报错要指路到 ToolSearch ——
        // 模型没见过 schema，编出来的参数不可信（fail-closed）。
        let events = run(&s, vec![call("t1", "mcp__x__y")]).await;
        let pairs = result_pairs(outcome(&events));
        assert!(
            pairs[0].1.contains("ToolSearch") && pairs[0].1.contains("select:mcp__x__y"),
            "报错要教模型下一步怎么做：{}",
            pairs[0].1
        );

        // 发现之后：specs 有它、调用放行。
        discovered.write().expect("锁").insert("mcp__x__y".into());
        assert!(
            s.specs().iter().any(|sp| sp.name == "mcp__x__y"),
            "发现之后要进请求"
        );
        let events = run(&s, vec![call("t2", "mcp__x__y")]).await;
        let pairs = result_pairs(outcome(&events));
        assert_eq!(pairs[0].1, "ran", "发现之后调用要正常执行");
    }

    #[tokio::test(start_paused = true)]
    async fn 结果按调用顺序返回_哪怕完成顺序相反() {
        // 最容易写错的地方：用 FuturesUnordered 的话，快的先出，
        // transcript 顺序就取决于调度时序，黄金回放会随机失败。
        let s = scheduler(vec![
            Arc::new(FakeTool::read_only("Slow").slow(500)) as Arc<dyn Tool>,
            Arc::new(FakeTool::read_only("Fast")),
        ]);

        let events = run(&s, vec![call("a", "Slow"), call("b", "Fast")]).await;
        let ids: Vec<String> = result_pairs(outcome(&events))
            .into_iter()
            .map(|(id, _)| id)
            .collect();

        assert_eq!(ids, vec!["a", "b"], "慢的先入队就要先出结果");
    }

    #[tokio::test(start_paused = true)]
    async fn 每个调用恰好一个结果() {
        let s = scheduler(vec![
            Arc::new(FakeTool::read_only("Read")) as Arc<dyn Tool>,
            Arc::new(FakeTool::writer("Edit")),
        ]);

        let calls = vec![
            call("a", "Read"),
            call("b", "Edit"),
            call("c", "Read"),
            call("d", "Read"),
        ];
        let events = run(&s, calls).await;
        let ids: Vec<String> = result_pairs(outcome(&events))
            .into_iter()
            .map(|(id, _)| id)
            .collect();

        assert_eq!(
            ids,
            vec!["a", "b", "c", "d"],
            "跨批次也要保序且不漏 —— 缺一个下次请求就是 400"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn 未注册的工具变成可纠错的结果() {
        let s = scheduler(vec![Arc::new(FakeTool::read_only("Read")) as Arc<dyn Tool>]);
        let events = run(&s, vec![call("a", "Reed")]).await;

        let pairs = result_pairs(outcome(&events));
        assert_eq!(pairs.len(), 1);
        assert!(
            pairs[0].1.contains("Read"),
            "要告诉模型有哪些工具可用，它才能改：{}",
            pairs[0].1
        );
    }

    #[tokio::test(start_paused = true)]
    async fn 工具_panic_不会让批次少一个结果() {
        // panic 让这个 tool_use 没有结果的话，下次请求直接 400
        struct Exploding;
        #[async_trait::async_trait]
        impl Tool for Exploding {
            fn name(&self) -> &'static str {
                "Boom"
            }
            fn input_schema(&self) -> schemars::Schema {
                schemars::json_schema!({ "type": "object" })
            }
            fn prompt(&self, _: &PromptContext) -> String {
                "boom".into()
            }
            fn describe(&self, _: &serde_json::Value) -> String {
                "boom".into()
            }
            async fn call(&self, _: serde_json::Value, _: ToolContext) -> ToolOutcome {
                panic!("工具炸了");
            }
            fn is_concurrency_safe(&self, _: &serde_json::Value) -> bool {
                true
            }
        }

        let s = scheduler(vec![
            Arc::new(Exploding) as Arc<dyn Tool>,
            Arc::new(FakeTool::read_only("Read")),
        ]);

        let events = run(&s, vec![call("a", "Boom"), call("b", "Read")]).await;
        let pairs = result_pairs(outcome(&events));

        assert_eq!(pairs.len(), 2, "panic 的那个也要有结果");
        assert!(pairs[0].1.contains("Boom"));
        assert_eq!(pairs[1].0, "b", "兄弟不受影响");
    }

    #[tokio::test(start_paused = true)]
    async fn bash_失败级联取消兄弟() {
        // mkdir foo 失败后 cd foo && ... 没有意义
        let hanging = FakeTool::read_only("Hang").hanging();
        let counter = hanging.counter();

        let s = scheduler(vec![
            Arc::new(FakeTool::read_only("Bash").failing("命令失败").cascading()) as Arc<dyn Tool>,
            Arc::new(hanging),
        ]);

        let events = run(&s, vec![call("a", "Bash"), call("b", "Hang")]).await;
        let pairs = result_pairs(outcome(&events));

        assert_eq!(pairs.len(), 2);
        assert!(
            pairs[1].1.contains("跳过"),
            "兄弟应该被跳过：{}",
            pairs[1].1
        );
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "级联必须在执行前阻止它 —— 否则副作用照样发生了，只是结果被丢弃"
        );
        assert_eq!(outcome(&events).cancelled, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn 只读工具失败不级联() {
        // Read 和 Grep 彼此独立，一个失败不该连累另一个
        let ok = FakeTool::read_only("Grep");
        let counter = ok.counter();

        let s = scheduler(vec![
            Arc::new(FakeTool::read_only("Read").failing("文件不存在")) as Arc<dyn Tool>,
            Arc::new(ok),
        ]);

        let events = run(&s, vec![call("a", "Read"), call("b", "Grep")]).await;
        let pairs = result_pairs(outcome(&events));

        assert_eq!(counter.load(Ordering::SeqCst), 1, "兄弟应该正常执行");
        assert!(!pairs[1].1.contains("跳过"));
        assert_eq!(outcome(&events).cancelled, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn 级联不影响后续批次() {
        // 级联用子令牌，所以它只杀同批的兄弟
        let later = FakeTool::writer("Edit");
        let counter = later.counter();

        let s = scheduler(vec![
            Arc::new(FakeTool::read_only("Bash").failing("失败").cascading()) as Arc<dyn Tool>,
            Arc::new(FakeTool::read_only("Hang").hanging()),
            Arc::new(later),
        ]);

        let events = run(
            &s,
            vec![call("a", "Bash"), call("b", "Hang"), call("c", "Edit")],
        )
        .await;

        assert_eq!(result_pairs(outcome(&events)).len(), 3);
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "后续批次不该被上一批的级联波及"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn 用户中断时剩余批次补齐结果() {
        let cancel = CancellationToken::new();
        cancel.cancel();

        let s = scheduler(vec![
            Arc::new(FakeTool::read_only("Read")) as Arc<dyn Tool>,
            Arc::new(FakeTool::writer("Edit")),
        ]);

        let events: Vec<_> = s
            .run_batch(
                vec![call("a", "Read"), call("b", "Edit"), call("c", "Read")],
                ctx(cancel),
            )
            .collect()
            .await;

        let o = outcome(&events);
        assert_eq!(
            result_pairs(o).len(),
            3,
            "中断也要补齐结果，否则下次请求 400"
        );
        assert_eq!(o.cancelled, 3);
    }

    #[tokio::test(start_paused = true)]
    async fn 进度事件在结果之前吐出来() {
        // 攒到批次结束才吐的话，UI 上就是"卡住然后一次刷完"
        let s = scheduler(vec![Arc::new(
            FakeTool::read_only("Bash").with_progress(&["第一行", "第二行"]),
        ) as Arc<dyn Tool>]);

        let events = run(&s, vec![call("a", "Bash")]).await;

        let progress_count = events
            .iter()
            .filter(|e| matches!(e, BatchEvent::Progress { .. }))
            .count();
        assert_eq!(progress_count, 2);

        let done_at = events
            .iter()
            .position(|e| matches!(e, BatchEvent::Done(_)))
            .expect("有 Done");
        assert_eq!(done_at, events.len() - 1, "Done 必须是最后一个");
    }

    #[tokio::test(start_paused = true)]
    async fn 空调用列表也要吐_done() {
        let s = scheduler(vec![Arc::new(FakeTool::read_only("Read")) as Arc<dyn Tool>]);
        let events = run(&s, vec![]).await;

        let o = outcome(&events);
        assert!(result_pairs(o).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn 写工具串行执行() {
        let s = scheduler(vec![Arc::new(FakeTool::writer("Edit")) as Arc<dyn Tool>]);
        let events = run(&s, vec![call("a", "Edit"), call("b", "Edit")]).await;

        let ids: Vec<String> = result_pairs(outcome(&events))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, vec!["a", "b"]);
    }
}
