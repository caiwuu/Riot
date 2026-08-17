//! 长期服务的读与停。起服务在 Bash 的 `background` 参数上。
//!
//! 真正干活的在宿主（[`riot_protocol::terminal::TerminalAccess`]），这一层
//! 只负责参数校验和把结果说成模型读得懂的话。
//!
//! 这两个工具只能碰模型自己起的终端 —— 用户那个 shell 归他自己，边界由
//! 宿主实现把关，见 `term_access`。

use async_trait::async_trait;
use riot_protocol::message::ToolResultContent;
use riot_protocol::tool::{PromptContext, Tool, ToolContext, ToolOutcome};
use serde::Deserialize;

/// 一次最多读多少行。再多模型也读不完，只会把上下文顶掉。
const MAX_LINES: usize = 400;
const DEFAULT_LINES: usize = 80;

/// 等哨兵时的轮询间隔。
///
/// 走注入的 Clock，不是真实时钟 —— 这一层要能被黄金回放驱动。
const POLL_MS: u64 = 250;
const DEFAULT_WAIT_MS: u64 = 30_000;
const MAX_WAIT_MS: u64 = 120_000;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct OutputInput {
    /// 终端 id，起服务时返回的那个。
    id: u32,
    /// 读末尾多少行。默认 80，最多 400。
    #[serde(default)]
    lines: Option<usize>,
    /// 等到输出里出现匹配这个正则的内容再返回。
    ///
    /// 用它代替"读一次、没好、再读一次"的轮询 —— 那样每次都是一轮完整的
    /// 模型调用，几十秒的启动等待能烧掉十几轮。
    #[serde(default)]
    wait_for: Option<String>,
    /// 等多久（毫秒）。默认 30000，最多 120000。
    #[serde(default)]
    wait_timeout_ms: Option<u64>,
}

pub struct TerminalOutput;

#[async_trait]
impl Tool for TerminalOutput {
    fn name(&self) -> &'static str {
        "TerminalOutput"
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(OutputInput)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        format!(
            "读一个后台服务的最新输出（用 Bash 的 background 起的那些）。\n\
             \n\
             - 默认末尾 {DEFAULT_LINES} 行，最多 {MAX_LINES}。\n\
             - 已经去掉颜色和控制字符，进度条只保留最后一次重画。\n\
             - 服务退出后输出仍然读得到 —— 挂了正是要看日志的时候。\n\
             - 能读你自己起的，以及用户在面板上共享给你的那些。别的读不到。\n\
             \n\
             **等服务就绪用 `wait_for`，不要自己轮询。** 传一个正则，\
             它会等到输出里出现匹配的内容才返回（默认最多等 \
             {}s，可用 wait_timeout_ms 调，上限 {}s）：\n\
             \n\
             - 例：起完 dev server 用 `wait_for: \"compiled|ready in|Local:\"`；\
               跑 watch 编译用 `wait_for: \"Finished|error\\\\[\"`。\n\
             - 正则要**同时**匹配成功和失败的标志。只等成功那句的话，\
               启动失败时你会干等到超时，而答案第一秒就在输出里了。\n\
             - 进程在哨兵出现之前就退了会立刻返回失败并附上最后的输出 ——\
               不会白等。\n\
             \n\
             反面：`wait_for` 不是「睡一会儿」。没有哨兵可等就直接读，\
             别拿一个匹配不到的正则当定时器。",
            DEFAULT_WAIT_MS / 1000,
            MAX_WAIT_MS / 1000,
        )
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        match input.get("id").and_then(serde_json::Value::as_u64) {
            Some(id) => format!("读终端 {id} 的输出"),
            None => "读服务输出".to_owned(),
        }
    }

    /// 只是读日志，不动任何东西。可以和别的只读工具并行。
    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let parsed: OutputInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolOutcome::failed(format!("参数不对：{e}")),
        };
        let lines = parsed.lines.unwrap_or(DEFAULT_LINES).clamp(1, MAX_LINES);

        if let Some(pattern) = parsed.wait_for.as_deref() {
            return wait_for_sentinel(pattern, &parsed, lines, &ctx).await;
        }

        match ctx.terminal.read(parsed.id, lines).await {
            Ok(text) if text.trim().is_empty() => ToolOutcome::Ok {
                model_content: ToolResultContent::text(format!(
                    "终端 {} 目前没有输出。",
                    parsed.id
                )),
                ui_payload: None,
                side_messages: Vec::new(),
            },
            Ok(text) => ToolOutcome::Ok {
                model_content: ToolResultContent::text(text),
                ui_payload: None,
                side_messages: Vec::new(),
            },
            Err(e) => ToolOutcome::failed(e.0),
        }
    }
}

/// 等输出里出现哨兵。
///
/// 三条出路，缺一条都会让模型白等：
///
/// 1. 匹配到 → 成功，附上当前输出；
/// 2. 进程先退了 → 立刻失败并附上最后的输出。这是最常见的真实失败
///    （服务启动崩了），等满超时对谁都没好处；
/// 3. 超时 → 失败，同样附上输出。只说"超时了"等于让模型从零猜。
async fn wait_for_sentinel(
    pattern: &str,
    parsed: &OutputInput,
    lines: usize,
    ctx: &ToolContext,
) -> ToolOutcome {
    let re = match regex_lite::Regex::new(pattern) {
        Ok(r) => r,
        Err(e) => {
            return ToolOutcome::failed(format!(
                "`wait_for` 不是合法正则（{e}）。注意 JSON 字符串里反斜杠要写两个。"
            ));
        }
    };
    let budget = parsed
        .wait_timeout_ms
        .unwrap_or(DEFAULT_WAIT_MS)
        .min(MAX_WAIT_MS);
    let deadline = ctx.clock.now_ms().saturating_add(budget);

    loop {
        let text = match ctx.terminal.read(parsed.id, lines).await {
            Ok(t) => t,
            Err(e) => return ToolOutcome::failed(e.0),
        };
        if re.is_match(&text) {
            return ToolOutcome::ok_text(format!("等到了 `{pattern}`。当前输出：\n\n{text}"));
        }

        // 进程退了就别再等。哨兵已经不可能出现了，而它最后那几行正是原因。
        let alive = ctx
            .terminal
            .list()
            .await
            .into_iter()
            .any(|t| t.id == parsed.id && t.running);
        if !alive {
            return ToolOutcome::failed(format!(
                "终端 {} 的进程已经退出，`{pattern}` 始终没出现。以下是它最后的输出：\n\n{text}",
                parsed.id
            ));
        }

        if ctx.clock.now_ms() >= deadline {
            return ToolOutcome::failed(format!(
                "等了 {}s，`{pattern}` 没有出现，但进程还在跑。以下是目前的输出 —— \
                 先看它到了哪一步，再决定是继续等（调大 wait_timeout_ms）还是换个哨兵：\n\n{text}",
                budget / 1000
            ));
        }
        if ctx.cancel.is_cancelled() {
            return ToolOutcome::Cancelled;
        }
        ctx.clock.sleep_ms(POLL_MS).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    use riot_protocol::terminal::{TerminalAccess, TerminalInfo, TerminalUnavailable};
    use riot_protocol::tool::Clock as _;

    use super::*;

    /// 按脚本吐输出的终端替身。
    struct FakeTerm {
        /// 每次 read 返回一条；用完之后重复最后一条（服务安静下来了）。
        script: Mutex<Vec<String>>,
        reads: AtomicU32,
        running: AtomicBool,
    }

    impl FakeTerm {
        fn new(script: &[&str], running: bool) -> Self {
            Self {
                script: Mutex::new(script.iter().map(|s| (*s).to_owned()).collect()),
                reads: AtomicU32::new(0),
                running: AtomicBool::new(running),
            }
        }
    }

    #[async_trait]
    impl TerminalAccess for FakeTerm {
        async fn spawn(&self, _c: &str, _t: &str) -> Result<u32, TerminalUnavailable> {
            Err(TerminalUnavailable("替身不起终端".into()))
        }
        async fn read(&self, _id: u32, _lines: usize) -> Result<String, TerminalUnavailable> {
            let g = self.script.lock().expect("脚本锁");
            let n = self.reads.fetch_add(1, Ordering::SeqCst) as usize;
            Ok(g.get(n).or_else(|| g.last()).cloned().unwrap_or_default())
        }
        async fn kill(&self, _id: u32) -> Result<(), TerminalUnavailable> {
            Ok(())
        }
        async fn list(&self) -> Vec<TerminalInfo> {
            vec![TerminalInfo {
                id: 1,
                title: "t".into(),
                command: Some("serve".into()),
                running: self.running.load(Ordering::SeqCst),
            }]
        }
    }

    fn ctx_with(term: Arc<FakeTerm>, clock: Arc<crate::testing::FixedClock>) -> ToolContext {
        let id = riot_protocol::id::ToolUseId::from_raw("t1");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        ToolContext {
            session_id: riot_protocol::id::SessionId::from_raw("s1"),
            tool_use_id: id.clone(),
            cwd: "/work".into(),
            artifacts_dir: "/artifacts".into(),
            cancel: tokio_util::sync::CancellationToken::new(),
            progress: riot_protocol::tool::ProgressSink::new(id, tx),
            file_state: Arc::new(crate::testing::NullFileState),
            fs: Arc::new(crate::tools::memfs::MemFs::new()),
            proc: Arc::new(crate::testing::NullProc),
            web: Arc::new(riot_protocol::web::NoWeb),
            browser: Arc::new(riot_protocol::browser::NoBrowser),
            terminal: term,
            vision: Arc::new(riot_protocol::vision::NoVision),
            clock,
        }
    }

    fn input(wait_for: &str) -> OutputInput {
        OutputInput {
            id: 1,
            lines: None,
            wait_for: Some(wait_for.to_owned()),
            wait_timeout_ms: None,
        }
    }

    #[tokio::test]
    async fn 等到哨兵就返回() {
        let term = Arc::new(FakeTerm::new(
            &["starting...", "starting...\nbuilding", "starting...\nready in 412ms"],
            true,
        ));
        let clock = Arc::new(crate::testing::FixedClock::default());
        let ctx = ctx_with(Arc::clone(&term), Arc::clone(&clock));

        let out = wait_for_sentinel("ready in", &input("ready in"), 80, &ctx).await;
        let ToolOutcome::Ok { model_content, .. } = out else {
            panic!("该等到：{out:?}");
        };
        assert!(format!("{model_content:?}").contains("ready in 412ms"));
        assert!(clock.now_ms() < DEFAULT_WAIT_MS, "该提前返回，不是等满超时");
    }

    /// 进程在哨兵出现之前就退了 —— 最常见的真实失败（服务启动崩了）。
    ///
    /// 必须**立刻**返回：等满 30 秒对谁都没好处，而它最后那几行正是原因。
    #[tokio::test]
    async fn 进程先退了就立刻失败并附上最后的输出() {
        // 别用 "already in use" 当样本 —— 它里面就含 "ready"，会被哨兵匹上。
        // 这是子串匹配的经典坑，写正则的人（和模型）都会踩。
        let term = Arc::new(FakeTerm::new(&["Error: EADDRINUSE port 3000"], false));
        let clock = Arc::new(crate::testing::FixedClock::default());
        let ctx = ctx_with(Arc::clone(&term), Arc::clone(&clock));

        let out = wait_for_sentinel("ready", &input("ready"), 80, &ctx).await;
        let ToolOutcome::Failed { error_for_model, .. } = out else {
            panic!("该失败：{out:?}");
        };
        assert!(error_for_model.contains("已经退出"), "{error_for_model}");
        assert!(
            error_for_model.contains("EADDRINUSE"),
            "要附上最后的输出，它才是原因：{error_for_model}"
        );
        assert_eq!(clock.now_ms(), 0, "不该等，一次都不该睡");
    }

    /// 超时也要把当前输出给出去。只说"超时了"等于让模型从零开始猜。
    #[tokio::test]
    async fn 超时失败要附上目前的输出和下一步() {
        let term = Arc::new(FakeTerm::new(&["installing deps..."], true));
        let clock = Arc::new(crate::testing::FixedClock::default());
        let ctx = ctx_with(Arc::clone(&term), Arc::clone(&clock));

        let out = wait_for_sentinel("ready", &input("ready"), 80, &ctx).await;
        let ToolOutcome::Failed { error_for_model, .. } = out else {
            panic!("该超时失败：{out:?}");
        };
        assert!(error_for_model.contains("installing deps"), "{error_for_model}");
        assert!(
            error_for_model.contains("wait_timeout_ms"),
            "要给下一步，否则模型只会原样再等一次：{error_for_model}"
        );
        assert!(clock.now_ms() >= DEFAULT_WAIT_MS, "该等满预算");
    }

    #[tokio::test]
    async fn 正则写错要说清怎么改() {
        let term = Arc::new(FakeTerm::new(&["x"], true));
        let clock = Arc::new(crate::testing::FixedClock::default());
        let ctx = ctx_with(term, clock);
        let out = wait_for_sentinel("[unclosed", &input("[unclosed"), 80, &ctx).await;
        let ToolOutcome::Failed { error_for_model, .. } = out else {
            panic!("非法正则该失败：{out:?}");
        };
        assert!(error_for_model.contains("反斜杠"), "要提醒 JSON 转义：{error_for_model}");
    }

    #[test]
    fn 等待预算被夹在上限内() {
        // 模型传一个 wait_timeout_ms: 3600000 就等于把整轮挂在那里。
        let over = OutputInput {
            id: 1,
            lines: None,
            wait_for: Some("x".into()),
            wait_timeout_ms: Some(9_999_999),
        };
        assert_eq!(
            over.wait_timeout_ms.unwrap_or(DEFAULT_WAIT_MS).min(MAX_WAIT_MS),
            MAX_WAIT_MS
        );
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct KillInput {
    /// 要停的终端 id。
    id: u32,
}

pub struct TerminalKill;

#[async_trait]
impl Tool for TerminalKill {
    fn name(&self) -> &'static str {
        "TerminalKill"
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(KillInput)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "停掉一个后台服务，并把它的终端标签关掉。\n\
         \n\
         - 只能停你自己起的。\n\
         - 任务做完记得把起过的服务停掉，除非用户还要接着用。"
            .to_owned()
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        match input.get("id").and_then(serde_json::Value::as_u64) {
            Some(id) => format!("停掉终端 {id}"),
            None => "停掉服务".to_owned(),
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        false
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let parsed: KillInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolOutcome::failed(format!("参数不对：{e}")),
        };
        match ctx.terminal.kill(parsed.id).await {
            Ok(()) => ToolOutcome::Ok {
                model_content: ToolResultContent::text(format!("终端 {} 已停。", parsed.id)),
                ui_payload: None,
                side_messages: Vec::new(),
            },
            Err(e) => ToolOutcome::failed(e.0),
        }
    }
}
