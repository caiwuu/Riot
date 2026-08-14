//! 子 agent：Task 工具。
//!
//! # 骨架（对照 Claude Code 的 AgentTool / runAgent）
//!
//! Task 就是**再跑一遍主循环**：独立的系统提示、独立的工具集、全新的
//! 上下文（一条 user 消息），跑完取最后一条 assistant 文本回给父。
//! 它不是旁路 —— 子 agent 的每个工具调用走和父完全相同的调度器和
//! 权限闸（同一个 HostGate，弹窗、规则、模式全部一致）。
//!
//! # 两个内置类型
//!
//! - `general-purpose`：全套文件/命令/联网工具，自主完成多步任务；
//! - `explore`：**只读**侦察（Read/Grep/Glob/WebFetch/WebSearch），
//!   给"到处找找"这类任务 —— 便宜、可并行、绝不改东西。
//!
//! # 递归与并发
//!
//! 子 agent 的注册表里**没有 Task 工具**，递归在结构上就不存在
//! （CC 的教训清单："子 agent 能再 spawn 子 agent → 无限递归"）。
//! 并发安全：多个 Task 可以同批并行 —— 这正是它的核心价值；写操作的
//! 风险由子 agent 内层的权限闸逐项把关，外壳不需要独占。
//!
//! # 结果与可观测性
//!
//! - 结果 = 最后一条有文本的 assistant 消息（CC 同款），附用量脚注；
//! - 过程以 Progress 事件流回父会话的工具卡片（工具调用逐行可见）；
//! - transcript 落在 `sessions/subagents/<会话>/<agent>.jsonl`，和主
//!   transcript 隔开 —— 放同一目录会被索引重建当成会话捞回来。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;

use riot_core::{AgentDeps, AgentState, run_agent};
use riot_protocol::event::{AgentEvent, OutputStream, ProgressPayload, TerminalReason};
use riot_protocol::id::{IdGenerator, SessionId};
use riot_protocol::message::{AssistantContent, Message, Usage};
use riot_protocol::permission::{DecisionReason, PermissionContext, PermissionGate, PermissionResult};
use riot_protocol::tool::{PromptContext, Tool, ToolContext, ToolOutcome, UiPayload};
use riot_runtime::{MemoryFileState, SystemFs, SystemProcessRunner};
use riot_tools::registry::Registry;
use riot_tools::scheduler::Scheduler;

/// 组装一个子 agent 轮次所需的一切。由 run_inner 从当轮快照。
pub struct SubagentDeps {
    pub provider: Arc<dyn riot_protocol::provider::Provider>,
    pub model: String,
    /// 和父共用同一个权限闸：弹窗、会话规则、模式全部一致。
    pub gate: Arc<dyn PermissionGate>,
    pub web: Arc<dyn riot_protocol::web::WebAccess>,
    pub vision: Arc<dyn riot_protocol::vision::VisionAccess>,
    pub clock: Arc<dyn riot_protocol::tool::Clock>,
    pub ids: Arc<dyn IdGenerator>,
    pub cwd: PathBuf,
    pub artifacts_dir: PathBuf,
    pub max_turns: u32,
    /// 子 agent transcript 的落盘处（`sessions/subagents/<会话>/`）。
    /// None = 父会话本身不持久化（测试）。
    pub transcripts: Option<Arc<riot_store::Transcripts>>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct Input {
    /// 三五个词的任务名，显示在父会话的进度里。
    description: String,
    /// 给子 agent 的完整任务描述。它看不到本对话的任何内容 ——
    /// 背景、目标、范围、已知线索都要写进来。
    prompt: String,
    /// `general-purpose`（默认，全工具）或 `explore`（只读侦察）。
    #[serde(default)]
    subagent_type: Option<String>,
}

pub struct TaskTool {
    deps: SubagentDeps,
}

impl TaskTool {
    pub fn new(deps: SubagentDeps) -> Self {
        Self { deps }
    }
}

fn kind_of(input: &serde_json::Value) -> &str {
    input
        .get("subagent_type")
        .and_then(|v| v.as_str())
        .unwrap_or("general-purpose")
}

/// 各类型的工具集。
///
/// `[约束]` 两个清单里都没有 Task —— 递归要在结构上不存在，不能靠
/// 提示词劝。也没有 TodoWrite（子 agent 的清单父会话看不见，白记）、
/// Browser*（浏览器是会话级独占资源，并发子 agent 抢一个面板会打架）。
fn tools_for(kind: &str) -> Vec<Arc<dyn Tool>> {
    use riot_tools::tools;
    match kind {
        "explore" => {
            let cache = Arc::new(tools::web::PageCache::default());
            vec![
                Arc::new(tools::Read),
                Arc::new(tools::Grep),
                Arc::new(tools::Glob),
                Arc::new(tools::WebSearch),
                Arc::new(tools::WebFetch::new(cache)),
            ]
        }
        _ => {
            let cache = Arc::new(tools::web::PageCache::default());
            vec![
                Arc::new(tools::Read),
                Arc::new(tools::Edit),
                Arc::new(tools::Write),
                Arc::new(tools::Bash),
                Arc::new(tools::Grep),
                Arc::new(tools::Glob),
                Arc::new(tools::WebSearch),
                Arc::new(tools::WebFetch::new(cache)),
            ]
        }
    }
}

fn system_prompt_for(kind: &str, cwd: &std::path::Path) -> String {
    let base = format!(
        "工作目录：{}\n平台：{}\n\n",
        cwd.display(),
        std::env::consts::OS
    );
    match kind {
        "explore" => format!(
            "你是只读侦察专家，任务是快速、准确地摸清情况并汇报。\n\n{base}\
             规则：\n\
             - 只读。不修改任何文件、不执行有副作用的操作。\n\
             - 并行地广撒网（Grep/Glob 可以同批多个），再对命中处精读。\n\
             - 汇报要可跳转：结论都带文件路径和行号。\n\
             - 你的回复会**原样**作为调查结果交回，写成一份紧凑的报告：\
               先结论，再证据，不要过程独白。\n\n回答用中文。",
        ),
        _ => format!(
            "你是自主完成任务的执行者。委托方给你一个任务，你独立做完并汇报。\n\n{base}\
             规则：\n\
             - 动手前先看清楚：改文件前 Read，找位置用 Grep。\n\
             - 只做任务描述里的事，不顺手扩展。\n\
             - 你的最后一条回复会**原样**作为任务结果交回 —— 写清楚做了什么、\
               改了哪些文件、验证结果如何；失败就如实说失败和原因。\n\n回答用中文。",
        ),
    }
}

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        "Task"
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        // CC AgentTool prompt 的精华：何时用/不该用/怎么写任务描述。
        "启动一个子 agent 自主完成任务。适合：需要多步探索的调研（不确定\
         东西在哪、要广撒网）、可以并行的独立子问题、一段可独立交付的实现。\
         不适合：读一个已知路径的文件（直接 Read）、找一个具体符号（直接 \
         Grep）—— 那些一步就完，包一层子 agent 只是变慢。\n\n\
         subagent_type 选 `explore`（只读侦察，便宜、可并行）或 \
         `general-purpose`（全工具，能改代码跑命令）。\n\n\
         写 prompt 时把它当成一个刚进门的同事：它**看不到**本对话的任何内容。\
         背景、目标、范围、已排除的方向、相关文件路径都要写进去；要求它汇报\
         什么形式的结果也写明。它的回复不会直接展示给用户 —— 你要自己转述\
         要点。可以在一条消息里并行发起多个 Task。"
            .into()
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let desc = input.get("description").and_then(|v| v.as_str()).unwrap_or("子任务");
        format!("子 agent（{}）：{desc}", kind_of(input))
    }

    /// explore 是只读的（按输入判定）；general-purpose 会写。
    fn is_read_only(&self, input: &serde_json::Value) -> bool {
        kind_of(input) == "explore"
    }

    /// 并行子 agent 是这个工具的核心价值。写操作的风险由子 agent 内层
    /// 的权限闸逐项把关，外壳不需要独占。
    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    /// 外壳放行：启动子任务本身没有副作用，副作用全在内层工具，而内层
    /// 每一个都过同一个权限闸。在这里多问一次，用户会在"允许启动子任务"
    /// 和"允许子任务里的写文件"上连答两遍 —— 第一遍没有信息量。
    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Preapproved {
                what: "子任务（内部操作仍逐项过权限）".into(),
            },
        }
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let parsed: Input = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutcome::failed(format!("参数不对：{e}")),
        };
        if parsed.prompt.trim().is_empty() {
            return ToolOutcome::failed(
                "prompt 是空的。子 agent 看不到本对话 —— 把背景、目标、范围写进去。",
            );
        }
        let kind = parsed
            .subagent_type
            .as_deref()
            .unwrap_or("general-purpose")
            .to_owned();
        if kind != "general-purpose" && kind != "explore" {
            return ToolOutcome::failed(format!(
                "没有叫「{kind}」的子 agent 类型。可用：general-purpose、explore。"
            ));
        }

        let agent_id = self.deps.ids.agent_id();
        ctx.progress.send(ProgressPayload::Status {
            text: format!("[{}] {} 启动", kind, parsed.description),
        });

        // ── 装配子 agent 的一轮 ────────────────────────────
        let tools = tools_for(&kind);
        let prompt_ctx = PromptContext {
            cwd: self.deps.cwd.clone(),
            platform: std::env::consts::OS.to_owned(),
            sibling_tools: tools.iter().map(|t| t.name().to_owned()).collect(),
            today: riot_tools::tools::web::date::year_month(self.deps.clock.now_ms()),
        };
        let registry = match Registry::new(tools) {
            Ok(r) => Arc::new(r),
            Err(e) => return ToolOutcome::failed(format!("子 agent 工具装配失败：{e}")),
        };
        let scheduler = Scheduler::new(
            registry,
            prompt_ctx,
            Arc::new(SystemFs::new()),
            Arc::new(SystemProcessRunner::default()),
            // 全新的先读后写缓存：子 agent 的"读过"和父互不作数 ——
            // 共享的话，父读过的文件子 agent 没看就能改。
            MemoryFileState::shared() as Arc<dyn riot_protocol::tool::FileStateCache>,
            Arc::clone(&self.deps.ids),
            Arc::clone(&self.deps.clock),
        )
        .with_web(Arc::clone(&self.deps.web))
        .with_vision(Arc::clone(&self.deps.vision))
        .with_gate(Arc::clone(&self.deps.gate))
        .with_artifacts_dir(self.deps.artifacts_dir.clone());

        let sub_session = SessionId::from_raw(agent_id.as_str().to_owned());
        let deps = AgentDeps {
            provider: Arc::clone(&self.deps.provider),
            compactor: Arc::new(riot_core::Layered::new(
                Arc::clone(&self.deps.provider),
                self.deps.model.clone(),
                Arc::clone(&self.deps.ids),
                ctx.cancel.child_token(),
            )),
            clock: Arc::clone(&self.deps.clock),
            ids: Arc::clone(&self.deps.ids),
            tools: Arc::new(scheduler),
            // 子 agent 没有"用户插话"一说 —— 插话进主 agent 的队列。
            queue: Arc::new(riot_core::state::NoQueue),
            // Stop hooks 只管主 agent 的产出。挂到子 agent 上，一次 Task
            // 会触发两层检查，反馈还会互相污染。
            stop_gate: Arc::new(riot_core::state::NoStopGate),
        };

        let user_msg = Message::User {
            id: riot_protocol::id::MessageId::from_raw(self.deps.ids.next_id("msg")),
            content: vec![riot_protocol::message::UserContent::Text {
                text: parsed.prompt.clone(),
            }],
            meta: Default::default(),
        };
        let state = AgentState::new(sub_session.clone(), self.deps.model.clone())
            .with_messages(vec![user_msg.clone()])
            .with_max_turns(self.deps.max_turns);
        let state = AgentState {
            system: system_prompt_for(&kind, &self.deps.cwd),
            ..state
        };

        // transcript：独立文件，和主会话隔开。
        let log = self.deps.transcripts.as_ref().map(|t| {
            let log = t.open(riot_store::TranscriptMeta {
                id: sub_session,
                root: self.deps.cwd.clone(),
                created_at_ms: self.deps.clock.now_ms(),
            });
            log.append(&user_msg);
            log
        });

        // ── 跑 ────────────────────────────────────────────
        let stream = run_agent(state, deps, ctx.cancel.child_token());
        futures::pin_mut!(stream);

        let mut collected: Vec<Message> = Vec::new();
        let mut usage = Usage::default();
        let mut tool_uses = 0u32;
        let mut terminal: Option<TerminalReason> = None;

        while let Some(ev) = stream.next().await {
            match ev {
                AgentEvent::Message(m) => {
                    if let Some(l) = &log {
                        l.append(&m);
                    }
                    if let Message::Assistant { content, usage: u, .. } = &m {
                        if let Some(u) = u {
                            usage.merge(u);
                        }
                        // 进度：让父会话的工具卡片看得到子 agent 在干什么。
                        for c in content {
                            match c {
                                AssistantContent::ToolUse { name, .. } => {
                                    tool_uses += 1;
                                    ctx.progress.send(ProgressPayload::Line {
                                        stream: OutputStream::Stdout,
                                        text: format!("→ {name}"),
                                    });
                                }
                                AssistantContent::Text { text } => {
                                    let first = text.lines().find(|l| !l.trim().is_empty());
                                    if let Some(f) = first {
                                        ctx.progress.send(ProgressPayload::Line {
                                            stream: OutputStream::Stdout,
                                            text: truncate_chars(f, 120),
                                        });
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    collected.push(m);
                }
                AgentEvent::Done { reason } => terminal = Some(reason),
                // Delta/Progress/权限事件不上转：权限弹窗由共享的 gate 直接
                // 发到父会话的事件流（同一个 sink），这里转发会出现两份。
                _ => {}
            }
        }

        if let Some(l) = &log {
            l.flush().await;
        }

        // ── 收尾 ──────────────────────────────────────────
        let footer = format!(
            "\n\n[子任务 {}：{} tokens · {} 次工具调用]",
            agent_id.as_str(),
            usage.input_tokens + usage.output_tokens,
            tool_uses,
        );
        match terminal {
            Some(TerminalReason::Completed) | Some(TerminalReason::MaxTurns { .. }) => {
                let text = last_assistant_text(&collected);
                match text {
                    Some(t) => {
                        let capped = if matches!(terminal, Some(TerminalReason::MaxTurns { .. })) {
                            format!("{t}\n\n[注意：子任务达到步数上限被停止，以上可能是未完成的结果]")
                        } else {
                            t
                        };
                        ToolOutcome::Ok {
                            ui_payload: Some(UiPayload::Plain {
                                text: format!("{} 完成{footer}", parsed.description),
                            }),
                            model_content: riot_protocol::message::ToolResultContent::text(
                                format!("{capped}{footer}"),
                            ),
                            side_messages: Vec::new(),
                        }
                    }
                    None => ToolOutcome::failed(
                        "子任务结束但没有产出任何文本结果。把任务描述写得更具体，或拆小再试。",
                    ),
                }
            }
            Some(TerminalReason::Aborted { .. }) | Some(TerminalReason::AbortedTools { .. }) => {
                ToolOutcome::Cancelled
            }
            Some(TerminalReason::Error { error }) => ToolOutcome::failed(format!(
                "子任务失败：{error:?}。可以调整任务描述重试一次；连续失败就自己动手做。"
            )),
            // 子 agent 没有 stop hooks；这个变体理论上到不了，但穷举比
            // 通配安全 —— 将来加了 hooks 这里会被编译器点名重审。
            Some(TerminalReason::StopHookPrevented { .. }) => ToolOutcome::failed(
                "子任务被 stop hook 拦下 —— 这不该发生在子 agent 上，请上报这个问题。",
            ),
            None => ToolOutcome::failed("子任务的事件流异常结束（没有终止事件）"),
        }
    }
}

fn last_assistant_text(messages: &[Message]) -> Option<String> {
    messages.iter().rev().find_map(|m| match m {
        Message::Assistant { content, .. } => {
            let text: String = content
                .iter()
                .filter_map(|c| match c {
                    AssistantContent::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            (!text.trim().is_empty()).then_some(text)
        }
        _ => None,
    })
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        s.chars().take(max).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use riot_core::testing::ScriptedProvider;
    use riot_protocol::id::{NanoIdGenerator, ToolUseId};
    use riot_protocol::provider::ProviderEvent;
    use riot_protocol::tool::ProgressSink;

    struct AllowAll;
    #[async_trait]
    impl PermissionGate for AllowAll {
        async fn check(
            &self,
            _tool: &dyn Tool,
            _input: &serde_json::Value,
            _id: &ToolUseId,
            _cancel: &tokio_util::sync::CancellationToken,
        ) -> riot_protocol::permission::GateOutcome {
            riot_protocol::permission::GateOutcome::Allow { updated_input: None }
        }
    }

    fn assistant(text: &str) -> Message {
        Message::Assistant {
            id: riot_protocol::id::MessageId::from_raw("a1"),
            content: vec![AssistantContent::Text { text: text.into() }],
            usage: Some(Usage {
                input_tokens: 100,
                output_tokens: 50,
                ..Default::default()
            }),
            meta: Default::default(),
        }
    }

    fn deps(provider: Arc<ScriptedProvider>) -> SubagentDeps {
        SubagentDeps {
            provider,
            model: "test-model".into(),
            gate: Arc::new(AllowAll),
            web: Arc::new(riot_protocol::web::NoWeb),
            vision: Arc::new(riot_protocol::vision::NoVision),
            clock: Arc::new(riot_providers::watchdog::TokioClock),
            ids: Arc::new(NanoIdGenerator),
            cwd: "/tmp".into(),
            artifacts_dir: std::env::temp_dir(),
            max_turns: 8,
            transcripts: None,
        }
    }

    fn ctx() -> (ToolContext, tokio::sync::mpsc::UnboundedReceiver<(ToolUseId, ProgressPayload)>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let id = ToolUseId::from_raw("t1");
        (
            ToolContext {
                session_id: SessionId::from_raw("s1"),
                tool_use_id: id.clone(),
                cwd: "/tmp".into(),
                artifacts_dir: std::env::temp_dir(),
                cancel: tokio_util::sync::CancellationToken::new(),
                progress: ProgressSink::new(id, tx),
                file_state: MemoryFileState::shared(),
                fs: Arc::new(SystemFs::new()),
                proc: Arc::new(SystemProcessRunner::default()),
                web: Arc::new(riot_protocol::web::NoWeb),
                browser: Arc::new(riot_protocol::browser::NoBrowser),
                vision: Arc::new(riot_protocol::vision::NoVision),
                clock: Arc::new(riot_providers::watchdog::TokioClock),
            },
            rx,
        )
    }

    #[tokio::test]
    async fn 子任务的最后一条文本回给父_带用量脚注() {
        let provider = Arc::new(ScriptedProvider::new(vec![vec![ProviderEvent::Message(
            assistant("调查完成：入口在 src/main.rs:42"),
        )]]));
        let tool = TaskTool::new(deps(Arc::clone(&provider)));
        let (c, _rx) = ctx();

        let out = tool
            .call(
                serde_json::json!({
                    "description": "找入口",
                    "prompt": "在这个仓库里找程序入口",
                    "subagent_type": "explore"
                }),
                c,
            )
            .await;

        let ToolOutcome::Ok { model_content, .. } = out else {
            panic!("该成功：{out:?}");
        };
        let text = format!("{model_content:?}");
        assert!(text.contains("src/main.rs:42"), "子 agent 的报告要原样回来");
        assert!(text.contains("150 tokens"), "用量脚注要在：{text}");

        // 子 agent 的请求不该带 Task 工具（递归在结构上不存在）
        let reqs = provider.requests();
        assert!(
            reqs[0].tools.iter().all(|t| t.name != "Task"),
            "子 agent 的工具清单里不能有 Task"
        );
        assert!(
            reqs[0].tools.iter().any(|t| t.name == "Read"),
            "explore 有只读工具"
        );
        assert!(
            reqs[0].tools.iter().all(|t| t.name != "Write" && t.name != "Bash"),
            "explore 不能有写工具"
        );
        assert!(
            reqs[0].system.contains("只读"),
            "explore 的系统提示要立只读规矩"
        );
    }

    #[tokio::test]
    async fn 未知类型报可用清单() {
        let provider = Arc::new(ScriptedProvider::new(vec![]));
        let tool = TaskTool::new(deps(provider));
        let (c, _rx) = ctx();
        let out = tool
            .call(
                serde_json::json!({ "description": "x", "prompt": "y", "subagent_type": "ninja" }),
                c,
            )
            .await;
        let ToolOutcome::Failed { error_for_model, .. } = out else {
            panic!("该失败")
        };
        assert!(error_for_model.contains("general-purpose"), "{error_for_model}");
    }

    #[test]
    fn explore_按输入判定为只读() {
        let provider = Arc::new(ScriptedProvider::new(vec![]));
        let tool = TaskTool::new(deps(provider));
        assert!(tool.is_read_only(&serde_json::json!({ "subagent_type": "explore" })));
        assert!(!tool.is_read_only(&serde_json::json!({ "subagent_type": "general-purpose" })));
        assert!(!tool.is_read_only(&serde_json::json!({})), "缺省是 general-purpose，会写");
    }
}
