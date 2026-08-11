import { memo, useCallback, useEffect, useRef, useState } from "react";

import {
  addProject,
  type ConfigStatus,
  createSession,
  deleteSession,
  getConfig,
  hasActiveKey,
  listSessions,
  type PermissionMode,
  pickDirectory,
  type ProviderConfig,
  removeProject,
  renameSession,
  revealInFinder,
  type Sampling,
  setConfig as saveConfig,
  type SessionInfo,
  setPermissionMode,
  setSessionSampling,
  setWindowTitle,
} from "./bridge";
import { BrowserPanel } from "./components/BrowserPanel";
import { ConfirmDialog, type ConfirmRequest } from "./components/ConfirmDialog";
import { Markdown } from "./components/Markdown";
import { PermissionDialog } from "./components/PermissionDialog";
import { Settings } from "./components/Settings";
import { ToolCard } from "./components/ToolCard";
import { type Item, useSession } from "./hooks/useSession";

/**
 * 布局照着 Codex 桌面端：左侧按项目分组的会话列表，右侧对话流。
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
        { label: "重命名", action: () => setRenaming(s.id) },
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
    <div className="shell">
      <Sidebar
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

      <div className="main">
        {activeSession ? (
          // 浏览器面板和对话左右分栏，共享同一个会话。
          // 这正是它存在的意义 —— 你和模型看同一个页面。
          <div className={showBrowser ? "split" : undefined}>
            <Chat
              key={activeSession.id}
              sessionId={activeSession.id}
              config={config}
              workspace={activeSession.root}
              initialSampling={activeSession.sampling}
              initialMode={activeSession.mode}
              onConfig={setConfig}
              onOpenSettings={() => setShowSettings(true)}
              onFirstMessage={onFirstMessage}
              onToggleBrowser={() => setShowBrowser((v) => !v)}
              browserOpen={showBrowser}
            />
            {showBrowser ? (
              <BrowserPanel
                key={activeSession.id}
                sessionId={activeSession.id}
                onClose={() => setShowBrowser(false)}
              />
            ) : null}
          </div>
        ) : (
          <Welcome
            projects={projects}
            onNewSession={newSession}
            onOpenProject={openProject}
          />
        )}
      </div>

      {showSettings ? (
        <Settings
          status={config}
          onStatus={setConfig}
          onClose={() => setShowSettings(false)}
        />
      ) : null}

      {menu ? <ContextMenu menu={menu} onClose={() => setMenu(null)} /> : null}
      {confirm ? <ConfirmDialog c={confirm} onClose={() => setConfirm(null)} /> : null}
    </div>
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
  const { projects, sessions, onOpenProject, onSettings } = props;

  // 有会话但不在项目列表里的根也要显示（理论上不会发生，但真发生时
  // 隐藏会话比多显示一个组糟得多）。
  const roots = [...projects];
  for (const s of sessions) {
    if (!roots.includes(s.root)) roots.push(s.root);
  }

  return (
    <aside className="sidebar">
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
  initialSampling,
  initialMode,
  onConfig,
  onOpenSettings,
  onFirstMessage,
  onToggleBrowser,
  browserOpen,
}: {
  sessionId: string;
  config: ConfigStatus;
  workspace: string;
  initialSampling: Sampling;
  initialMode: PermissionMode;
  onConfig: (s: ConfigStatus) => void;
  onOpenSettings: () => void;
  onFirstMessage: (sessionId: string, text: string) => void;
  onToggleBrowser: () => void;
  browserOpen: boolean;
}) {
  const session = useSession(sessionId);
  const empty =
    session.items.length === 0 && !session.streaming && !session.thinking;

  const send = (text: string) => {
    onFirstMessage(sessionId, text);
    session.send(text);
  };

  const composer = (
    <Composer
      sessionId={sessionId}
      busy={session.busy}
      config={config}
      initialSampling={initialSampling}
      onConfig={onConfig}
      initialMode={initialMode}
      tokens={session.tokens}
      onSend={send}
      onStop={session.stop}
      onOpenSettings={onOpenSettings}
      onToggleBrowser={onToggleBrowser}
      browserOpen={browserOpen}
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
          <p className="hero-hint">
            它能读写这个目录里的文件、跑命令、搜代码。目录外的路径会被拒绝。
          </p>
        </div>
      ) : (
        <>
          <Transcript
            items={session.items}
            streaming={session.streaming}
            thinking={session.thinking}
            busy={session.busy}
          />
          <div className="composer-dock">{composer}</div>
        </>
      )}

      {session.asks[0] ? (
        // key 让每个请求拿到全新的弹窗实例：并发的两个请求先后弹出时，
        // 第一个里勾的"总是允许"不会残留到第二个上。
        <PermissionDialog
          key={session.asks[0].requestId}
          ask={session.asks[0].detail}
          pendingCount={session.asks.length}
          onAnswer={session.answer}
        />
      ) : null}
    </div>
  );
}

function Transcript({
  items,
  streaming,
  thinking,
  busy,
}: {
  items: Item[];
  streaming: string;
  thinking: string;
  busy: boolean;
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
  }, [items, streaming, thinking]);

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
        {busy && !streaming && !thinking ? <Dots /> : null}

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
      return <div className="msg user">{item.text}</div>;
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
  }
});

/**
 * 思考过程默认折叠成一行。它是过程不是结论，占满屏幕会把真正的
 * 回答挤出视野 —— 但必须可看，"模型为什么这么做"的答案在里面。
 */
function ThinkingBlock({ text, live }: { text: string; live?: boolean }) {
  const [open, setOpen] = useState(false);

  return (
    <div className={live ? "think-block live" : "think-block"}>
      <button type="button" className="think-head" onClick={() => setOpen(!open)}>
        <span className="think-icon">{open ? "▾" : "▸"}</span>
        {live ? "思考中…" : "思考过程"}
        <span className="think-chars">{text.length} 字</span>
      </button>
      {open ? <div className="think-body">{text}</div> : null}
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

function Dots() {
  return (
    <div className="dots">
      <span />
      <span />
      <span />
    </div>
  );
}

/* ── 输入框 ─────────────────────────────────── */

const MODE_LABEL: Record<string, string> = {
  default: "每次询问",
  acceptEdits: "自动接受编辑",
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
const drafts = new Map<string, string>();

/** 会话级采样覆盖的 UI 缓存。真值在宿主的 Session 里，这里只是
 *  让"切走再切回"不丢显示（listSessions 只在启动时拉一次）。 */
const sampCache = new Map<string, Sampling>();

/**
 * 权限模式的 UI 缓存，理由同上，但它错了会出安全问题而不只是显示问题。
 *
 * Composer 在同一个会话里就会重挂载一次：发出第一条消息后 `empty` 翻转，
 * 它从 hero 区挪到 composer-dock，React 视作两个不同位置的组件，本地
 * state 全部丢弃。少了这层缓存，模式就退回全局默认值显示，而宿主那边
 * 还是用户选的那个 —— 屏幕上写着「每次询问」，实际每一步都在静默放行。
 */
const modeCache = new Map<string, PermissionMode>();

function Composer({
  sessionId,
  busy,
  config,
  initialSampling,
  onConfig,
  initialMode,
  tokens,
  onSend,
  onStop,
  onOpenSettings,
  onToggleBrowser,
  browserOpen,
}: {
  sessionId: string;
  busy: boolean;
  config: ConfigStatus;
  initialSampling: Sampling;
  onConfig: (s: ConfigStatus) => void;
  /** 宿主侧这个会话的当前模式，不是全局默认值。 */
  initialMode: PermissionMode;
  tokens: { input: number; output: number };
  onSend: (t: string) => void;
  onStop: () => void;
  onOpenSettings: () => void;
  onToggleBrowser: () => void;
  browserOpen: boolean;
}) {
  const [draft, setDraftRaw] = useState(() => drafts.get(sessionId) ?? "");
  const [mode, setMode] = useState<PermissionMode>(
    () => modeCache.get(sessionId) ?? initialMode,
  );
  const [sampling, setSamplingRaw] = useState<Sampling>(
    () => sampCache.get(sessionId) ?? initialSampling,
  );
  const [modeConfirm, setModeConfirm] = useState<ConfirmRequest | null>(null);
  const ref = useRef<HTMLTextAreaElement>(null);
  // 中文 IME：确认候选/上屏英文时，keydown(Enter) 常在 compositionend 之后到达，
  // 此时 nativeEvent.isComposing 已是 false，会被误当成发送。用 ref 盖住这一拍。
  const imeRef = useRef(false);

  const cfg = config.config;
  const hasKey = hasActiveKey(config);
  const activeProvider =
    cfg.providers.find((p) => p.id === cfg.activeProvider) ?? cfg.providers[0] ?? null;

  const changeSampling = (s: Sampling) => {
    setSamplingRaw(s);
    sampCache.set(sessionId, s);
    // 乐观更新；覆盖存宿主的 Session，下一轮生效
    setSessionSampling(sessionId, s).catch(() => {});
  };

  // 内联切换：直接改激活的 provider/model 并回写配置。和设置页共用
  // 同一条 setConfig 通道，宿主 resolve 一次挡住坏状态。切 provider 时
  // 若当前模型不属于新家，跳到新家的第一个模型。
  const switchProvider = (p: ProviderConfig) => {
    if (p.id === cfg.activeProvider) return;
    const model = p.models.includes(cfg.activeModel) ? cfg.activeModel : (p.models[0] ?? "");
    void saveConfig({ ...cfg, activeProvider: p.id, activeModel: model })
      .then(onConfig)
      .catch(() => {});
  };
  const switchModel = (m: string) => {
    if (m === cfg.activeModel) return;
    void saveConfig({ ...cfg, activeModel: m }).then(onConfig).catch(() => {});
  };

  const setDraft = (v: string) => {
    setDraftRaw(v);
    if (v) drafts.set(sessionId, v);
    else drafts.delete(sessionId);
  };

  const submit = () => {
    const text = draft.trim();
    if (!text || busy || !hasKey || !cfg.activeModel) return;
    setDraft("");
    onSend(text);
  };

  // 跟着内容长高，到 40% 屏幕封顶。固定三行的话，贴一段代码进去就只能
  // 看到最后三行。
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, window.innerHeight * 0.4)}px`;
  }, [draft]);

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

      <form
        className="composer"
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        <textarea
          ref={ref}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onCompositionStart={() => {
            imeRef.current = true;
          }}
          onCompositionEnd={() => {
            // compositionend 与确认用的 Enter 可能跨到下一个宏任务，
            // microtask 不够，用 setTimeout(0) 盖住这一拍。
            setTimeout(() => {
              imeRef.current = false;
            }, 0);
          }}
          onKeyDown={(e) => {
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
          placeholder={busy ? "它正在做事…" : "描述一个任务，或问点什么"}
          rows={1}
          autoFocus
        />

        <div className="composer-bar">
          <ModeMenu mode={mode} onChange={changeMode} />
          <Picker
            title="切换服务方"
            label={activeProvider?.name ?? "选择服务方"}
            items={cfg.providers.map((p) => ({
              id: p.id,
              label: p.name,
              active: p.id === cfg.activeProvider,
              ...(config.keyStatus[p.id] ? {} : { note: "未配置 key" }),
            }))}
            onPick={(id) => {
              const p = cfg.providers.find((x) => x.id === id);
              if (p) switchProvider(p);
            }}
          />
          <Picker
            title="切换模型"
            label={cfg.activeModel || "选择模型"}
            items={(activeProvider?.models ?? []).map((m) => ({
              id: m,
              label: m,
              active: m === cfg.activeModel,
            }))}
            emptyHint="这个服务方还没有模型"
            onEmpty={onOpenSettings}
            onPick={switchModel}
          />
          <SamplingMenu
            value={sampling}
            inherited={activeProvider?.sampling ?? {}}
            onChange={changeSampling}
          />
          <button
            type="button"
            className={browserOpen ? "pill active" : "pill"}
            onClick={onToggleBrowser}
            title="内置浏览器"
          >
            浏览器
          </button>
          <span className="bar-spacer" />
          {tokens.input + tokens.output > 0 ? (
            <span className="usage" title="本会话累计 token（输入 / 输出）">
              {fmtTokens(tokens.input)} / {fmtTokens(tokens.output)}
            </span>
          ) : null}
          {busy ? (
            <button type="button" className="send stop" onClick={onStop} title="停止">
              <StopIcon />
            </button>
          ) : (
            <button
              type="submit"
              className="send"
              disabled={!draft.trim() || !hasKey || !cfg.activeModel}
              title={cfg.activeModel ? "发送" : "先选择一个模型"}
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
  /** 次要说明，如"未配置 key"，灰在右边。 */
  note?: string;
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
              className={it.active ? "menu-item active" : "menu-item"}
              onClick={() => {
                onPick(it.id);
                setOpen(false);
              }}
            >
              <span className="pick-label">{it.label}</span>
              {it.note ? <span className="menu-warn">{it.note}</span> : null}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

/**
 * 会话级采样参数覆盖。占位符显示 provider 的默认（继承值），
 * 输入即覆盖该字段；清空恢复继承。真值存宿主的 Session，下一轮生效。
 */
const SAMP_FIELDS: { key: keyof Sampling; label: string; step: string; integer?: boolean }[] = [
  { key: "temperature", label: "temperature", step: "0.1" },
  { key: "topP", label: "top_p", step: "0.05" },
  { key: "topK", label: "top_k", step: "1", integer: true },
  { key: "maxOutputTokens", label: "max tokens", step: "256", integer: true },
];

function SamplingMenu({
  value,
  inherited,
  onChange,
}: {
  value: Sampling;
  inherited: Sampling;
  onChange: (s: Sampling) => void;
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

  const overrides = SAMP_FIELDS.filter((f) => value[f.key] != null).length;

  const setField = (key: keyof Sampling, raw: string, integer?: boolean) => {
    const t = raw.trim();
    let v: number | null = null;
    if (t) {
      const n = Number(t);
      if (Number.isFinite(n)) v = integer ? Math.round(n) : n;
    }
    onChange({ ...value, [key]: v });
  };

  return (
    <div className="mode-menu" ref={rootRef}>
      <button
        type="button"
        className={overrides ? "pill samp-active" : "pill"}
        title="采样参数（本会话覆盖）"
        onClick={() => setOpen(!open)}
      >
        <SlidersIcon />
        {overrides ? <span className="samp-count">{overrides}</span> : null}
      </button>
      {open ? (
        <div className="menu samp-menu">
          <div className="samp-head">本会话参数 <span className="samp-sub">留空继承 Provider</span></div>
          {SAMP_FIELDS.map((f) => (
            <label key={f.key} className="samp-row">
              <span className="samp-label">{f.label}</span>
              <input
                type="number"
                step={f.step}
                defaultValue={value[f.key] ?? ""}
                placeholder={inherited[f.key]?.toString() ?? "默认"}
                onBlur={(e) => setField(f.key, e.target.value, f.integer)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                }}
                spellCheck={false}
              />
            </label>
          ))}
          {overrides ? (
            <button
              type="button"
              className="samp-reset"
              onClick={() => {
                onChange({});
                setOpen(false);
              }}
            >
              全部恢复继承
            </button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

/* ── 图标 ───────────────────────────────────── */

function SlidersIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M2 4.5h7M12.5 4.5H14M2 11.5h1.5M7 11.5h7"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
      />
      <circle cx="10.75" cy="4.5" r="1.75" stroke="currentColor" strokeWidth="1.4" />
      <circle cx="5.25" cy="11.5" r="1.75" stroke="currentColor" strokeWidth="1.4" />
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
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <circle cx="8" cy="8" r="2.2" stroke="currentColor" strokeWidth="1.2" />
      <path
        d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M12.6 3.4l-1.4 1.4M4.8 11.2l-1.4 1.4"
        stroke="currentColor"
        strokeWidth="1.2"
        strokeLinecap="round"
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

function DotsIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" aria-hidden>
      <circle cx="3.5" cy="8" r="1.3" />
      <circle cx="8" cy="8" r="1.3" />
      <circle cx="12.5" cy="8" r="1.3" />
    </svg>
  );
}
