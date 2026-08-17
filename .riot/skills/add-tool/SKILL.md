---
name: add-tool
description: 给 Riot 加一个模型能调的工具时用。Tool trait 的必答项、fail-closed 默认值的判据、注册位置、以及几条写错了不报错只是行为悄悄降级的规则。
---

# 加一个工具

工具定义在 `crates/riot-protocol/src/tool.rs` 的 `Tool` trait，实现放
`crates/riot-tools/src/tools/<名字>.rs`。

## 1. 五个必须实现的

没有默认实现，漏一个编译不过：

| 方法 | 给谁看 | 注意 |
|---|---|---|
| `name` | API + 权限规则的键 | 返回 `&str` 而非 `&'static str`（MCP 工具的名字来自运行时） |
| `input_schema` | API | `schemars::schema_for!(Input)` |
| `prompt` | **模型** | 进 `tools[].description` 的完整说明。要写清和其它工具的分工、以及 NEVER 列表 |
| `describe` | **界面** | 一句话，如「读取 src/main.rs」 |
| `call` | — | 返回 `ToolOutcome`，**不是** `Result` |

`prompt` 和 `describe` 是两条独立的路，不要让其中一个凑合另一个。同理
`ToolOutcome::Ok` 里 `model_content`（模型看的）和 `ui_payload`（界面看的
**结构化数据**，不是渲染好的字符串）也是分开的。

## 2. `call` 里不许把错误抛出去

失败是正常返回值：`ToolOutcome::failed("...")`。函数内部可以自由用 `?`，
但必须在边界把 `Result` 转成 `Failed`。类型系统由此保证「工具错误不会抛穿
主循环」。

错误文本是**给模型的纠错指令**，用祈使句，不要贴原始错误：

```rust
// 好：告诉它下一步做什么
ToolOutcome::failed("`pattern` 不能为空。要列出文件请用 Glob。")
// 坏：把内部诊断甩给它
ToolOutcome::failed(format!("{e:?}"))
```

## 3. fail-closed 默认值：判据是什么

漏写不会造成危险行为，但**写错方向会**。逐条的判据：

- **`is_read_only`** —— 判据不是「感觉安不安全」，是**会不会执行任意代码
  或改变任何状态**。`Diagnostics` 跑 `cargo check`，而它会执行 `build.rs`
  和过程宏，所以它**不是**只读的。标错的后果是这个工具在所有权限模式下
  自动放行（`mode_default` 里只读一律 Allow）。
- **`is_concurrency_safe`** —— 按**输入**判定，不是静态标签。同一个 Bash，
  `ls -la` 可以并行，`rm -rf` 必须独占。参考 `bash.rs` 的
  `is_concurrency_safe(input) -> self.is_read_only(input)`。
- **`cascades_on_failure`** —— 默认 `false`，方向和别的 fail-closed 默认
  **相反**。这里「安全」不是少做事：级联会误杀无关工具，用户看到一串
  「已取消」却不知道为什么。只有 shell 类（命令间有隐式依赖）该返回 true。
- **`classifier_input`** —— 安全敏感的必须覆盖，否则 Auto 模式下这个工具
  永远得手动点。返回一句给小模型判危的文本。
- **`result_budget`** —— 默认 `Limit(50_000)`（超了落盘 + 预览）。
  **读类工具必须返回 `Unlimited`**，否则会出现「Read → 结果落盘成文件 →
  模型又去 Read 那个文件」的循环。自己在工具内部已经截断的也用 `Unlimited`。
- **`check_permissions`** —— 默认 `Passthrough`（交给通用决策链）。在这里
  返回 `Allow` 会**绕过**第 4 步的安全检查，参考 `grep.rs` 的注释：
  `Grep -l "BEGIN PRIVATE KEY" ~/.ssh` 也是一次读取。只有确实需要特化
  逻辑时才表态。

## 4. 参数校验分两层

- **结构**：`Input` 结构体加 `#[serde(deny_unknown_fields)]`，解析失败走
  一个 `schema_hint(&e)` 把 serde 的报错翻成人话（照抄 `bash.rs` /
  `grep.rs` 的写法：认出 `missing field` / `unknown field` / `unknown variant`
  分别给不同的指路）。
- **语义**：`validate_input` 用 `ValidationError::rejected("...")`。文件
  存在吗、已经 Read 过吗、参数组合有意义吗（例：`context_lines` 只在
  `content` 模式下有意义）。

正则这类东西**在本地先编译一遍**再交给下游 —— 让下游报错的话模型收到的是
一段内部诊断，而且白等一次执行。

## 5. 注册

内置工具在 `crates/riot-tools/src/tools/mod.rs` 的 `builtin()`。

`[约束]` **追加在末尾，不要往中间插。** 那个顺序就是发给模型的顺序，而它
进 prompt cache 的前缀 —— 往中间插会让所有活跃会话的缓存失效。

宿主层的工具（需要 `AppConfig`、会话状态的，如 `Task`）在
`src-tauri/src/session.rs` 每轮装配那一段加，不进 `builtin()`。

## 6. 测试

测试写在模块内的 `#[cfg(test)] mod tests`。要造 `ToolContext` 时，不碰
fs / proc / 网络的工具可以全给占位实现（`crate::testing::NullProc`、
`riot_protocol::web::NoWeb`、`NoBrowser`、`NoTerminal`、`NoVision`、
`crate::testing::FixedClock`）—— 照抄 `ask.rs` 或 `terminal.rs` 的 `ctx()`。

要断言的重点：

- **fail-closed 的方向**。认不出的输入落在保守那一侧（例：未知的
  `subagent_type` 要算「会写」而不是「只读」）。
- **失败的理由对不对**，不只是「失败了」。仓库里有过真实教训：变异之后
  工具仍然拒绝，只是理由从「你还没读过」变成「文件内容对不上」——
  这个区别对模型是实打实的。
- **不完整的结果要说出来**。截断、超时、提前收工都必须进 `model_content`，
  否则模型拿半份结果当全部，不报错不崩，只是结论错。

需要注入时钟才能测的（超时、TTL、轮询）用 `FixedClock`，它的 `sleep_ms`
会推进时间而不真睡。

## 7. 收尾

```bash
cargo test -p riot-tools
cargo clippy -p riot-tools --all-targets -- -D warnings
```

工具涉及权限判定、路径围栏、或子进程时，跟着跑 `mutate` 技能。
