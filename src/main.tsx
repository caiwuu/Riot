import React from "react";
import ReactDOM from "react-dom/client";

import { App } from "./App";
import "./styles.css";

/* 侧栏的磨砂底来自窗口那层系统材质：macOS 是 NSVisualEffectView 的
   sidebar（tauri.conf.json 的 windowEffects），Windows 是 DWM 的 mica
   （配置表达不全，由宿主设，见 src-tauri/src/vibrancy.rs）。

   要透出材质，页面得先把自己的背景让开，而这只能在真有材质的平台干：
   Linux 那层不存在，背景透掉只会露出窗口底下的桌面。所以门控挂在 html
   上，配色规则跟着这个属性走（见 styles.css 的 [data-vibrancy]）。属性
   带值是因为两边的材质深浅不同，侧栏的不透明度要分开调。
   macOS 的判断跟顶栏给红绿灯让位那处保持一致（App.tsx 的 IS_MAC）。

   代价记在这里，因为 JSON 写不下注释：材质要求窗口透明，而 macOS 上
   透明窗口走的是 Tauri 的 `macOSPrivateApi`（私有 API）——用了它就上不了
   Mac App Store。Riot 走 Developer ID 签名 + DMG 自分发，这条路本来
   就没在计划里；哪天要上架，得连着这三处一起撤（配置、这里、CSS）。
   Windows 的 acrylic 不涉及私有 API。 */
const ua = navigator.userAgent;
if (ua.includes("Mac")) {
  document.documentElement.dataset.vibrancy = "mac";
} else if (ua.includes("Windows NT 10.0")) {
  // 材质要 Win11 22523+（DWMSBT），而 UA 里所有 Win10/11 都报 10.0，
  // 拿不到 build 号。老系统上 vibrancy.rs 会把逐像素 alpha 关掉，侧栏
  // 退化成纯色 —— 所以 Windows 的侧栏留得比 macOS 实一档，退化了也只是
  // 深一点，不至于破相。
  document.documentElement.dataset.vibrancy = "win";
}

const root = document.getElementById("root");
if (!root) throw new Error("#root 不存在");

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
