//! Riot 协议层：宿主、内核、前端三方共享的契约。
//!
//! 这个 crate 是依赖图的叶子 —— 它不依赖 workspace 内任何其它 crate。
//! 所有类型都 derive [`schemars::JsonSchema`]，构建时生成 JSON Schema
//! 与 TypeScript 类型定义。
//!
//! **TS 类型必须是生成的，不允许手写。** 见 docs/VERIFICATION.md §2

pub mod browser;
pub mod changes;
pub mod compact;
pub mod env;
pub mod event;
pub mod hook;
pub mod hostcall;
pub mod id;
pub mod message;
pub mod permission;
pub mod provider;
pub mod rpc;
pub mod runner;
pub mod schedule;
pub mod task;
pub mod terminal;
pub mod tool;
pub mod turn;
pub mod vision;
pub mod web;

pub use browser::{
    Action as BrowserAction, BrowserAccess, BrowserUnavailable, Command as BrowserCommand,
    Event as BrowserEvent, InteractError, InterceptOp, Nav as BrowserNav, NetQuery, NoBrowser,
    Target as BrowserTarget, WaitCondition,
};
pub use changes::{ChangeStatus, DiffLine, FileChange, GitChanges, Hunk, LineKind};
pub use compact::{CompactBudget, CompactResult, Compactor};
pub use env::{BrowserGlance, EnvAlert, EnvProbe, EnvSnapshot, NoEnvProbe};
pub use event::{
    AbortSource, AgentError, AgentEvent, CompactStrategy, ProgressPayload, StreamDelta,
    TerminalReason, Transition,
};
pub use id::{
    AgentId, IdGenerator, MessageId, NanoIdGenerator, RequestId, SessionId, ToolUseId, TurnId,
};
pub use message::{
    AssistantContent, Attachment, Message, MessageMeta, SystemLevel, ToolResultContent, Usage,
    UserContent,
};
pub use permission::{
    AskPreview, DecisionReason, PermissionAsk, PermissionContext, PermissionMode,
    PermissionResponse, PermissionResult, PermissionRule, PermissionUpdate, RuleDecision,
    RuleSource, SafetyKind, UpdateScope,
};
pub use provider::{
    Provider, ProviderError, ProviderEvent, ProviderRequest, ProviderStream, ThinkingConfig,
    ThinkingEffort, ThinkingPolicy, ToolSpec,
};
pub use rpc::{RpcEnvelope, RpcError, RpcErrorCode, RpcNotification, RpcRequest, RpcResponse};
pub use schedule::{
    MissedRun, NoSchedule, Repeat, RunTargetSpec, ScheduleAccess, ScheduleDraft, ScheduleError,
    SchedulePatch, ScheduleRun, ScheduleRunPhase, ScheduleSpec, ScheduledTask, WhenSpec,
};
pub use task::{BackgroundTaskStatus, BackgroundTaskView, TaskNotice};
pub use tool::{
    Clock, FileMeta, FileState, FileStateCache, FileSystem, FileView, InterruptBehavior,
    ProcessOutput, ProcessRunner, ProcessSpec, ProgressSink, PromptContext, ResultBudget, Tool,
    ToolContext, ToolOutcome, UiPayload, ValidationError,
};
pub use turn::{
    ApiProtocol, EndpointSampling, ImageInput, ModelEndpoint, Nudge, QueuedSummary, SandboxKind,
    TurnConfig, TurnInput, TurnLimits, VisionSetup, WebSetup,
};
pub use vision::{DescribeRequest, NoVision, VisionAccess, VisionError};
pub use web::{
    DistillRequest, NoWeb, SearchHit, SearchQuery, WebAccess, WebError, WebRequest, WebResponse,
};
