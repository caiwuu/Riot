import { useEffect, useRef, useState } from "react";

import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";

import { type TermEvent, termClose, termOpen, termResize, termWrite } from "../bridge";

/**
 * 底部终端面板。布局照 Codex：标签栏一行（目录名做标题），下面是终端。
 *
 * 画面是 xterm.js 画的，shell 是宿主里的真 PTY —— 输出按字节流推过来，
 * 键盘原样打回去。
 *
 * `[约束]` 这个组件**常驻挂载**，收起面板只是 display:none。卸载会杀掉
 * xterm 实例，而回滚缓冲和正在跑的进程状态就存在实例里 —— 用户收起面板
 * 再打开，dev server 的日志不该消失。shell 进程本身在宿主里，跟组件
 * 生死无关，但"看到的历史"在这边。
 */
export function TerminalPanel({
  visible,
  height,
  defaultRoot,
  onHide,
}: {
  visible: boolean;
  height: number;
  /** 新标签在哪个目录开 shell。null = 家目录。 */
  defaultRoot: string | null;
  /** 用户收面板、或最后一个标签关闭。shell 不一定死，见组件注释。 */
  onHide: () => void;
}) {
  const [state, setState] = useState<{ tabs: Tab[]; active: string | null }>({
    tabs: [],
    active: null,
  });
  const instances = useRef(new Map<string, Inst>());
  /** 关标签的收尾。exit 事件（shell 自己退出）也走这条路。 */
  const closeTab = (uid: string, opts?: { hostDead?: boolean }) => {
    const inst = instances.current.get(uid);
    if (inst) {
      inst.ro.disconnect();
      inst.term.dispose();
      // shell 自己退出（exit/崩溃）时宿主已经收过尸，再发 close 只是
      // 对一个不存在的 id 的无操作 —— 但没必要发。
      if (!opts?.hostDead && inst.hostId != null) void termClose(inst.hostId);
      instances.current.delete(uid);
    }
    setState((prev) => {
      const i = prev.tabs.findIndex((t) => t.uid === uid);
      if (i < 0) return prev;
      const tabs = prev.tabs.filter((t) => t.uid !== uid);
      if (tabs.length === 0) {
        // 最后一个标签关掉 = 收起面板，和浏览器面板关最后一页同一个逻辑
        onHide();
        return { tabs, active: null };
      }
      const active =
        prev.active === uid ? (tabs[Math.min(i, tabs.length - 1)]?.uid ?? null) : prev.active;
      return { tabs, active };
    });
  };
  // closeTab 被 termOpen 的回调闭包长期持有，而它内部引用了 onHide ——
  // 用 ref 兜住最新值，免得回调里捕获的是旧 props。
  const closeRef = useRef(closeTab);
  closeRef.current = closeTab;

  const addTab = (root: string | null) => {
    const tab = mkTab(root);
    setState((prev) => ({ tabs: [...prev.tabs, tab], active: tab.uid }));
  };

  // 面板打开且一个标签都没有 → 自动开一个。第一次点开就该能用，
  // 而不是先看到一个空面板再去找"+"。
  //
  // 守卫写在函数式更新里：StrictMode 会把 effect 连跑两遍，第二遍的
  // updater 看到的是第一遍之后的状态（tabs 已经有了）—— 直接在 effect
  // 体里调 addTab 的写法会开出两个终端。
  useEffect(() => {
    if (!visible) return;
    const tab = mkTab(defaultRoot);
    setState((prev) =>
      prev.tabs.length > 0 ? prev : { tabs: [tab], active: tab.uid },
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible, state.tabs.length]);

  // 显示/切标签/改高度之后重新量尺寸。display:none 期间 xterm 量不到
  // 自己，切回来那一拍必须补一次 fit，否则列数还是上次的。
  useEffect(() => {
    if (!visible || !state.active) return;
    const inst = instances.current.get(state.active);
    if (!inst) return;
    const raf = requestAnimationFrame(() => safeFit(inst));
    return () => cancelAnimationFrame(raf);
  }, [visible, state.active, height]);

  // 聚焦只跟"打开面板/切标签"走，不跟高度走 —— 用户在输入框打字时
  // 拖终端分隔线，焦点不该被抢过来。
  useEffect(() => {
    if (!visible || !state.active) return;
    instances.current.get(state.active)?.term.focus();
  }, [visible, state.active]);

  /**
   * 给一个标签建 xterm + PTY。ref 回调每次渲染都会来一遍（内联箭头函数），
   * 靠 instances 去重 —— StrictMode 的双挂载也被同一个守卫挡住。
   */
  const mount = (tab: Tab, el: HTMLDivElement) => {
    if (instances.current.has(tab.uid)) return;

    const term = new Terminal({
      fontFamily: 'ui-monospace, "SF Mono", "JetBrains Mono", Menlo, monospace',
      fontSize: 12,
      scrollback: 5000,
      theme: THEME,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(el);

    const inst: Inst = {
      term,
      fit,
      hostId: null,
      pending: [],
      ro: new ResizeObserver(() => safeFit(inst)),
    };
    instances.current.set(tab.uid, inst);
    safeFit(inst);
    inst.ro.observe(el);

    // PTY 还没落地时敲的字先攒着。丢掉的话，面板刚开就打字的人会看到
    // 开头缺了几个字符 —— 而那正是最常见的使用方式。
    term.onData((d) => {
      if (inst.hostId == null) inst.pending.push(d);
      else void termWrite(inst.hostId, d).catch(() => {});
    });
    term.onResize(({ cols, rows }) => {
      if (inst.hostId != null) void termResize(inst.hostId, cols, rows).catch(() => {});
    });

    termOpen(tab.root, term.cols, term.rows, (ev: TermEvent) => {
      if (ev.kind === "data") term.write(b64ToBytes(ev.data));
      else closeRef.current(tab.uid, { hostDead: true });
    })
      .then((id) => {
        // 等待期间用户把标签关了：实例已不在，宿主那个 shell 没人认领，杀掉
        if (!instances.current.has(tab.uid)) {
          void termClose(id);
          return;
        }
        inst.hostId = id;
        for (const d of inst.pending) void termWrite(id, d).catch(() => {});
        inst.pending = [];
        // 等待期间尺寸可能变了，按现在的实际列数对齐一次
        void termResize(id, term.cols, term.rows).catch(() => {});
      })
      .catch((e: unknown) => {
        term.write(`\r\n\x1b[31m终端启动失败：${String(e)}\x1b[0m\r\n`);
      });
  };

  return (
    <div
      className="term-panel"
      style={{ height, display: visible ? undefined : "none" }}
    >
      <div className="term-tabs">
        {state.tabs.map((t) => (
          <button
            key={t.uid}
            className={t.uid === state.active ? "term-tab active" : "term-tab"}
            onClick={() => setState((prev) => ({ ...prev, active: t.uid }))}
            title={t.root ?? "~"}
          >
            <TermIcon />
            <span className="term-tab-title">{t.title}</span>
            {/* span 而不是嵌套 button —— button 套 button 是非法 HTML，
                浏览器会把内层拆出去，点击行为不可预料。 */}
            <span
              className="term-tab-close"
              role="button"
              aria-label="关闭终端"
              onClick={(e) => {
                e.stopPropagation();
                closeTab(t.uid);
              }}
            >
              <CloseIcon />
            </span>
          </button>
        ))}
        <button className="icon" onClick={() => addTab(defaultRoot)} title="新终端">
          <PlusIcon />
        </button>
        <span className="term-tabs-spacer" />
        {/* 收起 ≠ 关闭：shell 继续活着，再点开还是原样 */}
        <button className="icon" onClick={onHide} title="收起终端面板">
          <ChevronDownIcon />
        </button>
      </div>

      <div className="term-body">
        {state.tabs.map((t) => (
          <div
            key={t.uid}
            className="term-slot"
            style={{ display: t.uid === state.active ? undefined : "none" }}
            ref={(el) => {
              if (el) mount(t, el);
            }}
          />
        ))}
      </div>
    </div>
  );
}

interface Tab {
  uid: string;
  title: string;
  root: string | null;
}

function mkTab(root: string | null): Tab {
  return {
    uid: `t-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
    title: root?.split("/").pop() || "终端",
    root,
  };
}

interface Inst {
  term: Terminal;
  fit: FitAddon;
  /** 宿主侧的终端 id。termOpen 落地前是 null。 */
  hostId: number | null;
  /** PTY 落地前攒下的键盘输入。 */
  pending: string[];
  ro: ResizeObserver;
}

/**
 * 量得出尺寸才 fit。面板 display:none 时容器是 0×0，这时候 fit 会把
 * 终端缩成 1 列 —— 切回来之前的所有输出都按 1 列折行，historial 全花。
 */
function safeFit(inst: Inst) {
  const dims = inst.fit.proposeDimensions();
  if (dims && dims.cols > 2 && dims.rows > 1) inst.fit.fit();
}

/** base64 → 字节。xterm 自带跨 chunk 的 UTF-8 解码，给它字节即可。 */
function b64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/**
 * 配色对齐 styles.css 的 :root。xterm 读不了 CSS 变量，只能抄一份。
 *
 * `scrollbarSlider*` 必须在这里给：xterm 6 的滚动条是自绘的 div，颜色由它
 * 自己注入一段 `<style>`，样式表里写什么都抢不过。默认值是前景色 20%
 * 透明度 —— 深底上是一条很亮的灰。
 */
const THEME = {
  background: "#141416",
  foreground: "#ececf1",
  cursor: "#ececf1",
  cursorAccent: "#141416",
  selectionBackground: "#3d3d45",
  black: "#1b1b1e",
  red: "#f87171",
  green: "#4ade80",
  yellow: "#fbbf24",
  blue: "#7cb3ff",
  magenta: "#c792ea",
  cyan: "#7fdbca",
  white: "#ececf1",
  brightBlack: "#6e6e78",
  brightRed: "#fca5a5",
  brightGreen: "#86efac",
  brightYellow: "#fde68a",
  brightBlue: "#a5c8ff",
  brightMagenta: "#ddb6f2",
  brightCyan: "#a2e8dd",
  brightWhite: "#ffffff",
  // 半透明而不是实色：滑块压在输出上，底下的字还得看得见
  scrollbarSliderBackground: "#ffffff1a",
  scrollbarSliderHoverBackground: "#ffffff2e",
  scrollbarSliderActiveBackground: "#ffffff42",
};

/* ── 图标 ───────────────────────────────────── */

function TermIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M3 4.5l3.5 3.5L3 11.5M8.5 12H13"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function PlusIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path d="M8 3v10M3 8h10" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
    </svg>
  );
}

function CloseIcon() {
  return (
    <svg width="11" height="11" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M4 4l8 8M12 4l-8 8"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
      />
    </svg>
  );
}

/** 收起面板（往下收）。 */
function ChevronDownIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M3.5 6l4.5 4.5L12.5 6"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}
