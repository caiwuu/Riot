import { memo, startTransition, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import { readImage } from "../bridge";
import type { Item } from "../hooks/useSession";
import { Chevron } from "./Chevron";
import { useEscLayer } from "./Modal";
import { SmoothFold } from "./SmoothFold";

type Tool = Extract<Item, { kind: "tool" }>;

/** TodoWrite 输入里的一项。宽松解析 —— 界面拿到什么画什么。 */
interface TodoInput {
  content?: string;
  status?: string;
  activeForm?: string;
}

function todosOf(t: Tool): TodoInput[] {
  const i = t.input as Record<string, unknown>;
  return Array.isArray(i?.todos) ? (i.todos as TodoInput[]) : [];
}

/**
 * 一次工具调用。
 *
 * 默认折叠。展开的话，一次 `cargo build` 的输出就会把整个对话冲走 ——
 * 用户来这里是看模型在干什么，不是读日志。摘要行说清"做了什么、成没成"，
 * 想看细节再点开。
 *
 * memo：流式输出时 transcript 每帧重渲染，历史工具卡片不该跟着刷。
 */
export const ToolCard = memo(function ToolCard({
  tool,
  eager = false,
}: {
  tool: Tool;
  /**
   * 贴底的那几条立刻画详情。其余的等进视野再挂 —— Edit/Write 默认
   * 展开，长会话里几十份 diff 一起进 DOM 会把切回卡成白屏。
   */
  eager?: boolean;
}) {
  const [userToggle, setUserToggle] = useState<boolean | null>(null);
  const cardRef = useRef<HTMLDivElement>(null);
  const [near, setNear] = useState(eager);
  // 图片结果（截图、读图）默认展开：截图的意义就是给人看，藏在"展开"
  // 后面的话用户不知道图已经在这里了，会转头让模型"把图贴出来"。
  // Edit / Write 也默认展开：用户要看的就是改了什么、写了什么。
  // 文本结果维持默认折叠 —— 一次 cargo build 的输出会把对话冲走。
  //
  // TodoWrite 刻意**不**默认展开：进度由输入框上方的常驻面板就地更新，
  // 对话流里的每次调用只是历史快照 —— 都展开的话，一个十步任务会在
  // 对话里铺十张几乎相同的清单。摘要行说清"几之几、正在干什么"，
  // 点开看的是"那一刻清单长什么样"。
  const open =
    userToggle ??
    (Boolean(tool.resultImage || tool.resultImagePath) ||
      tool.name === "Edit" ||
      tool.name === "Write");
  const detail = hasDetail(tool);
  const summary = summarize(tool);

  useEffect(() => {
    if (near) return;
    const el = cardRef.current;
    if (!el) return;
    const root = el.closest(".transcript");
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          io.disconnect();
          startTransition(() => setNear(true));
        }
      },
      { root: root instanceof Element ? root : null, rootMargin: "800px 0px" },
    );
    io.observe(el);
    return () => io.disconnect();
  }, [near]);

  useEffect(() => {
    if (eager) setNear(true);
  }, [eager]);

  // 摘要行被 CSS 截成一行，title 兜底让全文悬停可见 —— 长 Bash 命令
  // 在展开区还有完整版（见 renderDetail）。
  const head = (
    <>
      {/* 运行中转起来 —— 静止的 ◐ 和卡死看起来一模一样 */}
      <span className={tool.status === "running" ? "tool-icon tool-icon-spin" : "tool-icon"}>
        {icon(tool.status)}
      </span>
      <span className="tool-name">{tool.name}</span>
      {/* 失败不能只靠 12px 图标变红 —— 扫视时根本发现不了 */}
      {tool.status === "error" ? <span className="tool-fail">失败</span> : null}
      <span className="tool-summary" title={summary}>
        {summary}
      </span>
    </>
  );

  return (
    <div ref={cardRef} className={`tool tool-${tool.status}`}>
      {detail ? (
        <button
          className="tool-head"
          onClick={() => {
            setNear(true);
            setUserToggle(!open);
          }}
          type="button"
          aria-expanded={open}
        >
          {head}
          <Chevron open={open} />
        </button>
      ) : (
        // 没有详情就别渲染成按钮 —— 可点但点了毫无反应，比不可点更糟
        <div className="tool-head">{head}</div>
      )}

      {detail ? (
        <SmoothFold open={open && near}>
          <div className="tool-detail">{renderDetail(tool)}</div>
        </SmoothFold>
      ) : null}
    </div>
  );
});

function icon(s: Tool["status"]): string {
  if (s === "running") return "◐";
  if (s === "ok") return "✓";
  return "✕";
}

/** 一行说清这次调用在做什么。参数原样 dump 没人看得下去。
 *  导出给过程组的直播头复用 —— 组头滚的就是这句。 */
export function summarize(t: Tool): string {
  const i = t.input as Record<string, unknown>;
  const str = (k: string) => (typeof i?.[k] === "string" ? (i[k] as string) : "");
  const num = (k: string) => (typeof i?.[k] === "number" ? (i[k] as number) : 0);
  // 点击/输入的三种定位（ref/selector/text）挑给出的那个显示。
  const target = (textKey = "text") => {
    if (typeof i?.ref === "number") return `[${i.ref as number}]`;
    if (str("selector")) return `\`${str("selector")}\``;
    if (str(textKey)) return `“${str(textKey)}”`;
    return "元素";
  };

  switch (t.name) {
    case "Task": {
      const kind = str("subagent_type") || "general-purpose";
      return `${kind === "explore" ? "侦察" : "执行"} · ${str("description") || "子任务"}`;
    }
    case "TodoWrite": {
      const todos = todosOf(t);
      const done = todos.filter((x) => x.status === "completed").length;
      const doing = todos.find((x) => x.status === "in_progress");
      return `${done}/${todos.length} 完成${doing?.activeForm ? ` · ${doing.activeForm}` : ""}`;
    }
    case "Bash":
      return str("command");
    // 计划正文由下面的草稿卡/批准卡承担。不写这条的话会落到 default，
    // 把整份计划 dump 进摘要行。
    case "ExitPlanMode":
      return "撰写计划";
    case "AskUserQuestion":
      return str("question") || "提问";
    case "Read":
    case "Write":
    case "Edit":
    case "PreviewFile":
      return short(str("path") || str("file_path"));
    case "Grep":
      return `${str("pattern")}${str("path") ? ` 在 ${short(str("path"))}` : ""}`;
    case "BrowserNavigate":
      return short(str("url"), 80);
    case "BrowserClick": {
      const verb = i?.double === true ? "双击" : i?.right === true ? "右键" : "点击";
      return `${verb} ${target()}`;
    }
    case "BrowserType":
      return `在 ${target("target_text")} 输入 ${clip(str("text"), 40)}${i?.submit === true ? " ⏎" : ""}`;
    case "BrowserKey":
      return `按 ${str("key")}`;
    case "BrowserScroll": {
      const d = num("delta_y");
      return d < 0 ? `向上 ${Math.round(-d)}px` : `向下 ${Math.round(d)}px`;
    }
    case "BrowserHover":
      return `悬停 ${target()}`;
    case "BrowserSelect":
      return `${target()} 选 ${str("value")}`;
    case "BrowserDrag":
      return "拖拽元素";
    case "BrowserWaitFor": {
      if (str("selector")) return `等 \`${str("selector")}\` 出现`;
      if (str("selector_gone")) return `等 \`${str("selector_gone")}\` 消失`;
      if (str("text")) return `等文本 “${str("text")}”`;
      if (str("url_contains")) return `等地址含 “${str("url_contains")}”`;
      if (i?.network_idle === true) return "等网络空闲";
      return "等待条件";
    }
    case "BrowserGo":
      return { back: "后退", forward: "前进", reload: "刷新" }[str("direction")] ?? "历史导航";
    case "BrowserTabs":
      return `标签页: ${str("action") || "list"}`;
    case "BrowserEvaluate":
      return `执行 JS: ${clip(str("expression"), 60)}`;
    case "BrowserCookies":
      return "读 Cookie";
    case "BrowserNetwork":
      return `抓包: ${str("action") || "list"}${str("filter") ? ` (${str("filter")})` : ""}`;
    case "BrowserReplay":
      return `重放 ${str("method") || "GET"} ${short(str("url"), 60)}`;
    case "BrowserIntercept":
      return `拦截: ${str("action")}${str("url_pattern") ? ` \`${str("url_pattern")}\`` : ""}`;
    case "BrowserSecrets":
      return "扫描密钥泄露";
    case "BrowserDiscover":
      return "枚举表单/链接";
    case "BrowserFuzz":
      return `fuzz ${short(str("url"), 60)}`;
    case "BrowserUpload": {
      const n = Array.isArray(i?.paths) ? (i.paths as unknown[]).length : 0;
      return `上传 ${n} 个文件`;
    }
    case "BrowserCrawl":
      return `爬取 ${short(str("url"), 60)}`;
    case "BrowserReport": {
      const n = Array.isArray(i?.findings) ? (i.findings as unknown[]).length : 0;
      return `生成渗透报告（${n} 条发现）`;
    }
    default:
      return clip(
        Object.entries(i ?? {})
          .map(([k, v]) => `${k}=${typeof v === "string" ? v : JSON.stringify(v)}`)
          .join(" "),
        120,
      );
  }
}

/** 超长才截，截了要看得出来 —— 没有省略号的硬截像话说了一半。 */
function clip(s: string, max: number): string {
  return s.length <= max ? s : `${s.slice(0, max - 1)}…`;
}

/** 有没有值得展开的内容。只读字段，不建 React 树。 */
function hasDetail(t: Tool): boolean {
  if (t.name === "TodoWrite") return todosOf(t).length > 0;
  if (t.resultImage || t.resultImagePath) return true;
  if (t.output.length > 0) return true;
  if (t.status !== "running" && t.result) return true;
  const i = t.input as Record<string, unknown>;
  if (t.name === "Bash" && typeof i.command === "string" && i.command) return true;
  if (t.name === "Edit" && (i.old_string || i.new_string)) return true;
  if (t.name === "Write" && typeof i.content === "string" && i.content) return true;
  return false;
}

/**
 * 展开后的内容。按工具语义渲染，不是 JSON dump：
 * Edit 给 diff，Write 给内容预览，Bash 给实时输出或结果。
 */
function renderDetail(t: Tool): React.ReactNode {
  const i = t.input as Record<string, unknown>;
  const str = (k: string) => (typeof i?.[k] === "string" ? (i[k] as string) : "");

  // 任务清单：从 tool_use 的输入渲染（清单在输入里；结果只是一句固定
  // 确认，显示它反而是噪音）。
  if (t.name === "TodoWrite") {
    const todos = todosOf(t);
    if (todos.length === 0) return null;
    return (
      <ul className="todo-list">
        {todos.map((x, n) => (
          <li key={n} className={`todo-item ${x.status ?? "pending"}`}>
            <span className="todo-mark" aria-hidden>
              {x.status === "completed" ? "✓" : x.status === "in_progress" ? "◐" : "○"}
            </span>
            <span className="todo-text">
              {x.status === "in_progress" ? (x.activeForm ?? x.content) : x.content}
            </span>
          </li>
        ))}
      </ul>
    );
  }

  const parts: React.ReactNode[] = [];

  // 长命令的摘要行被截断，全文在这里 —— 审计的核心信息不能在界面上无处可看。
  if (t.name === "Bash") {
    const cmd = str("command");
    if (cmd) {
      parts.push(
        <pre key="cmd" className="tool-body tool-cmd">
          {cmd}
        </pre>,
      );
    }
  }

  if (t.name === "Edit") {
    const oldS = str("old_string");
    const newS = str("new_string");
    if (oldS || newS) {
      parts.push(
        <pre key="diff" className="tool-body tool-diff">
          {oldS.split("\n").map((l, n) => (
            <div key={`o${n}`} className="del">
              - {l}
            </div>
          ))}
          {newS.split("\n").map((l, n) => (
            <div key={`n${n}`} className="add">
              + {l}
            </div>
          ))}
        </pre>,
      );
    }
  } else if (t.name === "Write") {
    const content = str("content");
    if (content) {
      const lines = content.split("\n");
      // 写的过程中跟着尾巴走：定在开头的话，写到第 100 行时画面已经
      // 十几秒没动过了，和卡住一样。落定之后回到开头 —— 那时用户要
      // 确认的是"写了个什么东西"，不是逐行审阅。
      const live = t.status === "running";
      const from = live ? Math.max(0, lines.length - 30) : 0;
      parts.push(
        <pre key="w" className="tool-body">
          {from > 0 ? `… 前 ${from} 行\n` : ""}
          {lines.slice(from, from + 30).join("\n")}
          {!live && lines.length > 30 ? `\n… 共 ${lines.length} 行` : ""}
        </pre>,
      );
    }
  }

  // 结果里的图（截图、读图）贴出来 —— 这就是用户点开想看的东西
  if (t.resultImage || t.resultImagePath) {
    parts.push(
      <ShotImage
        key="img"
        alt={`${t.name} 结果图`}
        {...(t.resultImagePath !== undefined ? { path: t.resultImagePath } : {})}
        {...(t.resultImage !== undefined ? { fallback: t.resultImage } : {})}
      />,
    );
  }

  // 运行中显示实时输出；结束后最终结果更权威（实时行只是进度侧影）
  const live = t.output.length > 0 ? t.output.join("\n") : "";
  const result = t.status === "running" ? live : t.result || live;
  if (result) {
    parts.push(
      <pre key="r" className="tool-body">
        {result}
      </pre>,
    );
  }

  return parts.length ? parts : null;
}

/**
 * 工具结果里的图。
 *
 * 消息里只带压缩图（给模型的那份，看布局够、看文字糊），原图在磁盘上 ——
 * 先显示压缩图占住位置，按路径把原图读回来后无缝换上。原图读不回来
 * （被清理、超上限）就一直用压缩图，不报错:图能看就行。
 *
 * 整页截图是极端长图，卡片里按容器宽显示、限高纵向滚；点击开查看器
 * 看大图。
 */
function ShotImage({
  path,
  fallback,
  // 每张结果图都叫"工具结果图片"的话，读屏用户分不清哪张是哪次调用的
  alt = "工具结果图片",
}: {
  path?: string;
  fallback?: string;
  alt?: string;
}) {
  const [src, setSrc] = useState<string | undefined>(fallback);
  const [viewer, setViewer] = useState(false);

  useEffect(() => {
    if (!path) return;
    let alive = true;
    readImage(path)
      .then((img) => {
        if (alive) setSrc(`data:${img.mediaType};base64,${img.data}`);
      })
      .catch(() => {
        // 原图没了就用压缩图，什么都没有才隐藏。
      });
    return () => {
      alive = false;
    };
  }, [path]);

  if (!src) return null;
  return (
    <>
      {/* button 包一层：键盘可达 + 读屏知道可点开大图，不再是"只能鼠标点" */}
      <button
        type="button"
        className="tool-shot-wrap"
        onClick={() => setViewer(true)}
        aria-label={`放大查看：${alt}`}
      >
        <img className="tool-shot" src={src} alt={alt} />
      </button>
      {viewer ? <ShotViewer src={src} alt={alt} onClose={() => setViewer(false)} /> : null}
    </>
  );
}

/**
 * 图片查看器:全屏遮罩，图在视口里上下左右居中；超大的缩到视口内。
 *
 * portal 到 body —— 卡片在带 overflow 的滚动容器里，fixed 遮罩留在原地
 * 会被裁掉。导出给聊天区图片和磁盘图片文件的放大查看共用。
 */
export function ShotViewer({
  src,
  alt = "工具结果图片",
  onClose,
}: {
  src: string;
  alt?: string;
  onClose: () => void;
}) {
  // Esc 走公共栈 —— 查看器开在权限卡之上时，Esc 只关查看器，
  // 不会顺手把底下的权限请求也拒了。
  useEscLayer(onClose);

  return createPortal(
    // 点空白处（遮罩本身）关闭；点图不关，方便拖滚动条。
    <div
      className="shot-viewer"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <button className="shot-viewer-close" onClick={onClose} type="button" aria-label="关闭">
        <svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true">
          <path
            d="M2 2l8 8M10 2L2 10"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
          />
        </svg>
      </button>
      <img src={src} alt={`${alt}（原图）`} />
    </div>,
    document.body,
  );
}

/** 长路径留尾部 —— 文件名比目录前缀有用得多。 */
function short(p: string, max = 48): string {
  return p.length <= max ? p : `…${p.slice(-(max - 1))}`;
}
