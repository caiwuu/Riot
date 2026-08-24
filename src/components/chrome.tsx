/**
 * 窗口级的 chrome 件：顶部工具栏、拖拽分隔线、全局右键菜单。
 * 从 App.tsx 拆出 —— 布局状态仍在 App，这里只有展示与交互。
 */

import { useEffect, useRef, useState } from "react";

import { browserScopeList, type SessionInfo } from "../bridge";
import { Chevron } from "./Chevron";
import { useEscLayer } from "./Modal";

/** Overlay 标题栏的红绿灯只在 macOS 占左上角。Windows / Linux 的窗口
 * 控件在右侧，左边不用让位。 */
export const IS_MAC = navigator.userAgent.includes("Mac");
import {
  BrowserIcon,
  DiffIcon,
  GearIcon,
  PanelBottomIcon,
  SidebarToggleIcon,
} from "./icons";

/** 右键 / "…" 菜单。全局单实例，点哪开哪 —— 免得每行都挂一份状态。 */
export interface MenuState {
  x: number;
  y: number;
  entries: { label: string; danger?: boolean; action: () => void }[];
}

/* ── 顶部工具栏 ─────────────────────────────── */

/**
 * 主区顶部的工具栏（照 Codex）：左边收放侧栏，中间是当前会话的标题
 * （点开就是会话菜单），右边是面板开关和会话设置。整条都是窗口拖拽区。
 */
export function TopBar({
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
export function Resizer({
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
export function ContextMenu({ menu, onClose }: { menu: MenuState; onClose: () => void }) {
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

