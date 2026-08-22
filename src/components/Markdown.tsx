import { createContext, memo, useContext, useState } from "react";
import ReactMarkdown from "react-markdown";
import rehypeHighlight from "rehype-highlight";
import remarkGfm from "remark-gfm";

import { openInBrowser, openPath } from "../bridge";
import { MermaidBlock } from "./Mermaid";

import "highlight.js/styles/github-dark-dimmed.css";

/**
 * 当前会话的项目根。代码引用要靠它把模型写的相对路径拼成能打开的绝对路径。
 *
 * 用 context 而不是逐层传 prop：`Markdown` 有四个调用点，其中两个在
 * `memo` 过的 `Row` 里 —— 加一个 prop 就要把 root 一路穿过去，而它在整个
 * 会话里是个不变的字符串。
 */
export const ProjectRootContext = createContext<string>("");

/**
 * 代码引用的 info string：`起始行:结束行:路径`。
 *
 * 模型引用**仓库里已有的**代码时用这个格式（提示词里有约定），渲染成一个
 * 带路径标题、点一下能打开文件的代码块。新写的代码走普通语言标签，
 * 两者要看得出区别 —— 前者是"去看这里"，后者是"这是我建议加的"。
 */
const CODE_REF = /^(\d+):(\d+):(.+)$/;

/**
 * 扩展名 → highlight.js 的语言名。
 *
 * 代码引用的 info string 位置被路径占了，highlight.js 认不出来会整块不高亮。
 * 认不出的扩展名返回 undefined，退化成纯文本 —— 那只是不好看，不是坏了。
 */
const EXT_LANG: Record<string, string> = {
  rs: "rust",
  ts: "typescript",
  tsx: "tsx",
  js: "javascript",
  jsx: "jsx",
  mjs: "javascript",
  json: "json",
  toml: "ini",
  md: "markdown",
  css: "css",
  html: "xml",
  py: "python",
  sh: "bash",
  bash: "bash",
  zsh: "bash",
  yml: "yaml",
  yaml: "yaml",
  sql: "sql",
};

type MdNode = {
  type: string;
  lang?: string | null;
  data?: { hProperties?: Record<string, string> };
  children?: MdNode[];
};

/**
 * 把 `12:14:src/foo.rs` 这样的 info string 拆成 data 属性，并把 lang 换成
 * 真语言，让后面的 rehypeHighlight 照常工作。
 *
 * 做在 remark 这一层（mdast）而不是渲染时：`code` 节点的 `data.hProperties`
 * 会被 mdast-util-to-hast 应用到外层的 `<pre>` 上，正好是 [`CodeBlock`]
 * 接到的那个元素。放到渲染时再拆的话，语言已经错过高亮那一步了。
 */
function remarkCodeRefs() {
  return (tree: MdNode) => {
    walk(tree, (node) => {
      if (node.type !== "code" || !node.lang) return;
      const m = CODE_REF.exec(node.lang);
      if (!m) return;
      const [, start, end, path] = m;
      if (!start || !end || !path) return;
      node.data = {
        ...node.data,
        hProperties: {
          ...node.data?.hProperties,
          "data-ref-path": path,
          "data-ref-start": start,
          "data-ref-end": end,
        },
      };
      const ext = path.split(".").pop()?.toLowerCase() ?? "";
      node.lang = EXT_LANG[ext] ?? null;
    });
  };
}

function walk(node: MdNode, visit: (n: MdNode) => void) {
  visit(node);
  for (const child of node.children ?? []) walk(child, visit);
}

/**
 * 助手输出的 Markdown 渲染。
 *
 * `[约束]` memo 是性能要求，不是锦上添花。流式输出时 transcript 每帧
 * 重渲染，没有 memo 的话每条历史消息都要重新 parse 一遍 markdown ——
 * 长对话里打字机效果会肉眼可见地掉帧。有 memo 后只有正在流式的那一段
 * 反复 parse，它通常只有几百字。
 *
 * 用户消息**不走这里**：用户输入的 `# 标题` 就是字面上的井号标题，
 * 按 markdown 渲染等于篡改他说的话。
 */
export const Markdown = memo(function Markdown({ text }: { text: string }) {
  return (
    <div className="md">
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkCodeRefs]}
        rehypePlugins={[[rehypeHighlight, { ignoreMissing: true, detect: false }]]}
        components={{
          pre: CodeBlock,
          a: MdLink,
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
});

/**
 * Markdown 链接。
 *
 * 不能把 href 原样丢给 `<a>`：相对路径和空地址会被浏览器解析成
 * `http://localhost:1420/…`（开发时的 Vite 页，打包后是 webview 自己的
 * origin）。点下去不是打开文件，而是把应用导航走 —— 而且没有后退按钮。
 *
 * 本地路径（含模型误写成应用 origin 的假网址）走系统默认应用；真正的
 * http(s) 走系统浏览器。
 */
function MdLink({
  href,
  children,
}: {
  href?: string | undefined;
  children?: React.ReactNode;
}) {
  const root = useContext(ProjectRootContext);
  const [err, setErr] = useState(false);
  const label = extractText(children);
  const target = resolveMdLink(href, label, root);

  if (!target) return <>{children}</>;

  const onClick = (e: React.MouseEvent<HTMLAnchorElement>) => {
    e.preventDefault();
    const go =
      target.kind === "url" ? openInBrowser(target.value) : openPath(target.value);
    go.catch(() => {
      setErr(true);
      setTimeout(() => setErr(false), 2000);
    });
  };

  return (
    <a href={target.href} title={err ? "打不开" : target.title} onClick={onClick}>
      {children}
    </a>
  );
}

type MdLinkTarget = {
  kind: "url" | "file";
  value: string;
  href: string;
  title: string;
};

/** 把模型写的 href 收成"打开网址"或"打开本地文件"。 */
function resolveMdLink(href: string | undefined, label: string, root: string): MdLinkTarget | null {
  const raw = (href ?? "").trim();

  if (raw.startsWith("file://")) {
    const path = fileUrlToPath(raw);
    return path ? fileTarget(path) : null;
  }

  if (/^https?:\/\//i.test(raw)) {
    try {
      const u = new URL(raw);
      if (u.origin === window.location.origin) {
        const rel = decodeURIComponent(u.pathname).replace(/^\/+/, "");
        const name = rel || label.trim();
        if (!name) return null;
        return fileTarget(looksAbsPath(name) ? name : joinRoot(root, name));
      }
      return { kind: "url", value: u.href, href: u.href, title: u.href };
    } catch {
      return null;
    }
  }

  if (/^[a-z][a-z0-9+.-]*:/i.test(raw)) {
    return { kind: "url", value: raw, href: raw, title: raw };
  }

  const pathish = raw || label.trim();
  if (!pathish || pathish.startsWith("#")) return null;
  return fileTarget(looksAbsPath(pathish) ? pathish : joinRoot(root, pathish));
}

function fileTarget(path: string): MdLinkTarget {
  return { kind: "file", value: path, href: toFileHref(path), title: path };
}

function looksAbsPath(s: string): boolean {
  return s.startsWith("/") || s.startsWith("\\\\") || /^[A-Za-z]:[\\/]/.test(s);
}

function joinRoot(root: string, rel: string): string {
  const cleaned = rel.replace(/^\.[\\/]+/, "");
  if (!root) return cleaned;
  const sep = root.includes("\\") ? "\\" : "/";
  return `${root.replace(/[\\/]+$/, "")}${sep}${cleaned.replace(/[\\/]+/g, sep)}`;
}

function toFileHref(absPath: string): string {
  const unified = absPath.replace(/\\/g, "/");
  if (/^[A-Za-z]:/.test(unified)) return `file:///${unified}`;
  if (unified.startsWith("/")) return `file://${unified}`;
  return `file:///${unified}`;
}

function fileUrlToPath(url: string): string | null {
  try {
    let p = decodeURIComponent(new URL(url).pathname);
    if (/^\/[A-Za-z]:/.test(p)) p = p.slice(1);
    return p || null;
  } catch {
    return null;
  }
}

/** 代码块：语言标签（或代码引用的路径）+ 复制按钮。 */
function CodeBlock(props: React.HTMLAttributes<HTMLPreElement>) {
  const [copied, setCopied] = useState<"idle" | "ok" | "fail">("idle");
  const [refErr, setRefErr] = useState(false);
  const root = useContext(ProjectRootContext);

  const child = props.children as React.ReactElement<{
    className?: string;
    children?: React.ReactNode;
  }> | null;
  const lang =
    child?.props?.className
      ?.split(" ")
      .find((c) => c.startsWith("language-"))
      ?.slice("language-".length) ?? "";

  // remarkCodeRefs 把引用信息挂成 data 属性（见那个插件的说明），
  // 而 React 的元素 props 类型里没有它们。
  const attrs = props as Record<string, unknown>;
  const str = (k: string) => (typeof attrs[k] === "string" ? (attrs[k] as string) : "");
  const refPath = str("data-ref-path");
  const refStart = str("data-ref-start");
  const refEnd = str("data-ref-end");

  const copy = () => {
    const text = extractText(child?.props?.children);
    // 剪贴板在窗口失焦等情况下会拒绝 —— 失败还报"已复制"等于骗人
    navigator.clipboard.writeText(text).then(
      () => setCopied("ok"),
      () => setCopied("fail"),
    );
    setTimeout(() => setCopied("idle"), 1500);
  };

  // 不走 bridge 的 openInDefaultApp —— 它把失败吞掉了（那是给"静默降级"
  // 场景用的），这里要拿到失败才能在界面上说"打不开"。
  const openRef = () => {
    const full = refPath.startsWith("/") || !root ? refPath : `${root}/${refPath}`;
    openPath(full).catch(() => {
      setRefErr(true);
      setTimeout(() => setRefErr(false), 2000);
    });
  };

  const lines = refStart === refEnd ? `${refStart}` : `${refStart}-${refEnd}`;
  const source = extractText(child?.props?.children);

  if (lang === "mermaid") {
    return <MermaidBlock source={source} />;
  }

  return (
    <div className={refPath ? "codeblock is-ref" : "codeblock"}>
      <div className="codeblock-bar">
        {refPath ? (
          <>
            <button
              type="button"
              className="codeblock-ref"
              // 相对路径以项目根为基准。模型偶尔会写绝对路径，那时直接用。
              onClick={openRef}
              title={`用默认应用打开 ${refPath}`}
            >
              <span className="codeblock-ref-path">{refPath}</span>
              <span className="codeblock-ref-lines">:{lines}</span>
            </button>
            {refErr ? (
              <span className="codeblock-ref-err" role="status">
                打不开
              </span>
            ) : null}
          </>
        ) : (
          <span className="codeblock-lang">{lang}</span>
        )}
        <button type="button" className="codeblock-copy" onClick={copy}>
          {copied === "ok" ? "已复制" : copied === "fail" ? "复制失败" : "复制"}
        </button>
      </div>
      <pre {...props} />
    </div>
  );
}

/** 从 React 节点树里抠出纯文本。高亮后的代码是嵌套 span，得递归。 */
function extractText(node: React.ReactNode): string {
  if (node == null || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(extractText).join("");
  if (typeof node === "object" && "props" in node) {
    return extractText((node.props as { children?: React.ReactNode }).children);
  }
  return "";
}
