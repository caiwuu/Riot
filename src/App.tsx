import {
  memo,
  type ReactNode,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import {
  addProject,
  browserScopeList,
  clipboardPaths,
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
  openInBrowser,
  pickDirectory,
  probeDirs,
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
  subscribeDragDrop,
  subscribeFullscreen,
} from "./bridge";
import { BrowserPanel } from "./components/BrowserPanel";
import { Chevron } from "./components/Chevron";
import { GitChangesPanel } from "./components/GitChangesPanel";
import { SessionChangesBar } from "./components/SessionChangesBar";
import { ScopePanel } from "./components/ScopePanel";
import { SessionSettings } from "./components/SessionSettings";
import { ConfirmDialog, type ConfirmRequest } from "./components/ConfirmDialog";
import { MissingProjectDialog } from "./components/MissingProjectDialog";
import { LazyMarkdown, Markdown, ProjectRootContext } from "./components/Markdown";
import {
  AskChoiceCard,
  PermissionDialog,
  PlanApprovalCard,
  PlanDraft,
} from "./components/PermissionDialog";
import { Settings } from "./components/Settings";
import { groupBlocks, ProcessGroup, ThinkingBlock } from "./components/ProcessFold";
import { useEscLayer } from "./components/Modal";
import { TerminalPanel } from "./components/TerminalPanel";
import { hasActiveTodos, TodoPanel } from "./components/TodoPanel";
import { ToolCard } from "./components/ToolCard";
import {
  type Item,
  type QueuedItem,
  type WithdrawnPrompt,
  forgetSession,
  useSession,
  waitStartedAt,
} from "./hooks/useSession";
import { useAppUpdate } from "./hooks/useAppUpdate";
import { basename, parentOf, tildify } from "./pathDisplay";

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
  collapsedProjects: "riot.layout.collapsedProjects",
  drawer: "riot.layout.drawer",
  term: "riot.layout.term",
};

function loadCollapsedProjects(): Set<string> {
  try {
    const raw = localStorage.getItem(LS.collapsedProjects);
    if (!raw) return new Set();
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return new Set();
    return new Set(parsed.filter((x): x is string => typeof x === "string"));
  } catch {
    return new Set();
  }
}

function saveCollapsedProjects(roots: Set<string>) {
  localStorage.setItem(LS.collapsedProjects, JSON.stringify([...roots]));
}

/** 同时保活的会话树上限。切走不卸 DOM，切回才不是白屏。 */
const KEEP_CHATS = 4;

/**
 * 对话流滚到哪、跟不跟随底部。
 *
 * 正式包 WKWebView 一给面板加 `visibility:hidden` / 改 `position`，
 * 会把 scrollTop 清成 0 并冒一次 scroll。dev 的 WebView 常常不这么做，
 * 所以只有打包后才表现为"切回来跳到顶"。记的必须是用户还看得见时的
 * 位置，隐藏之后那次假滚动一律丢掉。
 */
const transcriptView = new Map<string, { top: number; stick: boolean }>();

const SIDEBAR = { def: 280, min: 180, max: 420 };
/** Overlay 标题栏的红绿灯只在 macOS 占左上角。Windows / Linux 的窗口
 *  按钮在系统标题栏右侧，不进 webview。平台判断跟 main.tsx 的
 *  data-vibrancy、以及下面 padTraffic 用同一条 UA。 */
const IS_MAC = navigator.userAgent.includes("Mac");
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

/** 宿主把缺目录收成 `项目目录不存在：…`；老错误文案也认，免得宿主没跟上。 */
function isMissingProjectError(e: unknown): boolean {
  const s = String(e);
  return s.startsWith("项目目录不存在：") || s.includes("无法解析路径");
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
  /** 目录已不在磁盘上、等用户决定怎么处理的那个项目。 */
  const [goneRoot, setGoneRoot] = useState<string | null>(null);
  /** 探测过、确认不存在的项目根。用来在侧栏和欢迎页标「已失效」。 */
  const [missing, setMissing] = useState<Set<string>>(() => new Set());
  const [renaming, setRenaming] = useState<string | null>(null);
  /** 右侧抽屉此刻装着谁。两个都是整列，只能二选一。 */
  const [drawer, setDrawer] = useState<"browser" | "changes" | null>(null);
  /**
   * 用户主动关过浏览器抽屉的会话。模型在这些会话里再用浏览器工具，
   * 抽屉不再自动弹出 —— 用户已经表过态，每次工具调用都弹回去等于
   * 反复跟他抢屏幕。手动重开视为又想看了，从集合里移除、恢复自动弹出。
   * 存会话 id 而不是一个布尔：别的会话的浏览器活动不该被这个会话连坐。
   */
  const browserDismissed = useRef(new Set<string>());
  const [showTerm, setShowTerm] = useState(false);
  const [showSessionCfg, setShowSessionCfg] = useState(false);
  /** 递增一次，改动面板重新比对一次。轮次结束时推一下。 */
  const [changesRev, setChangesRev] = useState(0);
  /** 用户从终端选中、要交给模型的一段输出。塞进输入框而不是直接发送 ——
   *  他多半还要在前面补一句"这个报错怎么回事"。 */
  const [termSnippet, setTermSnippet] = useState<string | null>(null);
  /** 最近看过的会话 id（LRU）。这些 Chat 卸不掉，切回去是显示/隐藏。 */
  const [kept, setKept] = useState<string[]>([]);
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

  const projectList = config?.config.projects;
  const projects = projectList ?? [];
  const activeSession = sessions.find((s) => s.id === active) ?? null;
  const update = useAppUpdate(!booting && !bootError);
  const updateNotice = update.banner;

  // 项目列表不会因为用户在访达里删了文件夹而自己更新。启动和窗口
  // 回到前台时探一次，侧栏才能把失效项标出来，而不必等点「新会话」才知道。
  useEffect(() => {
    const roots = [...new Set([...(projectList ?? []), ...sessions.map((s) => s.root)])];
    if (roots.length === 0) {
      setMissing(new Set());
      return;
    }
    const scan = () => {
      probeDirs(roots)
        .then((gone) => setMissing(new Set(gone)))
        .catch(() => {});
    };
    scan();
    const onVisible = () => {
      if (document.visibilityState === "visible") scan();
    };
    document.addEventListener("visibilitychange", onVisible);
    window.addEventListener("focus", scan);
    return () => {
      document.removeEventListener("visibilitychange", onVisible);
      window.removeEventListener("focus", scan);
    };
  }, [projectList, sessions]);

  useLayoutEffect(() => {
    if (!active) return;
    setKept((prev) => {
      const alive = new Set(sessions.map((s) => s.id));
      const next = [...prev.filter((id) => id !== active && alive.has(id)), active];
      const clipped = next.length > KEEP_CHATS ? next.slice(next.length - KEEP_CHATS) : next;
      if (clipped.length === prev.length && clipped.every((id, i) => id === prev[i])) return prev;
      return clipped;
    });
  }, [active, sessions]);

  const mountedSessions = useMemo(() => {
    const alive = new Set(sessions.map((s) => s.id));
    const ids = kept.filter((id) => alive.has(id));
    if (active && !ids.includes(active)) ids.push(active);
    return ids
      .map((id) => sessions.find((s) => s.id === id))
      .filter((s): s is SessionInfo => !!s);
  }, [kept, sessions, active]);

  // 窗口标题跟随项目。多窗口/多桌面时，标题栏是用户分辨"哪个是哪个"
  // 的唯一线索。
  useEffect(() => {
    const name = activeSession?.root ? basename(activeSession.root) : undefined;
    setWindowTitle(name ? `${name} — Riot` : "Riot").catch(() => {});
  }, [activeSession?.root]);

  const noteError = useCallback((title: string, e: unknown) => {
    setConfirm({
      title,
      body: String(e),
      confirmLabel: "知道了",
      danger: false,
      action: () => {},
    });
  }, []);

  const newSession = useCallback(async (root: string) => {
    try {
      const info = await createSession(root);
      setMissing((prev) => {
        if (!prev.has(root)) return prev;
        const next = new Set(prev);
        next.delete(root);
        return next;
      });
      setSessions((prev) => [...prev, info]);
      setActive(info.id);
    } catch (e) {
      // 目录被删是可恢复的：问要不要从列表拿掉或另选，别整页「出错了」。
      if (isMissingProjectError(e)) {
        setMissing((prev) => new Set(prev).add(root));
        setGoneRoot(root);
        return;
      }
      noteError("无法创建会话", e);
    }
  }, [noteError]);

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
      if (isMissingProjectError(e)) {
        setMissing((prev) => new Set(prev).add(dir));
        setGoneRoot(dir);
        return;
      }
      noteError("打不开这个目录", e);
    }
  }, [newSession, noteError]);

  /** 会话发出第一条消息后补标题。宿主的 title 来自历史，UI 上要即时。 */
  const onFirstMessage = useCallback((sessionId: string, text: string) => {
    setSessions((prev) =>
      prev.map((s) =>
        s.id === sessionId && !s.title ? { ...s, title: text.slice(0, 40) } : s,
      ),
    );
  }, []);

  /**
   * 唯一那条提问被撤回了（模型没开口就停了），会话空了。
   *
   * 标题正是从那句话取的，得跟着撤 —— 宿主同步在做同一件事（见
   * `HostNotice::PromptWithdrawn`），这里只是让侧栏当场跟上，不然要
   * 等下一次拉列表才对。
   */
  const onSessionEmptied = useCallback((sessionId: string) => {
    setSessions((prev) => prev.map((s) => (s.id === sessionId ? { ...s, title: null } : s)));
  }, []);

  /** 会话设置提交成功后回写列表。listSessions 只在启动时拉一次，
   *  不回写的话，弹窗关掉再打开显示的就是旧值。 */
  const patchSession = useCallback((id: string, patch: Partial<SessionInfo>) => {
    setSessions((prev) => prev.map((s) => (s.id === id ? { ...s, ...patch } : s)));
  }, []);

  // 后台会话的忙碌状态没有事件可订阅（只有挂载中的会话有事件流），
  // 侧栏的"正在跑"指示点只能靠轮询对齐。只在确实有会话在跑时才轮 ——
  // 全员空闲时一次都不发。
  const anyBusy = sessions.some((s) => s.busy);
  useEffect(() => {
    if (!anyBusy) return;
    const t = setInterval(() => {
      listSessions()
        .then(setSessions)
        .catch(() => {});
    }, 5000);
    return () => clearInterval(t);
  }, [anyBusy]);

  /**
   * 全局 ⌘ 快捷键。macOS 用户带着 Finder/浏览器的肌肉记忆来 ——
   * ⌘N 新会话、⌘, 设置、⌘B 侧栏、⌘J 终端。
   *
   * `[约束]` ⌘W 必须拦下来 preventDefault，否则 WKWebView 默认会关掉整个
   * 窗口 —— 用户想关的多半是一个面板，不是整个应用。这里把它接住当"收起
   * 当前抽屉/终端"，没有可收的就静默吃掉，绝不放它去关窗。
   */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.metaKey || e.ctrlKey || e.altKey) return;
      const k = e.key.toLowerCase();
      switch (k) {
        case "n": {
          if (e.shiftKey) return;
          const root = activeSession?.root ?? projects[0];
          if (root) {
            e.preventDefault();
            void newSession(root);
          }
          return;
        }
        case ",":
          e.preventDefault();
          setShowSettings(true);
          return;
        case "b":
          e.preventDefault();
          toggleSidebar();
          return;
        case "j":
          e.preventDefault();
          setShowTerm((v) => !v);
          return;
        case "w":
          // 永远拦下，绝不让它冒泡去关窗口。收起一个面板：抽屉优先，
          // 其次终端；都没开就静默吃掉（绝不关窗）。
          e.preventDefault();
          if (drawer) {
            // 用键盘收掉浏览器抽屉也是主动关闭，之后不再自动弹。
            if (drawer === "browser" && activeSession) {
              browserDismissed.current.add(activeSession.id);
            }
            setDrawer(null);
          } else {
            setShowTerm((v) => (v ? false : v));
          }
          return;
        default:
          return;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [activeSession, projects, newSession, toggleSidebar, drawer]);

  /* ── 会话 / 项目操作 ──────────────────────── */

  const doDeleteSession = async (id: string) => {
    await deleteSession(id);
    forgetSession(id);
    transcriptView.delete(id);
    setKept((prev) => prev.filter((x) => x !== id));
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
    const gone = new Set([
      ...closed,
      ...sessions.filter((s) => s.root === root).map((s) => s.id),
    ]);
    for (const id of gone) {
      forgetSession(id);
      transcriptView.delete(id);
    }
    setKept((prev) => prev.filter((id) => !gone.has(id)));
    setConfig(await getConfig());
    const next = sessions.filter((s) => s.root !== root && !closed.includes(s.id));
    setSessions(next);
    setMissing((prev) => {
      if (!prev.has(root)) return prev;
      const n = new Set(prev);
      n.delete(root);
      return n;
    });
    if (goneRoot === root) setGoneRoot(null);
    const activeGone =
      active !== null &&
      (closed.includes(active) || sessions.find((s) => s.id === active)?.root === root);
    if (activeGone) setActive(next[next.length - 1]?.id ?? null);
  };

  /**
   * 失效项目换一个还在的目录。先打开新的，再把旧的从列表拿掉 ——
   * 反过来的话，选目录失败会先丢掉旧会话。
   */
  const relocateGone = async (oldRoot: string) => {
    const dir = await pickDirectory();
    if (!dir) return;
    try {
      const root = await addProject(dir);
      const info = await createSession(root);
      const closed = root === oldRoot ? [] : await removeProject(oldRoot);
      const dropped = new Set(closed);
      for (const id of dropped) {
        forgetSession(id);
        transcriptView.delete(id);
      }
      setKept((prev) => prev.filter((id) => !dropped.has(id)));
      setConfig(await getConfig());
      setSessions((prev) => [
        ...prev.filter((s) => s.root !== oldRoot && !dropped.has(s.id)),
        info,
      ]);
      setActive(info.id);
      setMissing((prev) => {
        const n = new Set(prev);
        n.delete(root);
        n.delete(oldRoot);
        return n;
      });
    } catch (e) {
      if (isMissingProjectError(e)) {
        setMissing((prev) => new Set(prev).add(dir));
        setGoneRoot(dir);
        return;
      }
      noteError("打不开这个目录", e);
    }
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
    const name = basename(root) || root;
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
    // 冷启动那一两秒别留纯白屏 —— 给个品牌骨架，让"在启动"看得出来。
    return (
      <div className="booting">
        <div className="booting-logo">Riot</div>
        <div className="booting-spinner" aria-label="启动中" />
      </div>
    );
  }

  return (
    <ProjectRootContext.Provider value={activeSession?.root ?? ""}>
    <div className="shell" data-fullscreen={fullscreen ? "" : undefined}>
      {sidebarOpen ? (
        <>
          <Sidebar
            width={sidebarW}
            projects={projects}
            missing={missing}
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
          browserOpen={drawer === "browser"}
          browserEnabled={activeSession !== null}
          onToggleBrowser={() => {
            if (drawer === "browser") {
              if (activeSession) browserDismissed.current.add(activeSession.id);
              setDrawer(null);
            } else {
              if (activeSession) browserDismissed.current.delete(activeSession.id);
              setDrawer("browser");
            }
          }}
          terminalOpen={showTerm}
          onToggleTerminal={() => setShowTerm((v) => !v)}
          sessionCfgOpen={showSessionCfg}
          sessionCfgEnabled={activeSession !== null}
          onToggleSessionCfg={() => setShowSessionCfg((v) => !v)}
          changesOpen={drawer === "changes"}
          changesEnabled={activeSession !== null}
          onToggleChanges={() => {
            // 切到改动面板会把浏览器顶掉 —— 这也是"用户不想看浏览器"的
            // 表态，不记下来的话，模型下一次导航又把改动面板抢回去。
            if (drawer === "browser" && activeSession) {
              browserDismissed.current.add(activeSession.id);
            }
            setDrawer((d) => (d === "changes" ? null : "changes"));
          }}
        />

        {updateNotice ? (
          <div className="update-banner" role="status">
            <span>Riot {updateNotice.latest} 已发布</span>
            <button className="ghost" onClick={() => void openInBrowser(updateNotice.url)}>
              去下载
            </button>
            <button
              className="update-banner-dismiss"
              onClick={update.dismiss}
              title="关闭"
              aria-label="关闭"
            >
              ×
            </button>
          </div>
        ) : null}

        <div className="workarea">
          <div className="chat-col">
            {activeSession ? (
              mountedSessions.map((s) => {
                const visible = s.id === activeSession.id;
                return (
                  <div
                    key={s.id}
                    className={visible ? "chat-pane" : "chat-pane is-hidden"}
                    aria-hidden={!visible}
                    inert={!visible}
                  >
                    <Chat
                      sessionId={s.id}
                      visible={visible}
                      expectHistory={s.title != null}
                      config={config}
                      workspace={s.root}
                      workspaceMissing={missing.has(s.root)}
                      onMissingWorkspace={() => setGoneRoot(s.root)}
                      initialMode={s.mode}
                      onConfig={setConfig}
                      onOpenSettings={() => setShowSettings(true)}
                      onFirstMessage={onFirstMessage}
                      onSessionEmptied={onSessionEmptied}
                      onAgentBrowser={() => {
                        if (!visible) return;
                        if (browserDismissed.current.has(s.id)) return;
                        setDrawer("browser");
                      }}
                      onTurnEnd={() => setChangesRev((n) => n + 1)}
                      onBusy={(b) => patchSession(s.id, { busy: b })}
                      insertText={visible ? termSnippet : null}
                      onInserted={() => setTermSnippet(null)}
                    />
                  </div>
                );
              })
            ) : (
              <Welcome
                projects={projects}
                missing={missing}
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
          onAgentTerminal={() => setShowTerm(true)}
          onSendSelection={setTermSnippet}
        />
      </div>

      {/* 抽屉是 main 的兄弟：整列全高（Codex 同款），terminal 只垫在对话
          下面。抽屉和对话共享同一个会话 —— 这正是它存在的意义：你和模型
          看同一个页面。

          浏览器和改动共用这一个槽位，互斥：两个都是"右边整列"，同时开
          会把对话挤成一条缝。宽度共享，拖过一次两个都记住。 */}
      {activeSession && drawer ? (
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
            {drawer === "browser" ? (
              <>
                <BrowserPanel
                  key={activeSession.id}
                  sessionId={activeSession.id}
                  onClose={() => {
                    browserDismissed.current.add(activeSession.id);
                    setDrawer(null);
                  }}
                />
                <ScopePanel sessionId={activeSession.id} />
              </>
            ) : (
              <GitChangesPanel
                key={activeSession.id}
                sessionId={activeSession.id}
                refreshKey={changesRev}
                onClose={() => setDrawer(null)}
              />
            )}
          </div>
        </>
      ) : null}

      {showSettings ? (
        <Settings
          status={config}
          onStatus={setConfig}
          onClose={() => setShowSettings(false)}
          activeRoot={activeSession?.root ?? null}
          appVersion={update.version}
          update={update.info}
          updateChecking={update.checking}
          updateError={update.error}
          onCheckUpdate={() => void update.check()}
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
      {goneRoot ? (
        <MissingProjectDialog
          root={goneRoot}
          onClose={() => setGoneRoot(null)}
          onRemove={() => void doRemoveProject(goneRoot).catch((err: unknown) => noteError("无法移除项目", err))}
          onRelocate={() => void relocateGone(goneRoot)}
        />
      ) : null}
    </div>
    </ProjectRootContext.Provider>
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
  changesOpen,
  changesEnabled,
  onToggleChanges,
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
  changesOpen: boolean;
  changesEnabled: boolean;
  onToggleChanges: () => void;
}) {
  // 侧栏收起后 macOS 的红绿灯悬在主区左上角，工具栏给它们让位。
  // 全屏没有红绿灯（见 shell[data-fullscreen]），Windows/Linux 的窗口
  // 按钮在右上且不在 webview 里，都不用让。
  const padTraffic = !sidebarOpen && IS_MAC;
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
          <Chevron down />
        </button>
      ) : null}

      <div className="tb-spacer" data-tauri-drag-region />

      {/* 渗透授权常驻角标：授权列表原本只在浏览器抽屉里可见，抽屉一关，
          "允许对哪些站做侵入性操作"就从界面上彻底消失 —— 授权还生效着，
          它的可见性不能跟着面板走。 */}
      {session ? <ScopeBadge sessionId={session.id} onOpen={onToggleBrowser} browserOpen={browserOpen} /> : null}

      <button
        className={changesOpen ? "tb-btn active" : "tb-btn"}
        onClick={onToggleChanges}
        disabled={!changesEnabled}
        title={changesEnabled ? "Git 改动（未提交的工作区差异）" : "先打开一个会话"}
        aria-label="Git 改动"
      >
        <DiffIcon />
      </button>
      <button
        className={browserOpen ? "tb-btn active" : "tb-btn"}
        onClick={onToggleBrowser}
        disabled={!browserEnabled}
        title={browserEnabled ? "浏览器抽屉" : "先打开一个会话再用浏览器"}
        aria-label="浏览器抽屉"
      >
        <BrowserIcon />
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

/**
 * 顶栏的渗透授权角标。有生效的 scope 授权时亮起（盾牌 + 数量），
 * 点击打开浏览器抽屉 —— 撤销入口（ScopePanel）在那里。
 * 没有授权时整个不渲染，颗粒无声。
 */
function ScopeBadge({
  sessionId,
  browserOpen,
  onOpen,
}: {
  sessionId: string;
  browserOpen: boolean;
  onOpen: () => void;
}) {
  const [count, setCount] = useState(0);

  useEffect(() => {
    let alive = true;
    const pull = () => {
      browserScopeList(sessionId)
        .then((hs) => {
          if (alive) setCount(hs.length);
        })
        .catch(() => {
          // 浏览器还没起来。下一拍再问。
        });
    };
    pull();
    const t = setInterval(pull, 3000);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [sessionId]);

  if (count === 0) return null;
  return (
    <button
      className="tb-btn scope-badge"
      onClick={() => {
        if (!browserOpen) onOpen();
      }}
      title={`${count} 个站点授权了侵入性渗透操作 —— 点击查看和撤销`}
      aria-label={`渗透授权 ${count} 个站点`}
    >
      <ShieldIcon />
      <span className="scope-badge-count">{count}</span>
    </button>
  );
}

function ShieldIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M8 1.8l5 1.8v3.9c0 3.2-2.1 5.6-5 6.7-2.9-1.1-5-3.5-5-6.7V3.6L8 1.8z"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinejoin="round"
      />
    </svg>
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
  // Esc 走公共栈：菜单叠在弹窗上时只关菜单，不连带关底下的。
  useEscLayer(onClose);
  const boxRef = useRef<HTMLDivElement>(null);
  /** 键盘高亮到第几项。-1 = 还没用键盘。 */
  const [pick, setPick] = useState(-1);

  // 贴近视口边缘时往回顶，别让菜单被截掉。右缘按典型菜单宽估算 ——
  // 菜单还没渲染，量不到自己。
  const top = Math.min(menu.y, window.innerHeight - menu.entries.length * 34 - 16);
  const left = Math.min(menu.x, window.innerWidth - CTX_MENU_W - 8);

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
        role="menu"
        ref={boxRef}
        style={{ left, top }}
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          // 上下键 + Enter：右键菜单的标配键盘模型。
          const n = menu.entries.length;
          if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            const next =
              e.key === "ArrowDown" ? (pick + 1) % n : (pick - 1 + n) % n;
            setPick(next);
            boxRef.current?.querySelectorAll("button")[next]?.focus();
          }
        }}
      >
        {menu.entries.map((en, i) => (
          <button
            key={en.label}
            role="menuitem"
            className={en.danger ? "ctx-item danger" : "ctx-item"}
            // 打开时聚焦第一项，键盘直接能用
            autoFocus={i === 0}
            onFocus={() => setPick(i)}
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

/** 上下文菜单的估算宽度（clamp 右缘用）。和 .ctx-menu 的 CSS 保持一致。 */
const CTX_MENU_W = 180;

/* ── 侧边栏 ─────────────────────────────────── */

interface SidebarProps {
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

function Sidebar(props: SidebarProps) {
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
  missing,
  onNewSession,
  onOpenProject,
}: {
  projects: string[];
  missing: ReadonlySet<string>;
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
            <button
              key={root}
              className={missing.has(root) ? "recent-row gone" : "recent-row"}
              onClick={() => onNewSession(root)}
            >
              <FolderIcon />
              <span className="recent-name">{basename(root)}</span>
              {/* 只显示父目录。完整路径的最后一段就是左边那个名字，
                  重复一遍既占地方又要截断。失效项改说「找不到」，
                  父目录还在也帮不上忙。 */}
              <span className="recent-path">
                {missing.has(root) ? "找不到这个目录" : tildify(parentOf(root))}
              </span>
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

/* ── 对话 ───────────────────────────────────── */

/**
 * 长任务在后台跑完时发系统通知。权限只在第一次要 —— 被拒绝就永远
 * 沉默，不反复骚扰。失败静默：通知是锦上添花，不值得报错。
 */
async function notifyTurnDone() {
  try {
    const { isPermissionGranted, requestPermission, sendNotification } = await import(
      "@tauri-apps/plugin-notification"
    );
    let ok = await isPermissionGranted();
    if (!ok) ok = (await requestPermission()) === "granted";
    if (ok) sendNotification({ title: "Riot", body: "任务完成了，回来看看结果吧。" });
  } catch {
    // 平台不支持或用户拒绝 —— 无声跳过
  }
}

function Chat({
  sessionId,
  visible,
  expectHistory,
  config,
  workspace,
  workspaceMissing,
  onMissingWorkspace,
  initialMode,
  onConfig,
  onOpenSettings,
  onFirstMessage,
  onSessionEmptied,
  onAgentBrowser,
  onTurnEnd,
  onBusy,
  insertText,
  onInserted,
}: {
  sessionId: string;
  /** 在前台。隐藏的保活实例不接全局快捷键 / 粘贴 / 权限弹窗。 */
  visible: boolean;
  /** 侧栏已经有标题：历史还没到时别画成空招呼页。 */
  expectHistory: boolean;
  config: ConfigStatus;
  workspace: string;
  /** 绑定的项目目录已经不在磁盘上。 */
  workspaceMissing?: boolean;
  onMissingWorkspace?: () => void;
  initialMode: PermissionMode;
  onConfig: (s: ConfigStatus) => void;
  onOpenSettings: () => void;
  onFirstMessage: (sessionId: string, text: string) => void;
  /** 撤回把会话清空了。侧栏那句自动标题该跟着撤。 */
  onSessionEmptied?: (sessionId: string) => void;
  /** 模型调用浏览器工具时打开右侧抽屉，让用户看见同一页。 */
  onAgentBrowser?: () => void;
  /** 一轮跑完。改动面板据此重新比对 —— 抽屉是常驻的，模型改完文件
   *  不刷新的话，那里还停在上一轮的样子。 */
  onTurnEnd?: () => void;
  /** 忙碌状态变化。侧栏的"正在跑"指示点靠它即时更新。 */
  onBusy?: (busy: boolean) => void;
  /** 要塞进输入框的一段文字（终端选中的输出）。null = 没有。 */
  insertText?: string | null;
  onInserted?: () => void;
}) {
  const session = useSession(
    sessionId,
    onAgentBrowser ? { onBrowserOpen: onAgentBrowser } : undefined,
  );

  const busy = session.busy;
  const turnEndRef = useRef(onTurnEnd);
  turnEndRef.current = onTurnEnd;
  const busyRef = useRef(onBusy);
  busyRef.current = onBusy;
  // 跳过挂载那次：挂载时的 busy 是历史快照，不是一次"变化"。
  const sawBusy = useRef(false);
  useEffect(() => {
    if (!busy) turnEndRef.current?.();
    busyRef.current?.(busy);
    if (busy) {
      sawBusy.current = true;
    } else if (sawBusy.current && !document.hasFocus()) {
      // 长任务跑完而窗口在后台：发一条系统通知，不然就错过了。
      // 窗口在前台时不发 —— 用户正看着呢。
      notifyTurnDone();
    }
  }, [busy]);
  const empty =
    session.items.length === 0 &&
    !session.streaming &&
    !session.thinking &&
    (session.ready || !expectHistory);

  // 撤回把会话清空了：那句话回到了输入框，侧栏不该再拿它当名字。
  // 只报一次 —— 输入框消费掉之后 withdrawn 就置回 null 了。
  const emptiedRef = useRef(onSessionEmptied);
  emptiedRef.current = onSessionEmptied;
  const withdrawn = session.withdrawn;
  useEffect(() => {
    if (withdrawn?.sessionEmpty) emptiedRef.current?.(sessionId);
  }, [withdrawn, sessionId]);

  // 每有一次编辑工具落盘就递增,改动条跟着重新比对 —— 跑轮当中改动
  // 也要实时长出来,不能等轮子结束才一次性冒出一排文件。
  const editCount = useMemo(
    () =>
      session.items.filter(
        (it) =>
          it.kind === "tool" &&
          it.status === "ok" &&
          (it.name === "Edit" || it.name === "Write"),
      ).length,
    [session.items],
  );

  // 输入框上方那一格的占位规则:跑轮期间有没做完的任务清单,就让
  // 任务临时顶掉改动条;清单全部完成、或轮子停了(含切回已结束的
  // 会话),任务自动让位 —— 它是进行时的进度,不是要留档的结果。
  const todoActive = busy && hasActiveTodos(session.items);

  // 计划和选择题都走对话流里的内联卡：它们是对话的一部分，不是危险
  // 操作。Bash / Write 这类权限询问仍弹窗 —— 必须看见原文才能签。
  const isPlanAsk = (a: (typeof session.asks)[number]) =>
    a.detail.suggestions.some((s) => s.type === "set_mode");
  const isChoiceAsk = (a: (typeof session.asks)[number]) => a.detail.preview.kind === "choice";
  const planAsk = session.asks.find(isPlanAsk);
  const choiceAsk = session.asks.find(isChoiceAsk);
  const modalAsk = session.asks.find((a) => !isPlanAsk(a) && !isChoiceAsk(a));

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
      {...(workspaceMissing ? { workspaceMissing: true, onMissingWorkspace } : {})}
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
      withdrawn={session.withdrawn}
      onWithdrawnRestored={session.clearWithdrawn}
      onOpenSettings={onOpenSettings}
      insertText={insertText ?? null}
      armed={visible}
      {...(onInserted ? { onInserted } : {})}
    />
  );

  /**
   * 输入框那一格。空会话和有对话时是**同一个** dock —— 位置、结构、
   * DOM 顺序都不变，翻转的只是它上面那块（招呼语 ↔ 对话流）。
   *
   * `[约束]` 两个分支里 dock 的孩子必须逐位对应。React 按位置复用，
   * 形状一样才不会在发出第一条消息时把 Composer 整个重挂载 ——
   * 重挂载会丢掉草稿、待发的图和刚选好的权限模式。
   */
  const dock = (
    <div className="composer-dock">
      {/* 任务清单和改动条共用输入框上方这一格:跑轮时看进度,
          其余时间看改动(Cursor 同款)。两个都常驻会叠成两层横条,
          把输入框越垫越高。空会话时两者都渲染成空,不占位置。 */}
      {todoActive ? (
        <TodoPanel items={session.items} />
      ) : (
        <SessionChangesBar
          sessionId={sessionId}
          refreshKey={editCount}
          paused={!visible}
        />
      )}
      {composer}
    </div>
  );

  return (
    <div className="chat">
      {empty ? (
        <div className="hero">
          <span className="hero-logo">
            <RiotMark />
          </span>
          <h1 className="hero-title">今天做点什么？</h1>
          <p className="hero-ws" title={workspace}>
            {workspaceMissing ? (
              <button
                type="button"
                className="hero-ws-missing"
                onClick={onMissingWorkspace}
              >
                目录已不存在，点这里处理
              </button>
            ) : (
              <>
                <FolderIcon /> <span className="hero-ws-path">{workspace}</span>
              </>
            )}
          </p>
        </div>
      ) : (
        <Transcript
          sessionId={sessionId}
          items={session.items}
          streaming={session.streaming}
          thinking={session.thinking}
          streamingPlan={session.streamingPlan}
          busy={session.busy}
          compacting={session.compacting}
          waitSince={waitStartedAt(sessionId)}
          armed={visible}
          onRegenerate={session.regenerate}
          {...(planAsk ? { planAsk } : {})}
          {...(choiceAsk ? { choiceAsk } : {})}
          onAnswerPlan={(r) => planAsk && void session.answer(r, planAsk.requestId)}
          onAnswerChoice={(r) => choiceAsk && void session.answer(r, choiceAsk.requestId)}
        />
      )}
      {dock}

      {visible && modalAsk ? (
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

/**
 * 会话内查找（⌘F）。
 *
 * 高亮用 CSS Custom Highlight API：直接在文本节点上建 Range，不往
 * React 管理的 DOM 里塞 <mark> —— 塞了的话下一次渲染要么被抹掉、
 * 要么把 React 的 diff 弄糊涂。旧 WebView 没有这个 API 时退化成
 * 只滚动定位、不上色。
 */
function FindBar({
  box,
  onClose,
}: {
  /** 对话流的滚动容器。 */
  box: React.RefObject<HTMLElement | null>;
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const [cur, setCur] = useState(0);
  const hitsRef = useRef<Range[]>([]);
  const [total, setTotal] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  const highlights = (CSS as unknown as { highlights?: Map<string, unknown> }).highlights;

  const clear = () => {
    highlights?.delete("riot-find");
    highlights?.delete("riot-find-cur");
  };

  /** 全量重扫。对话流不重排 DOM 的话 Range 一直有效，扫一次够用。 */
  const scan = (q: string): Range[] => {
    const root = box.current;
    if (!root || !q) return [];
    const needle = q.toLowerCase();
    const ranges: Range[] = [];
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const text = node.textContent ?? "";
      const hay = text.toLowerCase();
      let at = hay.indexOf(needle);
      while (at !== -1) {
        const r = document.createRange();
        r.setStart(node, at);
        r.setEnd(node, at + needle.length);
        ranges.push(r);
        at = hay.indexOf(needle, at + needle.length);
      }
    }
    return ranges;
  };

  const paint = (ranges: Range[], current: number) => {
    if (!highlights) return;
    const H = (window as unknown as { Highlight?: new (...r: Range[]) => unknown }).Highlight;
    if (!H) return;
    clear();
    if (ranges.length) {
      highlights.set("riot-find", new H(...ranges));
      const c = ranges[current];
      if (c) highlights.set("riot-find-cur", new H(c));
    }
  };

  const jump = (ranges: Range[], i: number) => {
    const r = ranges[i];
    if (!r) return;
    const el = r.startContainer.parentElement;
    el?.scrollIntoView({ block: "center" });
  };

  const run = (q: string) => {
    setQuery(q);
    const ranges = scan(q);
    hitsRef.current = ranges;
    setTotal(ranges.length);
    setCur(0);
    paint(ranges, 0);
    jump(ranges, 0);
  };

  const step = (dir: 1 | -1) => {
    const ranges = hitsRef.current;
    if (!ranges.length) return;
    const next = (cur + dir + ranges.length) % ranges.length;
    setCur(next);
    paint(ranges, next);
    jump(ranges, next);
  };

  // 关闭（含卸载）时清掉高亮，别在页面上留一堆黄块
  useEffect(() => clear, []);

  return (
    <div className="find-wrap">
      <div className="find-bar" role="search">
        <input
          ref={inputRef}
          autoFocus
          value={query}
          placeholder="在会话中查找"
          onChange={(e) => run(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              step(e.shiftKey ? -1 : 1);
            } else if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              clear();
              onClose();
            }
          }}
        />
        <span className="find-count">{total ? `${cur + 1}/${total}` : query ? "0/0" : ""}</span>
        <button
          type="button"
          className="find-btn"
          title="上一个 (⇧Enter)"
          aria-label="上一个"
          disabled={!total}
          onClick={() => step(-1)}
        >
          ▲
        </button>
        <button
          type="button"
          className="find-btn"
          title="下一个 (Enter)"
          aria-label="下一个"
          disabled={!total}
          onClick={() => step(1)}
        >
          ▼
        </button>
        <button
          type="button"
          className="find-btn"
          title="关闭 (Esc)"
          aria-label="关闭查找"
          onClick={() => {
            clear();
            onClose();
          }}
        >
          ✕
        </button>
      </div>
    </div>
  );
}

function Transcript({
  sessionId,
  items,
  streaming,
  thinking,
  streamingPlan,
  busy,
  compacting,
  waitSince,
  armed = true,
  planAsk,
  choiceAsk,
  onAnswerPlan,
  onAnswerChoice,
  onRegenerate,
}: {
  sessionId: string;
  items: Item[];
  streaming: string;
  thinking: string;
  streamingPlan: string | null;
  busy: boolean;
  /** 前台才接 ⌘F。保活的隐藏实例不能跟前台抢查找。 */
  armed?: boolean;
  /** 宿主正在压缩上下文。见 useSession 里同名字段。 */
  compacting: boolean;
  /**
   * 当前这轮等待的起点（epoch ms）。挂在组件外（见 waitStartedAt），
   * 活得过切会话导致的重挂载 —— 状态行的秒数靠它接着数而不是清零。
   */
  waitSince: number | null;
  /** 待批准的计划（ExitPlanMode 的询问）。内联在对话流末尾。 */
  planAsk?: { requestId: string; detail: PermissionAsk };
  /** 模型主动提的选择题。同样内联，不弹窗。 */
  choiceAsk?: { requestId: string; detail: PermissionAsk };
  onAnswerPlan?: (r: PermissionResponse) => void;
  onAnswerChoice?: (r: PermissionResponse) => void;
  onRegenerate?: (itemId: string) => void;
}) {
  const boxRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  /** 程序化贴底时挡住 onScroll，免得自己把 stick 打成 false。 */
  const pinning = useRef(false);
  // 渲染期就写：正式包隐藏面板时 scroll 发生在 commit 里，
  // effect 还没跑，闭包里的 armed 仍是 true，会把清零后的 0 记进去。
  const armedRef = useRef(armed);
  armedRef.current = armed;

  const rememberView = (box: HTMLElement) => {
    if (!armedRef.current) return;
    transcriptView.set(sessionId, { top: box.scrollTop, stick: stick.current });
  };

  /** 离底超过一屏时浮现「回到底部」按钮。 */
  const [awayFromBottom, setAwayFromBottom] = useState(false);
  /** ⌘F 查找条。长对话找不到历史内容是真实痛点。 */
  const [findOpen, setFindOpen] = useState(false);

  const pinBottom = () => {
    const box = boxRef.current;
    if (!box) return;
    pinning.current = true;
    box.scrollTop = box.scrollHeight;
    stick.current = true;
    rememberView(box);
    // 程序化滚动被 pinning 挡掉 onScroll，这里自己收按钮 ——
    // 不收的话点了「回到底部」它还挂着。
    setAwayFromBottom(false);
    requestAnimationFrame(() => {
      pinning.current = false;
    });
  };

  const restoreView = () => {
    const box = boxRef.current;
    const saved = transcriptView.get(sessionId);
    if (!box || !saved) return;
    if (saved.stick) {
      pinBottom();
      return;
    }
    stick.current = false;
    pinning.current = true;
    box.scrollTop = saved.top;
    const gap = box.scrollHeight - box.scrollTop - box.clientHeight;
    setAwayFromBottom(gap > box.clientHeight);
    requestAnimationFrame(() => {
      pinning.current = false;
    });
  };

  useEffect(() => {
    if (!armed) {
      setFindOpen(false);
      return;
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "f") {
        e.preventDefault();
        setFindOpen(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [armed]);

  // 只在用户本来就贴着底部时才自动滚。他往上翻着看历史的时候把他拽回来，
  // 是聊天界面里最招人烦的一件事。
  //
  // 解锁看意图，重吸看位置（非对称迟滞，同 ChatGPT / use-stick-to-bottom）：
  // 任何向上滚动立即交出跟随。早先"离底 < 80px 仍算贴底"是位置阈值，
  // 流式期间内容每帧都在长、pinBottom 每帧都在追，向上滚没能一口气
  // 甩出 80px 就会在下一帧被拽回 —— 触控板小幅滚动永远赛不过生成速度，
  // 夺回控制权要靠大力猛划。方向判断没有这场赛跑：动一下就是想走。
  useEffect(() => {
    const box = boxRef.current;
    if (!box) return;
    let lastTop = box.scrollTop;
    const onScroll = () => {
      const top = box.scrollTop;
      const delta = top - lastTop;
      // pinning 帧也要记位置，不然下一次用户滚动会拿到跨帧的假 delta。
      lastTop = top;
      // 隐藏那一帧正式包会把 scrollTop 打成 0。armed 在渲染时已是
      // false，这次滚动不是用户翻的，不能写进缓存、也不能改 stick。
      if (!armedRef.current) return;
      if (pinning.current) return;
      const gap = box.scrollHeight - top - box.clientHeight;
      if (delta < 0 && gap > 1) {
        // gap > 1 挡掉底部橡皮筋回弹：过冲弹回时 scrollTop 也在变小，
        // 但那不是"想往上翻"。
        stick.current = false;
      } else if (delta > 0 && gap < 24) {
        // 只有自己滚回贴底才恢复跟随。阈值收窄到约一行 —— 停在离底
        // 几十像素处阅读时，跟随不该被抢回去。
        stick.current = true;
      }
      rememberView(box);
      setAwayFromBottom(gap > box.clientHeight);
    };
    box.addEventListener("scroll", onScroll, { passive: true });
    return () => box.removeEventListener("scroll", onScroll);
  }, [sessionId]);

  // 切回来把位置还回去。layout 一次不够：正式包揭开 visibility
  // 之后还要再排一次，第二帧再写才能压住 WebKit 的清零。
  useLayoutEffect(() => {
    if (!armed) return;
    restoreView();
    const again = requestAnimationFrame(() => restoreView());
    return () => cancelAnimationFrame(again);
  }, [armed, sessionId]);

  // 正文晚一拍量完（markdown / 图片）时高度还会涨，贴着就跟上。
  useEffect(() => {
    const box = boxRef.current;
    const col = box?.querySelector(".thread-col");
    if (!box || !col) return;
    const ro = new ResizeObserver(() => {
      if (stick.current) {
        pinBottom();
        return;
      }
      // 交出跟随后内容还在下面长，gap 变大但不触发 scroll 事件 ——
      // 「回到底部」得靠这里浮出来，不然用户翻上去就找不到回程。
      const gap = box.scrollHeight - box.scrollTop - box.clientHeight;
      setAwayFromBottom(gap > box.clientHeight);
    });
    ro.observe(col);
    return () => ro.disconnect();
  }, []);

  // 自己发的消息无条件回到底部。没有这条的话，在上面翻历史时发了新
  // 消息，stick 是 false，整轮生成都不跟随 —— 得手动滑到底才恢复。
  const lastUserId = useRef("");
  useLayoutEffect(() => {
    for (let i = items.length - 1; i >= 0; i--) {
      const it = items[i];
      if (it?.kind !== "user") continue;
      if (it.id !== lastUserId.current) {
        lastUserId.current = it.id;
        stick.current = true;
      }
      break;
    }
    if (stick.current) pinBottom();
  }, [items, streaming, thinking, streamingPlan, planAsk?.requestId, choiceAsk?.requestId, busy]);

  // 还在转圈的工具。底部状态行靠它说清此刻在等谁 —— 一次 build 跑两
  // 分钟的时候，"生成中"是句废话。
  const runningTool = useMemo(() => {
    for (let i = items.length - 1; i >= 0; i--) {
      const it = items[i];
      if (it && it.kind === "tool" && it.status === "running") return it.name;
    }
    return null;
  }, [items]);

  const waitLabel = runningTool ? `正在执行 ${runningTool}` : "正在生成…";

  // 连续的思考 / 工具折成组（学 Cursor）。长探索几十行连排会把回答
  // 挤出屏幕，折完对话流里剩下的才是内容。生成期间正在跑的工具也在
  // 组里，组头单行直播（见 ProcessGroup）—— 工具完成只换字不增删行，
  // 底部不再随每个工具弹跳。正文开始流（streaming 非空）说明尾部那段
  // 过程已经讲完，live 撤下、单条段落还原成普通行 —— 一行换一行，
  // 这次落定同样不跳。liveTail 先算成布尔再进 useMemo：流式期间
  // Transcript 每帧重渲染，分组不该跟着每帧重算。
  const liveTail = busy && !streaming;
  const blocks = useMemo(() => groupBlocks(items, liveTail), [items, liveTail]);
  // 正在流的思考并进尾部直播组（组头滚思考预览、落定进组行数不变），
  // 没有直播组可挂时才单独成行 —— 那一行随后被组头原地接替，也不跳。
  const tailBlock = blocks[blocks.length - 1];
  const liveFold = tailBlock?.kind === "fold" && tailBlock.live ? tailBlock : undefined;
  // 贴底的那几条立刻走完整 markdown / diff。更早的等进视野再解析，
  // 否则长会话第一次打开会把主线程卡死。查找时全量水合，否则搜不到。
  const hydrateFrom = Math.max(0, blocks.length - 12);

  return (
    <main className="transcript" ref={boxRef}>
      {findOpen && armed ? <FindBar box={boxRef} onClose={() => setFindOpen(false)} /> : null}
      <div className="thread-col">
        {blocks.map((b, i) =>
          b.kind === "row" ? (
            <Row
              key={b.item.id}
              item={b.item}
              hydrate={findOpen || i >= hydrateFrom}
              regenEnabled={!busy}
              {...(onRegenerate ? { onRegenerate } : {})}
            />
          ) : (
            <ProcessGroup
              key={b.id}
              items={b.items}
              live={b.live}
              {...(b === liveFold && thinking ? { thinkingText: thinking } : {})}
            />
          ),
        )}

        {thinking && !liveFold ? <ThinkingBlock text={thinking} live /> : null}
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
        {choiceAsk && onAnswerChoice ? (
          <AskChoiceCard
            key={choiceAsk.requestId}
            ask={choiceAsk.detail}
            onAnswer={onAnswerChoice}
          />
        ) : null}
        {/*
         * 状态行在整个忙碌期间常驻，**不和流式内容二选一**。
         *
         * `[约束]` 早先的写法是"有流式文本就把它藏起来"，理由是文字本身
         * 就在动。但模型说完"先写文件："之后要花十几秒生成工具参数，
         * 那段时间一个字都不吐 —— 屏幕彻底静止，和卡死没有区别。等的
         * 是什么、等了多久，只有这一行能回答。
         *
         * 压缩优先且不看 busy：手动 `/compact` 不开轮次，不占 busy。
         */}
        {compacting ? (
          <Dots label="正在压缩上下文…" timed since={waitSince} />
        ) : busy && !planAsk && !choiceAsk ? (
          <Dots label={waitLabel} timed since={waitSince} />
        ) : null}
      </div>
      {/* 往上翻了超过一屏才出现 —— 贴底时这按钮只是噪音。点了重新贴底，
          流式输出会继续跟随。 */}
      {awayFromBottom ? (
        <button
          type="button"
          className="jump-bottom"
          title="回到底部"
          aria-label="回到底部"
          onClick={() => {
            stick.current = true;
            pinBottom();
          }}
        >
          <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
            <path
              d="M8 3v10M3.5 8.5L8 13l4.5-4.5"
              stroke="currentColor"
              strokeWidth="1.8"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        </button>
      ) : null}
    </main>
  );
}

/**
 * memo：流式输出时 Transcript 每帧重渲染，历史条目不该跟着刷。
 * items 数组里未变化的元素引用是稳定的（更新走的是替换单个元素），
 * 所以浅比较有效。
 */
const Row = memo(function Row({
  item,
  onRegenerate,
  regenEnabled,
  hydrate,
}: {
  item: Item;
  onRegenerate?: (itemId: string) => void;
  regenEnabled?: boolean;
  /** 贴底 / 查找中：立刻解析 markdown 和工具详情。 */
  hydrate?: boolean;
}) {
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
          <LazyMarkdown text={item.text} eager={!!hydrate} />
          {/* 半截话得说明白它为什么半截 —— 不标的话，用户过一会儿回来
              看到的是一句戛然而止的回答，分不清是自己停的还是模型崩了。 */}
          {item.stopped ? <div className="msg-stopped">已停止生成</div> : null}
          <MsgActions
            text={item.text}
            regenEnabled={!!regenEnabled && !!onRegenerate}
            {...(onRegenerate ? { onRegenerate: () => onRegenerate(item.id) } : {})}
          />
        </div>
      );
    case "thinking":
      return <ThinkingBlock text={item.text} />;
    case "tool":
      return <ToolCard tool={item} eager={!!hydrate} />;
    case "error":
      return <div className="msg error">{item.text}</div>;
    case "notice":
      return <div className="msg notice">{item.text}</div>;
    case "compact":
      return (
        <div className="compact-rule" role="separator">
          以上消息已被压缩
        </div>
      );
  }
});

/** 悬停出现的消息操作：复制 + 重新生成。占位始终在，hover 才可见。 */
function MsgActions({
  text,
  onRegenerate,
  regenEnabled,
}: {
  text: string;
  onRegenerate?: () => void;
  regenEnabled: boolean;
}) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="msg-actions">
      <button
        type="button"
        className={copied ? "msg-action done" : "msg-action"}
        title={copied ? "已复制" : "复制原文"}
        aria-label={copied ? "已复制" : "复制原文"}
        onClick={() => {
          void navigator.clipboard.writeText(text);
          setCopied(true);
          setTimeout(() => setCopied(false), 1500);
        }}
      >
        {copied ? <CheckIcon /> : <CopyIcon />}
      </button>
      {onRegenerate ? (
        <button
          type="button"
          className="msg-action"
          title={regenEnabled ? "重新生成" : "生成中，结束后才能重新生成"}
          aria-label="重新生成"
          disabled={!regenEnabled}
          onClick={() => onRegenerate()}
        >
          <RegenIcon />
        </button>
      ) : null}
    </div>
  );
}

function CopyIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden>
      <rect x="5.5" y="5.5" width="8" height="8" rx="1.4" stroke="currentColor" strokeWidth="1.3" />
      <path
        d="M10.5 5.5V4.2A1.7 1.7 0 0 0 8.8 2.5H4.2A1.7 1.7 0 0 0 2.5 4.2v4.6A1.7 1.7 0 0 0 4.2 10.5H5.5"
        stroke="currentColor"
        strokeWidth="1.3"
      />
    </svg>
  );
}

function CheckIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M3.5 8.2L6.6 11.2 12.5 4.8"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function RegenIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M3.2 8.2A4.8 4.8 0 0 1 12 5.4l.2-2.1"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="M12.8 7.8A4.8 4.8 0 0 1 4 10.6l-.2 2.1"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="M12.2 2.8v2.6H9.6"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="M3.8 13.2V10.6H6.4"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/**
 * 等待指示。
 *
 * `label` 说明这次等的是什么。同一个动画表示好几件事的话，用户只能按
 * 最常见的那个理解 —— 所以有具体原因时必须写出来。
 *
 * `timed` 让它自己数秒。模型准备工具参数的那十几秒里一个字都不会吐，
 * 静止的三个点和"卡死了"看起来一模一样；走动的秒数是那段时间里唯一
 * 能证明系统还活着的东西。
 */
function Dots({
  label,
  timed,
  since,
}: {
  label?: string;
  timed?: boolean;
  /**
   * 计时起点（epoch ms）。不给就从挂载时刻起数。
   * 切会话会把整棵 Chat 重挂载，挂载时刻起数的话，切走再切回秒数从 0
   * 重来 —— 等待的起点必须由活得过重挂载的地方（useSession 的模块级
   * 表）给进来。
   */
  since?: number | null;
}) {
  const mountedAt = useRef(Date.now());
  const start = since ?? mountedAt.current;
  const [elapsed, setElapsed] = useState(() => Math.round((Date.now() - start) / 1000));

  useEffect(() => {
    if (!timed) return;
    const tick = () => setElapsed(Math.round((Date.now() - start) / 1000));
    // 立即算一次：切回会话时等待往往已经进行了很久，先显示旧值再等
    // 一秒才跳到真实值，看起来像计时器坏了。
    tick();
    const id = setInterval(tick, 1000);
    return () => clearInterval(id);
  }, [timed, start]);

  const dots = (
    <div className="dots">
      <span />
      <span />
      <span />
    </div>
  );
  // 头几秒不报时:答得快的时候跳一下数字，看着像出了故障。
  const secs = timed && elapsed >= 3 ? `${elapsed}s` : "";
  if (!label && !secs) return dots;
  return (
    <div className="wait-note" role="status">
      {dots}
      {label ? <span className="wait-note-text">{label}</span> : null}
      {secs ? <span className="wait-note-time">{secs}</span> : null}
    </div>
  );
}

/* ── 输入框 ─────────────────────────────────── */

const MODE_LABEL: Record<string, string> = {
  default: "每次询问",
  acceptEdits: "自动接受编辑",
  plan: "规划模式",
  auto: "自动判危",
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
 * 输入框里的一段内容：一截文字、一个文件引用块，或一条斜杠命令/技能块。
 *
 * 输入框是 contenteditable 而不是 textarea —— 引用块要和文字**排在
 * 同一行**（用户是在句子中间点名文件的："打开 [index.html] 看看"），
 * 而 textarea 只能装纯文本，块只能堆到框外面去，读起来就和正文脱节了。
 *
 * 命令/技能同样不能是一段可被改坏的 `/compact` 字符串：选中之后变成
 * 色块，退格整块删掉，和普通输入一眼能分开。
 */
type Seg =
  | { kind: "text"; value: string }
  | { kind: "ref"; value: string }
  | { kind: "cmd"; value: string };

/** 斜杠名：字母数字、中文、冒号命名空间。名字里不含 `/`，免得把 /usr/bin 认成命令。 */
const SLASH_CH = String.raw`[\w\p{L}\p{N}:-]`;
const SLASH_QUERY_RE = new RegExp(`^/(${SLASH_CH}*)$`, "u");
const SLASH_LEAD_RE = new RegExp(`^/(${SLASH_CH}+)(\\s)([\\s\\S]*)$`, "u");
const SLASH_SUBMIT_RE = new RegExp(`^/(${SLASH_CH}+)\\s*([\\s\\S]*)$`, "u");
const SLASH_HEAD_RE = new RegExp(`^/(${SLASH_CH}+)(?=\\s|$)`, "u");

const drafts = new Map<string, Seg[]>();

const CHIP_ICON =
  '<svg width="11" height="11" viewBox="0 0 16 16" fill="none" aria-hidden="true">' +
  '<path d="M9 1.8H4.5a1 1 0 0 0-1 1v10.4a1 1 0 0 0 1 1h7a1 1 0 0 0 1-1V5.3L9 1.8z" ' +
  'stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/>' +
  '<path d="M8.9 2v3.4h3.4" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/></svg>';

/** 造一个引用块。`contenteditable=false` 让它在编辑器里是一个整体。
 *  文件名走 `data-label` + CSS `::after`，块里不放文本节点 —— WebKit
 *  否则会把光标塞进色块内部，退格先"走进去"再删。 */
function chipEl(path: string): HTMLElement {
  const span = document.createElement("span");
  span.className = "ref-chip";
  span.contentEditable = "false";
  span.dataset.path = path;
  span.dataset.label = basename(path) || path;
  span.title = path;
  span.innerHTML = CHIP_ICON;
  return bindChip(span);
}

/** 造一个命令/技能色块。名字只走 dataset，不拼进 HTML。 */
function cmdChipEl(name: string): HTMLElement {
  const span = document.createElement("span");
  span.className = "cmd-chip";
  span.contentEditable = "false";
  span.dataset.cmd = name;
  span.dataset.label = `/${name}`;
  span.title = `/${name}`;
  return bindChip(span);
}

function isChip(node: Node | null): node is HTMLElement {
  return node instanceof HTMLElement && (node.dataset.path != null || node.dataset.cmd != null);
}

function chipAround(node: Node | null, root: HTMLElement): HTMLElement | null {
  let n: Node | null = node;
  while (n && n !== root) {
    if (isChip(n)) return n;
    n = n.parentNode;
  }
  return null;
}

function skipEmpty(node: Node | null, dir: 1 | -1): Node | null {
  let n = node;
  while (n && n.nodeType === Node.TEXT_NODE && !(n.nodeValue ?? "").length) {
    n = dir === 1 ? n.nextSibling : n.previousSibling;
  }
  return n;
}

function placeCaretAfter(chip: HTMLElement) {
  const sel = window.getSelection();
  if (!sel) return;
  let next = chip.nextSibling;
  if (!next || next.nodeType !== Node.TEXT_NODE) {
    next = document.createTextNode("");
    chip.after(next);
  }
  const r = document.createRange();
  r.setStart(next, 0);
  r.collapse(true);
  sel.removeAllRanges();
  sel.addRange(r);
}

function placeCaretBefore(chip: HTMLElement) {
  const sel = window.getSelection();
  if (!sel) return;
  let prev = chip.previousSibling;
  if (!prev || prev.nodeType !== Node.TEXT_NODE) {
    prev = document.createTextNode("");
    chip.before(prev);
  }
  const r = document.createRange();
  r.setStart(prev, prev.nodeValue?.length ?? 0);
  r.collapse(true);
  sel.removeAllRanges();
  sel.addRange(r);
}

/** 点在块上时把光标放到外侧，不要让 WebKit 把插入点放进边框里。 */
function bindChip(span: HTMLElement): HTMLElement {
  span.addEventListener("mousedown", (e) => {
    e.preventDefault();
    const root = span.parentElement;
    if (!root) return;
    root.focus();
    const mid = span.getBoundingClientRect().left + span.getBoundingClientRect().width / 2;
    if (e.clientX < mid) placeCaretBefore(span);
    else placeCaretAfter(span);
  });
  return span;
}

function removeChip(chip: HTMLElement) {
  const pad = chip.nextSibling;
  const dropPad =
    pad?.nodeType === Node.TEXT_NODE && (pad.nodeValue === " " || pad.nodeValue === "");
  placeCaretBefore(chip);
  chip.remove();
  if (dropPad) pad.remove();
}

/**
 * 光标紧挨着的块。`before` = 块在光标前面（退格要删的那个）。
 * 插块时留下的那个空格算"紧挨着"，好一次退格把块带走。
 */
function adjacentChip(root: HTMLElement, side: "before" | "after"): HTMLElement | null {
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0 || !sel.isCollapsed) return null;
  const range = sel.getRangeAt(0);
  if (!root.contains(range.startContainer)) return null;

  const inside = chipAround(range.startContainer, root);
  if (inside) return inside;

  const node = range.startContainer;
  const offset = range.startOffset;

  if (node === root) {
    const child = root.childNodes[side === "before" ? offset - 1 : offset] ?? null;
    return isChip(child) ? child : null;
  }

  if (node.nodeType !== Node.TEXT_NODE) return null;
  const text = node.nodeValue ?? "";
  if (side === "before") {
    if (offset > 0 && !(text.slice(0, offset).trim() === "" && text.slice(offset) === "")) {
      return null;
    }
    const prev = skipEmpty(node.previousSibling, -1);
    return isChip(prev) ? prev : null;
  }
  if (offset < text.length && !(text.slice(offset).trim() === "" && text.slice(0, offset) === "")) {
    return null;
  }
  const next = skipEmpty(node.nextSibling, 1);
  return isChip(next) ? next : null;
}

/** 方向键跨过整块，退格/删除一次拿掉整块。处理了就返回 true。 */
function handleChipKey(e: { key: string; altKey: boolean; metaKey: boolean; ctrlKey: boolean }, root: HTMLElement): boolean {
  const key = e.key;
  if (key === "Backspace") {
    const chip = adjacentChip(root, "before");
    if (!chip) return false;
    removeChip(chip);
    return true;
  }
  if (key === "Delete") {
    const chip = adjacentChip(root, "after");
    if (!chip) return false;
    removeChip(chip);
    return true;
  }
  if (key === "ArrowLeft" && !e.altKey && !e.metaKey && !e.ctrlKey) {
    const chip =
      chipAround(window.getSelection()?.anchorNode ?? null, root) ?? adjacentChip(root, "before");
    if (!chip) return false;
    placeCaretBefore(chip);
    return true;
  }
  if (key === "ArrowRight" && !e.altKey && !e.metaKey && !e.ctrlKey) {
    const chip =
      chipAround(window.getSelection()?.anchorNode ?? null, root) ?? adjacentChip(root, "after");
    if (!chip) return false;
    placeCaretAfter(chip);
    return true;
  }
  return false;
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
        const cmd = child.dataset["cmd"];
        if (path) {
          push({ kind: "ref", value: path });
        } else if (cmd) {
          push({ kind: "cmd", value: cmd });
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
    else if (s.kind === "ref") el.appendChild(chipEl(s.value));
    else el.appendChild(cmdChipEl(s.value));
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
  return segs
    .map((s, i) => {
      if (s.kind === "text") return s.value;
      if (s.kind === "cmd") return `/${s.value}`;
      const next = segs[i + 1];
      return mentionToken(s.value, next?.kind === "text" ? next.value : "");
    })
    .join("");
}

/** 把开头的 `/已知命令 ` 收成色块。已经有块、或名字还不完整，原样返回。 */
function promoteLeadingCmd(segs: Seg[], known: Set<string>): Seg[] | null {
  if (segs.some((s) => s.kind === "cmd")) return null;
  const first = segs[0];
  if (first?.kind !== "text") return null;
  const m = SLASH_LEAD_RE.exec(first.value);
  const [, name, gap, rest] = m ?? [];
  if (name === undefined || gap === undefined || rest === undefined) return null;
  if (!known.has(name)) return null;
  return [{ kind: "cmd", value: name }, { kind: "text", value: gap + rest }, ...segs.slice(1)];
}

/**
 * 引用块 → 正文里的 `@路径`。裸写法认不回来就加引号。
 *
 * 断在哪里由解析器说了算，所以这里直接**拿解析器试一遍**：路径带空格
 * （`@/tmp/报表 (1).xlsx`）、或者后面紧跟着别的字（`@src/a.rs然后改` ——
 * 中文不写空格，这很常见）都会被吞掉半截，只有引号形式才回得来。
 */
function mentionToken(path: string, after = ""): string {
  const bare = `@${path}`;
  const [span] = extractMentionSpans(bare + after);
  const intact = span?.path === path && span.index === 0 && span.length === bare.length;
  return intact ? bare : `@"${path}"`;
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
  // 边界规则与内核一致：中文后面直接敲 `@` 也要出菜单（"读下@" 是中文
  // 用户的常态写法，要求先打个空格等于让他们用不了这个菜单）。
  return /(?:^|[^A-Za-z0-9._%+-])@([^\s@]*)$/.exec(before)?.[1];
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
    const m = /(^|[^A-Za-z0-9._%+-])@[^\s@]*$/.exec(before);
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
 * Chat 按会话 id 重挂载，Composer 的本地 state 跟着一起丢。少了这层
 * 缓存，模式就退回全局默认值显示，而宿主那边还是用户选的那个 ——
 * 屏幕上写着「每次询问」，实际每一步都在静默放行。
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
 * 每个会话待发的截图。和 drafts 同一个问题：Chat 按会话 id 重挂载，
 * 粘贴的图是组件 state，切走再切回就没了 —— 文字有 drafts 兜着，
 * 图同样是用户放进输入框的内容，不该丢。发送或删除后由同步 effect 清掉。
 */
const shotsCache = new Map<string, Shot[]>();

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

/** 粘贴快捷键在界面上怎么写。 */
const PASTE_KEY = navigator.userAgent.includes("Mac") ? "⌘V" : "Ctrl+V";

/** 看着像一条绝对路径吗。三种写法:`/a/b`、`file://…`、`C:\a\b` 或 UNC。 */
function looksAbsolute(line: string): boolean {
  return (
    line.startsWith("/") ||
    line.startsWith("file://") ||
    line.startsWith("\\\\") ||
    /^[A-Za-z]:[\\/]/.test(line)
  );
}

/**
 * 这次粘贴带的是附件吗（图、或在文件管理器里复制的文件）。
 *
 * 三条判据满足一条就算:
 * - `files` 有东西 —— 截图这种剪贴板里躺着像素的；
 * - types 里有 `Files` —— webview 认出了文件；
 * - 文字整段都是绝对路径 —— 在访达里 ⌘C 一个文件，WebKit 只把**路径当
 *   文字**递过来，前两条都是空的。真正的路径要再问一次系统粘贴板
 *   （见 `clipboardPaths`），这里只负责决定"值不值得问"。
 *
 * 宁可问多了:一行以 `/` 开头的普通文字（shell 命令、注释）会白问一次
 * IPC，然后按文本粘贴，用户看不出区别。
 */
function hasAttachment(dt: DataTransfer | null): boolean {
  if (!dt) return false;
  if (dt.files.length > 0 || dt.types.includes("Files")) return true;
  const lines = dt
    .getData("text/plain")
    .split("\n")
    .filter((l) => l.trim());
  return lines.length > 0 && lines.every(looksAbsolute);
}

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

/** 与 `mentions.rs` 的 `is_stop_punct` 对齐：这些字符在路径里几乎不会出现。 */
const MENTION_STOP = new Set("，。；：、！？）（「」《》“”");

/**
 * `@` 前面这个字符算不算边界（与 `mentions.rs` 的 `is_mention_boundary`
 * 对齐 —— 内核认不认和界面画不画必须是同一条规则）。
 *
 * 反着定义：只有 ASCII 标识符字符才**不是**边界，那正是 `me@example.com`
 * 的形状。中文不写空格，"读下@src/a.rs" 里的 `下` 必须算边界。
 */
const MENTION_GLUE = /[A-Za-z0-9._%+-]/;

/**
 * 正文里一段 `@路径` 标记。`index`/`length` 覆盖整段 token（含 `@` 和引号）。
 *
 * 规则跟内核 `mentions::extract` 对齐：邮箱、行内代码、中文口语不当引用。
 */
interface MentionSpan {
  path: string;
  index: number;
  length: number;
}

/** 长得像路径才画成块 —— `@这里` 这种口语必须留在原文里。 */
function mentionLooksLikePath(s: string): boolean {
  if (!s) return false;
  if (s.includes("/") || s.includes("\\") || s.startsWith(".") || s.startsWith("~")) return true;
  return /^[A-Za-z0-9_.-]+$/.test(s);
}

function mentionTrimPunct(s: string): string {
  return s.replace(/[.,;:!?)"']+$/, "");
}

/**
 * 从用户气泡正文里挑出 `@路径` 标记，好把块画回原位。
 *
 * 发送时乐观气泡带着 `files`；切会话后界面按历史重画。二进制、目录、
 * 读失败的引用不会落成 `user_file` 附件，`files` 就是空的 —— 但标记还
 * 在正文里（见 segsToPrompt），靠它重建，不能只拿附件当白名单。
 */
function extractMentionSpans(text: string): MentionSpan[] {
  const spans: MentionSpan[] = [];
  let inFence = false;
  let offset = 0;
  const lines = text.split("\n");
  for (let li = 0; li < lines.length; li++) {
    const line = lines[li] ?? "";
    if (line.trimStart().startsWith("```")) {
      inFence = !inFence;
    } else if (!inFence) {
      let i = 0;
      let inTick = false;
      let prevBoundary = true;
      while (i < line.length) {
        const c = line[i] ?? "";
        if (c === "`") {
          inTick = !inTick;
          prevBoundary = true;
          i += 1;
          continue;
        }
        if (inTick || c !== "@" || !prevBoundary) {
          prevBoundary = !MENTION_GLUE.test(c);
          i += 1;
          continue;
        }
        if (line[i + 1] === '"') {
          const start = i + 2;
          const end = line.indexOf('"', start);
          if (end >= 0) {
            const raw = line.slice(start, end);
            if (raw.trim()) {
              spans.push({ path: raw, index: offset + i, length: end + 1 - i });
            }
            i = end + 1;
            prevBoundary = false;
            continue;
          }
        }
        let j = i + 1;
        while (j < line.length) {
          const ch = line[j] ?? "";
          if (/\s/u.test(ch) || MENTION_STOP.has(ch)) break;
          j += 1;
        }
        const raw = line.slice(i + 1, j);
        const cleaned = mentionTrimPunct(raw);
        if (mentionLooksLikePath(cleaned)) {
          spans.push({ path: cleaned, index: offset + i, length: 1 + cleaned.length });
        }
        i += 1 + raw.length;
        prevBoundary = false;
      }
    }
    offset += line.length + 1;
  }
  return spans;
}

/**
 * 发出去的正文 → 输入框里的段落：`@路径` 标记原位还原成引用块。
 *
 * 放回输入框的是**发出去的那一份文本**，块在原位留下了 `@路径`（见
 * `segsToPrompt`）。不还原的话，用户看到的是一句夹着裸路径的话，而且
 * 再发一次会连块带标记发出两份同样的引用。
 */
function promptToSegs(text: string, refs: string[] = [], skip: string[] = []): Seg[] {
  const segs: Seg[] = [];
  const seen = new Set<string>();
  let last = 0;
  for (const s of extractMentionSpans(text)) {
    if (s.index > last) segs.push({ kind: "text", value: text.slice(last, s.index) });
    segs.push({ kind: "ref", value: s.path });
    seen.add(s.path);
    last = s.index + s.length;
  }
  if (last < text.length) segs.push({ kind: "text", value: text.slice(last) });
  // 正文里没留下标记的引用（老消息、用户把标记删了）补在末尾 ——
  // 丢掉的话模型就看不到那个文件了。
  for (const r of refs) {
    if (!mentionCovers(seen, r) && !skip.includes(r)) segs.push({ kind: "ref", value: r });
  }
  return segs;
}

/** 历史附件里的绝对路径，和正文里的相对 `@src/a.rs` 算同一个文件。 */
function mentionCovers(seen: Set<string>, file: string): boolean {
  if (seen.has(file)) return true;
  for (const p of seen) {
    if (file.endsWith(`/${p}`) || file.endsWith(`\\${p}`) || p.endsWith(`/${file}`) || p.endsWith(`\\${file}`)) {
      return true;
    }
  }
  return false;
}

/**
 * 用户消息的正文：把 `@路径` 标记画成引用块，其余原样。
 *
 * 用户在输入框里看到的是一行"分别打开 [a] [b]"，气泡里就该是同一行 ——
 * 把块抽出来堆到文字下面，等于把他写的句子拆了。
 */
function UserText({ text, files = [] }: { text: string; files?: string[] }) {
  const lead = SLASH_HEAD_RE.exec(text);
  const cmdName = lead?.[1];
  const body = lead ? text.slice(lead[0].length).replace(/^\s/, "") : text;

  const fileNodes = (src: string): ReactNode => {
    const spans = extractMentionSpans(src);
    if (spans.length === 0 && files.length === 0) return src;
    const out: React.ReactNode[] = [];
    const seen = new Set<string>();
    let last = 0;
    for (const s of spans) {
      if (s.index > last) out.push(src.slice(last, s.index));
      out.push(<FileChip key={`${s.path}-${s.index}`} path={s.path} />);
      seen.add(s.path);
      last = s.index + s.length;
    }
    if (last < src.length) out.push(src.slice(last));
    const orphans = files.filter((f) => !mentionCovers(seen, f));
    return (
      <>
        {out}
        {orphans.map((p) => (
          <FileChip key={`orphan-${p}`} path={p} />
        ))}
      </>
    );
  };

  if (!cmdName) {
    return <>{fileNodes(body)}</>;
  }

  return (
    <>
      <CmdChip name={cmdName} />
      {body || files.length > 0 ? <> {fileNodes(body)}</> : null}
    </>
  );
}

function FileChip({ path }: { path: string }) {
  return (
    <span className="ref-chip static" title={path}>
      <FileIcon />
      {basename(path)}
    </span>
  );
}

function CmdChip({ name }: { name: string }) {
  return (
    <span className="cmd-chip static" title={`/${name}`}>
      /{name}
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
        <Chevron open={open} />
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
  workspaceMissing,
  onMissingWorkspace,
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
  withdrawn,
  onWithdrawnRestored,
  onOpenSettings,
  insertText,
  onInserted,
  armed = true,
}: {
  sessionId: string;
  /** 会话的项目根。斜杠命令要按它找项目级 commands/。 */
  workspace: string;
  workspaceMissing?: boolean;
  onMissingWorkspace?: () => void;
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
  /** 被撤回的提问（模型没开口就停了）。放回输入框，然后 `onWithdrawnRestored`。 */
  withdrawn: WithdrawnPrompt | null;
  onWithdrawnRestored: () => void;
  onOpenSettings: () => void;
  /** 外部要塞进来的一段文字（终端选中的输出）。null = 没有。 */
  insertText?: string | null;
  onInserted?: () => void;
  /** 前台才接全局拖放 / 粘贴。隐藏的保活实例不能跟前台抢。 */
  armed?: boolean;
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
  /** 这个会话可用的斜杠命令 + 技能。每次挂载拉一次（用户加了 .md 切一下会话就有）。 */
  const [commands, setCommands] = useState<SlashCommand[]>([]);
  /** 已经落成色块的那条命令/技能名。有它就算有内容，占位提示该让开。 */
  const [cmdName, setCmdName] = useState<string | null>(null);
  /** 补全菜单里高亮到第几条。 */
  const [slashPick, setSlashPick] = useState(0);
  /** `@` 引用的候选文件。 */
  const [fileHits, setFileHits] = useState<string[]>([]);
  const [filePick, setFilePick] = useState(0);
  /** 光标前那个没敲完的 `@查询`。undefined = 不在引用语境里。 */
  const [mentionQuery, setMentionQuery] = useState<string | undefined>(undefined);
  /** 斜杠命令的执行反馈（压缩中、展开失败）。 */
  const [slashNote, setSlashNote] = useState("");
  /** 待发的图。发出去就清空。挂载时从模块级缓存恢复（见 shotsCache）。 */
  const [shots, setShots] = useState<Shot[]>(() => shotsCache.get(sessionId) ?? []);

  // 写通到模块级缓存。挂在 effect 而不是每个 setShots 调用点：
  // 调用点有五六处（粘贴、拖放、删除、发送、失败回滚），漏一处
  // 就是一个静默丢图的洞。
  useEffect(() => {
    if (shots.length) shotsCache.set(sessionId, shots);
    else shotsCache.delete(sessionId);
  }, [sessionId, shots]);
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
    if (m === cfg.activeModel && activeProvider?.id === cfg.activeProvider) return;
    // 菜单里列的是 activeProvider（含 providers[0] 兜底）的模型，所以
    // provider 要一起写。只写 activeModel 的话，active 为空时会留下
    // 「模型有值、provider 是空 id」的配置 —— keyStatus 按空 id 查不到，
    // 表现为 key 已保存、横幅却说没配。
    void saveConfig({
      ...cfg,
      activeProvider: activeProvider?.id ?? cfg.activeProvider,
      activeModel: m,
    })
      .then(onConfig)
      .catch(() => {});
  };

  // 技能也在这份清单里 —— 宿主那边把命令和技能并成了一条发现管道
  // （`slash::discover`）。这里曾经自己拉一次 skillsList 再合并，那是
  // 两个真相：优先级规则（内置 > 命令 > 技能）在两处各写一遍，改一边
  // 就会不一致。
  useEffect(() => {
    let alive = true;
    void slashCommands(workspace)
      .then((cmds) => {
        if (alive) setCommands(cmds);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [workspace]);

  /** 把编辑区当前的内容读进 state（每次输入、每次光标移动后调）。 */
  const sync = () => {
    const el = ref.current;
    if (!el) return;
    let segs = readEditor(el);
    const known = new Set(commands.map((c) => c.name));
    const promoted = promoteLeadingCmd(segs, known);
    if (promoted) {
      writeEditor(el, promoted);
      caretToEnd(el);
      segs = promoted;
    }
    const text = segsText(segs);
    const paths = segs.flatMap((s) => (s.kind === "ref" ? [s.value] : []));
    const cmd = segs.find((s) => s.kind === "cmd")?.value ?? null;
    setDraftRaw(text);
    setRefs(paths);
    setCmdName(cmd);
    setMentionQuery(queryAtCaret(el));
    // 删光内容后浏览器常留一个 `<br>`，读出来是个 "\n"。当成有内容的话，
    // 占位提示不再出现、草稿缓存里也会存下一堆看不见的空行。
    if (text.trim() || paths.length || cmd) drafts.set(sessionId, segs);
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
   * `[约束]` 只在"整条文字都要被替换"时用（Esc 清掉半截 `/xxx`）。
   * 别拿它做追加 —— 块会被重排到前面去，用户会看到自己刚插在句中的
   * 引用莫名其妙跳到了句首。要在光标处加东西用 `insertChipAtCaret`。
   */
  const replaceText = (v: string) => {
    const el = ref.current;
    if (!el) return;
    const keep = readEditor(el).filter((s) => s.kind === "ref" || s.kind === "cmd");
    setContent(v ? [{ kind: "text", value: v }, ...keep] : keep);
  };

  // 终端选中的那段输出：追加到现有草稿后面，不是替换。
  //
  // 包在代码围栏里 —— 报错栈里的尖括号和缩进不这么处理会被 markdown
  // 吃掉一半。追加完把焦点放回输入框，用户接着就能在前面补一句
  // "这个报错怎么回事"，那才是他按下那个键的目的。
  const insertedRef = useRef(onInserted);
  insertedRef.current = onInserted;
  useEffect(() => {
    if (!insertText) return;
    const el = ref.current;
    if (!el) return;
    const cur = readEditor(el);
    const prefix = segsText(cur).trim() ? "\n\n" : "";
    setContent([...cur, { kind: "text", value: `${prefix}\`\`\`\n${insertText}\n\`\`\`\n` }]);
    el.focus();
    caretToEnd(el);
    insertedRef.current?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [insertText]);

  // 切会话：编辑区是非受控的，组件复用时内容不会自己跟着换。
  // 顺带把焦点放进去 —— contenteditable 不吃 autoFocus（React 只对
  // 表单元素生效），少了这一步切完会话得先点一下才能打字。
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    writeEditor(el, drafts.get(sessionId) ?? []);
    if (armed) {
      el.focus();
      caretToEnd(el);
    }
    sync();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  // 保活的会话切回来：编辑区没重挂，得自己把焦点要回来。
  useEffect(() => {
    if (!armed) return;
    ref.current?.focus();
  }, [armed]);

  // 补全菜单只在"还没敲空格"时出：`/co` 出菜单，`/compact 参数` 不出 ——
  // 后者用户已经选定命令在写参数了，菜单只会挡住视线。
  const slashQuery = cmdName ? undefined : SLASH_QUERY_RE.exec(draft)?.[1];
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

  /** 选中一条命令/技能：收成色块，光标停在后面的空格上写参数。 */
  const chooseSlash = (c: SlashCommand) => {
    const el = ref.current;
    if (!el) return;
    const refsOnly = readEditor(el).filter((s) => s.kind === "ref");
    setContent([{ kind: "cmd", value: c.name }, { kind: "text", value: " " }, ...refsOnly]);
    setSlashPick(0);
    el.focus();
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
    if ((!text && shots.length === 0 && refs.length === 0 && !cmdName) || !hasKey || !cfg.activeModel) {
      return;
    }

    // 斜杠命令：内置的当场执行，能展开的展开成 prompt 再走正常发送。
    //
    // 普通技能**不**当命令跑（`expandInline` 为假）—— 只把名字发给模型，
    // 由它用 Skill 工具按需加载正文。展开了就等于把几 KB 正文塞进用户可见
    // 的消息，渐进披露白做。写了 disable-model-invocation 的技能例外：
    // 模型的清单里没有它，不展开谁都跑不了。判据由宿主给，见 slash.rs。
    //
    // 认不出的 `/xxx` 原样发出去 —— 用户可能真想跟模型说这个词。
    const sentSegsNow = ref.current ? readEditor(ref.current) : [];
    const cmdSeg = sentSegsNow.find((s) => s.kind === "cmd");
    const cmd = cmdSeg
      ? commands.find((c) => c.name === cmdSeg.value)
      : (() => {
          const slash = SLASH_SUBMIT_RE.exec(text);
          return slash ? commands.find((c) => c.name === slash[1]) : undefined;
        })();
    if (cmd && (cmd.source === "builtin" || cmd.expandInline)) {
      const args = cmdSeg
        ? sentSegsNow
            .filter((s) => s.kind === "text")
            .map((s) => s.value)
            .join("")
            .trim()
        : (SLASH_SUBMIT_RE.exec(text)?.[2] ?? "");
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
        // 进行中的提示在对话流里（「正在压缩上下文…」），这里不再横幅重复一遍。
        try {
          await compactSession(sessionId);
        } catch (e) {
          setSlashNote(String(e));
        }
      }
      return;
    }
    // 失败时把 `/命令 参数` 和引用块原样放回去：展开出来的 prompt 是
    // 派生物，用户手里那行才是他打的东西。
    const restore = () => {
      const cur = ref.current ? readEditor(ref.current) : [];
      const back: Seg[] = sentRefs
        .filter((r) => !cur.some((s) => s.kind === "ref" && s.value === r))
        .map((value) => ({ kind: "ref", value }));
      setContent([
        { kind: "cmd", value: cmd.name },
        { kind: "text", value: args ? ` ${args} ` : " " },
        ...back,
        ...cur,
      ]);
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

  /**
   * 把一条已经离开输入框的消息放回来（撤回的提问、撤回来改的排队插话）。
   * 原有草稿接在它后面，谁都不丢。
   */
  const putBack = (
    input: { text: string; images: ImageInput[]; refs: string[] },
    imageLabel: string,
  ) => {
    const cur = ref.current ? readEditor(ref.current) : [];
    const held = cur.flatMap((s) => (s.kind === "ref" ? [s.value] : []));
    const gap: Seg[] = segsText(cur).trim() ? [{ kind: "text", value: "\n" }] : [];
    setContent([...promptToSegs(input.text, input.refs, held), ...gap, ...cur]);
    if (input.images.length > 0) {
      setShots((prev) => [
        ...prev,
        ...input.images.map((img, i) => ({
          id: `back-${Date.now()}-${i}`,
          name: `${imageLabel} ${i + 1}`,
          mediaType: img.mediaType,
          data: img.data,
        })),
      ]);
    }
    ref.current?.focus();
  };

  /** 把一条排队插话撤回输入框改。 */
  const editQueued = async (id: string) => {
    const input = await onQueueEdit(id);
    if (!input) return;
    putBack(input, "排队图片");
  };

  // 撤回的提问回到输入框：模型一个字都没给出就被停了，那句话从没被
  // 回答过 —— 用户按停止的意思是"我重说一遍"，而不是"扔掉我刚打的字"。
  //
  // `[约束]` 按 id 记账防重入。撤回往往正好把会话清空，输入框那一刻
  // 从对话区挪回首屏 —— 那是一次**重挂载**，StrictMode 会把挂载时的
  // effect 跑两遍，不挡的话用户看到自己那句话被放回来两份。
  const restoredRef = useRef(onWithdrawnRestored);
  restoredRef.current = onWithdrawnRestored;
  const restoredId = useRef<string | null>(null);
  useEffect(() => {
    if (!withdrawn || restoredId.current === withdrawn.id) return;
    restoredId.current = withdrawn.id;
    putBack(withdrawn, "撤回图片");
    restoredRef.current();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [withdrawn]);

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
    setShots((prev) => {
      const merged = [...prev, ...scaled];
      // 超上限要说出来 —— 静默丢掉的话，用户以为十张全发出去了。
      if (merged.length > MAX_SHOTS) {
        setDropError(`一条消息最多 ${MAX_SHOTS} 张图，已忽略多出的 ${merged.length - MAX_SHOTS} 张。`);
      }
      return merged.slice(0, MAX_SHOTS);
    });
  };

  /** webview 给的 `File`（剪贴板里只有像素的那种）:图片收下，其它的说清为什么不收。 */
  const takeFiles = async (files: File[]) => {
    const images = files.filter((f) => f.type.startsWith("image/"));
    const rest = files.filter((f) => !f.type.startsWith("image/"));

    if (images.length) {
      const read = await Promise.all(images.map(toShot));
      await addShots(read);
    }
    if (rest.length) {
      // 走到这里说明系统没给出路径（`File` 对象自己是没有的）。非图片文件
      // 只能靠路径进对话 —— 引用块认的就是路径。
      setDropError(
        `${rest[0]?.name ?? "这个文件"} 不是图片，而系统没给出它的路径。` +
          `请用左下角的「+」选择，或者在输入框里打 @ 找它。`,
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
        // 光标未必在输入框里 —— 拖放和粘贴发生时焦点常在别处，甚至正
        // 选着对话流里的一段文字。不校正的话块会插到那段选区上去。
        const sel = window.getSelection();
        const inside =
          sel && sel.rangeCount > 0 && el.contains(sel.getRangeAt(0).startContainer);
        if (!inside) caretToEnd(el);
        for (const p of files) {
          // 两种分隔符都认:Windows 上拖进来的是 `C:\proj\a.md`。
          const inWs = p.startsWith(`${workspace}/`) || p.startsWith(`${workspace}\\`);
          insertChipAtCaret(el, inWs ? p.slice(workspace.length + 1) : p);
        }
        sync();
      }
    }
  };

  /**
   * 收下剪贴板里的附件。返回是否真的收下了。
   *
   * 先问宿主要磁盘路径:拿得到就和拖放走同一条路，非图片文件也能变成
   * 引用块。拿不到（剪贴板里只有像素的截图、或非 macOS）再退回 webview
   * 给的 `File`。两样都没有就还给调用方按文字处理。
   */
  const pasteFiles = async (files: File[]): Promise<boolean> => {
    const paths = await clipboardPaths().catch(() => []);
    if (paths.length) {
      await takePaths(paths);
      return true;
    }
    if (files.length) {
      await takeFiles(files);
      return true;
    }
    return false;
  };

  // 拖到窗口任何地方都算数。只认输入框那一小条的话用户得先瞄准，而窗口
  // 大半面积是对话流 —— 拖偏了什么都不会发生，还以为这个功能没做。
  //
  // 处理函数放 ref 里:订阅只在挂载时建一次，而 takePaths 每次渲染都是
  // 新的闭包，直接进依赖数组会让拖放订阅跟着输入框的每一次输入重建。
  const dropRef = useRef(takePaths);
  dropRef.current = takePaths;
  useEffect(() => {
    if (!armed) {
      setDragging(false);
      return;
    }
    return subscribeDragDrop((e) => {
      if (e.kind === "leave") {
        setDragging(false);
        return;
      }
      if (e.kind === "enter") {
        // 没有路径的拖拽（拖一段文字、拖网页里的图）不亮落点提示 ——
        // 亮了却接不住是更糟的反馈。
        setDragging(e.paths.length > 0);
        return;
      }
      if (e.kind !== "drop") return;
      setDragging(false);
      if (e.paths.length) {
        void dropRef.current(e.paths);
      } else {
        setDropError(
          "拖进来的东西在磁盘上没有对应文件（多半是从网页里直接拖的图）。" +
            `复制它，再回到这里 ${PASTE_KEY}。`,
        );
      }
    });
  }, [armed]);

  // 焦点不在输入框时 ⌘V 也算数 —— 在 Finder 里复制完文件回到窗口，第一
  // 反应是直接粘，不会先去点一下输入框。
  //
  // 别处的可编辑元素（终端、设置里的输入框）不抢:那是人家的内容。
  const pasteRef = useRef(pasteFiles);
  pasteRef.current = pasteFiles;
  useEffect(() => {
    if (!armed) return;
    const onPaste = (e: ClipboardEvent) => {
      const t = e.target;
      if (t instanceof Element && t.closest("input, textarea, [contenteditable='true']")) return;
      if (!hasAttachment(e.clipboardData)) return;
      e.preventDefault();
      void pasteRef.current(Array.from(e.clipboardData?.files ?? []));
    };
    document.addEventListener("paste", onPaste);
    return () => document.removeEventListener("paste", onPaste);
  }, [armed]);

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
      {/* 落点提示铺满整个窗口 —— 因为落点确实是整个窗口，提示只圈住输入框
          会让人以为必须拖到那一条上。 */}
      {dragging ? (
        <div className="drop-veil" aria-hidden>
          <div className="drop-veil-card">松手，加进输入框</div>
        </div>
      ) : null}

      {workspaceMissing ? (
        <button className="key-banner" onClick={onMissingWorkspace}>
          项目目录已经不在磁盘上。点这里移除或另选目录。
        </button>
      ) : null}

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
              <CmdChip name={c.name} />
              {c.argumentHint ? <span className="slash-hint">{c.argumentHint}</span> : null}
              <span className="slash-desc">{c.description}</span>
              {c.source !== "builtin" ? (
                <span className="slash-src">
                  {c.source === "skill" ? "技能" : c.source === "project" ? "项目" : "全局"}
                </span>
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
              <FileChip path={p} />
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
          className={draft.trim() || refs.length || cmdName ? "composer-input" : "composer-input empty"}
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
          // 粘贴板里的图和文件直接收下。这是"看这个截图"最常用的发法 ——
          // 截完图 ⌘V 就完事，不用先存盘再选文件；在 Finder 里复制的文件
          // 同理，粘进来就是一个引用块。
          onPaste={(e) => {
            const files = Array.from(e.clipboardData.files);
            const text = e.clipboardData.getData("text/plain");
            // 富文本粘贴一律降级成纯文本：contenteditable 默认会把网页的
            // 样式、图片、甚至整个表格结构原样塞进来。
            e.preventDefault();
            if (hasAttachment(e.clipboardData)) {
              // 问宿主要路径是一次 IPC，所以只在"看着像附件"时才走这条 ——
              // 每敲一次 ⌘V 都异步一下，粘长文本时会看见一帧空白。
              void pasteFiles(files).then((took) => {
                // 只是一段以 / 开头的普通文字（比如一行 shell 命令），
                // 按纯文本粘贴，别把它吃掉。
                if (took) return;
                ref.current?.focus();
                document.execCommand("insertText", false, text);
                sync();
              });
              return;
            }
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
            // 色块当原子：退格一次整块删掉，方向键整块跳过。
            // 交给浏览器的话，WebKit 会先把光标塞进块里（或先选中再删）。
            if (
              ref.current &&
              !e.nativeEvent.isComposing &&
              !imeRef.current &&
              handleChipKey(e, ref.current)
            ) {
              e.preventDefault();
              sync();
              return;
            }

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
            // 空输入时 Esc 中断当前轮 —— 想停不必去够那个停止按钮。
            // 有草稿时 Esc 留给"清空/退出引用"这类局部撤销，不误伤。
            if (e.key === "Escape" && busy && !draft.trim() && !e.nativeEvent.isComposing) {
              e.preventDefault();
              onStop();
              return;
            }
            // 敲空格且整段正好是一条已知命令：收成色块，别留下 `/compact ` 纯文字。
            if (e.key === " " && !e.nativeEvent.isComposing && !imeRef.current) {
              const typed = SLASH_QUERY_RE.exec(draft)?.[1];
              const exact = typed ? commands.find((c) => c.name === typed) : undefined;
              if (exact) {
                e.preventDefault();
                chooseSlash(exact);
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
          <div className="composer-tools">
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
            {/* 窄列藏起来：三个 pill 并排是挤的源头，换服务方/模型去设置里也能做。 */}
            <div className="composer-picks">
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
            </div>
          </div>
          <div className="composer-actions">
            {tokens.input + tokens.output > 0 ? (
              // "a / b" 会被读成"已用 / 上限"，箭头形式没有歧义
              <span className="usage" title="本会话累计 token：↑输入 ↓输出">
                ↑{fmtTokens(tokens.input)} ↓{fmtTokens(tokens.output)}
              </span>
            ) : null}
            {/* 停止常驻：只要在忙就显示，不再被"打了字"的发送按钮顶掉 ——
                想中止不必先清空输入。有草稿时它和发送并排，各司其职。 */}
            {busy ? (
              <button type="button" className="send stop" onClick={onStop} title="停止 (Esc)" aria-label="停止">
                <StopIcon />
              </button>
            ) : null}
            {!busy || draft.trim() || shots.length > 0 || refs.length > 0 || cmdName ? (
              <button
                type="submit"
                className="send"
                disabled={
                  (!draft.trim() && shots.length === 0 && refs.length === 0 && !cmdName) ||
                  !hasKey ||
                  !cfg.activeModel
                }
                title={
                  busy ? "排队发送（当前任务完成后自动发出）" : cfg.activeModel ? "发送" : "先选择一个模型"
                }
                aria-label={busy ? "排队发送" : cfg.activeModel ? "发送" : "先选择一个模型"}
              >
                <ArrowUpIcon />
              </button>
            ) : null}
          </div>
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

/**
 * 上拉菜单的公共行为：点外面关、Esc 关（焦点还给 pill）、上下键在
 * 菜单项间移动。三个 pill 菜单（模式/服务方/模型）共用，键盘模型
 * 才能一致。
 */
function useDropdown(open: boolean, setOpen: (v: boolean) => void) {
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [open, setOpen]);

  // 打开后把焦点放到当前选中项，键盘用户不用先按好几下 Tab
  useEffect(() => {
    if (!open) return;
    const root = rootRef.current;
    const target =
      root?.querySelector<HTMLButtonElement>(".menu-item.active") ??
      root?.querySelector<HTMLButtonElement>(".menu-item");
    target?.focus();
  }, [open]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!open) return;
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
      rootRef.current?.querySelector<HTMLButtonElement>(".picker-pill")?.focus();
      return;
    }
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const items = [...(rootRef.current?.querySelectorAll<HTMLButtonElement>(".menu-item") ?? [])];
      if (!items.length) return;
      const cur = items.indexOf(document.activeElement as HTMLButtonElement);
      const n = items.length;
      const next = e.key === "ArrowDown" ? (cur + 1) % n : (cur - 1 + n) % n;
      items[next]?.focus();
    }
  };

  return { rootRef, onKeyDown };
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
  const { rootRef, onKeyDown } = useDropdown(open, setOpen);
  // 危险模式常态化后不能和安全模式长得一样 —— pill 要一直带警示色
  const danger = Boolean(MODE_WARN[mode]);

  return (
    <div className="mode-menu" ref={rootRef} onKeyDown={onKeyDown}>
      <button
        type="button"
        className={danger ? "pill picker-pill pill-danger" : "pill picker-pill"}
        title={danger ? `${MODE_LABEL[mode]}（${MODE_WARN[mode]}）` : (MODE_LABEL[mode] ?? mode)}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        {danger ? <span className="pill-danger-dot" aria-hidden /> : null}
        <span className="pick-label">{MODE_LABEL[mode] ?? mode}</span>
        <Chevron down open={open} />
      </button>
      {open ? (
        <div className="menu" role="menu">
          {(Object.keys(MODE_LABEL) as PermissionMode[]).map((m) => (
            <button
              key={m}
              type="button"
              role="menuitemradio"
              aria-checked={m === mode}
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
  const isEmpty = items.length === 0;
  const { rootRef, onKeyDown } = useDropdown(open, setOpen);

  return (
    <div className="mode-menu" ref={rootRef} onKeyDown={onKeyDown}>
      <button
        type="button"
        className="pill picker-pill"
        title={isEmpty ? (emptyHint ?? title) : label}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => (isEmpty ? onEmpty?.() : setOpen(!open))}
      >
        <span className="pick-label">{label}</span>
        <Chevron down open={open} />
      </button>
      {open && !isEmpty ? (
        <div className="menu" role="menu">
          {items.map((it) => (
            <button
              key={it.id}
              type="button"
              role="menuitemradio"
              aria-checked={Boolean(it.active)}
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

/**
 * 浏览器：Chrome 的轮廓（内置浏览器跑的就是 Chromium）。
 *
 * 圆形在一排矩形图标里本身就够显眼，不用靠颜色 —— 旁边几个都是矩形
 * 加一道线，只差线的方向，扫一眼分不出谁是谁。
 */
function BrowserIcon() {
  return (
    <svg
      width="15"
      height="15"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      aria-hidden
    >
      <circle cx="12" cy="12" r="9.5" />
      <circle cx="12" cy="12" r="4" />
      <path d="M21.17 8H12M3.95 6.06 8.54 14M10.88 21.94 15.46 14" />
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

/** 改动一览：上面一个加号、下面一道减号 —— diff 的通用符号（octicon 同款）。 */
function DiffIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M8 2.5v7M4.5 6h7"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
      />
      <path d="M4.5 13h7" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
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

/**
 * 空会话上方的品牌标记：应用图标里那个 R，只留线条。
 *
 * 用的不是 icons/ 里那张位图 —— 那是给 Dock 和安装包用的立体图标，
 * 摆进这块纯色界面里像贴了一张贴纸。这里跟界面其余图标同一套语言：
 * 描边、圆角端点、颜色跟着 currentColor 走。
 *
 * 三笔各自成类，是为了让它动起来：这个 R 拆开看就是个小人 —— 上面
 * 的半圆是头、竖笔是身子、右下那折是腿，腿要能自己甩（见 .riot-mark）。
 */
function RiotMark() {
  return (
    <svg className="riot-mark" width="42" height="42" viewBox="0 0 24 24" fill="none" aria-hidden>
      <g stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
        <path className="riot-mark-body" d="M7.4 4.2v15.6" />
        <path className="riot-mark-head" d="M7.4 4.2h5A4.2 4.2 0 0 1 12.4 12.6H7.4" />
        <path className="riot-mark-leg" d="M11.6 12.6l5 4.9-2.7 2.3" />
      </g>
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
