import { useCallback, useEffect, useRef, useState } from "react";

import {
  type BrowserFrame,
  type BrowserInput,
  type PanelState,
  type TabInfo,
  browserCloseTab,
  browserHistory,
  browserInput,
  browserNavigate,
  browserNewTab,
  browserReload,
  browserResize,
  browserSelectTab,
  browserState,
  closeBrowser,
  openBrowser,
} from "../bridge";

/**
 * 内置浏览器面板。
 *
 * 画面是宿主推来的 JPEG 帧，输入原路打回页面。看起来像内嵌了一个浏览器，
 * 实际是另一个进程在渲染 —— 那个进程跑的是 CEF，而它必须活在自己的
 * `.app` 里（见 crates/riot-browser）。
 *
 * # 为什么不用 iframe
 *
 * iframe 只能显示，读不到跨域页面的 DOM、截不了图、也拿不到 console。
 * 而这个面板存在的意义正是"你和模型看同一个东西" —— 模型那边靠 CDP，
 * 面板这边靠帧流，两者看的是同一个渲染结果。
 */
export function BrowserPanel({
  sessionId,
  onClose,
}: {
  sessionId: string;
  onClose: () => void;
}) {
  const [frame, setFrame] = useState<BrowserFrame | null>(null);
  const [panel, setPanel] = useState<PanelState>(EMPTY_PANEL);
  const [address, setAddress] = useState("");
  const [busy, setBusy] = useState(false);
  const viewRef = useRef<HTMLDivElement>(null);
  const imeRef = useRef<HTMLTextAreaElement>(null);
  /** 上一次点击的位置。输入法的候选窗口贴着它弹。 */
  const [caret, setCaret] = useState({ x: 0, y: 0 });
  const addressRef = useRef<HTMLInputElement>(null);

  /**
   * 把新的导航状态铺到界面上。
   *
   * `[约束]` 地址栏有焦点的时候一个字都不能动 —— 那时候它属于用户。
   *
   * 判据必须是**真实焦点**，不能是自己记的一个"正在编辑"标志。之前记标志
   * 的版本在提交地址时把它清掉了（想着"提交完就该跟页面走"），可输入框还
   * 握着焦点 —— 于是用户接着打的每一个字都被一秒一次的同步冲掉，地址栏
   * 像有人在抢着输入，而且怎么都打不完一个地址。
   */
  const apply = useCallback((s: PanelState) => {
    setPanel(s);
    const url = s.tabs.find((t) => t.id === s.active)?.url ?? "";
    if (document.activeElement !== addressRef.current) setAddress(url);
  }, []);

  useEffect(() => {
    // 第三个参数让标签栏在浏览器就绪的那一刻就填上，不用等下一次定时同步 ——
    // 浏览器起来本身就要一秒，再叠一个轮询间隔就是两秒的空白。
    const sub = openBrowser(sessionId, setFrame, apply);
    return () => {
      sub.unsubscribe();
      // 让宿主也停下来。只取消本地订阅的话，另一头还在编码 JPEG。
      void closeBrowser(sessionId);
    };
  }, [sessionId, apply]);

  /**
   * 只换一页的信息。前进后退回来的就是这一条 —— 它作用在活动页上。
   *
   * 用函数式更新而不是读一份快照:定时同步随时可能在中间落地，读快照会
   * 把它的结果覆盖掉。
   */
  const patch = useCallback((info: TabInfo) => {
    setPanel((prev) => ({
      ...prev,
      tabs: prev.tabs.map((t) => (t.id === info.id ? info : t)),
    }));
    if (document.activeElement !== addressRef.current) setAddress(info.url);
  }, []);

  /**
   * 地址栏跟着页面走。
   *
   * `[取舍]` 定期问，而不是订阅页面的导航事件。
   *
   * 地址会在四种情况下变：地址栏跳转、前进后退、点页面里的链接、SPA 自己
   * 改 history。前两种是我们发的命令，回值里就带着新状态；后两种只有页面
   * 知道，要拿到就得订阅 `Page.frameNavigated` 再给面板铺一条推送通道。
   * 而这个面板每秒本来就在收十几帧 JPEG，一次几十字节的往返在那个背景噪音
   * 里量都量不出来。等真的需要"地址变化的精确时刻"再改。
   */
  useEffect(() => {
    let alive = true;
    const poll = () => {
      // 失败就保持上一次的显示。页面换文档的瞬间这条问不到，
      // 把地址栏清空会让它每次跳转都闪一下。
      browserState(sessionId)
        .then((s) => {
          if (alive) apply(s);
        })
        .catch(() => {});
    };
    poll();
    const timer = window.setInterval(poll, NAV_POLL_MS);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [sessionId, apply]);

  /**
   * 让页面的视口和画面区一样大、一样清晰。
   *
   * `[约束]` 画面区尺寸变了必须同步过去。页面按 1280×800 渲染、而面板是
   * 竖着的一条时，等比缩放后上下会各留出两百多像素的黑边 —— 用户看到的是
   * "页面只占中间一条"，而且整页被缩到六成，16px 的字只剩九个像素高。
   * 同步之后帧和面板同比例，正好铺满，页面也是 1:1 显示。
   *
   * `[约束]` 像素密度也要一起给。Retina 上面板占的是两倍物理像素，帧却是
   * 按一倍出的 —— 浏览器把它放大一倍铺上去，文字边缘全是虚的。这种糊
   * 尤其难归因：内容、比例、点击位置全对，看起来只像是画质差。
   */
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;

    let timer: number | undefined;
    let last = "";
    const push = () => {
      const width = view.clientWidth;
      const height = view.clientHeight;
      // 刚挂上还没布局时量到的是 0。0 尺寸的视口在 CEF 那边等同于"看不见"，
      // 从此不再出帧 —— 画面会停在最后一帧，而且不报错。
      if (width < MIN_VIEWPORT || height < MIN_VIEWPORT) return;
      const scale = window.devicePixelRatio || 1;
      const key = `${width}x${height}@${scale}`;
      if (key === last) return;
      last = key;
      void browserResize(sessionId, width, height, scale).catch(() => {});
    };

    // 防抖。拖窗口时 ResizeObserver 每帧都回调，每次都让页面重排一遍的话，
    // 拖动过程中画面整个卡住。
    const later = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(push, RESIZE_QUIET_MS);
    };

    const observer = new ResizeObserver(later);
    observer.observe(view);

    // 窗口从外接屏拖回内置屏时，CSS 尺寸一点没变，只有密度变了 ——
    // ResizeObserver 不会响。没有这条监听的话，画面会一直按上一块屏的
    // 密度出，直到下一次拖动窗口。
    //
    // 每次变完要重新挂：这个查询问的是"密度还是不是 X"，而 X 是挂的那一刻
    // 的值。不重挂的话，1.5 换到 3 这种两头都不等于 X 的跳变一声不响。
    let media: MediaQueryList | null = null;
    const onDensity = () => {
      later();
      watchDensity();
    };
    const watchDensity = () => {
      media?.removeEventListener("change", onDensity);
      media = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
      media.addEventListener("change", onDensity);
    };
    watchDensity();

    push();

    return () => {
      observer.disconnect();
      media?.removeEventListener("change", onDensity);
      window.clearTimeout(timer);
    };
  }, [sessionId]);

  /**
   * DOM 坐标 → 页面坐标。
   *
   * `[约束]` 必须按帧的真实尺寸换算，不能直接用鼠标事件的 offsetX/Y。
   * 画面是等比缩放后铺在面板里的，两者的比例通常不是 1 —— 不换算的话
   * 点击会系统性地偏，而且窗口越窄偏得越多。
   */
  const toPage = useCallback(
    (e: React.MouseEvent): { x: number; y: number } | null => {
      const img = viewRef.current?.querySelector("img");
      if (!img || !frame) return null;
      const r = img.getBoundingClientRect();
      if (r.width === 0 || r.height === 0) return null;
      return {
        x: ((e.clientX - r.left) / r.width) * frame.width,
        y: ((e.clientY - r.top) / r.height) * frame.height,
      };
    },
    [frame],
  );

  const send = useCallback(
    (input: BrowserInput) => void browserInput(sessionId, input).catch(() => {}),
    [sessionId],
  );

  const go = async () => {
    const url = normalize(address);
    if (!url) return;
    // 先把补全后的地址显示出来。之后交给同步 —— 用户输的是意图，该显示的
    // 是真的落在哪儿（重定向之后的地址、跳转到的登录页）。
    setAddress(url);
    setBusy(true);
    try {
      await browserNavigate(sessionId, url);
    } finally {
      setBusy(false);
    }
  };

  const step = (delta: number) =>
    void browserHistory(sessionId, delta).then(patch).catch(() => {});

  /**
   * 关完一页之后。
   *
   * 一页都不剩就把面板收起来 —— 和浏览器里关掉最后一个标签页等于关窗口
   * 一个道理。宿主那边此时也没有活动页了，模型下次用到浏览器会现开一个。
   */
  const closed = (s: PanelState) => {
    if (s.tabs.length === 0) {
      onClose();
      return;
    }
    apply(s);
  };

  const active = panel.tabs.find((t) => t.id === panel.active);
  /**
   * 浏览器还在起。
   *
   * `[约束]` 这段时间标签栏不能是空的。CEF 起来要一秒左右（六个进程），
   * 空着的话面板会先显示一条只有"+"的光秃秃的栏，一秒后才"长出"一个标签，
   * 看起来像刚才那下没点上。摆一个占位标签，界面的形状从第一帧就是对的。
   */
  const starting = panel.tabs.length === 0;

  return (
    <div className="browser-panel">
      <div className="browser-tabs">
        {starting ? (
          <span className="browser-tab active">
            <GlobeIcon />
            <span className="browser-tab-title">{TAB_PLACEHOLDER}</span>
          </span>
        ) : null}
        {panel.tabs.map((t) => (
          <button
            key={t.id}
            className={t.id === panel.active ? "browser-tab active" : "browser-tab"}
            onClick={() => void browserSelectTab(sessionId, t.id).then(apply).catch(() => {})}
            title={t.url || TAB_PLACEHOLDER}
          >
            <GlobeIcon />
            <span className="browser-tab-title">{t.title || TAB_PLACEHOLDER}</span>
            {/*
             * 关闭做成 span 而不是嵌套 button：button 套 button 是非法的
             * HTML，React 会照渲染，但浏览器会把内层拆出去，点击行为随之
             * 不可预料。
             */}
            <span
              className="browser-tab-close"
              role="button"
              aria-label="关闭标签页"
              onClick={(e) => {
                // 不冒泡给外层的"切到这一页" —— 否则关掉的同时又切了过去。
                e.stopPropagation();
                void browserCloseTab(sessionId, t.id).then(closed).catch(() => {});
              }}
            >
              <CloseIcon />
            </span>
          </button>
        ))}
        <button
          className="icon"
          onClick={() => void browserNewTab(sessionId).then(apply).catch(() => {})}
          disabled={starting}
          title="新标签页"
        >
          <PlusIcon />
        </button>
        <span className="browser-tabs-spacer" />
        <button className="icon" onClick={onClose} title="关闭面板">
          <PanelIcon />
        </button>
      </div>

      <div className="browser-bar">
        <button
          className="icon"
          onClick={() => step(-1)}
          disabled={!active?.canBack}
          title="后退"
        >
          <BackIcon />
        </button>
        <button
          className="icon"
          onClick={() => step(1)}
          disabled={!active?.canForward}
          title="前进"
        >
          <ForwardIcon />
        </button>
        <button
          className="icon"
          onClick={() => void browserReload(sessionId).catch(() => {})}
          disabled={!active?.url}
          title="刷新"
        >
          <ReloadIcon />
        </button>
        <div className="browser-address">
          <input
            ref={addressRef}
            value={address}
            onChange={(e) => setAddress(e.target.value)}
            onKeyDown={(e) => {
              // 回车之后把焦点交出去。地址栏这才重新开始跟着页面走，而键盘
              // 也回到页面上 —— 打完地址接着就能在页面里打字。
              if (e.key === "Enter") {
                void go();
                e.currentTarget.blur();
              }
              // Esc 放弃这次编辑。失焦之后下一次同步会把真实地址填回来。
              if (e.key === "Escape") e.currentTarget.blur();
            }}
            placeholder="输入 URL"
            spellCheck={false}
          />
          <button
            className="icon"
            onClick={() => void go()}
            disabled={busy || !address.trim()}
            title="打开"
          >
            <OpenIcon />
          </button>
        </div>
      </div>

      <div
        className="browser-view"
        ref={viewRef}
        onClick={(e) => {
          const p = toPage(e);
          if (p) send({ kind: "click", ...p, button: "left" });
          // 焦点交给下面那个隐藏的 textarea，键盘的事都由它接。
          const r = viewRef.current?.getBoundingClientRect();
          if (r) setCaret({ x: e.clientX - r.left, y: e.clientY - r.top });
          imeRef.current?.focus();
        }}
        onMouseMove={(e) => {
          const p = toPage(e);
          if (p) send({ kind: "move", ...p });
        }}
        onWheel={(e) => {
          // 两个轴原样转发，不自己判断方向。macOS 上按住 shift 滚轮时
          // 系统已经把量放进了 deltaX，这里再换一次就换回去了。
          const p = toPage(e as unknown as React.MouseEvent);
          if (p) send({ kind: "scroll", ...p, deltaX: e.deltaX, deltaY: e.deltaY });
        }}
      >
        {/*
         * 键盘的落点。
         *
         * `[约束]` 必须是个真的可编辑元素，不能把 tabIndex 挂在上面那个 div 上。
         * 输入法只在有文本输入上下文的地方才挂得上来 —— div 拿不到组字，
         * 于是拼音的原始字母被当成普通字符送进页面，表现是"只能输入英文"。
         *
         * 位置跟着上一次点击走。它是透明的 1 像素，看不见，但候选窗口是贴着
         * 它弹的 —— 钉死在角上的话，你在页面中间打字、候选词在左上角。
         */}
        <textarea
          className="browser-ime"
          ref={imeRef}
          style={{ left: caret.x, top: caret.y }}
          // 输入法自己有候选和纠错，浏览器再插一手只会打架。
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          onInput={(e) => {
            const ev = e.nativeEvent as InputEvent;
            // 组字中间态交给 composition 那条路。这里也收的话，一个字会
            // 进去两次 —— 一次临时的、一次最终的。
            if (ev.isComposing) return;
            const text = ev.data;
            // 清空。留着的话它会一直攒，而我们要的只是这一次的增量。
            e.currentTarget.value = "";
            if (text) send({ kind: "text", text });
          }}
          onCompositionUpdate={(e) => send({ kind: "compose", text: e.data })}
          onCompositionEnd={(e) => {
            e.currentTarget.value = "";
            // 有内容就是确认，没内容就是取消（按了 Esc 或者删空了拼音）。
            send(e.data ? { kind: "text", text: e.data } : { kind: "compose", text: "" });
          }}
          onKeyDown={(e) => {
            // 组字期间的按键属于输入法：回车是"选定候选"、退格是"删拼音"。
            // 一并转给页面的话，回车会把表单提前提交掉。
            if (e.nativeEvent.isComposing) return;
            // 普通字符不在这儿发 —— 它们会变成 input 事件，那条路连中文、
            // emoji 和粘贴一起管了。
            if (FUNCTION_KEYS.has(e.key)) {
              e.preventDefault();
              send({ kind: "key", key: e.key });
            }
          }}
        />

        {/*
         * 空标签页给一句人话，而不是把空白页的画面摆上来。
         *
         * 空白页渲染出来是一整片纯色 —— 和"画面没出来"、"页面白屏"长得
         * 一模一样，用户没法分辨面板是在等还是坏了。
         *
         * `busy` 期间不显示:那时候导航已经发出去了，但地址要等页面提交才
         * 更新（最多一秒）。不看这个标志的话，输完地址回车会先看到一秒
         * "开始浏览"，像是没接收到。
         */}
        {frame && (active?.url || busy) ? (
          <img
            src={`data:image/jpeg;base64,${frame.data}`}
            alt=""
            draggable={false}
          />
        ) : (
          <div className="browser-empty">
            {frame ? (
              <>
                <GlobeIcon size={30} />
                <p className="browser-empty-title">开始浏览</p>
                <p className="hint">输入网址打开页面。这是远程渲染的画面，和模型看到的是同一帧。</p>
              </>
            ) : (
              <p className="hint">浏览器启动中…</p>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

/** 面板刚挂上、宿主还没回状态时的样子。 */
const EMPTY_PANEL: PanelState = { tabs: [], active: 0 };

/** 还没加载过东西的标签页显示成这个。 */
const TAB_PLACEHOLDER = "新标签页";

/**
 * 多久问一次页面地址。
 *
 * 一秒是"点了链接之后瞥一眼地址栏，它已经变了"的量级。再快没有意义 ——
 * 用户的视线从页面移到地址栏本来就要这么久。
 */
const NAV_POLL_MS = 1000;

/** 小于这个尺寸不同步 —— 那种尺寸下的页面没法看，而且 0 会让 CEF 停止出帧。 */
const MIN_VIEWPORT = 80;

/** 尺寸稳定多久之后才同步。拖动过程中每一帧都发的话，页面会一直在重排。 */
const RESIZE_QUIET_MS = 120;

/** 交给宿主处理的功能键。列表外的组合键留给系统。 */
const FUNCTION_KEYS = new Set([
  "Enter",
  "Backspace",
  "Tab",
  "Escape",
  "ArrowLeft",
  "ArrowUp",
  "ArrowRight",
  "ArrowDown",
  "Delete",
  "Home",
  "End",
  "PageUp",
  "PageDown",
]);

/**
 * 补全用户输入的地址。
 *
 * 直接把 `example.com` 传下去的话，CDP 会当成相对路径而导航到一个不存在
 * 的地方，页面白屏但不报错。本地绝对路径不要补 https —— 那会变成
 * `https:///Users/...`，同样白屏。
 */
function normalize(raw: string): string | null {
  const s = raw.trim();
  if (!s) return null;
  if (/^[a-z][a-z0-9+.-]*:/i.test(s)) return s;
  if (s.startsWith("/")) return `file://${s}`;
  if (/^[a-zA-Z]:[\\/]/.test(s)) return `file:///${s.replace(/\\/g, "/")}`;
  return `https://${s}`;
}

/* ── 图标 ───────────────────────────────────── */

/** 标签页和空状态用的地球。空状态那处要大一号，所以尺寸可传。 */
function GlobeIcon({ size = 12 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="none" aria-hidden>
      <circle cx="8" cy="8" r="6" stroke="currentColor" strokeWidth="1.3" />
      <path
        d="M2 8h12M8 2c1.8 2 1.8 10 0 12M8 2C6.2 4 6.2 12 8 14"
        stroke="currentColor"
        strokeWidth="1.3"
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

/** 关闭面板。画的是"右边那一栏收起来"。 */
function PanelIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <rect x="2" y="3" width="12" height="10" rx="2" stroke="currentColor" strokeWidth="1.3" />
      <path d="M10 3v10" stroke="currentColor" strokeWidth="1.3" />
    </svg>
  );
}

/** 地址栏右端的"打开"。斜向上的箭头 —— 和回车一个意思。 */
function OpenIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M5 11L11 5M11 5H6.5M11 5v4.5"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function BackIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M10 3L5 8l5 5"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function ForwardIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M6 3l5 5-5 5"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function ReloadIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M14 8a6 6 0 1 1-6-6c1.68 0 3.29.71 4.45 1.86L14 5.33M14 2v3.33h-3.33"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}
