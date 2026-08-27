//! 权限决策类型。
//!
//! 核心不变式：**deny > ask > allow；显式规则 > 模式；
//! hook 的 allow 不能越过配置文件里的 deny/ask。**
//! 见 ARCHITECTURE.md §9.2

use crate::id::{RequestId, ToolUseId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 工具执行前的最后一道闸。
///
/// 决策链（`riot-permissions::decide`）是纯函数，算得出 allow/ask/deny
/// 但没法**问用户** —— 问用户要弹窗、要等回应、要能被取消，那些是宿主的事。
/// 这个 trait 就是那条缝：调度器在执行每个工具前问一次，宿主决定怎么答。
///
/// `[约束]` 实现必须能被取消。用户按了停止之后还在等一个没人回答的弹窗，
/// 会话就永远结束不了。
#[async_trait::async_trait]
pub trait PermissionGate: Send + Sync {
    async fn check(
        &self,
        tool: &dyn crate::tool::Tool,
        input: &serde_json::Value,
        tool_use_id: &ToolUseId,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> GateOutcome;
}

#[derive(Debug, Clone, PartialEq)]
pub enum GateOutcome {
    Allow {
        /// 权限层可以改写输入（如给命令补上安全 flag）。
        updated_input: Option<serde_json::Value>,
    },
    /// 拒绝。`message` 会作为 tool_result 发回模型 —— 所以要写成模型能
    /// 据此改变行为的话，而不是给人看的错误码。
    Deny { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "behavior", rename_all = "snake_case")]
pub enum PermissionResult {
    Allow {
        /// 权限层可以改写输入（如为命令加上安全 flag）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        updated_input: Option<serde_json::Value>,
        reason: DecisionReason,
    },
    Ask {
        message: String,
        /// UI 的"永久同意"候选项。结构化而非自由文本，
        /// 这样同一套类型能同时驱动弹窗、会话状态和配置持久化。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        suggestions: Vec<PermissionUpdate>,
        reason: DecisionReason,
    },
    Deny {
        message: String,
        reason: DecisionReason,
    },
    /// 工具内部"未决"。上层收敛为 Ask。
    ///
    /// 这是 `Tool::check_permissions` 的默认返回值 —— 工具不表态时
    /// 交给通用权限系统，而不是默认放行。
    Passthrough,
}

impl PermissionResult {
    pub fn is_allow(&self) -> bool {
        matches!(self, PermissionResult::Allow { .. })
    }
}

/// 决策理由。UI 的解释、日志、遥测共用同一份数据。
///
/// 没有理由的决策无法调试 —— 用户报"为什么它问我这个"时，
/// 你需要能立刻回答。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DecisionReason {
    /// 匹配到显式规则。
    Rule { source: RuleSource, pattern: String },
    /// 由权限模式决定。
    Mode { mode: PermissionMode },
    /// hook 做出的决策。
    Hook { name: String },
    /// LLM 分类器判定。
    Classifier { confidence: f32 },
    /// 敏感操作安全检查。**对 bypass 模式免疫。**
    SafetyCheck { safety: SafetyKind },
    /// OS 沙箱已提供硬边界，策略层放行。
    Sandbox,
    /// 命中内置白名单（官方文档站之类）。
    ///
    /// 不复用 `Rule` 是因为这不是用户配的规则 —— 用户在"为什么它没问我"
    /// 里看到一条自己从没写过的规则会更困惑。
    Preapproved { what: String },
    /// 工具要为某个具体目标征求同意 —— 没有任何规则命中，是默认行为。
    ///
    /// 典型场景是 WebFetch 抓一个陌生域名：工具想问"这个站可以吗"，
    /// 但这不构成安全发现，也不是用户写过的规则。
    ///
    /// `[约束]` 不能复用 `Rule` 顶替。理由和 `Preapproved` 一样（用户会
    /// 在解释里看到一条自己没写过的规则），但后果更严重 —— 决策链靠这个
    /// 变体区分"可被 bypass 压过的例行询问"和"必须坚持的询问"。冒充成
    /// `Rule` 会让「全部放行」对这类工具彻底失效，见
    /// [`chain::decide`](../../riot_permissions/chain/fn.decide.html) 第 3 步。
    Consent { what: String },
    /// 静态分析看不懂这次调用，所以问一句。**不是**说它危险。
    ///
    /// Bash 的命令分析是主要来源：`echo $HOME` 里的变量展开、
    /// `$(git rev-parse HEAD)` 里的命令替换、`for` 循环 —— 分析器无法
    /// 断定它们最终会执行什么，于是保守地问。
    ///
    /// `[约束]` 和 [`DecisionReason::SafetyCheck`] 的区别是**不确定性**
    /// 与**危险**之别，而这个区别决定了「全部放行」管不管用。这类判定
    /// 在正常开发里触发得极其频繁（模型干活必然用变量和管道），标成
    /// 安全发现的话，「全部放行」会变成一个几乎无法工作的模式 ——
    /// 用户开着它，却在 `echo $HOME` 上被拦住。
    Unverifiable { what: String },
    /// 用户在弹窗里的选择。
    UserChoice { remembered: bool },
    /// 无人应答超时。**默认 deny，不是 allow。**
    Timeout,
}

impl DecisionReason {
    /// 这个理由产生的 ask，能不能被「全部放行」压过。
    ///
    /// 收成一个谓词而不是散在决策链里，是因为它是**安全边界的定义**：
    /// 哪些询问在用户说"别问了"之后仍然坚持。散开写的话，每加一个
    /// `DecisionReason` 变体都要有人想起来去改决策链，而漏改的那一侧
    /// 不会有任何报错 —— 要么变成"放行模式下还在弹框"（烦），要么变成
    /// "该拦的没拦"（危险）。两种都发生过。
    ///
    /// 判据是**不确定性 vs 危险**，不是"重要程度"：
    ///
    /// - 让步：例行同意请求、静态分析看不懂 —— 这些是保守的默认行为，
    ///   而「全部放行」的语义正是替用户回答这类默认询问。
    /// - 坚持：安全发现、用户亲手写下的规则 —— 前者指向具体的高价值
    ///   目标（SSH 密钥、shell 启动脚本），后者是用户明确表达过的意愿。
    pub fn yields_to_bypass(&self) -> bool {
        match self {
            DecisionReason::Consent { .. } | DecisionReason::Unverifiable { .. } => true,
            DecisionReason::Rule { .. }
            | DecisionReason::Mode { .. }
            | DecisionReason::Hook { .. }
            | DecisionReason::Classifier { .. }
            | DecisionReason::SafetyCheck { .. }
            | DecisionReason::Sandbox
            | DecisionReason::Preapproved { .. }
            | DecisionReason::UserChoice { .. }
            | DecisionReason::Timeout => false,
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RuleSource {
    /// 组织策略。优先级最高，用户不可覆盖。
    Policy,
    /// 命令行参数。
    CliArg,
    /// 会话内用户亲手点的"总是允许"。
    ///
    /// 排在配置文件（Local/Project/User）之前：用户刚刚对着具体的命令
    /// 做了决定，那个决定比他半年前写的配置更能代表当前意图。
    Session,
    /// 项目本地未提交配置。
    Local,
    /// 项目提交进版本库的配置。
    Project,
    /// 用户全局配置。
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    /// 默认：写操作询问。
    Default,
    /// 工作区内编辑自动通过。
    AcceptEdits,
    /// 只读规划模式。
    Plan,
    /// 小模型判危：明确安全的自动放行，其余照常弹窗。
    ///
    /// 定位是 [`Self::Default`] 和 [`Self::Unattended`] 之间缺的那一档。
    /// 长任务里的绝大多数询问是 `cargo check`、`ls`、读一个文件这类东西，
    /// 逐个点"允许"会把人训练成无脑点 —— 而无脑点的人在真正危险的那次
    /// 也会点。让小模型先筛掉显然安全的，剩下的才值得打断人。
    ///
    /// `[约束]` 分类器的权力**不超过** [`Self::BypassPermissions`]：只有
    /// `yields_to_bypass()` 为真的询问才可能被它自动放行。安全检查
    /// （SSH 密钥、shell 启动脚本）和用户亲手写的 ask 规则对它免疫 ——
    /// 判据和分层免疫是同一条，见 [`DecisionReason::yields_to_bypass`]。
    Auto,
    /// 全部放行 —— 但敏感操作安全检查仍然生效。
    BypassPermissions,
    /// 什么都不问，安全检查也一并放行。
    ///
    /// `[约束]` 这是本产品能给出的最弱保护，只有显式的 deny 规则还拦得住。
    /// 它存在的理由是长任务：用户要离开电脑，而每一次询问都会把任务
    /// 停在那里等一个不在场的人。
    ///
    /// 和 [`PermissionMode::BypassPermissions`] 的区别就是那层分层免疫 ——
    /// 放行模式仍然守着 SSH 密钥、shell 启动脚本这些能换来持久执行权的
    /// 目标，这个模式连它们一起交出去。UI 上必须写清这一点。
    Unattended,
    /// ask 一律转 deny（无人值守场景）。
    DontAsk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SafetyKind {
    /// 写 .git/ 目录。
    GitInternals,
    /// SSH 配置与密钥。
    SshConfig,
    /// shell 启动脚本（.zshrc 等）—— 改这些等于取得持久化执行权。
    ShellRc,
    /// 本应用自己的配置目录。
    AgentConfig,
    /// 构建工具链的配置与可执行目录（`~/.cargo/config.toml`、`.envrc`……）。
    ///
    /// 和 [`SafetyKind::ShellRc`] 同类:写它等于取得持久化执行权,只是触发
    /// 点从"下次开终端"变成"下次构建"。单独成一档是因为它有一个 ShellRc
    /// 没有的性质 —— 这些目录**在沙箱的可写集里**（不放开的话第一条
    /// `cargo build` 就死在写不了缓存上），于是 OS 边界指望不上,只能靠
    /// 这一层拦。见 `riot_permissions::bash::write_targets`。
    ToolchainConfig,
    /// 疑似凭证文件。
    Credentials,
    /// 命令里检测到注入模式。
    CommandInjection,
    /// 命令 AST 解析失败或含不认识的结构。
    UnparseableCommand,
    /// 主动渗透动作打到了**未授权**的目标（不在渗透 scope 内）。
    ///
    /// `[约束]` 归到安全检查而不是普通同意，就是为了让它**对 bypass 免疫**:
    /// 「全部放行」的语义是"信任 agent 做常规开发"，不是"允许它对任意目标
    /// 发起攻击"。scope 外的改包、fuzzing、爬虫必须由用户显式授权目标，
    /// 哪怕开着 bypass。只有无人值守模式（用户明示交出一切）才放行。
    OutOfScope,
    /// 这条命令会在 **OS 沙箱之外**执行。
    ///
    /// `[约束]` 归到安全检查而不是普通询问，是为了让它**对 bypass 免疫**。
    /// 「全部放行」的语义是"信任 agent 做常规开发"，而开着沙箱的会话里，
    /// 沙箱是最后一道边界 —— 允许模型自己把它关掉、还不用问一声，等于
    /// bypass 模式下根本没有边界。这一档存在的全部意义就是：出沙箱这件事
    /// 由**用户**点头，不由模型决定。只有无人值守模式（用户明示交出一切）
    /// 才放行。
    SandboxEscape,
}

/// 结构化的"永久同意"。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PermissionUpdate {
    /// 加一条规则，如 `Bash(npm run *)`。
    AddRule {
        tool: String,
        pattern: Option<String>,
        decision: RuleDecision,
        scope: UpdateScope,
    },
    /// 切换权限模式。
    SetMode {
        mode: PermissionMode,
        scope: UpdateScope,
    },
    /// 把目录加入工作区围栏。
    AddWorkingDirectory { path: PathBuf, scope: UpdateScope },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuleDecision {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UpdateScope {
    /// 仅本次会话。
    Session,
    /// 写入项目本地配置。
    Local,
    /// 写入项目配置（会提交进版本库）。
    Project,
    /// 写入用户全局配置。
    User,
}

/// 发给 UI 的权限请求详情。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionAsk {
    pub tool_use_id: ToolUseId,
    pub tool_name: String,
    /// 给用户看的一句话描述，如 "运行 npm test"。
    pub summary: String,
    /// 结构化预览：diff、命令、URL。UI 据此渲染。
    pub preview: AskPreview,
    pub suggestions: Vec<PermissionUpdate>,
    pub reason: DecisionReason,
}

/// 一条还在等用户回答的权限询问（会话快照用）。
///
/// `permission_request` 事件只在询问产生那一刻发一次；界面切走的话它发进
/// 没人听的旧通道。快照不带的话，切回来弹窗再也不出现 —— 那次询问只能
/// 等到超时被拒，模型收到"授权请求没有得到回应"。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PendingAsk {
    pub request_id: RequestId,
    pub detail: PermissionAsk,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AskPreview {
    Command {
        command: String,
        cwd: PathBuf,
    },
    FileEdit {
        path: PathBuf,
        diff: String,
    },
    /// 写整文件。`preview` 是内容的前若干行 —— 只显示路径和字节数
    /// 等于让用户盲签，而这个文件顶上的约束明说了不能这样。`lines`
    /// 是总行数，`truncated` 标记 preview 是否被截断。
    FileWrite {
        path: PathBuf,
        bytes: u64,
        preview: String,
        lines: u64,
        truncated: bool,
    },
    NetworkFetch {
        url: String,
    },
    Plain {
        text: String,
    },
    /// 模型主动提的结构化问题（`AskUserQuestion` 工具）。
    ///
    /// `[取舍]` 复用权限的 ask 通道而不是另建一条问答链路。理由是这条通道
    /// 已经解决了同一批难题：等用户时的超时（按拒绝处理）、中断时补齐
    /// tool_result 配对、子 agent 不许弹窗（`can_prompt_user`）。另建一条
    /// 就要把这些全部重做一遍，而它们每一个都是踩过坑才对的。
    ///
    /// 代价是"提问"被塞进了权限语义 —— 用户的选择经 [`PermissionResponse::Allow`]
    /// 的 `choice` 回来，再由宿主写进工具输入。
    Choice {
        question: String,
        options: Vec<AskChoiceOption>,
        /// 允许选多项。
        allow_multiple: bool,
    },
}

/// 结构化提问的一个候选项。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AskChoiceOption {
    /// 回传给模型的稳定标识。用它而不是 label —— label 是给人读的文案，
    /// 改一个字就会让模型收到不一样的答案。
    pub id: String,
    /// 给用户看的文案。
    pub label: String,
}

/// 分类器的判定。
///
/// 只有两个结果，不是三个。"判不准"和"不安全"都归到 [`Self::Hold`] ——
/// 对调用方来说它们的后续完全一样（继续等用户），分成三档只会诱使调用方
/// 对"判不准"另做处理，而那条路唯一能通向的地方是放行。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SafetyVerdict {
    /// 明确安全，可以不打断用户。`confidence` 进 [`DecisionReason::Classifier`]，
    /// 让日志和界面能解释这次放行是谁批的。
    Safe { confidence: f32 },
    /// 继续等用户。不安全、判不准、请求失败、输出看不懂，全在这里。
    Hold,
}

/// 小模型判危。[`PermissionMode::Auto`] 用它。
///
/// 和 [`PermissionGate`] 同一个路子：决策链是纯函数，跑不了 IO，所以这层
/// 判断挂在闸上（那里本来就是 async，也本来就是弹窗与等待的地方）。
#[async_trait::async_trait]
pub trait SafetyClassifier: Send + Sync + 'static {
    /// 判断这次调用安不安全。`what` 来自 [`crate::tool::Tool::classifier_input`]。
    ///
    /// `[约束]` 任何异常都要返回 [`SafetyVerdict::Hold`]，不能返回 Safe。
    /// 分类器坏掉（没配模型、超时、输出对不上格式）时该退回问人 ——
    /// 一个坏掉的判危器静默放行，比没有判危器危险得多。
    async fn judge(&self, tool: &str, what: &str) -> SafetyVerdict;
}

/// 不判危的占位实现。没配便宜档模型时用它 —— 所有询问照常弹窗。
pub struct NoClassifier;

#[async_trait::async_trait]
impl SafetyClassifier for NoClassifier {
    async fn judge(&self, _tool: &str, _what: &str) -> SafetyVerdict {
        SafetyVerdict::Hold
    }
}

/// 工具做权限判断时能看到的上下文。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PermissionContext {
    pub mode: PermissionModeState,
    /// 已生效的规则，按来源优先级排序。
    pub rules: Vec<PermissionRule>,
    /// 该工具是否运行在 OS 沙箱内。沙箱已提供硬边界时策略层可放宽。
    pub sandboxed: bool,
    /// 异步子 agent 不能弹窗，ask 只能收敛为 deny。
    pub can_prompt_user: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PermissionModeState(pub Option<PermissionMode>);

impl PermissionModeState {
    pub fn get(&self) -> PermissionMode {
        self.0.unwrap_or(PermissionMode::Default)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionRule {
    pub tool: String,
    /// None = 整工具规则；Some = 内容级规则，如 `npm run *`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    pub decision: RuleDecision,
    pub source: RuleSource,
}

/// 宿主对权限请求的应答。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PermissionResponse {
    Allow {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        remember: Vec<PermissionUpdate>,
        /// 用户选中的选项 id（只有 [`AskPreview::Choice`] 会用到）。
        ///
        /// 空 = 这不是一次提问，或者用户没选任何一项。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        choice: Vec<String>,
    },
    Deny {
        /// 用户可以说明理由，会作为 tool_result 喂回模型。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_is_not_allow() {
        assert!(!PermissionResult::Passthrough.is_allow());
    }

    #[test]
    fn rule_source_priority_ordering() {
        // Policy 优先级最高（Ord 最小），User 最低。
        assert!(RuleSource::Policy < RuleSource::CliArg);
        assert!(RuleSource::CliArg < RuleSource::Local);
        assert!(RuleSource::Project < RuleSource::User);
    }
}
