/**
 * 项目文件树（Codex 同款）：预览面板旁边那一栏，只读浏览，点文件开预览。
 *
 * 逐层懒加载：展开哪个目录才列哪个目录（宿主的 `list_dir` 一次一层，
 * 边界和截断规则见 src-tauri/src/tree.rs）。不整棵预扫 —— 大仓库扫一遍
 * 是秒级，而用户一次只看一两个目录。
 *
 * 写操作一概没有：新建 / 重命名 / 删除交给 agent 走工具链和权限管线，
 * 这里不另开一条绕过权限的写路径。
 *
 * 行可以按住往输入框拖（见 `dragRow` 和 lib/fileDrag）。右键菜单里的
 * "添加到对话"是同一件事的键盘 / 无鼠标路径，两条都留着。
 *
 * 展开状态按会话记在模块级缓存里：预览面板跨会话整块重挂，切回来时
 * 树不该缩回全收起的样子。
 */

import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";

import { type DirEntry, listDir, searchFiles } from "../bridge";
import { startFileDrag } from "../lib/fileDrag";
import { basename, joinRoot, relativeTo } from "../pathDisplay";
import { Chevron } from "./Chevron";
import { FileIcon } from "./FileIcon";

/** 右键菜单的目标：树里的一项。 */
export interface TreeTarget {
  abs: string;
  rel: string;
  isDir: boolean;
}

/** 一层目录的加载结果。undefined（不在 Map 里）= 还没加载。 */
type Listing = { entries: DirEntry[]; truncated: number } | { error: string };

/** 可见行：一个条目，或条目下面的一行说明（加载中 / 出错 / 截断）。 */
type Row =
  | {
      kind: "entry";
      rel: string;
      name: string;
      depth: number;
      isDir: boolean;
      isSymlink: boolean;
      open: boolean;
    }
  | { kind: "note"; key: string; depth: number; text: string; error?: boolean };

/** 筛选结果最多几条。一屏几十条，滚两屏还找不到就该换关键词了。 */
const FILTER_LIMIT = 200;
/** 筛选输入的防抖。每敲一个字就扫一次仓库缓存没必要。 */
const FILTER_DEBOUNCE_MS = 120;

/** 每个会话记住的展开集合。key 是会话 id。 */
const expandedBySession = new Map<string, Set<string>>();

/** 相对路径的所有祖先目录（不含自己、不含根）。`a/b/c.rs` → [`a`, `a/b`]。 */
function ancestorsOf(rel: string): string[] {
  const out: string[] = [];
  const segs = rel.split("/");
  for (let i = 1; i < segs.length; i++) out.push(segs.slice(0, i).join("/"));
  return out;
}

function parentOfRel(rel: string): string {
  const i = rel.lastIndexOf("/");
  return i < 0 ? "" : rel.slice(0, i);
}

export function FileTree({
  sessionId,
  root,
  selected,
  refreshKey,
  onOpen,
  onContextMenu,
}: {
  sessionId: string;
  /** 项目根（绝对路径）。树的相对路径都拼在它上面。 */
  root: string;
  /** 正在预览的文件（绝对路径）。树里高亮它，并把它的祖先展开、滚到可见。 */
  selected: string | null;
  /** 递增一次就把已展开的目录全部重新列一遍（轮次结束后模型可能建了新文件）。 */
  refreshKey: number;
  onOpen: (abs: string) => void;
  onContextMenu?: (e: React.MouseEvent, target: TreeTarget) => void;
}) {
  const [listings, setListings] = useState<Map<string, Listing>>(() => new Map());
  const [expanded, setExpanded] = useState<Set<string>>(
    () => new Set(expandedBySession.get(sessionId) ?? []),
  );
  /** 键盘光标落在哪一行（entry 的 rel）。null = 还没用过键盘。 */
  const [cursor, setCursor] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  /** 防抖后的筛选词。空 = 树模式。 */
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<string[] | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  /** 正在请求中的目录，防重复发。 */
  const inflight = useRef(new Set<string>());
  /** 上次已经滚到可见的那个 selected（相对路径）。同一个文件不反复滚。 */
  const revealed = useRef<string | null>(null);
  const uid = useId().replace(/[^a-zA-Z0-9]/g, "");

  useEffect(() => {
    expandedBySession.set(sessionId, expanded);
  }, [sessionId, expanded]);

  // 不做"卸载后丢弃过期结果"的守卫。`[约束]` 别用"卸载时计数器 +1、
  // 回来比对"那种写法：StrictMode 在开发模式下会把 effect 挂一遍、拆一遍、
  // 再挂一遍，拆那一下把计数加了 1，首屏唯一的一次请求回来就被当成过期
  // 丢掉，而 inflight 去重又不许再发 —— 树永远停在"正在读取"。真出过。
  // 组件按会话 key 重挂，跨会话串不了；React 18+ 对已卸载组件 setState
  // 是空操作，不需要守卫。
  const fetchDir = useCallback(
    (rel: string) => {
      if (inflight.current.has(rel)) return;
      inflight.current.add(rel);
      listDir(sessionId, rel).then(
        (l) => {
          inflight.current.delete(rel);
          setListings((prev) => {
            const next = new Map(prev);
            next.set(rel, { entries: l.entries, truncated: l.truncated });
            return next;
          });
        },
        (e: unknown) => {
          inflight.current.delete(rel);
          setListings((prev) => {
            const next = new Map(prev);
            next.set(rel, { error: String(e) });
            return next;
          });
        },
      );
    },
    [sessionId],
  );

  // 根和所有展开着的目录都要有内容；没有的补拉。listings 变化也要跑 ——
  // 刷新时旧内容被换掉，或者展开一个从没列过的目录。
  useEffect(() => {
    if (!listings.has("")) fetchDir("");
    for (const rel of expanded) {
      if (!listings.has(rel)) fetchDir(rel);
    }
  }, [expanded, listings, fetchDir]);

  // 轮次结束：把已展开的目录全部重列。旧内容留着直到新的到位，不闪。
  const firstRefresh = useRef(true);
  useEffect(() => {
    if (firstRefresh.current) {
      firstRefresh.current = false;
      return;
    }
    fetchDir("");
    for (const rel of expanded) fetchDir(rel);
    // expanded 只是刷新时的读取对象，不是触发条件。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshKey, fetchDir]);

  // 预览换了文件：展开它的祖先。滚到可见在下面的 layout effect 里 ——
  // 行要等祖先列出来才存在。
  useEffect(() => {
    if (!selected) return;
    const rel = relativeTo(root, selected);
    if (rel === null || rel === "") return;
    const anc = ancestorsOf(rel);
    setExpanded((prev) => {
      if (anc.every((a) => prev.has(a))) return prev;
      const next = new Set(prev);
      for (const a of anc) next.add(a);
      return next;
    });
  }, [selected, root]);

  // 筛选词防抖 → 搜索。
  useEffect(() => {
    const q = filter.trim();
    const t = window.setTimeout(() => setQuery(q), FILTER_DEBOUNCE_MS);
    return () => window.clearTimeout(t);
  }, [filter]);

  useEffect(() => {
    if (!query) {
      setHits(null);
      return;
    }
    let stale = false;
    searchFiles(sessionId, query, FILTER_LIMIT).then(
      (r) => {
        if (!stale) setHits(r);
      },
      () => {
        if (!stale) setHits([]);
      },
    );
    return () => {
      stale = true;
    };
  }, [sessionId, query]);

  const toggleDir = useCallback(
    (rel: string) => {
      const opening = !expanded.has(rel);
      setExpanded((prev) => {
        const next = new Set(prev);
        if (opening) next.add(rel);
        else next.delete(rel);
        return next;
      });
      // 展开就重列一次：收着的时候模型可能在里面建了文件，而轮次结束的
      // 刷新只顾得上当时展开着的目录。旧内容先显示，新的到了原地换掉。
      if (opening) fetchDir(rel);
    },
    [expanded, fetchDir],
  );

  const selectedRel = selected ? relativeTo(root, selected) : null;

  /** 树模式的可见行：从根开始按展开状态展平。 */
  const rows = useMemo<Row[]>(() => {
    const out: Row[] = [];
    const walk = (dir: string, depth: number) => {
      const l = listings.get(dir);
      if (!l) {
        out.push({ kind: "note", key: `${dir}\u0000loading`, depth, text: "正在读取…" });
        return;
      }
      if ("error" in l) {
        out.push({ kind: "note", key: `${dir}\u0000error`, depth, text: l.error, error: true });
        return;
      }
      for (const e of l.entries) {
        const rel = dir ? `${dir}/${e.name}` : e.name;
        const open = e.isDir && expanded.has(rel);
        out.push({
          kind: "entry",
          rel,
          name: e.name,
          depth,
          isDir: e.isDir,
          isSymlink: e.isSymlink,
          open,
        });
        if (open) walk(rel, depth + 1);
      }
      if (l.truncated > 0) {
        out.push({
          kind: "note",
          key: `${dir}\u0000more`,
          depth,
          text: `还有 ${l.truncated} 项未显示`,
        });
      }
    };
    walk("", 0);
    return out;
  }, [listings, expanded]);

  /** 键盘能停留的行（筛选模式是命中列表，树模式是条目）。 */
  const navRels = useMemo<string[]>(
    () => (hits ? hits : rows.flatMap((r) => (r.kind === "entry" ? [r.rel] : []))),
    [hits, rows],
  );

  // 正在预览的文件一进树就滚到可见。每次渲染都试：它的行可能要等祖先
  // 展开、目录列出来之后才存在，哪次渲染出来了就哪次滚。用 layout
  // effect 是为了在绘制前滚，不会先看到旧位置再跳一下。
  useLayoutEffect(() => {
    if (!selectedRel || hits || revealed.current === selectedRel) return;
    const el = listRef.current?.querySelector<HTMLElement>(
      `[data-rel="${cssEscape(selectedRel)}"]`,
    );
    if (!el) return;
    revealed.current = selectedRel;
    el.scrollIntoView({ block: "nearest" });
  });

  const openRel = (rel: string) => onOpen(joinRoot(root, rel));

  const activate = (rel: string) => {
    if (hits) {
      openRel(rel);
      return;
    }
    const row = rows.find((r) => r.kind === "entry" && r.rel === rel);
    if (!row || row.kind !== "entry") return;
    if (row.isDir) toggleDir(rel);
    else openRel(rel);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (navRels.length === 0) return;
    const idx = cursor ? navRels.indexOf(cursor) : -1;
    const moveTo = (i: number) => {
      const next = navRels[Math.max(0, Math.min(navRels.length - 1, i))];
      if (next === undefined) return;
      setCursor(next);
      listRef.current
        ?.querySelector<HTMLElement>(`[data-rel="${cssEscape(next)}"]`)
        ?.scrollIntoView({ block: "nearest" });
    };
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        moveTo(idx + 1);
        break;
      case "ArrowUp":
        e.preventDefault();
        moveTo(idx <= 0 ? 0 : idx - 1);
        break;
      case "Home":
        e.preventDefault();
        moveTo(0);
        break;
      case "End":
        e.preventDefault();
        moveTo(navRels.length - 1);
        break;
      case "ArrowRight": {
        if (hits || !cursor) return;
        e.preventDefault();
        const row = rows.find((r) => r.kind === "entry" && r.rel === cursor);
        if (!row || row.kind !== "entry" || !row.isDir) return;
        // 收着就展开；已展开就进到第一个孩子（VS Code 同款）。
        if (!row.open) toggleDir(cursor);
        else moveTo(idx + 1);
        break;
      }
      case "ArrowLeft": {
        if (hits || !cursor) return;
        e.preventDefault();
        const row = rows.find((r) => r.kind === "entry" && r.rel === cursor);
        if (!row || row.kind !== "entry") return;
        // 展开着的目录先收；文件或收着的目录跳回父目录。
        if (row.isDir && row.open) {
          toggleDir(cursor);
        } else {
          const parent = parentOfRel(cursor);
          if (parent) moveTo(navRels.indexOf(parent));
        }
        break;
      }
      case "Enter":
      case " ":
        if (!cursor) return;
        e.preventDefault();
        activate(cursor);
        break;
      default:
    }
  };

  const target = (rel: string, isDir: boolean): TreeTarget => ({
    abs: joinRoot(root, rel),
    rel,
    isDir,
  });

  /**
   * 按住一行往外拖 = 把它放进输入框（落点在 Composer，机械见 lib/fileDrag）。
   *
   * 走出几像素才算拖，所以单击照旧是"打开预览 / 展开目录"。给的是绝对
   * 路径 —— 输入框那边和从访达拖进来的走同一条收件路径，拖一张图进去
   * 一样会变成待发的图片而不是一个引用块。
   */
  const dragRow = (e: React.PointerEvent<HTMLElement>, rel: string) => {
    startFileDrag(e, { abs: joinRoot(root, rel) }, e.currentTarget);
  };

  return (
    <div className="file-tree">
      <div className="file-tree-filter">
        <SearchIcon />
        <input
          type="text"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape" && filter) {
              e.preventDefault();
              e.stopPropagation();
              setFilter("");
            } else if (e.key === "ArrowDown" || e.key === "Enter") {
              // 从输入框直接下到列表：第一条命中就是最想要的那条。
              e.preventDefault();
              listRef.current?.focus();
              const first = navRels[0];
              if (first !== undefined) {
                if (e.key === "Enter" && hits) openRel(first);
                else setCursor(first);
              }
            }
          }}
          placeholder="筛选文件…"
          aria-label="筛选文件"
          spellCheck={false}
        />
        {filter ? (
          <button type="button" className="icon file-tree-clear" onClick={() => setFilter("")} title="清除">
            <ClearIcon />
          </button>
        ) : null}
      </div>

      <div
        ref={listRef}
        className="file-tree-list"
        role="tree"
        tabIndex={0}
        aria-label="项目文件"
        aria-activedescendant={cursor ? `${uid}-${idFor(cursor)}` : undefined}
        onKeyDown={onKeyDown}
      >
        {hits ? (
          hits.length === 0 ? (
            <div className="file-tree-empty">没有匹配的文件</div>
          ) : (
            hits.map((rel) => (
              <div
                key={rel}
                id={`${uid}-${idFor(rel)}`}
                role="treeitem"
                aria-selected={rel === selectedRel}
                className={rowClass(rel === selectedRel, rel === cursor)}
                data-rel={rel}
                title={rel}
                onPointerDown={(e) => dragRow(e, rel)}
                onClick={() => {
                  setCursor(rel);
                  openRel(rel);
                }}
                onContextMenu={(e) => onContextMenu?.(e, target(rel, false))}
              >
                <span className="tree-slot">
                  <FileIcon path={rel} />
                </span>
                <span className="tree-name">{basename(rel)}</span>
                <span className="tree-dir">{parentOfRel(rel)}</span>
              </div>
            ))
          )
        ) : (
          rows.map((r) =>
            r.kind === "note" ? (
              <div
                key={r.key}
                className={r.error ? "tree-note error" : "tree-note"}
                style={{ paddingLeft: indent(r.depth) }}
              >
                {r.text}
              </div>
            ) : (
              <div
                key={r.rel}
                id={`${uid}-${idFor(r.rel)}`}
                role="treeitem"
                aria-level={r.depth + 1}
                aria-expanded={r.isDir ? r.open : undefined}
                aria-selected={r.rel === selectedRel}
                className={rowClass(r.rel === selectedRel, r.rel === cursor)}
                style={{ paddingLeft: indent(r.depth) }}
                data-rel={r.rel}
                title={r.rel}
                onPointerDown={(e) => dragRow(e, r.rel)}
                onClick={() => {
                  setCursor(r.rel);
                  activate(r.rel);
                }}
                onContextMenu={(e) => onContextMenu?.(e, target(r.rel, r.isDir))}
              >
                <span className="tree-slot">
                  {r.isDir ? <Chevron open={r.open} /> : <FileIcon path={r.name} />}
                </span>
                <span className="tree-name">{r.name}</span>
                {r.isSymlink ? (
                  <span className="tree-link" title="符号链接" aria-label="符号链接">
                    ↗
                  </span>
                ) : null}
              </div>
            ),
          )
        )}
      </div>
    </div>
  );
}

/** 每层缩进的像素。根层留出一点左边距。 */
function indent(depth: number): number {
  return 6 + depth * 14;
}

function rowClass(selected: boolean, cursor: boolean): string {
  let cls = "tree-row";
  if (selected) cls += " selected";
  if (cursor) cls += " cursor";
  return cls;
}

/** 相对路径 → 能当 DOM id 用的串。id 只要唯一，不用可读。 */
function idFor(rel: string): string {
  let h = 0;
  for (let i = 0; i < rel.length; i++) h = (h * 31 + rel.charCodeAt(i)) | 0;
  return `r${(h >>> 0).toString(36)}${rel.length}`;
}

/** 放进 `[data-rel="…"]` 引号里的转义：只有引号和反斜杠需要处理。 */
function cssEscape(s: string): string {
  return s.replace(/["\\]/g, "\\$&");
}

function SearchIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden>
      <circle cx="7" cy="7" r="4.4" stroke="currentColor" strokeWidth="1.4" />
      <path d="M10.3 10.3L14 14" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
    </svg>
  );
}

function ClearIcon() {
  return (
    <svg width="10" height="10" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
    </svg>
  );
}
