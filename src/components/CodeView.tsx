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

/**
 * 没有扩展名、但文件名本身说明了语言的那些（小写比对）。文件树里这类
 * 文件是常客；按扩展名认不出来就只能当纯文本，白白少了高亮。
 */
const NAME_LANG: Record<string, string> = {
  makefile: "makefile",
  gnumakefile: "makefile",
  dockerfile: "dockerfile",
  containerfile: "dockerfile",
  "cmakelists.txt": "cmake",
  ".bashrc": "bash",
  ".zshrc": "bash",
  ".profile": "bash",
  ".bash_profile": "bash",
  ".env": "ini",
  ".gitconfig": "ini",
  ".npmrc": "ini",
  ".editorconfig": "ini",
};

/** 选高亮语言：先看文件名，再看扩展名。都认不出返回 undefined（纯文本）。 */
function langFor(ext: string, name: string | undefined): string | undefined {
  if (name) {
    const byName = NAME_LANG[name.toLowerCase()];
    if (byName) return byName;
    // `.env.local`、`.env.production` 这类：按前缀归到 .env。
    if (/^\.env(\.|$)/i.test(name)) return "ini";
  }
  return EXT_LANG[ext];
}

/** 超过这个字节数不做高亮 —— hljs 对大文件是秒级卡顿。 */
const HIGHLIGHT_MAX = 512 * 1024;
/** 超过这个字节数截断显示。几 MB 的文本一次性进 DOM 会把渲染卡死。 */
const RENDER_MAX = 2 * 1024 * 1024;

/**
 * 行号栏的内容：`1\n2\n…`。一个文本节点而不是每行一个元素 —— 几万行
 * 的文件多出几万个节点，首屏和滚动都会顿。行数按换行符数 + 1 算，和
 * `<pre>` 实际渲染的行一一对应（结尾的换行会多出一个空行，两边一致）。
 */
function lineNumbers(text: string): string {
  let n = 1;
  for (let i = text.indexOf("\n"); i >= 0; i = text.indexOf("\n", i + 1)) n++;
  return Array.from({ length: n }, (_, i) => i + 1).join("\n");
}

/**
 * 算好的一份正文。
 *
 * `[约束]` 三样东西整包一起换，别拆回三个 state。文件被 agent 改了会
 * 原地重读（见 FilePreview 的 PreviewBody），而正文要等 hljs 那个
 * Promise，行号和截断提示是同步算出来的 —— 拆开就是"行号已经跳到新
 * 行数、正文还是旧的"那一帧。
 */
interface Body {
  /** 高亮后的 HTML。null = 认不出语言或文件太大，走 `text`。 */
  html: string | null;
  text: string;
  gutter: string;
  truncated: boolean;
}

export default function CodeView({
  buf,
  ext,
  name,
}: {
  buf: ArrayBuffer;
  /** 小写扩展名（可为空串）。 */
  ext: string;
  /** 文件名。Makefile / Dockerfile 这类靠它认语言。 */
  name?: string;
}) {
  const [body, setBody] = useState<Body | null>(null);

  useEffect(() => {
    let stale = false;
    // 不先清空 body。重读一遍磁盘就闪一下"正在高亮…"的话，内容整块
    // 消失再回来，滚动位置也跟着甩回顶部 —— 而用户多半正盯着某一段看
    // 模型改了什么。旧内容留到新的算好为止（文件树刷新同一个取舍）。
    // 首次挂载它本来就是 null，该有的加载态一样在。
    const truncated = buf.byteLength > RENDER_MAX;
    // fatal:false —— 非 UTF-8（老 GBK 文件、二进制误入）出替换字符，
    // 显示成乱码但不抛错；真要看的话头部按钮有系统应用兜底。
    const text = new TextDecoder("utf-8", { fatal: false }).decode(
      truncated ? buf.slice(0, RENDER_MAX) : buf,
    );
    const base = { text, gutter: lineNumbers(text), truncated };
    const lang = langFor(ext, name);
    if (!lang || buf.byteLength > HIGHLIGHT_MAX) {
      setBody({ ...base, html: null });
      return;
    }
    void import("highlight.js").then(
      ({ default: hljs }) => {
        if (stale) return;
        try {
          // hljs 会转义源文本，dangerouslySetInnerHTML 是安全的。
          setBody({
            ...base,
            html: hljs.highlight(text, { language: lang, ignoreIllegals: true }).value,
          });
        } catch {
          setBody({ ...base, html: null });
        }
      },
      () => {
        if (!stale) setBody({ ...base, html: null });
      },
    );
    return () => {
      stale = true;
    };
  }, [buf, ext, name]);

  return (
    <div className="code-view">
      {body?.truncated ? (
        <div className="code-view-note">文件太大，只显示前 2 MB。完整内容请用"系统应用打开"。</div>
      ) : null}
      {body ? (
        // 行号栏和正文并排、共用一个滚动容器；横向滚时行号钉在左缘
        // （sticky）。行号用 aria-hidden：读屏器念一串数字没有意义，
        // 选中复制正文时也不该把行号一起带走（user-select: none）。
        <div className="code-view-lines">
          <pre className="code-view-gutter" aria-hidden>
            {body.gutter}
          </pre>
          <pre>
            {body.html !== null ? (
              <code className="hljs" dangerouslySetInnerHTML={{ __html: body.html }} />
            ) : (
              <code>{body.text}</code>
            )}
          </pre>
        </div>
      ) : (
        <div className="preview-panel-state">正在高亮…</div>
      )}
    </div>
  );
}
