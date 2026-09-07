/**
 * 界面明暗主题：用户的选择、"跟随系统"的解析，以及把结果挂到 `<html>` 上。
 *
 * 和 `i18n/index.ts` 的语言选择同一个模式：模块级状态 + `useSyncExternalStore`
 * 订阅。选择存 localStorage（`riot.theme`，对齐 `riot.locale`）：它是纯界面
 * 偏好，不值得进宿主配置；网页版（远程访问）也因此能各自记住自己的主题。
 * 没存过就跟系统走。
 *
 * `[约束]` 配色由 CSS 按 `html[data-theme='light']` 切（styles.css 的 token
 * 覆盖块），CSS 里**不写** `prefers-color-scheme` —— "跟随系统"由这里解析成
 * `data-theme`。分开判断会对不上：桌面窗口里 webview 的 `prefers-color-scheme`
 * 报的是宿主钉住的外观，不是系统的（见 [`pushToHost`]），CSS 自己去问系统
 * 得到的答案和页面实际挂的属性可能是两个值。
 *
 * `[约束]` `data-theme` 永远是 `"light"` 或 `"dark"`，不留空、不写 `"system"`：
 * 样式表只认这两个值，缺省（没有属性）按深色处理。
 *
 * 首帧防闪：index.html 里有一段内联脚本先按同样的规则挂好 `data-theme` 和
 * 底色，本模块加载时只是接管后续同步。两处的规则必须一致 —— 那边改了这边
 * 也要改。
 */

import { useSyncExternalStore } from "react";

import { host } from "./bridge";

const LS_KEY = "riot.theme";

/** 解析后真正生效的主题。 */
export type Theme = "light" | "dark";

/** 用户的显式选择；`null` = 跟随系统。 */
export type ThemeChoice = Theme | null;

/** 两套正文底色。index.html 的防闪底色和 `<meta name="theme-color">` 用同一份。 */
const PAGE_BG: Record<Theme, string> = { light: "#ffffff", dark: "#181818" };

export function isTheme(v: unknown): v is Theme {
  return v === "light" || v === "dark";
}

function readChoice(): ThemeChoice {
  if (typeof localStorage === "undefined") return null;
  const v = localStorage.getItem(LS_KEY);
  return isTheme(v) ? v : null;
}

/**
 * 系统明暗的查询句柄。模块级只建一个：`change` 事件要长期监听，而这个对象
 * 的身份就是监听器挂着的地方。没有 `matchMedia` 的环境（组件测试）按深色。
 */
const darkQuery: MediaQueryList | null =
  typeof matchMedia === "function" ? matchMedia("(prefers-color-scheme: dark)") : null;

function systemTheme(): Theme {
  if (!darkQuery) return "dark";
  return darkQuery.matches ? "dark" : "light";
}

let choice: ThemeChoice = readChoice();
let current: Theme = choice ?? systemTheme();
const listeners = new Set<() => void>();

/** 把解析结果写到文档上：`<html data-theme>`，以及两个 meta。 */
function applyTheme() {
  if (typeof document === "undefined") return;
  document.documentElement.dataset.theme = current;
  // color-scheme 决定表单控件、滚动条这类引擎自绘部件的明暗；theme-color 是
  // 网页版在手机浏览器里的地址栏底色。两者都得跟着走，否则浅色页面配着
  // 深色的下拉箭头和地址栏。
  document.querySelector('meta[name="color-scheme"]')?.setAttribute("content", current);
  document.querySelector('meta[name="theme-color"]')?.setAttribute("content", PAGE_BG[current]);
}

/**
 * 让宿主把原生窗口（侧栏材质、标题栏、NSApp 外观）切到同一档。
 *
 * `[约束]` 选"跟随系统"时这一步必须**先于**读 `prefers-color-scheme`。只要
 * NSApp 外观被宿主钉成 DarkAqua，WKWebView 里的 `prefers-color-scheme` 就永远
 * 报 dark，和真正的系统设置无关；宿主解除钉死之后媒体查询才会翻到真值并
 * 触发 `change` —— 下面那个监听器接住它，链路才闭合。所以这里是"告诉宿主"，
 * 真正的解析交给事件。
 *
 * 失败只记日志：外观没换成是可见的、可重试的（再切一次），不值得弹错。
 */
function pushToHost() {
  if (!host.nativeWindow) return;
  host.setAppearance(choice ?? "system").catch((e: unknown) => {
    console.warn("Failed to apply native window appearance", e);
  });
}

/** 重新解析生效主题。变了就写到文档上并返回 true；通知由调用方做。 */
function resolve(): boolean {
  const next = choice ?? systemTheme();
  if (next === current) return false;
  current = next;
  applyTheme();
  return true;
}

function notifyAll() {
  for (const notify of listeners) notify();
}

// 模块加载即生效，不等 React：内联脚本已经挂过一次，这里是幂等的接管；
// 顺带把宿主对齐 —— 存的是"跟随系统"时，这一步就是启动时的解除钉死。
applyTheme();
pushToHost();

// 系统明暗变了：只在跟随系统时有意义。显式选了明 / 暗的用户，系统怎么切
// 都不关他的页面。
darkQuery?.addEventListener("change", () => {
  if (choice !== null) return;
  if (resolve()) notifyAll();
});

/** 此刻生效的主题。非组件代码（xterm 建实例、mermaid 配置）直接用。 */
export function getTheme(): Theme {
  return current;
}

/** 用户的显式选择（设置页显示用）。`null` = 跟随系统。 */
export function getThemeChoice(): ThemeChoice {
  return choice;
}

/**
 * 改主题。`null` 表示回到跟随系统。立即生效：订阅了 [`useTheme`] 的组件
 * 重渲染，不重挂载。
 *
 * 选择变了就通知，哪怕生效主题没变（系统就是深色时，"跟随系统"和"深色"
 * 来回切）：[`useThemeChoice`] 和 [`useTheme`] 共用一组订阅者，不通知的话
 * 设置页的下拉会停在旧项。
 *
 * 顺序：先告诉宿主，再解析。理由见 [`pushToHost`] —— 选"跟随系统"时此刻
 * 读到的媒体查询可能还是钉住的旧值，宿主解除钉死后 `change` 事件会补正。
 */
export function setThemeChoice(next: ThemeChoice): void {
  const choiceChanged = next !== choice;
  choice = next;
  if (typeof localStorage !== "undefined") {
    if (next) localStorage.setItem(LS_KEY, next);
    else localStorage.removeItem(LS_KEY);
  }
  if (choiceChanged) pushToHost();
  const themeChanged = resolve();
  if (choiceChanged || themeChanged) notifyAll();
}

function subscribe(notify: () => void): () => void {
  listeners.add(notify);
  return () => {
    listeners.delete(notify);
  };
}

/** 组件里订阅生效主题。凡是按主题挑配色的组件（xterm、mermaid）都要调。 */
export function useTheme(): Theme {
  return useSyncExternalStore(subscribe, getTheme, getTheme);
}

/** 主题的选择项（含"跟随系统"），给设置页。 */
export function useThemeChoice(): ThemeChoice {
  return useSyncExternalStore(subscribe, getThemeChoice, getThemeChoice);
}

/**
 * 非组件代码订阅生效主题（highlight.js 的那个 `<style>`）。只在主题真的
 * 变了才回调 —— 底层订阅者对"选择变了但主题没变"也会被通知，这里滤掉。
 * 返回退订函数。
 */
export function subscribeTheme(cb: (theme: Theme) => void): () => void {
  let seen = current;
  return subscribe(() => {
    if (current === seen) return;
    seen = current;
    cb(current);
  });
}
