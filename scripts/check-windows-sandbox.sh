#!/usr/bin/env bash
# 在非 Windows 机器上检查沙箱的 Windows 代码。
#
# 为什么要这么绕：整包 `cargo check --target x86_64-pc-windows-msvc` 在 mac 上
# 跑不起来 —— reqwest → ring 要 C 交叉编译，`assert.h` 都找不到。但沙箱那几个
# 文件的依赖是纯 Rust + windows crate 的元数据（平台无关），隔离出一个只带这些
# 依赖的壳就能查。
#
# `[约束]` 查过 ≠ 跑得对。`SetTokenInformation` 到底生效没有、Low 进程是不是
# 真写不进未打标签的目录，只有 Windows CI 上的真机测试说了算。这个脚本挡的是
# 另一类错：FFI 签名写错、cfg(windows) 分支里的类型不匹配 —— 那些在 mac 上
# 改代码时**完全看不见**，一路推到 CI 才炸。
#
# 用法：scripts/check-windows-sandbox.sh [clippy|check]   默认 clippy
set -euo pipefail

CMD="${1:-clippy}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SHELL_DIR="${TMPDIR:-/tmp}/riot-winchk"
TARGET="x86_64-pc-windows-msvc"

if ! rustup target list --installed | grep -qx "$TARGET"; then
  echo "缺少目标 $TARGET，先跑：rustup target add $TARGET" >&2
  exit 1
fi

mkdir -p "$SHELL_DIR/src"

cat > "$SHELL_DIR/Cargo.toml" <<TOML
# 由 scripts/check-windows-sandbox.sh 生成，勿手改。
[package]
name = "riot-winchk"
version = "0.0.0"
edition = "2024"

[dependencies]
riot-protocol = { path = "$ROOT/crates/riot-protocol" }
async-trait = "0.1"
process-wrap = { version = "9", features = ["tokio1"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time", "fs", "io-util", "process"] }
tokio-util = { version = "0.7", features = ["rt"] }
tracing = "0.1"

[dependencies.windows]
version = "0.62"
features = [
  "Win32_System_Threading",
  "Win32_Security",
  "Win32_Security_Authorization",
  "Win32_System_SystemServices",
  "Win32_Foundation",
  "Win32_System_Memory",
  "Win32_System_Pipes",
  "Win32_System_JobObjects",
  "Win32_Storage_FileSystem",
]

[dev-dependencies]
tempfile = "3"

[workspace]
TOML

# 直接 include 真实源码（不是拷贝）—— 拷贝会漂移，而漂移的那一侧不报错。
{
  echo "//! 由 scripts/check-windows-sandbox.sh 生成，勿手改。"
  for m in sandbox sandbox_cmdline sandbox_labels sandbox_win proc; do
    echo "#[path = \"$ROOT/crates/riot-runtime/src/$m.rs\"]"
    echo "pub mod $m;"
  done
} > "$SHELL_DIR/src/lib.rs"

# 指纹按壳自己的 mtime 算，而源码在别处 —— 不清掉的话改完 sandbox_win.rs
# 再跑会直接报 "Finished"，一个字都没查。
rm -rf "$SHELL_DIR/target/$TARGET/debug/.fingerprint/riot-winchk-"*

cd "$SHELL_DIR"
exec cargo "$CMD" --target "$TARGET" --all-targets
