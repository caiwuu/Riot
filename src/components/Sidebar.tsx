/**
 * 侧边栏：按项目分组的会话列表。从 App.tsx 拆出 —— 宽度/开合等
 * 布局真值仍在 App，这里管分组折叠与每行的交互。
 */

import { useCallback, useEffect, useRef, useState } from "react";

import type { SessionInfo } from "../bridge";
import { basename } from "../pathDisplay";
import { Chevron } from "./Chevron";
import { IS_MAC } from "./chrome";
import { DotsIcon, FolderIcon, GearIcon, PlusIcon } from "./icons";

/** 折叠集的持久化键。纯 UI 状态，存 localStorage。 */
const COLLAPSED_KEY = "riot.layout.collapsedProjects";

function loadCollapsedProjects(): Set<string> {
  try {
    const raw = localStorage.getItem(COLLAPSED_KEY);
    if (!raw) return new Set();
    const arr: unknown = JSON.parse(raw);
    return new Set(Array.isArray(arr) ? arr.filter((x): x is string => typeof x === "string") : []);
  } catch {
    return new Set();
  }
}

function saveCollapsedProjects(roots: Set<string>) {
  localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...roots]));
}


export interface SidebarProps {
  /** 用户拖出来的宽度。真值和持久化都在 App 那层。 */
  width: number;
  projects: string[];
  /** 磁盘上已经找不到的项目根。 */
  missing: ReadonlySet<string>;
  sessions: SessionInfo[];
  active: string | null;
  renaming: string | null;
  onSelect: (id: string) => void;
  onNewSession: (root: string) => void;
  onOpenProject: () => void;
  onSettings: () => void;
  onSessionMenu: (e: React.MouseEvent, s: SessionInfo) => void;
  onProjectMenu: (e: React.MouseEvent, root: string) => void;
  onRenameSubmit: (id: string, title: string) => void;
  onRenameCancel: () => void;
}

export function Sidebar(props: SidebarProps) {
  const { width, projects, sessions, active, onOpenProject, onSettings } = props;
  const [collapsed, setCollapsed] = useState(loadCollapsedProjects);
  // 用来分辨「刚切到一个会话」和「本来就停在这个会话」。后者不能
  // 强制展开 —— 用户把当前项目折起来是有意的。
  const prevActive = useRef(active);

  const toggleCollapsed = useCallback((root: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(root)) next.delete(root);
      else next.add(root);
      saveCollapsedProjects(next);
      return next;
    });
  }, []);

  const expandProject = useCallback((root: string) => {
    setCollapsed((prev) => {
      if (!prev.has(root)) return prev;
      const next = new Set(prev);
      next.delete(root);
      saveCollapsedProjects(next);
      return next;
    });
  }, []);

  // 新会话从右键菜单、欢迎页进来时不会经过项目行上的 +，不展开的话
  // 建出来的会话会藏在折叠组里，看起来像没建成功。
  useEffect(() => {
    if (active && active !== prevActive.current) {
      const s = sessions.find((x) => x.id === active);
      if (s) expandProject(s.root);
    }
    prevActive.current = active;
  }, [active, sessions, expandProject]);

  // 有会话但不在项目列表里的根也要显示（理论上不会发生，但真发生时
  // 隐藏会话比多显示一个组糟得多）。
  const roots = [...projects];
  for (const s of sessions) {
    if (!roots.includes(s.root)) roots.push(s.root);
  }

  return (
    <aside className="sidebar" style={{ width }}>
      {/* macOS 红绿灯占左上角，这块留空且可拖动。Windows / Linux
          没有这块控件，只留一点顶距，免得「打开目录」贴顶。 */}
      <div
        className={IS_MAC ? "traffic-space" : "traffic-space compact"}
        data-tauri-drag-region
      />

      <button className="new-thread" onClick={onOpenProject}>
        <PlusIcon />
        打开目录…
      </button>

      <nav className="threads">
        {roots.length ? <div className="group-caption">项目</div> : null}
        {roots.map((root) => (
          <ProjectGroup
            key={root}
            {...props}
            root={root}
            gone={props.missing.has(root)}
            sessions={sessions.filter((s) => s.root === root)}
            collapsed={collapsed.has(root)}
            onToggle={() => toggleCollapsed(root)}
            onExpand={() => expandProject(root)}
          />
        ))}
      </nav>

      <div className="sidebar-foot">
        <button className="side-item" onClick={onSettings}>
          <GearIcon />
          <span className="side-label">设置</span>
        </button>
      </div>
    </aside>
  );
}

function ProjectGroup(
  props: SidebarProps & {
    root: string;
    gone: boolean;
    collapsed: boolean;
    onToggle: () => void;
    onExpand: () => void;
  },
) {
  const {
    root,
    gone,
    sessions,
    active,
    renaming,
    collapsed,
    onSelect,
    onNewSession,
    onSessionMenu,
    onProjectMenu,
    onRenameSubmit,
    onRenameCancel,
    onToggle,
    onExpand,
  } = props;
  const name = basename(root) || root;
  // 最近的在上面
  const ordered = [...sessions].sort((a, b) => b.seq - a.seq);
  const busy = collapsed && ordered.some((s) => s.busy);

  return (
    <div className={collapsed ? "project collapsed" : "project"}>
      <div
        className={gone ? "project-head gone" : "project-head"}
        onContextMenu={(e) => onProjectMenu(e, root)}
      >
        <button
          type="button"
          className="project-toggle"
          aria-expanded={!collapsed}
          aria-label={`${name}，${collapsed ? "已折叠" : "已展开"}${gone ? "，目录已不存在" : ""}`}
          title={gone ? `${root}（目录已不存在）` : root}
          onClick={onToggle}
        >
          <Chevron open={!collapsed} />
          <FolderIcon />
          <span className="project-name">{name}</span>
          {gone ? (
            <span className="project-gone" title="目录已不存在">
              已失效
            </span>
          ) : null}
          {busy ? (
            <span className="thread-busy" title="有会话正在运行" aria-label="有会话正在运行" />
          ) : null}
          {collapsed && ordered.length > 0 ? (
            <span className="project-count">{ordered.length}</span>
          ) : null}
        </button>
        <button
          className="row-btn"
          onClick={() => {
            onExpand();
            onNewSession(root);
          }}
          title={`在 ${name} 开新会话`}
        >
          <PlusIcon />
        </button>
        <button className="row-btn" onClick={(e) => onProjectMenu(e, root)} title="项目操作">
          <DotsIcon />
        </button>
      </div>

      <div
        className={collapsed ? "smooth-fold" : "smooth-fold open"}
        inert={collapsed}
        aria-hidden={collapsed}
      >
        <div className="smooth-fold-inner project-threads-inner">
          {ordered.map((s) =>
            renaming === s.id ? (
              <input
                key={s.id}
                className="rename-input"
                defaultValue={s.title ?? ""}
                autoFocus
                onFocus={(e) => e.currentTarget.select()}
                onKeyDown={(e) => {
                  if (e.key === "Enter") onRenameSubmit(s.id, e.currentTarget.value);
                  if (e.key === "Escape") onRenameCancel();
                }}
                onBlur={(e) => onRenameSubmit(s.id, e.currentTarget.value)}
              />
            ) : (
              <div
                key={s.id}
                className={s.id === active ? "thread active" : "thread"}
                onContextMenu={(e) => onSessionMenu(e, s)}
              >
                <button className="thread-label" onClick={() => onSelect(s.id)}>
                  {/* 正在跑的会话给个小圆点 —— 切走之后它还在干活，列表里
                      得看得出来，不然用户以为它闲着。 */}
                  {s.busy ? (
                    <span className="thread-busy" title="正在运行" aria-label="正在运行" />
                  ) : null}
                  {s.title ?? "新会话"}
                </button>
                <button className="row-btn" onClick={(e) => onSessionMenu(e, s)} title="会话操作">
                  <DotsIcon />
                </button>
              </div>
            ),
          )}
        </div>
      </div>
    </div>
  );
}
