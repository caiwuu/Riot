//! 主循环。
//!
//! # 签名里没有 Result
//!
//! `run_agent` 返回 `impl Stream<Item = AgentEvent>`，**不是**
//! `Stream<Item = Result<AgentEvent, E>>`。这不是风格问题，是用类型系统
//! 强制「错误是对话内容，不是异常」这条哲学。
//!
//! 因为返回类型里没有错误通道，`stream!` 块内部就不能用 `?` 往外抛 ——
//! 编译器会拒绝。实现者被迫在每个可能失败的地方显式决定：这个错误是转成
//! 消息给模型看（让它自我纠正），还是转成 `Done { Error }` 终止会话。
//!
//! 对照 TS 版本：那边靠约定保证错误不抛穿主循环，实际要靠 code review
//! 和运行时兜底。这里靠编译器。
//!
//! 见 ARCHITECTURE.md §5

use std::sync::Arc;

use async_stream::stream;
use futures::StreamExt;
use futures_core::Stream;
use riot_protocol::compact::{CompactBudget, CompactResult};
use riot_protocol::event::{AbortSource, AgentError, AgentEvent, TerminalReason};
use riot_protocol::id::ToolUseId;
use riot_protocol::message::{Message, MessageMeta, ToolResultContent, UserContent};
use riot_protocol::provider::{ProviderError, ProviderEvent, ProviderRequest, ThinkingConfig};
use tokio_util::sync::CancellationToken;

use crate::guard::guarded;
use crate::invariant;
use crate::invariants;
use crate::state::{AgentDeps, AgentState, BatchContext, BatchEvent, Transition};
use crate::turn::TurnAccumulator;

/// 输出上限恢复的最大次数。
///
/// 超过就说明不是「这次输出偏长」而是「任务本身要求的输出超出模型能力」，
/// 继续对半砍只会让模型输出越来越短的半成品。
const MAX_OUTPUT_LIMIT_RECOVERY: u8 = 2;

/// 压缩连续失败多少次熔断。
const MAX_COMPACT_FAILURES: u8 = 3;

pub fn run_agent(
    initial: AgentState,
    deps: AgentDeps,
    cancel: CancellationToken,
) -> impl Stream<Item = AgentEvent> + Send {
    guarded(stream! {
        let mut state = initial;

        loop {
            // ── 1. 中断检查 ──────────────────────────────────────
            if cancel.is_cancelled() {
                // 补齐所有悬空的 tool_result 再走。少一个，下次带着这段历史
                // 发请求就是 400。由 INV-1 断言。
                if let Some(msg) = synthesize_cancelled_results(&state) {
                    state.messages.push(msg.clone());
                    yield AgentEvent::Message(msg);
                }
                yield AgentEvent::Done {
                    reason: TerminalReason::Aborted { by: AbortSource::User },
                };
                return;
            }

            yield AgentEvent::RequestStart {
                turn: state.turn,
                model: state.model.clone(),
                after: state.transition,
            };

            // ── 2. 组装请求 ──────────────────────────────────────
            let messages = state.model_messages();
            invariants::check_api_payload(&messages);
            invariants::check_thinking_signatures(&messages, &state.model);

            let request = ProviderRequest {
                model: state.model.clone(),
                messages,
                system: state.system.clone(),
                tools: deps.tools.specs(),
                max_output_tokens: state.max_output_tokens_override,
                thinking: ThinkingConfig::Off,
            };

            // ── 3. 流式消费模型输出 ──────────────────────────────
            let mut turn = TurnAccumulator::new();
            let mut model_stream = deps.provider.stream(request, cancel.child_token());

            while let Some(item) = model_stream.next().await {
                match item {
                    ProviderEvent::Delta(d) => yield AgentEvent::Delta(d),
                    ProviderEvent::Usage(u) => turn.merge_usage(&u),
                    ProviderEvent::Message(m) => {
                        turn.push(m.clone());
                        yield AgentEvent::Message(m);
                    }
                    // 可恢复错误先扣下，不进事件流。UI 一看到错误就会
                    // 结束渲染，而此时恢复循环还在跑，没人在听结果。
                    ProviderEvent::Error(e) if e.is_recoverable() => turn.withhold(e),
                    ProviderEvent::Error(e) => {
                        let msg = error_message_for_user(&deps, &e);
                        state.messages.push(msg.clone());
                        yield AgentEvent::Message(msg);
                        yield AgentEvent::Done {
                            reason: TerminalReason::Error {
                                error: AgentError::Provider {
                                    message: e.to_string(),
                                    retryable: false,
                                },
                            },
                        };
                        return;
                    }
                }
            }
            drop(model_stream);

            // ── 4. 恢复路径 ──────────────────────────────────────
            if let Some(err) = turn.withheld().cloned() {
                let before = state.counters();
                let outcome = attempt_recovery(&mut state, &err);
                invariants::check_recovery_monotonic(before, state.counters());

                match outcome {
                    Recovery::Retry(transition) => {
                        // 被截断的响应里可能有半个 tool_use，不能进 transcript
                        turn.discard_for_retry();

                        // 决策是纯函数（attempt_recovery），副作用在这里执行。
                        // 分开是为了让「什么情况下该重试」能脱离 IO 单测 ——
                        // 那部分逻辑的护栏最多，也最容易改错。
                        if transition == Transition::ReactiveCompactRetry {
                            let current = deps.provider.count_tokens(&state.messages);
                            let budget = CompactBudget {
                                target_tokens: current / 2,
                                current_tokens: current,
                            };
                            match deps.compactor.compact(state.messages.clone(), budget).await {
                                CompactResult::Compacted {
                                    messages,
                                    before_tokens,
                                    after_tokens,
                                    strategy,
                                } => {
                                    // 压缩后必须仍然配对，否则重试的请求照样 400。
                                    invariants::check_tool_pairing(&messages);
                                    state.messages = messages;
                                    state.compact_failure_streak = 0;
                                    yield AgentEvent::Compacted { before_tokens, after_tokens, strategy };
                                }
                                CompactResult::Failed { reason } => {
                                    // 不在这里终止 —— 让下一轮的 attempt_recovery
                                    // 拿着累加后的 streak 决定要不要熔断。
                                    // 终止逻辑只有一处，不要散在两个地方。
                                    tracing::warn!(reason, "压缩失败");
                                    state.compact_failure_streak += 1;
                                }
                            }
                        }

                        state.transition = Some(transition);
                        continue;
                    }
                    Recovery::Surface(error) => {
                        let msg = error_message_for_user(&deps, &err);
                        state.messages.push(msg.clone());
                        yield AgentEvent::Message(msg);
                        yield AgentEvent::Done { reason: TerminalReason::Error { error } };
                        return;
                    }
                }
            }

            // ── 5. 退出判据：只看有没有 tool_use ──────────────────
            if !turn.has_tool_use() {
                state.messages.extend(turn.take_messages());
                yield AgentEvent::Done { reason: TerminalReason::Completed };
                return;
            }

            // ── 6. 工具执行 ──────────────────────────────────────
            let calls = turn.tool_calls();
            state.messages.extend(turn.take_messages());

            let batch_ctx = BatchContext {
                session_id: state.session_id.clone(),
                cancel: cancel.child_token(),
            };
            let mut batch = deps.tools.run_batch(calls, batch_ctx);
            let mut outcome = None;

            while let Some(ev) = batch.next().await {
                match ev {
                    BatchEvent::Progress { tool_use_id, payload } => {
                        yield AgentEvent::Progress { tool_use_id, payload };
                    }
                    BatchEvent::Done(o) => outcome = Some(o),
                }
            }
            drop(batch);

            match outcome {
                Some(o) => {
                    state.messages.push(o.results.clone());
                    yield AgentEvent::Message(o.results);
                    for m in o.side_messages {
                        state.messages.push(m.clone());
                        yield AgentEvent::Message(m);
                    }
                    if o.cancelled > 0 {
                        invariants::check_tool_pairing(&state.messages);
                        yield AgentEvent::Done {
                            reason: TerminalReason::AbortedTools { cancelled: o.cancelled },
                        };
                        return;
                    }
                }
                // ToolRunner 违约：流结束了却没给 Done。补齐结果再终止，
                // 否则悬空的 tool_use 会让下一次请求 400。
                None => {
                    invariant!(false, "ToolRunner 的流结束了但没有 BatchEvent::Done");
                    if let Some(msg) = synthesize_cancelled_results(&state) {
                        state.messages.push(msg.clone());
                        yield AgentEvent::Message(msg);
                    }
                    yield AgentEvent::Done {
                        reason: TerminalReason::Error {
                            error: AgentError::Internal {
                                message: "工具批次没有返回结果".into(),
                            },
                        },
                    };
                    return;
                }
            }

            invariants::check_tool_pairing(&state.messages);

            // ── 7. 收尾 ──────────────────────────────────────────
            state.advance_turn();

            if state.turn >= state.max_turns {
                yield AgentEvent::Done {
                    reason: TerminalReason::MaxTurns { limit: state.max_turns },
                };
                return;
            }
        }
    })
}

enum Recovery {
    Retry(Transition),
    Surface(AgentError),
}

/// 决定一个可恢复错误怎么处理。
///
/// `[约束]` 三条防死循环护栏都在这里，改动前读 ARCHITECTURE.md §5.4：
///
/// 1. `attempted_reactive_compact` 一旦置位就不再重置（stop-hook 重试路径也不行）
/// 2. 压缩连续失败 3 次熔断
/// 3. 输出上限恢复最多 2 次
///
/// 这个函数**不是** async，也不碰 IO —— 它是纯状态转移，可以直接单测。
/// 真正的压缩动作由调用方在 `Retry` 之后执行。
fn attempt_recovery(state: &mut AgentState, err: &ProviderError) -> Recovery {
    match err {
        ProviderError::OutputLimit => {
            if state.output_limit_recovery_count >= MAX_OUTPUT_LIMIT_RECOVERY {
                return Recovery::Surface(AgentError::Provider {
                    message: format!(
                        "输出 token 连续 {MAX_OUTPUT_LIMIT_RECOVERY} 次耗尽，任务需要的输出超出模型能力"
                    ),
                    retryable: false,
                });
            }
            state.output_limit_recovery_count += 1;
            // 对半砍。不设下限的话第三次会砍到几乎不能输出任何东西。
            let current = state.max_output_tokens_override.unwrap_or(8192);
            state.max_output_tokens_override = Some((current / 2).max(1024));
            Recovery::Retry(Transition::OutputLimitRecovery)
        }

        ProviderError::ContextOverflow { used, limit } => {
            // 熔断是**跨会话轮次**的：`compact_failure_streak` 不随 turn 重置，
            // 只在压缩真正成功时清零（见 AgentState::advance_turn）。
            //
            // 所以这条分支的触发路径是「用户发消息 → 溢出 → 压缩失败 → 终止」
            // 连续发生 3 次，而不是单次 run_agent 内部循环 3 圈 —— 单次调用里
            // `attempted_reactive_compact` 已经把压缩限制成最多一次了。
            // 判错这一点会写出一个永远不执行的分支。
            if state.compact_failure_streak >= MAX_COMPACT_FAILURES {
                return Recovery::Surface(AgentError::CompactCircuitOpen {
                    attempts: state.compact_failure_streak,
                });
            }
            if state.attempted_reactive_compact {
                // 压过一次还是溢出，说明压不动了。再压一次只是重复烧钱。
                return Recovery::Surface(AgentError::ContextExhausted {
                    used: *used,
                    limit: *limit,
                });
            }
            state.attempted_reactive_compact = true;
            Recovery::Retry(Transition::ReactiveCompactRetry)
        }

        ProviderError::MediaTooLarge { .. } => Recovery::Surface(AgentError::Provider {
            message: err.to_string(),
            retryable: false,
        }),

        // is_recoverable() 已经挡掉了其余变体，走到这里说明那两处判断不一致。
        other => {
            invariant!(false, "不可恢复的错误进了恢复路径：{other:?}");
            Recovery::Surface(AgentError::Provider {
                message: other.to_string(),
                retryable: false,
            })
        }
    }
}

/// 为所有悬空的 tool_use 合成「已取消」结果。
///
/// 中断时必须做这件事。Anthropic API 要求每个 tool_use 都有配对的
/// tool_result，缺一个就整条请求 400 —— 而且报错信息不会告诉你缺哪个。
fn synthesize_cancelled_results(state: &AgentState) -> Option<Message> {
    let orphans: Vec<ToolUseId> = invariants::orphan_tool_uses(&state.messages);
    if orphans.is_empty() {
        return None;
    }
    Some(Message::User {
        id: riot_protocol::id::MessageId::from_raw(format!(
            "{}_cancelled",
            state.session_id.as_str()
        )),
        content: orphans
            .into_iter()
            .map(|id| UserContent::ToolResult {
                tool_use_id: id,
                content: ToolResultContent::text("已取消"),
                is_error: true,
            })
            .collect(),
        meta: MessageMeta {
            synthetic: true,
            ..Default::default()
        },
    })
}

/// 把 provider 错误转成给用户看的系统消息。
///
/// `[约束]` 用 `System` 而不是 `Assistant`。System 消息不回送模型 ——
/// 让模型看到「你上次请求失败了」的元信息会让它开始为错误道歉，
/// 而不是继续干活。由 INV-7 断言。
fn error_message_for_user(deps: &AgentDeps, err: &ProviderError) -> Message {
    Message::System {
        id: deps.ids.message_id(),
        level: riot_protocol::message::SystemLevel::Error,
        text: err.to_string(),
    }
}

/// 让 `AgentDeps` 里的 Arc 用起来顺手一点。
impl AgentDeps {
    pub fn provider(&self) -> &Arc<dyn riot_protocol::provider::Provider> {
        &self.provider
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use riot_protocol::id::SessionId;

    fn state() -> AgentState {
        AgentState::new(SessionId::from_raw("s1"), "test-model")
    }

    #[test]
    fn 输出上限恢复两次后放弃() {
        let mut s = state();

        assert!(matches!(
            attempt_recovery(&mut s, &ProviderError::OutputLimit),
            Recovery::Retry(Transition::OutputLimitRecovery)
        ));
        assert_eq!(s.max_output_tokens_override, Some(4096));

        assert!(matches!(
            attempt_recovery(&mut s, &ProviderError::OutputLimit),
            Recovery::Retry(_)
        ));
        assert_eq!(s.max_output_tokens_override, Some(2048));

        assert!(
            matches!(
                attempt_recovery(&mut s, &ProviderError::OutputLimit),
                Recovery::Surface(_)
            ),
            "无限对半砍只会让模型输出越来越短的半成品"
        );
    }

    #[test]
    fn 输出上限有下限不会砍到不可用() {
        let mut s = state();
        s.max_output_tokens_override = Some(1024);
        attempt_recovery(&mut s, &ProviderError::OutputLimit);
        assert_eq!(
            s.max_output_tokens_override,
            Some(1024),
            "砍到 512 就没法输出了"
        );
    }

    #[test]
    fn 压缩只试一次() {
        let mut s = state();
        let err = ProviderError::ContextOverflow {
            used: 200_000,
            limit: 180_000,
        };

        assert!(matches!(
            attempt_recovery(&mut s, &err),
            Recovery::Retry(Transition::ReactiveCompactRetry)
        ));
        assert!(s.attempted_reactive_compact);

        assert!(
            matches!(
                attempt_recovery(&mut s, &err),
                Recovery::Surface(AgentError::ContextExhausted { .. })
            ),
            "压过一次还溢出说明压不动了，再压只是重复烧钱"
        );
    }

    #[test]
    fn 压缩连续失败会熔断() {
        let mut s = state();
        s.compact_failure_streak = MAX_COMPACT_FAILURES;

        assert!(matches!(
            attempt_recovery(
                &mut s,
                &ProviderError::ContextOverflow { used: 1, limit: 1 }
            ),
            Recovery::Surface(AgentError::CompactCircuitOpen { .. })
        ));
    }

    #[test]
    fn 恢复标志位只增不减() {
        // 这条对应 INV-5。历史上这个 bug 表现为无限压缩循环：
        // 某条重试路径把 attempted_reactive_compact 重置了。
        let mut s = state();
        attempt_recovery(&mut s, &ProviderError::OutputLimit);
        let after_first = s.counters();

        attempt_recovery(&mut s, &ProviderError::OutputLimit);
        invariants::check_recovery_monotonic(after_first, s.counters());
        assert!(invariants::take_violations().is_empty());
    }
}
