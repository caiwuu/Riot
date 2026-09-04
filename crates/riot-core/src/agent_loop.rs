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
use riot_protocol::provider::{ProviderError, ProviderEvent, ProviderRequest};
use tokio_util::sync::CancellationToken;

use crate::guard::guarded;
use crate::invariant;
use crate::invariants;
use crate::state::{AgentDeps, AgentState, BatchContext, BatchEvent, StopDecision, Transition};
use crate::turn::TurnAccumulator;

/// 输出上限恢复的最大次数。
///
/// 超过就说明不是「这次输出偏长」而是「任务本身要求的输出超出模型能力」，
/// 继续对半砍只会让模型输出越来越短的半成品。
const MAX_OUTPUT_LIMIT_RECOVERY: u8 = 2;

/// 用户取消之后，等下游自己收场的宽限期。
///
/// 礼貌的下游（真实的 provider、调度器）在几毫秒内就结束了，走它们
/// 那条路收尾信息更完整。超过这个时间还没动静，就说明它没在听 ——
/// 主循环自己收场，别让用户对着一个按了没反应的停止键。
///
/// 2 秒是照着最慢的正当路径定的：杀进程组要走 SIGTERM → 500ms 宽限
/// → SIGKILL，再加上读干管道。
const ABORT_GRACE_MS: u64 = 2_000;

/// 取消发生**之后**再等 `grace_ms`。没取消就永远不醒。
///
/// `[约束]` 这里刻意用真实时钟，是这条 crate 规矩（一切等待走注入的
/// Clock）唯一的例外，理由是它测量的东西本身就是墙上时钟：
/// "用户按了停止之后，现实世界过去了多久还没反应"。换成注入的 Clock，
/// 一个立即返回的 mock 会让这个兜底在每次取消时都抢跑，把调度器更
/// 完整的收尾路径挤掉（黄金回放抓到过这个退化）。
///
/// 这不会让回放变得不确定：mock 下游在微秒级就收场了，2 秒的余量不是
/// 任何真实机器会踩到的窗口。
#[allow(clippy::disallowed_methods)]
async fn grace_after_cancel(cancel: &CancellationToken, grace_ms: u64) {
    cancel.cancelled().await;
    tokio::time::sleep(std::time::Duration::from_millis(grace_ms)).await;
}

/// stop hook 在一次 run 内最多阻止收尾几次。
///
/// 超过就说明 hook 的判据永远满足不了（或者脚本本身坏了），继续给机会
/// 只是无限烧 API。熔断时以 `StopHookPrevented` 终止，理由带给用户。
const MAX_STOP_HOOK_BLOCKS: u32 = 5;

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
            // 队列 drain 时机错了（用户插话被夹进 tool_use 和 tool_result
            // 之间）会在这里现形，而不是等服务方 400。
            invariants::check_message_sequence(&messages);

            let request = ProviderRequest {
                model: state.model.clone(),
                messages,
                system: state.system.clone(),
                tools: deps.tools.specs(),
                max_output_tokens: state.max_output_tokens_override,
                // 按请求序号解析：Adaptive 在首请求和工具续轮给不同档。
                thinking: state.thinking.config_for(state.turn),
            };

            // ── 3. 流式消费模型输出 ──────────────────────────────
            let mut turn = TurnAccumulator::new();
            let mut model_stream = deps.provider.stream(request, cancel.child_token());

            // `[约束]` 取消之后给下游一个宽限期，超了就自己收场 ——
            // 不能只靠 provider 自己停。
            //
            // Provider 拿到的是子令牌、也确实会检查它，但那是**约定**：
            // 一个没检查的实现（或一次"等首字节等了半分钟"的慢请求）
            // 就会让停止键变成装饰品 —— 用户按了，界面照转，而没有任何
            // 报错能解释这件事。停止是用户对系统最基本的控制权，必须由
            // 主循环兜底。
            //
            // 留宽限期而不是当场掐断：礼貌的下游会在几毫秒内自己结束流，
            // 那条路产出的收尾信息更完整（谁被取消了、取消了几个）。
            let abort_deadline = grace_after_cancel(&cancel, ABORT_GRACE_MS);
            futures::pin_mut!(abort_deadline);

            let mut stream_abandoned = false;
            'stream: loop {
                let item = tokio::select! {
                    item = model_stream.next() => match item {
                        Some(item) => item,
                        None => break 'stream,
                    },
                    _ = &mut abort_deadline => {
                        stream_abandoned = true;
                        break 'stream;
                    }
                };
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
                        // 流中途断开时，provider 会先把解码器里攒了一半的
                        // assistant 消息吐出来（可能带完整的 tool_calls）
                        // 再报这个错，而宿主对着事件流逐条持久化 —— 那条
                        // 消息已经进了它的历史。这里必须把它 commit 进
                        // 状态并补齐悬空的 tool_use：不补的话宿主历史从此
                        // 带着孤儿 tool_calls，下一轮请求在严格校验的
                        // 服务端（DeepSeek 等）上必然 400，重试也救不回。
                        state.messages.extend(turn.take_messages());
                        if let Some(msg) = synthesize_orphan_results(
                            &state,
                            "interrupted",
                            "Result lost: the request was interrupted and this call never ran.",
                        ) {
                            state.messages.push(msg.clone());
                            yield AgentEvent::Message(msg);
                        }
                        invariants::check_tool_pairing(&state.messages);
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

            // 只有**流被放弃**才在这里收场。
            //
            // 取消了但流自己正常结束时不能走这条 —— 让它照常往下走：
            // 那边的调度器会给每个工具补一条"已取消"结果并报出取消
            // 个数，收尾信息比这里合成的完整。抢在它前面，等于用一条
            // 更粗糙的路径替掉一条更细的。
            if stream_abandoned {
                state.messages.extend(turn.take_messages());
                if let Some(msg) = synthesize_cancelled_results(&state) {
                    state.messages.push(msg.clone());
                    yield AgentEvent::Message(msg);
                }
                invariants::check_tool_pairing(&state.messages);
                yield AgentEvent::Done {
                    reason: TerminalReason::Aborted { by: AbortSource::User },
                };
                return;
            }

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
                            // 说一声再压。阶梯压缩器的重档要真调一次模型，
                            // 而这条路上用户刚发出一句话、什么都还没看到 ——
                            // 不说的话那几十秒和"模型不理人"没有区别。
                            yield AgentEvent::Compacting;
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
                                    // 用户按停止会掐断总结请求，那不是"压不动"——
                                    // 计进 streak 的话，停止几次之后下一次真溢出
                                    // 直接熔断。
                                    if !cancel.is_cancelled() {
                                        state.compact_failure_streak += 1;
                                    }
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

                // 收尾闸（stop hooks）：产出检查脚本说"活没干完"就不让停，
                // 反馈注入对话强制再跑一轮。排在队列 drain **之前** ——
                // 活没干完就不该开始处理插话。
                //
                // INV-6：API 错误上跑 stop hooks 是"error → hook 注入 →
                // 重试 → 又 error"死循环的入口，错误路径必须在到达这里
                // 之前就 return。
                invariants::check_hook_eligibility(state.messages.last(), true);
                match deps.stop_gate.check(state.stop_hook_blocks).await {
                    StopDecision::Allow => {}
                    StopDecision::Block { reason } => {
                        state.stop_hook_blocks += 1;
                        // 硬熔断：hook 拿到过这么多次机会还在拦，多半是它的
                        // 判据永远满足不了（或脚本本身坏了）。带着理由终止，
                        // 而不是无限烧 API。
                        if state.stop_hook_blocks > MAX_STOP_HOOK_BLOCKS {
                            yield AgentEvent::Done {
                                reason: TerminalReason::StopHookPrevented { message: reason },
                            };
                            return;
                        }
                        // 两条消息两个读者：System 给用户解释为什么没停
                        // （不进模型），SystemReminder 给模型布置整改。
                        let notice = Message::System {
                            id: riot_protocol::id::MessageId::from_raw(deps.ids.next_id("msg")),
                            level: riot_protocol::message::SystemLevel::Info,
                            text: format!("Stop hook 要求继续：{reason}"),
                        };
                        state.messages.push(notice.clone());
                        yield AgentEvent::Message(notice);
                        let feedback = Message::User {
                            id: riot_protocol::id::MessageId::from_raw(deps.ids.next_id("msg")),
                            content: vec![UserContent::Attachment(
                                riot_protocol::message::Attachment::SystemReminder {
                                    text: format!(
                                        "A stop hook check did not pass: {reason}\n\
                                         Deal with the problem above before you wrap up. This is \
                                         feedback from an automated check, not a user message, \
                                         but treat it with the same authority — the user \
                                         configured the check."
                                    ),
                                },
                            )],
                            meta: MessageMeta { synthetic: true, ..Default::default() },
                        };
                        state.messages.push(feedback.clone());
                        yield AgentEvent::Message(feedback);

                        // `[约束]` 这里**不走** advance_turn：它会重置
                        // attempted_reactive_compact，而 stop-hook 重试路径
                        // 必须保留它 —— 否则 hook 注入 → 溢出 → 压缩 →
                        // hook 又注入 → 又溢出，压缩循环永不熔断（CC 的
                        // 注释里记录过这个 bug）。轮数照常推进，受
                        // max_turns 兜底。
                        state.turn += 1;
                        state.transition = Some(Transition::StopHookBlocking);
                        if state.turn >= state.max_turns {
                            yield AgentEvent::Done {
                                reason: TerminalReason::MaxTurns { limit: state.max_turns },
                            };
                            return;
                        }
                        continue;
                    }
                }

                // 收尾前再看队列：模型答完了、用户在它工作时又说了话，
                // 直接开下一轮（普通 user 消息，不加"插话"包装 —— 对模型
                // 来说这就是新的一轮对话）。
                let queued = deps.queue.drain();
                if queued.is_empty() {
                    yield AgentEvent::Done { reason: TerminalReason::Completed };
                    return;
                }
                for msg in queued {
                    state.messages.push(msg.clone());
                    yield AgentEvent::Message(msg);
                }
                state.advance_turn();
                if state.turn >= state.max_turns {
                    yield AgentEvent::Done {
                        reason: TerminalReason::MaxTurns { limit: state.max_turns },
                    };
                    return;
                }
                continue;
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
            let mut abandoned = false;

            // 同样是"宽限期 + 兜底"（理由见上面消费模型流那段）。正常
            // 情况下调度器自己会在取消后很快收场，而且它给出的结果更
            // 完整：每个工具一条"已取消"的 tool_result，还带取消计数。
            let abort_deadline = grace_after_cancel(&cancel, ABORT_GRACE_MS);
            futures::pin_mut!(abort_deadline);

            loop {
                let ev = tokio::select! {
                    ev = batch.next() => match ev {
                        Some(ev) => ev,
                        None => break,
                    },
                    _ = &mut abort_deadline => {
                        abandoned = true;
                        break;
                    }
                };
                match ev {
                    BatchEvent::Progress { tool_use_id, payload } => {
                        yield AgentEvent::Progress { tool_use_id, payload };
                    }
                    BatchEvent::Done(o) => outcome = Some(o),
                }
            }
            // drop 掉批次 = 丢弃还在跑的工具 future。子进程由
            // kill_on_drop / 进程组清理兜底（见 riot-runtime 的 proc）。
            drop(batch);

            if abandoned {
                // 结果没收齐，但**每个 tool_use 都必须有 tool_result**，
                // 否则带着这段历史再请求就是 400。
                if let Some(msg) = synthesize_cancelled_results(&state) {
                    state.messages.push(msg.clone());
                    yield AgentEvent::Message(msg);
                }
                invariants::check_tool_pairing(&state.messages);
                yield AgentEvent::Done {
                    reason: TerminalReason::Aborted { by: AbortSource::User },
                };
                return;
            }

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

            // 刻意**不在这里** drain 用户插话。工具结果就位后插入虽然对
            // API 是安全的（CC 就这么做），但对用户是惊吓：排队面板里的
            // 消息突然在任务中途蹦进对话，模型分心去答它。这里的语义是
            // Cursor 式的 —— 排队的消息等当前任务**完全跑完**（见第 5 步
            // 的收尾 drain），要插队由用户在面板上点"立即发送"（中断）。
            //
            // 带外消息是另一回事，它们**必须**在这里注入：「转到后台」
            // 和后台子 agent 的完成通知说的都是"你现在手上这件事"，等整
            // 轮跑完再给模型看等于按钮没有生效 —— 用户点完看着它继续干
            // 到底，几分钟后才冒出一个子 agent 去做已经做完的活。
            //
            // 这个位置就是那个安全点：tool_result 已经成对进历史，插一条
            // user 消息不会夹在 tool_use 和 tool_result 之间（INV-2）。
            for msg in deps.queue.drain_out_of_band() {
                state.messages.push(msg.clone());
                yield AgentEvent::Message(msg);
            }

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
    synthesize_orphan_results(state, "cancelled", "Cancelled by the user.")
}

/// 同上，但措辞和 id 后缀由调用方定 —— 致命错误路径的中断不是用户
/// 取消，对模型说「已取消」会让它以为是用户的意思。
fn synthesize_orphan_results(state: &AgentState, id_suffix: &str, text: &str) -> Option<Message> {
    let orphans: Vec<ToolUseId> = invariants::orphan_tool_uses(&state.messages);
    if orphans.is_empty() {
        return None;
    }
    Some(Message::User {
        id: riot_protocol::id::MessageId::from_raw(format!(
            "{}_{id_suffix}",
            state.session_id.as_str()
        )),
        content: orphans
            .into_iter()
            .map(|id| UserContent::ToolResult {
                tool_use_id: id,
                content: ToolResultContent::text(text),
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
