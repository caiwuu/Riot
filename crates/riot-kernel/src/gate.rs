//! 宿主侧的权限闸：决策链算出 allow/ask/deny 之后，ask 那一支在这里
//! 落地 —— 弹窗、挂起等待、超时、Auto 模式的判危竞速，以及"总是允许"
//! 的落实。
//!
//! 从 session.rs 拆出来的独立职责。会话只负责在每轮开工时装配
//! [`HostGate`]（共享 rules/mode 的活引用），其余都发生在这里。
//! 决策本身在 `riot-permissions::decide` —— 这里不做任何 allow/deny
//! 的**判定**，只做"问用户"这个动作。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;

use riot_permissions::RuleSet;
use riot_protocol::event::AgentEvent;
use riot_protocol::id::{IdGenerator, RequestId};
use riot_protocol::permission::{
    AskPreview, DecisionReason, GateOutcome, PendingAsk, PermissionAsk, PermissionContext,
    PermissionGate, PermissionMode, PermissionModeState, PermissionResponse, PermissionResult,
    PermissionRule, SafetyVerdict,
};
use riot_protocol::tool::Tool;

use crate::session::SessionSink;

/// 判危通过之后，等这么久再自动放行。
///
/// 存在的理由是防误触：弹窗不该在用户手指正落下的那一刻消失，把这次点击
/// 漏给底下的界面。它挡不住"看到弹窗、想两秒才点"—— 那时早放行了；挡的是
/// 判危结果和点击几乎同时到达的那一小段。
const CLASSIFY_GRACE: Duration = Duration::from_millis(200);

/// 一条挂着的询问：应答通道，加上给界面重建弹窗用的详情。
struct PendingEntry {
    tx: oneshot::Sender<PermissionResponse>,
    detail: PermissionAsk,
    /// 到达序号。HashMap 不保序，快照按它排 —— 乱序重建会让弹窗
    /// 顺序和产生顺序对不上。
    seq: u64,
}

#[derive(Default)]
pub struct PendingAsks {
    map: Mutex<HashMap<String, PendingEntry>>,
    seq: AtomicU64,
}

impl PendingAsks {
    pub(crate) async fn insert(
        &self,
        id: String,
        tx: oneshot::Sender<PermissionResponse>,
        detail: PermissionAsk,
    ) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        self.map
            .lock()
            .await
            .insert(id, PendingEntry { tx, detail, seq });
    }

    pub async fn resolve(&self, id: &str, response: PermissionResponse) -> bool {
        match self.map.lock().await.remove(id) {
            // 接收端已经走了（超时或取消）。不是错误 —— 用户在超时之后
            // 才点了按钮，这时候什么都不该发生。
            Some(e) => e.tx.send(response).is_ok(),
            None => false,
        }
    }

    async fn forget(&self, id: &str) {
        self.map.lock().await.remove(id);
    }

    /// 还在等回答的询问，按到达顺序。进会话快照（session.resume）——
    /// `permission_request` 事件只发一次，切走的界面靠这份快照把弹窗
    /// 重建出来，否则那次询问只能等到超时被拒。
    pub async fn snapshot(&self) -> Vec<PendingAsk> {
        let g = self.map.lock().await;
        let mut v: Vec<(u64, PendingAsk)> = g
            .iter()
            .map(|(id, e)| {
                (
                    e.seq,
                    PendingAsk {
                        request_id: RequestId::from_raw(id.clone()),
                        detail: e.detail.clone(),
                    },
                )
            })
            .collect();
        v.sort_by_key(|(seq, _)| *seq);
        v.into_iter().map(|(_, a)| a).collect()
    }

    pub(crate) async fn clear(&self) {
        self.map.lock().await.clear();
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
pub(crate) struct HostGate {
    pub(crate) sink: SessionSink,
    pub(crate) pending: Arc<PendingAsks>,
    pub(crate) ids: Arc<dyn IdGenerator>,
    pub(crate) ctx: PermissionContext,
    /// 和 Session.rules 是同一份。"总是允许"写进这里，同一轮内的
    /// 下一次调用立即生效。
    pub(crate) rules_live: Arc<Mutex<Vec<PermissionRule>>>,
    /// 和 Session.mode 是同一份。批准计划把模式切到执行档之后，
    /// 同一轮的下一个工具调用就要按新模式判定。
    pub(crate) mode_live: Arc<Mutex<PermissionMode>>,
    pub(crate) cwd: std::path::PathBuf,
    /// 等用户回应的上限，来自配置。见 `crate::session` 的 ASK_TIMEOUT_RANGE。
    pub(crate) ask_timeout: Duration,
    /// PreToolUse hooks。deny 一票否决、ask 强制询问、allow 只把
    /// "要问"升级成"放行" —— 内置决策链的 Deny 不可被 hook 压过。
    pub(crate) hooks: Arc<crate::hooks::HookEngine>,
    /// Auto 模式的判危分类器。没配便宜档模型时是
    /// [`riot_protocol::permission::NoClassifier`]（永远 Hold），
    /// Auto 模式于是退化成 Default —— 不会静默放行。
    pub(crate) classifier: Arc<dyn riot_protocol::permission::SafetyClassifier>,
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
        let outcome = if let Some(reason) =
            hook_ask.filter(|_| !matches!(decided, PermissionResult::Deny { .. }))
        {
            let spec = AskSpec {
                message: format!("PreToolUse hook 要求确认：{reason}"),
                suggestions: vec![],
                reason: DecisionReason::Hook {
                    name: "PreToolUse".into(),
                },
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
                PermissionResult::Passthrough if hook_allow => GateOutcome::Allow {
                    updated_input: None,
                },
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
                        GateOutcome::Allow {
                            updated_input: None,
                        }
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
            (
                GateOutcome::Allow {
                    updated_input: None,
                },
                Some(r),
            ) => GateOutcome::Allow {
                updated_input: Some(r),
            },
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
///
/// 也不能：提问（`UserChoice`，即 `AskUserQuestion` 的选项卡）。这不是
/// 信任问题 —— hook 的 allow 回答不了一道选择题。跳过卡片的话工具拿着
/// 空选择跑，必然以"没有收到用户的选择"失败。
pub(crate) fn hook_may_skip_ask(reason: &DecisionReason) -> bool {
    !matches!(
        reason,
        DecisionReason::SafetyCheck { .. }
            | DecisionReason::Rule { .. }
            | DecisionReason::UserChoice { .. }
    )
}

/// 把用户选中的选项写进工具输入，交给 `AskUserQuestion` 读。
///
/// 走 `updated_input` 而不是另开一条通道：权限层本来就有改写输入的权力
/// （给命令补安全 flag 用的就是它），提问的答案是同一件事的另一种用法。
///
/// 返回 None = 不改输入。空选择必须走这条路：普通的"允许一次"也经过这里，
/// 给每个工具都塞一个空的 `__chosen` 字段会让工具入参多出一个没人要的键。
pub(crate) fn inject_choice(
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
pub(crate) fn apply_remember(
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
        // 详情和应答通道一起挂着：事件只发一次，界面切走再切回时靠
        // session.resume 的快照重建弹窗（见 PendingAsks::snapshot）。
        self.pending
            .insert(request_id.clone(), tx, ask.clone())
            .await;

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
        if let Some(verdict) = self
            .classify_race(tool, input, &reason, &mut rx, cancel)
            .await
        {
            self.pending.forget(&request_id).await;
            // 告诉界面这个弹窗作废了，理由是分类器 —— 不发的话它挂在那里，
            // 用户点"允许"毫无反应（操作早就放行并跑完了）。
            self.resolved(&request_id, verdict);
            return GateOutcome::Allow {
                updated_input: None,
            };
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
                // `[约束]` 超时按拒绝处理。见 crate::session 里
                // ASK_TIMEOUT_RANGE 的注释。
                //
                // 提问的超时不能劝模型"重新提出"：没人在场时重新提问的结局
                // 还是超时，一来一回就成了每分钟一轮的提问循环。和工具自己
                // 的空选择失败同一个口径 —— 讲清取舍，停下来等人。
                let message = if tool.name() == "AskUserQuestion" {
                    format!(
                        "等了 {} 秒没有人回答。不要立刻重新提问，也不要自己替用户挑一个 —— \
                         用普通回复把这个决定和各选项的取舍讲清楚，然后停下来等他说。",
                        timeout.as_secs()
                    )
                } else {
                    format!(
                        "等待授权超过 {} 秒，本次操作未执行。如果仍然需要，请重新提出。",
                        timeout.as_secs()
                    )
                };
                GateOutcome::Deny { message }
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
    pub(crate) async fn classify_race(
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

/// 弹窗预览：把工具入参变成用户看得懂的形状。
pub(crate) fn preview_of(
    tool: &dyn Tool,
    input: &serde_json::Value,
    cwd: &std::path::Path,
) -> AskPreview {
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
            let content = input
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
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
            || AskPreview::Plain {
                text: tool.describe(input),
            },
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
