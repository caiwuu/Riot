//! 应用内文件预览：Office / PDF / 图片 / 文本 / 压缩包在右侧抽屉里直接看。
//!
//! 做成抽屉而不是弹窗（Codex 同款）：预览常常是拿着文档和对话对照着看
//! 的。多文件用标签页 —— 模型交付一批文档时挨个点开，像浏览器一样切
//! 着看，不用看一个关一个。
//!
//! 渲染交给 @file-viewer —— 全部在 webview 本地解析（Worker / WASM 自托管），
//! 文件不出机器。字节由宿主的 `read_file_bytes` 命令读进来：不开 Tauri 的
//! asset protocol，webview 依旧只有"经命令授权"这一条文件访问路径。
//!
//! 样式走宿主接管（styleIsolation: none）：viewer 渲染在普通 DOM 里，
//! styles.css 可以压它的任何内置样式 —— 深浅皮的硬编码、边距、残留
//! 控件都在那边收拾，不 fork 上游。
//!
//! `openFilePreview()` 是模块级入口。文件在聊天引用、改动列表、Markdown
//! 链接好几处出现，逐层穿 props 会弄脏一串中间组件 —— App 订阅这里的
//! 请求去开抽屉，各处一行调用。

import { lazy, Suspense, useEffect, useState } from "react";

import { openPath, readFileBytes, revealInFinder } from "../bridge";
import { basename, looksAbsPath, parentOf, tildify } from "../pathDisplay";
// CodeView 组件本身很小可以直接进主 bundle；大头 highlight.js
// 在它内部按需加载。
import CodeView, { CODE_EXTS } from "./CodeView";
// Markdown 用聊天同款 react-markdown（本来就在主 bundle）。
// 为什么不用 file-viewer 的 md 管线：见 MarkdownView 头注释。
import MarkdownView from "./MarkdownView";
import { ShotViewer } from "./ToolCard";

// 预览器和它的 renderer 管线只在第一次打开预览时加载，
// 启动 bundle 不背这份体积。
const Viewer = lazy(() =>
  import("@file-viewer/react").then((m) => ({ default: m.FileViewer })),
);

/**
 * viewer 配置。`[约束]` 必须是模块级常量：React 包对 options 按**引用**
 * 比较，新引用 = 配置变了 = 重新加载文档。写成内联字面量的话，拖抽屉
 * 宽度导致的每一帧重渲染都会让 PDF 从头解析一遍 —— 表现为"拖着拖着
 * 变成加载中"。引用稳定后，容器尺寸变化由渲染器内部的 ResizeObserver
 * 接手（resize: always），才是"拖多宽内容跟多宽"的实时缩放。
 */
const VIEWER_OPTIONS = {
  // 固定浅色：文档天然是白纸（Quick Look 同款）。深色皮各管线底色
  // 硬编码不一，追着对齐不值得。
  theme: "light",
  locale: "zh-CN",
  // 宿主接管样式：渲染进普通 DOM，styles.css 压内置样式的边距、残留
  // 控件。代价（Riot 全局控件样式渗入）在 styles.css 里就地中和。
  styleIsolation: "none",
  // 只显示内容。翻页用滚动，搜索、打印走头部的"系统应用打开"。
  toolbar: false,
  // `[约束]` 不传全局 fit。fit 是对**所有**渲染器的一刀切指令 ——
  // fit-width 会把几十列的 Excel 整表压进面板宽，列全挤成竖条。
  // 各管线的默认首屏已经是各自的最佳形态：表格原始尺寸 + 横向滚动，
  // PDF 默认贴宽并跟随容器（fork 源码确认），docx 的贴宽预留量和
  // PDF 的一样在 fork 里归零（file-viewer 仓库的 Riot 定制 commit）。
  pdf: { toolbar: false, navigation: false },
  // 拖表头边界调列宽 / 行高。库默认关闭（保持它的历史行为），
  // 但看宽表时把挤住的列拉开是刚需。
  // worker: true —— 不用它的"小文件主线程解析"快捷路径：WKWebView 里
  // 先渲染过 pptx/docx 之后，主线程的同步表格解析会冻结（sheetjs 的
  // read() 悬死，探针钉死过），真 Worker 的干净线程环境没这个问题。
  spreadsheet: { worker: true, resizableColumns: true, resizableRows: true },
} as const;

/**
 * 走 @file-viewer 渲染的扩展名 —— 有意收窄的白名单，不是渲染库的
 * 能力边界。库还认得 100+ 种格式，但只有验证过体验的才放进来；
 * 名单外的文件点击直接访达定位，好过一个半糟的预览。要扩类型往这里加。
 * md 不在这里：走 Riot 自制的 MarkdownView（原因见那个文件的头注释）。
 */
const PREVIEWABLE_EXTS = new Set(["pdf", "docx", "xlsx", "csv", "pptx"]);

/** Markdown：进预览抽屉，但渲染走聊天同款 react-markdown。 */
const MD_EXTS = new Set(["md", "markdown"]);

/** 图片不进预览抽屉：点击全屏放大，和聊天区附图同一个查看器。 */
const IMAGE_EXTS = new Set(["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp"]);

const IMAGE_MIME: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  svg: "image/svg+xml",
  bmp: "image/bmp",
};

function extOf(path: string): string {
  const name = basename(path);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

let listener: ((path: string) => void) | null = null;
let imageListener: ((path: string) => void) | null = null;

/**
 * 打开一个文件。传绝对路径。按类型分流：白名单文档和代码进预览
 * 抽屉（代码走自制高亮视图），图片开全屏查看，其余访达定位 ——
 * 调用方（聊天引用、改动列表、Markdown 链接、文档互链）不用各自判断。
 */
export function openFilePreview(path: string) {
  const ext = extOf(path);
  if (IMAGE_EXTS.has(ext)) {
    imageListener?.(path);
    return;
  }
  if (!PREVIEWABLE_EXTS.has(ext) && !CODE_EXTS.has(ext) && !MD_EXTS.has(ext)) {
    void revealInFinder(path);
    return;
  }
  listener?.(path);
}

/**
 * 磁盘图片的全屏查看。App 顶层挂一次。字节走 read_file_bytes 转
 * blob URL —— 不复用 read_image（那是给附件发送用的，带大小上限
 * 和类型白名单，svg 就不在里面）。
 */
export function ImageLightboxHost() {
  const [path, setPath] = useState<string | null>(null);
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    imageListener = setPath;
    return () => {
      imageListener = null;
    };
  }, []);

  useEffect(() => {
    if (!path) {
      setSrc(null);
      return;
    }
    let stale = false;
    let url: string | null = null;
    readFileBytes(path).then(
      (buf) => {
        if (stale) return;
        url = URL.createObjectURL(
          new Blob([buf], { type: IMAGE_MIME[extOf(path)] ?? "application/octet-stream" }),
        );
        setSrc(url);
      },
      () => {
        // 读不到（超限 / 已删除）退到访达定位，点击不能悄无声息。
        if (!stale) {
          setPath(null);
          void revealInFinder(path);
        }
      },
    );
    return () => {
      stale = true;
      if (url) URL.revokeObjectURL(url);
    };
  }, [path]);

  if (!path || !src) return null;
  return <ShotViewer src={src} alt={basename(path)} onClose={() => setPath(null)} />;
}

/** App 订阅预览请求（用来把右侧抽屉切到预览）。返回退订函数。 */
export function subscribeFilePreview(cb: (path: string) => void): () => void {
  listener = cb;
  return () => {
    if (listener === cb) listener = null;
  };
}

/** 右侧抽屉里的预览面板：标签行 + 当前文件的操作行 + 渲染区。 */
export function FilePreviewPanel({
  paths,
  active,
  onSelect,
  onCloseTab,
  onClose,
}: {
  /** 打开着的所有文件（绝对路径），也是标签顺序。 */
  paths: string[];
  /** 正在看的那个。一定在 paths 里。 */
  active: string;
  onSelect: (path: string) => void;
  onCloseTab: (path: string) => void;
  /** 收起整个面板（标签保留，回来还在）。 */
  onClose: () => void;
}) {
  const [openErr, setOpenErr] = useState(false);

  const sysOpen = () => {
    openPath(active).catch(() => {
      setOpenErr(true);
      setTimeout(() => setOpenErr(false), 2000);
    });
  };

  return (
    <div className="preview-panel">
      <div className="preview-tabs">
        {paths.map((p) => (
          <button
            type="button"
            key={p}
            className={p === active ? "preview-tab active" : "preview-tab"}
            title={p}
            onClick={() => onSelect(p)}
          >
            <span className="preview-tab-title">{basename(p)}</span>
            <span
              className="preview-tab-close"
              role="button"
              tabIndex={0}
              aria-label={`关闭 ${basename(p)}`}
              onClick={(e) => {
                e.stopPropagation();
                onCloseTab(p);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  e.stopPropagation();
                  onCloseTab(p);
                }
              }}
            >
              <CloseIcon />
            </span>
          </button>
        ))}
      </div>

      <div className="preview-panel-head">
        <span className="preview-panel-path" title={active}>
          {tildify(active)}
        </span>
        <span className="preview-panel-spacer" />
        {openErr ? (
          <span className="preview-panel-err" role="status">
            打不开
          </span>
        ) : null}
        <button
          type="button"
          className="icon"
          onClick={sysOpen}
          title="用系统默认应用打开"
        >
          <LaunchIcon />
        </button>
        <button
          type="button"
          className="icon"
          onClick={() => void revealInFinder(active)}
          title="在访达 / 资源管理器中显示"
        >
          <FolderMarkIcon />
        </button>
        <button type="button" className="icon" onClick={onClose} title="收起面板">
          <PanelIcon />
        </button>
      </div>

      {/* 全部标签保活：切换只切 display（终端面板同款手法）。渲染器、
          滚动位置、表格列宽都留在原地，切回即所见；关闭标签才卸载
          释放。首次挂载一定处于激活态（新标签即激活），不会在
          display:none 里做首屏贴宽；切回时渲染器自己的 ResizeObserver
          收到尺寸恢复会重新适配。内存由标签数量自然约束 —— 标签是
          用户手动开的，真出现大量大文件常驻再上 LRU 上限。 */}
      {paths.map((p) => (
        <PreviewBody key={p} path={p} visible={p === active} />
      ))}
    </div>
  );
}

/** 单个标签的渲染区。挂载后常驻（见上方保活注释），visible 只控 display。 */
function PreviewBody({ path, visible }: { path: string; visible: boolean }) {
  const [buf, setBuf] = useState<ArrayBuffer | null>(null);
  const [err, setErr] = useState<string | null>(null);
  /** 错误态"系统应用打开"的失败提示。 */
  const [openErr, setOpenErr] = useState(false);

  const sysOpen = () => {
    openPath(path).catch(() => {
      setOpenErr(true);
      setTimeout(() => setOpenErr(false), 2000);
    });
  };

  useEffect(() => {
    let stale = false;
    setBuf(null);
    setErr(null);
    readFileBytes(path).then(
      (b) => {
        if (!stale) setBuf(b);
      },
      (e: unknown) => {
        if (!stale) setErr(String(e));
      },
    );
    return () => {
      stale = true;
    };
  }, [path]);

  // 文档里的**本地相对链接**（md 互链、file: 链接）按当前文件所在目录
  // 解析成真实路径，开成新预览标签 —— 文档之间就能接着点。React 的
  // 合成事件先于 window 上的全局链接兜底触发，preventDefault 之后
  // 兜底会放过这次点击；http(s)/mailto 这里不碰，冒上去由兜底转交
  // 系统浏览器。
  const onBodyClick = (e: React.MouseEvent) => {
    if (e.defaultPrevented) return;
    const a = (e.target as Element).closest?.("a[href]");
    if (!a) return;
    // getAttribute 拿的是原始书写（"./other.md"），.href 属性会被
    // webview 解析成 http://localhost:1420/... —— 那正是要避开的。
    const href = a.getAttribute("href") ?? "";
    if (!href || href.startsWith("#") || /^https?:|^mailto:/i.test(href)) return;
    e.preventDefault();
    const target = resolveDocLink(href, path);
    if (target) openFilePreview(target);
  };

  return (
    <div
      className="preview-panel-body"
      style={visible ? undefined : { display: "none" }}
      onClick={onBodyClick}
    >
      {err ? (
        <div className="preview-panel-state">
          <p>{err}</p>
          {openErr ? (
            <span className="preview-panel-err" role="status">
              打不开
            </span>
          ) : null}
          <button type="button" className="preview-panel-fallback" onClick={sysOpen}>
            用系统应用打开
          </button>
        </div>
      ) : buf ? (
        MD_EXTS.has(extOf(path)) ? (
          <MarkdownView key={path} buf={buf} path={path} />
        ) : CODE_EXTS.has(extOf(path)) ? (
          // 代码走自制视图：深色、highlight.js，和聊天代码块同一套配色。
          <CodeView key={path} buf={buf} ext={extOf(path)} />
        ) : (
          <Suspense fallback={<div className="preview-panel-state">正在加载预览器…</div>}>
            <Viewer
              // 换文件整个重挂：renderer 管线各自管理 Worker / 缓存，
              // 重挂比原地换 source 走的路径少得多。
              key={path}
              className="preview-panel-viewer"
              file={buf}
              filename={basename(path)}
              options={VIEWER_OPTIONS}
            />
          </Suspense>
        )
      ) : (
        <div className="preview-panel-state">正在读取文件…</div>
      )}
    </div>
  );
}

/**
 * 把文档里的本地链接解析成绝对文件路径。`fromDoc` 是当前预览文件。
 * 支持 file: URL、绝对路径和相对路径（含 `..`）；带的 ?query / #锚点
 * 一律剪掉 —— 目标是磁盘文件不是网页。解析不出来返回 null，调用方
 * 静默放弃（点了没反应好过跳去一个 404）。
 */
function resolveDocLink(href: string, fromDoc: string): string | null {
  let raw = href.split(/[?#]/, 1)[0] ?? "";
  try {
    raw = decodeURIComponent(raw);
  } catch {
    // 文件名里真有孤立 % 时按原样用
  }
  if (!raw) return null;

  if (raw.startsWith("file://")) {
    try {
      let p = decodeURIComponent(new URL(raw).pathname);
      if (/^\/[A-Za-z]:/.test(p)) p = p.slice(1);
      return p || null;
    } catch {
      return null;
    }
  }
  if (looksAbsPath(raw)) return raw;

  // 相对路径：从当前文件所在目录出发逐段归一，`..` 往上走。
  const sep = fromDoc.includes("\\") ? "\\" : "/";
  const segs = parentOf(fromDoc).split(/[\\/]/).filter(Boolean);
  for (const part of raw.split(/[\\/]/)) {
    if (!part || part === ".") continue;
    if (part === "..") {
      if (segs.length > 0) segs.pop();
      continue;
    }
    segs.push(part);
  }
  if (segs.length === 0) return null;
  const joined = segs.join(sep);
  // Unix 绝对路径要补回根斜杠；Windows 的 `C:` 已在首段里。
  return sep === "/" ? `/${joined}` : joined;
}

function CloseIcon() {
  return (
    <svg width="10" height="10" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M4 4l8 8M12 4l-8 8"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
      />
    </svg>
  );
}

/** 系统应用打开：一个往外飞的箭头。 */
function LaunchIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M6.5 3.5H4a1.5 1.5 0 0 0-1.5 1.5v7A1.5 1.5 0 0 0 4 13.5h7A1.5 1.5 0 0 0 12.5 12V9.5"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
      />
      <path
        d="M9.5 2.5h4v4M13.2 2.8 8.2 7.8"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/** 在访达中显示：文件夹上一个定位点。 */
function FolderMarkIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M1.8 4.2a1.4 1.4 0 0 1 1.4-1.4h3l1.4 1.6h5.2a1.4 1.4 0 0 1 1.4 1.4v6a1.4 1.4 0 0 1-1.4 1.4H3.2a1.4 1.4 0 0 1-1.4-1.4v-7.6z"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinejoin="round"
      />
      <circle cx="8" cy="8.9" r="1.6" stroke="currentColor" strokeWidth="1.3" />
    </svg>
  );
}

/** 关闭面板。画的是"右边那一栏收起来"，和浏览器 / 改动面板同一个手势。 */
function PanelIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <rect x="1.5" y="2.5" width="13" height="11" rx="2" stroke="currentColor" strokeWidth="1.3" />
      <path d="M10.5 2.5v11" stroke="currentColor" strokeWidth="1.3" />
    </svg>
  );
}
