# Riot

本地跑的 AI coding agent 桌面端。内核是 Rust，界面是 Tauri + React。

模型走你自己配的 API（OpenAI 兼容或 Anthropic）。工具在本机执行；写文件、跑命令会走权限确认，默认先问再做。

现在能对着真实模型日常用。内核是独立进程（`riot-kernel` 二进制），宿主经 stdio 上的 JSON-RPC 驱动它——agent 崩溃或阻塞不再拖垮窗口。终端/浏览器工具的跨进程反向 RPC 还在路上（面板功能不受影响）。桌面端以 macOS 为主；CI 在 Linux 跑内核测试，在 macOS / Windows 跑宿主。Linux 桌面不是首发目标。

## 要求

- Rust 1.88+（见根目录 `Cargo.toml` 的 `rust-version`）
- Node 22、pnpm 10（和 CI 一致）
- 一个能用的模型 API key

## 跑起来

```bash
pnpm install
pnpm tauri dev
```

第一次会让你选一个项目目录。API key 在设置里贴，保存后进同目录的 `auth.json`（权限 0600），不进 `config.json`。环境变量同名时优先于存档，用来临时覆盖。

从 Dock 或访达启动的应用继承不到 shell 的环境变量，这是 macOS 的行为。开发时用 `pnpm tauri dev`；key 已经在设置里存过就不用再 `export`。

想先在终端确认链路（GUI 会把 key 没读到、base URL 写错、模型名不存在都藏成「没反应」）：

```bash
export DEEPSEEK_API_KEY=sk-...
cargo run -p riot-providers --example smoke

# 换模型（smoke 仍走 OpenAI 兼容报文）
MODEL=deepseek-reasoner cargo run -p riot-providers --example smoke

BASE_URL=https://api.moonshot.cn MODEL=kimi-k2-turbo-preview \
  KEY_ENV=MOONSHOT_API_KEY cargo run -p riot-providers --example smoke
```

设置里 `provider` 选协议：`anthropic` 走 Messages，`openai` 走 Chat Completions。DeepSeek、Kimi、Qwen、OpenRouter、vLLM、Ollama 都是后者。

只看前端布局（bridge 调用会失败）：

```bash
pnpm dev
```

浏览器是可选能力，见下面「浏览器」。没打过包主应用也能起，只是 Browser* 工具和面板不可用。

## 配置与扩展

配置目录在设置 → 关于里能看到。macOS 默认是 `~/Library/Application Support/riot`。

| 能力 | 全局 | 项目级 |
| --- | --- | --- |
| 记忆 | `<配置目录>/AGENTS.md` | `<项目>/AGENTS.md`（没有则读 `CLAUDE.md`） |
| Skills | `<配置目录>/skills/<名>/SKILL.md` | `<项目>/.riot/skills/` |
| 斜杠命令 | `<配置目录>/commands/*.md` | `<项目>/.riot/commands/` |
| Hooks | `<配置目录>/hooks.json` | `<项目>/.riot/hooks.json`（两层都跑） |
| MCP | 设置 → MCP | — |

改完下一轮对话生效，不用重启。

- 记忆注入首条消息，支持 `@路径` 引用。
- Skills 是渐进披露：清单进上下文，正文用到才读。
- 斜杠命令是提示词模板。输入框敲 `/`。`$ARGUMENTS` 是整段参数，`$1`–`$9` 是第 N 个（引号内算一个）。子目录是命名空间：`commands/git/pr.md` → `/git:pr`。内置 `/compact` 手动压缩历史。
- 输入框敲 `@` 引用文件，发送时带上内容。单个文件 24 KB、一条消息合计 64 KB，超出截断并告诉模型自己读。
- MCP 走 stdio，工具和内置工具同一套权限管线。

Hooks 对齐 Claude Code 的四个检查点：`PreToolUse`、`PostToolUse`、`Stop`、`UserPromptSubmit`。脚本从 stdin 收一行事件 JSON。**exit 2 = 拦下**，stderr 是给模型的理由；exit 0 的 stdout 可作为补充上下文。别的退出码只记日志。hook 的 `allow` 压不过安全检查和你自己写的 ask 规则。

## 仓库

```
crates/
  riot-protocol/     宿主、内核、前端共享的契约。依赖图的叶子
  riot-core/         主循环、状态机
  riot-tools/        工具实现
  riot-providers/    模型适配
  riot-permissions/  权限决策、Bash AST 分析
  riot-store/        会话持久化（append-only JSONL）
  riot-mcp/          MCP 客户端
  riot-runtime/      文件系统、进程、网络的可注入实现
  riot-kernel/       独立内核进程入口（尚未启用）
  riot-browser/      CEF 离屏浏览器子进程（独立 workspace，不进主构建）
src-tauri/           Tauri 宿主：窗口、会话、浏览器、终端、权限
src/                 React 前端
  bridge/            唯一允许调用 Tauri API 的地方
schemas/             从 Rust 类型生成，勿手改
docs/
```

两条依赖方向：

- `protocol ← core ← kernel`。`core` 不得依赖 UI。
- 前端只能通过 `src/bridge/` 访问宿主，别处不要 `import @tauri-apps/api`。

宿主和内核共享 `riot-protocol`，对不上直接编译失败。JSON Schema 是给 TypeScript 用的。

## 开发

日常迭代。`--workspace` 碰不到 `riot-browser`，那一套在下面单独写。

```bash
cargo check --workspace                    # 比 build 快，类型对不对就够
cargo check -p riot-host --all-targets     # 只动宿主时；不带 --all-targets 测试代码不参与编译
cargo test --workspace                     # 不变量断言只在 debug 下 panic，不要只跑 release
cargo test -p riot-host                    # 宿主（含进程生命周期）
cargo clippy --workspace --all-targets -- -D warnings
cargo lint                                 # 同上，.cargo/config.toml 里的别名
cargo fmt
cargo fmt --all --check                    # CI 用这个
pnpm typecheck

# 改了 riot-protocol 里的类型之后必须重新生成，CI 会检查是否同步
pnpm gen
```

分层验证、黄金回放、变异测试见 [docs/VERIFICATION.md](docs/VERIFICATION.md)。

### 浏览器

CEF 子进程单独编、单独打成 `.app`。macOS 上必须从 bundle 启动，裸二进制会卡在 `icudtl.dat`。宿主启动时在 `crates/riot-browser/target/bundle/riot-browser.app` 找它，找不到就装 `NoBrowser`，聊天照常。

首次要把 CEF 二进制拉到本地（`CEF_PATH` 默认 `$HOME/.local/share/cef`）：

```bash
# 在 https://github.com/tauri-apps/cef-rs 仓库里
cargo run -p export-cef-dir -- --force "$HOME/.local/share/cef"
```

```bash
./scripts/build-browser.sh                 # 编 + 打成 .app
# 产物：crates/riot-browser/target/bundle/riot-browser.app
```

`pnpm tauri dev` 已经在跑的话，打完要重启一次 —— 宿主只在启动时定位浏览器。

```bash
cargo test -p riot-host --test browser_e2e # 没打包会跳过，不是失败
```

### 清理

两套 `target`，互不影响。

```bash
cargo clean                                # 主 workspace（target/）
cargo clean --manifest-path crates/riot-browser/Cargo.toml
```

对 `riot-browser` 跑 `cargo clean` 会把已经打好的 `.app` 一起删掉。清完 Browser* 起不来，再跑一遍 `./scripts/build-browser.sh`，然后重启 `pnpm tauri dev`。只想重编、还要留着包，不要对它 clean。

### 打包

```bash
./scripts/build-browser.sh                 # 浏览器（开发、发版都要，没有 debug 裸跑这条路）
pnpm tauri build                           # 桌面端 .app / .dmg / NSIS
```

主应用产物在 `src-tauri/target/release/bundle/`。开发时宿主读的是 crate 下的 bundle；发版时它会在 `Riot.app/Contents/Resources/riot-browser.app` 再找一次。

编译慢的话，装 `sccache` 和 `lld`，然后打开 `.cargo/config.toml` 里对应注释。

## 文档

| 文档 | 内容 |
| --- | --- |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | 进程、主循环、工具、权限、上下文。标了 `[约束]` 的是硬性要求 |
| [docs/VERIFICATION.md](docs/VERIFICATION.md) | 验证分层、回放、故障注入、变异测试 |

这个仓库按「架构写在文档里、正确性靠断言」组织。内核里不能直接调 `SystemTime::now()`、`std::fs`、`std::process`，一律走注入的 trait，否则黄金回放不成立。细节以那两份文档为准，不要只看这份 README。

## 许可

MIT。见各 crate 的 `Cargo.toml`。
