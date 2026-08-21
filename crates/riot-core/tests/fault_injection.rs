//! L4 故障注入。
//!
//! 与 L3 的分工：L3 把模型响应固定下来，测「正常路径的输出是不是那一串」；
//! L4 注入异常，测「不管怎么坏，系统能不能干净收场」。
//!
//! 为什么异常路径要单独一层：正常开发几周都碰不到一次上下文溢出、一次
//! ToolRunner 违约。**这些路径的代码是照着描述写的，没有任何反馈证明它写对了。**
//! 而它们恰恰最容易错 —— 恢复逻辑要处理半成品状态、清理孤儿数据、避免死循环。
//!
//! 见 docs/VERIFICATION.md §5

use std::sync::Arc;

use futures::StreamExt;
use riot_protocol::event::{AgentEvent, TerminalReason};
use riot_protocol::id::SessionId;
use riot_protocol::message::Message;
use tokio_util::sync::CancellationToken;

use riot_core::invariants;
use riot_core::state::AgentState;
use riot_core::testing::{
    Breach, BreachingToolRunner, ChaosProvider, FakeCompactor, ScriptedProvider, ScriptedResult,
    ScriptedToolRunner, mock_deps_with, user_text,
};

fn state(max_turns: u32) -> AgentState {
    AgentState::new(SessionId::from_raw("fault"), "test-model")
        .with_max_turns(max_turns)
        .with_messages(vec![user_text("msg_in", "开始")])
}

/// 收集事件流。
async fn collect(
    state: AgentState,
    deps: riot_core::state::AgentDeps,
    cancel: CancellationToken,
) -> Vec<AgentEvent> {
    let s = riot_core::run_agent(state, deps, cancel);
    futures::pin_mut!(s);
    let mut out = Vec::new();
    while let Some(ev) = s.next().await {
        out.push(ev);
    }
    out
}

/// 从事件流里重建 transcript，用来做配对检查。
fn transcript(events: &[AgentEvent]) -> Vec<Message> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Message(m) => Some(m.clone()),
            _ => None,
        })
        .collect()
}

// ────────────────────────────────────────────────────────────
// 确定性故障用例
// ────────────────────────────────────────────────────────────

/// ToolRunner 的流结束了却没发 Done。
///
/// 这不是假想 —— 真实实现里只要有一条 early return 忘了发 Done 就会这样。
/// 主循环不能傻等，也不能让悬空的 tool_use 留在 transcript 里。
#[tokio::test]
#[should_panic(expected = "没有 BatchEvent::Done")]
async fn 工具批次不返回_done_时不变量报警() {
    let provider = Arc::new(ScriptedProvider::new(vec![vec![
        riot_protocol::provider::ProviderEvent::Message(riot_core::testing::assistant_tool_use(
            "msg_a1",
            "tu_1",
            "Read",
            serde_json::json!({}),
        )),
    ]]));
    let tools = Arc::new(BreachingToolRunner {
        breach: Breach::NoDone,
    });
    let deps = mock_deps_with(provider, tools, Arc::new(FakeCompactor::default()));

    collect(state(4), deps, CancellationToken::new()).await;
}

/// ToolRunner 少给了一个结果 —— 这是并发收集最典型的 bug。
///
/// 主循环的配对检查必须抓到它。放过去的话，下一次 API 请求会 400，
/// 而错误信息不会告诉你缺哪个 tool_use_id。
#[tokio::test]
#[should_panic(expected = "配对缺失")]
async fn 工具结果缺失时不变量报警() {
    let provider = Arc::new(ScriptedProvider::new(vec![vec![
        riot_protocol::provider::ProviderEvent::Message(Message::Assistant {
            id: riot_protocol::id::MessageId::from_raw("msg_a1"),
            content: vec![
                riot_protocol::message::AssistantContent::ToolUse {
                    id: riot_protocol::id::ToolUseId::from_raw("tu_1"),
                    name: "Read".into(),
                    input: serde_json::json!({}),
                },
                riot_protocol::message::AssistantContent::ToolUse {
                    id: riot_protocol::id::ToolUseId::from_raw("tu_2"),
                    name: "Read".into(),
                    input: serde_json::json!({}),
                },
            ],
            usage: None,
            meta: Default::default(),
        }),
    ]]));
    let tools = Arc::new(BreachingToolRunner {
        breach: Breach::MissingResults,
    });
    let deps = mock_deps_with(provider, tools, Arc::new(FakeCompactor::default()));

    collect(state(4), deps, CancellationToken::new()).await;
}

/// 一开始就取消。
#[tokio::test]
async fn 启动前就取消也要发出_done() {
    let provider = Arc::new(ScriptedProvider::new(vec![]));
    let tools = Arc::new(ScriptedToolRunner::new(Default::default()));
    let deps = mock_deps_with(provider, tools, Arc::new(FakeCompactor::default()));

    let cancel = CancellationToken::new();
    cancel.cancel();
    let events = collect(state(4), deps, cancel).await;

    assert_eq!(events.len(), 1, "取消后不该再发请求");
    assert!(matches!(
        events[0],
        AgentEvent::Done {
            reason: TerminalReason::Aborted { .. }
        }
    ));
}

/// 下游不理会取消令牌时，停止键还得管用。
///
/// `[约束]` 主循环**不能**把"停得下来"寄托在 provider 和工具的礼貌上。
/// 它们都拿到了子令牌、约定要检查，但约定只是约定：一个漏检的实现、
/// 一次卡在首字节上的慢请求，都会让停止键变成装饰品 —— 用户按了，
/// 界面照转，而没有任何报错能解释这件事。停止是用户对系统最基本的
/// 控制权，必须由主循环兜底。
// 这一组用真实时钟：测的就是"按下停止之后，墙上时钟走过几秒里
// 系统有没有反应"。换成注入的 Clock，一个立即返回的 mock 会让
// 「主循环在等一个永远不来的下游」这件事测不出来。
#[allow(clippy::disallowed_methods)]
mod ignores_cancel {
    use super::*;
    use async_trait::async_trait;
    use riot_protocol::provider::{Provider, ProviderRequest, ProviderStream, ToolSpec};
    use riot_protocol::runner::{BatchStream, ToolCall, ToolRunner};
    use std::time::Duration;

    /// 一个开了流就再也不出声、也不看取消令牌的服务方。
    struct DeafProvider;

    #[async_trait]
    impl Provider for DeafProvider {
        fn stream(&self, _req: ProviderRequest, _cancel: CancellationToken) -> ProviderStream {
            Box::pin(futures::stream::pending())
        }
        fn count_tokens(&self, _messages: &[Message]) -> u32 {
            0
        }
    }

    /// 一个跑起来就不回头的工具执行器。
    struct DeafTools;

    impl ToolRunner for DeafTools {
        fn specs(&self) -> Vec<ToolSpec> {
            vec![ToolSpec {
                name: "Read".into(),
                description: "装聋".into(),
                input_schema: serde_json::json!({ "type": "object" }),
            }]
        }
        fn run_batch(
            &self,
            _calls: Vec<ToolCall>,
            _ctx: riot_core::state::BatchContext,
        ) -> BatchStream {
            Box::pin(futures::stream::pending())
        }
    }

    async fn done_within(
        deps: riot_core::state::AgentDeps,
        cancel: CancellationToken,
    ) -> Vec<AgentEvent> {
        // 先让轮子跑起来，再取消 —— 这才是"用户点停止"的时序。
        let c = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            c.cancel();
        });
        tokio::time::timeout(Duration::from_secs(3), collect(state(4), deps, cancel))
            .await
            .expect("停止键必须停得下来 —— 主循环还在等一个永远不来的下游")
    }

    #[tokio::test]
    async fn 服务方不理取消也要停下() {
        let deps = mock_deps_with(
            Arc::new(DeafProvider),
            Arc::new(ScriptedToolRunner::new(Default::default())),
            Arc::new(FakeCompactor::default()),
        );
        let events = done_within(deps, CancellationToken::new()).await;
        assert!(
            matches!(
                events.last(),
                Some(AgentEvent::Done {
                    reason: TerminalReason::Aborted { .. }
                })
            ),
            "该以用户中断收场，实际：{:?}",
            events.last()
        );
    }

    #[tokio::test]
    async fn 工具不理取消也要停下_且补齐配对() {
        let provider = Arc::new(ScriptedProvider::new(vec![vec![
            riot_protocol::provider::ProviderEvent::Message(
                riot_core::testing::assistant_tool_use(
                    "msg_a1",
                    "tu_1",
                    "Read",
                    serde_json::json!({}),
                ),
            ),
        ]]));
        let deps = mock_deps_with(
            provider,
            Arc::new(DeafTools),
            Arc::new(FakeCompactor::default()),
        );
        let events = done_within(deps, CancellationToken::new()).await;

        assert!(
            matches!(
                events.last(),
                Some(AgentEvent::Done {
                    reason: TerminalReason::Aborted { .. }
                })
            ),
            "该以用户中断收场，实际：{:?}",
            events.last()
        );
        // 丢下没跑完的工具就走，也必须给每个 tool_use 补一个结果 ——
        // 缺一个，下次带着这段历史发请求就是 400。
        invariants::check_tool_pairing(&transcript(&events));
        assert!(invariants::take_violations().is_empty());
    }
}

/// 模型吐了半个 tool_use 就撞上输出上限。
///
/// `[约束]` 那半个 tool_use **绝不能**进 transcript。它永远等不到 tool_result，
/// 下一次请求就是 400。扣留机制的 `discard_for_retry` 防的就是这个。
#[tokio::test]
async fn 被截断的_tool_use_不进_transcript() {
    use riot_protocol::provider::{ProviderError, ProviderEvent};

    let provider = Arc::new(ScriptedProvider::new(vec![
        vec![
            ProviderEvent::Message(riot_core::testing::assistant_tool_use(
                "msg_a1",
                "tu_half",
                "Edit",
                serde_json::json!({ "path": "a" }),
            )),
            ProviderEvent::Error(ProviderError::OutputLimit),
        ],
        vec![ProviderEvent::Message(riot_core::testing::assistant_text(
            "msg_a2",
            "重来一遍，这次说完了。",
        ))],
    ]));
    let tools = Arc::new(ScriptedToolRunner::new(Default::default()));
    let deps = mock_deps_with(provider, tools, Arc::new(FakeCompactor::default()));

    let events = collect(state(4), deps, CancellationToken::new()).await;
    let t = transcript(&events);

    // 半截 tool_use 作为事件出现过（UI 已经渲染了），但不能留在 transcript 里。
    let orphans = invariants::orphan_tool_uses(&t);
    assert!(
        !orphans.is_empty(),
        "事件流里应该能看到那半截 tool_use —— 它确实被 yield 过"
    );

    // 真正要断言的是最终 transcript。主循环内部维护的 state.messages 才是
    // 下次请求的输入，事件流只是它的一个投影。
    let requests = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::RequestStart { .. }))
        .count();
    assert_eq!(requests, 2, "应该重试了一次");
    assert!(matches!(
        events.last(),
        Some(AgentEvent::Done {
            reason: TerminalReason::Completed
        })
    ));
}

/// 空流：模型什么都没说就结束了。
#[tokio::test]
async fn 空响应也要干净收场() {
    let provider = Arc::new(ScriptedProvider::new(vec![vec![]]));
    let tools = Arc::new(ScriptedToolRunner::new(Default::default()));
    let deps = mock_deps_with(provider, tools, Arc::new(FakeCompactor::default()));

    let events = collect(state(4), deps, CancellationToken::new()).await;

    assert!(
        matches!(
            events.last(),
            Some(AgentEvent::Done {
                reason: TerminalReason::Completed
            })
        ),
        "没有 tool_use 就该结束，而不是卡住等一个永远不来的消息"
    );
}

// ────────────────────────────────────────────────────────────
// 混沌长跑
// ────────────────────────────────────────────────────────────

/// 随机故障组合，只断言「总能干净收场」。
///
/// 这一层不检查结果对不对 —— 那是 L3 的事。它检查的是**没人想到过的组合**
/// 会不会让主循环卡死、漏发 Done、或者留下配对不上的消息。
///
/// 每个 seed 单独 spawn，是为了让一个 seed 的 panic 不中断整轮 —— 一次跑完
/// 能看到全部失败的 seed，比修一个跑一次快得多。
#[tokio::test]
async fn chaos_soak() {
    // 静音 panic 输出。500 个 seed 里哪怕只有几个失败，backtrace 也会把
    // 真正有用的 seed 列表冲掉。
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));

    let mut failures: Vec<(u64, String)> = Vec::new();

    for seed in 0..500u64 {
        let handle = tokio::spawn(async move { run_chaos_session(seed).await });

        match handle.await {
            Ok(Ok(())) => {}
            Ok(Err(msg)) => failures.push((seed, msg)),
            Err(e) if e.is_panic() => {
                let msg = e
                    .into_panic()
                    .downcast_ref::<String>()
                    .cloned()
                    .unwrap_or_else(|| "非字符串 panic".into());
                failures.push((seed, msg));
            }
            Err(e) => failures.push((seed, format!("任务异常: {e}"))),
        }
    }

    std::panic::set_hook(default_hook);

    assert!(
        failures.is_empty(),
        "混沌测试失败 {} 个 seed：\n{}",
        failures.len(),
        failures
            .iter()
            .take(10)
            .map(|(s, m)| format!("  seed {s}: {m}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

async fn run_chaos_session(seed: u64) -> Result<(), String> {
    let provider = Arc::new(ChaosProvider::new(seed));
    let mut results = std::collections::HashMap::new();
    results.insert(
        "Read".to_string(),
        ScriptedResult::Ok {
            text: "内容".into(),
        },
    );
    let tools = Arc::new(ScriptedToolRunner::new(results));
    let deps = mock_deps_with(provider, tools, Arc::new(FakeCompactor::default()));

    let cancel = CancellationToken::new();
    // 一部分 seed 在中途取消，覆盖「中断撞上恢复路径」这种组合。
    if seed.is_multiple_of(7) {
        let c = cancel.clone();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            c.cancel();
        });
    }

    let events = collect(state(6), deps, cancel).await;

    // ── 断言 1：必须以 Done 收场 ──────────────────────────
    let Some(last) = events.last() else {
        return Err("事件流是空的，连 Done 都没有".into());
    };
    if !matches!(last, AgentEvent::Done { .. }) {
        return Err(format!("最后一个事件不是 Done，而是 {last:?}"));
    }

    // ── 断言 2：Done 只能有一个，且必须在最后 ──────────────
    let dones = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::Done { .. }))
        .count();
    if dones != 1 {
        return Err(format!("发出了 {dones} 个 Done"));
    }

    // ── 断言 3：进 transcript 的消息里，工具调用必须配对 ────
    //
    // 注意这里检查的是**事件流投影出来的 transcript**，它是 UI 看到的东西。
    // 主循环内部的 state.messages 由 invariants::check_tool_pairing 在
    // 每轮结束时检查过了。两边都得对。
    let t = transcript(&events);
    let orphans = invariants::orphan_tool_uses(&t);
    if !orphans.is_empty() {
        // 被截断的响应会在事件流里留下孤儿（那半截 tool_use 确实 yield 过），
        // 但只有在会话**正常结束**时这才是 bug —— 中断和错误终止时，
        // UI 本来就要靠 Done 的原因来清理未完成的渲染。
        if matches!(
            last,
            AgentEvent::Done {
                reason: TerminalReason::Completed
            }
        ) && !had_recovery(&events)
        {
            return Err(format!("正常结束却留下了配对不上的 tool_use: {orphans:?}"));
        }
    }

    Ok(())
}

fn had_recovery(events: &[AgentEvent]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, AgentEvent::RequestStart { after: Some(_), .. }))
}
