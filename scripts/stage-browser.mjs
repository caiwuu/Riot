// 发版前确认 riot-browser.app 已打好。
//
// 宿主在 Riot.app/Contents/Resources/riot-browser.app 找它（见
// locate_app）。tauri.conf.json 的 bundle.resources 负责拷进去，
// 但 tauri 不会替你编 CEF —— 缺了就打出一份没有浏览器的包。
//
// 用法:先 ./scripts/build-browser.sh，再 pnpm tauri build。

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const app = path.join(root, "crates/riot-browser/target/bundle/riot-browser.app");

if (!fs.existsSync(app)) {
  console.error(`找不到浏览器包: ${app}`);
  console.error("先跑 ./scripts/build-browser.sh，再打包。");
  process.exit(1);
}

console.log(`浏览器包已就位: ${app}`);
