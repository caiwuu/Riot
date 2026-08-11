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
use std::time::Duration;

use tauri::ipc::Channel;
use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;

use riot_core::{AgentDeps, AgentState, ClearOldResults, run_agent};
use riot_permissions::RuleSet;
use riot_protocol::event::AgentEvent;
use riot_protocol::id::{IdGenerator, MessageId, NanoIdGenerator, RequestId, SessionId};
use riot_protocol::message::{Message, MessageMeta, UserContent};
use riot_protocol::permission::{
    AskPreview, DecisionReason, GateOutcome, PermissionAsk, PermissionContext, PermissionGate,
    PermissionMode, PermissionModeState, PermissionResponse, PermissionResult, PermissionRule,
};
use riot_protocol::provider::Provider;
use riot_protocol::tool::{PromptContext, Tool};
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

pub struct Session {
    pub id: SessionId,
    pub cwd: std::path::PathBuf,
    history: Mutex<Vec<Message>>,
    /// 当前这一轮的取消令牌。没有正在跑的轮次时是 None。
    running: Mutex<Option<CancellationToken>>,
    pending_asks: Arc<PendingAsks>,
    /// 会话级采样覆盖。字段为 None 表示继承 provider 的设置。
    /// 模型本身不存这里 —— 每轮由宿主按当前激活配置解析传入，
    /// 用户在对话中途切换模型，下一轮立即生效。
    sampling_override: Mutex<Sampling>,
    /// 会话内累积的权限规则（用户点了"总是允许"）。
    ///
    /// `Arc` 是刻意的：HostGate 持有同一份，规则在**同一轮内**立即生效。
    /// 拿快照的话，用户点了"总是允许 npm run *"，十秒后模型跑
    /// `npm run build` 还会弹窗 —— 用户会认为按钮坏了。
    rules: Arc<Mutex<Vec<PermissionRule>>>,
    mode: Mutex<PermissionMode>,
    /// 用户手动改过的标题。None 时回退到第一条消息。
    custom_title: Mutex<Option<String>>,
    file_state: Arc<MemoryFileState>,
    ids: Arc<NanoIdGenerator>,
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

impl Session {
    pub fn new(id: SessionId, cwd: std::path::PathBuf) -> Self {
        Self {
            id,
            cwd,
            history: Mutex::new(Vec::new()),
            running: Mutex::new(None),
            pending_asks: Arc::new(PendingAsks::default()),
            sampling_override: Mutex::new(Sampling::default()),
            rules: Arc::new(Mutex::new(Vec::new())),
            mode: Mutex::new(PermissionMode::Default),
            custom_title: Mutex::new(None),
            file_state: MemoryFileState::shared(),
            ids: Arc::new(NanoIdGenerator),
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

    pub async fn set_sampling(&self, s: Sampling) {
        *self.sampling_override.lock().await = s;
    }

    pub async fn sampling(&self) -> Sampling {
        *self.sampling_override.lock().await
    }

    pub async fn interrupt(&self) {
        if let Some(t) = self.running.lock().await.as_ref() {
            t.cancel();
        }
    }

    pub async fn history_len(&self) -> usize {
        self.history.lock().await.len()
    }

    /// 历史快照。切回一个会话时前端用它重建对话流。
    pub async fn history(&self) -> Vec<Message> {
        self.history.lock().await.clone()
    }

    /// 手动设置标题。None 或空串表示清除，回退到自动标题。
    pub async fn set_title(&self, title: Option<String>) {
        *self.custom_title.lock().await = title.filter(|t| !t.trim().is_empty());
    }

    /// 会话标题：手动改过的优先，否则取第一条用户消息的开头。
    /// 都没有就是 None（还没说过话）。
    pub async fn title(&self) -> Option<String> {
        if let Some(t) = self.custom_title.lock().await.clone() {
            return Some(t);
        }
        let h = self.history.lock().await;
        h.iter().find_map(|m| match m {
            Message::User { content, .. } => content.iter().find_map(|c| match c {
                riot_protocol::message::UserContent::Text { text } => {
                    let t = text.trim();
                    (!t.is_empty()).then(|| t.chars().take(40).collect())
                }
                _ => None,
            }),
            _ => None,
        })
    }

    /// 跑一轮。事件边产生边推给 `sink`，返回时这一轮已经结束。
    ///
    /// `model` 是宿主对"此刻激活配置"的解析结果（含会话覆盖合并后的
    /// 采样参数）。每轮传入而不是创建时锁死 —— 换模型下一轮就生效。
    pub async fn run_turn(
        &self,
        text: String,
        model: ResolvedModel,
        web: Arc<dyn riot_protocol::web::WebAccess>,
        sink: Channel<AgentEvent>,
        ask_timeout_secs: u32,
    ) -> Result<(), String> {
        let cancel = CancellationToken::new();
        {
            let mut g = self.running.lock().await;
            if g.is_some() {
                return Err("上一轮还在进行中".into());
            }
            *g = Some(cancel.clone());
        }

        let result = self
            .run_inner(text, model, web, sink, cancel, ask_timeout_secs)
            .await;
        *self.running.lock().await = None;
        result
    }

    /// 装配这一轮的工具调度器。
    ///
    /// 单独提出来是为了能被测到。`with_*` 系列每漏一个都是静默降级 ——
    /// 漏 `with_gate` 是所有操作不再询问，漏 `with_web` 是联网工具一律
    /// 报"未配置"。两者都编译得过，都要跑起来才发现。
    fn build_scheduler(
        &self,
        registry: Arc<Registry>,
        prompt_ctx: PromptContext,
        clock: Arc<dyn riot_protocol::tool::Clock>,
        web: Arc<dyn riot_protocol::web::WebAccess>,
        gate: Arc<dyn PermissionGate>,
    ) -> Scheduler {
        Scheduler::new(
            registry,
            prompt_ctx,
            Arc::new(SystemFs::new()),
            Arc::new(SystemProcessRunner::default()),
            Arc::clone(&self.file_state) as Arc<dyn riot_protocol::tool::FileStateCache>,
            Arc::clone(&self.ids) as Arc<dyn IdGenerator>,
            clock,
        )
        .with_web(web)
        .with_gate(gate)
    }

    async fn run_inner(
        &self,
        text: String,
        model: ResolvedModel,
        web: Arc<dyn riot_protocol::web::WebAccess>,
        sink: Channel<AgentEvent>,
        cancel: CancellationToken,
        ask_timeout_secs: u32,
    ) -> Result<(), String> {
        let provider = provider_for(&model)?;
        let clock: Arc<dyn riot_protocol::tool::Clock> =
            Arc::new(riot_providers::watchdog::TokioClock);

        let prompt_ctx = PromptContext {
            cwd: self.cwd.clone(),
            platform: std::env::consts::OS.to_owned(),
            sibling_tools: Vec::new(),
            // 模型对"今天"没有概念，它的年份停在训练截止那天。不注入的
            // 话它搜"最新版本"会带上一个两年前的年份，然后拿着过期结果
            // 言之凿凿。见 tools::web::date。
            today: riot_tools::tools::web::date::year_month(clock.now_ms()),
        };

        // 注册失败说明有重名或别名冲突 —— 那是代码错误，不是运行时状况。
        // 用 expect 让它在开发时就炸，而不是变成"某个工具神秘消失"。
        let registry = Arc::new(
            Registry::new(riot_tools::tools::builtin()).expect("内置工具注册表有冲突"),
        );

        let gate = Arc::new(HostGate {
            sink: sink.clone(),
            pending: Arc::clone(&self.pending_asks),
            ids: Arc::clone(&self.ids) as Arc<dyn IdGenerator>,
            ctx: PermissionContext {
                mode: PermissionModeState(Some(*self.mode.lock().await)),
                rules: self.rules.lock().await.clone(),
                sandboxed: false,
                can_prompt_user: true,
            },
            rules_live: Arc::clone(&self.rules),
            cwd: self.cwd.clone(),
            // 再夹一次。配置加载时已经夹过，但这里是唯一真正用到它的
            // 地方 —— 上游多一条没走 normalize 的路，这里就是最后一道。
            ask_timeout: Duration::from_secs(
                u64::from(ask_timeout_secs)
                    .clamp(*ASK_TIMEOUT_RANGE.start(), *ASK_TIMEOUT_RANGE.end()),
            ),
        });

        let scheduler = self.build_scheduler(registry, prompt_ctx, clock.clone(), web, gate);

        let deps = AgentDeps {
            provider,
            compactor: Arc::new(ClearOldResults::new()),
            clock: Arc::clone(&clock),
            ids: Arc::clone(&self.ids) as Arc<dyn IdGenerator>,
            tools: Arc::new(scheduler),
        };

        let mut history = self.history.lock().await.clone();
        history.push(Message::User {
            id: MessageId::from_raw(self.ids.next_id("msg")),
            content: vec![UserContent::Text { text }],
            meta: MessageMeta::default(),
        });

        let state = AgentState::new(self.id.clone(), model.model.clone())
            .with_messages(history)
            .with_max_turns(48);

        let system = system_prompt(&self.cwd);
        let state = AgentState {
            system,
            max_output_tokens_override: model.sampling.max_output_tokens,
            ..state
        };

        let mut collected: Vec<Message> = state.messages.clone();

        let stream = run_agent(state, deps, cancel.clone());
        futures::pin_mut!(stream);

        use futures::StreamExt;
        while let Some(ev) = stream.next().await {
            if let AgentEvent::Message(m) = &ev {
                collected.push(m.clone());
            }
            // 发送失败说明前端窗口没了。继续跑完只会白烧 API 额度。
            if sink.send(ev).is_err() {
                tracing::warn!("事件通道已断开，中止本轮");
                cancel.cancel();
                break;
            }
        }

        *self.history.lock().await = collected;
        Ok(())
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
    let url = format!("{}/v1/models", p.base_url.trim_end_matches('/'));

    let client = reqwest::Client::new();
    let req = match p.protocol {
        crate::config::Protocol::Openai => client.get(&url).bearer_auth(key),
        crate::config::Protocol::Anthropic => client
            .get(&url)
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
    };

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
    let mut ids: Vec<String> = list.data.into_iter().map(|m| m.id).collect();
    ids.sort();
    Ok(ids)
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
    sink: Channel<AgentEvent>,
    pending: Arc<PendingAsks>,
    ids: Arc<dyn IdGenerator>,
    ctx: PermissionContext,
    /// 和 Session.rules 是同一份。"总是允许"写进这里，同一轮内的
    /// 下一次调用立即生效。
    rules_live: Arc<Mutex<Vec<PermissionRule>>>,
    cwd: std::path::PathBuf,
    /// 等用户回应的上限，来自配置。见 [`ASK_TIMEOUT_RANGE`]。
    ask_timeout: Duration,
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
        // 每次都从共享状态取最新规则，不用构建时的快照 —— 快照意味着
        // "总是允许"要到下一轮才生效。
        let rules = RuleSet::new(self.rules_live.lock().await.clone());

        match riot_permissions::decide(tool, input, &self.ctx, &rules) {
            PermissionResult::Allow { updated_input, .. } => GateOutcome::Allow { updated_input },

            PermissionResult::Deny { message, .. } => GateOutcome::Deny { message },

            // Passthrough 到这里说明决策链没能定性。收敛成询问，不是放行 ——
            // 「不知道该不该」和「可以」是两回事。
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
                let spec = AskSpec {
                    message,
                    suggestions,
                    reason,
                };
                self.ask(tool, input, tool_use_id, cancel, spec).await
            }
        }
    }
}

/// 落实"总是允许"：把 AddRule 建议转成会话级规则。只处理加规则；
/// 改模式、扩围栏牵动的状态面更大，明确不支持好过半支持。
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

        // 这里等的是**用户**，用真实时钟而不是注入的 Clock。黄金回放里
        // 走不到这条路径（那些用例不弹窗），注入只会多一层没人用的间接。
        let answer = tokio::select! {
            r = tokio::time::timeout(self.ask_timeout, rx) => r,
            _ = cancel.cancelled() => {
                self.pending.forget(&request_id).await;
                self.resolved(&request_id, DecisionReason::UserChoice { remembered: false });
                return GateOutcome::Deny { message: "用户已中断，本次操作未执行".into() };
            }
        };

        match answer {
            Ok(Ok(PermissionResponse::Allow { remember })) => {
                self.remember(remember).await;
                GateOutcome::Allow {
                    updated_input: None,
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
                        self.ask_timeout.as_secs()
                    ),
                }
            }
        }
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
        "Write" => AskPreview::FileWrite {
            path: tool.target_path(input).unwrap_or_default(),
            bytes: input
                .get("content")
                .and_then(|v| v.as_str())
                .map(|s| s.len() as u64)
                .unwrap_or(0),
        },
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
        _ => AskPreview::Plain {
            text: tool.describe(input),
        },
    }
}

fn system_prompt(cwd: &std::path::Path) -> String {
    format!(
        "你是 Riot，一个跑在用户机器上的编码助手。\n\
         \n\
         工作目录：{}\n\
         平台：{}\n\
         \n\
         行为准则：\n\
         - 动手之前先看清楚。改代码前用 Read 读过要改的文件，用 Grep 找过相关位置。\n\
         - 一次只做被要求的事。顺手重构、顺手加注释、顺手改格式都会让 review 变难。\n\
         - 写代码要像周围的代码。命名、注释密度、错误处理方式都跟着现有风格走。\n\
         - 不确定就问，别猜。猜错的代价比多问一句大。\n\
         - 工具失败时读错误信息再动作，不要换个参数重试同一件事。\n\
         \n\
         回答用中文。代码和标识符保持原文。",
        cwd.display(),
        std::env::consts::OS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[tokio::test]
    async fn 回应不存在的请求不会崩() {
        // 用户在超时之后才点按钮，这时候什么都不该发生
        let p = PendingAsks::default();
        assert!(
            !p.resolve("nope", PermissionResponse::Allow { remember: vec![] })
                .await
        );
    }

    #[tokio::test]
    async fn 回应之后请求就被摘掉了() {
        let p = PendingAsks::default();
        let (tx, rx) = oneshot::channel();
        p.insert("a1".into(), tx).await;

        assert!(
            p.resolve("a1", PermissionResponse::Allow { remember: vec![] })
                .await
        );
        assert!(rx.await.is_ok());
        // 第二次应该找不到 —— 否则重复点击会让同一个操作跑两遍
        assert!(
            !p.resolve("a1", PermissionResponse::Allow { remember: vec![] })
                .await
        );
    }

    #[test]
    fn 系统提示里带上工作目录() {
        // 没有它模型会用相对路径乱猜
        let p = system_prompt(std::path::Path::new("/tmp/proj"));
        assert!(p.contains("/tmp/proj"));
    }

    #[tokio::test]
    async fn 装配好的调度器带齐权限闸围栏和联网() {
        // 这三样每漏一个都编译得过、跑得起来，只是行为悄悄降级：
        // 漏权限闸 = 所有操作不再询问；漏围栏 = 什么文件都写不了；
        // 漏联网 = WebFetch/WebSearch 一律说"未配置"。
        let s = Session::new(SessionId::from_raw("s1"), std::path::PathBuf::from("/tmp"));

        let gate = Arc::new(HostGate {
            sink: Channel::new(|_| Ok(())),
            pending: Arc::clone(&s.pending_asks),
            ids: Arc::clone(&s.ids) as Arc<dyn IdGenerator>,
            ctx: PermissionContext {
                mode: PermissionModeState(Some(PermissionMode::Default)),
                rules: Vec::new(),
                sandboxed: false,
                can_prompt_user: true,
            },
            rules_live: Arc::clone(&s.rules),
            cwd: s.cwd.clone(),
            ask_timeout: Duration::from_secs(60),
        });

        let scheduler = s.build_scheduler(
            Arc::new(Registry::new(riot_tools::tools::builtin()).expect("注册表")),
            PromptContext {
                cwd: s.cwd.clone(),
                platform: "test".into(),
                sibling_tools: Vec::new(),
                today: "2026年8月".into(),
            },
            Arc::new(riot_providers::watchdog::TokioClock),
            Arc::new(riot_protocol::web::NoWeb),
            gate,
        );

        assert!(scheduler.has_gate(), "没装权限闸，所有操作都会静默放行");
        assert!(scheduler.has_web(), "没装联网能力，联网工具会一律报未配置");
    }

    #[tokio::test]
    async fn 同一会话不允许并发两轮() {
        let s = Session::new(SessionId::from_raw("s1"), std::path::PathBuf::from("/tmp"));
        let model = ResolvedModel {
            protocol: crate::config::Protocol::Openai,
            base_url: "https://api.deepseek.com".into(),
            api_key_env: "RIOT_NOT_SET".into(),
            model: "deepseek-chat".into(),
            fallback_model: None,
            sampling: Sampling::default(),
        };
        // 第一轮会因为缺 key 立刻失败，但它必须把 running 清干净，
        // 否则会话就卡死了 —— 用户看到的是"发消息没反应"
        let ch = Channel::new(|_| Ok(()));
        let web = Arc::new(riot_protocol::web::NoWeb);
        let _ = s.run_turn("hi".into(), model, web, ch, 60).await;
        assert!(s.running.lock().await.is_none(), "失败路径没有清理 running");
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
