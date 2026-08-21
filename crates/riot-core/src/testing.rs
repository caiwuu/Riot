//! 测试替身。
//!
//! 这些存在的唯一理由是让黄金回放能控制非确定性。每一个都对应
//! docs/VERIFICATION.md §4.2 表格里的一行。
//!
//! 无条件编译（不加 feature gate）是有意的：宿主层的集成测试也要用它们，
//! 加 feature 会导致 core 被编译两遍。代价是生产二进制里多几百行死代码，
//! 换来的是构建图简单。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use async_trait::async_trait;
use riot_protocol::event::ProgressPayload;
use riot_protocol::id::{IdGenerator, MessageId, ToolUseId};
use riot_protocol::message::{Message, MessageMeta, ToolResultContent, UserContent};
use riot_protocol::provider::{Provider, ProviderEvent, ProviderRequest, ProviderStream, ToolSpec};
use riot_protocol::tool::Clock;
use tokio_util::sync::CancellationToken;

use crate::state::{BatchContext, BatchEvent, BatchOutcome, BatchStream, ToolCall, ToolRunner};

// ────────────────────────────────────────────────────────────
// 时间
// ────────────────────────────────────────────────────────────

/// 手动控制的时钟。`sleep_ms` 直接把时间往前拨，不真的等。
///
/// 这让「microcompact 的 60 分钟缓存冷热判断」这类逻辑能在毫秒内测完，
/// 而不是让测试真的睡一小时或者去 mock tokio 的时间轮。
#[derive(Debug)]
pub struct MockClock {
    now_ms: AtomicU64,
}

impl MockClock {
    pub fn new(start_ms: u64) -> Self {
        Self {
            now_ms: AtomicU64::new(start_ms),
        }
    }

    pub fn advance_ms(&self, ms: u64) {
        self.now_ms.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Default for MockClock {
    fn default() -> Self {
        // 2024-01-01T00:00:00Z。固定值，不是 now() —— 回放要可复现。
        Self::new(1_704_067_200_000)
    }
}

#[async_trait]
impl Clock for MockClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.load(Ordering::SeqCst)
    }

    async fn sleep_ms(&self, ms: u64) {
        self.advance_ms(ms);
    }
}

// ────────────────────────────────────────────────────────────
// ID
// ────────────────────────────────────────────────────────────

/// 按前缀分别计数的确定性 ID 生成器：`msg_1`、`msg_2`、`toolu_1`……
///
/// 分前缀计数而不是用全局序号，是为了让用例里的 ID 稳定 —— 全局序号下，
/// 中间插入一个 message 会让后面所有 tool_use_id 全变，diff 没法看。
#[derive(Debug, Default)]
pub struct SeqIdGenerator {
    counters: std::sync::Mutex<HashMap<String, u32>>,
}

impl IdGenerator for SeqIdGenerator {
    fn next_id(&self, prefix: &str) -> String {
        let mut c = self.counters.lock().expect("id counter poisoned");
        let n = c.entry(prefix.to_string()).or_insert(0);
        *n += 1;
        format!("{prefix}_{n}")
    }
}

// ────────────────────────────────────────────────────────────
// Provider
// ────────────────────────────────────────────────────────────

/// 按脚本逐次返回预录的响应。
///
/// 第 n 次调用 `stream()` 返回 `responses[n]`。脚本用完还被调用会返回一个
/// 说明性的错误而不是 panic —— panic 在 stream 里会被 `futures` 吞成
/// 一个难懂的测试失败，显式错误更好定位。
pub struct ScriptedProvider {
    responses: Vec<Vec<ProviderEvent>>,
    cursor: AtomicUsize,
    /// 每次调用收到的请求，供测试断言。
    seen: std::sync::Mutex<Vec<ProviderRequest>>,
}

impl ScriptedProvider {
    pub fn new(responses: Vec<Vec<ProviderEvent>>) -> Self {
        Self {
            responses,
            cursor: AtomicUsize::new(0),
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// 已消费的响应数。用来断言「主循环发了几次请求」。
    pub fn call_count(&self) -> usize {
        self.cursor.load(Ordering::SeqCst)
    }

    pub fn requests(&self) -> Vec<ProviderRequest> {
        self.seen.lock().expect("seen poisoned").clone()
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn stream(&self, req: ProviderRequest, _cancel: CancellationToken) -> ProviderStream {
        let n = self.cursor.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().expect("seen poisoned").push(req);

        let events = self.responses.get(n).cloned().unwrap_or_else(|| {
            vec![ProviderEvent::Error(
                riot_protocol::provider::ProviderError::Transport {
                    message: format!(
                        "脚本只有 {} 个响应，但主循环发起了第 {} 次请求。\
                         要么用例缺响应，要么主循环多转了一圈。",
                        self.responses.len(),
                        n + 1
                    ),
                },
            )]
        });

        Box::pin(futures::stream::iter(events))
    }

    fn count_tokens(&self, messages: &[Message]) -> u32 {
        // 粗略估算。回放测试不关心精度，只关心确定性。
        let chars: usize = messages
            .iter()
            .map(|m| serde_json::to_string(m).map(|s| s.len()).unwrap_or(0))
            .sum();
        (chars / 4) as u32
    }
}

// ────────────────────────────────────────────────────────────
// 工具
// ────────────────────────────────────────────────────────────

/// 一个工具调用的预设结果。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ScriptedResult {
    Ok { text: String },
    Failed { error_for_model: String },
    Cancelled,
}

/// 按工具名返回预设结果。
///
/// `[约束]` 它必须保证结果保序且不漏 —— 这正是真实 ToolRunner 最容易写错的
/// 地方（并发执行后按完成顺序收集，而不是按调用顺序）。测试替身如果不保序，
/// 回放用例就会随机失败，然后大家会以为是测试不稳定而不是实现有 bug。
pub struct ScriptedToolRunner {
    results: HashMap<String, ScriptedResult>,
    specs: Vec<ToolSpec>,
    /// 每个工具产生几条进度事件。默认 0。
    progress_per_tool: usize,
}

impl ScriptedToolRunner {
    pub fn new(results: HashMap<String, ScriptedResult>) -> Self {
        let specs = results
            .keys()
            .map(|name| ToolSpec {
                name: name.clone(),
                description: format!("scripted {name}"),
                input_schema: serde_json::json!({ "type": "object" }),
            })
            .collect();
        Self {
            results,
            specs,
            progress_per_tool: 0,
        }
    }

    pub fn with_progress(mut self, n: usize) -> Self {
        self.progress_per_tool = n;
        self
    }
}

impl ToolRunner for ScriptedToolRunner {
    fn specs(&self) -> Vec<ToolSpec> {
        let mut s = self.specs.clone();
        // HashMap 的迭代顺序不确定，排序保证请求内容可复现。
        s.sort_by(|a, b| a.name.cmp(&b.name));
        s
    }

    fn run_batch(&self, calls: Vec<ToolCall>, ctx: BatchContext) -> BatchStream {
        let mut events = Vec::new();
        let mut contents = Vec::new();
        let mut cancelled = 0;

        for call in &calls {
            for i in 0..self.progress_per_tool {
                events.push(BatchEvent::Progress {
                    tool_use_id: call.id.clone(),
                    payload: ProgressPayload::Status {
                        text: format!("step {i}"),
                    },
                });
            }

            // 已取消时后续工具一律记为取消，但**仍然产出 tool_result** ——
            // 少一个配对，下次请求就是 400。
            let scripted = if ctx.cancel.is_cancelled() {
                ScriptedResult::Cancelled
            } else {
                self.results
                    .get(&call.name)
                    .cloned()
                    .unwrap_or(ScriptedResult::Failed {
                        error_for_model: format!("用例没有为工具 {} 预设结果", call.name),
                    })
            };

            let (content, is_error) = match scripted {
                ScriptedResult::Ok { text } => (ToolResultContent::text(text), false),
                ScriptedResult::Failed { error_for_model } => {
                    (ToolResultContent::text(error_for_model), true)
                }
                ScriptedResult::Cancelled => {
                    cancelled += 1;
                    (ToolResultContent::text("已取消"), true)
                }
            };

            contents.push(UserContent::ToolResult {
                tool_use_id: call.id.clone(),
                content,
                is_error,
            });
        }

        events.push(BatchEvent::Done(BatchOutcome {
            results: Message::User {
                id: MessageId::from_raw(format!("{}_results", ctx.session_id.as_str())),
                content: contents,
                meta: MessageMeta {
                    synthetic: true,
                    ..Default::default()
                },
            },
            side_messages: Vec::new(),
            cancelled,
        }));

        Box::pin(futures::stream::iter(events))
    }
}

// ────────────────────────────────────────────────────────────
// 故障注入（L4）
// ────────────────────────────────────────────────────────────

/// 确定性伪随机。
///
/// 自己写而不是引 `rand`，是因为这里需要的是「同一个 seed 永远产生同一串
/// 数」这个保证跨版本稳定。`rand` 的算法在大版本间换过，那会让混沌测试
/// 报告的 seed 在升级依赖后失去复现价值 —— 而 seed 能复现正是它的全部意义。
#[derive(Debug)]
pub struct XorShift(u64);

impl XorShift {
    pub fn new(seed: u64) -> Self {
        // 0 是 xorshift 的不动点，会一直吐 0。
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

/// 每次请求随机挑一种行为的 provider。
///
/// 断言的不是「输出对不对」，而是「不管怎么坏，主循环都能干净收场」。
pub struct ChaosProvider {
    rng: std::sync::Mutex<XorShift>,
    calls: AtomicUsize,
}

impl ChaosProvider {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: std::sync::Mutex::new(XorShift::new(seed)),
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl Provider for ChaosProvider {
    fn stream(&self, _req: ProviderRequest, _cancel: CancellationToken) -> ProviderStream {
        use riot_protocol::provider::ProviderError as E;

        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let pick = self.rng.lock().expect("rng poisoned").below(9);

        let events = match pick {
            // 空流：什么都没说就结束了。主循环必须当成「没有 tool_use」收场，
            // 而不是卡住等一个永远不来的消息。
            0 => vec![],
            1 => vec![ProviderEvent::Message(assistant_text(
                &format!("msg_{n}"),
                "结束",
            ))],
            2 => vec![ProviderEvent::Message(assistant_tool_use(
                &format!("msg_{n}"),
                &format!("tu_{n}"),
                "Read",
                serde_json::json!({ "path": "x" }),
            ))],
            3 => vec![ProviderEvent::Error(E::ContextOverflow {
                used: 200_000,
                limit: 180_000,
            })],
            4 => vec![ProviderEvent::Error(E::OutputLimit)],
            5 => vec![ProviderEvent::Error(E::Auth {
                message: "401".into(),
            })],
            6 => vec![ProviderEvent::Error(E::RetriesExhausted {
                message: "529 x3".into(),
            })],
            // 先吐半截内容再出错：扣留机制必须丢掉这半截，
            // 否则 transcript 里会留下没有 tool_result 的 tool_use。
            7 => vec![
                ProviderEvent::Message(assistant_tool_use(
                    &format!("msg_{n}"),
                    &format!("tu_{n}"),
                    "Read",
                    serde_json::json!({ "path": "half" }),
                )),
                ProviderEvent::Error(E::OutputLimit),
            ],
            _ => vec![
                ProviderEvent::Delta(riot_protocol::event::StreamDelta::Text {
                    message_id: riot_protocol::id::MessageId::from_raw(format!("msg_{n}")),
                    text: "思考中".into(),
                }),
                ProviderEvent::Message(assistant_text(&format!("msg_{n}"), "好了")),
            ],
        };

        Box::pin(futures::stream::iter(events))
    }

    fn count_tokens(&self, messages: &[Message]) -> u32 {
        (messages.len() * 100) as u32
    }
}

/// 会违约的 ToolRunner。
///
/// 真实的 ToolRunner 由别人（或者 AI）实现，主循环不能假设它守规矩。
/// 这个替身把「不守规矩」变成可测的。
pub struct BreachingToolRunner {
    pub breach: Breach,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Breach {
    /// 流结束了但没发 `BatchEvent::Done`。
    NoDone,
    /// 结果数量对不上调用数量。
    MissingResults,
    /// 结果顺序被打乱（并发收集时按完成顺序而不是调用顺序）。
    ShuffledResults,
}

impl ToolRunner for BreachingToolRunner {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "Read".into(),
            description: "breaching".into(),
            input_schema: serde_json::json!({ "type": "object" }),
        }]
    }

    fn run_batch(&self, calls: Vec<ToolCall>, ctx: BatchContext) -> BatchStream {
        if self.breach == Breach::NoDone {
            return Box::pin(futures::stream::iter(Vec::new()));
        }

        let mut contents: Vec<UserContent> = calls
            .iter()
            .map(|c| UserContent::ToolResult {
                tool_use_id: c.id.clone(),
                content: ToolResultContent::text("ok"),
                is_error: false,
            })
            .collect();

        match self.breach {
            Breach::MissingResults => {
                contents.pop();
            }
            Breach::ShuffledResults => contents.reverse(),
            Breach::NoDone => unreachable!(),
        }

        Box::pin(futures::stream::iter(vec![BatchEvent::Done(
            BatchOutcome {
                results: Message::User {
                    id: MessageId::from_raw(format!("{}_results", ctx.session_id.as_str())),
                    content: contents,
                    meta: MessageMeta {
                        synthetic: true,
                        ..Default::default()
                    },
                },
                side_messages: Vec::new(),
                cancelled: 0,
            },
        )]))
    }
}

// ────────────────────────────────────────────────────────────
// 压缩
// ────────────────────────────────────────────────────────────

/// 假压缩器：把旧消息的**内容**掏空，但保留消息本身。
///
/// `[约束]` 这里不能图省事直接删掉旧消息。删一条带 tool_use 的 assistant
/// 消息，它的 tool_result 就成了「无来源」的孤儿，下一次 API 请求 400。
///
/// 混沌测试第一次跑就抓到了这个 —— 最初的实现是「保留首尾，丢掉中间」，
/// 500 个 seed 里有 10 个踩中。真实压缩器会掉进同一个坑，所以连测试替身
/// 也必须守这条契约，否则它会把真实现的同类 bug 掩盖掉。
pub struct FakeCompactor {
    /// 末尾保留多少条消息的完整内容。
    keep: usize,
    /// 前 n 次压缩返回失败，用来测熔断。
    fail_first: std::sync::atomic::AtomicUsize,
}

impl FakeCompactor {
    pub fn new(keep: usize) -> Self {
        Self {
            keep,
            fail_first: AtomicUsize::new(0),
        }
    }

    pub fn failing(times: usize) -> Self {
        Self {
            keep: 2,
            fail_first: AtomicUsize::new(times),
        }
    }
}

impl Default for FakeCompactor {
    fn default() -> Self {
        Self::new(2)
    }
}

#[async_trait]
impl riot_protocol::compact::Compactor for FakeCompactor {
    async fn compact(
        &self,
        messages: Vec<Message>,
        budget: riot_protocol::compact::CompactBudget,
    ) -> riot_protocol::compact::CompactResult {
        let remaining = self.fail_first.load(Ordering::SeqCst);
        if remaining > 0 {
            self.fail_first.store(remaining - 1, Ordering::SeqCst);
            return riot_protocol::compact::CompactResult::Failed {
                reason: "假压缩器按配置失败".into(),
            };
        }

        // 末尾 keep 条保持原样，更早的把内容掏空。消息条数不变 ——
        // 这正是保持配对的关键。
        let cutoff = messages.len().saturating_sub(self.keep);
        let kept: Vec<Message> = messages
            .into_iter()
            .enumerate()
            .map(|(i, m)| if i < cutoff { clear_content(m) } else { m })
            .collect();

        riot_protocol::compact::CompactResult::Compacted {
            before_tokens: budget.current_tokens,
            after_tokens: budget.target_tokens,
            strategy: riot_protocol::event::CompactStrategy::FullSummary,
            messages: kept,
        }
    }
}

/// 把一条消息的内容掏空，但保留它的结构与 ID。
///
/// tool_result 换成 `Cleared` 占位符而不是删掉 —— 这是唯一能既省 token
/// 又不破坏配对的做法。
fn clear_content(m: Message) -> Message {
    match m {
        Message::User { id, content, meta } => Message::User {
            id,
            content: content
                .into_iter()
                .map(|c| match c {
                    UserContent::ToolResult {
                        tool_use_id,
                        is_error,
                        ..
                    } => UserContent::ToolResult {
                        tool_use_id,
                        content: ToolResultContent::Cleared,
                        is_error,
                    },
                    other => other,
                })
                .collect(),
            meta,
        },
        // assistant 消息里的 tool_use 必须原样保留 —— 它是配对的另一半。
        // 只压缩 text 部分。
        Message::Assistant {
            id,
            content,
            usage,
            meta,
        } => Message::Assistant {
            id,
            content: content
                .into_iter()
                .map(|c| match c {
                    riot_protocol::message::AssistantContent::Text { .. } => {
                        riot_protocol::message::AssistantContent::Text {
                            text: "[已压缩]".into(),
                        }
                    }
                    other => other,
                })
                .collect(),
            usage,
            meta,
        },
        system => system,
    }
}

// ────────────────────────────────────────────────────────────
// 组装
// ────────────────────────────────────────────────────────────

/// 拼一套全 mock 的依赖。
pub fn mock_deps(
    provider: Arc<ScriptedProvider>,
    tools: Arc<ScriptedToolRunner>,
) -> crate::state::AgentDeps {
    mock_deps_with(provider, tools, Arc::new(FakeCompactor::default()))
}

pub fn mock_deps_with(
    provider: Arc<dyn Provider>,
    tools: Arc<dyn ToolRunner>,
    compactor: Arc<dyn riot_protocol::compact::Compactor>,
) -> crate::state::AgentDeps {
    crate::state::AgentDeps {
        provider,
        compactor,
        clock: Arc::new(MockClock::default()),
        ids: Arc::new(SeqIdGenerator::default()),
        tools,
        queue: Arc::new(crate::state::NoQueue),
        stop_gate: Arc::new(crate::state::NoStopGate),
    }
}

/// 一个方便构造 tool_use 响应的辅助。
pub fn assistant_tool_use(
    msg_id: &str,
    tool_id: &str,
    name: &str,
    input: serde_json::Value,
) -> Message {
    Message::Assistant {
        id: MessageId::from_raw(msg_id),
        content: vec![riot_protocol::message::AssistantContent::ToolUse {
            id: ToolUseId::from_raw(tool_id),
            name: name.into(),
            input,
        }],
        usage: None,
        meta: MessageMeta::default(),
    }
}

/// 跑轮中插话的脚本队列。
///
/// 每次 `drain` 弹出一个批次（可以是空批次，用来跳过一个 drain 点）。
/// 主循环的 drain 点顺序是固定的：每轮工具结果就位后一次、模型正常
/// 收尾前一次 —— 测试按这个顺序摆批次，就能精确控制"插话到达的时机"。
pub struct ScriptedQueue {
    batches: std::sync::Mutex<std::collections::VecDeque<Vec<Message>>>,
    drains: AtomicUsize,
}

impl ScriptedQueue {
    pub fn new(batches: Vec<Vec<Message>>) -> Self {
        Self {
            batches: std::sync::Mutex::new(batches.into()),
            drains: AtomicUsize::new(0),
        }
    }

    /// 主循环一共 drain 了几次。
    pub fn drain_count(&self) -> usize {
        self.drains.load(Ordering::SeqCst)
    }
}

impl crate::state::InputQueue for ScriptedQueue {
    fn drain(&self) -> Vec<Message> {
        self.drains.fetch_add(1, Ordering::SeqCst);
        self.batches
            .lock()
            .expect("batches poisoned")
            .pop_front()
            .unwrap_or_default()
    }
}

/// 收尾闸的脚本替身。每次 `check` 弹出一个预设裁决，弹完了默认放行。
pub struct ScriptedStopGate {
    decisions: std::sync::Mutex<std::collections::VecDeque<crate::state::StopDecision>>,
    /// 每次 check 收到的 blocks_so_far，供断言"计数有没有透传"。
    seen: std::sync::Mutex<Vec<u32>>,
}

impl ScriptedStopGate {
    pub fn new(decisions: Vec<crate::state::StopDecision>) -> Self {
        Self {
            decisions: std::sync::Mutex::new(decisions.into()),
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn seen(&self) -> Vec<u32> {
        self.seen.lock().expect("seen poisoned").clone()
    }
}

#[async_trait]
impl crate::state::StopGate for ScriptedStopGate {
    async fn check(&self, blocks_so_far: u32) -> crate::state::StopDecision {
        self.seen.lock().expect("seen poisoned").push(blocks_so_far);
        self.decisions
            .lock()
            .expect("decisions poisoned")
            .pop_front()
            .unwrap_or(crate::state::StopDecision::Allow)
    }
}

pub fn assistant_text(msg_id: &str, text: &str) -> Message {
    Message::Assistant {
        id: MessageId::from_raw(msg_id),
        content: vec![riot_protocol::message::AssistantContent::Text { text: text.into() }],
        usage: None,
        meta: MessageMeta::default(),
    }
}

pub fn user_text(msg_id: &str, text: &str) -> Message {
    Message::User {
        id: MessageId::from_raw(msg_id),
        content: vec![UserContent::Text { text: text.into() }],
        meta: MessageMeta::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_按前缀分别计数() {
        let g = SeqIdGenerator::default();
        assert_eq!(g.message_id().as_str(), "msg_1");
        assert_eq!(g.message_id().as_str(), "msg_2");
        assert_eq!(
            g.tool_use_id().as_str(),
            "tu_1",
            "换前缀应该重新计数，否则中间插一条消息会让所有 tool id 平移"
        );
    }

    #[tokio::test]
    async fn 时钟可以快进而不真的等() {
        let c = MockClock::new(1000);
        c.sleep_ms(3_600_000).await;
        assert_eq!(c.now_ms(), 1000 + 3_600_000);
    }

    #[test]
    fn 脚本用完时给出可定位的错误() {
        let p = ScriptedProvider::new(vec![]);
        let stream = p.stream(
            ProviderRequest {
                model: "m".into(),
                messages: vec![],
                system: String::new(),
                tools: vec![],
                max_output_tokens: None,
                thinking: Default::default(),
            },
            CancellationToken::new(),
        );
        let events: Vec<_> =
            futures::executor::block_on(futures::StreamExt::collect::<Vec<_>>(stream));
        match &events[0] {
            ProviderEvent::Error(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("第 1 次请求"),
                    "错误要能定位到第几次调用：{msg}"
                );
            }
            other => panic!("应该是错误：{other:?}"),
        }
    }
}
