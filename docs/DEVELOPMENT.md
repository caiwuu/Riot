# 开发

从源码跑、测、打包、发版。架构约束见 [ARCHITECTURE.md](ARCHITECTURE.md)，验证分层见 [VERIFICATION.md](VERIFICATION.md)。

需要 Rust 1.88+、Node 22、pnpm 10。

## 跑起来

```bash
pnpm install
pnpm tauri dev
```

只看前端布局（宿主 bridge 不可用）：

```bash
pnpm dev
```

从 Dock 或访达启动的应用继承不到 shell 环境变量，这是 macOS 的行为。日常开发用 `pnpm tauri dev`；key 已经在设置里存过，不必再 `export`。环境变量与存档同名时优先于存档，用来临时覆盖。

想先在终端确认模型链路（GUI 会把 key 没读到、base URL 写错、模型名不存在都表现成「没反应」）：

```bash
export DEEPSEEK_API_KEY=sk-...
cargo run -p riot-providers --example smoke

MODEL=deepseek-reasoner cargo run -p riot-providers --example smoke

BASE_URL=https://api.moonshot.cn MODEL=kimi-k2-turbo-preview \
  KEY_ENV=MOONSHOT_API_KEY cargo run -p riot-providers --example smoke
```

## 日常命令

```bash
cargo check --workspace
cargo test --workspace          # 不变量断言只在 debug 下 panic，不要只跑 release
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt
pnpm typecheck

# 改了 riot-protocol 里的类型之后必须重新生成，CI 会检查是否同步
pnpm gen
```

`--workspace` 不包含 `riot-browser`（独立 Cargo workspace，避免把 CEF 编进日常构建）。

## 内置浏览器

```bash
# 将 CEF 拉到 $HOME/.local/share/cef（或设置 CEF_PATH）
# 二进制来自 https://github.com/tauri-apps/cef-rs 的 export-cef-dir
./scripts/build-browser.sh
```

macOS 上浏览器必须从 `.app` bundle 启动。`pnpm tauri dev` 已经在跑的话，打完包要重启一次，宿主只在启动时定位浏览器。Windows 用 `scripts/build-browser.ps1`，需要 VS 的「使用 C++ 的桌面开发」工作负载。

浏览器是可选能力：没打过包也能起主应用，只是 Browser 工具和面板不可用。

## 打包

```bash
# mac
./scripts/build-browser.sh
# win
powershell -ExecutionPolicy Bypass -File .\scripts\build-browser.ps1

pnpm tauri build
```

产物在 `src-tauri/target/release/bundle/`（`.app` / `.dmg` / NSIS）。

## 仓库分层

```
crates/          内核、工具、权限、模型适配、协议
src-tauri/       宿主：窗口、会话、终端、浏览器、权限弹窗
src/             React 界面；只有 src/bridge/ 可以调用 Tauri
schemas/         从 Rust 类型生成，勿手改
docs/            架构、验证、开发
```

依赖方向是 `protocol ← core ← kernel`。`core` 不得依赖 UI。前端不得在 `bridge/` 以外 `import @tauri-apps/api`。

## 发新版本

设置 → 关于里的「检查更新」比的是 Riot 应用本身的版本。改这三处，保持一致：

- `package.json` 的 `version`
- `Cargo.toml` 的 `[workspace.package].version`（各 crate 写 `version.workspace = true`，不用逐个改）
- `src-tauri/tauri.conf.json` 的 `version`

宿主从 `tauri.conf.json` 读这份号：关于页显示、对照 GitHub 最新正式 Release，都是它。Release tag 写成 `0.1.1`、`v0.1.1` 或 `Riot_0.1.1` 都行。

测试夹具里的 `0.1.0`（例如 `session.rs` 的 `doc_pack()`）不用改。文档能力包是另一条线，见 [ARCHITECTURE.md](ARCHITECTURE.md) §15.5。
