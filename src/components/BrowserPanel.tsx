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
  /**
   * 收到过画面没有。只在第一帧翻一次 —— 帧本身不进 React 状态。
   *
   * `[约束]` 不能每帧 setState。一帧就是一次整棵子树的重渲染，15~30fps
   * 下主线程全花在 React 提交上，输入事件被挤在后面，表现就是"看着卡、
   * 点着也卡"。画面走 canvas 直绘（见 paint），React 只管空状态的切换。
   */
  const [hasFrame, setHasFrame] = useState(false);
  /** 画布。帧解码完直接画上来，不经过 React。 */
  const canvasRef = useRef<HTMLCanvasElement>(null);
  /** 最近一帧的 CSS 尺寸。toPage 的坐标换算靠它。 */
  const frameSize = useRef<{ w: number; h: number } | null>(null);
  /** 正在解码一帧。期间到的新帧放 nextFrame，解完立刻画它。 */
  const painting = useRef(false);
  /**
   * 等着画的下一帧。永远只留最新的一帧 —— 解码追不上推流时，追着播
   * 旧帧只会让画面越来越落后于手上的操作。丢帧丢的是中间过程，
   * 最终画面永远是最新的。
   */
  const nextFrame = useRef<BrowserFrame | null>(null);
  const [panel, setPanel] = useState<PanelState>(EMPTY_PANEL);
  const [address, setAddress] = useState("");
  const [busy, setBusy] = useState(false);
  /** 导航失败给一行原因 —— 之前 go() 无 catch，慢站/打不开时画面停在原地，
   *  用户分不清"在加载/挂了/没点上"。 */
  const [navError, setNavError] = useState("");
  const viewRef = useRef<HTMLDivElement>(null);
  const imeRef = useRef<HTMLTextAreaElement>(null);
  /** 上一次点击的位置。输入法的候选窗口贴着它弹。 */
  const [caret, setCaret] = useState({ x: 0, y: 0 });
  const addressRef = useRef<HTMLInputElement>(null);
  /**
   * 地址栏 IME：确认候选/上屏英文时，keydown(Enter) 常在 compositionend
   * 之后到达，此时 isComposing 已是 false，会被误当成导航。用 ref 盖住
   * 这一拍 —— 和对话输入同一套坑。
   */
  const addressIme = useRef(false);
  /**
   * 视口模式。自适应＝页面按面板的实际尺寸渲染（所见即面板）；Web＝按
   * 桌面宽度渲染，再整体缩小塞进面板。
   *
   * 面板窄的时候页面会切进移动端布局，模型的整页截图也跟着变窄 ——
   * 而用户常常正想确认"桌面版长什么样"。Web 模式把渲染宽度和面板宽度
   * 解耦，用户和模型看到的都是桌面版。
   */
  const [viewMode, setViewMode] = useState<ViewMode>(() =>
    localStorage.getItem(VIEW_MODE_KEY) === "web" ? "web" : "fit",
  );

  /** 换模式并记住。这是"怎么看页面"的个人偏好，跨会话、跨重启生效。 */
  const switchMode = (mode: ViewMode) => {
    setViewMode(mode);
    localStorage.setItem(VIEW_MODE_KEY, mode);
  };

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

  /**
   * 把一帧画到 canvas 上。
   *
   * `createImageBitmap` 的解码是异步的、不占布局和绘制的档期 ——
   * 这正是不用 `<img src="data:...">` 的原因:data URL 每帧都要在主线程
   * 上解析几百 KB 的字符串再同步解码 JPEG，滚动时一卡一卡的就是它。
   *
   * `[约束]` 一次只解一帧。解码期间来的新帧放进 nextFrame（只留最新），
   * 解完接着画 —— 不排队。排队的话，解码一旦慢于推流，队伍只会越来越长，
   * 画面对操作的延迟跟着一路涨。
   */
  const paint = useCallback((f: BrowserFrame) => {
    painting.current = true;
    createImageBitmap(new Blob([f.data], { type: "image/jpeg" }))
      .then((bmp) => {
        const canvas = canvasRef.current;
        if (canvas) {
          // 改尺寸会清空画布，所以只在真的变了的时候改。
          if (canvas.width !== bmp.width || canvas.height !== bmp.height) {
            canvas.width = bmp.width;
            canvas.height = bmp.height;
          }
          canvas.getContext("2d")?.drawImage(bmp, 0, 0);
          frameSize.current = { w: f.width, h: f.height };
        }
        bmp.close();
      })
      .catch(() => {
        // 坏一帧就丢一帧。下一帧马上就到，报错没人能做什么。
      })
      .finally(() => {
        painting.current = false;
        const next = nextFrame.current;
        nextFrame.current = null;
        if (next) paint(next);
      });
  }, []);

  const onFrame = useCallback(
    (f: BrowserFrame) => {
      setHasFrame(true); // 同值 setState 会被 React 跳过，不会每帧重渲染
      if (painting.current) {
        nextFrame.current = f;
      } else {
        paint(f);
      }
    },
    [paint],
  );

  useEffect(() => {
    // 第三个参数让标签栏在浏览器就绪的那一刻就填上，不用等下一次定时同步 ——
    // 浏览器起来本身就要一秒，再叠一个轮询间隔就是两秒的空白。
    const sub = openBrowser(sessionId, onFrame, apply);
    return () => {
      sub.unsubscribe();
      // 只停宿主的 JPEG 编码 —— 没人看的时候继续推是白烧 CPU。
      // 浏览器进程和标签页都留着：收起面板 ≠ 关浏览器，切去看一眼
      // 改动再切回来，页面还是原样。
      void closeBrowser(sessionId);
    };
  }, [sessionId, apply, onFrame]);

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
   *
   * Web 模式下宽度不跟面板，钉在 WEB_WIDTH 上；高度按面板比例折算，
   * 帧和画面区同比例，contain 缩放后正好铺满、不留边。高度有上下限
   * （见 WEB_MAX_HEIGHT 的说明），顶到限之后比例对不上的部分由
   * contain 留边，坐标换算在 toPage 里按内容矩形做，不受影响。
   */
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;

    let timer: number | undefined;
    let last = "";
    const push = () => {
      const vw = view.clientWidth;
      const vh = view.clientHeight;
      // 刚挂上还没布局时量到的是 0。0 尺寸的视口在 CEF 那边等同于"看不见"，
      // 从此不再出帧 —— 画面会停在最后一帧，而且不报错。
      if (vw < MIN_VIEWPORT || vh < MIN_VIEWPORT) return;
      const scale = window.devicePixelRatio || 1;
      const web = viewMode === "web";
      const width = web ? WEB_WIDTH : vw;
      const height = web
        ? Math.min(
            WEB_MAX_HEIGHT,
            Math.max(WEB_MIN_HEIGHT, Math.round((vh / vw) * WEB_WIDTH)),
          )
        : vh;
      const key = `${width}x${height}@${scale}`;
      if (key === last) return;
      last = key;
      void browserResize(sessionId, width, height, scale, vw, vh).catch(() => {});
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
  }, [sessionId, viewMode]);

  /**
   * DOM 坐标 → 页面坐标。
   *
   * `[约束]` 必须按帧的真实尺寸换算，不能直接用鼠标事件的 offsetX/Y。
   * 画面是等比缩放后铺在面板里的，两者的比例通常不是 1 —— 不换算的话
   * 点击会系统性地偏，而且窗口越窄偏得越多。
   *
   * `[约束]` 换算的参照系是 contain 之后的**内容矩形**，不是 img 的元素框。
   * 两者在帧和面板同比例时重合（自适应模式的常态），但 Web 模式下帧是
   * 桌面比例、面板是竖条，contain 会居中留边 —— 按元素框算的话，留边
   * 越宽点击偏得越多。
   */
  const toPage = useCallback((e: React.MouseEvent): { x: number; y: number; s: number } | null => {
    const canvas = canvasRef.current;
    const size = frameSize.current;
    if (!canvas || !size || !size.w || !size.h) return null;
    const r = canvas.getBoundingClientRect();
    if (r.width === 0 || r.height === 0) return null;
    // 比例用 CSS 尺寸算。canvas 的位图是物理像素（CSS × 密度），
    // 两者同比，而页面坐标要的是 CSS 像素。
    const s = Math.min(r.width / size.w, r.height / size.h);
    const left = r.left + (r.width - size.w * s) / 2;
    const top = r.top + (r.height - size.h * s) / 2;
    const x = (e.clientX - left) / s;
    const y = (e.clientY - top) / s;
    // 点在留边上不算点在页面里。硬夹回页面范围的话，点黑边会命中
    // 页面边缘的元素 —— 用户明明什么都没点到，页面却有反应。
    if (x < 0 || y < 0 || x > size.w || y > size.h) return null;
    return { x, y, s };
  }, []);

  const send = useCallback(
    (input: BrowserInput) => void browserInput(sessionId, input).catch(() => {}),
    [sessionId],
  );

  /**
   * 攒着还没发的移动/滚轮。
   *
   * `[约束]` 这两类不能逐事件发。触控板一秒出上百个事件，每个都是一次
   * IPC 往返，和帧传输挤同一条主线程 —— 滚得越快画面越卡，正好和用户的
   * 预期相反。节流的形状是"起手立发、帧内合并"（见 scheduleFlush）:
   * 滚轮增量累加、位置取最新，页面收到的滚动总量一点不少。
   * 按下/抬起/按键不合并 —— 它们是离散动作，丢一个语义就变了。
   */
  const pending = useRef<{
    move?: { x: number; y: number };
    scroll?: { x: number; y: number; deltaX: number; deltaY: number };
  }>({});
  const flushTimer = useRef<number | undefined>(undefined);

  /**
   * 把攒着的立刻发出去。
   *
   * `[约束]` 按下/抬起之前必须调它。页面看到的顺序得是"移到那儿、再按下"
   * —— 攒着的移动排在按下后面的话，拖拽的起点就偏了。
   */
  const flushInputs = useCallback(() => {
    if (flushTimer.current !== undefined) {
      cancelAnimationFrame(flushTimer.current);
      flushTimer.current = undefined;
    }
    const p = pending.current;
    pending.current = {};
    if (p.move) send({ kind: "move", x: p.move.x, y: p.move.y });
    if (p.scroll) send({ kind: "scroll", ...p.scroll });
  }, [send]);

  const scheduleFlush = useCallback(() => {
    // 帧内的后续事件只累积，等尾巴上的 rAF 一起走。
    if (flushTimer.current !== undefined) return;
    // 一段手势的**第一个**事件立刻发,不等 rAF —— 攒到下一帧平白多
    // 0~16ms,而"跟手"恰恰取决于手指刚动那一下画面多快响应。代价是
    // 持续滚动时每帧最多两次 IPC(头一次+尾一次),量级无碍。
    flushInputs();
    flushTimer.current = requestAnimationFrame(() => {
      flushTimer.current = undefined;
      flushInputs();
    });
  }, [flushInputs]);

  // 卸载时把没发完的丢掉 —— 面板都没了，页面不需要最后那半下滚动。
  useEffect(
    () => () => {
      if (flushTimer.current !== undefined) cancelAnimationFrame(flushTimer.current);
    },
    [],
  );

  const go = async () => {
    const url = normalize(address);
    if (!url) return;
    // 先把补全后的地址显示出来。之后交给同步 —— 用户输的是意图，该显示的
    // 是真的落在哪儿（重定向之后的地址、跳转到的登录页）。
    setAddress(url);
    setBusy(true);
    setNavError("");
    try {
      await browserNavigate(sessionId, url);
    } catch (e) {
      // DNS 错、拒绝连接 —— 画面停在原地，不给一行原因用户会以为是自己的问题。
      setNavError(`打不开：${String(e)}`);
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
  /** 画面该不该露出来。没画面、或停在空白页（且不在导航中）时给空状态。 */
  const showFrame = hasFrame && (Boolean(active?.url) || busy);
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
              tabIndex={0}
              aria-label="关闭标签页"
              onClick={(e) => {
                // 不冒泡给外层的"切到这一页" —— 否则关掉的同时又切了过去。
                e.stopPropagation();
                void browserCloseTab(sessionId, t.id).then(closed).catch(() => {});
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  e.stopPropagation();
                  void browserCloseTab(sessionId, t.id).then(closed).catch(() => {});
                }
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
        {/* 收起 ≠ 关闭：浏览器进程和标签页都留着，再点开还是原样 */}
        <button className="icon" onClick={onClose} title="收起面板（页面保留）">
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
          {/* 加载中转个圈 —— 慢站上除此之外画面没有任何"在加载"的迹象 */}
          {busy ? <span className="browser-spinner" aria-label="加载中" /> : null}
          <input
            ref={addressRef}
            value={address}
            onChange={(e) => setAddress(e.target.value)}
            onFocus={(e) => e.currentTarget.select()}
            onCompositionStart={() => {
              addressIme.current = true;
            }}
            onCompositionEnd={() => {
              // compositionend 与确认用的 Enter 可能跨到下一个宏任务，
              // microtask 不够，用 setTimeout(0) 盖住这一拍。
              setTimeout(() => {
                addressIme.current = false;
              }, 0);
            }}
            onKeyDown={(e) => {
              // 229 = IME 处理中的占位 keyCode，部分 WebView 上比 isComposing 更准
              const composing =
                e.nativeEvent.isComposing || e.keyCode === 229 || addressIme.current;
              // 组字中的回车是上屏（拼音/候选），不是导航。
              if (e.key === "Enter" && !composing) {
                // 回车之后把焦点交出去。地址栏这才重新开始跟着页面走，而键盘
                // 也回到页面上 —— 打完地址接着就能在页面里打字。
                void go();
                e.currentTarget.blur();
              }
              // Esc 放弃这次编辑：立即把真实地址填回来（不等下一拍轮询），
              // 再失焦 —— 否则这一秒里地址栏显示的是打了一半的"假地址"。
              // 组字中的 Esc 是取消候选，别连地址一起清掉。
              if (e.key === "Escape" && !composing) {
                setAddress(active?.url ?? "");
                e.currentTarget.blur();
              }
            }}
            // 启动期导航会静默失败 —— 与其让人输完地址没反应，不如先禁掉
            disabled={starting}
            placeholder={starting ? "浏览器启动中…" : "输入 URL"}
            aria-label="地址栏"
            spellCheck={false}
          />
          <button
            className="icon"
            onClick={() => void go()}
            disabled={starting || busy || !address.trim()}
            title="打开"
          >
            <OpenIcon />
          </button>
        </div>
        {/*
         * 视口模式。挨着地址栏 —— 它决定"页面以多宽渲染"，和导航一族。
         * 用图标不用文字:面板可以窄到 320px，见上面三个导航键的说明。
         */}
        <div className="browser-mode" role="group" aria-label="视口模式">
          <button
            className={viewMode === "fit" ? "icon active" : "icon"}
            aria-pressed={viewMode === "fit"}
            onClick={() => switchMode("fit")}
            title="自适应：页面按面板宽度渲染"
          >
            <FitIcon />
          </button>
          <button
            className={viewMode === "web" ? "icon active" : "icon"}
            aria-pressed={viewMode === "web"}
            onClick={() => switchMode("web")}
            title={`Web：按 ${WEB_WIDTH}px 桌面宽度渲染，整体缩放进面板`}
          >
            <WebIcon />
          </button>
        </div>
      </div>
      {navError ? <div className="browser-nav-error">{navError}</div> : null}

      <div
        className="browser-view"
        ref={viewRef}
        onMouseDown={(e) => {
          // `[约束]` 必须拦掉 mousedown 的默认行为。默认行为是"把焦点移给
          // 被点的元素"，而画面区不可聚焦 —— WebKit（Tauri 在 macOS 上的
          // 引擎）会在处理器返回之后把焦点清到 body，正好把下面那句 focus()
          // 的结果抹掉。表现是"页面里点了输入框却一个字都打不进去"，而且
          // 只在 WKWebView 里发生，Chromium 系（含 dev 里常用来对照的
          // Chrome）会尊重处理器里刚设的焦点，怎么测都测不出来。
          e.preventDefault();
          // 转发原生 down（带 button 和 clickCount）而不是等 onClick 合成 ——
          // 页面里选字、拖滑块、双击选词、三击选段全靠这条真实的按下序列。
          const p = toPage(e);
          if (p) {
            flushInputs(); // 攒着的移动要排在按下前面，拖拽起点才不偏
            send({
              kind: "down",
              x: p.x,
              y: p.y,
              button: mouseButton(e.button),
              clickCount: e.detail || 1,
            });
          }
          // 焦点交给下面那个隐藏的 textarea，键盘的事都由它接。
          const r = viewRef.current?.getBoundingClientRect();
          if (r) setCaret({ x: e.clientX - r.left, y: e.clientY - r.top });
          imeRef.current?.focus();
        }}
        onMouseUp={(e) => {
          const p = toPage(e);
          if (p) {
            flushInputs(); // 拖拽的最后一段移动要先落地，抬起的位置才对
            send({
              kind: "up",
              x: p.x,
              y: p.y,
              button: mouseButton(e.button),
              clickCount: e.detail || 1,
            });
          }
        }}
        onContextMenu={(e) => {
          // 右键已经作为 down/up 转发给页面了。这里拦掉外壳 webview 自己的
          // 上下文菜单，否则会弹出 Tauri 的菜单穿帮。
          e.preventDefault();
        }}
        onMouseMove={(e) => {
          const p = toPage(e);
          if (p) {
            pending.current.move = { x: p.x, y: p.y }; // 只留最新位置
            scheduleFlush();
          }
        }}
        onWheel={(e) => {
          // 两个轴都转发，不自己判断方向。macOS 上按住 shift 滚轮时
          // 系统已经把量放进了 deltaX，这里再换一次就换回去了。
          const p = toPage(e as unknown as React.MouseEvent);
          if (p) {
            const prev = pending.current.scroll;
            // `[约束]` 滚轮增量必须按 contain 缩放折到页面坐标。
            // 事件给的是面板像素；Web 模式页面按 1280 渲染再缩小塞进
            // 面板，s 常是 0.3~0.5。原样转发的话，手指滑 100px，页面
            // 只走 100 个 CSS 像素，缩回去只动三四十像素 —— 自适应
            // 看着跟手，Web 模式整段慢半拍，像卡。除以 s 让"滑多少
            // 画面走多少"。自适应下 s≈1，这条是空操作。
            pending.current.scroll = {
              x: p.x,
              y: p.y,
              deltaX: (prev?.deltaX ?? 0) + e.deltaX / p.s,
              deltaY: (prev?.deltaY ?? 0) + e.deltaY / p.s,
            };
            scheduleFlush();
          }
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
          aria-label="页面键盘输入"
          style={{ left: caret.x, top: caret.y }}
          // 输入法自己有候选和纠错，浏览器再插一手只会打架。
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          onPaste={(e) => {
            // 粘贴不能走 onInput：InputEvent.data 对粘贴常是 null，
            // 整段剪贴板内容会一声不响地丢掉。这里显式读出来发走，
            // 再拦掉默认行为，防止 onInput 那条路再来一遍造成重复。
            const text = e.clipboardData.getData("text/plain");
            e.preventDefault();
            if (text) send({ kind: "text", text });
          }}
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
         * 画面画在这块 canvas 上（见 paint 的说明）。
         *
         * `[约束]` canvas 要一直挂着，藏是用 visibility 藏。卸掉再挂回来
         * 画布是空的，而静止页面不会再推新帧 —— 空白要一直留到用户碰一下
         * 页面为止。留着的话，最后一帧还在画布里，切回来立刻就能看。
         */}
        <canvas
          ref={canvasRef}
          style={{ visibility: showFrame ? "visible" : "hidden" }}
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
        {showFrame ? null : (
          <div className="browser-empty">
            {hasFrame ? (
              <>
                <GlobeIcon size={30} />
                <p className="browser-empty-title">开始浏览</p>
                <p className="hint">输入网址，与模型同看。</p>
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

/** 视口模式:自适应＝跟着面板尺寸走，Web＝按桌面宽度渲染再缩放显示。 */
type ViewMode = "fit" | "web";

/** 视口模式存这里。跨会话共享 —— 这是人的偏好，不是某个会话的状态。 */
const VIEW_MODE_KEY = "riot.browser.viewMode";

/**
 * Web 模式的渲染宽度（CSS 像素）。
 *
 * 1280 是桌面站点最普遍的设计宽度:比它窄的视口常被响应式断点切进
 * 平板/移动布局 —— 那正是这个模式要避开的。模型整页截图的宽度就是
 * 当前视口宽（见宿主 ops::screenshot），所以这个数同时决定了模型
 * 眼里"桌面版"的宽度。
 */
const WEB_WIDTH = 1280;

/**
 * Web 模式视口高度的上下限（CSS 像素）。
 *
 * 高度跟着面板比例折算，好让帧与画面区同比例、缩放后正好铺满。但渲染
 * 表面是 宽×高×密度 的位图:竖条面板能折算出几千像素的高度，内存和
 * 每一帧的 JPEG 编码都跟着翻倍，还会顶穿 screencast 的物理像素上限
 * （见宿主 start_screencast，Retina 下 1500×2 正好贴着 3000）。顶到
 * 限之后比例对不上的部分由 contain 留边，点击换算见 toPage。
 */
const WEB_MIN_HEIGHT = 600;
const WEB_MAX_HEIGHT = 1500;

/** DOM 的 MouseEvent.button（0/1/2）→ CDP 认的名字。 */
function mouseButton(b: number): string {
  return b === 2 ? "right" : b === 1 ? "middle" : "left";
}

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

/** 自适应模式:两边的箭头把页面撑到面板边上。 */
function FitIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M2.5 3.5v9M13.5 3.5v9"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
      />
      <path
        d="M5 8h6M6.8 6.2 5 8l1.8 1.8M9.2 6.2 11 8l-1.8 1.8"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/** Web 模式:一台桌面显示器。 */
function WebIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <rect
        x="1.5"
        y="3"
        width="13"
        height="8.5"
        rx="1.5"
        stroke="currentColor"
        strokeWidth="1.3"
      />
      <path
        d="M5.5 13.5h5M8 11.5v2"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
      />
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
