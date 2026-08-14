import { memo, useCallback, useEffect, useRef, useState } from "react";

import {
  addProject,
  type ConfigStatus,
  createSession,
  deleteSession,
  getConfig,
  type ImageInput,
  hasActiveKey,
  pickFiles,
  readImage,
  listSessions,
  type PermissionAsk,
  type PermissionMode,
  type PermissionResponse,
  pickDirectory,
  type ProviderConfig,
  removeProject,
  renameSession,
  revealInFinder,
  setConfig as saveConfig,
  type SlashCommand,
  slashCommands,
  slashExpand,
  searchFiles,
  compactSession,
  type SessionInfo,
  setPermissionMode,
  setWindowTitle,
  subscribeFullscreen,
} from "./bridge";
import { BrowserPanel } from "./components/BrowserPanel";
import { ScopePanel } from "./components/ScopePanel";
import { SessionSettings } from "./components/SessionSettings";
import { ConfirmDialog, type ConfirmRequest } from "./components/ConfirmDialog";
import { Markdown } from "./components/Markdown";
import { PermissionDialog, PlanApprovalCard, PlanDraft } from "./components/PermissionDialog";
import { Settings } from "./components/Settings";
import { TerminalPanel } from "./components/TerminalPanel";
import { ToolCard } from "./components/ToolCard";
import { type Item, type QueuedItem, useSession } from "./hooks/useSession";

/**
 * 布局照着 Codex 桌面端：左侧按项目分组的会话列表（可拖宽、可收起），
 * 主区顶部一条工具栏，中间是对话流，右侧一个抽屉（放浏览器），底部一条
 * 终端面板。三块附属面板的尺寸都能拖，且记住上次的位置。
 *
 * 没有"当前工作区"这个全局概念 —— 每个会话在创建时绑定自己的项目
 * 目录，之后永不改变。多项目并行时谁也不影响谁；"换了目录代码还写进
 * 旧目录"那类 bug 在这个模型下没有生存空间。
 */
/** 右键 / "…" 菜单。全局单实例，点哪开哪 —— 免得每行都挂一份状态。 */
interface MenuState {
  x: number;
  y: number;
  entries: { label: string; danger?: boolean; action: () => void }[];
}

/* ── 布局尺寸 ───────────────────────────────── */

/** 布局尺寸的持久化键。存 localStorage —— 纯 UI 状态，不值得进宿主配置。 */
const LS = {
  sidebar: "riot.layout.sidebar",
  sidebarOpen: "riot.layout.sidebarOpen",
  drawer: "riot.layout.drawer",
  term: "riot.layout.term",
};

const SIDEBAR = { def: 230, min: 180, max: 420 };
/** 抽屉窄过这个值页面就没法看了，浏览器面板自己也有同样的下限。 */
const DRAWER_MIN = 320;
const TERM = { def: 260, min: 110 };

/** 抽屉的默认宽度跟着窗口走 —— 固定像素在小窗口上会把对话挤没。 */
const drawerDefault = () => Math.round(window.innerWidth * 0.42);

function loadPx(key: string, fallback: number): number {
  const v = Number(localStorage.getItem(key));
  return Number.isFinite(v) && v > 0 ? v : fallback;
}

function savePx(key: string, v: number) {
  localStorage.setItem(key, String(Math.round(v)));
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(Math.max(v, lo), hi);
}

export function App() {
  const [config, setConfig] = useState<ConfigStatus | null>(null);
  const [bootError, setBootError] = useState<string | null>(null);
  const [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [booting, setBooting] = useState(true);
  const [showSettings, setShowSettings] = useState(false);
  const [menu, setMenu] = useState<MenuState | null>(null);
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [showBrowser, setShowBrowser] = useState(false);
  const [showTerm, setShowTerm] = useState(false);
  const [showSessionCfg, setShowSessionCfg] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(
    () => localStorage.getItem(LS.sidebarOpen) !== "0",
  );
  const [sidebarW, setSidebarW] = useState(() => loadPx(LS.sidebar, SIDEBAR.def));
  const [drawerW, setDrawerW] = useState(() => loadPx(LS.drawer, drawerDefault()));
  const [termH, setTermH] = useState(() => loadPx(LS.term, TERM.def));
  const [fullscreen, setFullscreen] = useState(false);
  /** 正在拖的分隔线：按下时的尺寸 + 拖动中的最新值（onEnd 写盘用）。
   *  同一时刻只可能拖一条线，所以三条线共用这一对 ref。 */
  const dragFrom = useRef(0);
  const dragLive = useRef(0);

  useEffect(() => subscribeFullscreen(setFullscreen), []);

  const toggleSidebar = useCallback(() => {
    setSidebarOpen((v) => {
      localStorage.setItem(LS.sidebarOpen, v ? "0" : "1");
      return !v;
    });
  }, []);

  useEffect(() => {
    // 必须 catch。没有它的话，任何一次失败都表现为永远停在"启动中" ——
    // 而那种状态不给用户任何可操作的信息，是最糟的一种失败。
    getConfig()
      .then(async (c) => {
        setConfig(c);
        // 宿主还活着的话（前端刷新、HMR），把它内存里的会话捞回来。
        const live = await listSessions().catch(() => [] as SessionInfo[]);
        setSessions(live);
        const last = live[live.length - 1];
        if (last) setActive(last.id);
      })
      .catch((e: unknown) => setBootError(String(e)))
      .finally(() => setBooting(false));
  }, []);

  const projects = config?.config.projects ?? [];
  const activeSession = sessions.find((s) => s.id === active) ?? null;

  // 窗口标题跟随项目。多窗口/多桌面时，标题栏是用户分辨"哪个是哪个"
  // 的唯一线索。
  useEffect(() => {
    const name = activeSession?.root.split("/").pop();
    setWindowTitle(name ? `${name} — Riot` : "Riot").catch(() => {});
  }, [activeSession?.root]);

  const newSession = useCallback(async (root: string) => {
    try {
      const info = await createSession(root);
      setSessions((prev) => [...prev, info]);
      setActive(info.id);
    } catch (e) {
      setBootError(String(e));
    }
  }, []);

  const openProject = useCallback(async () => {
    const dir = await pickDirectory();
    if (!dir) return;
    try {
      const root = await addProject(dir);
      // 宿主更新了 projects 列表，重新拉一份而不是本地拼 —— 规范化
      // 和去重的规则在宿主那边，两边各写一遍迟早不一致。
      setConfig(await getConfig());
      await newSession(root);
    } catch (e) {
      setBootError(String(e));
    }
  }, [newSession]);

  /** 会话发出第一条消息后补标题。宿主的 title 来自历史，UI 上要即时。 */
  const onFirstMessage = useCallback((sessionId: string, text: string) => {
    setSessions((prev) =>
      prev.map((s) =>
        s.id === sessionId && !s.title ? { ...s, title: text.slice(0, 40) } : s,
      ),
    );
  }, []);

  /** 会话设置提交成功后回写列表。listSessions 只在启动时拉一次，
   *  不回写的话，弹窗关掉再打开显示的就是旧值。 */
  const patchSession = useCallback((id: string, patch: Partial<SessionInfo>) => {
    setSessions((prev) => prev.map((s) => (s.id === id ? { ...s, ...patch } : s)));
  }, []);

  /* ── 会话 / 项目操作 ──────────────────────── */

  const doDeleteSession = async (id: string) => {
    await deleteSession(id);
    const victim = sessions.find((s) => s.id === id);
    const next = sessions.filter((s) => s.id !== id);
    setSessions(next);
    if (active === id) {
      // 优先切到同项目最近的会话；没有就全局最近；再没有就回欢迎页
      const sibling = victim
        ? [...next].reverse().find((s) => s.root === victim.root)
        : undefined;
      setActive((sibling ?? next[next.length - 1])?.id ?? null);
    }
  };

  const doRename = async (id: string, title: string) => {
    setRenaming(null);
    try {
      await renameSession(id, title);
      // 空标题会回退到"第一条消息"，那个值只有宿主知道 —— 重新拉列表
      // 而不是本地猜
      setSessions(await listSessions());
    } catch (e) {
      setBootError(String(e));
    }
  };

  const doRemoveProject = async (root: string) => {
    const closed = await removeProject(root);
    setConfig(await getConfig());
    const next = sessions.filter((s) => s.root !== root && !closed.includes(s.id));
    setSessions(next);
    const activeGone =
      active !== null &&
      (closed.includes(active) || sessions.find((s) => s.id === active)?.root === root);
    if (activeGone) setActive(next[next.length - 1]?.id ?? null);
  };

  const sessionMenu = (e: React.MouseEvent, s: SessionInfo) => {
    e.preventDefault();
    e.stopPropagation();
    setMenu({
      x: e.clientX,
      y: e.clientY,
      entries: [
        {
          label: "重命名",
          action: () => {
            // 改名的输入框在侧栏里。从顶栏标题进来的时候侧栏可能是
            // 收起的 —— 不展开的话点了像没反应。
            setSidebarOpen(true);
            setRenaming(s.id);
          },
        },
        {
          label: "删除会话",
          danger: true,
          action: () =>
            setConfirm({
              title: "删除这个会话？",
              body: `「${s.title ?? "新会话"}」的历史会丢失。`,
              confirmLabel: "删除",
              action: () => void doDeleteSession(s.id).catch((err: unknown) => setBootError(String(err))),
            }),
        },
      ],
    });
  };

  const projectMenu = (e: React.MouseEvent, root: string) => {
    e.preventDefault();
    e.stopPropagation();
    const name = root.split("/").pop() || root;
    const count = sessions.filter((s) => s.root === root).length;
    setMenu({
      x: e.clientX,
      y: e.clientY,
      entries: [
        { label: "新会话", action: () => void newSession(root) },
        { label: "在访达中显示", action: () => void revealInFinder(root) },
        {
          label: "复制路径",
          action: () => void navigator.clipboard.writeText(root),
        },
        {
          label: "从列表移除",
          danger: true,
          action: () =>
            setConfirm({
              title: `移除 ${name}？`,
              body:
                count > 0
                  ? `下面 ${count} 个会话会被关闭。目录不会被删除。`
                  : "目录不会被删除。",
              confirmLabel: "移除",
              action: () => void doRemoveProject(root).catch((err: unknown) => setBootError(String(err))),
            }),
        },
      ],
    });
  };

  if (bootError) {
    return (
      <div className="boot-fail">
        <h1>出错了</h1>
        <pre className="boot-error">{bootError}</pre>
        <button className="primary" onClick={() => window.location.reload()}>
          重新加载
        </button>
      </div>
    );
  }

  if (booting || !config) {
    return <div className="booting">{booting ? "" : "启动中…"}</div>;
  }

  return (
    <div className="shell" data-fullscreen={fullscreen ? "" : undefined}>
      {sidebarOpen ? (
        <>
          <Sidebar
            width={sidebarW}
            projects={projects}
            sessions={sessions}
            active={active}
            renaming={renaming}
            onSelect={setActive}
            onNewSession={newSession}
            onOpenProject={openProject}
            onSettings={() => setShowSettings(true)}
            onSessionMenu={sessionMenu}
            onProjectMenu={projectMenu}
            onRenameSubmit={doRename}
            onRenameCancel={() => setRenaming(null)}
          />
          <Resizer
            axis="x"
            onStart={() => {
              dragFrom.current = sidebarW;
              dragLive.current = sidebarW;
            }}
            onDelta={(d) => {
              const w = clamp(dragFrom.current + d, SIDEBAR.min, SIDEBAR.max);
              dragLive.current = w;
              setSidebarW(w);
            }}
            onEnd={() => savePx(LS.sidebar, dragLive.current)}
            onReset={() => {
              setSidebarW(SIDEBAR.def);
              savePx(LS.sidebar, SIDEBAR.def);
            }}
          />
        </>
      ) : null}

      <div className="main">
        <TopBar
          sidebarOpen={sidebarOpen}
          onToggleSidebar={toggleSidebar}
          session={activeSession}
          onSessionMenu={sessionMenu}
          browserOpen={showBrowser}
          browserEnabled={activeSession !== null}
          onToggleBrowser={() => setShowBrowser((v) => !v)}
          terminalOpen={showTerm}
          onToggleTerminal={() => setShowTerm((v) => !v)}
          sessionCfgOpen={showSessionCfg}
          sessionCfgEnabled={activeSession !== null}
          onToggleSessionCfg={() => setShowSessionCfg((v) => !v)}
        />

        <div className="workarea">
          <div className="chat-col">
            {activeSession ? (
              <Chat
                key={activeSession.id}
                sessionId={activeSession.id}
                config={config}
                workspace={activeSession.root}
                initialMode={activeSession.mode}
                onConfig={setConfig}
                onOpenSettings={() => setShowSettings(true)}
                onFirstMessage={onFirstMessage}
                onAgentBrowser={() => setShowBrowser(true)}
              />
            ) : (
              <Welcome
                projects={projects}
                onNewSession={newSession}
                onOpenProject={openProject}
              />
            )}
          </div>
        </div>

        {showTerm ? (
          <Resizer
            axis="y"
            onStart={() => {
              dragFrom.current = termH;
              dragLive.current = termH;
            }}
            onDelta={(d) => {
              // 终端拖的是上缘：往上（负位移）变高
              const h = clamp(
                dragFrom.current - d,
                TERM.min,
                Math.round(window.innerHeight * 0.65),
              );
              dragLive.current = h;
              setTermH(h);
            }}
            onEnd={() => savePx(LS.term, dragLive.current)}
            onReset={() => {
              setTermH(TERM.def);
              savePx(LS.term, TERM.def);
            }}
          />
        ) : null}
        {/* 常驻挂载：收起只是 display:none，shell 和回滚缓冲都留着。 */}
        <TerminalPanel
          visible={showTerm}
          height={termH}
          defaultRoot={activeSession?.root ?? projects[0] ?? null}
          onHide={() => setShowTerm(false)}
        />
      </div>

      {/* 抽屉是 main 的兄弟：整列全高（Codex 同款），terminal 只垫在对话
          下面。抽屉和对话共享同一个会话 —— 这正是它存在的意义：你和模型
          看同一个页面。 */}
      {activeSession && showBrowser ? (
        <>
          <Resizer
            axis="x"
            onStart={() => {
              dragFrom.current = drawerW;
              dragLive.current = drawerW;
            }}
            onDelta={(d) => {
              // 抽屉拖的是左缘：往左（负位移）变宽
              const w = clamp(
                dragFrom.current - d,
                DRAWER_MIN,
                Math.round(window.innerWidth * 0.7),
              );
              dragLive.current = w;
              setDrawerW(w);
            }}
            onEnd={() => savePx(LS.drawer, dragLive.current)}
            onReset={() => {
              const w = drawerDefault();
              setDrawerW(w);
              savePx(LS.drawer, w);
            }}
          />
          <div className="drawer" style={{ width: drawerW }}>
            <BrowserPanel
              key={activeSession.id}
              sessionId={activeSession.id}
              onClose={() => setShowBrowser(false)}
            />
            <ScopePanel sessionId={activeSession.id} />
          </div>
        </>
      ) : null}

      {showSettings ? (
        <Settings
          status={config}
          onStatus={setConfig}
          onClose={() => setShowSettings(false)}
          activeRoot={activeSession?.root ?? null}
        />
      ) : null}

      {activeSession && showSessionCfg ? (
        // key 让切换会话时弹窗重挂载，草稿不会串到另一个会话头上。
        <SessionSettings
          key={activeSession.id}
          session={activeSession}
          inherited={
            config.config.providers.find((p) => p.id === config.config.activeProvider)
              ?.sampling ?? {}
          }
          onPatch={(patch) => patchSession(activeSession.id, patch)}
          onClose={() => setShowSessionCfg(false)}
        />
      ) : null}

      {menu ? <ContextMenu menu={menu} onClose={() => setMenu(null)} /> : null}
      {confirm ? <ConfirmDialog c={confirm} onClose={() => setConfirm(null)} /> : null}
    </div>
  );
}

/* ── 顶部工具栏 ─────────────────────────────── */

/**
 * 主区顶部的工具栏（照 Codex）：左边收放侧栏，中间是当前会话的标题
 * （点开就是会话菜单），右边是面板开关和会话设置。整条都是窗口拖拽区。
 */
function TopBar({
  sidebarOpen,
  onToggleSidebar,
  session,
  onSessionMenu,
  browserOpen,
  browserEnabled,
  onToggleBrowser,
  terminalOpen,
  onToggleTerminal,
  sessionCfgOpen,
  sessionCfgEnabled,
  onToggleSessionCfg,
}: {
  sidebarOpen: boolean;
  onToggleSidebar: () => void;
  session: SessionInfo | null;
  onSessionMenu: (e: React.MouseEvent, s: SessionInfo) => void;
  browserOpen: boolean;
  /** 浏览器抽屉跟着会话走，没有会话时开关置灰。 */
  browserEnabled: boolean;
  onToggleBrowser: () => void;
  terminalOpen: boolean;
  onToggleTerminal: () => void;
  sessionCfgOpen: boolean;
  /** 会话设置管的是单个会话的参数，没有会话时置灰。 */
  sessionCfgEnabled: boolean;
  onToggleSessionCfg: () => void;
}) {
  // 侧栏收起后 macOS 的红绿灯悬在主区左上角，工具栏给它们让位。
  // 全屏没有红绿灯（见 shell[data-fullscreen]），Windows/Linux 的窗口
  // 按钮在右上且不在 webview 里，都不用让。
  const padTraffic = !sidebarOpen && navigator.userAgent.includes("Mac");
  return (
    <header className={padTraffic ? "topbar pad-traffic" : "topbar"} data-tauri-drag-region>
      <button
        className={sidebarOpen ? "tb-btn active" : "tb-btn"}
        onClick={onToggleSidebar}
        title={sidebarOpen ? "收起侧边栏" : "展开侧边栏"}
        aria-label={sidebarOpen ? "收起侧边栏" : "展开侧边栏"}
      >
        <SidebarToggleIcon />
      </button>

      {session ? (
        <button
          className="tb-title"
          onClick={(e) => onSessionMenu(e, session)}
          title={session.root}
        >
          <span className="tb-title-text">{session.title ?? "新会话"}</span>
          <span className="tb-caret">▾</span>
        </button>
      ) : null}

      <div className="tb-spacer" data-tauri-drag-region />

      <button
        className={browserOpen ? "tb-btn active" : "tb-btn"}
        onClick={onToggleBrowser}
        disabled={!browserEnabled}
        title={browserEnabled ? "浏览器抽屉" : "先打开一个会话再用浏览器"}
        aria-label="浏览器抽屉"
      >
        <PanelRightIcon />
      </button>
      <button
        className={terminalOpen ? "tb-btn active" : "tb-btn"}
        onClick={onToggleTerminal}
        title="终端面板"
        aria-label="终端面板"
      >
        <PanelBottomIcon />
      </button>
      <button
        className={sessionCfgOpen ? "tb-btn active" : "tb-btn"}
        onClick={onToggleSessionCfg}
        disabled={!sessionCfgEnabled}
        title={sessionCfgEnabled ? "会话设置" : "先打开一个会话"}
        aria-label="会话设置"
      >
        <GearIcon />
      </button>
    </header>
  );
}

/* ── 拖拽分隔线 ─────────────────────────────── */

/**
 * 面板之间的拖拽分隔线。
 *
 * 拖动用 pointer capture：按下之后事件全部路由到这条线上，滑得再快、
 * 划出面板都不丢 —— 挂 window mousemove 的写法会和浏览器面板的鼠标
 * 转发打架（拖着拖着页面开始收到 move 事件）。双击回到默认尺寸。
 */
function Resizer({
  axis,
  onStart,
  onDelta,
  onEnd,
  onReset,
}: {
  axis: "x" | "y";
  onStart: () => void;
  /** 相对按下点的位移，往右/往下为正。方向语义由调用方决定。 */
  onDelta: (d: number) => void;
  onEnd: () => void;
  onReset: () => void;
}) {
  const [dragging, setDragging] = useState(false);
  return (
    <div
      className={`rz ${axis === "x" ? "rz-x" : "rz-y"}${dragging ? " dragging" : ""}`}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        const from = axis === "x" ? e.clientX : e.clientY;
        onStart();
        setDragging(true);
        // capture 能防止快速拖动时指针飘出条外丢事件；但监听挂 window ——
        // 分隔条只有几像素宽，capture 万一失败（或 pointercancel 边界）
        // 也不能让拖拽卡死在"dragging"状态。
        try {
          e.currentTarget.setPointerCapture(e.pointerId);
        } catch {
          /* 没有活跃指针（如合成事件）时会抛，忽略 */
        }
        const move = (ev: PointerEvent) =>
          onDelta((axis === "x" ? ev.clientX : ev.clientY) - from);
        const up = () => {
          window.removeEventListener("pointermove", move);
          window.removeEventListener("pointerup", up);
          window.removeEventListener("pointercancel", up);
          setDragging(false);
          onEnd();
        };
        window.addEventListener("pointermove", move);
        window.addEventListener("pointerup", up);
        window.addEventListener("pointercancel", up);
      }}
      onDoubleClick={onReset}
      title="拖动调整大小，双击恢复默认"
    />
  );
}

/** 全局唯一的上下文菜单。透明遮罩负责"点外面关闭"。 */
function ContextMenu({ menu, onClose }: { menu: MenuState; onClose: () => void }) {
  useEffect(() => {
    const esc = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  }, [onClose]);

  // 贴近视口底部时往上顶，别让菜单被截掉
  const top = Math.min(menu.y, window.innerHeight - menu.entries.length * 34 - 16);

  return (
    <div
      className="ctx-backdrop"
      onMouseDown={onClose}
      onContextMenu={(e) => {
        e.preventDefault();
        onClose();
      }}
    >
      <div
        className="ctx-menu"
        style={{ left: menu.x, top }}
        onMouseDown={(e) => e.stopPropagation()}
      >
        {menu.entries.map((en) => (
          <button
            key={en.label}
            className={en.danger ? "ctx-item danger" : "ctx-item"}
            onClick={() => {
              onClose();
              en.action();
            }}
          >
            {en.label}
          </button>
        ))}
      </div>
    </div>
  );
}

/* ── 侧边栏 ─────────────────────────────────── */

interface SidebarProps {
  /** 用户拖出来的宽度。真值和持久化都在 App 那层。 */
  width: number;
  projects: string[];
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

function Sidebar(props: SidebarProps) {
  const { width, projects, sessions, onOpenProject, onSettings } = props;

  // 有会话但不在项目列表里的根也要显示（理论上不会发生，但真发生时
  // 隐藏会话比多显示一个组糟得多）。
  const roots = [...projects];
  for (const s of sessions) {
    if (!roots.includes(s.root)) roots.push(s.root);
  }

  return (
    <aside className="sidebar" style={{ width }}>
      {/* macOS 红绿灯占左上角，这块留空且可拖动 */}
      <div className="traffic-space" data-tauri-drag-region />

      <button className="new-thread" onClick={onOpenProject}>
        <PlusIcon />
        打开项目…
      </button>

      <nav className="threads">
        {roots.length ? <div className="group-caption">项目</div> : null}
        {roots.map((root) => (
          <ProjectGroup
            key={root}
            {...props}
            root={root}
            sessions={sessions.filter((s) => s.root === root)}
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

function ProjectGroup(props: SidebarProps & { root: string }) {
  const {
    root,
    sessions,
    active,
    renaming,
    onSelect,
    onNewSession,
    onSessionMenu,
    onProjectMenu,
    onRenameSubmit,
    onRenameCancel,
  } = props;
  const name = root.split("/").pop() || root;
  // 最近的在上面
  const ordered = [...sessions].sort((a, b) => b.seq - a.seq);

  return (
    <div className="project">
      <div className="project-head" title={root} onContextMenu={(e) => onProjectMenu(e, root)}>
        <FolderIcon />
        <span className="project-name">{name}</span>
        <button
          className="row-btn"
          onClick={() => onNewSession(root)}
          title={`在 ${name} 开新会话`}
        >
          <PlusIcon />
        </button>
        <button className="row-btn" onClick={(e) => onProjectMenu(e, root)} title="项目操作">
          <DotsIcon />
        </button>
      </div>

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
              {s.title ?? "新会话"}
            </button>
            <button className="row-btn" onClick={(e) => onSessionMenu(e, s)} title="会话操作">
              <DotsIcon />
            </button>
          </div>
        ),
      )}
    </div>
  );
}

/**
 * 欢迎页的插画。
 *
 * 手写 SVG 而不是位图：描边直接引用主题变量，换主题不用重新导出；
 * 任何 DPI 都锐利，不用准备 @2x/@3x；整体不到 2KB，不进构建产物的
 * 资源表。位图在这三件事上都要额外维护，而它换来的表现力这里用不上。
 */
function WelcomeArt() {
  return (
    <svg className="welcome-art" viewBox="0 0 200 128" fill="none" aria-hidden>
      {/* 这里曾经有一圈径向渐变光晕。删掉了：在纯色深底上，大面积的
          低不透明度柔光会因为 8 位色深出现色带，看起来像一块脏斑而不是
          发光。这套界面是平的、低对比的，本来就没有光源可言。 */}

      {/* 往后叠的两层：会话堆在同一个项目下 */}
      <rect
        x="54" y="18" width="92" height="58" rx="9"
        stroke="var(--border-strong)" strokeWidth="1.5" opacity="0.5"
      />
      <rect
        x="44" y="27" width="112" height="64" rx="10"
        fill="var(--bg)" stroke="var(--border-strong)" strokeWidth="1.5" opacity="0.85"
      />

      {/* 最前面那层：当前会话 */}
      <rect
        x="32" y="36" width="136" height="72" rx="11"
        fill="var(--bg-card)" stroke="var(--border-strong)" strokeWidth="1.5"
      />
      {/* 顶边一道高光，让最前面这层有厚度 */}
      <path d="M43.5 36.75h113" stroke="var(--text)" strokeWidth="1.2" opacity="0.07" />
      <path d="M32 51h136" stroke="var(--border)" strokeWidth="1.5" />
      <circle cx="43" cy="43.5" r="1.75" fill="var(--text-faint)" opacity="0.7" />
      <circle cx="50" cy="43.5" r="1.75" fill="var(--text-faint)" opacity="0.5" />
      <circle cx="57" cy="43.5" r="1.75" fill="var(--text-faint)" opacity="0.35" />

      {/* 提示符 + 三行"代码" */}
      <path
        d="M44 62.5l4 3-4 3" stroke="var(--text-faint)"
        strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round"
      />
      <g fill="var(--text-faint)">
        <rect x="55" y="63.5" width="48" height="4" rx="2" opacity="0.5" />
        <rect x="44" y="78" width="74" height="4" rx="2" opacity="0.3" />
        <rect x="44" y="92" width="38" height="4" rx="2" opacity="0.3" />
      </g>
      {/* 光标。唯一会动的东西，一点生气就够了 */}
      <rect className="wa-caret" x="86" y="90" width="2.5" height="8" rx="1.25" fill="var(--ok)" />
    </svg>
  );
}

/** 欢迎页最多列几个最近项目。再多就该去侧边栏找了。 */
const RECENT_LIMIT = 4;

function Welcome({
  projects,
  onNewSession,
  onOpenProject,
}: {
  projects: string[];
  onNewSession: (root: string) => void;
  onOpenProject: () => void;
}) {
  const recent = projects.slice(0, RECENT_LIMIT);

  return (
    <div className="welcome">
      <WelcomeArt />
      <h1>Riot</h1>
      <p>每个会话绑定一个项目目录。</p>

      {/* 按钮标签只放短动词。之前这里是「在 codeTest 开新会话」——
          把一句话塞进按钮，目录名还在中间，名字一长按钮就跟着变形。
          项目本身是数据，该列出来让人挑，不该编进标签里。 */}
      <button className="primary big" onClick={onOpenProject}>
        打开目录…
      </button>

      {recent.length > 0 ? (
        <div className="recent">
          <div className="recent-label">最近</div>
          {recent.map((root) => (
            <button key={root} className="recent-row" onClick={() => onNewSession(root)}>
              <FolderIcon />
              <span className="recent-name">{root.split("/").pop()}</span>
              {/* 只显示父目录。完整路径的最后一段就是左边那个名字，
                  重复一遍既占地方又要截断。 */}
              <span className="recent-path">{tildify(parentOf(root))}</span>
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

/** 把家目录换成 `~`。完整的 /Users/xxx 前缀在每一行重复，只是噪音。 */
function tildify(p: string): string {
  const m = /^\/Users\/[^/]+/.exec(p);
  return m ? `~${p.slice(m[0].length)}` || "~" : p;
}

function parentOf(p: string): string {
  const i = p.lastIndexOf("/");
  return i > 0 ? p.slice(0, i) : "/";
}

/* ── 对话 ───────────────────────────────────── */

function Chat({
  sessionId,
  config,
  workspace,
  initialMode,
  onConfig,
  onOpenSettings,
  onFirstMessage,
  onAgentBrowser,
}: {
  sessionId: string;
  config: ConfigStatus;
  workspace: string;
  initialMode: PermissionMode;
  onConfig: (s: ConfigStatus) => void;
  onOpenSettings: () => void;
  onFirstMessage: (sessionId: string, text: string) => void;
  /** 模型调用浏览器工具时打开右侧抽屉，让用户看见同一页。 */
  onAgentBrowser?: () => void;
}) {
  const session = useSession(
    sessionId,
    onAgentBrowser ? { onBrowserOpen: onAgentBrowser } : undefined,
  );
  const empty =
    session.items.length === 0 && !session.streaming && !session.thinking;

  // 计划批准走对话流里的内联卡（计划是要读的文档，长在对话里；
  // 弹窗会在等了很久之后突然糊脸）；其余权限询问保持弹窗。
  const isPlanAsk = (a: (typeof session.asks)[number]) =>
    a.detail.suggestions.some((s) => s.type === "set_mode");
  const planAsk = session.asks.find(isPlanAsk);
  const modalAsk = session.asks.find((a) => !isPlanAsk(a));

  const send = (
    text: string,
    images: ImageInput[] = [],
    refs: string[] = [],
  ): Promise<boolean> => {
    onFirstMessage(sessionId, text);
    return session.send(text, images, refs);
  };

  const composer = (
    <Composer
      sessionId={sessionId}
      workspace={workspace}
      busy={session.busy}
      config={config}
      onConfig={onConfig}
      initialMode={initialMode}
      hostMode={session.hostMode}
      tokens={session.tokens}
      queued={session.queued}
      onQueueDelete={session.queueDelete}
      onQueueEdit={session.queueEdit}
      onQueueSendNow={session.queueSendNow}
      onSend={send}
      onStop={session.stop}
      onOpenSettings={onOpenSettings}
    />
  );

  return (
    <div className="chat">
      {empty ? (
        <div className="hero">
          <h1 className="hero-title">今天做点什么？</h1>
          <p className="hero-ws" title={workspace}>
            <FolderIcon /> {workspace}
          </p>
          {composer}
        </div>
      ) : (
        <>
          <Transcript
            items={session.items}
            streaming={session.streaming}
            thinking={session.thinking}
            streamingPlan={session.streamingPlan}
            busy={session.busy}
            compacting={session.compacting}
            {...(planAsk ? { planAsk } : {})}
            onAnswerPlan={(r) => planAsk && void session.answer(r, planAsk.requestId)}
          />
          <div className="composer-dock">{composer}</div>
        </>
      )}

      {modalAsk ? (
        // key 让每个请求拿到全新的弹窗实例：并发的两个请求先后弹出时，
        // 第一个里勾的"总是允许"不会残留到第二个上。
        <PermissionDialog
          key={modalAsk.requestId}
          ask={modalAsk.detail}
          pendingCount={session.asks.length}
          onAnswer={(r) => void session.answer(r, modalAsk.requestId)}
        />
      ) : null}
    </div>
  );
}

function Transcript({
  items,
  streaming,
  thinking,
  streamingPlan,
  busy,
  compacting,
  planAsk,
  onAnswerPlan,
}: {
  items: Item[];
  streaming: string;
  thinking: string;
  streamingPlan: string | null;
  busy: boolean;
  /** 宿主正在压缩上下文。见 useSession 里同名字段。 */
  compacting: boolean;
  /** 待批准的计划（ExitPlanMode 的询问）。内联在对话流末尾。 */
  planAsk?: { requestId: string; detail: PermissionAsk };
  onAnswerPlan?: (r: PermissionResponse) => void;
}) {
  const endRef = useRef<HTMLDivElement>(null);
  const boxRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  // 只在用户本来就贴着底部时才自动滚。他往上翻着看历史的时候把他拽回来，
  // 是聊天界面里最招人烦的一件事。
  useEffect(() => {
    const box = boxRef.current;
    if (!box) return;
    const onScroll = () => {
      stick.current = box.scrollHeight - box.scrollTop - box.clientHeight < 80;
    };
    box.addEventListener("scroll", onScroll);
    return () => box.removeEventListener("scroll", onScroll);
  }, []);

  useEffect(() => {
    if (stick.current) endRef.current?.scrollIntoView({ block: "end" });
  }, [items, streaming, thinking, streamingPlan, planAsk?.requestId]);

  return (
    <main className="transcript" ref={boxRef}>
      <div className="thread-col">
        {items.map((it) => (
          <Row key={it.id} item={it} />
        ))}

        {thinking ? <ThinkingBlock text={thinking} live /> : null}
        {streaming ? (
          <div className="msg assistant">
            <Markdown text={streaming} />
          </div>
        ) : null}
        {/* 计划边写边显示，批准卡到手后再换成带按钮的那张。 */}
        {streamingPlan !== null && !planAsk ? <PlanDraft text={streamingPlan} /> : null}
        {/* 计划批准卡长在对话流里，跟在 ExitPlanMode 的工具卡后面 ——
            计划是要读的文档，弹窗会在等了很久之后突然糊脸。 */}
        {planAsk && onAnswerPlan ? (
          <PlanApprovalCard
            key={planAsk.requestId}
            ask={planAsk.detail}
            onAnswer={onAnswerPlan}
          />
        ) : null}
        {/*
         * 压缩中的提示优先于其他一切，而且不看 busy。
         *
         * 不看 busy 是因为手动 `/compact` 不占 busy（那条路不开轮次）。
         * 优先是因为它回答的正是"为什么在等" —— 反应式压缩发生在一轮
         * 中间，那时屏幕上可能还留着上一段流式文本，光秃秃的三个点
         * 混在里面看不出和平时有什么不同。
         */}
        {compacting ? (
          <Dots label="正在压缩上下文…" />
        ) : busy && !streaming && !thinking && streamingPlan === null && !planAsk ? (
          <Dots />
        ) : null}

        <div ref={endRef} />
      </div>
    </main>
  );
}

/**
 * memo：流式输出时 Transcript 每帧重渲染，历史条目不该跟着刷。
 * items 数组里未变化的元素引用是稳定的（更新走的是替换单个元素），
 * 所以浅比较有效。
 */
const Row = memo(function Row({ item }: { item: Item }) {
  switch (item.kind) {
    case "user":
      // 用户输入按原文显示，不走 markdown —— 渲染会篡改他说的话
      return (
        <div className="msg user">
          {/* 自己附的图要看得见。不回显的话，发完之后附件条一清空，
              用户就再也确认不了刚才发出去的是哪张。 */}
          {item.images?.length ? (
            <div className="msg-images">
              {item.images.map((src, i) => (
                <img key={i} src={src} alt="" />
              ))}
            </div>
          ) : null}
          <UserText text={item.text} {...(item.files ? { files: item.files } : {})} />
        </div>
      );
    case "assistant":
      return (
        <div className="msg assistant">
          <Markdown text={item.text} />
          <CopyMsg text={item.text} />
        </div>
      );
    case "thinking":
      return <ThinkingBlock text={item.text} />;
    case "tool":
      return <ToolCard tool={item} />;
    case "error":
      return <div className="msg error">{item.text}</div>;
    case "notice":
      return <div className="msg notice">{item.text}</div>;
  }
});

/**
 * 思考过程：写完的默认折叠（过程不是结论，铺开会把回答挤走）。
 * 正在流的那条默认展开 —— 只显示「思考中… N 字」的话，跟卡住没法区分。
 */
function ThinkingBlock({ text, live }: { text: string; live?: boolean }) {
  const [open, setOpen] = useState(Boolean(live));
  const bodyRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!live || !open) return;
    const el = bodyRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [text, live, open]);

  return (
    <div className={live ? "think-block live" : "think-block"}>
      <button type="button" className="think-head" onClick={() => setOpen(!open)}>
        <span className="think-icon">{open ? "▾" : "▸"}</span>
        {live ? "思考中…" : "思考过程"}
        <span className="think-chars">{text.length} 字</span>
      </button>
      {open ? (
        <div className="think-body" ref={bodyRef}>
          {text}
        </div>
      ) : null}
    </div>
  );
}

/** 悬停出现的整条复制。答案经常要贴回代码或文档，别让用户拖选。 */
function CopyMsg({ text }: { text: string }) {
  const [done, setDone] = useState(false);
  return (
    <button
      type="button"
      className="msg-copy"
      title="复制原文"
      onClick={() => {
        void navigator.clipboard.writeText(text);
        setDone(true);
        setTimeout(() => setDone(false), 1500);
      }}
    >
      {done ? "已复制" : "复制"}
    </button>
  );
}

/**
 * 等待指示。
 *
 * `label` 说明这次等的是什么。没有它的时候（模型正在应答）不需要文字 ——
 * 那是最常见的等待，用户认得。有具体原因时必须写出来:同一个动画表示
 * 两件事，用户只能按最常见的那个理解。
 */
function Dots({ label }: { label?: string }) {
  const dots = (
    <div className="dots">
      <span />
      <span />
      <span />
    </div>
  );
  if (!label) return dots;
  return (
    <div className="wait-note" role="status">
      {dots}
      <span className="wait-note-text">{label}</span>
    </div>
  );
}

/* ── 输入框 ─────────────────────────────────── */

const MODE_LABEL: Record<string, string> = {
  default: "每次询问",
  acceptEdits: "自动接受编辑",
  plan: "规划模式",
  bypassPermissions: "全部放行",
  unattended: "无人值守",
};

/** 菜单里跟在模式名后面的警示语。没有就是不需要提醒。 */
const MODE_WARN: Record<string, string> = {
  bypassPermissions: "风险自负",
  unattended: "含危险操作",
};

/**
 * 每个会话的未发送草稿。挂在模块级：Chat 按会话 id 重挂载，组件内
 * state 活不过切换 —— 用户打了一半的字换个会话再回来就没了，那是
 * 真实的内容损失。进程内存足够，不值得为草稿上持久化。
 */
/**
 * 输入框里的一段内容：一截文字，或一个文件引用块。
 *
 * 输入框是 contenteditable 而不是 textarea —— 引用块要和文字**排在
 * 同一行**（用户是在句子中间点名文件的："打开 [index.html] 看看"），
 * 而 textarea 只能装纯文本，块只能堆到框外面去，读起来就和正文脱节了。
 */
type Seg = { kind: "text"; value: string } | { kind: "ref"; value: string };

const drafts = new Map<string, Seg[]>();

const CHIP_ICON =
  '<svg width="11" height="11" viewBox="0 0 16 16" fill="none" aria-hidden="true">' +
  '<path d="M9 1.8H4.5a1 1 0 0 0-1 1v10.4a1 1 0 0 0 1 1h7a1 1 0 0 0 1-1V5.3L9 1.8z" ' +
  'stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/>' +
  '<path d="M8.9 2v3.4h3.4" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/></svg>';

/** 造一个引用块。`contenteditable=false` 让它在编辑器里是一个整体。 */
function chipEl(path: string): HTMLElement {
  const span = document.createElement("span");
  span.className = "ref-chip";
  span.contentEditable = "false";
  span.dataset.path = path;
  span.title = path;
  // 图标是常量 HTML；路径只走 textContent / dataset / title，不拼进 HTML。
  span.innerHTML = CHIP_ICON;
  span.appendChild(document.createTextNode(path.split("/").pop() ?? path));
  return span;
}

/** 把编辑区的 DOM 读成段落序列。 */
function readEditor(el: HTMLElement): Seg[] {
  const out: Seg[] = [];
  const push = (s: Seg) => {
    const last = out[out.length - 1];
    if (s.kind === "text" && last?.kind === "text") last.value += s.value;
    else if (s.kind !== "text" || s.value) out.push(s);
  };
  const walk = (node: Node, depth: number) => {
    let first = true;
    for (const child of Array.from(node.childNodes)) {
      if (child.nodeType === Node.TEXT_NODE) {
        push({ kind: "text", value: child.nodeValue ?? "" });
      } else if (child instanceof HTMLElement) {
        const path = child.dataset["path"];
        if (path) {
          push({ kind: "ref", value: path });
        } else if (child.tagName === "BR") {
          push({ kind: "text", value: "\n" });
        } else {
          // 浏览器在换行/粘贴时会包一层 div。除了第一层，块级元素的
          // 边界就是一个换行。
          if (depth > 0 || !first) push({ kind: "text", value: "\n" });
          walk(child, depth + 1);
        }
      }
      first = false;
    }
  };
  walk(el, 0);
  return out;
}

/** 用段落序列重建编辑区（切会话、发送失败回滚、程序化改写时用）。 */
function writeEditor(el: HTMLElement, segs: Seg[]) {
  el.replaceChildren();
  for (const s of segs) {
    if (s.kind === "text") el.appendChild(document.createTextNode(s.value));
    else el.appendChild(chipEl(s.value));
  }
}

/** 段落序列里的纯文字部分（补全菜单、空判断用）。 */
function segsText(segs: Seg[]): string {
  return segs.map((s) => (s.kind === "text" ? s.value : "")).join("");
}

/**
 * 段落序列 → 发出去的消息文本：引用块在**原位**留下 `@路径`。
 *
 * `[约束]` 不能把块的位置丢掉。"把 @a.css 的样式抄给 @b.css" 抹掉标记
 * 之后是"把 的样式抄给"，模型看到的是一句指代不明的话 —— 附件里有那
 * 两个文件也救不回来，它不知道谁抄给谁。顺带，界面重建气泡时也是靠
 * 这些标记把块画回原来的位置。
 */
function segsToPrompt(segs: Seg[]): string {
  return segs.map((s) => (s.kind === "text" ? s.value : mentionToken(s.value))).join("");
}

/** 路径带空格时要加引号，否则解析器会在空格处断开。 */
function mentionToken(path: string): string {
  return /\s/.test(path) ? `@"${path}"` : `@${path}`;
}

/** 把光标放到编辑区末尾。 */
function caretToEnd(el: HTMLElement) {
  const sel = window.getSelection();
  if (!sel) return;
  const r = document.createRange();
  r.selectNodeContents(el);
  r.collapse(false);
  sel.removeAllRanges();
  sel.addRange(r);
}

/** 光标前那个还没敲完的 `@查询`。没有就是 undefined（菜单不出）。 */
function queryAtCaret(el: HTMLElement): string | undefined {
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0) return undefined;
  const r = sel.getRangeAt(0);
  if (!el.contains(r.startContainer) || r.startContainer.nodeType !== Node.TEXT_NODE) {
    return undefined;
  }
  const before = (r.startContainer.nodeValue ?? "").slice(0, r.startOffset);
  return /(?:^|\s)@([^\s@]*)$/.exec(before)?.[1];
}

/**
 * 在光标处把 `@查询` 换成一个引用块。
 *
 * 光标停在块后面的空格上 —— 用户接着打字就是正常续写，不用再点一下
 * 输入框。
 */
function insertChipAtCaret(el: HTMLElement, path: string) {
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0) {
    el.appendChild(chipEl(path));
    el.appendChild(document.createTextNode(" "));
    caretToEnd(el);
    return;
  }
  const range = sel.getRangeAt(0);
  const node = range.startContainer;
  if (node.nodeType === Node.TEXT_NODE) {
    const before = (node.nodeValue ?? "").slice(0, range.startOffset);
    const m = /(^|\s)@[^\s@]*$/.exec(before);
    if (m) {
      const cut = before.length - (m[0].length - (m[1]?.length ?? 0));
      (node as Text).deleteData(cut, range.startOffset - cut);
      range.setStart(node, cut);
      range.collapse(true);
    }
  }
  const chip = chipEl(path);
  range.insertNode(chip);
  const space = document.createTextNode(" ");
  chip.after(space);
  const after = document.createRange();
  after.setStart(space, 1);
  after.collapse(true);
  sel.removeAllRanges();
  sel.addRange(after);
}

/** 去掉光标前那段 `@查询`（Esc 收起文件菜单时用）。 */
function dropQueryAtCaret() {
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0) return;
  const range = sel.getRangeAt(0);
  const node = range.startContainer;
  if (node.nodeType !== Node.TEXT_NODE) return;
  const before = (node.nodeValue ?? "").slice(0, range.startOffset);
  const m = /@[^\s@]*$/.exec(before);
  if (!m) return;
  const cut = before.length - m[0].length;
  (node as Text).deleteData(cut, range.startOffset - cut);
}

/**
 * 权限模式的 UI 缓存，理由同上，但它错了会出安全问题而不只是显示问题。
 *
 * Composer 在同一个会话里就会重挂载一次：发出第一条消息后 `empty` 翻转，
 * 它从 hero 区挪到 composer-dock，React 视作两个不同位置的组件，本地
 * state 全部丢弃。少了这层缓存，模式就退回全局默认值显示，而宿主那边
 * 还是用户选的那个 —— 屏幕上写着「每次询问」，实际每一步都在静默放行。
 */
const modeCache = new Map<string, PermissionMode>();

/** 待发的一张图。`data` 是 base64，不含 `data:` 前缀。 */
interface Shot {
  id: string;
  name: string;
  mediaType: string;
  data: string;
}

/**
 * 一条消息最多附几张图。
 *
 * 不是技术上限，是成本上限:每张图都要过一遍模型的视觉编码，五张已经能吃掉
 * 相当可观的一段上下文。真要看更多，分两条消息发更清楚。
 */
const MAX_SHOTS = 5;

/**
 * 缩到长边不超过这个值。
 *
 * 1568 是 Anthropic 文档给的"再大也不会更清楚"的门槛，两家的视觉编码都在
 * 这个量级上把图切成图块。粘一张 Retina 截图往往是 3000 多宽，缩一半之后
 * 体积掉到四分之一，而模型看到的信息一样多。
 */
const MAX_EDGE = 1568;

/** 认得出是图片的扩展名。拖进来的路径靠它分流。 */
const IMAGE_EXT = /\.(png|jpe?g|gif|webp)$/i;

/** 把 webview 的 `File` 读成待发的图。 */
async function toShot(file: File): Promise<Shot> {
  const buf = await file.arrayBuffer();
  return {
    id: `${file.name}-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
    name: file.name || "粘贴的图片",
    mediaType: file.type || "image/png",
    data: bytesToBase64(new Uint8Array(buf)),
  };
}

/**
 * 长边超了就缩，并统一转成 JPEG。
 *
 * 原图是 PNG 的截图尤其值得转:同样内容 JPEG 往往只有三分之一大，而模型
 * 判断的是布局和颜色，不是无损像素。
 *
 * 缩不动（canvas 用不了、图解不开）时原样返回 —— 有图比没图好。
 */
async function shrink(shot: Shot): Promise<Shot> {
  try {
    const img = new Image();
    img.src = `data:${shot.mediaType};base64,${shot.data}`;
    await img.decode();
    const edge = Math.max(img.naturalWidth, img.naturalHeight);
    if (edge <= MAX_EDGE) return shot;

    const scale = MAX_EDGE / edge;
    const canvas = document.createElement("canvas");
    canvas.width = Math.round(img.naturalWidth * scale);
    canvas.height = Math.round(img.naturalHeight * scale);
    const ctx = canvas.getContext("2d");
    if (!ctx) return shot;
    ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
    const url = canvas.toDataURL("image/jpeg", 0.85);
    const data = url.slice(url.indexOf(",") + 1);
    return { ...shot, mediaType: "image/jpeg", data };
  } catch {
    return shot;
  }
}

/**
 * 字节转 base64。
 *
 * 分块喂给 `String.fromCharCode`:一次展开几 MB 的数组会超过参数个数上限，
 * 表现是 `RangeError: too many arguments`，而那个报错完全不像"图太大"。
 */
function bytesToBase64(bytes: Uint8Array): string {
  const CHUNK = 0x8000;
  let binary = "";
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(binary);
}

/**
 * 用户消息的正文：把 `@路径` 标记画成引用块，其余原样。
 *
 * 用户在输入框里看到的是一行"分别打开 [a] [b]"，气泡里就该是同一行 ——
 * 把块抽出来堆到文字下面，等于把他写的句子拆了。
 */
function UserText({ text, files = [] }: { text: string; files?: string[] }) {
  if (files.length === 0) return <>{text}</>;

  const escaped = files.map((f) => f.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
  const re = new RegExp(`@"(?:${escaped.join("|")})"|@(?:${escaped.join("|")})`, "g");
  const out: React.ReactNode[] = [];
  const seen = new Set<string>();
  let last = 0;
  for (const m of text.matchAll(re)) {
    const path = m[0].replace(/^@"?/, "").replace(/"$/, "");
    if (m.index > last) out.push(text.slice(last, m.index));
    out.push(<FileChip key={`${path}-${m.index}`} path={path} />);
    seen.add(path);
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push(text.slice(last));

  // 正文里没留下标记的引用（老消息、命令展开出来的）照样要露出来，
  // 否则用户看不出这条消息带了什么文件。
  const orphans = files.filter((f) => !seen.has(f));
  return (
    <>
      {out}
      {orphans.map((p) => (
        <FileChip key={`orphan-${p}`} path={p} />
      ))}
    </>
  );
}

function FileChip({ path }: { path: string }) {
  return (
    <span className="ref-chip static" title={path}>
      <FileIcon />
      {path.split("/").pop()}
    </span>
  );
}

function FileIcon() {
  return (
    <svg width="11" height="11" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M9 1.8H4.5a1 1 0 0 0-1 1v10.4a1 1 0 0 0 1 1h7a1 1 0 0 0 1-1V5.3L9 1.8z"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinejoin="round"
      />
      <path d="M8.9 2v3.4h3.4" stroke="currentColor" strokeWidth="1.3" strokeLinejoin="round" />
    </svg>
  );
}

/**
 * 排队面板：模型跑动中发的插话停在这里（Cursor 同款交互），当前任务
 * **完全跑完**才自动发出、变成对话气泡 —— 中途不插队。想立刻处理就点
 * ↑（停止当前轮，优先发这条）；也可以撤回编辑、删除。
 */
function QueuePanel({
  queued,
  onEdit,
  onSendNow,
  onDelete,
}: {
  queued: QueuedItem[];
  onEdit: (id: string) => void;
  onSendNow: (id: string) => void;
  onDelete: (id: string) => void;
}) {
  const [open, setOpen] = useState(true);
  return (
    <div className="queue-panel">
      <button type="button" className="queue-head" onClick={() => setOpen((v) => !v)}>
        <span className={open ? "queue-chevron open" : "queue-chevron"}>
          <ChevronRightIcon />
        </span>
        {queued.length} 条排队
      </button>
      {open
        ? queued.map((q) => (
            <div className="queue-row" key={q.id}>
              <span className="queue-ring" aria-hidden />
              <span className="queue-text" title={q.text}>
                {q.text || "（仅图片）"}
              </span>
              {q.images.length > 0 ? (
                <span className="queue-imgs">{q.images.length} 图</span>
              ) : null}
              {q.refs.length > 0 ? (
                <span className="queue-imgs" title={q.refs.join("\n")}>
                  {q.refs.length} 文件
                </span>
              ) : null}
              <span className="queue-actions">
                <button
                  type="button"
                  title="编辑（放回输入框）"
                  aria-label="编辑"
                  onClick={() => onEdit(q.id)}
                >
                  <PencilIcon />
                </button>
                <button
                  type="button"
                  title="立即发送（停止当前轮，优先处理这条）"
                  aria-label="立即发送"
                  onClick={() => onSendNow(q.id)}
                >
                  <ArrowUpIcon />
                </button>
                <button type="button" title="删除" aria-label="删除" onClick={() => onDelete(q.id)}>
                  <TrashIcon />
                </button>
              </span>
            </div>
          ))
        : null}
    </div>
  );
}

function Composer({
  sessionId,
  workspace,
  busy,
  config,
  onConfig,
  initialMode,
  hostMode,
  tokens,
  queued,
  onQueueDelete,
  onQueueEdit,
  onQueueSendNow,
  onSend,
  onStop,
  onOpenSettings,
}: {
  sessionId: string;
  /** 会话的项目根。斜杠命令要按它找项目级 commands/。 */
  workspace: string;
  busy: boolean;
  config: ConfigStatus;
  onConfig: (s: ConfigStatus) => void;
  /** 宿主侧这个会话的当前模式，不是全局默认值。 */
  initialMode: PermissionMode;
  /** 宿主主动切的模式（批准计划）。null = 没发生过。 */
  hostMode: PermissionMode | null;
  tokens: { input: number; output: number };
  /** 排队面板：跑轮中发的、还没注入对话的插话。 */
  queued: QueuedItem[];
  onQueueDelete: (id: string) => void;
  onQueueEdit: (
    id: string,
  ) => Promise<{ text: string; images: ImageInput[]; refs: string[] } | null>;
  onQueueSendNow: (id: string) => void;
  /** 返回 false = 没发出去（hook 拦了、模型没配好），输入要放回输入框。 */
  onSend: (t: string, images: ImageInput[], refs: string[]) => Promise<boolean>;
  onStop: () => void;
  onOpenSettings: () => void;
}) {
  // 编辑区是**非受控**的：内容住在 DOM 里，这些 state 只是它的投影。
  // 受控写法（每次输入都回写 innerHTML）会在每一次按键后重置光标，
  // 中文输入法更是直接不能用。
  const [draft, setDraftRaw] = useState(() => segsText(drafts.get(sessionId) ?? []));
  const [mode, setMode] = useState<PermissionMode>(
    () => modeCache.get(sessionId) ?? initialMode,
  );

  // 宿主主动切换（批准计划时用户选的执行档）→ 界面跟上。
  // 回写 setPermissionMode 是为了把模式落进会话索引（幂等 —— 宿主
  // 内存里已经是这个值了，这一步只补持久化）。
  useEffect(() => {
    if (!hostMode) return;
    setMode(hostMode);
    modeCache.set(sessionId, hostMode);
    void setPermissionMode(sessionId, hostMode).catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hostMode, sessionId]);
  const [modeConfirm, setModeConfirm] = useState<ConfirmRequest | null>(null);
  /** 这个会话可用的斜杠命令。每次挂载拉一次（用户加了 .md 切一下会话就有）。 */
  const [commands, setCommands] = useState<SlashCommand[]>([]);
  /** 补全菜单里高亮到第几条。 */
  const [slashPick, setSlashPick] = useState(0);
  /** `@` 引用的候选文件。 */
  const [fileHits, setFileHits] = useState<string[]>([]);
  const [filePick, setFilePick] = useState(0);
  /** 光标前那个没敲完的 `@查询`。undefined = 不在引用语境里。 */
  const [mentionQuery, setMentionQuery] = useState<string | undefined>(undefined);
  /** 斜杠命令的执行反馈（压缩中、展开失败）。 */
  const [slashNote, setSlashNote] = useState("");
  /** 待发的图。发出去就清空。 */
  const [shots, setShots] = useState<Shot[]>([]);
  /**
   * 编辑区里的文件引用块（按出现顺序）。发出去就清空。
   *
   * `@wechat.html` 是给解析器看的写法，让用户对着它编辑（删一半、光标
   * 插在中间）只会把引用弄坏。块是一个整体：点 ✕ 或退格整个删掉。
   */
  const [refs, setRefs] = useState<string[]>([]);
  /** 拖/选进来失败的那一条。附件是"扔进去就走"的操作，不报的话用户以为成了。 */
  const [dropError, setDropError] = useState("");
  const [dragging, setDragging] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  // 中文 IME：确认候选/上屏英文时，keydown(Enter) 常在 compositionend 之后到达，
  // 此时 nativeEvent.isComposing 已是 false，会被误当成发送。用 ref 盖住这一拍。
  const imeRef = useRef(false);

  const cfg = config.config;
  const hasKey = hasActiveKey(config);
  const activeProvider =
    cfg.providers.find((p) => p.id === cfg.activeProvider) ?? cfg.providers[0] ?? null;

  // 内联切换：直接改激活的 provider/model 并回写配置。和设置页共用
  // 同一条 setConfig 通道，宿主 resolve 一次挡住坏状态。切 provider 时
  // 若当前模型不属于新家，跳到新家的第一个模型。
  const switchProvider = (p: ProviderConfig) => {
    if (p.id === cfg.activeProvider) return;
    const model = p.models.some((m) => m.id === cfg.activeModel)
      ? cfg.activeModel
      : (p.models[0]?.id ?? "");
    void saveConfig({ ...cfg, activeProvider: p.id, activeModel: model })
      .then(onConfig)
      .catch(() => {});
  };
  const switchModel = (m: string) => {
    if (m === cfg.activeModel) return;
    void saveConfig({ ...cfg, activeModel: m }).then(onConfig).catch(() => {});
  };

  useEffect(() => {
    let alive = true;
    void slashCommands(workspace)
      .then((c) => alive && setCommands(c))
      .catch(() => {}); // 没有命令目录不是错误
    return () => {
      alive = false;
    };
  }, [workspace]);

  /** 把编辑区当前的内容读进 state（每次输入、每次光标移动后调）。 */
  const sync = () => {
    const el = ref.current;
    if (!el) return;
    const segs = readEditor(el);
    const text = segsText(segs);
    const paths = segs.flatMap((s) => (s.kind === "ref" ? [s.value] : []));
    setDraftRaw(text);
    setRefs(paths);
    setMentionQuery(queryAtCaret(el));
    // 删光内容后浏览器常留一个 `<br>`，读出来是个 "\n"。当成有内容的话，
    // 占位提示不再出现、草稿缓存里也会存下一堆看不见的空行。
    if (text.trim() || paths.length) drafts.set(sessionId, segs);
    else drafts.delete(sessionId);
  };

  /** 程序化改写编辑区内容（清空、回滚、撤回排队项）。 */
  const setContent = (segs: Seg[]) => {
    const el = ref.current;
    if (!el) return;
    writeEditor(el, segs);
    caretToEnd(el);
    sync();
  };

  /**
   * 换掉文字、留下已有的块。
   *
   * `[约束]` 只在"整条文字都要被替换"时用（选中斜杠命令、Esc 清空）。
   * 别拿它做追加 —— 块会被重排到前面去，用户会看到自己刚插在句中的
   * 引用莫名其妙跳到了句首。要在光标处加东西用 `insertChipAtCaret`。
   */
  const replaceText = (v: string) => {
    const el = ref.current;
    if (!el) return;
    const keep = readEditor(el).filter((s) => s.kind === "ref");
    setContent(v ? [{ kind: "text", value: v }, ...keep] : keep);
  };

  // 切会话：编辑区是非受控的，组件复用时内容不会自己跟着换。
  // 顺带把焦点放进去 —— contenteditable 不吃 autoFocus（React 只对
  // 表单元素生效），少了这一步切完会话得先点一下才能打字。
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    writeEditor(el, drafts.get(sessionId) ?? []);
    el.focus();
    caretToEnd(el);
    sync();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  // 补全菜单只在"还没敲空格"时出：`/co` 出菜单，`/compact 参数` 不出 ——
  // 后者用户已经选定命令在写参数了，菜单只会挡住视线。
  const slashQuery = /^\/([\w:-]*)$/.exec(draft)?.[1];
  const matches =
    slashQuery === undefined
      ? []
      : commands
          .filter((c) => c.name.toLowerCase().includes(slashQuery.toLowerCase()))
          // 前缀匹配排在包含匹配前面（敲 `co` 时 `compact` 该在最上面）
          .sort((a, b) => {
            const q = slashQuery.toLowerCase();
            const ap = a.name.toLowerCase().startsWith(q) ? 0 : 1;
            const bp = b.name.toLowerCase().startsWith(q) ? 0 : 1;
            return ap - bp || a.name.localeCompare(b.name);
          })
          .slice(0, 8);
  const pick = Math.min(slashPick, Math.max(matches.length - 1, 0));

  /** 选中一条命令：填进输入框等用户敲参数（没有参数的直接可发）。 */
  const chooseSlash = (c: SlashCommand) => {
    replaceText(`/${c.name} `);
    setSlashPick(0);
    ref.current?.focus();
  };

  // `@` 文件引用：认的是**光标处**那个没敲完的 token（由 sync 算出来），
  // 所以在句子中间插引用也能用。
  useEffect(() => {
    if (mentionQuery === undefined) {
      setFileHits([]);
      return;
    }
    // 防抖：每敲一个字都问一次宿主，大仓库上菜单会跳。
    let alive = true;
    const t = setTimeout(() => {
      void searchFiles(sessionId, mentionQuery)
        .then((r) => alive && setFileHits(r))
        .catch(() => {});
    }, 60);
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [mentionQuery, sessionId]);

  const fileMatches = mentionQuery === undefined ? [] : fileHits;
  const fpick = Math.min(filePick, Math.max(fileMatches.length - 1, 0));

  /** 选中一个文件：把光标处的 `@查询` 换成一个块，就地插在句子里。 */
  const chooseFile = (p: string) => {
    const el = ref.current;
    if (!el) return;
    el.focus();
    insertChipAtCaret(el, p);
    setFilePick(0);
    sync();
  };

  const submit = () => {
    const text = draft.trim();
    // 只附了图/只挂了引用、什么都没打也算一条消息 —— "看这个截图"、
    // "看看这个文件"都是这么发的。
    // busy 不拦：模型干活时发的消息进排队面板，内核在安全点注入。
    if ((!text && shots.length === 0 && refs.length === 0) || !hasKey || !cfg.activeModel) return;

    // 斜杠命令：内置的当场执行，自定义的展开成 prompt 再走正常发送。
    // 认不出的 `/xxx` 原样发出去 —— 用户可能真想跟模型说这个词。
    const slash = /^\/([\w:-]+)\s*([\s\S]*)$/.exec(text);
    const cmd = slash ? commands.find((c) => c.name === slash[1]) : undefined;
    if (slash && cmd) {
      const args = slash[2] ?? "";
      const sentRefs = refs;
      setContent([]);
      setShots([]);
      void runSlash(cmd, args, sentRefs);
      return;
    }

    // 乐观清空，被拒了再放回来。清空是为了让"发出去了"这件事立刻可见；
    // 而拒绝路径上宿主既没收下消息、界面也撤掉了气泡 —— 不放回的话，
    // 用户刚打的那段字在两头都不存在了。
    const sent = shots;
    const sentSegs = ref.current ? readEditor(ref.current) : [];
    const sentRefs = refs;
    // 发出去的是**带标记**的文本：块在原位留下 `@路径`（见 segsToPrompt）。
    const prompt = segsToPrompt(sentSegs).trim();
    setContent([]);
    setShots([]);
    void onSend(
      prompt,
      sent.map(({ mediaType, data }) => ({ mediaType, data })),
      sentRefs,
    ).then((ok) => {
      if (ok) return;
      // 连块带字整段放回去（等待期间新打的接在后面）。
      const cur = ref.current ? readEditor(ref.current) : [];
      setContent([...sentSegs, ...cur]);
      setShots((prev) => [...sent, ...prev]);
    });
  };

  /**
   * 执行一条斜杠命令。
   *
   * 自定义命令展开成 prompt 后**当普通消息发出去** —— 模型看到的和
   * 对话流里显示的是同一段文字。藏起原文只会让"模型为什么这么答"
   * 变得无从追溯（切回会话时更是只剩展开结果）。
   */
  const runSlash = async (cmd: SlashCommand, args: string, sentRefs: string[] = []) => {
    if (cmd.source === "builtin") {
      if (cmd.name === "compact") {
        setSlashNote("正在压缩历史…");
        try {
          await compactSession(sessionId);
          setSlashNote("");
        } catch (e) {
          setSlashNote(String(e));
        }
      }
      return;
    }
    // 失败时把 `/命令 参数` 和引用块原样放回去：展开出来的 prompt 是
    // 派生物，用户手里那行才是他打的东西。
    const typed = args ? `/${cmd.name} ${args}` : `/${cmd.name}`;
    const restore = () => {
      const cur = ref.current ? readEditor(ref.current) : [];
      const back: Seg[] = sentRefs
        .filter((r) => !cur.some((s) => s.kind === "ref" && s.value === r))
        .map((value) => ({ kind: "ref", value }));
      setContent([{ kind: "text", value: `${typed} ` }, ...back, ...cur]);
    };
    try {
      const prompt = await slashExpand(sessionId, cmd.name, args);
      if (!prompt) {
        setSlashNote(`/${cmd.name} 展开失败：命令可能刚被删掉`);
        restore();
        return;
      }
      if (!(await onSend(prompt, [], sentRefs))) restore();
    } catch (e) {
      setSlashNote(String(e));
      restore();
    }
  };

  /** 把一条排队插话撤回输入框改。原有草稿接在它后面，谁都不丢。 */
  const editQueued = async (id: string) => {
    const input = await onQueueEdit(id);
    if (!input) return;
    const cur = ref.current ? readEditor(ref.current) : [];
    const back: Seg[] = input.refs
      .filter((r) => !cur.some((s) => s.kind === "ref" && s.value === r))
      .map((value) => ({ kind: "ref", value }));
    setContent([
      ...back,
      { kind: "text", value: segsText(cur).trim() ? `${input.text}\n` : input.text },
      ...cur,
    ]);
    if (input.images.length > 0) {
      setShots((prev) => [
        ...prev,
        ...input.images.map((img, i) => ({
          id: `q-${Date.now()}-${i}`,
          name: `排队图片 ${i + 1}`,
          mediaType: img.mediaType,
          data: img.data,
        })),
      ]);
    }
    ref.current?.focus();
  };

  /**
   * 收下一批图。
   *
   * 在前端先缩一遍:粘一张 Retina 截图动辄五六 MB，原样发过去要么撞服务方的
   * 单图上限，要么白烧一大截上下文 —— 而模型看布局用不到那个分辨率。
   */
  const addShots = async (items: { data: string; mediaType: string; name: string }[]) => {
    const scaled = await Promise.all(
      items.map((it) =>
        shrink({
          id: `${it.name}-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
          ...it,
        }),
      ),
    );
    setShots((prev) => [...prev, ...scaled].slice(0, MAX_SHOTS));
  };

  /** 拖进来或粘贴进来的 `File`:图片收下，其它的说清为什么不收。 */
  const takeFiles = async (files: File[]) => {
    const images = files.filter((f) => f.type.startsWith("image/"));
    const rest = files.filter((f) => !f.type.startsWith("image/"));

    if (images.length) {
      const read = await Promise.all(images.map(toShot));
      await addShots(read);
    }
    if (rest.length) {
      // webview 拿到的 `File` 没有磁盘路径（拖放数据里就没有），所以这条路
      // 只能收图片。非图片文件请走「+」按钮 —— 那条走系统对话框，回的是路径。
      setDropError(
        `${rest[0]?.name ?? "这个文件"} 不是图片。非图片文件请用左下角的「+」选择，` +
          `或者在输入框里打 @ 找它 —— 那两条能拿到路径。`,
      );
    }
  };

  /** 拖进来或从对话框选的路径:图片读成内容，其它的变成引用块。 */
  const takePaths = async (paths: string[]) => {
    const images = paths.filter((p) => IMAGE_EXT.test(p));
    const files = paths.filter((p) => !IMAGE_EXT.test(p));

    if (images.length) {
      const read = await Promise.all(
        images.map((p) => readImage(p).catch((e: unknown) => String(e))),
      );
      const ok = read.filter((r): r is Awaited<ReturnType<typeof readImage>> =>
        typeof r !== "string",
      );
      await addShots(ok);
      const failed = read.filter((r): r is string => typeof r === "string");
      if (failed.length) setDropError(failed[0] ?? "");
    }

    // 非图片文件走和 `@` 一样的引用块：都是"用户点名了这个文件"，
    // 没道理一个变成块、另一个变成一串裸路径。项目内的收成相对路径，
    // 块上只显示文件名，长路径不会把输入框撑变形。
    if (files.length) {
      const el = ref.current;
      if (el) {
        el.focus();
        for (const p of files) {
          insertChipAtCaret(el, p.startsWith(`${workspace}/`) ? p.slice(workspace.length + 1) : p);
        }
        sync();
      }
    }
  };

  const applyMode = (m: PermissionMode) => {
    const prev = mode;
    setMode(m);
    modeCache.set(sessionId, m);
    // 失败必须回滚到宿主的真实值。这里显示的是"它会不会问我"，
    // 显示成放行而实际在问只是啰嗦，反过来则是用户以为有人把关。
    setPermissionMode(sessionId, m).catch(() => {
      setMode(prev);
      modeCache.set(sessionId, prev);
    });
  };

  const changeMode = (m: PermissionMode) => {
    // 无人值守关掉的是最后一层保护，不能一次点击就生效。
    if (m === "unattended" && mode !== "unattended") {
      setModeConfirm({
        title: "切到无人值守？",
        body: "这个会话之后不会再有任何权限弹窗，包括危险操作。",
        confirmLabel: "确认切换",
        action: () => applyMode(m),
      });
      return;
    }
    applyMode(m);
  };

  return (
    <div className="composer-wrap">
      {/* 三种"还不能发消息"要分开说。都写成"还没有 API key"的话，
          一个服务方都没有的新用户会去找那个根本不存在的 key 输入框。 */}
      {cfg.providers.length === 0 ? (
        <button className="key-banner" onClick={onOpenSettings}>
          还没有配置服务方，点这里添加
        </button>
      ) : !hasKey ? (
        <button className="key-banner" onClick={onOpenSettings}>
          {activeProvider?.name ?? "当前服务方"}还没有 API key，点这里配置
        </button>
      ) : !cfg.activeModel ? (
        <button className="key-banner" onClick={onOpenSettings}>
          {activeProvider?.name ?? "当前服务方"}还没有选中模型，点这里配置
        </button>
      ) : null}

      {dropError ? (
        <button className="key-banner" onClick={() => setDropError("")} title="点击关闭">
          {dropError}
        </button>
      ) : null}

      {slashNote ? (
        <button className="key-banner" onClick={() => setSlashNote("")} title="点击关闭">
          {slashNote}
        </button>
      ) : null}

      {queued.length > 0 ? <QueuePanel queued={queued} onEdit={(id) => void editQueued(id)} onSendNow={onQueueSendNow} onDelete={onQueueDelete} /> : null}

      {matches.length > 0 ? (
        <div className="slash-menu">
          {matches.map((c, i) => (
            <button
              type="button"
              key={c.name}
              className={i === pick ? "slash-item active" : "slash-item"}
              // mousedown 而不是 click：click 之前 textarea 先失焦，
              // 焦点一跑菜单就关了，点击落空。
              onMouseDown={(e) => {
                e.preventDefault();
                chooseSlash(c);
              }}
              onMouseEnter={() => setSlashPick(i)}
            >
              <span className="slash-name">/{c.name}</span>
              {c.argumentHint ? <span className="slash-hint">{c.argumentHint}</span> : null}
              <span className="slash-desc">{c.description}</span>
              {c.source !== "builtin" ? (
                <span className="slash-src">{c.source === "project" ? "项目" : "全局"}</span>
              ) : null}
            </button>
          ))}
        </div>
      ) : null}

      {fileMatches.length > 0 && matches.length === 0 ? (
        <div className="slash-menu">
          {fileMatches.map((p, i) => (
            <button
              type="button"
              key={p}
              className={i === fpick ? "slash-item active" : "slash-item"}
              onMouseDown={(e) => {
                e.preventDefault();
                chooseFile(p);
              }}
              onMouseEnter={() => setFilePick(i)}
            >
              {/* 文件名在前、目录在后：一屏候选里先扫到的是名字。 */}
              <span className="slash-name">{p.split("/").pop()}</span>
              <span className="slash-desc">{p}</span>
            </button>
          ))}
        </div>
      ) : null}

      <form
        className={dragging ? "composer dragging" : "composer"}
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
        // 拖拽在 Tauri 里有两条路:窗口级的原生事件（给得到真实路径）和
        // webview 的 HTML5 事件。这里接后者，因为它能拿到从浏览器里直接拖
        // 过来的图片数据 —— 那种图在磁盘上没有路径。
        onDragOver={(e) => {
          e.preventDefault();
          setDragging(true);
        }}
        onDragLeave={() => setDragging(false)}
        onDrop={(e) => {
          e.preventDefault();
          setDragging(false);
          void takeFiles(Array.from(e.dataTransfer.files));
        }}
      >
        {shots.length ? (
          <div className="attachments">
            {shots.map((s) => (
              <div className="attachment" key={s.id} title={s.name}>
                <img src={`data:${s.mediaType};base64,${s.data}`} alt={s.name} />
                <button
                  type="button"
                  className="attachment-remove"
                  onClick={() => setShots((prev) => prev.filter((x) => x.id !== s.id))}
                  aria-label="移除"
                >
                  ✕
                </button>
              </div>
            ))}
          </div>
        ) : null}

        {/* 引用块住在编辑区里、和文字同一行，所以这里没有单独的块列表。 */}
        <div
          ref={ref}
          className={draft.trim() || refs.length ? "composer-input" : "composer-input empty"}
          contentEditable
          suppressContentEditableWarning
          role="textbox"
          aria-multiline="true"
          data-placeholder={
            busy ? "它正在做事…此刻发送会排队，当前任务完成后自动发出" : "描述一个任务，或问点什么"
          }
          onInput={sync}
          // 光标挪动也要重算 `@查询` —— 用户可能把光标移回句子中间的
          // 一个半截 @ 上继续挑文件。
          onKeyUp={sync}
          onMouseUp={sync}
          onBlur={sync}
          // 粘贴板里的图直接收下。这是"看这个截图"最常用的发法 ——
          // 截完图 ⌘V 就完事，不用先存盘再选文件。
          onPaste={(e) => {
            const files = Array.from(e.clipboardData.files);
            if (files.some((f) => f.type.startsWith("image/"))) {
              // 只有真拿到图才拦默认行为，否则会把普通的文本粘贴也吃掉。
              e.preventDefault();
              void takeFiles(files);
              return;
            }
            // 富文本粘贴要降级成纯文本：contenteditable 默认会把网页的
            // 样式、图片、甚至整个表格结构原样塞进来。
            e.preventDefault();
            const text = e.clipboardData.getData("text/plain");
            document.execCommand("insertText", false, text);
            sync();
          }}
          onCompositionStart={() => {
            imeRef.current = true;
          }}
          onCompositionEnd={() => {
            // compositionend 与确认用的 Enter 可能跨到下一个宏任务，
            // microtask 不够，用 setTimeout(0) 盖住这一拍。
            setTimeout(() => {
              imeRef.current = false;
            }, 0);
            sync();
          }}
          onKeyDown={(e) => {
            // 退格删块交给浏览器：contenteditable=false 的元素整个删掉，
            // 而且删的是光标**紧邻**那个，不是"最后一个"。
            //
            // 补全菜单开着时，方向键和 Tab/Enter 归它用。两个菜单不会
            // 同时开：`/` 要求整条草稿就是命令，`@` 认的是末尾那一段。
            const menu = matches.length > 0 ? "slash" : fileMatches.length > 0 ? "file" : null;
            if (menu && !e.nativeEvent.isComposing && !imeRef.current) {
              const len = menu === "slash" ? matches.length : fileMatches.length;
              const move = menu === "slash" ? setSlashPick : setFilePick;
              if (e.key === "ArrowDown") {
                e.preventDefault();
                move((p) => (p + 1) % len);
                return;
              }
              if (e.key === "ArrowUp") {
                e.preventDefault();
                move((p) => (p - 1 + len) % len);
                return;
              }
              if (e.key === "Escape") {
                e.preventDefault();
                // 斜杠菜单：整条草稿就是那个命令，清掉即可。
                // 文件菜单：正文还在，只把光标前那段 @ 抹掉收起菜单。
                if (menu === "slash") {
                  replaceText("");
                } else {
                  dropQueryAtCaret();
                  sync();
                }
                return;
              }
              if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey && e.keyCode !== 229)) {
                e.preventDefault();
                if (menu === "slash") {
                  const c = matches[pick];
                  if (c) chooseSlash(c);
                } else {
                  const f = fileMatches[fpick];
                  if (f) chooseFile(f);
                }
                return;
              }
            }
            // 229 = IME 处理中的占位 keyCode，部分 WebView 上比 isComposing 更准
            if (
              e.key === "Enter" &&
              !e.shiftKey &&
              !e.nativeEvent.isComposing &&
              e.keyCode !== 229 &&
              !imeRef.current
            ) {
              e.preventDefault();
              submit();
            }
          }}
        />

        <div className="composer-bar">
          <button
            type="button"
            className="composer-icon"
            onClick={() => void pickFiles().then(takePaths).catch(() => {})}
            title="附加图片或文件"
            aria-label="附加图片或文件"
          >
            <PlusIcon />
          </button>
          <ModeMenu mode={mode} onChange={changeMode} />
          <Picker
            title="切换服务方"
            label={activeProvider?.name ?? "选择服务方"}
            items={cfg.providers.map((p) => ({
              id: p.id,
              label: p.name,
              active: p.id === cfg.activeProvider,
              ...(config.keyStatus[p.id] ? {} : { note: "未配置 key", warn: true }),
            }))}
            onPick={(id) => {
              const p = cfg.providers.find((x) => x.id === id);
              if (p) switchProvider(p);
            }}
          />
          <Picker
            title="切换模型"
            label={modelLabel(activeProvider, cfg.activeModel) || "选择模型"}
            items={(activeProvider?.models ?? []).map((m) => ({
              id: m.id,
              // 有显示名就用它。菜单里那一列越短越好读，模型 ID 常常很长。
              label: m.name?.trim() || m.id,
              active: m.id === cfg.activeModel,
              ...(m.vision ? { vision: true } : {}),
            }))}
            emptyHint="这个服务方还没有模型"
            onEmpty={onOpenSettings}
            onPick={switchModel}
          />
          <span className="bar-spacer" />
          {tokens.input + tokens.output > 0 ? (
            <span className="usage" title="本会话累计 token（输入 / 输出）">
              {fmtTokens(tokens.input)} / {fmtTokens(tokens.output)}
            </span>
          ) : null}
          {/* 一个位置一个按钮（Cursor 同款）：忙 + 空输入 = 停止；
              一旦打了字就变成发送（排队），清空又变回停止。 */}
          {busy && !draft.trim() && shots.length === 0 ? (
            <button type="button" className="send stop" onClick={onStop} title="停止" aria-label="停止">
              <StopIcon />
            </button>
          ) : (
            <button
              type="submit"
              className="send"
              disabled={(!draft.trim() && shots.length === 0) || !hasKey || !cfg.activeModel}
              title={
                busy ? "排队发送（当前任务完成后自动发出）" : cfg.activeModel ? "发送" : "先选择一个模型"
              }
              aria-label={busy ? "排队发送" : cfg.activeModel ? "发送" : "先选择一个模型"}
            >
              <ArrowUpIcon />
            </button>
          )}
        </div>
      </form>
      {modeConfirm ? (
        <ConfirmDialog c={modeConfirm} onClose={() => setModeConfirm(null)} />
      ) : null}
    </div>
  );
}

/** 模型在界面上叫什么。没配显示名就用 ID。 */
function modelLabel(p: ProviderConfig | null, id: string): string {
  return p?.models.find((m) => m.id === id)?.name?.trim() || id;
}

function fmtTokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(2)}M`;
}

/** 权限模式的上拉菜单。原生 select 样式改不动，自己画一个。 */
function ModeMenu({
  mode,
  onChange,
}: {
  mode: PermissionMode;
  onChange: (m: PermissionMode) => void;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [open]);

  return (
    <div className="mode-menu" ref={rootRef}>
      <button type="button" className="pill" onClick={() => setOpen(!open)}>
        {MODE_LABEL[mode] ?? mode}
        <span className="pick-caret">▾</span>
      </button>
      {open ? (
        <div className="menu">
          {(Object.keys(MODE_LABEL) as PermissionMode[]).map((m) => (
            <button
              key={m}
              type="button"
              className={m === mode ? "menu-item active" : "menu-item"}
              onClick={() => {
                onChange(m);
                setOpen(false);
              }}
            >
              {MODE_LABEL[m]}
              {MODE_WARN[m] ? <span className="menu-warn">{MODE_WARN[m]}</span> : null}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

/**
 * 输入框里的内联下拉（服务方 / 模型共用）。样式沿用 ModeMenu 的
 * pill + 上拉菜单，长文本（模型名）截断，避免把整条工具栏撑开。
 */
interface PickerItem {
  id: string;
  label: string;
  active?: boolean;
  /** 次要说明，靠右。默认淡灰；`warn` 才用黄，留给"未配置 key"这类。 */
  note?: string;
  warn?: boolean;
  /** 能收图片。图标跟在名字后面，不单独占一列。 */
  vision?: boolean;
}

function Picker({
  label,
  title,
  items,
  onPick,
  emptyHint,
  onEmpty,
}: {
  label: string;
  title?: string;
  items: PickerItem[];
  onPick: (id: string) => void;
  /** 列表为空时点 pill 的提示与去向（一般是打开设置补模型）。 */
  emptyHint?: string;
  onEmpty?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const isEmpty = items.length === 0;

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [open]);

  return (
    <div className="mode-menu" ref={rootRef}>
      <button
        type="button"
        className="pill picker-pill"
        title={isEmpty ? emptyHint : title}
        onClick={() => (isEmpty ? onEmpty?.() : setOpen(!open))}
      >
        <span className="pick-label">{label}</span>
        <span className="pick-caret">▾</span>
      </button>
      {open && !isEmpty ? (
        <div className="menu">
          {items.map((it) => (
            <button
              key={it.id}
              type="button"
              className={it.active ? "menu-item picker-item active" : "menu-item picker-item"}
              onClick={() => {
                onPick(it.id);
                setOpen(false);
              }}
            >
              <span className="pick-main">
                <span className="pick-label">{it.label}</span>
                {it.vision ? (
                  <span className="cap-icon" role="img" aria-label="能收图片" title="能收图片">
                    <EyeIcon />
                  </span>
                ) : null}
              </span>
              {it.note ? (
                <span className={it.warn ? "menu-warn" : "menu-hint"}>{it.note}</span>
              ) : null}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

/* ── 图标 ───────────────────────────────────── */

/** 侧边栏开关：矩形 + 左侧一道竖线。 */
function SidebarToggleIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden>
      <rect x="1.5" y="2.5" width="13" height="11" rx="2" stroke="currentColor" strokeWidth="1.3" />
      <path d="M6 2.5v11" stroke="currentColor" strokeWidth="1.3" />
    </svg>
  );
}

/** 右侧抽屉开关：矩形 + 右侧一道竖线。 */
function PanelRightIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden>
      <rect x="1.5" y="2.5" width="13" height="11" rx="2" stroke="currentColor" strokeWidth="1.3" />
      <path d="M10 2.5v11" stroke="currentColor" strokeWidth="1.3" />
    </svg>
  );
}

/** 底部终端开关：矩形 + 底部一道横线。 */
function PanelBottomIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden>
      <rect x="1.5" y="2.5" width="13" height="11" rx="2" stroke="currentColor" strokeWidth="1.3" />
      <path d="M1.5 9.5h13" stroke="currentColor" strokeWidth="1.3" />
    </svg>
  );
}

function PlusIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path d="M8 3v10M3 8h10" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
    </svg>
  );
}

function FolderIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M1.5 4a1 1 0 011-1h3l1.5 1.5h6a1 1 0 011 1V12a1 1 0 01-1 1h-11a1 1 0 01-1-1V4z"
        stroke="currentColor"
        strokeWidth="1.2"
      />
    </svg>
  );
}

function GearIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden>
      <circle cx="12" cy="12" r="3" stroke="currentColor" strokeWidth="2" />
      <path
        d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.38a2 2 0 0 0-.73-2.73l-.15-.1a2 2 0 0 1-1-1.74v-.47a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function ArrowUpIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M8 13V3M3.5 7.5L8 3l4.5 4.5"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function StopIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden>
      <rect x="1.5" y="1.5" width="9" height="9" rx="1.5" fill="currentColor" />
    </svg>
  );
}

function ChevronRightIcon() {
  return (
    <svg width="11" height="11" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M6 3.5L10.5 8L6 12.5"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function PencilIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M11.3 2.2a1.6 1.6 0 0 1 2.3 2.3l-8 8-3.1.8.8-3.1 8-8z"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function TrashIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M2.5 4.5h11M5.5 4.5V3a1 1 0 0 1 1-1h3a1 1 0 0 1 1 1v1.5M4 4.5l.7 8.6a1 1 0 0 0 1 .9h4.6a1 1 0 0 0 1-.9l.7-8.6"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function DotsIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" aria-hidden>
      <circle cx="3.5" cy="8" r="1.3" />
      <circle cx="8" cy="8" r="1.3" />
      <circle cx="12.5" cy="8" r="1.3" />
    </svg>
  );
}

function EyeIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M1.8 8s2.4-4.5 6.2-4.5S14.2 8 14.2 8s-2.4 4.5-6.2 4.5S1.8 8 1.8 8z"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinejoin="round"
      />
      <circle cx="8" cy="8" r="1.9" stroke="currentColor" strokeWidth="1.3" />
    </svg>
  );
}
