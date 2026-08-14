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

# [约束] 先删掉旧 bundle，不要原地覆盖。
#
# 原地覆盖一个正在被加载的 Mach-O 会把 dyld 卡死:进程停在 _dyld_start，
# 状态 UE（不可中断地退出中），SIGKILL 也收不掉。更糟的是它会传染 ——
# 之后每一次启动这个二进制都卡在同一个地方，表现是"浏览器起不来"，而
# stdout/stderr 一个字都没有（main 都没进）。清掉重建换的是新 inode，
# 老进程各自抱着老文件慢慢死，不影响新的。
rm -rf "$OUT/riot-browser.app"

bundle-cef-app riot-browser \
  --output "$OUT" \
  --identifier "dev.riot.browser" \
  --display-name "Riot Browser"

# 不要在 Dock、启动台和 ⌘-Tab 里露脸。
#
# [约束] 这一步 bundle-cef-app 不管:它只给 CEF 的五个 helper 加了
# LSUIElement，主 bundle 按普通应用打。而这个进程恰恰也没有窗口 ——
# 它是纯离屏渲染的，画面通过 stdio 交给主应用。不加的话 Dock 里会多出一个
# riot-browser 图标，用户只开了 Riot 却看到两个东西，点那个图标还什么都不弹。
#
# 走 plist 而不是运行时改 activation policy:LaunchServices 在进程加载前就
# 读它，图标一次都不会出现；而且这正是 Chromium 自家 helper 用的办法，
# 和 CEF 的兼容性有现成的证据。
plutil -replace LSUIElement -bool true \
  "$OUT/riot-browser.app/Contents/Info.plist"

echo "打包完成: $OUT/riot-browser.app"
