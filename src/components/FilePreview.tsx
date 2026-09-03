//! 应用内文件预览：Office / PDF / 图片 / 文本 / 压缩包在右侧抽屉里直接看。
//!
//! 做成抽屉而不是弹窗（Codex 同款）：预览常常是拿着文档和对话对照着看
//! 的。多文件各占一个工作台标签（标签栏在抽屉顶部，见 Workbench）——
//! 模型交付一批文档时挨个点开，像浏览器一样切着看，不用看一个关一个。
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

import { lazy, Suspense, useContext, useEffect, useRef, useState } from "react";

import { openPath, readFileBytes, revealInFinder } from "../bridge";
import { useTimedFlag } from "../hooks/useTimedFlag";
import { basename, looksAbsPath, parentOf, relativeTo, tildify } from "../pathDisplay";
import { Resizer } from "./chrome";
// CodeView 组件本身很小可以直接进主 bundle；大头 highlight.js
// 在它内部按需加载。
import CodeView, { CODE_EXTS } from "./CodeView";
import { FileTree, type TreeTarget } from "./FileTree";
import { FolderIcon } from "./icons";
import { ProjectRootContext } from "./Markdown";
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
 * 能力边界。库还认得 100+ 种格式，但只有验证过体验的才放进来；名单外
 * 的文件不进这条管线：明确的二进制访达定位，其余按纯文本嗅探（见
 * BINARY_EXTS），好过一个半糟的预览。要扩类型往这里加。
 * md 不在这里：走 Riot 自制的 MarkdownView（原因见那个文件的头注释）。
 */
const PREVIEWABLE_EXTS = new Set(["pdf", "docx", "xlsx", "csv", "pptx"]);

/** Markdown：进预览抽屉，但渲染走聊天同款 react-markdown。 */
const MD_EXTS = new Set(["md", "markdown"]);

/** 图片不进预览抽屉：点击全屏放大，和聊天区附图同一个查看器。 */
const IMAGE_EXTS = new Set(["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp"]);

/**
 * 一眼就知道是二进制的扩展名：不读、不嗅探，直接访达定位。读一个
 * 30 MB 的压缩包只为了确认它不是文本，不值。名单外的未知扩展名
 * （`.env`、`Makefile`、`.lock`、没有扩展名的脚本）读进来嗅探：像文本
 * 就按纯文本显示，不像就给"系统应用打开"。
 */
const BINARY_EXTS = new Set([
  "zip", "gz", "tgz", "tar", "bz2", "xz", "zst", "7z", "rar", "jar", "war",
  "dmg", "pkg", "iso", "exe", "msi", "dll", "so", "dylib", "o", "a", "lib",
  "class", "pyc", "pyo", "wasm", "node", "bin", "dat", "db", "sqlite", "sqlite3",
  "mp3", "mp4", "m4a", "m4v", "mov", "avi", "mkv", "webm", "wav", "flac", "ogg", "aac",
  "ttf", "otf", "woff", "woff2", "eot", "ico", "icns", "psd", "ai", "sketch", "fig",
  "doc", "xls", "ppt", "numbers", "pages", "key", "heic", "tif", "tiff",
]);

/** 嗅探前多少字节。控制字符只要出现在开头一段就够判了。 */
const SNIFF_BYTES = 8192;

/**
 * 像不像文本：开头一段里没有 NUL 就算。UTF-16 带 BOM 的文件会误判成
 * 二进制（BOM 后的每个 ASCII 字符都跟着一个 0x00）—— 这类文件在代码
 * 仓库里少见，误判的结局也只是多点一次"系统应用打开"。
 */
function looksLikeText(buf: ArrayBuffer): boolean {
  const bytes = new Uint8Array(buf, 0, Math.min(buf.byteLength, SNIFF_BYTES));
  return !bytes.includes(0);
}

/**
 * 两段字节一模一样吗。重读之后拿它判"这个文件其实没被动过"。
 *
 * 逐字节比看着笨，但它省下的是整条渲染管线：高亮、Worker、文档解析。
 * 一兆的文件比一遍是毫秒级，而且只发生在用户正看着的那一个标签上。
 */
function sameBytes(a: ArrayBuffer, b: ArrayBuffer): boolean {
  if (a.byteLength !== b.byteLength) return false;
  const x = new Uint8Array(a);
  const y = new Uint8Array(b);
  for (let i = 0; i < x.length; i++) {
    if (x[i] !== y[i]) return false;
  }
  return true;
}

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

/** `openFilePreview` 把文件送去了哪条路。 */
export type PreviewRoute = "preview" | "image" | "reveal";

/**
 * 打开一个文件。传绝对路径。按类型分流：白名单文档和代码进预览
 * 抽屉（代码走自制高亮视图），图片开全屏查看，明确的二进制访达定位，
 * 剩下的未知类型也进抽屉、读进来嗅探（见 BINARY_EXTS）—— 调用方（聊天
 * 引用、改动列表、Markdown 链接、文档互链、文件树）不用各自判断。
 * 返回走了哪条路，文件树据此决定要不要把树钉住。
 */
export function openFilePreview(path: string): PreviewRoute {
  const ext = extOf(path);
  if (IMAGE_EXTS.has(ext)) {
    imageListener?.(path);
    return "image";
  }
  if (BINARY_EXTS.has(ext)) {
    void revealInFinder(path);
    return "reveal";
  }
  listener?.(path);
  return "preview";
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

/** 文件树栏的宽度：持久化键、默认与上下限。上限按面板宽的比例在拖时算。 */
const TREE_W_KEY = "riot.layout.tree";
const TREE_W = { def: 220, min: 150 } as const;

function loadTreeW(): number {
  const v = Number(localStorage.getItem(TREE_W_KEY));
  return Number.isFinite(v) && v >= TREE_W.min ? v : TREE_W.def;
}

/**
 * 右侧抽屉里的预览面板：左边是当前文件（操作行 + 渲染区），右边一栏
 * 可收起的项目文件树（Codex 同款布局，见 FileTree）。
 *
 * 标签属于抽屉顶部的统一标签栏（见 Workbench）—— 每个文件是一个顶层
 * 工作台标签，和浏览器 / Git 改动平级。这里不再自带标签行，但仍然
 * 一次性挂着**全部**打开的文件（见下方保活注释），visible 只控 display：
 * 切去别的标签（哪怕是浏览器）再切回来，渲染器和滚动位置都在原地。
 *
 * "文件"标签（active 为 null）也落在这个面板上：树栏必显，左边是一句
 * 占位。树只有这一份实例，展开状态、滚动位置在两种标签之间共用。
 */
export function FilePreviewPanel({
  sessionId,
  paths,
  active,
  visible,
  tree,
  onToggleTree,
  refreshKey,
  onOpen,
  onTreeContextMenu,
}: {
  sessionId: string;
  /** 打开着的所有文件（绝对路径），即标签顺序。 */
  paths: string[];
  /** 正在看（或上次看）的那个。在 paths 里；null = 激活的是"文件"标签。 */
  active: string | null;
  /** 激活的工作台标签是不是预览 / 文件。不是的话整个面板 display:none 保活。 */
  visible: boolean;
  /** 预览文件时显不显示树栏（"文件"标签下不看这个，总显示）。 */
  tree: boolean;
  onToggleTree: () => void;
  /**
   * 递增一次就重新读一遍磁盘：树重列已展开的目录，正在看的那个标签
   * 重新读字节。轮次结束时由外层推 —— 模型刚改完的文件，用户切过来
   * 看到的必须是改完的样子。
   */
  refreshKey: number;
  /** 树里点了一个文件。 */
  onOpen: (abs: string) => void;
  onTreeContextMenu: (e: React.MouseEvent, target: TreeTarget) => void;
}) {
  const [openErr, flashOpenErr] = useTimedFlag(false, 2000);
  const root = useContext(ProjectRootContext);
  const [treeW, setTreeW] = useState(loadTreeW);
  const panelRef = useRef<HTMLDivElement>(null);
  const dragFrom = useRef(0);
  const dragLive = useRef(0);

  const sysOpen = () => {
    if (active) openPath(active).catch(() => flashOpenErr(true));
  };

  const showTree = active === null || tree;

  return (
    <div
      ref={panelRef}
      className="preview-panel"
      style={visible ? undefined : { display: "none" }}
    >
      {/* 头部横跨整个面板（Codex 同款）：按钮贴面板最右缘，树栏从头部
          下方开始。头部若只占编辑器那一列，按钮就夹在编辑器和树中间。 */}
      {active ? (
        <div className="preview-panel-head">
          <PathCrumbs path={active} root={root} />
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
          <button
            type="button"
            className={tree ? "icon on" : "icon"}
            onClick={onToggleTree}
            title={tree ? "隐藏文件树" : "显示文件树"}
            aria-pressed={tree}
          >
            <FolderIcon />
          </button>
        </div>
      ) : null}

      <div className="preview-split">
        <div className="preview-main">
          {active ? null : <div className="preview-panel-state">从右侧选择一个文件</div>}

          {/* 全部标签保活：切换只切 display（终端面板同款手法）。渲染器、
              滚动位置、表格列宽都留在原地，切回即所见；关闭标签才卸载
              释放。首次挂载一定处于激活态（新标签即激活），不会在
              display:none 里做首屏贴宽；切回时渲染器自己的 ResizeObserver
              收到尺寸恢复会重新适配。内存由标签数量自然约束 —— 标签是
              用户手动开的，真出现大量大文件常驻再上 LRU 上限。 */}
          {paths.map((p) => (
            <PreviewBody key={p} path={p} visible={p === active} rev={refreshKey} />
          ))}
        </div>

        {showTree ? (
          <>
            <Resizer
              axis="x"
              onStart={() => {
                dragFrom.current = treeW;
                dragLive.current = treeW;
              }}
              onDelta={(d) => {
                // 拖的是树栏的左缘：往左（负位移）变宽。最多占面板六成，
                // 编辑器那边总得剩下能读代码的宽度。
                const max = Math.max(
                  TREE_W.min,
                  (panelRef.current?.clientWidth ?? 600) * 0.6,
                );
                const w = Math.min(Math.max(dragFrom.current - d, TREE_W.min), max);
                dragLive.current = w;
                setTreeW(w);
              }}
              onEnd={() =>
                localStorage.setItem(TREE_W_KEY, String(Math.round(dragLive.current)))
              }
              onReset={() => {
                setTreeW(TREE_W.def);
                localStorage.setItem(TREE_W_KEY, String(TREE_W.def));
              }}
            />
            <div className="file-tree-col" style={{ width: treeW }}>
              <FileTree
                sessionId={sessionId}
                root={root}
                selected={active}
                refreshKey={refreshKey}
                onOpen={onOpen}
                onContextMenu={onTreeContextMenu}
              />
            </div>
          </>
        ) : null}
      </div>
    </div>
  );
}

/**
 * 单个标签的渲染区。挂载后常驻（见上方保活注释），visible 只控 display。
 *
 * `rev` 变一次就重新读一遍盘。文件是活的 —— agent 改完之后这里还挂着
 * 打开那一刻的字节，用户得关掉标签再开一次才看得到新内容。
 */
function PreviewBody({ path, visible, rev }: { path: string; visible: boolean; rev: number }) {
  const [buf, setBuf] = useState<ArrayBuffer | null>(null);
  const [err, setErr] = useState<string | null>(null);
  /** 错误态"系统应用打开"的失败提示。 */
  const [openErr, flashOpenErr] = useTimedFlag(false, 2000);
  /** 最后一次发出去的那次读取是哪一版 rev。null = 还没读过。 */
  const reqRev = useRef<number | null>(null);

  const sysOpen = () => {
    openPath(path).catch(() => flashOpenErr(true));
  };

  // 藏着的标签不跟着刷 —— 开了八个标签的话，每轮结束就是八次全量读盘，
  // 而其中七个用户根本没在看。切回来时 visible 一翻这个 effect 会再跑
  // 一次，那一刻才补读，看到的照样是最新的。
  //
  // path 不进"读过没有"的记账：这个组件按 path 做 key（见上面的 map），
  // 换文件是换实例，不是换 prop。
  //
  // `[约束]` 不要加"cleanup 里置 stale、回来丢弃过期结果"的守卫（文件树
  // 那边同样的坑，见 FileTree 的 fetchDir）。这个 effect 是**带记账的**：
  // reqRev 记住"这一版已经发过请求了"。cleanup 一旦把飞行中的那次作废，
  // 记账又不许重发，两下一夹就是永远停在"正在读取文件…" —— StrictMode
  // 的挂-拆-挂会当场触发，切走再切回来（visible 翻两下）在正式包里也会。
  // 过期结果改由 reqRev 自己认：回包时它已经不是最后一次请求就丢掉。
  useEffect(() => {
    if (!visible || reqRev.current === rev) return;
    // 只有第一次才清空。重读时旧内容留到新的到位为止 —— 清一下就是
    // "内容整块消失 → 正在读取 → 回来"，滚动位置也跟着回到顶部，而
    // 十有八九这个文件这一轮压根没被动过。
    if (reqRev.current === null) {
      setBuf(null);
      setErr(null);
    }
    reqRev.current = rev;
    readFileBytes(path).then(
      (b) => {
        if (reqRev.current !== rev) return;
        setErr(null);
        // 内容真没变就不换引用。换了的话下游全得重来一遍：CodeView 重新
        // 解码 + 重新高亮，@file-viewer 连 Worker 带文档重新解析一次。
        setBuf((cur) => (cur && sameBytes(cur, b) ? cur : b));
      },
      (e: unknown) => {
        if (reqRev.current === rev) setErr(String(e));
      },
    );
  }, [path, visible, rev]);

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
          <CodeView key={path} buf={buf} ext={extOf(path)} name={basename(path)} />
        ) : PREVIEWABLE_EXTS.has(extOf(path)) ? (
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
        ) : looksLikeText(buf) ? (
          // 名单外的扩展名（.env、Makefile、.lock、无后缀脚本）：像文本就
          // 当纯文本看。文件名认得出的（Makefile、Dockerfile）照样高亮。
          <CodeView key={path} buf={buf} ext={extOf(path)} name={basename(path)} />
        ) : (
          <div className="preview-panel-state">
            <p>这是二进制文件，应用内看不了。</p>
            {openErr ? (
              <span className="preview-panel-err" role="status">
                打不开
              </span>
            ) : null}
            <button type="button" className="preview-panel-fallback" onClick={sysOpen}>
              用系统应用打开
            </button>
          </div>
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

/**
 * 头部的路径面包屑（Codex 同款）：项目内的文件按段显示，目录段淡、
 * 文件名亮；项目外的文件退回 `~/…` 整条路径 —— 没有"相对谁"可言。
 * 整条路径仍放在 title 里，悬停能看全。
 */
function PathCrumbs({ path, root }: { path: string; root: string }) {
  const rel = relativeTo(root, path);
  if (rel === null || rel === "") {
    return (
      <span className="preview-panel-path" title={path}>
        {tildify(path)}
      </span>
    );
  }
  const segs = rel.split("/");
  const name = segs.pop() ?? rel;
  return (
    <span className="preview-panel-path preview-crumbs" title={path}>
      {/* 所有段收在一个 LTR 内层里：外层为了"从头截断"设成 RTL，段要是
          各自成一个盒子，会被按从右到左排（见 .preview-crumbs 注释）。 */}
      <span className="preview-crumbs-inner">
        {segs.map((s, i) => (
          // 目录段可能重名（a/x/a/y），key 带上下标。
          <span key={`${i}:${s}`}>
            {s}
            <span className="preview-crumb-sep" aria-hidden>
              ›
            </span>
          </span>
        ))}
        <span className="preview-crumb-name">{name}</span>
      </span>
    </span>
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

