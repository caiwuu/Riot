import { useEffect, useRef, useState } from "react";

import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";

import {
  type TermEvent,
  termAttach,
  termBusy,
  termClose,
  termList,
  termOpen,
  termResize,
  termShare,
  termWrite,
} from "../bridge";
import { type ConfirmRequest, ConfirmDialog } from "./ConfirmDialog";
import { basename } from "../pathDisplay";

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
  onAgentTerminal,
  onSendSelection,
}: {
  visible: boolean;
  height: number;
  /** 新标签在哪个目录开 shell。null = 家目录。 */
  defaultRoot: string | null;
  /** 用户收面板、或最后一个标签关闭。shell 不一定死，见组件注释。 */
  onHide: () => void;
  /** 模型起了个服务。面板该自己弹出来 —— 那是它跑在哪里的唯一线索。 */
  onAgentTerminal?: () => void;
  /** 把选中的输出交给输入框。用户的终端模型读不到，要给就这么给。 */
  onSendSelection?: (text: string) => void;
}) {
  const [state, setState] = useState<{ tabs: Tab[]; active: string | null }>({
    tabs: [],
    active: null,
  });
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  /** 共享开关被宿主拒绝。不给一行字的话，用户会以为已经共享上了。 */
  const [shareError, setShareError] = useState(false);
  const shareErrTimer = useRef<number | undefined>(undefined);
  /** 当前有选区的标签。"发给模型"按钮据此禁用 —— 没选中时点了
   *  悄无声息什么都不发生，比灰掉更让人困惑。 */
  const [selected, setSelected] = useState<Set<string>>(new Set());
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
    setSelected((prev) => {
      if (!prev.has(uid)) return prev;
      const next = new Set(prev);
      next.delete(uid);
      return next;
    });
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

  /**
   * 用户点关闭。和 closeTab 分开：closeTab 是无条件收尾（exit 事件也走它），
   * 这条路先问宿主"前台有没有东西在跑" —— 一键杀掉正忙的 dev server
   * 不该是无声的。问不到（宿主没起来等）就当不忙，别把关闭堵死。
   */
  const requestClose = (tab: Tab) => {
    const doClose = () => closeTab(tab.uid);
    if (tab.hostId == null) {
      // PTY 还没落地，shell 里不可能跑着东西
      doClose();
      return;
    }
    termBusy(tab.hostId).then((busy) => {
      if (!busy) {
        doClose();
        return;
      }
      setConfirm({
        title: `关闭「${tab.title}」？`,
        body: tab.fromAgent
          ? "这是模型起的服务，关闭会立即终止它 —— 模型可能正依赖这个服务。"
          : "这个终端里有正在运行的进程，关闭会立即终止它。",
        confirmLabel: "关闭并终止",
        action: doClose,
      });
    }, doClose);
  };

  const addTab = (root: string | null) => {
    // 标题去重要看现有标签，所以 mkTab 挪进 updater 里拿最新的 tabs
    setState((prev) => {
      const tab = mkTab(root, prev.tabs);
      return { tabs: [...prev.tabs, tab], active: tab.uid };
    });
  };

  // 模型起的服务：宿主那边已经在跑了，这里认领过来变成一个标签。
  //
  // 轮询而不是等事件：起服务发生在一轮的中间，等轮次结束才显示的话，
  // 用户要盯着一个"它在干什么"的空白等上几分钟。三秒一次 IPC 返回几个
  // 小结构，这点开销换的是"服务一起来就看得见"。
  const agentRef = useRef(onAgentTerminal);
  agentRef.current = onAgentTerminal;
  useEffect(() => {
    let alive = true;
    const scan = () => {
      void termList()
        .then((list) => {
          if (!alive) return;
          const known = new Set(
            [...instances.current.values()].flatMap((i) => (i.hostId == null ? [] : [i.hostId])),
          );
          const fresh = list.filter((t) => t.command != null && !known.has(t.id));
          if (fresh.length === 0) return;
          setState((prev) => {
            // 已经认领过的不再来一遍：hostId 落地有一拍延迟，
            // 光看 instances 会在那一拍里重复建标签。
            const claimed = new Set(prev.tabs.flatMap((t) => (t.hostId == null ? [] : [t.hostId])));
            const add = fresh
              .filter((t) => !claimed.has(t.id))
              .map((t) => adoptTab(t.id, t.title));
            if (add.length === 0) return prev;
            // 不抢 active —— 用户正看着/用着当前标签，服务在后台认领即可，
            // 面板底部会亮出"模型"标签作为线索。只有面板原本空着（没有
            // 任何标签）时才切过去，否则会打断正在进行的操作。
            const last = add[add.length - 1];
            const active = prev.active == null && last ? last.uid : prev.active;
            return { tabs: [...prev.tabs, ...add], active };
          });
          agentRef.current?.();
        })
        .catch(() => {
          // 宿主还没起来 / 命令被拒。下一轮再说。
        });
    };
    scan();
    const timer = setInterval(scan, 3000);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, []);

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
  //
  // 守卫：焦点正在别的可编辑元素上（对话输入框、地址栏）时不抢 ——
  // 模型在后台起了服务触发面板弹出，不该把用户正打的字劫进终端。
  useEffect(() => {
    if (!visible || !state.active) return;
    const el = document.activeElement;
    const editingElsewhere =
      el instanceof HTMLElement &&
      (el.isContentEditable ||
        el.tagName === "INPUT" ||
        el.tagName === "TEXTAREA") &&
      !el.closest(".term-panel");
    if (editingElsewhere) return;
    instances.current.get(state.active)?.term.focus();
  }, [visible, state.active]);

  /**
   * 给一个标签建 xterm + PTY。ref 回调每次渲染都会来一遍（内联箭头函数），
   * 靠 instances 去重 —— StrictMode 的双挂载也被同一个守卫挡住。
   */
  const mount = (tab: Tab, el: HTMLDivElement) => {
    if (instances.current.has(tab.uid)) return;

    const term = new Terminal({
      // 和 styles.css 的 --mono 一致。自带的 Riot Mono 在前 ——
      // 打包的 WKWebView 会把 ui-monospace 错误解析成非等宽字体，
      // 终端里 ls 的列对齐全花（dev 下看不出来，只在安装包里出现）。
      fontFamily: '"Riot Mono", Menlo, ui-monospace, monospace',
      fontSize: 12,
      scrollback: 5000,
      theme: THEME,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);

    const inst: Inst = {
      term,
      fit,
      hostId: null,
      pending: [],
      ro: new ResizeObserver(() => safeFit(inst)),
    };
    instances.current.set(tab.uid, inst);

    // 字体就绪后再 open：xterm 在 open 时量字符格宽度，早于字体加载的话
    // 量的是回退字体，列数和渲染宽度都会错一截。字体是本地资源，这里
    // 等的通常是几毫秒；加载失败也照常打开（回退 Menlo）。
    const fontsReady: Promise<unknown> =
      "fonts" in document ? document.fonts.load('12px "Riot Mono"') : Promise.resolve();
    void fontsReady.catch(() => {}).then(() => {
      // 等待期间标签可能已经被关掉，实例没了就别再画
      if (!instances.current.has(tab.uid)) return;
      term.open(el);
      safeFit(inst);
      inst.ro.observe(el);
    });

    // PTY 还没落地时敲的字先攒着。丢掉的话，面板刚开就打字的人会看到
    // 开头缺了几个字符 —— 而那正是最常见的使用方式。
    term.onData((d) => {
      if (inst.hostId == null) inst.pending.push(d);
      else void termWrite(inst.hostId, d).catch(() => {});
    });
    term.onResize(({ cols, rows }) => {
      if (inst.hostId != null) void termResize(inst.hostId, cols, rows).catch(() => {});
    });
    // 选区变化喂给"发给模型"按钮的禁用态。只在有无之间翻转时更新，
    // 拖选过程中每个字符都触发这个事件，不守着会白渲染一片。
    term.onSelectionChange(() => {
      const has = term.getSelection().trim().length > 0;
      setSelected((prev) => {
        if (prev.has(tab.uid) === has) return prev;
        const next = new Set(prev);
        if (has) next.add(tab.uid);
        else next.delete(tab.uid);
        return next;
      });
    });

    // attach 对已死终端会立刻补一条 Exit，读线程退出时又发一条 ——
    // 两条都到时"已退出"那行别写两遍。
    let sawExit = false;
    const onEvent = (ev: TermEvent) => {
      if (ev.kind === "data") {
        term.write(b64ToBytes(ev.data));
        return;
      }
      if (sawExit) return;
      sawExit = true;
      if (tab.fromAgent) {
        // 模型起的服务退出后，宿主**保留**条目（模型要读最后的报错），
        // 只有 termClose 才移除。这里跟着关标签的话，本地没了、宿主还在，
        // 3 秒一次的认领轮询又把它捡回来 —— 表现为标签几秒闪现一次，
        // 面板关了也自己弹出来。留着标签展示最后输出，用户点 X 才真正关。
        term.write("\r\n\x1b[2m[进程已退出。日志留在这里，点标签上的 × 关闭。]\x1b[0m\r\n");
        setState((prev) => ({
          ...prev,
          tabs: prev.tabs.map((t) => (t.uid === tab.uid ? { ...t, exited: true } : t)),
        }));
      } else {
        // 用户自己的 shell：exit/崩溃时宿主已把条目摘掉，这里只收尾界面。
        closeRef.current(tab.uid, { hostDead: true });
      }
    };

    // 模型起的服务已经在宿主那边跑着了：挂上去接住后续输出，顺便回放
    // 它已经打出来的那些。不是新开一个 shell。
    if (tab.hostId != null) {
      const id = tab.hostId;
      inst.hostId = id;
      termAttach(id, onEvent)
        .then(() => void termResize(id, term.cols, term.rows).catch(() => {}))
        .catch((e: unknown) => {
          term.write(
            `\r\n\x1b[31m接不上这个终端。它对应的服务可能已经退出了，可以关掉这个标签。（${String(e)}）\x1b[0m\r\n`,
          );
        });
      return;
    }

    termOpen(tab.root, term.cols, term.rows, onEvent)
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
        term.write(
          `\r\n\x1b[31m终端没能启动，可以关掉这个标签再开一个试试。（${String(e)}）\x1b[0m\r\n`,
        );
      });
  };

  const activeTab = state.tabs.find((t) => t.uid === state.active);
  const hasSelection = state.active != null && selected.has(state.active);

  /** 共享开关。宿主是真相，本地状态只为了让按钮立刻亮起来。 */
  const toggleShare = (tab: Tab) => {
    if (tab.hostId == null) return;
    const next = !tab.shared;
    void termShare(tab.hostId, next).then(
      () =>
        setState((prev) => ({
          ...prev,
          tabs: prev.tabs.map((t) => (t.uid === tab.uid ? { ...t, shared: next } : t)),
        })),
      () => {
        // 宿主没接受就别改按钮 —— 显示成"已共享"而实际没共享是最坏的错。
        // 但按钮纹丝不动也不行，用户会当作点上了：给一行短暂的红字。
        setShareError(true);
        window.clearTimeout(shareErrTimer.current);
        shareErrTimer.current = window.setTimeout(() => setShareError(false), 3000);
      },
    );
  };

  return (
    <div
      className="term-panel"
      style={{ height, display: visible ? undefined : "none" }}
    >
      <div
        className="term-tabs"
        onWheel={(e) => {
          // 这条栏只能横向滚，普通鼠标的纵向滚轮在这里原本是死的 ——
          // 转成横向，被挤出去的标签才滚得回来。
          if (Math.abs(e.deltaY) > Math.abs(e.deltaX)) e.currentTarget.scrollLeft += e.deltaY;
        }}
      >
        {state.tabs.map((t) => (
          <button
            key={t.uid}
            className={t.uid === state.active ? "term-tab active" : "term-tab"}
            onClick={() => setState((prev) => ({ ...prev, active: t.uid }))}
            onAuxClick={(e) => {
              // 中键关标签（浏览器惯例）。照样走 requestClose ——
              // 中键点到正在跑东西的标签，确认一步不能省。
              if (e.button === 1) {
                e.preventDefault();
                requestClose(t);
              }
            }}
            title={t.hostId != null ? `${t.title}（模型起的服务）` : (t.root ?? "~")}
          >
            {/* 标出哪些不是自己开的。用户看到一个没印象的标签在跑东西，
                第一反应是"这哪来的" —— 这个点直接回答它。 */}
            {t.hostId != null ? (
              <span className={t.exited ? "term-tab-badge exited" : "term-tab-badge"}>
                {t.exited ? "已退出" : "模型"}
              </span>
            ) : (
              <TermIcon />
            )}
            <span className="term-tab-title">{t.title}</span>
            {/* span 而不是嵌套 button —— button 套 button 是非法 HTML，
                浏览器会把内层拆出去，点击行为不可预料。tabIndex + 键盘触发
                自己补：role 只是声明，键盘可达要真做。 */}
            <span
              className="term-tab-close"
              role="button"
              tabIndex={0}
              aria-label="关闭终端"
              onClick={(e) => {
                e.stopPropagation();
                requestClose(t);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  e.stopPropagation();
                  requestClose(t);
                }
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
        {/* 把整个终端交给模型看。默认关 —— 用户自己的 shell 里有他敲过的
            密码和与本次任务无关的一切。开了之后模型能读这里的输出（但停不掉
            这个终端），省掉「我的 dev server 报错了」时手动复制几十行日志。
            模型起的服务不需要这个开关，它本来就读得到自己起的。 */}
        {shareError ? <span className="term-share-error">共享失败</span> : null}
        {activeTab && !activeTab.fromAgent && activeTab.hostId != null ? (
          <button
            className={activeTab.shared ? "icon term-share on" : "icon term-share"}
            onClick={() => toggleShare(activeTab)}
            aria-pressed={!!activeTab.shared}
            title={
              activeTab.shared
                ? "正在共享给 agent：它能读这个终端的输出（点击收回）"
                : "共享给 agent：让它能读这个终端的输出，但不能停它"
            }
          >
            <ShareIcon />
            {/* 隐私开关不能只靠图标变色表达 —— 亮出文字，色弱也看得清 */}
            {activeTab.shared ? <span className="term-share-mark">已共享</span> : null}
          </button>
        ) : null}
        {/* 只想给一小段而不是整个终端时用这个 —— 给什么完全由用户决定。 */}
        {onSendSelection ? (
          <button
            className="icon"
            disabled={!hasSelection}
            onClick={() => {
              const sel = state.active
                ? instances.current.get(state.active)?.term.getSelection().trim()
                : "";
              if (sel) onSendSelection(sel);
            }}
            title={
              hasSelection
                ? "把选中的内容发给模型"
                : "先在终端里选中一段文本，再从这里发给模型"
            }
          >
            <SendUpIcon />
          </button>
        ) : null}
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
      {confirm ? <ConfirmDialog c={confirm} onClose={() => setConfirm(null)} /> : null}
    </div>
  );
}

interface Tab {
  uid: string;
  title: string;
  root: string | null;
  /** 已经在宿主那边跑着的终端（模型起的服务）。挂上去而不是新开。 */
  hostId?: number;
  /** 模型起的服务（不是用户自己开的 shell）。 */
  fromAgent?: boolean;
  /** 服务进程已退出，标签只剩日志。宿主侧条目还在，点 × 才真正移除。 */
  exited?: boolean;
  /** 用户把这个终端共享给模型了。只对自己开的 shell 有意义。 */
  shared?: boolean;
}

function mkTab(root: string | null, existing: Tab[] = []): Tab {
  // 同目录开出来的标签标题一模一样，撞了就加序号 —— 三个"Riot"
  // 并排时用户只能挨个点开猜哪个是哪个。
  const base = (root ? basename(root) : "") || "终端";
  let title = base;
  for (let n = 2; existing.some((t) => t.title === title); n++) title = `${base} ${n}`;
  return {
    uid: `t-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
    title,
    root,
  };
}

/** 认领一个模型起的终端。 */
function adoptTab(hostId: number, title: string): Tab {
  return {
    uid: `t-agent-${hostId}`,
    title,
    root: null,
    hostId,
    fromAgent: true,
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
  // 和聊天区同底（--bg）。曾经用侧栏的 #121212，深一档的结果是终端、
  // 抽屉、主区三种灰凑在一屏上。
  background: "#181818",
  foreground: "#ececf1",
  cursor: "#ececf1",
  cursorAccent: "#181818",
  selectionBackground: "#3d3d3d",
  // ANSI black 不能和背景同色：TUI 拿它画分隔和填充。
  black: "#121212",
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
  scrollbarSliderBackground: "#ffffff10",
  scrollbarSliderHoverBackground: "#ffffff1c",
  scrollbarSliderActiveBackground: "#ffffff28",
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

/** 把选中的输出送去输入框。 */
/** 共享给 agent：一个从方框里指出去的箭头。 */
function ShareIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M9.5 2.5H13v3.5M13 2.5L8 7.5"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="M12 9.5v3a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1v-7a1 1 0 0 1 1-1h3"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
      />
    </svg>
  );
}

function SendUpIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M8 13V3.5M4 7.5L8 3.5l4 4"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
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
