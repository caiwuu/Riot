import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import {
  addProject,
  type ConfigStatus,
  createSession,
  deleteSession,
  getConfig,
  type ImageInput,
  listSessions,
  type PermissionMode,
  openInBrowser,
  pickDirectory,
  probeDirs,
  removeProject,
  renameSession,
  revealInFinder,
  type SessionInfo,
  setWindowTitle,
  subscribeFullscreen,
} from "./bridge";
import { BrowserPanel } from "./components/BrowserPanel";
import { GitChangesPanel } from "./components/GitChangesPanel";
import { SessionChangesBar } from "./components/SessionChangesBar";
import { ScopePanel } from "./components/ScopePanel";
import { SessionSettings } from "./components/SessionSettings";
import { ConfirmDialog, type ConfirmRequest } from "./components/ConfirmDialog";
import { MissingProjectDialog } from "./components/MissingProjectDialog";
import { ProjectRootContext } from "./components/Markdown";
import {
  PermissionDialog,
} from "./components/PermissionDialog";
import { Settings } from "./components/Settings";
import { TerminalPanel } from "./components/TerminalPanel";
import { hasActiveTodos, TodoPanel } from "./components/TodoPanel";
import {
  forgetSession,
  useSession,
  waitStartedAt,
} from "./hooks/useSession";
import { useAppUpdate } from "./hooks/useAppUpdate";
import { Transcript, transcriptView } from "./components/Transcript";
import { ContextMenu, type MenuState, Resizer, TopBar } from "./components/chrome";
import { Sidebar } from "./components/Sidebar";
import { Welcome } from "./components/Welcome";
import { Composer } from "./components/Composer";
import {
  FolderIcon,
  RiotMark,
} from "./components/icons";
import { basename } from "./pathDisplay";

/**
 * 布局照着 Codex 桌面端：左侧按项目分组的会话列表（可拖宽、可收起），
 * 主区顶部一条工具栏，中间是对话流，右侧一个抽屉（放浏览器），底部一条
 * 终端面板。三块附属面板的尺寸都能拖，且记住上次的位置。
 *
 * 没有"当前工作区"这个全局概念 —— 每个会话在创建时绑定自己的项目
 * 目录，之后永不改变。多项目并行时谁也不影响谁；"换了目录代码还写进
 * 旧目录"那类 bug 在这个模型下没有生存空间。
 */
/* ── 布局尺寸 ───────────────────────────────── */

/** 布局尺寸的持久化键。存 localStorage —— 纯 UI 状态，不值得进宿主配置。 */
const LS = {
  sidebar: "riot.layout.sidebar",
  sidebarOpen: "riot.layout.sidebarOpen",
  drawer: "riot.layout.drawer",
  term: "riot.layout.term",
};



/** 同时保活的会话树上限。切走不卸 DOM，切回才不是白屏。 */
const KEEP_CHATS = 4;

const SIDEBAR = { def: 280, min: 180, max: 420 };
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

/* ── 输入框 ─────────────────────────────────── */
