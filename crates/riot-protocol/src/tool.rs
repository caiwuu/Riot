//! 工具契约。
//!
//! 设计要点：trait 的默认方法就是 fail-closed 默认值，而**没有默认值的
//! 方法编译器强制实现**。这比 TS 版本的 `buildTool()` 工厂更强 ——
//! 漏写 `prompt()` 在那边要到运行时才发现。
//!
//! 见 ARCHITECTURE.md §6

use crate::event::ProgressPayload;
use crate::id::{SessionId, ToolUseId};
use crate::message::{Message, ToolResultContent};
use crate::permission::{PermissionContext, PermissionResult};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[async_trait]
pub trait Tool: Send + Sync + 'static {
    // ────────────────────────────────────────────────────────
    // 必须实现 —— 不要给这些加默认实现
    // ────────────────────────────────────────────────────────

    /// 工具名。进 API 的 `tools[].name`，也是权限规则匹配的键。
    ///
    /// 返回 `&str` 而不是 `&'static str`：内置工具的名字是编译期常量，
    /// 但 MCP 工具的名字（`mcp__server__tool`）来自运行时的服务器清单 ——
    /// 要求 'static 会逼适配层去 `Box::leak`。
    fn name(&self) -> &str;

    fn input_schema(&self) -> schemars::Schema;

    /// 进 API `tools[].description` 的完整使用说明。
    ///
    /// 这里要写清与其它工具的分工和 NEVER 列表
    /// （例："搜索永远用 Grep 工具，不要在 Bash 里跑 grep"）。
    fn prompt(&self, ctx: &PromptContext) -> String;

    /// 给 UI 看的一句话描述，如 "读取 src/main.rs"。
    fn describe(&self, input: &serde_json::Value) -> String;

    /// 执行。
    ///
    /// 返回 [`ToolOutcome`] 而**不是** `Result` —— 失败是正常的返回值。
    /// 工具内部可以自由用 `?`，但必须在函数边界把 `Result` 转成
    /// `ToolOutcome::Failed`。这样类型系统就保证了"工具错误不会抛穿主循环"。
    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome;

    // ────────────────────────────────────────────────────────
    // fail-closed 默认值 —— 漏写不会造成危险行为
    // ────────────────────────────────────────────────────────

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        false
    }

    /// 能否与同批次其它工具并行执行。
    ///
    /// **这是按输入判定的函数，不是静态标签。** 同一个 Bash 工具，
    /// `ls -la` 可以并行，`rm -rf` 必须独占。
    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        false
    }

    fn is_destructive(&self, _input: &serde_json::Value) -> bool {
        false
    }

    fn interrupt_behavior(&self) -> InterruptBehavior {
        InterruptBehavior::Cancel
    }

    /// 本工具失败时，是否取消同批次正在跑的兄弟工具。
    ///
    /// shell 类工具应该返回 `true` —— 命令之间常有隐式依赖，
    /// `mkdir foo` 失败之后 `cd foo && ...` 就没有意义了。
    /// Read / Grep 这类彼此独立的保持 `false`。
    ///
    /// 注意默认值是 `false`，方向和其它 fail-closed 默认值相反。这里
    /// "安全"指的不是少做事：级联会误杀无关工具，用户看到一串"已取消"
    /// 却不知道为什么；而不级联最多是多跑几个注定失败的命令，那些失败
    /// 本身是可见的。误杀比浪费更难排查。
    fn cascades_on_failure(&self) -> bool {
        false
    }

    /// 结果超过预算时落盘，模型收到路径与预览。
    ///
    /// Read 类工具必须返回 [`ResultBudget::Unlimited`]，否则会产生
    /// "Read → 结果落盘成文件 → 模型又去 Read 那个文件"的循环。
    fn result_budget(&self) -> ResultBudget {
        ResultBudget::Limit(50_000)
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        PermissionResult::Passthrough
    }

    /// 语义校验（结构校验由 schema 负责）。
    ///
    /// 例：文件存在吗？已经 Read 过吗？
    async fn validate_input(
        &self,
        _input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        Ok(())
    }

    /// 喂给安全分类器的文本。None = 跳过分类器。
    ///
    /// 安全敏感工具必须覆盖。
    fn classifier_input(&self, _input: &serde_json::Value) -> Option<String> {
        None
    }

    /// 该工具操作的路径。用于路径围栏检查与 hook 规则匹配。
    fn target_path(&self, _input: &serde_json::Value) -> Option<PathBuf> {
        None
    }

    /// 是否参与延迟加载（工具目录瘦身）。
    ///
    /// 延迟工具在总量超过阈值时不进请求的 tools 数组，模型只知道名字，
    /// 用 ToolSearch 按需取回完整定义。MCP 工具返回 true —— 它们是
    /// 按工作流配的，大多数轮次用不到；内置工具保持 false，它们的
    /// 描述是模型的基本操作手册。
    fn should_defer(&self) -> bool {
        false
    }

    fn user_facing_name(&self) -> &str {
        self.name()
    }

    /// 兼容改名 —— 旧 transcript 里的名字还能被解析。
    fn aliases(&self) -> &[&'static str] {
        &[]
    }
}

/// 工具执行结果。
///
/// 注意 `Failed` 是 enum 变体而不是 `Err` —— 工具失败是正常的返回值，
/// 会转成 `tool_result(is_error)` 喂回模型让它自我纠正。
#[derive(Debug, Clone, PartialEq)]
pub enum ToolOutcome {
    Ok {
        /// 给模型看的结果。
        model_content: ToolResultContent,
        /// 给 UI 看的**结构化数据**（不是渲染好的字符串）。
        /// None = UI 不显示（如 TodoWrite，结果显示在待办面板里）。
        ui_payload: Option<UiPayload>,
        /// 旁路注入的消息（图片 metadata 等），不塞进 tool_result。
        side_messages: Vec<Message>,
    },
    Failed {
        /// 给模型的纠错指令。用祈使句，不要贴原始错误。
        /// 见 ARCHITECTURE.md §6.5
        error_for_model: String,
        ui_payload: Option<UiPayload>,
    },
    Cancelled,
}

impl ToolOutcome {
    pub fn ok_text(text: impl Into<String>) -> Self {
        Self::Ok {
            model_content: ToolResultContent::text(text),
            ui_payload: None,
            side_messages: Vec::new(),
        }
    }

    pub fn failed(msg: impl Into<String>) -> Self {
        Self::Failed {
            error_for_model: msg.into(),
            ui_payload: None,
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self, ToolOutcome::Failed { .. })
    }
}

/// 给 UI 的结构化结果。渲染在 React 侧做。
///
/// 这是相对 TS 版本的实质改进：Claude Code 的 `renderToolResultMessage()`
/// 直接返回 Ink 组件，内核与 UI 耦合。桌面端不能这么做 ——
/// 内核要能在没有 UI 的情况下跑（测试、headless）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UiPayload {
    FileRead {
        path: PathBuf,
        line_count: usize,
        truncated: bool,
    },
    FileDiff {
        path: PathBuf,
        hunks: Vec<DiffHunk>,
    },
    FileWrite {
        path: PathBuf,
        bytes: u64,
        created: bool,
    },
    BashOutput {
        stdout: String,
        stderr: String,
        exit_code: i32,
        duration_ms: u64,
    },
    SearchResults {
        matches: Vec<SearchMatch>,
        total: usize,
        truncated: bool,
    },
    Plain {
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DiffHunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SearchMatch {
    pub path: PathBuf,
    pub line: usize,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InterruptBehavior {
    /// 可以立即取消（Bash、WebFetch）。
    Cancel,
    /// 不可中断，让新消息排队等（正在写文件）。
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultBudget {
    Limit(usize),
    /// 禁止落盘。Read 类工具必须用这个。
    Unlimited,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ValidationError {
    /// 给模型的纠错指令。
    #[error("{0}")]
    Rejected(String),
}

impl ValidationError {
    pub fn rejected(msg: impl Into<String>) -> Self {
        Self::Rejected(msg.into())
    }
}

/// 工具执行时拿到的上下文。
#[derive(Clone)]
pub struct ToolContext {
    pub session_id: SessionId,
    pub tool_use_id: ToolUseId,
    pub cwd: PathBuf,
    /// 工具产物的落盘目录（会话专属）。截图的原图写在这里 —— 消息里只放
    /// 压缩图和这个目录下的路径，几 MB 的 base64 不进会话历史。
    /// 目录由宿主创建；写不进时工具自行降级（消息里不带路径），不报错。
    pub artifacts_dir: PathBuf,
    /// 本工具专属的取消令牌。父级取消会传播下来。
    pub cancel: CancellationToken,
    /// 进度上报通道。
    pub progress: ProgressSink,
    /// 先读后写协议的状态缓存。
    pub file_state: Arc<dyn FileStateCache>,
    /// 注入的文件系统。core / tools 里不允许直接用 std::fs。
    pub fs: Arc<dyn FileSystem>,
    /// 注入的子进程执行器。
    pub proc: Arc<dyn ProcessRunner>,
    /// 注入的联网能力。只有 WebFetch / WebSearch 用。
    ///
    /// 默认是 [`crate::web::NoWeb`]（一律拒绝）—— 宿主没装配就等于没网，
    /// 而不是悄悄用上某个兜底后端。
    pub web: Arc<dyn crate::web::WebAccess>,
    /// 注入的浏览器能力。只有 Browser* 系列工具用。
    ///
    /// 默认是 [`crate::browser::NoBrowser`]（一律说"用不了"）—— 和 web
    /// 同理，宿主没装配就该明说，不该悄悄换个行为。
    pub browser: Arc<dyn crate::browser::BrowserAccess>,
    /// 注入的终端面板。长期服务（dev server）跑在这里，不走 `proc` ——
    /// 那条路收尾时会清掉整个进程组，服务活不过一次调用。
    ///
    /// 默认是 [`crate::terminal::NoTerminal`]，同 web / browser 的规矩。
    pub terminal: Arc<dyn crate::terminal::TerminalAccess>,
    /// 图片怎么交给模型。产出图片的工具（截图）用它。
    ///
    /// 默认是 [`crate::vision::NoVision`]（"模型不收图片，也没配兼容模型"）
    /// —— 装配漏了的时候工具会明确让用户去配，而不是让图片在 provider 那层
    /// 被静默替换成一句话。
    pub vision: Arc<dyn crate::vision::VisionAccess>,
    /// 注入的时间源。
    ///
    /// WebFetch 的响应缓存要判 TTL，工具耗时统计也要它。不能用
    /// `Instant::now` —— 那会让缓存过期行为在黄金回放里不可复现。
    pub clock: Arc<dyn Clock>,
}

#[derive(Clone)]
pub struct ProgressSink {
    tool_use_id: ToolUseId,
    tx: tokio::sync::mpsc::UnboundedSender<(ToolUseId, ProgressPayload)>,
}

impl ProgressSink {
    pub fn new(
        tool_use_id: ToolUseId,
        tx: tokio::sync::mpsc::UnboundedSender<(ToolUseId, ProgressPayload)>,
    ) -> Self {
        Self { tool_use_id, tx }
    }

    /// 上报进度。通道关闭时静默丢弃 —— 进度丢失不是错误。
    pub fn send(&self, payload: ProgressPayload) {
        let _ = self.tx.send((self.tool_use_id.clone(), payload));
    }
}

/// 组装工具 prompt 时的上下文。
pub struct PromptContext {
    pub cwd: PathBuf,
    pub platform: String,
    /// 当前会话可用的其它工具名。用于在 prompt 里写清分工。
    pub sibling_tools: Vec<String>,
    /// 当前年月，如 `2026年8月`。
    ///
    /// WebSearch 的描述里要写死这个 —— 否则模型会按自己的知识截止日期
    /// 去构造搜索词（"React 文档 2024"），拿回一堆过时结果还深信不疑。
    pub today: String,
}

// ────────────────────────────────────────────────────────────
// 注入的基础设施 trait
//
// 这些存在的唯一理由是让黄金回放测试能控制非确定性。
// 见 docs/VERIFICATION.md §4.2
// ────────────────────────────────────────────────────────────

/// 先读后写协议的状态缓存。
pub trait FileStateCache: Send + Sync {
    fn get(&self, path: &std::path::Path) -> Option<FileState>;
    fn put(&self, path: PathBuf, state: FileState);
    fn invalidate(&self, path: &std::path::Path);
    /// 最近读过的文件，最新在前。压缩后恢复工作集要用。
    fn recent(&self, limit: usize) -> Vec<(PathBuf, FileState)>;

    /// 记下这个文件被本会话改动**之前**的样子。`None` = 那时它还不存在。
    ///
    /// `[约束]` 只有第一次算数。同一个文件改五次，基线永远是最初那份 ——
    /// 每次都覆盖的话，"这次会话到底动了什么"会退化成"最后一次改了什么"，
    /// 而前者才是 review 要回答的问题。
    ///
    /// 默认空实现：只有宿主那份缓存要给改动视图供货，测试替身不必
    /// 为此各写一遍。
    fn note_baseline(&self, _path: PathBuf, _before: Option<String>) {}

    /// 本会话改动过的文件，附各自的基线。顺序不定。
    fn baselines(&self) -> Vec<(PathBuf, Option<String>)> {
        Vec::new()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileState {
    pub content: String,
    /// 读取时的文件 mtime，单位毫秒。
    pub mtime_ms: u64,
    pub view: FileView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileView {
    Full,
    /// 部分视图不可作为编辑依据。
    Partial {
        offset: usize,
        limit: usize,
    },
}

#[async_trait]
pub trait FileSystem: Send + Sync {
    async fn read(&self, path: &std::path::Path) -> std::io::Result<Vec<u8>>;
    async fn write(&self, path: &std::path::Path, data: &[u8]) -> std::io::Result<()>;
    async fn metadata(&self, path: &std::path::Path) -> std::io::Result<FileMeta>;
    async fn read_dir(&self, path: &std::path::Path) -> std::io::Result<Vec<PathBuf>>;
    async fn canonicalize(&self, path: &std::path::Path) -> std::io::Result<PathBuf>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMeta {
    pub mtime_ms: u64,
    pub len: u64,
    pub is_dir: bool,
}

#[async_trait]
pub trait ProcessRunner: Send + Sync {
    async fn run(
        &self,
        spec: ProcessSpec,
        cancel: CancellationToken,
    ) -> std::io::Result<ProcessOutput>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
    /// 执行耗时。
    ///
    /// 由 runner 提供而不是让调用方自己掐表：runner 本来就在为超时计时，
    /// 而调用方要拿到同样的数字就得注入一个 Clock（`SystemTime::now` 在
    /// 内核里是禁用的）。放这里省掉那层注入。
    pub duration_ms: u64,
}

/// 时间源。
///
/// microcompact 的 60 分钟缓存冷热判断依赖时间，黄金回放测试
/// 必须能把时间快进。
#[async_trait]
pub trait Clock: Send + Sync {
    /// 当前 Unix 毫秒时间戳。
    fn now_ms(&self) -> u64;
    async fn sleep_ms(&self, ms: u64);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyTool;

    #[async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &'static str {
            "Dummy"
        }
        fn input_schema(&self) -> schemars::Schema {
            schemars::json_schema!({ "type": "object" })
        }
        fn prompt(&self, _: &PromptContext) -> String {
            "dummy".into()
        }
        fn describe(&self, _: &serde_json::Value) -> String {
            "dummy".into()
        }
        async fn call(&self, _: serde_json::Value, _: ToolContext) -> ToolOutcome {
            ToolOutcome::ok_text("ok")
        }
    }

    #[test]
    fn defaults_are_fail_closed() {
        let t = DummyTool;
        let input = serde_json::json!({});

        assert!(!t.is_concurrency_safe(&input), "默认必须不可并发");
        assert!(!t.is_read_only(&input), "默认必须视为会写");
        assert!(!t.is_destructive(&input));
        assert_eq!(
            t.check_permissions(&input, &PermissionContext::default()),
            PermissionResult::Passthrough
        );
        assert_eq!(t.classifier_input(&input), None);
    }
}
