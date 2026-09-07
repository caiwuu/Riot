/**
 * Vite 的模块声明（`?inline` / `?raw` / `?url` 这类资源后缀的类型）。
 *
 * 只有 Markdown.tsx 把 highlight.js 的两套主题 CSS 以 `?inline` 引成字符串用到
 * 它；纯副作用的 `import "x.css"` 不需要（tsc 不检查副作用 import 的解析）。
 */
/// <reference types="vite/client" />
