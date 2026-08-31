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
  browserCloseTab,
  browserNewTab,
  browserSelectTab,
  type ConfigStatus,
  createSession,
  deleteSession,
  encodePlainForComposer,
  getConfig,
  type ImageInput,
  listSessions,
  notify,
  type PermissionMode,
  openInBrowser,
  pickDirectory,
  pickFiles,
  probeDirs,
  type MissedRun,
  removeProject,
  renameSession,
  revealInFinder,
  scheduleAckMissed,
  scheduleDelete,
  scheduleList,
  scheduleMissed,
  scheduleRunNow,
  scheduleSetEnabled,
  type ScheduledTask,
  type SessionInfo,
  setConfig as saveConfig,
  setWindowTitle,
  subscribeFullscreen,
  subscribeScheduleChanges,
  subscribeScheduleRuns,
} from "./bridge";
import { BrowserPanel } from "./components/BrowserPanel";
import { GitChangesPanel } from "./components/GitChangesPanel";
import { SessionChangesBar } from "./components/SessionChangesBar";
import { ScopePanel } from "./components/ScopePanel";
import { SessionSettings } from "./components/SessionSettings";
import { ConfirmDialog, type ConfirmRequest } from "./components/ConfirmDialog";
import {
  FilePreviewPanel,
  ImageLightboxHost,
  openFilePreview,
  subscribeFilePreview,
} from "./components/FilePreview";
import { MissingProjectDialog } from "./components/MissingProjectDialog";
import { ProjectRootContext } from "./components/Markdown";
import {
  PermissionDialog,
} from "./components/PermissionDialog";
import { Settings } from "./components/Settings";
import { closeSessionTerminals, TerminalPanel } from "./components/TerminalPanel";
import { hasActiveTodos, TodoPanel } from "./components/TodoPanel";
import {
  forgetSession,
  useSession,
  waitStartedAt,
} from "./hooks/useSession";
import { useAppUpdate } from "./hooks/useAppUpdate";
import { useBrowserPanel } from "./hooks/useBrowserPanel";
import { newPresetId } from "./lib/prompts";
import { inheritedSampling } from "./lib/sampling";
import { Transcript, transcriptView } from "./components/Transcript";
import {
  ContextMenu,
  type MenuState,
  Resizer,
  TopBar,
  WindowControls,
} from "./components/chrome";
import {
  EMPTY_WORKBENCH,
  tabId,
  WorkbenchEmpty,
  WorkbenchTabs,
  type WorkbenchState,
  type WorkbenchTab,
} from "./components/Workbench";
import { isDoneSchedule, ScheduleDetail, SchedulesPage } from "./components/SchedulesPage";
import { Sidebar } from "./components/Sidebar";
import { SlidePanel, usePresence } from "./components/SlidePanel";
import { Welcome } from "./components/Welcome";
import { Composer, forgetComposerSession } from "./components/Composer";
import {
  FolderIcon,
  RiotMark,
} from "./components/icons";
import { basename, looksAbsPath } from "./pathDisplay";

/**
 * 布局照着 Codex 桌面端：左侧按项目分组的会话列表（可拖宽、可收起），
 * 主区顶部一条工具栏，中间是对话流，右侧一个多标签工作台（浏览器 /
 * Git 改动 / 每个预览文件各占一个标签，共存不互斥，点着切换；以后的
 * 新面板就是新的一种标签），底部一条终端面板。附属面板的尺寸都能拖，
 * 且记住上次的位置。
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
  schedDetail: "riot.layout.schedDetail",
  recency: "riot.session.recency",
};

/** 侧栏按「最近聊过」排。纯 UI 顺序，不进宿主索引。 */
function loadRecency(): Record<string, number> {
  try {
    const raw = localStorage.getItem(LS.recency);
    if (!raw) return {};
    const v: unknown = JSON.parse(raw);
    if (!v || typeof v !== "object") return {};
    const out: Record<string, number> = {};
    for (const [k, n] of Object.entries(v as Record<string, unknown>)) {
      if (typeof n === "number" && Number.isFinite(n)) out[k] = n;
    }
    return out;
  } catch {
    return {};
  }
}



/** 同时保活的会话树上限。切走不卸 DOM，切回才不是白屏。 */
const KEEP_CHATS = 4;

const SIDEBAR = { def: 280, min: 180, max: 420 };
/** 抽屉窄过这个值页面就没法看了，浏览器面板自己也有同样的下限。 */
const DRAWER_MIN = 320;
const TERM = { def: 260, min: 110 };
/** 定时任务详情（占用系统右侧栏的位置，宽度独立于工作台抽屉）。 */
const SCHED_DETAIL = { def: 612, min: 500, max: 1008 };

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
  /** 每个会话最近一次真开聊的时刻。侧栏同项目里按它倒序。 */
  const [recency, setRecency] = useState(loadRecency);
  /**
   * 把会话顶到它那组最前。
   *
   * `[约束]` 只在**真开聊**（开轮）和新建时调，切换会话不算。翻旧会话
   * 找一句话是高频动作，跟着改顺序的话，用户扫两眼列表就被重排一遍，
   * 再也回不到刚才看的位置。
   */
  const touchSession = useCallback((id: string) => {
    setRecency((prev) => {
      const next = { ...prev, [id]: Date.now() };
      localStorage.setItem(LS.recency, JSON.stringify(next));
      return next;
    });
  }, []);
  const sawBusy = useRef(new Set<string>());
  const [booting, setBooting] = useState(true);
  const [showSettings, setShowSettings] = useState(false);
  /** 定时任务页在主区前台（侧栏一级菜单进入，选会话退出）。 */
  const [schedulePage, setSchedulePage] = useState(false);
  /** 详情面板正看着的任务。详情占用系统右侧栏（工作台抽屉的位置）。 */
  const [selectedSchedule, setSelectedSchedule] = useState<string | null>(null);
  /** 详情侧栏宽度（拖出来的值）。 */
  const [schedDetailW, setSchedDetailW] = useState(() => {
    const v = loadPx(LS.schedDetail, SCHED_DETAIL.def);
    // 旧默认 340，加宽之后低于新下限的存值作废。
    return v < SCHED_DETAIL.min ? SCHED_DETAIL.def : v;
  });
  /** 定时任务清单（任务页展示）。 */
  const [schedules, setSchedules] = useState<ScheduledTask[]>([]);
  /** 启动时发现的错过运行。行上标黄；补跑/忽略后逐条消掉。 */
  const [missedSchedules, setMissedSchedules] = useState<MissedRun[]>([]);
  /** 创建定时任务的开场白。走 Composer 的 insertText 通道。 */
  const [schedSnippet, setSchedSnippet] = useState<string | null>(null);
  const [menu, setMenu] = useState<MenuState | null>(null);
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  /** 目录已不在磁盘上、等用户决定怎么处理的那个项目。 */
  const [goneRoot, setGoneRoot] = useState<string | null>(null);
  /** 探测过、确认不存在的项目根。用来在侧栏和欢迎页标「已失效」。 */
  const [missing, setMissing] = useState<Set<string>>(() => new Set());
  const [renaming, setRenaming] = useState<string | null>(null);
  /** 右侧工作台：标签组 + 激活项 + 抽屉开合。标签共存不互斥（Codex
   *  同款），浏览器 / Git 改动 / 每个预览文件各占一个，点着切换。 */
  const [wb, setWb] = useState<WorkbenchState>(EMPTY_WORKBENCH);
  /** 每个会话自己的工作台（工具窗跟会话走：浏览器内容、预览标签本来
   *  就是会话的）。切会话整存整取；会话删除时由 dropSessionWorkbench
   *  清掉。 */
  const workbenchBySession = useRef(new Map<string, WorkbenchState>());
  /** 最近真正看过的预览文件。激活标签不是预览时，保活着的预览面板仍
   *  需要一个"当前文件"定住各 body 的 display —— 用它，免得隐藏的
   *  面板里乱切一通（渲染器会白做适配）。 */
  const lastPreview = useRef<string | null>(null);
  /**
   * 用户主动关过 / 切走过浏览器标签的会话。模型在这些会话里再用浏览器
   * 工具，标签不再自动抢到前台 —— 用户已经表过态，每次工具调用都夺回
   * 焦点等于反复跟他抢屏幕。主动回到浏览器标签视为又想看了，从集合里
   * 移除、恢复自动弹出。存会话 id 而不是一个布尔：别的会话的浏览器
   * 活动不该被这个会话连坐。
   */
  const browserDismissed = useRef(new Set<string>());
  const [showTerm, setShowTerm] = useState(false);
  const [showSessionCfg, setShowSessionCfg] = useState(false);
  /** 递增一次，改动面板重新比对一次。轮次结束时推一下。 */
  const [changesRev, setChangesRev] = useState(0);
  /** 用户从终端选中、要交给模型的一段输出。塞进输入框而不是直接发送 ——
   *  他多半还要在前面补一句"这个报错怎么回事"。 */
  const [termSnippet, setTermSnippet] = useState<string | null>(null);
  /** 用户在浏览器面板"取件"点中的元素选择器。同样塞进输入框而不是直接发 ——
   *  他还要补一句"把它改成…"。和 termSnippet 共用 Composer 的 insertText 通道。 */
  const [pickSnippet, setPickSnippet] = useState<string | null>(null);
  /** 最近看过的会话 id（LRU）。这些 Chat 卸不掉，切回去是显示/隐藏。 */
  const [kept, setKept] = useState<string[]>([]);
  const [sidebarOpen, setSidebarOpen] = useState(
    () => localStorage.getItem(LS.sidebarOpen) !== "0",
  );
  /** 侧栏壳真正改宽度的那一拍。顶栏让位跟这个走，不能跟 sidebarOpen。 */
  const [sidebarVisual, setSidebarVisual] = useState(
    () => localStorage.getItem(LS.sidebarOpen) !== "0",
  );
  /** 终端收起动画那一拍里内容还要显示（见 TerminalPanel 的 visible）。
   *  放在状态区：App 有条件早退，hook 不能出现在那之后。 */
  const termPresent = usePresence(showTerm);
  /** 任务详情占着右侧栏时，工作台抽屉让位。hook 必须在早退之前。 */
  const scheduleDetailOpen = Boolean(
    schedulePage && selectedSchedule && schedules.some((t) => t.id === selectedSchedule),
  );
  const rightDrawerOpen = Boolean(active && wb.open && !scheduleDetailOpen);
  /** 抽屉还在画面上（含收起动画）。内容要留着，否则是空壳在滑。 */
  const drawerPresent = usePresence(rightDrawerOpen);
  /** 壳真正开始改宽度的那一拍。窗口开关跟这个走：跟收起动画对齐
   *  坐回顶栏，设置钮才不会先被主区拽到窗口右缘、再被开关挤回来。 */
  const [drawerVisual, setDrawerVisual] = useState(false);
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
  // 走 useMemo 而不是就地 `?? []`：后者每次渲染都产出新数组，而快捷键
  // effect 依赖它 —— 等于每渲染一次就把 window 的 keydown 解绑重绑一次。
  const projects = useMemo(() => projectList ?? [], [projectList]);
  const activeSession = sessions.find((s) => s.id === active) ?? null;
  const update = useAppUpdate(!booting && !bootError);
  const updateNotice = update.banner;

  /** 激活的工作台标签。抽屉的渲染分支和顶栏开关的亮灭都从它派生。 */
  const activeTab = wb.tabs.find((t) => tabId(t) === wb.active) ?? null;
  /** 正显示着的标签种类。抽屉收起时是 null —— 开关状态别亮着。 */
  const activeKind = wb.open ? (activeTab?.kind ?? null) : null;
  /** 打开着的预览文件（kind=preview 的标签），即预览面板的保活集合。 */
  const previewPaths = useMemo(
    () => wb.tabs.flatMap((t) => (t.kind === "preview" ? [t.path] : [])),
    [wb.tabs],
  );
  /** 浏览器标签开着没有（不管在不在前台）。页面状态的轮询跟着它走 ——
   *  页面标签展开在统一标签栏上，浏览器在后台时标签也得是活的。 */
  const hasBrowserTab = wb.tabs.some((t) => t.kind === "browser");
  const {
    panel: browserPages,
    apply: applyBrowserPanel,
    patchTab: patchBrowserTab,
  } = useBrowserPanel(activeSession?.id ?? null, hasBrowserTab);

  // 记住最近真正看过的预览文件。effect 不带依赖数组没问题 —— 每次
  // 渲染就一次 ref 赋值，比精确依赖便宜也不会错。
  useEffect(() => {
    if (activeTab?.kind === "preview") lastPreview.current = activeTab.path;
  });
  /** 预览面板这个会话里显示过没有。面板的保活是"显示过之后切走不卸"，
   *  而不是"一恢复会话就在 display:none 里首挂" —— 文档渲染器的首屏
   *  适配必须发生在看得见的容器里（见 FilePreviewPanel 的保活注释）。 */
  const [previewWarm, setPreviewWarm] = useState(false);
  useEffect(() => {
    if (activeKind === "preview") setPreviewWarm(true);
  }, [activeKind]);
  /** 预览面板此刻该显示的文件：激活标签是预览就是它；不是（面板在后台
   *  保活）就停在最近看过的那个，别的标签切来切去不动它。 */
  const activePreviewPath =
    activeTab?.kind === "preview"
      ? activeTab.path
      : lastPreview.current && previewPaths.includes(lastPreview.current)
        ? lastPreview.current
        : (previewPaths[previewPaths.length - 1] ?? null);

  /** 打开（或激活）一个标签，抽屉随之展开。不碰 browserDismissed ——
   *  这是"程序把标签带上来"的中性入口；带表态的用户操作走 activateTab。 */
  const openTab = useCallback((tab: WorkbenchTab) => {
    setWb((prev) => {
      const id = tabId(tab);
      const exists = prev.tabs.some((t) => tabId(t) === id);
      return {
        tabs: exists ? prev.tabs : [...prev.tabs, tab],
        active: id,
        open: true,
      };
    });
  }, []);

  /**
   * 用户主动把某个标签带到前台（点标签、空状态、快捷键、"+"菜单）。
   *
   * 从正看着的浏览器切走是"现在不看浏览器"的表态 —— 记进
   * browserDismissed，模型的浏览器活动此后不再抢回焦点；主动回到
   * 浏览器则恢复自动弹出。
   */
  const activateTab = useCallback(
    (tab: WorkbenchTab) => {
      if (activeSession) {
        if (activeKind === "browser" && tab.kind !== "browser") {
          browserDismissed.current.add(activeSession.id);
        }
        if (tab.kind === "browser") {
          browserDismissed.current.delete(activeSession.id);
        }
      }
      openTab(tab);
    },
    [activeSession, activeKind, openTab],
  );

  /** 关一个标签。关的是激活的就让右邻顶上（没有则左邻，浏览器同款）；
   *  一个不剩就留在空状态（那里本身是"添加面板"的菜单，Codex 同款）。
   *  关浏览器标签同样是表态，此后不自动弹。 */
  const closeTab = useCallback(
    (id: string) => {
      if (id === "browser" && activeSession) {
        browserDismissed.current.add(activeSession.id);
      }
      const idx = wb.tabs.findIndex((t) => tabId(t) === id);
      if (idx < 0) return;
      const tabs = wb.tabs.filter((t) => tabId(t) !== id);
      let active = wb.active;
      if (active === id) {
        const neighbor = tabs[Math.min(idx, tabs.length - 1)];
        active = neighbor ? tabId(neighbor) : null;
      }
      setWb({ tabs, active, open: wb.open });
    },
    [wb, activeSession],
  );

  /** 收起抽屉（标签保留，激活项记着，再展开回到原处）。正看着浏览器
   *  时收起也是"不看了"的表态。 */
  const collapseDrawer = useCallback(() => {
    if (activeKind === "browser" && activeSession) {
      browserDismissed.current.add(activeSession.id);
    }
    setWb((prev) => ({ ...prev, open: false }));
  }, [activeKind, activeSession]);

  /** 展开抽屉。标签组原样回来；一个标签都没有就是空状态（添加面板的菜单）。 */
  const openDrawer = useCallback(() => {
    setWb((prev) => ({ ...prev, open: true }));
  }, []);

  /** 系统文件选择框 → 预览标签。空状态行、"+"菜单和 ⌘P 共用这一条。
   *  openFilePreview 自带分流：可预览的开成标签，图片开大图，其余访达定位。 */
  const pickAndPreview = useCallback(() => {
    void pickFiles().then((paths) => {
      for (const p of paths) openFilePreview(p);
    });
  }, []);

  /** 点了标签栏上的某个浏览器页面：把浏览器带到前台并切到那一页。 */
  const selectBrowserPage = (pageId: number) => {
    activateTab({ kind: "browser" });
    if (activeSession) {
      void browserSelectTab(activeSession.id, pageId)
        .then(applyBrowserPanel)
        .catch(() => {});
    }
  };

  /** 关一个浏览器页面。最后一页关掉 = 收掉整个浏览器标签（和浏览器里
   *  关最后一个标签页等于关窗口一个道理）。宿主的进程和授权都留着，
   *  模型下次用到会现开一页。 */
  const closeBrowserPage = useCallback(
    (pageId: number) => {
      if (!activeSession) return;
      void browserCloseTab(activeSession.id, pageId)
        .then((s) => {
          applyBrowserPanel(s);
          if (s.tabs.length === 0) closeTab("browser");
        })
        .catch(() => {});
    },
    [activeSession, applyBrowserPanel, closeTab],
  );

  // 预览请求来自聊天引用、改动列表、Markdown 链接的模块级入口。
  // 已开过的文件只激活标签不重复开。标签共存，不再有"顶掉谁、回到谁"。
  useEffect(
    () => subscribeFilePreview((p) => openTab({ kind: "preview", path: p })),
    [openTab],
  );

  // 切会话：工作台整组随会话切换（每个会话一份）。把当前组存回 Map、
  // 换上目标会话的组；目标会话没存过就是全收起 —— 新会话不该继承上
  // 一个会话临走前开着的面板。
  const prevSessionId = useRef<string | null>(null);
  useEffect(() => {
    const prev = prevSessionId.current;
    const next = activeSession?.id ?? null;
    if (prev === next) return;
    if (prev) workbenchBySession.current.set(prev, wb);
    prevSessionId.current = next;
    setWb((next ? workbenchBySession.current.get(next) : undefined) ?? EMPTY_WORKBENCH);
    lastPreview.current = null;
    setPreviewWarm(false);
    // wb 是被保存的对象而不是触发条件 —— 只在会话切换那一刻读它当时的值。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeSession?.id]);

  // 欢迎页没有工作台：终端跟着会话走，最后一个会话删掉后收起。右侧
  // 抽屉不用管 —— 它按会话整存整取，没有会话就不渲染，上面的切换
  // effect 也已经把状态清成空。
  useEffect(() => {
    if (activeSession) return;
    setShowTerm(false);
  }, [activeSession]);

  // `[约束]` 链接点击的全局兜底，堵"整窗被导航走"的逃逸口。
  //
  // 预览面板渲染的 markdown / PDF 注释 / PPT 超链接是第三方库自己的
  // <a>，没有组件替它 preventDefault —— webview 的默认行为是把**主窗口
  // 整个导航到目标网址**：Riot 瞬间变成一个无边框浏览器，关掉它等于
  // 关掉应用。挂在 bubble 末端而不是 capture：聊天里的 MdLink 等组件
  // 自己处理过的点击（defaultPrevented）在这里直接放过，不会双开。
  useEffect(() => {
    const onClick = (e: MouseEvent) => {
      if (e.defaultPrevented) return;
      const a = e
        .composedPath()
        .find(
          (el): el is HTMLAnchorElement =>
            el instanceof HTMLAnchorElement && el.hasAttribute("href"),
        );
      if (!a) return;
      const href = a.getAttribute("href") ?? "";
      // 文档内锚点（目录跳转）不产生真实导航，放行。
      if (href.startsWith("#")) return;
      // 其余一律不许在应用 webview 里发生导航。
      e.preventDefault();
      if (/^https?:\/\//i.test(href) || href.startsWith("mailto:")) {
        void openInBrowser(href);
      }
      // 相对路径 / file: 静默拦下：导航过去只会是 404 白屏或裸文件页。
    };
    window.addEventListener("click", onClick);
    return () => window.removeEventListener("click", onClick);
  }, []);

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
      touchSession(info.id);
      setSchedulePage(false);
    } catch (e) {
      // 目录被删是可恢复的：问要不要从列表拿掉或另选，别整页「出错了」。
      if (isMissingProjectError(e)) {
        setMissing((prev) => new Set(prev).add(root));
        setGoneRoot(root);
        return;
      }
      noteError("无法创建会话", e);
    }
  }, [noteError, touchSession]);

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

  /**
   * 会话设置里的「存为提示词」：把当前正文追加进提示词库。
   *
   * 不带标题存 —— 界面会拿正文首行顶上。写提示词的那一刻让人先给它
   * 取个名，是在打断他手头正在做的事；想改名去设置里改。
   */
  const savePromptPreset = useCallback(
    async (body: string) => {
      const cfg = config?.config;
      if (!cfg) return;
      const list = cfg.prompts ?? [];
      setConfig(
        await saveConfig({ ...cfg, prompts: [...list, { id: newPresetId(list), body }] }),
      );
    },
    [config],
  );

  /** 重拉任务清单（创建/删除/到点运行后侧栏要立即对齐）。 */
  const reloadSchedules = useCallback(() => {
    scheduleList()
      .then(setSchedules)
      .catch(() => {});
  }, []);

  // 定时任务：启动时拉清单和"错过了什么"；表变了（模型调工具创建/
  // 删除）就重拉；任务开跑/跑完时还要对齐会话列表 —— 后台新建的会话
  // 要马上出现在侧栏，不能等下一次轮询。
  useEffect(() => {
    reloadSchedules();
    scheduleMissed()
      .then(setMissedSchedules)
      .catch(() => {});
    const offRuns = subscribeScheduleRuns(() => {
      reloadSchedules();
      listSessions()
        .then(setSessions)
        .catch(() => {});
    });
    const offChanges = subscribeScheduleChanges(reloadSchedules);
    return () => {
      offRuns();
      offChanges();
    };
  }, [reloadSchedules]);

  /**
   * 处理掉一条错过记录（补跑或忽略）。全部处理完才告诉宿主清空 ——
   * 宿主只有整体 ack，逐条的账在前端记。
   */
  const settleMissed = useCallback((taskId: string) => {
    setMissedSchedules((prev) => {
      const left = prev.filter((m) => m.taskId !== taskId);
      if (left.length === 0 && prev.length > 0) void scheduleAckMissed().catch(() => {});
      return left;
    });
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

  // 会话刚开跑（含后台定时任务）顶到该项目最前。启动那次 busy 快照
  // 不当作一次新的聊天 —— 否则每次打开 App 列表都会被正在跑的会话打乱。
  useEffect(() => {
    const now = new Set(sessions.filter((s) => s.busy).map((s) => s.id));
    if (!booting) {
      for (const id of now) {
        if (!sawBusy.current.has(id)) touchSession(id);
      }
    }
    sawBusy.current = now;
  }, [sessions, booting, touchSession]);

  // 删掉的会话从顺序表里拿掉，免得 localStorage 越积越大。
  useEffect(() => {
    if (booting) return;
    const alive = new Set(sessions.map((s) => s.id));
    setRecency((prev) => {
      let changed = false;
      const next = { ...prev };
      for (const id of Object.keys(next)) {
        if (!alive.has(id)) {
          delete next[id];
          changed = true;
        }
      }
      if (!changed) return prev;
      localStorage.setItem(LS.recency, JSON.stringify(next));
      return next;
    });
  }, [sessions, booting]);

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
          // 终端组跟着会话走，欢迎页上没有归属可言 —— 不开。
          if (activeSession) setShowTerm((v) => !v);
          return;
        case "t":
          // ⌘T 浏览器（空状态菜单里标着同一个键）。已在前台就收起。
          if (e.shiftKey) return;
          e.preventDefault();
          if (!activeSession) return;
          if (activeKind === "browser") collapseDrawer();
          else activateTab({ kind: "browser" });
          return;
        case "g":
          // ⌘⇧G Git 改动。不带 shift 的 ⌘G 留给"查找下一个"这类惯例。
          if (!e.shiftKey) return;
          e.preventDefault();
          if (!activeSession) return;
          if (activeKind === "changes") collapseDrawer();
          else activateTab({ kind: "changes" });
          return;
        case "p":
          // ⌘P 打开文件进预览。必须拦下 —— webview 的默认行为是打印。
          if (e.shiftKey) return;
          e.preventDefault();
          if (activeSession) pickAndPreview();
          return;
        case "w":
          // 永远拦下，绝不让它冒泡去关窗口。浏览器在前台时关的是当前
          // 页面（Chrome 肌肉记忆），最后一页连带收掉浏览器标签；其余
          // 情况关激活的工作台标签；空状态收抽屉；没抽屉就收终端；
          // 都没有就静默吃掉（绝不关窗）。
          e.preventDefault();
          if (wb.open && wb.active === "browser" && browserPages.tabs.length > 0) {
            closeBrowserPage(browserPages.active);
          } else if (wb.open && wb.active) {
            closeTab(wb.active);
          } else if (wb.open) {
            collapseDrawer();
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
  }, [
    activeSession,
    projects,
    newSession,
    toggleSidebar,
    wb,
    activeKind,
    activateTab,
    collapseDrawer,
    pickAndPreview,
    closeTab,
    browserPages,
    closeBrowserPage,
  ]);

  /* ── 会话 / 项目操作 ──────────────────────── */

  /**
   * 会话没了，它在前端留下的一切跟着走：内存里的对话、滚动位置、
   * 输入框草稿与待发图、终端组、预览标签组。
   *
   * 收在一个函数里而不是在每个删除路径上各写一遍 —— 这些缓存都挂在
   * 模块级 Map 上，漏掉一处不会有任何报错，只是那份数据（含 base64
   * 图片）再也没人回收。删除会话有三条路径，以前每条都得记住三行。
   */
  const dropSessionWorkbench = (id: string) => {
    forgetSession(id);
    transcriptView.delete(id);
    forgetComposerSession(id);
    closeSessionTerminals(id);
    workbenchBySession.current.delete(id);
  };

  const doDeleteSession = async (id: string) => {
    await deleteSession(id);
    dropSessionWorkbench(id);
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
      dropSessionWorkbench(id);
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
        dropSessionWorkbench(id);
      }
      setKept((prev) => prev.filter((id) => !dropped.has(id)));
      setConfig(await getConfig());
      setSessions((prev) => [
        ...prev.filter((s) => s.root !== oldRoot && !dropped.has(s.id)),
        info,
      ]);
      setActive(info.id);
      touchSession(info.id);
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
      anchor: `session:${s.id}`,
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
      anchor: `project:${root}`,
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

  /** 定时任务行的「…」/ 右键菜单。操作全收在这里，行本身只负责跳转。 */
  const scheduleMenu = (e: React.MouseEvent, t: ScheduledTask) => {
    e.preventDefault();
    e.stopPropagation();
    const missed = missedSchedules.find((m) => m.taskId === t.id);
    const entries: MenuState["entries"] = [];
    if (missed) {
      entries.push({
        label: `补跑一次（错过 ${missed.count} 次）`,
        action: () => {
          settleMissed(t.id);
          void scheduleRunNow(t.id).catch((err: unknown) => noteError("补跑没成", err));
        },
      });
      entries.push({ label: "忽略这次错过", action: () => settleMissed(t.id) });
    }
    entries.push({
      label: "立即运行",
      action: () => void scheduleRunNow(t.id).catch((err: unknown) => noteError("没跑起来", err)),
    });
    // 一次性任务跑完就没有"恢复"可言 —— 时刻已经过了，恢复了也不会再跑。
    const spent = t.repeat.kind === "once" && !t.enabled && !t.nextRunMs;
    if (!spent) {
      entries.push({
        label: t.enabled ? "暂停" : "恢复",
        action: () =>
          void scheduleSetEnabled(t.id, !t.enabled)
            .then(reloadSchedules)
            .catch((err: unknown) => noteError(t.enabled ? "暂停失败" : "恢复失败", err)),
      });
    }
    // 和点击行同一个判定：会话还活着才给入口，跳到已删除的会话
    // 只会退回欢迎页，看起来像点坏了。
    if (t.lastSessionId && sessions.some((s) => s.id === t.lastSessionId)) {
      const sid = t.lastSessionId;
      entries.push({
        label: "看上次运行",
        action: () => {
          setActive(sid);
          setSchedulePage(false);
        },
      });
    }
    entries.push({
      label: "删除任务",
      danger: true,
      action: () =>
        setConfirm({
          title: `删除「${t.name}」？`,
          body: "到点就不会再跑了。已经跑过的会话不受影响。",
          confirmLabel: "删除",
          action: () =>
            void scheduleDelete(t.id)
              .then(reloadSchedules)
              .catch((err: unknown) => noteError("删除失败", err)),
        }),
    });
    setMenu({ x: e.clientX, y: e.clientY, anchor: `schedule:${t.id}`, entries });
  };

  /**
   * 把一段开场白送回输入框（创建的主入口是对话）。任务页会退到后台，
   * 让会话接手 —— 没有活跃会话就什么都不做。
   */
  const scheduleCompose = (snippet: string) => {
    if (!activeSession) return;
    setSchedSnippet(encodePlainForComposer(snippet));
    setSchedulePage(false);
  };

  /** 错过补跑：立即跑一次并把这条错过消掉。 */
  const rerunMissed = (m: MissedRun) => {
    settleMissed(m.taskId);
    void scheduleRunNow(m.taskId).catch((err: unknown) => noteError("补跑没成", err));
  };

  /** 错过全部忽略。 */
  const dismissAllMissed = () => {
    setMissedSchedules([]);
    void scheduleAckMissed().catch(() => {});
  };

  /** 工作台标签栏的"+"菜单：往抽屉里添一个标签。已开着的项只是激活。
   *  以后的新面板在这里加一行，再到抽屉里加一个渲染分支。 */
  const workbenchAddMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    setMenu({
      x: e.clientX,
      y: e.clientY,
      entries: [
        {
          label: "浏览器",
          action: () => {
            // 已经开着就再开一页（浏览器"+"的直觉）；还没开就先开起来，
            // 第一页宿主自己建。
            const had = wb.tabs.some((t) => t.kind === "browser");
            activateTab({ kind: "browser" });
            if (had && browserPages.tabs.length > 0 && activeSession) {
              void browserNewTab(activeSession.id)
                .then(applyBrowserPanel)
                .catch(() => {});
            }
          },
        },
        { label: "Git 改动", action: () => activateTab({ kind: "changes" }) },
        { label: "打开文件…", action: pickAndPreview },
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

  /** 抽屉逻辑上开着。窗口右上角归谁看 drawerPresent（含收起动画）。 */
  const drawerVisible = activeSession !== null && wb.open;
  /** 任务详情占用的就是抽屉这个位置：任务页前台 + 选中了任务才有。 */
  const selSchedule = schedulePage
    ? (schedules.find((t) => t.id === selectedSchedule) ?? null)
    : null;
  /** 窗口级分栏开关。钉在 .shell 右上角，不进顶栏 / 抽屉文档流。 */
  const windowControls = (
    <WindowControls
      terminalOpen={showTerm}
      terminalEnabled={activeSession !== null}
      onToggleTerminal={() => setShowTerm((v) => !v)}
      drawerOpen={wb.open}
      drawerEnabled={activeSession !== null}
      onToggleDrawer={() => {
        if (wb.open) collapseDrawer();
        else openDrawer();
      }}
    />
  );

  return (
    <ProjectRootContext.Provider value={activeSession?.root ?? ""}>
    <div className="shell" data-fullscreen={fullscreen ? "" : undefined}>
      <SlidePanel axis="x" open={sidebarOpen} size={sidebarW} keepMounted onVisualOpen={setSidebarVisual}>
          <Sidebar
            width={sidebarW}
            projects={projects}
            missing={missing}
            sessions={sessions}
            active={active}
            renaming={renaming}
            onSelect={(id) => {
              setActive(id);
              setSchedulePage(false);
            }}
            recency={recency}
            onNewSession={newSession}
            onOpenProject={openProject}
            onSettings={() => setShowSettings(true)}
            onSchedules={() => {
              // 任务详情要占用右侧栏 —— 工作台开着就先收起来。
              // 进菜单不记住上次打开的详情，每次都从列表开始。
              collapseDrawer();
              setSelectedSchedule(null);
              setSchedulePage(true);
            }}
            schedulesActive={schedulePage}
            missedSchedules={missedSchedules.length}
            onSessionMenu={sessionMenu}
            onProjectMenu={projectMenu}
            menuAnchor={menu?.anchor ?? null}
            onRenameSubmit={doRename}
            onRenameCancel={() => setRenaming(null)}
            onCollapse={toggleSidebar}
          />
      </SlidePanel>
      {sidebarOpen ? (
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
      ) : null}

      <div className="main">
        {/* 任务页不要这条会话工具栏 —— 标题、设置、面板开关管的都是
            会话。侧栏收起时开关由任务页顶部区承接，开着时在侧栏顶栏。 */}
        {schedulePage ? null : (
          <TopBar
            sidebarOpen={sidebarVisual}
            onToggleSidebar={toggleSidebar}
            session={activeSession}
            onSessionMenu={sessionMenu}
            sessionCfgOpen={showSessionCfg}
            sessionCfgEnabled={activeSession !== null}
            onToggleSessionCfg={() => setShowSessionCfg((v) => !v)}
            onOpenBrowser={() => activateTab({ kind: "browser" })}
            reserveControls={!drawerVisual}
          />
        )}

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
            {schedulePage ? (
              <SchedulesPage
                schedules={schedules}
                missed={missedSchedules}
                selected={selectedSchedule}
                onSelect={setSelectedSchedule}
                sidebarOpen={sidebarVisual}
                onToggleSidebar={toggleSidebar}
                onMenu={scheduleMenu}
                menuAnchor={menu?.anchor ?? null}
                onCreate={() => scheduleCompose("帮我设一个定时任务：")}
                onClearDone={() => {
                  const done = schedules.filter(isDoneSchedule);
                  if (done.length === 0) return;
                  setConfirm({
                    title: `清理 ${done.length} 个已完成的任务？`,
                    body: "一次性任务跑完的记录会从列表里去掉。已经跑过的会话不受影响。",
                    confirmLabel: "清理",
                    action: () =>
                      void Promise.all(done.map((t) => scheduleDelete(t.id)))
                        .then(() => {
                          setSelectedSchedule((id) =>
                            id && done.some((t) => t.id === id) ? null : id,
                          );
                          reloadSchedules();
                        })
                        .catch((err: unknown) => noteError("清理失败", err)),
                  });
                }}
                onSuggest={scheduleCompose}
                onRerunMissed={rerunMissed}
                onDismissMissed={dismissAllMissed}
              />
            ) : null}
            {activeSession ? (
              mountedSessions.map((s) => {
                const visible = !schedulePage && s.id === activeSession.id;
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
                        openTab({ kind: "browser" });
                      }}
                      onAgentPreview={(p) => {
                        // 后台会话不抢前台的预览面板 —— 切回来时不补开，
                        // 和浏览器同一个取舍。
                        if (!visible) return;
                        // 模型可能传相对路径（内核按会话目录解析成功了），
                        // 前端拿到的是原文，同样按会话根目录拼绝对路径。
                        const abs = looksAbsPath(p)
                          ? p
                          : `${s.root.replace(/[\\/]+$/, "")}/${p}`;
                        openFilePreview(abs);
                      }}
                      onTurnEnd={() => setChangesRev((n) => n + 1)}
                      onBusy={(b) => patchSession(s.id, { busy: b })}
                      insertText={visible ? (termSnippet ?? pickSnippet ?? schedSnippet) : null}
                      onInserted={() => {
                        setTermSnippet(null);
                        setPickSnippet(null);
                        setSchedSnippet(null);
                      }}
                    />
                  </div>
                );
              })
            ) : !schedulePage ? (
              <Welcome
                projects={projects}
                missing={missing}
                onNewSession={newSession}
                onOpenProject={openProject}
              />
            ) : null}
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
        {/* 常驻挂载（keepMounted）：收起只是壳收到 0，shell 和回滚缓冲
            都留着。visible 用 termPresent —— 收起动画那一拍里内容还得
            显示着，不然是内容先消失、空壳再收起。 */}
        <SlidePanel axis="y" anchor="end" open={showTerm} size={termH} keepMounted>
          <TerminalPanel
            visible={termPresent}
            height={termH}
            sessionId={activeSession?.id ?? null}
            defaultRoot={activeSession?.root ?? projects[0] ?? null}
            onHide={() => setShowTerm(false)}
            onAgentTerminal={() => setShowTerm(true)}
            onSendSelection={setTermSnippet}
          />
        </SlidePanel>
      </div>

      {/* 抽屉是 main 的兄弟：整列全高（Codex 同款），terminal 只垫在对话
          下面。抽屉和对话共享同一个会话 —— 这正是它存在的意义：你和模型
          看同一个页面。

          里面是多标签工作台：顶部一条统一标签栏，浏览器 / Git 改动 /
          每个预览文件各占一个标签，共存不互斥。整列同一时刻仍只显示
          一个标签的内容 —— 并排铺开会把对话挤成一条缝。宽度所有标签
          共享，拖过一次全都记住。

          任务页前台时这个位置归定时任务详情（进入任务页时工作台已被
          收起，见侧栏的 onSchedules）。 */}
      {selSchedule ? (
        <Resizer
          axis="x"
          onStart={() => {
            dragFrom.current = schedDetailW;
            dragLive.current = schedDetailW;
          }}
          onDelta={(d) => {
            // 详情拖的是左缘：往左（负位移）变宽
            const w = clamp(
              dragFrom.current - d,
              SCHED_DETAIL.min,
              Math.min(SCHED_DETAIL.max, Math.round(window.innerWidth * 0.7)),
            );
            dragLive.current = w;
            setSchedDetailW(w);
          }}
          onEnd={() => savePx(LS.schedDetail, dragLive.current)}
          onReset={() => {
            setSchedDetailW(SCHED_DETAIL.def);
            savePx(LS.schedDetail, SCHED_DETAIL.def);
          }}
        />
      ) : null}
      <SlidePanel axis="x" anchor="end" open={selSchedule !== null} size={schedDetailW} keepMounted>
        {selSchedule ? (
          <ScheduleDetail
            key={selSchedule.id}
            task={selSchedule}
            width={schedDetailW}
            sessions={sessions}
            projects={projects}
            onClose={() => setSelectedSchedule(null)}
            onMenu={scheduleMenu}
            onError={noteError}
            onOpenSession={(id) => {
              setActive(id);
              setSelectedSchedule(null);
              setSchedulePage(false);
            }}
          />
        ) : null}
      </SlidePanel>

      {!selSchedule && drawerVisible && activeSession ? (
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
      ) : null}
      <SlidePanel
        axis="x"
        anchor="end"
        open={rightDrawerOpen}
        size={drawerW}
        keepMounted
        onVisualOpen={setDrawerVisual}
      >
        {drawerPresent && activeSession ? (
          <div className="drawer" style={{ width: drawerW }}>
            {/* 标签栏常在（哪怕一个标签都没有）：它同时是窗口右上角那三个
                分栏开关的座位，抽屉开着的时候不能没有它。 */}
            <WorkbenchTabs
              tabs={wb.tabs}
              active={wb.active}
              pages={browserPages}
              onSelect={(id) => {
                const t = wb.tabs.find((x) => tabId(x) === id);
                if (t) activateTab(t);
              }}
              onClose={closeTab}
              onSelectPage={selectBrowserPage}
              onClosePage={closeBrowserPage}
              onAdd={workbenchAddMenu}
            />
            {/* 空状态即"添加面板"菜单（Codex 同款）。只列侧边标签；
                终端停靠在底部，入口在窗口开关那一组里。 */}
            {wb.tabs.length === 0 ? (
              <WorkbenchEmpty
                onChanges={() => activateTab({ kind: "changes" })}
                onBrowser={() => activateTab({ kind: "browser" })}
                onOpenFile={pickAndPreview}
              />
            ) : null}
            {/* 浏览器和改动只在前台时挂载：浏览器切走要停帧流（宿主
                不再白编码 JPEG），改动列表便宜、回来重比对一次就行。 */}
            {/* `[约束]` 这几块的 key 必须带各自的前缀，不能都用会话 id。
                它们是抽屉里的同层兄弟，React 在同层按 key 配对 —— 撞了 key
                的那两个，切换时旧的漏删、新的照建，每切一次就在面板里多叠
                一份（真出过：从预览切到 Git 改动，改动面板越攒越多）。
                会话 id 留在 key 里是为了跨会话整块重挂，各标签组互不串。 */}
            {activeKind === "browser" ? (
              <>
                <BrowserPanel
                  key={`browser:${activeSession.id}`}
                  sessionId={activeSession.id}
                  panel={browserPages}
                  onPanel={applyBrowserPanel}
                  onPatchTab={patchBrowserTab}
                  onSendToComposer={setPickSnippet}
                />
                <ScopePanel sessionId={activeSession.id} />
              </>
            ) : null}
            {activeKind === "changes" ? (
              <GitChangesPanel
                key={`changes:${activeSession.id}`}
                sessionId={activeSession.id}
                refreshKey={changesRev}
              />
            ) : null}
            {/* 预览面板显示过之后就常驻挂载（切走 display:none）：
                渲染器、滚动位置、表格列宽都留在原地，切回即所见。
                首挂必须在激活时（activeKind 那个分支先成立，previewWarm
                随后才置位）—— 渲染器不能在 display:none 里做首屏适配。
                跨会话仍整个重挂 —— 标签组每会话独立。 */}
            {(activeKind === "preview" || previewWarm) && activePreviewPath ? (
              <FilePreviewPanel
                key={`preview:${activeSession.id}`}
                paths={previewPaths}
                active={activePreviewPath}
                visible={activeKind === "preview"}
              />
            ) : null}
          </div>
        ) : null}
      </SlidePanel>

      {/* 钉在窗口右上角。任务页没有这条会话工具栏，开关也不露。 */}
      {schedulePage ? null : windowControls}

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
          navWidth={sidebarW}
        />
      ) : null}

      {activeSession && showSessionCfg ? (
        // key 让切换会话时弹窗重挂载，草稿不会串到另一个会话头上。
        <SessionSettings
          key={activeSession.id}
          session={activeSession}
          // 会话继承到的是"模型叠在服务方之上"的结果，和宿主 resolve() 同序。
          // 只传服务方的话，模型上单独设过的字段在这里会显示成另一个数。
          inherited={inheritedSampling(config.config)}
          presets={config.config.prompts ?? []}
          onSavePreset={savePromptPreset}
          onPatch={(patch) => patchSession(activeSession.id, patch)}
          onClose={() => setShowSessionCfg(false)}
        />
      ) : null}

      {menu ? <ContextMenu menu={menu} onClose={() => setMenu(null)} /> : null}
      <ImageLightboxHost />
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

/** 长任务在后台跑完时发系统通知。权限与失败处理见 bridge 的 `notify`。 */
function notifyTurnDone() {
  void notify("Riot", "任务完成了，回来看看结果吧。");
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
  onAgentPreview,
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
  /** 模型的 PreviewFile 工具成功后，把文件在预览面板展示给用户。
   *  路径是模型传的原文，可能是相对路径 —— 由外层按会话根目录解析。 */
  onAgentPreview?: (path: string) => void;
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
    onAgentBrowser || onAgentPreview
      ? {
          ...(onAgentBrowser ? { onBrowserOpen: onAgentBrowser } : {}),
          ...(onAgentPreview ? { onPreviewFile: onAgentPreview } : {}),
        }
      : undefined,
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
          onEditEntry={session.editEntry}
          onDeleteEntry={session.deleteEntry}
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
