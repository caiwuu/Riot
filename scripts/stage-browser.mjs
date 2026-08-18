// 发版前确认浏览器包已打好。
//
// macOS 上宿主在 Riot.app/Contents/Resources/riot-browser.app 找它，
// Windows 上在主 exe 旁边的 riot-browser\（见 locate_app）。平台各自的
// tauri.<platform>.conf.json 的 bundle.resources 负责拷进去，但 tauri
// 不会替你编 CEF —— 缺了就打出一份没有浏览器的包。
//
// 用法:先跑对应平台的 build-browser 脚本，再 pnpm tauri build。

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const isWin = process.platform === "win32";
const app = path.join(
  root,
  "crates/riot-browser/target/bundle",
  isWin ? "riot-browser" : "riot-browser.app",
);
const buildScript = isWin
  ? "powershell -ExecutionPolicy Bypass -File scripts/build-browser.ps1"
  : "./scripts/build-browser.sh";

if (!fs.existsSync(app)) {
  console.error(`找不到浏览器包: ${app}`);
  console.error(`先跑 ${buildScript}，再打包。`);
  process.exit(1);
}

console.log(`浏览器包已就位: ${app}`);
