import {
  createContext,
  memo,
  startTransition,
  useContext,
  useEffect,
  useRef,
  useState,
} from "react";
import ReactMarkdown, { defaultUrlTransform } from "react-markdown";
import rehypeHighlight from "rehype-highlight";
import remarkGfm from "remark-gfm";

import { openInBrowser, openPath } from "../bridge";
import { useTimedFlag } from "../hooks/useTimedFlag";
import { AGENT_LINK_SCHEME, openSubagent } from "../lib/subagentLink";
import { joinRoot, looksAbsPath } from "../pathDisplay";
import { openFilePreview } from "./FilePreview";
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
  value?: string;
  lang?: string | null;
  url?: string;
  position?: { start?: { offset?: number } };
  data?: { hProperties?: Record<string, string> };
  children?: MdNode[];
};

/**
 * 全角标点：裸 URL 撞上它们就该收尾。真实 URL 里这些字符几乎不会
 * 裸出现（浏览器复制出来是百分号编码），出现即意味着"正文开始了"。
 * 全角字母数字和汉字不在内 —— `wiki/汉字` 这类路径是合法的。
 * U+3000（全角空格）也算：GFM 只认 ASCII 空白，它拦不住自动链接。
 */
const CJK_PUNCT =
  /[\u2018-\u201F\u2026\u3000-\u3002\u3008-\u3011\u3014-\u301F\uFF01-\uFF0F\uFF1A-\uFF20\uFF3B-\uFF40\uFF5B-\uFF65]/;

/**
 * 把 GFM 自动识别的裸链接在第一个全角标点处截断，截掉的尾巴还回正文。
 *
 * GFM 自动链接到空白才停，尾部修剪只认 ASCII 标点。中文语境里
 * `（https://linux.do/latest）。需要…` 会把 `）。需要…` 整段吞进
 * URL —— 链接点开是 404，满屏蓝字也没法读。
 *
 * 只动自动链接（文字与 URL 一字不差、起点重合）；`[文字](url)` 和
 * `<url>` 是作者明确划的边界，URL 里就算有全角字符也照单全收。
 */
function remarkCjkAutolinks() {
  return (tree: MdNode) => {
    walk(tree, (node) => {
      const kids = node.children;
      if (!kids) return;
      for (let i = 0; i < kids.length; i++) {
        const link = kids[i];
        if (!link || link.type !== "link" || link.children?.length !== 1) continue;
        const text = link.children[0];
        if (!text || text.type !== "text") continue;
        const value = text.value ?? "";
        const url = link.url ?? "";
        // 自动链接的特征：链接文字与 URL 一致（www 裸域名会被 GFM 补上
        // http:// 前缀），且节点起点与文字起点重合（`<url>` 的起点在 `<`，
        // `[文字](url)` 的起点在 `[`，都会错开）。
        if (url !== value && url !== `http://${value}`) continue;
        if (link.position?.start?.offset !== text.position?.start?.offset) continue;
        const m = CJK_PUNCT.exec(value);
        if (!m || m.index === 0) continue;
        const tail = value.slice(m.index);
        text.value = value.slice(0, m.index);
        link.url = url.slice(0, url.length - tail.length);
        kids.splice(i + 1, 0, { type: "text", value: tail });
      }
    });
  };
}

/** 软换行前后的空白：断行时要吃掉，否则新行开头挂着缩进。 */
const SOFT_BREAK = /[\t ]*\n[\t ]*/;

/**
 * 段内的单换行渲染成真断行（`<br>`），给思考过程用。
 *
 * markdown 的规矩是把连续的行并成一段，那是给"写出来的文章"定的。
 * 模型的思考是流水账，常拿单换行分句、列条目，并成整段就是一堵墙 ——
 * 比不渲染还难读。
 *
 * 做在 mdast 层而不是拿 CSS 的 `white-space: pre-line` 糊：换行是继承
 * 属性，而 mdast 转 HTML 时会在 `<li>`、`<blockquote>`、表格行里垫
 * 排版用的换行（松散列表是 `<li>⏎<p>…</p>⏎</li>`）。那些换行不该
 * 显示出来，pre-line 会把它们一并断掉，序号和正文当场错开两行。
 */
function remarkSoftBreaks() {
  return (tree: MdNode) => {
    walk(tree, (node) => {
      const kids = node.children;
      if (!kids) return;
      let split = false;
      const out: MdNode[] = [];
      for (const kid of kids) {
        // 只拆纯文本。代码块 / 行内代码是别的节点类型，天然不会进来。
        const parts = kid.type === "text" ? (kid.value ?? "").split(SOFT_BREAK) : [];
        if (parts.length < 2) {
          out.push(kid);
          continue;
        }
        split = true;
        parts.forEach((part, i) => {
          if (i > 0) out.push({ type: "break" });
          if (part) out.push({ ...kid, value: part });
        });
      }
      if (split) node.children = out;
    });
  };
}

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
export const Markdown = memo(function Markdown({
  text,
  breaks = false,
  animated = false,
}: {
  text: string;
  /** 段内单换行照原样断行。见 [`remarkSoftBreaks`]，只有思考过程要。 */
  breaks?: boolean;
  /**
   * 这段正文**正在流**：新长出来的块逐个淡入（样式见 styles.css 的
   * `.md[data-md-animated]`）。
   *
   * `[约束]` 只给正在流的那一条开。历史消息也开的话，切会话、懒水合
   * 落地、⌘F 全量水合都会让整屏重播一遍淡入。
   */
  animated?: boolean;
}) {
  return (
    <div className="md" {...(animated ? { "data-md-animated": "" } : {})}>
      <ReactMarkdown
        remarkPlugins={
          breaks
            ? [remarkGfm, remarkCjkAutolinks, remarkCodeRefs, remarkSoftBreaks]
            : [remarkGfm, remarkCjkAutolinks, remarkCodeRefs]
        }
        rehypePlugins={[[rehypeHighlight, { ignoreMissing: true, detect: false }]]}
        urlTransform={keepFileUrls}
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
 * 放行 `file:` 链接，其余交给默认的白名单（http/https/mailto 等）。
 *
 * react-markdown 的默认白名单会把 `file://` 的 href 清成空字符串 ——
 * 模型写的真实路径就这么丢了，[`MdLink`] 只能退回"拿链接文字当相对
 * 路径猜"，猜错就是无声无息。链接的打开全部由 [`MdLink`] 接管
 * （preventDefault），file: 不会真的交给 webview 导航，放行它不会
 * 打开 javascript: 那类注入面。
 */
function keepFileUrls(url: string): string {
  // `agent:` 是子 agent 链接（Task 工具让模型这么写），同样由 MdLink 接管。
  return /^(file|agent):/i.test(url) ? url : defaultUrlTransform(url);
}

/**
 * 视口外的正文先按纯文本占位，进视野再 parse。
 *
 * 切到长会话时上百条一起走 ReactMarkdown + highlight 会把主线程卡死，
 * 表现为白屏。贴底的那几条（`eager`）立刻渲染，用户看到的就是最新对话。
 */
export const LazyMarkdown = memo(function LazyMarkdown({
  text,
  eager = false,
}: {
  text: string;
  eager?: boolean;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [on, setOn] = useState(eager);

  useEffect(() => {
    if (eager) setOn(true);
  }, [eager]);

  useEffect(() => {
    if (on) return;
    const el = ref.current;
    if (!el) return;
    const root = el.closest(".transcript");
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          io.disconnect();
          startTransition(() => setOn(true));
        }
      },
      { root: root instanceof Element ? root : null, rootMargin: "1200px 0px" },
    );
    io.observe(el);
    return () => io.disconnect();
  }, [on]);

  if (on) return <Markdown text={text} />;
  return (
    <div ref={ref} className="md md-lazy">
      {text}
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
 * 本地路径（含模型误写成应用 origin 的假网址）在应用内预览 —— 模型交付
 * 的 docx / PDF 点开就能看，预览窗里有"系统应用打开"兜底；真正的
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
  const [err, flashErr] = useTimedFlag(false, 2000);
  const label = extractText(children);
  const target = resolveMdLink(href, label, root);

  if (!target) return <>{children}</>;

  const onClick = (e: React.MouseEvent<HTMLAnchorElement>) => {
    e.preventDefault();
    if (target.kind === "file") {
      openFilePreview(target.value);
      return;
    }
    if (target.kind === "agent") {
      openSubagent(target.value, label);
      return;
    }
    openInBrowser(target.value).catch(() => flashErr(true));
  };

  return (
    <>
      <a
        href={target.href}
        className={target.kind === "agent" ? "md-agent-link" : undefined}
        title={err ? "打不开" : target.title}
        onClick={onClick}
      >
        {target.kind === "agent" ? (
          <span className="md-agent-icon" aria-hidden>
            ⑂
          </span>
        ) : null}
        {children}
      </a>
      {err ? (
        <span className="md-link-err" role="status">
          打不开
        </span>
      ) : null}
    </>
  );
}

type MdLinkTarget = {
  kind: "url" | "file" | "agent";
  value: string;
  href: string;
  title: string;
};

/** 把模型写的 href 收成"打开网址"、"打开本地文件"或"打开子 agent 会话"。 */
function resolveMdLink(href: string | undefined, label: string, root: string): MdLinkTarget | null {
  const raw = (href ?? "").trim();

  // `agent:agt_xxx`：子 agent 的会话（Task 工具提示词里教模型这么写）。
  if (raw.toLowerCase().startsWith(AGENT_LINK_SCHEME)) {
    const id = raw.slice(AGENT_LINK_SCHEME.length).trim();
    if (!id) return null;
    return { kind: "agent", value: id, href: raw, title: `打开子 agent ${id} 的会话` };
  }

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

  // markdown 渲染成 href 时地址被 percent-encode 过（中文、空格都成了
  // %XX）。file:// 分支的 URL 解析自带解码；这条纯路径分支得自己解，
  // 否则中文文件名拼出来的路径永远不存在。链接文字是明文，不用解。
  const pathish = raw ? tryDecodePath(raw) : label.trim();
  if (!pathish || pathish.startsWith("#")) return null;
  return fileTarget(looksAbsPath(pathish) ? pathish : joinRoot(root, pathish));
}

/** 文件名里真有孤立 `%` 时 decodeURIComponent 会抛 —— 那就按原样用。 */
function tryDecodePath(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

function fileTarget(path: string): MdLinkTarget {
  return { kind: "file", value: path, href: toFileHref(path), title: path };
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
  const [copied, flashCopied] = useTimedFlag<"idle" | "ok" | "fail">("idle", 1500);
  const [refErr, flashRefErr] = useTimedFlag(false, 2000);
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
      () => flashCopied("ok"),
      () => flashCopied("fail"),
    );
  };

  // 不走 bridge 的 openInDefaultApp —— 它把失败吞掉了（那是给"静默降级"
  // 场景用的），这里要拿到失败才能在界面上说"打不开"。
  const openRef = () => {
    const full = refPath.startsWith("/") || !root ? refPath : `${root}/${refPath}`;
    openPath(full).catch(() => flashRefErr(true));
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
