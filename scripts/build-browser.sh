#!/usr/bin/env bash
#
# 构建并打包 riot-browser（CEF 浏览器宿主子进程）。
#
# 为什么要单独一个脚本:
#
# 1. riot-browser 不在主 workspace 里（见根 Cargo.toml 的 exclude），
#    `cargo build --workspace` 碰不到它;
# 2. macOS 上 CEF **必须**从 .app 里启动 —— 资源经 [NSBundle mainBundle]
#    定位，裸二进制会卡在 `icudtl.dat not found in bundle`。所以这里没有
#    "debug 直接跑二进制"的选项，dev 和生产都得打包。
#
# 首次使用前要先把 CEF 二进制拉到本地:
#   cargo run -p export-cef-dir -- --force "$HOME/.local/share/cef"
# 那个命令在 https://github.com/tauri-apps/cef-rs 仓库里。
set -euo pipefail

CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../crates/riot-browser" && pwd)"
CEF_PATH="${CEF_PATH:-$HOME/.local/share/cef}"
OUT="${1:-$CRATE_DIR/target/bundle}"

if [[ ! -d "$CEF_PATH/Chromium Embedded Framework.framework" ]]; then
  echo "找不到 CEF: $CEF_PATH" >&2
  echo "先跑 cef-rs 仓库里的 export-cef-dir，或设 CEF_PATH 指向已有的副本。" >&2
  exit 1
fi

export CEF_PATH
export DYLD_FALLBACK_LIBRARY_PATH="${DYLD_FALLBACK_LIBRARY_PATH:-}:$CEF_PATH:$CEF_PATH/Chromium Embedded Framework.framework/Libraries"

cd "$CRATE_DIR"

cargo build --quiet

# bundle-cef-app 由 cef crate 提供，知道 macOS 上那五个 helper .app 各自的
# Info.plist 和命名规则 —— 手写这部分是纯粹的重复劳动，而且错了只会在运行时
# 以"renderer 起不来"的形式暴露。
#
# 它是依赖里的 bin，cargo 没法直接 run，只能先装。
if ! command -v bundle-cef-app >/dev/null 2>&1; then
  echo "缺 bundle-cef-app，正在安装（首次会编译 CEF 绑定，几分钟）..." >&2
  cargo install cef --version "^151.3" --bin bundle-cef-app --locked
fi

bundle-cef-app riot-browser \
  --output "$OUT" \
  --identifier "dev.riot.browser" \
  --display-name "Riot Browser"

echo "打包完成: $OUT/riot-browser.app"
