//! 代码 / 纯文本预览。
//!
//! 不走 @file-viewer 的 text 管线：它是一套浅色皮，摆在 Riot 的深色壳里
//! 突兀，代码高亮的配色也和聊天里的代码块不一致。聊天代码块用的是
//! highlight.js + github-dark-dimmed（Markdown.tsx 全局引入了主题 css），
//! 这里用同一套 —— 颜色、字体和对话里看到的代码完全一致。
//!
//! highlight.js 全量包不小，动态 import：只有真的打开代码文件才加载。

import { useEffect, useState } from "react";

/**
 * 扩展名 → highlight.js 语言名。认不出的按纯文本渲染 —— 不好看，
 * 但不会坏。清单和 Markdown.tsx 的 EXT_LANG 同源，这里为"文件预览"
 * 场景补了更多后缀。
 */
const EXT_LANG: Record<string, string> = {
  ts: "typescript",
  tsx: "typescript",
  js: "javascript",
  jsx: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  json: "json",
  jsonc: "json",
  json5: "json",
  py: "python",
  rs: "rust",
  go: "go",
  java: "java",
  c: "c",
  h: "c",
  cpp: "cpp",
  cc: "cpp",
  hpp: "cpp",
  cs: "csharp",
  php: "php",
  rb: "ruby",
  swift: "swift",
  kt: "kotlin",
  sql: "sql",
  sh: "bash",
  bash: "bash",
  zsh: "bash",
  yaml: "yaml",
  yml: "yaml",
  toml: "ini",
  ini: "ini",
  xml: "xml",
  html: "xml",
  htm: "xml",
  vue: "xml",
  svelte: "xml",
  css: "css",
  scss: "scss",
  less: "less",
  diff: "diff",
  patch: "diff",
};

/** 能进代码预览的扩展名：有高亮映射的 + 纯文本两种。 */
export const CODE_EXTS = new Set([...Object.keys(EXT_LANG), "txt", "log"]);

/** 超过这个字节数不做高亮 —— hljs 对大文件是秒级卡顿。 */
const HIGHLIGHT_MAX = 512 * 1024;
/** 超过这个字节数截断显示。几 MB 的文本一次性进 DOM 会把渲染卡死。 */
const RENDER_MAX = 2 * 1024 * 1024;

export default function CodeView({ buf, ext }: { buf: ArrayBuffer; ext: string }) {
  /** 高亮后的 HTML；null = 走纯文本或还没高亮完。 */
  const [html, setHtml] = useState<string | null>(null);
  const [plain, setPlain] = useState<string | null>(null);
  const [truncated, setTruncated] = useState(false);

  useEffect(() => {
    let stale = false;
    setHtml(null);
    setPlain(null);
    const clipped = buf.byteLength > RENDER_MAX;
    setTruncated(clipped);
    // fatal:false —— 非 UTF-8（老 GBK 文件、二进制误入）出替换字符，
    // 显示成乱码但不抛错；真要看的话头部按钮有系统应用兜底。
    const text = new TextDecoder("utf-8", { fatal: false }).decode(
      clipped ? buf.slice(0, RENDER_MAX) : buf,
    );
    const lang = EXT_LANG[ext];
    if (!lang || buf.byteLength > HIGHLIGHT_MAX) {
      setPlain(text);
      return;
    }
    void import("highlight.js").then(
      ({ default: hljs }) => {
        if (stale) return;
        try {
          // hljs 会转义源文本，dangerouslySetInnerHTML 是安全的。
          setHtml(hljs.highlight(text, { language: lang, ignoreIllegals: true }).value);
        } catch {
          setPlain(text);
        }
      },
      () => {
        if (!stale) setPlain(text);
      },
    );
    return () => {
      stale = true;
    };
  }, [buf, ext]);

  return (
    <div className="code-view">
      {truncated ? (
        <div className="code-view-note">文件太大，只显示前 2 MB。完整内容请用"系统应用打开"。</div>
      ) : null}
      {html !== null ? (
        <pre>
          <code className="hljs" dangerouslySetInnerHTML={{ __html: html }} />
        </pre>
      ) : plain !== null ? (
        <pre>
          <code>{plain}</code>
        </pre>
      ) : (
        <div className="preview-panel-state">正在高亮…</div>
      )}
    </div>
  );
}
