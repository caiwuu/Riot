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
   Windows 的 acrylic 不涉及私有 API。

   材质明暗跟系统外观走。应用只有一套深色，所以宿主把窗口钉成 Dark
   （tauri.conf.json 的 theme，以及 src-tauri/src/vibrancy.rs），浅色
   系统上侧栏才不会翻成浅灰。 */
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

/* macOS 系统 overlay 滚动条在深色底上偏亮，又不能用 ::-webkit-scrollbar
   改颜色（一写就把 overlay 打成占位槽）。藏掉原生条，滚动时自己画一条
   更暗的滑块，叠在内容上、不占宽度。 */
function installMacOverlayScrollbar() {
  if (document.documentElement.dataset.vibrancy !== "mac") return;
  document.querySelector(".riot-osb")?.remove();

  const thumb = document.createElement("div");
  thumb.className = "riot-osb";
  thumb.setAttribute("aria-hidden", "true");
  document.body.appendChild(thumb);

  let hide = 0;

  /* 滑块位置只在 scroll 事件里算一次。拖抽屉 / 拖终端高度时容器在
     变形，右缘持续移动却不再来 scroll 事件 —— 滑块会滞留在旧右缘，
     看起来像一条拖影。观察显示中的容器，尺寸一变立即藏：变形期间
     滚动指示本来就没意义。 */
  let watched: HTMLElement | null = null;
  let baseW = -1;
  let baseH = -1;
  const ro = new ResizeObserver((entries) => {
    const rect = entries[entries.length - 1]?.contentRect;
    if (!rect) return;
    if (baseW < 0) {
      // observe() 的首次回调是基准，不算"变形"。
      baseW = rect.width;
      baseH = rect.height;
      return;
    }
    if (rect.width !== baseW || rect.height !== baseH) {
      baseW = rect.width;
      baseH = rect.height;
      thumb.classList.remove("show");
    }
  });

  const show = (el: EventTarget | null) => {
    if (!(el instanceof HTMLElement)) return;
    if (el === document.documentElement || el === document.body) return;
    // 拖分隔条时聊天区贴底逻辑会程序化滚动、连发 scroll 事件，而容器
    // 右缘正在移动 —— 滑块跟不上就是一条拖影。拖动期间整个静默。
    if (document.querySelector(".rz.dragging")) return;
    const overflowY = getComputedStyle(el).overflowY;
    if (overflowY !== "auto" && overflowY !== "scroll" && overflowY !== "overlay") {
      return;
    }
    const { scrollTop, scrollHeight, clientHeight } = el;
    if (scrollHeight - clientHeight < 2) {
      thumb.classList.remove("show");
      return;
    }
    if (watched !== el) {
      if (watched) ro.unobserve(watched);
      watched = el;
      baseW = -1;
      baseH = -1;
      ro.observe(el);
    }
    const r = el.getBoundingClientRect();
    const pad = 3;
    const track = Math.max(0, r.height - pad * 2);
    const h = Math.min(track, Math.max(18, (clientHeight / scrollHeight) * track));
    const top = r.top + pad + (scrollTop / (scrollHeight - clientHeight)) * (track - h);
    thumb.style.height = `${h}px`;
    thumb.style.transform = `translate(${Math.round(r.right - 7)}px, ${Math.round(top)}px)`;
    thumb.classList.add("show");
    window.clearTimeout(hide);
    hide = window.setTimeout(() => thumb.classList.remove("show"), 680);
  };

  document.addEventListener("scroll", (e) => show(e.target), {
    capture: true,
    passive: true,
  });
}

installMacOverlayScrollbar();

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
