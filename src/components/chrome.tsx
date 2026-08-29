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
  GearIcon,
  PanelBottomIcon,
  PanelRightIcon,
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
 * （点开就是会话菜单），右边是会话设置。整条都是窗口拖拽区。
 *
 * 侧栏开关留在左边 —— 它就贴着自己管的那一栏，指哪开哪。终端和侧边
 * 面板的开关不在这里：那两块在窗口的下边和右边，归 [`WindowControls`]。
 */
export function TopBar({
  sidebarOpen,
  onToggleSidebar,
  session,
  onSessionMenu,
  sessionCfgOpen,
  sessionCfgEnabled,
  onToggleSessionCfg,
  onOpenBrowser,
  controls,
}: {
  sidebarOpen: boolean;
  onToggleSidebar: () => void;
  session: SessionInfo | null;
  onSessionMenu: (e: React.MouseEvent, s: SessionInfo) => void;
  sessionCfgOpen: boolean;
  /** 会话设置管的是单个会话的参数，没有会话时置灰。 */
  sessionCfgEnabled: boolean;
  onToggleSessionCfg: () => void;
  /** 渗透授权角标要能直达浏览器标签 —— 撤销入口（ScopePanel）在那里。 */
  onOpenBrowser: () => void;
  /** 窗口级分栏开关。抽屉收起时这条栏的右端就是窗口右上角，由它承载；
   *  抽屉开着时交给抽屉的标签栏（见 WindowControls 的说明）。 */
  controls?: React.ReactNode;
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
        title={sidebarOpen ? "收起侧边栏（⌘B）" : "展开侧边栏（⌘B）"}
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

      {/* 渗透授权常驻角标：授权列表原本只在浏览器标签里可见，标签一关，
          "允许对哪些站做侵入性操作"就从界面上彻底消失 —— 授权还生效着，
          它的可见性不能跟着面板走。 */}
      {session ? <ScopeBadge sessionId={session.id} onOpen={onOpenBrowser} /> : null}

      <button
        className={sessionCfgOpen ? "tb-btn active" : "tb-btn"}
        onClick={onToggleSessionCfg}
        disabled={!sessionCfgEnabled}
        title={sessionCfgEnabled ? "会话设置" : "先打开一个会话"}
        aria-label="会话设置"
      >
        <GearIcon />
      </button>
      {controls}
    </header>
  );
}

/**
 * 窗口级的分栏开关：底部终端、右侧面板。
 *
 * `[约束]` 它们停的是**窗口**的右上角，不是某一栏的右上角。这两个键改的
 * 是整个窗口怎么分块，跟着对话列走的话，抽屉一开它们就落到窗口中间去了
 * （图标指着"最右边那一栏"，人却在中间点它）。所以抽屉收起时由顶栏渲染，
 * 抽屉开着时由抽屉的标签栏渲染 —— 那时窗口的右上角属于抽屉。
 *
 * 侧栏开关不在这里：它在顶栏左端，紧贴自己管的那一栏。会话设置也不在，
 * 它管的是单个会话的参数，留在消息栏。
 */
export function WindowControls({
  terminalOpen,
  terminalEnabled,
  onToggleTerminal,
  drawerOpen,
  drawerEnabled,
  onToggleDrawer,
}: {
  terminalOpen: boolean;
  /** 终端组跟着会话走（每个会话一份），没有会话时置灰。 */
  terminalEnabled: boolean;
  onToggleTerminal: () => void;
  /** 右侧工作台抽屉。里面装什么由抽屉自己的标签栏 / 空状态管，
   *  这里只有总开关（Codex 同款）。 */
  drawerOpen: boolean;
  drawerEnabled: boolean;
  onToggleDrawer: () => void;
}) {
  return (
    // 按钮之间的缝也归窗口拖拽 —— 顶栏其余空白处都是这么用的，
    // 到了这一组突然拖不动会显得这块是"别的东西"。
    <div className="win-controls" data-tauri-drag-region>
      <button
        className={terminalOpen ? "tb-btn active" : "tb-btn"}
        onClick={onToggleTerminal}
        disabled={!terminalEnabled}
        title={terminalEnabled ? "终端面板（⌘J）" : "先打开一个会话再用终端"}
        aria-label="终端面板"
      >
        <PanelBottomIcon />
      </button>
      <button
        className={drawerOpen ? "tb-btn active" : "tb-btn"}
        onClick={onToggleDrawer}
        disabled={!drawerEnabled}
        title={drawerEnabled ? "侧边面板" : "先打开一个会话"}
        aria-label="侧边面板"
      >
        <PanelRightIcon />
      </button>
    </div>
  );
}

/**
 * 顶栏的渗透授权角标。有生效的 scope 授权时亮起（盾牌 + 数量），
 * 点击激活浏览器标签 —— 撤销入口（ScopePanel）在那里。
 * 没有授权时整个不渲染，颗粒无声。
 */
function ScopeBadge({
  sessionId,
  onOpen,
}: {
  sessionId: string;
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
      onClick={onOpen}
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

