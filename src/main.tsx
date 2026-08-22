import React from "react";
import ReactDOM from "react-dom/client";

import { App } from "./App";
import "./styles.css";

/* 侧栏的磨砂底来自窗口那层 NSVisualEffectView（tauri.conf.json 的
   windowEffects: sidebar）。要透出它，页面得先把自己的背景让开 ——
   而这只能在 macOS 干：别的平台没有那层材质，背景透掉只会露出
   webview 的白底。所以门控挂在 html 上，配色规则跟着这个属性走
   （见 styles.css 的 [data-vibrancy]）。
   平台判断跟顶栏给红绿灯让位那处保持一致（App.tsx 的 padTraffic）。

   代价记在这里，因为 JSON 写不下注释：材质要求窗口透明，而 macOS 上
   透明窗口走的是 Tauri 的 `macOSPrivateApi`（私有 API）——用了它就上不了
   Mac App Store。Riot 走 Developer ID 签名 + DMG 自分发，这条路本来
   就没在计划里；哪天要上架，得连着这三处一起撤（配置、这里、CSS）。 */
if (navigator.userAgent.includes("Mac")) {
  document.documentElement.dataset.vibrancy = "";
}

const root = document.getElementById("root");
if (!root) throw new Error("#root 不存在");

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
