#!/usr/bin/env bash
# 在非 Windows 机器上检查沙箱的 Windows 代码。
#
# `[约束]` 查过 ≠ 跑得对。`SetTokenInformation` 到底生效没有、ACE 写下去
# 沙箱账户是不是真能写那棵树，只有 Windows 上的真机测试说了算（srt-win 的
# 真机冒烟脚本见 vendor/srt-win/NOTICE.md）。这个脚本挡的是另一类错：FFI
# 签名写错、cfg(windows) 分支里的类型不匹配 —— 那些在 mac 上改代码时
# **完全看不见**，一路推到 CI 才炸。
#
# 用法：scripts/check-windows-sandbox.sh [clippy|check]   默认 clippy
#
# ── 为什么要造假编译器 ────────────────────────────────────────────
#
# 直接 `cargo check --target x86_64-pc-windows-msvc` 在 mac 上会死在
# `ring` 的 build script 上（`fatal error: 'assert.h' file not found`）：
# 它要为 Windows 交叉编译一段 C，而这台机器没有 Windows SDK。同样的墙
# rusqlite 的 `bundled` sqlite3 也会撞。
#
# 但 **`cargo check` 不链接**。那些 .o / .lib 从头到尾没人读，只是 cc-rs
# 要确认文件存在。所以把 CC/AR 换成两个「只 touch 出目标文件就返回 0」的
# 桩，整包检查就能跑起来 —— 查的是真实 crate、真实依赖图。
#
# 这里曾经用过另一个办法：现生成一个只带沙箱那几个依赖的壳 crate，把源码
# `#[path]` include 进去。那样能绕开 ring，但壳的依赖表是手抄的，和
# riot-runtime 真实的 Cargo.toml 会漂移，而漂移的那一侧不报错。
set -euo pipefail

CMD="${1:-clippy}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="x86_64-pc-windows-msvc"
STUB_DIR="${TMPDIR:-/tmp}/riot-winchk-stub"

if ! rustup target list --installed | grep -qx "$TARGET"; then
  echo "缺少目标 $TARGET，先跑：rustup target add $TARGET" >&2
  exit 1
fi

mkdir -p "$STUB_DIR"

# 假编译器。要认三种输出写法：`-o <path>`（GNU）、`-Fo<path>`（MSVC，
# cc-rs 对 msvc target 用这个，单参数）、`-E`（预处理探测，不产文件）。
cat > "$STUB_DIR/cc" <<'STUB'
#!/bin/sh
out=""; prev=""
for a in "$@"; do
  case "$a" in
    -Fo*) out="${a#-Fo}" ;;
    -E)   exit 0 ;;
  esac
  [ "$prev" = "-o" ] && out="$a"
  prev="$a"
done
if [ -n "$out" ]; then
  mkdir -p "$(dirname "$out")" 2>/dev/null
  : > "$out"
fi
exit 0
STUB

# 假归档器。MSVC 走 lib.exe（`-out:path`），GNU 走 ar（第一个非 flag 参数）。
cat > "$STUB_DIR/ar" <<'STUB'
#!/bin/sh
out=""
for a in "$@"; do
  case "$a" in
    -out:*|-OUT:*|/out:*|/OUT:*) out="${a#*:}" ;;
  esac
done
if [ -z "$out" ]; then
  for a in "$@"; do
    case "$a" in
      -*|/*) ;;
      *.a|*.lib) out="$a"; break ;;
    esac
  done
fi
if [ -n "$out" ]; then
  mkdir -p "$(dirname "$out")" 2>/dev/null
  : > "$out"
fi
exit 0
STUB

chmod +x "$STUB_DIR/cc" "$STUB_DIR/ar"

cd "$ROOT"

run() {
  env \
    "CC_${TARGET//-/_}=$STUB_DIR/cc" \
    "AR_${TARGET//-/_}=$STUB_DIR/ar" \
    cargo "$CMD" --target "$TARGET" "$@"
}

# 我们自己的 Windows 沙箱代码，连测试一起查。
run --all-targets -p riot-runtime

# vendored 的底层实现（见 vendor/srt-win/NOTICE.md）。**不带 --all-targets**：
# 它的单元测试里有一处 `include_bytes!("../../../test/fixtures/…")`，指向上游
# npm 仓库、不在 vendored 范围内。那是 `cert_store` 的测试，而 cert_store
# 是 TLS 终止用的、Riot 根本不调（NOTICE.md「只用了它的一半」）。
#
# 不为它把 fixture 也搬进来，是因为那些测试**在 mac 上本来就跑不了** —— 它们
# 要真实的 Win32 状态。真机验证走上游的 ci/*.ps1 冒烟脚本（CI 的
# win-sandbox-smoke job）。
#
# 单独点名那个集成测试：它不吃 fixture，而且 CI 的 host job 会真跑它 ——
# 两边查的目标要一致，否则本地过了推上去照样红。
run -p srt-win
run -p srt-win --test sd_access_check_matrix
