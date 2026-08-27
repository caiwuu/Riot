//! Markdown → HTML，跑在 Worker 线程。
//!
//! 为什么不在主线程解析：大表格（MB 级 CSV/XLSX）解析会占满 WKWebView
//! 里 JSC 的进程级 JIT 配额，之后主线程 JS 降级到解释器，react-markdown
//! 这类重解析在降速窗口里能冻住界面约 1 秒。挪进 Worker 后主线程只做
//! 原生 HTML 贴入（浏览器 C++ 解析，不受 JS 降速影响），切换零冻结。
//!
//! micromark 是 react-markdown 的底层解析器：状态机、无回溯型正则，
//! 默认转义原始 HTML（allowDangerousHtml 不开），输出可直接贴。

import { micromark } from "micromark";
import { gfm, gfmHtml } from "micromark-extension-gfm";

self.onmessage = (e: MessageEvent<{ id: number; text: string }>) => {
  const { id, text } = e.data;
  let html: string;
  try {
    html = micromark(text, {
      extensions: [gfm()],
      htmlExtensions: [gfmHtml()],
    });
  } catch (err) {
    // 解析器抛错时退回转义后的原文 —— 内容还能看，只是没排版。
    const escaped = text
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;");
    html = `<pre>${escaped}</pre>`;
    console.warn("[markdown.worker] 解析失败，按纯文本显示", err);
  }
  self.postMessage({ id, html });
};
