# srt-win（vendored）

Windows 沙箱的底层实现，从 Anthropic 的 `sandbox-runtime` 原样搬过来。

| | |
|---|---|
| 上游 | https://github.com/anthropic-experimental/sandbox-runtime |
| 路径 | `vendor/srt-win-src/` |
| commit | `e5fb1b93ba61bab8e916bee7541860bbdaa612cf`（2026-08-25）|
| 许可 | Apache-2.0（见同目录 `LICENSE`）|

Riot 本体是 PolyForm Noncommercial 1.0.0（见仓库根 `LICENSE`），
这个目录是 Apache-2.0。第三方代码仍走原许可，但**分发时这份 LICENSE
必须跟着走**，别在打包脚本里把 `vendor/` 过滤掉。

## 为什么是搬而不是依赖

上游 `publish = false`，不在 crates.io 上。而它恰好是为嵌入设计的：整个库
`#![cfg(windows)]`，在 macOS/Linux 上编译成空 crate，对非 Windows 构建零
影响；`lib.rs` 还导出了 `run_from_args` 和 `SRT_WIN_DISPATCH_ARG1`，明确
支持「链接进别人的二进制、按 `argv[1]` 分发」。

对照过的另一个候选是 OpenAI 的 `codex-windows-sandbox`（架构几乎相同）。
它走不通：依赖 `codex-otel`，而后者拖着 OpenTelemetry 全家桶 + reqwest +
tokio-tungstenite + `codex-api` + `codex-protocol`。为一个沙箱往 Riot 里塞
一个 WebSocket 客户端不合理，剥依赖又等于永久维护一个 20k 行的分支。

## 改了什么

**只改了 `Cargo.toml`**（删 `[profile.release]`、加注释，理由写在文件里）。
`src/`、`tests/`、`ci/`、`rustfmt.toml` 与上游逐字节相同。

`[约束]` 保持这个状态。同步上游 = 整个目录重拷一遍 + 重放 `Cargo.toml` 那
两处改动。一旦开始改 `src/`，11,315 行的 rebase 成本会立刻超过「不需要的
功能就不调用」。

## 只用了它的一半

上游的完整方案是「文件系统隔离 + WFP 出网栅栏」，两半都要。Riot **只用文件
系统那一半**：

- 用：专用本地账户、附加 ACE、Medium IL 受限令牌、job object、独立桌面
- 不用：`wfp` 子命令、`cert_store`（MITM CA，为 TLS 终止服务）

原因是 WFP 栅栏会拦掉沙箱账户的**全部**外连，只放行 loopback 上的代理端口
段——它假设调用方跑着 HTTP/SOCKS5 代理。Riot 没有代理层，也刻意保留
`allow_network: true`（见 `riot_runtime::sandbox` 的取舍）。整套搬过来的后果
是沙箱内彻底断网，`npm install` / `cargo build` 全死。

`Wfp` / `Acl` / `Exec` / `User` / `Install` 在它的 CLI 里是分开的子命令，
`exec` 路径上没有 WFP 检查，所以这个子集是成立的——**通过不调用来裁剪，
不通过改代码来裁剪**。

## 上游的已知限制（会原样继承）

- **alpha**。上游自己这么标的。
- **需要一次提权安装**：建本地用户账户、写 `HKLM\SOFTWARE\sandbox-runtime`
  下的 DPAPI 凭证。
- **够不到 per-user 安装的工具**。沙箱进程以专用账户身份跑，所以 nvm/fnm 管
  的 Node、per-user 的 Scoop/winget 包、`pip install --user`、
  `%LOCALAPPDATA%\Programs\…` 在 PATH 上解析得到但打不开。出路是改用机器级
  安装，或把具体路径加进 allowRead 授权。
- **DNS 不受栅栏管**（我们不用 WFP，这条无所谓）。

## 怎么验证

`ci/*.ps1` 是上游的真机冒烟脚本，都接一个 `srt-win.exe` 路径参数。这个
vendored crate 保留了 `[[bin]] srt-win`，所以：

```powershell
cargo build -p srt-win --release
pwsh vendor/srt-win/ci/smoke.ps1 target\release\srt-win.exe
pwsh vendor/srt-win/ci/cleanup.ps1 target\release\srt-win.exe   # 收尾
```

`[约束]` 这些脚本会**真的**建账户、装 WFP 过滤器、改 ACL。在开发机上跑完
一定要跑 `cleanup.ps1`。
