/**
 * 侧边栏：定时任务入口 + 按项目分组的会话列表。从 App.tsx 拆出 ——
 * 宽度/开合等布局真值仍在 App，这里管分组折叠与每行的交互。
 */

import { useCallback, useEffect, useRef, useState } from "react";

import type { SessionInfo } from "../bridge";
import { useImeGuard } from "../hooks/useImeGuard";
import { useT } from "../i18n";
import { basename } from "../pathDisplay";
import { Chevron } from "./Chevron";
import {
  ClockIcon,
  DotsIcon,
  FolderIcon,
  GearIcon,
  PlusIcon,
  SidebarToggleIcon,
} from "./icons";

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

/** 大分组（定时任务 / 项目）的折叠开关，各存一个布尔键。 */
function useSectionFold(key: string): [boolean, () => void] {
  const [folded, setFolded] = useState(() => localStorage.getItem(key) === "1");
  const toggle = useCallback(() => {
    setFolded((v) => {
      localStorage.setItem(key, v ? "0" : "1");
      return !v;
    });
  }, [key]);
  return [folded, toggle];
}


export interface SidebarProps {
  /** 用户拖出来的宽度。真值和持久化都在 App 那层。 */
  width: number;
  projects: string[];
  /** 磁盘上已经找不到的项目根。 */
  missing: ReadonlySet<string>;
  sessions: SessionInfo[];
  active: string | null;
  /** 最近开聊的时刻（切换会话不算）。同项目里按它倒序，没记录的退回创建序。 */
  recency: Readonly<Record<string, number>>;
  renaming: string | null;
  onSelect: (id: string) => void;
  onNewSession: (root: string) => void;
  onOpenProject: () => void;
  onSettings: () => void;
  /** 打开主区的定时任务页（一级菜单项，Codex 的「已安排」同款）。 */
  onSchedules: () => void;
  /** 任务页正在前台 —— 菜单项画高亮，跟会话行互斥。 */
  schedulesActive: boolean;
  /** 启动时发现的错过运行条数。>0 时菜单项挂红点。 */
  missedSchedules: number;
  onSessionMenu: (e: React.MouseEvent, s: SessionInfo) => void;
  onProjectMenu: (e: React.MouseEvent, root: string) => void;
  /** 右键 / … 菜单正对着的行。菜单在文档别处，行要靠这个保住 hover。 */
  menuAnchor: string | null;
  onRenameSubmit: (id: string, title: string) => void;
  onRenameCancel: () => void;
  /** 收起侧栏。按钮坐在侧栏顶栏；收起后入口回到主区顶栏。 */
  onCollapse: () => void;
}

export function Sidebar(props: SidebarProps) {
  const { width, projects, sessions, active, onOpenProject, onSettings, onSchedules, onCollapse } =
    props;
  const { t, tn } = useT();
  const [collapsed, setCollapsed] = useState(loadCollapsedProjects);
  const [projectsFolded, toggleProjectsFolded] = useSectionFold("riot.layout.projectsFold");
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
      {/* macOS 红绿灯占左上角；开关靠右，贴着侧栏和主区的缝。空白处仍可拖窗口。 */}
      <div className="traffic-space" data-tauri-drag-region>
        <button
          className="tb-btn"
          onClick={onCollapse}
          title={t("app.sidebar.collapseTitle")}
          aria-label={t("app.sidebar.collapse")}
        >
          <SidebarToggleIcon />
        </button>
      </div>

      <button className="new-thread" onClick={onOpenProject}>
        <PlusIcon />
        {t("app.openDir")}
      </button>

      {/* 菜单项在滚动区里跟着列表一起走 —— 项目多的时候它占着顶部不动，
          等于白吃一行可视高度。 */}
      <nav className="threads">
        <button
          className={props.schedulesActive ? "side-item side-nav active" : "side-item side-nav"}
          onClick={onSchedules}
        >
          <ClockIcon />
          <span className="side-label">{t("app.sidebar.schedules")}</span>
          {props.missedSchedules > 0 ? (
            <span
              className="side-badge"
              title={tn("app.sidebar.missedTitle", props.missedSchedules)}
            >
              {props.missedSchedules}
            </span>
          ) : null}
        </button>

        {roots.length ? (
          <div className="section-head">
            <button
              type="button"
              className="section-toggle"
              aria-expanded={!projectsFolded}
              onClick={toggleProjectsFolded}
            >
              <Chevron open={!projectsFolded} />
              <span className="section-name">{t("app.sidebar.projects")}</span>
              {projectsFolded ? <span className="project-count">{roots.length}</span> : null}
            </button>
            <button className="row-btn" onClick={onOpenProject} title={t("app.openDir")}>
              <PlusIcon />
            </button>
          </div>
        ) : null}
        <div
          className={projectsFolded ? "smooth-fold" : "smooth-fold open"}
          inert={projectsFolded}
          aria-hidden={projectsFolded}
        >
          <div className="smooth-fold-inner section-body">
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
          </div>
        </div>
      </nav>

      <div className="sidebar-foot">
        <button className="side-item" onClick={onSettings}>
          <GearIcon />
          <span className="side-label">{t("app.sidebar.settings")}</span>
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
    schedulesActive,
    menuAnchor,
    recency,
  } = props;
  const name = basename(root) || root;
  const { t } = useT();
  // 同一时刻只有一行在改名，一个 guard 够用。
  const ime = useImeGuard();
  // 刚聊过的在上面；并列时退回创建序。
  const ordered = [...sessions].sort((a, b) => {
    const ra = recency[a.id] ?? 0;
    const rb = recency[b.id] ?? 0;
    if (ra !== rb) return rb - ra;
    return b.seq - a.seq;
  });
  const busy = collapsed && ordered.some((s) => s.busy);
  const foldState = t(collapsed ? "app.project.collapsed" : "app.project.expanded");

  return (
    <div className={collapsed ? "project collapsed" : "project"}>
      <div
        className={
          (gone ? "project-head gone" : "project-head") +
          (menuAnchor === `project:${root}` ? " menu-open" : "")
        }
        onContextMenu={(e) => onProjectMenu(e, root)}
      >
        <button
          type="button"
          className="project-toggle"
          aria-expanded={!collapsed}
          aria-label={t(gone ? "app.project.ariaLabelGone" : "app.project.ariaLabel", {
            name,
            state: foldState,
          })}
          title={gone ? t("app.project.titleGone", { root }) : root}
          onClick={onToggle}
        >
          <Chevron open={!collapsed} />
          <FolderIcon />
          <span className="project-name">{name}</span>
          {gone ? (
            <span className="project-gone" title={t("app.project.dirGone")}>
              {t("app.project.gone")}
            </span>
          ) : null}
          {busy ? (
            <span
              className="thread-busy"
              title={t("app.project.busy")}
              aria-label={t("app.project.busy")}
            />
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
          title={t("app.project.newSessionIn", { name })}
        >
          <PlusIcon />
        </button>
        <button
          className="row-btn"
          onClick={(e) => onProjectMenu(e, root)}
          title={t("app.project.actions")}
        >
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
                onCompositionStart={ime.onCompositionStart}
                onCompositionEnd={ime.onCompositionEnd}
                onKeyDown={(e) => {
                  // 组字中的回车确认候选词、Esc 取消候选，都不该动这次改名。
                  if (ime.isComposing(e)) return;
                  if (e.key === "Enter") onRenameSubmit(s.id, e.currentTarget.value);
                  if (e.key === "Escape") onRenameCancel();
                }}
                onBlur={(e) => onRenameSubmit(s.id, e.currentTarget.value)}
              />
            ) : (
              <div
                key={s.id}
                className={
                  (s.id === active && !schedulesActive ? "thread active" : "thread") +
                  (menuAnchor === `session:${s.id}` ? " menu-open" : "")
                }
                onContextMenu={(e) => onSessionMenu(e, s)}
              >
                <button className="thread-label" onClick={() => onSelect(s.id)}>
                  {/* 正在跑的会话给个小圆点 —— 切走之后它还在干活，列表里
                      得看得出来，不然用户以为它闲着。 */}
                  {s.busy ? (
                    <span
                      className="thread-busy"
                      title={t("common.running")}
                      aria-label={t("common.running")}
                    />
                  ) : null}
                  {s.title ?? t("app.newSession")}
                </button>
                <button
                  className="row-btn"
                  onClick={(e) => onSessionMenu(e, s)}
                  title={t("app.session.actions")}
                >
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
