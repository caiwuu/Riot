//! Markdown 文件预览。解析跑在 Worker（见 lib/markdown.worker.ts 的
//! 头注释：主线程在大表格解析后的 JIT 降速窗口里跑不动重解析），
//! 主线程只贴 HTML。
//!
//! 链接点击不需要本组件操心：预览面板容器的 onBodyClick 把本地相对
//! 链接按"当前文件所在目录"解析成新预览标签，window 级兜底把外部
//! 链接转交系统浏览器 —— 纯 HTML 输出正好落在这两层防线里。

import { useEffect, useState } from "react";

/** 超过这个字节数截断渲染。整页 DOM 一次性挂几 MB 文本会卡渲染。 */
const RENDER_MAX = 1024 * 1024;

let worker: Worker | null = null;
let workerBroken = false;
let nextId = 1;
const pending = new Map<number, (html: string) => void>();

/** Worker 挂了（脚本加载失败 / 超时）就永久回退主线程，别反复撞墙。 */
function markWorkerBroken(reason: unknown) {
  if (workerBroken) return;
  workerBroken = true;
  console.warn("[markdown] Worker 不可用，回退主线程渲染", reason);
  worker?.terminate();
  worker = null;
  pending.clear();
}

/** Worker 常驻单例：md 预览是高频操作，每次开一个 Worker 不值得。 */
function renderViaWorker(text: string): Promise<string> {
  if (!worker) {
    worker = new Worker(new URL("../lib/markdown.worker.ts", import.meta.url), {
      type: "module",
    });
    worker.onmessage = (e: MessageEvent<{ id: number; html: string }>) => {
      pending.get(e.data.id)?.(e.data.html);
      pending.delete(e.data.id);
    };
    worker.onerror = (e) => markWorkerBroken(e.message || e);
  }
  const id = nextId++;
  return new Promise((resolve, reject) => {
    // 3 秒兜底：Worker 静默失败（模块加载被拦等）时不能让界面
    // 永远停在"正在渲染"。
    const timer = setTimeout(() => {
      if (pending.delete(id)) {
        markWorkerBroken("timeout");
        reject(new Error("worker timeout"));
      }
    }, 3000);
    pending.set(id, (html) => {
      clearTimeout(timer);
      resolve(html);
    });
    worker?.postMessage({ id, text });
  });
}

/** 主线程渲染。micromark 很轻（几十 KB、状态机解析），即使处在大表格
 *  解析后的 JIT 降速窗口里，也远好于 react-markdown 的全套管线。 */
async function renderOnMainThread(text: string): Promise<string> {
  const [{ micromark }, { gfm, gfmHtml }] = await Promise.all([
    import("micromark"),
    import("micromark-extension-gfm"),
  ]);
  try {
    return micromark(text, { extensions: [gfm()], htmlExtensions: [gfmHtml()] });
  } catch {
    const escaped = text
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;");
    return `<pre>${escaped}</pre>`;
  }
}

async function renderMarkdown(text: string): Promise<string> {
  if (!workerBroken) {
    try {
      return await renderViaWorker(text);
    } catch {
      /* 已标记 broken，走主线程 */
    }
  }
  return renderOnMainThread(text);
}

export default function MarkdownView({ buf, path }: { buf: ArrayBuffer; path: string }) {
  const [html, setHtml] = useState<string | null>(null);
  const [truncated, setTruncated] = useState(false);

  useEffect(() => {
    let stale = false;
    setHtml(null);
    const clipped = buf.byteLength > RENDER_MAX;
    setTruncated(clipped);
    const text = new TextDecoder("utf-8", { fatal: false }).decode(
      clipped ? buf.slice(0, RENDER_MAX) : buf,
    );
    void renderMarkdown(text).then((out) => {
      if (!stale) setHtml(out);
    });
    return () => {
      stale = true;
    };
  }, [buf, path]);

  return (
    <div className="markdown-doc">
      {truncated ? (
        <div className="code-view-note">文件太大，只显示前 1 MB。完整内容请用"系统应用打开"。</div>
      ) : null}
      {html === null ? (
        <div className="preview-panel-state">正在渲染…</div>
      ) : (
        // micromark 默认转义原始 HTML（allowDangerousHtml 未开），
        // 输出里不会有文件自带的 <script> 之类，可直接贴。
        <div className="md markdown-doc-body" dangerouslySetInnerHTML={{ __html: html }} />
      )}
    </div>
  );
}
