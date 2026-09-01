/**
 * 对话流：消息列表、会话内查找、贴底跟随，以及消息行的渲染。
 *
 * 从 App.tsx 拆出的独立职责。滚动语义（倒排容器的距底坐标、非对称
 * 迟滞、程序化滚动 pinning、锚点补偿、每会话位置缓存）全在这里 ——
 * 改贴底/恢复行为只动这个文件。
 *
 * 滚动容器是 column-reverse 的倒排容器（学 Codex 桌面端）：scrollTop
 * 贴底为 0、向上翻为负，浏览器保持的是"距底距离"。于是贴底跟随免费、
 * 视口上方的行懒水合长高不再撼动画面；剩下要自己管的是翻历史时下方
 * 流式长高（锚点补偿）。取舍见 styles.css 的 .transcript 注释。
 * 渲染预算的三层（懒解析 / memo / content-visibility）见
 * ARCHITECTURE §13.1。
 */

import {
  Fragment,
  memo,
  type ReactNode,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import type { PermissionAsk, PermissionResponse } from "../bridge";
import { useImeGuard } from "../hooks/useImeGuard";
import type { Item, TextItem } from "../hooks/useSession";
import {
  caretToEnd,
  handleChipKey,
  normalizePads,
  readEditor,
  writeEditor,
} from "../lib/chipEditor";
import {
  SLASH_HEAD_RE,
  extractElemSpans,
  extractMentionSpans,
  mentionCovers,
  promptToSegs,
  segsToPrompt,
} from "../lib/promptText";
import { Chip, FileChip } from "./Chip";
import { ConfirmDialog, type ConfirmRequest } from "./ConfirmDialog";
import { LazyMarkdown, Markdown } from "./Markdown";
import { AskChoiceCard, PlanApprovalCard, PlanDraft } from "./PermissionDialog";
import { groupBlocks, ProcessGroup, ThinkingBlock } from "./ProcessFold";
import { ShotViewer, ToolCard } from "./ToolCard";

/**
 * 对话流滚到哪、跟不跟随底部。top 存原始 scrollTop —— 倒排容器里
 * 贴底为 0、向上翻为负。
 *
 * 正式包 WKWebView 一给面板加 `visibility:hidden` / 改 `position`，
 * 会把 scrollTop 清成 0 并冒一次 scroll（倒排后 0 是底部，症状从
 * "切回来跳到顶"变成"跳回底部"，机制相同）。dev 的 WebView 常常不
 * 这么做，所以只有打包后才复现。记的必须是用户还看得见时的位置，
 * 隐藏之后那次假滚动一律丢掉。
 */
export const transcriptView = new Map<string, { top: number; stick: boolean }>();

/**
 * 距底距离。倒排容器的滚动原点在底部：scrollTop 贴底为 0、向上翻为
 * 负，取负即距底。clamp 到 0 —— 底部橡皮筋回弹时 scrollTop 会短暂
 * 冲成正值，那不算"离开了底部"。
 */
const distFromBottom = (box: HTMLElement) => Math.max(0, -box.scrollTop);

/**
 * 这次滚轮会不会被目标和对话流之间的子层截住。
 *
 * 只有这种时候才自己写 scrollTop：一律 preventDefault 会关掉系统
 * 平滑 / 触控板惯性，中间对话就一格一格跳，左边会话栏却是原生的。
 * overflow:clip 不是滚动容器，不必接手。overflow:auto 没竖向溢出
 * 时也不接手（overflow-x:auto 会把 overflow-y 算成 auto）。
 */
function wheelNeedsHijack(start: EventTarget | null, box: HTMLElement, deltaY: number): boolean {
  let el: HTMLElement | null =
    start instanceof HTMLElement ? start : start instanceof Element ? start.parentElement : null;
  while (el && el !== box) {
    const oy = getComputedStyle(el).overflowY;
    if (oy === "auto" || oy === "scroll" || oy === "overlay") {
      const max = el.scrollHeight - el.clientHeight;
      if (max > 1) {
        const goingUp = deltaY < 0;
        const atTop = el.scrollTop <= 1;
        const atBottom = el.scrollTop >= max - 1;
        if ((goingUp && atTop) || (!goingUp && atBottom)) return true;
        return false;
      }
    } else if (oy === "hidden") {
      return true;
    }
    el = el.parentElement;
  }
  return false;
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
  const ime = useImeGuard();

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

  // 关闭（含卸载）时清掉高亮，别在页面上留一堆黄块。
  // clear 不能进依赖：它是每次渲染新建的函数，进去之后每渲染一次就跑一遍
  // cleanup —— 也就是每敲一个字都把刚画上的高亮擦掉。
  // eslint-disable-next-line react-hooks/exhaustive-deps
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
          onCompositionStart={ime.onCompositionStart}
          onCompositionEnd={ime.onCompositionEnd}
          onKeyDown={(e) => {
            // 组字中的回车是确认候选、Esc 是收候选列表 —— 都不该动查找条，
            // 尤其 Esc：那会把整个查找关掉，用户只是想撤掉一个选词框。
            if (ime.isComposing(e)) return;
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

export function Transcript({
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
  onEditEntry,
  onDeleteEntry,
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
  /** 上下文编辑：把这条气泡的文本换掉。false = 没改成，编辑框保留草稿。 */
  onEditEntry?: (item: TextItem, text: string) => Promise<boolean>;
  /** 上下文删除：把这条气泡从历史里抹掉。 */
  onDeleteEntry?: (item: TextItem) => Promise<boolean>;
}) {
  const boxRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  /** 程序化贴底时挡住 onScroll，免得自己把 stick 打成 false。 */
  const pinning = useRef(false);
  /**
   * 向上翻看时的滚动锚点：视口里第一个顶边完整可见的块 + 它到视口顶的距离。
   *
   * 倒排容器把"上方长高"消化掉了（懒水合落高度不动视口），剩下会动
   * 视口的是**下方**长高：用户翻着历史时流式输出还在底部追加，浏览器
   * 保持"距底距离"，正读的内容就往下漂。WKWebView 没有原生滚动锚定
   * （overflow-anchor 到 Safari 27 才有）—— 自己记住「正看着哪个块」，
   * 高度变了把 scrollTop 补回去（补偿在下面的 ResizeObserver 里）。
   * 贴底时用不上，锚点清空。
   *
   * 基线只在**用户**滚动时重记，补偿性滚动绝不重记 —— 引擎会把
   * scrollTop 写入取整，补完重记等于把取整误差吸进新基线，永不回正；
   * 反复开合折叠每轮攒下几像素，页面就一点点爬（WebKit 每轮 5~7px，
   * Chromium 也有）。对着原始基线做伺服，误差不再累积。
   */
  const anchor = useRef<{ el: Element; top: number } | null>(null);
  /** 补偿写入后 scrollTop 的回读值。scroll 事件里等值命中 = 补偿滚动。 */
  const expectedTop = useRef<number | null>(null);
  /**
   * 亚像素残差的 transform 修正量。引擎把 scrollTop 取整到整数 CSS
   * 像素（WKWebView 还朝零截断），光靠滚动补偿必然留下 <1px 的绘制
   * 抖动 —— 折叠动画的每一帧余数都不同，上方内容就闪。整数部分走
   * scrollTop，余数落到 thread-col 的 translateY（合成层支持亚像素），
   * 开合时上方内容纹丝不动。列里的全屏查看器都是 portal 到 body 的，
   * 这个 transform 不会劫持它们的 fixed 定位。
   */
  const colShift = useRef(0);
  const setColShift = (v: number) => {
    colShift.current = v;
    const col = boxRef.current?.querySelector<HTMLElement>(".thread-col");
    if (col) col.style.transform = v ? `translateY(${v}px)` : "";
  };
  // 渲染期就写：正式包隐藏面板时 scroll 发生在 commit 里，
  // effect 还没跑，闭包里的 armed 仍是 true，会把清零后的 0 记进去。
  const armedRef = useRef(armed);
  armedRef.current = armed;

  const rememberView = (box: HTMLElement) => {
    if (!armedRef.current) return;
    transcriptView.set(sessionId, { top: box.scrollTop, stick: stick.current });
  };

  const captureAnchor = () => {
    // 重记基线前清掉残差修正：基线要取自然几何。清零带来的 ≤0.5px
    // 位移发生在用户正在滚动的时刻，动势掩住了它。
    if (colShift.current !== 0) setColShift(0);
    if (stick.current) {
      anchor.current = null;
      return;
    }
    const box = boxRef.current;
    const col = box?.querySelector(".thread-col");
    if (!box || !col || !col.children.length) {
      anchor.current = null;
      return;
    }
    const kids = col.children;
    const boxTop = box.getBoundingClientRect().top;
    // 二分找第一个底边越过视口顶的块。块按文档序从上到下单调排列，
    // 长会话逐块量 getBoundingClientRect 太贵 —— 这里每次滚动都要跑。
    let lo = 0;
    let hi = kids.length - 1;
    let first = kids.length - 1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      const el = kids[mid];
      if (!el) break;
      if (el.getBoundingClientRect().bottom > boxTop) {
        first = mid;
        hi = mid - 1;
      } else {
        lo = mid + 1;
      }
    }
    let el = kids[first];
    if (!el) {
      anchor.current = null;
      return;
    }
    // 跨着视口顶边的块锚不住：它水合长高时顶边不动、内部整体重排，
    // 补偿无从算起。锚下一个块（顶边在视口内的）—— 除非它一块占满全屏。
    if (el.getBoundingClientRect().top < boxTop - 1) {
      const next = kids[first + 1];
      if (next && next.getBoundingClientRect().top - boxTop < box.clientHeight) el = next;
    }
    anchor.current = { el, top: el.getBoundingClientRect().top - boxTop };
  };

  /** 离底超过一屏时浮现「回到底部」按钮。 */
  const [awayFromBottom, setAwayFromBottom] = useState(false);
  /** ⌘F 查找条。长对话找不到历史内容是真实痛点。 */
  const [findOpen, setFindOpen] = useState(false);
  /** 待确认的上下文删除。删的是一整轮，动手前必须看得见后果。 */
  const [confirmDel, setConfirmDel] = useState<ConfirmRequest | null>(null);

  /** 点删除按钮 → 弹确认框，确认后才真删。 */
  const requestDelete = useCallback(
    (item: TextItem) => {
      if (!onDeleteEntry) return;
      setConfirmDel({
        title: "删除这一轮问答",
        body:
          "这条消息所属的提问，连同它引出的全部回应（回复、工具调用），" +
          "会一起从上下文中删除，之后的对话不再受这一轮影响。",
        confirmLabel: "删除",
        action: () => void onDeleteEntry(item),
      });
    },
    [onDeleteEntry],
  );

  const pinBottom = () => {
    const box = boxRef.current;
    if (!box) return;
    pinning.current = true;
    // 倒排容器的底部就是滚动原点。
    box.scrollTop = 0;
    stick.current = true;
    anchor.current = null;
    if (colShift.current !== 0) setColShift(0);
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
    captureAnchor();
    setAwayFromBottom(distFromBottom(box) > box.clientHeight);
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
      // 锚点补偿自己冒的 scroll：不改 stick、更不重记锚点基线（见
      // anchor 的注释）。等值不命中说明同帧还叠着用户滚动，按用户算。
      const compScroll = expectedTop.current !== null && top === expectedTop.current;
      expectedTop.current = null;
      if (compScroll) return;
      // 隐藏那一帧正式包会把 scrollTop 打成 0。armed 在渲染时已是
      // false，这次滚动不是用户翻的，不能写进缓存、也不能改 stick。
      if (!armedRef.current) return;
      if (pinning.current) return;
      const dfb = distFromBottom(box);
      if (delta < 0 && dfb > 1) {
        // 倒排坐标里向上翻 = scrollTop 变负，和正排同号。dfb > 1 挡掉
        // 底部橡皮筋回弹：过冲弹回时 scrollTop 也在变小，但那不是
        // "想往上翻"。
        stick.current = false;
      } else if (delta > 0 && dfb < 24) {
        // 只有自己滚回贴底才恢复跟随。阈值收窄到约一行 —— 停在离底
        // 几十像素处阅读时，跟随不该被抢回去。
        stick.current = true;
      }
      rememberView(box);
      setAwayFromBottom(dfb > box.clientHeight);
      captureAnchor();
    };
    box.addEventListener("scroll", onScroll, { passive: true });
    return () => box.removeEventListener("scroll", onScroll);
    // rememberView / captureAnchor 每次渲染都是新函数，但它们只读 ref。
    // 放进依赖等于流式输出时每帧解绑重绑一次 scroll 监听，白付开销。
    // 监听的生命周期就该跟着会话走。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  // 只在子层会吞掉滚轮时才接手。平时交给浏览器，才能跟左边会话栏
  // 一样走系统平滑和触控板惯性。passive:false 才能 preventDefault。
  useEffect(() => {
    const box = boxRef.current;
    if (!box) return;
    const onWheel = (e: WheelEvent) => {
      if (e.defaultPrevented) return;
      if (Math.abs(e.deltaX) > Math.abs(e.deltaY)) return;
      if (!wheelNeedsHijack(e.target, box, e.deltaY)) return;
      if (box.scrollHeight - box.clientHeight < 2) return;
      let dy = e.deltaY;
      if (e.deltaMode === WheelEvent.DOM_DELTA_LINE) dy *= 16;
      else if (e.deltaMode === WheelEvent.DOM_DELTA_PAGE) dy *= box.clientHeight;
      box.scrollTop += dy;
      e.preventDefault();
    };
    box.addEventListener("wheel", onWheel, { passive: false });
    return () => box.removeEventListener("wheel", onWheel);
  }, []);

  // 切回来把位置还回去。layout 一次不够：正式包揭开 visibility
  // 之后还要再排一次，第二帧再写才能压住 WebKit 的清零。
  useLayoutEffect(() => {
    if (!armed) return;
    restoreView();
    const again = requestAnimationFrame(() => restoreView());
    return () => cancelAnimationFrame(again);
    // `[约束]` restoreView 绝不能进依赖。它每次渲染都是新引用，进去之后
    // 这个 effect 每渲染一次就把滚动位置写回缓存值一次 —— 流式输出期间
    // 等于每帧把用户拽回上次记录的位置，页面再也滚不动。
    // 位置恢复只该发生在"切回来"这一刻，也就是 armed / sessionId 变化时。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [armed, sessionId]);

  // 内容高度一变（流式追加、懒水合落地、图片 / mermaid 出图）就到这里
  // 校正。倒排容器把贴底和"上方长高"都交给了浏览器，这里剩两件事。
  useEffect(() => {
    const box = boxRef.current;
    const col = box?.querySelector(".thread-col");
    if (!box || !col) return;
    const ro = new ResizeObserver(() => {
      if (stick.current) {
        // 贴底本是免费的（scrollTop 恒为 0 指着底部），pinBottom 只是
        // 把补偿期间可能攒下的几像素残差归零，顺手收掉回程按钮。
        pinBottom();
        return;
      }
      // 翻着历史时会动视口的只剩**下方**长高：流式输出在底部追加、
      // 折叠组在视口内开合，浏览器保持"距底距离"，正读的内容就漂 ——
      // 把锚点块拉回原位。上方长高（懒水合）倒排后天然不动视口，那时
      // dy 恰好是 0。ResizeObserver 回调在 layout 之后、paint 之前跑，
      // 这里补写 scrollTop 用户看不到中间态。写完回读进 expectedTop，
      // 让 onScroll 认出这次滚动不是用户翻的；基线不重记（见 anchor
      // 注释）。pinning 帧（restoreView 正在二次补写）不掺和；隐藏的
      // 保活面板（!armed）量出来的是假布局，也不动。
      const a = anchor.current;
      if (a && a.el.isConnected && armedRef.current && !pinning.current) {
        // dy 是布局几何的偏差（去掉 transform 修正），伺服对它做；
        // dy + colShift 是画面上的偏差，决定这一帧要不要动手。
        const rawTop =
          a.el.getBoundingClientRect().top - box.getBoundingClientRect().top - colShift.current;
        const dy = rawTop - a.top;
        if (Math.abs(dy + colShift.current) > 0.05) {
          const desired = box.scrollTop + dy;
          box.scrollTop = Math.round(desired);
          expectedTop.current = box.scrollTop;
          setColShift(-(desired - box.scrollTop));
          rememberView(box);
        }
      }
      // 交出跟随后内容还在下面长，离底距离变大但不触发 scroll 事件 ——
      // 「回到底部」得靠这里浮出来，不然用户翻上去就找不到回程。
      setAwayFromBottom(distFromBottom(box) > box.clientHeight);
    });
    ro.observe(col);
    return () => ro.disconnect();
    // pinBottom / rememberView 同上：只读 ref 的新引用函数。放进依赖会让
    // ResizeObserver 每次渲染 disconnect 再重建，而它正是用来观测流式
    // 长高的 —— 每帧重建既贵又会丢掉观测基线。
    // eslint-disable-next-line react-hooks/exhaustive-deps
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
    // pinBottom 不进依赖：依赖列的是"什么变化该重新贴底"，那是内容本身。
    // 把每帧新建的函数混进去只会让这个列表失去表达力。
    // eslint-disable-next-line react-hooks/exhaustive-deps
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
    <div className="transcript-shell">
      <main className="transcript" ref={boxRef}>
        <div className="thread-col">
        {blocks.map((b, i) =>
          b.kind === "row" ? (
            <Row
              key={b.item.id}
              item={b.item}
              hydrate={findOpen || i >= hydrateFrom}
              regenEnabled={!busy}
              mutateEnabled={!busy}
              {...(onRegenerate ? { onRegenerate } : {})}
              {...(onEditEntry ? { onEditEntry } : {})}
              {...(onDeleteEntry ? { onDeleteEntry: requestDelete } : {})}
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
        {/* animated：正在流的这一条里，新长出来的块逐个淡入 —— token
            是成撮到的，不淡的话是一坨一坨往外蹦。落定成 Row 之后就是
            普通历史消息，不再带这个开关（见 styles.css 的
            .md[data-md-animated]）。 */}
        {streaming ? (
          <div className="msg assistant">
            <Markdown text={streaming} animated />
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
        {findOpen && armed ? <FindBar box={boxRef} onClose={() => setFindOpen(false)} /> : null}
      </main>
      {/* 删除确认放在滚动容器**外**、shell 内：main 是倒排滚动容器、
          内部 thread-col 带 transform，把 fixed 遮罩的包含块从视口改成了
          滚动区，弹窗会偏移错位。挂到 shell 层用 absolute 罩住聊天区域，
          既躲开那个包含块陷阱，又正好相对聊天区域居中（不盖侧栏）。 */}
      {confirmDel ? (
        <div className="transcript-confirm">
          <ConfirmDialog c={confirmDel} onClose={() => setConfirmDel(null)} />
        </div>
      ) : null}
      {/* 叠在滚动容器外面。倒排 flex 里 sticky + 负边距：WebKit 把按钮
          挤出视口（Mac 上看不见），Chromium 把它压扁（Windows 上不圆）。
          往上翻超过一屏才出现 —— 贴底时它只是噪音。 */}
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
    </div>
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
  onEditEntry,
  onDeleteEntry,
  mutateEnabled,
}: {
  item: Item;
  onRegenerate?: (itemId: string) => void;
  regenEnabled?: boolean;
  /** 贴底 / 查找中：立刻解析 markdown 和工具详情。 */
  hydrate?: boolean;
  /** 上下文编辑（见 Transcript 的同名 prop）。 */
  onEditEntry?: (item: TextItem, text: string) => Promise<boolean>;
  /** 上下文删除。收到的是 Transcript 的确认包装 —— 点击先弹确认框。 */
  onDeleteEntry?: (item: TextItem) => void;
  /** 编辑/删除此刻可用（空闲）。生成中改历史会和正在写的轮子打架。 */
  mutateEnabled?: boolean;
}) {
  // 编辑态挂在 Row 上（hooks 不能进 switch 分支），只有文本气泡用它。
  const [editing, setEditing] = useState(false);

  switch (item.kind) {
    case "user":
      if (editing && onEditEntry) {
        return (
          <div className="msg user editing">
            <MsgEditor
              initial={item.text}
              parseChips
              onCancel={() => setEditing(false)}
              onSave={async (text) => {
                const ok = await onEditEntry(item, text);
                if (ok) setEditing(false);
                return ok;
              }}
            />
          </div>
        );
      }
      // 用户输入按原文显示，不走 markdown —— 渲染会篡改他说的话。
      // 操作按钮排在气泡右下方的流内位置（不用绝对定位：thread-col 的
      // content-visibility 隐含 paint containment，定位出气泡边界会被裁掉）。
      return (
        <div className="user-row">
          <div className="msg user">
            {/* 自己附的图要看得见。不回显的话，发完之后附件条一清空，
                用户就再也确认不了刚才发出去的是哪张。 */}
            {item.images?.length ? (
              <div className="msg-images">
                {item.images.map((src, i) => (
                  <UserImage key={i} src={src} />
                ))}
              </div>
            ) : null}
            <UserText text={item.text} {...(item.files ? { files: item.files } : {})} />
          </div>
          <MsgActions
            text={item.text}
            mutateEnabled={!!mutateEnabled}
            {...(item.at ? { at: item.at } : {})}
            {...(onEditEntry ? { onEdit: () => setEditing(true) } : {})}
            {...(onDeleteEntry ? { onDelete: () => onDeleteEntry(item) } : {})}
          />
        </div>
      );
    case "assistant":
      if (editing && onEditEntry) {
        return (
          <div className="msg assistant editing">
            <MsgEditor
              initial={item.text}
              onCancel={() => setEditing(false)}
              onSave={async (text) => {
                const ok = await onEditEntry(item, text);
                if (ok) setEditing(false);
                return ok;
              }}
            />
          </div>
        );
      }
      return (
        <div className="msg assistant">
          <LazyMarkdown text={item.text} eager={!!hydrate} />
          {/* 半截话得说明白它为什么半截 —— 不标的话，用户过一会儿回来
              看到的是一句戛然而止的回答，分不清是自己停的还是模型崩了。 */}
          {item.stopped ? <div className="msg-stopped">已停止生成</div> : null}
          <MsgActions
            text={item.text}
            regenEnabled={!!regenEnabled && !!onRegenerate}
            mutateEnabled={!!mutateEnabled}
            {...(item.at ? { at: item.at } : {})}
            {...(onRegenerate ? { onRegenerate: () => onRegenerate(item.id) } : {})}
            {...(onEditEntry ? { onEdit: () => setEditing(true) } : {})}
            {...(onDeleteEntry ? { onDelete: () => onDeleteEntry(item) } : {})}
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

/**
 * 气泡上的时刻只到分钟。
 *
 * 秒对读对话没有意义，而且这个格式化器是模块级单例 —— 每条消息每帧
 * 现造一个 Intl 实例，流式输出时是几百次没有产出的构造。
 */
const HHMM = new Intl.DateTimeFormat(undefined, {
  hour: "2-digit",
  minute: "2-digit",
  hour12: false,
});

/** hover 提示给完整日期：长会话里光看"15:18"分不出是哪天。 */
const FULL_STAMP = new Intl.DateTimeFormat(undefined, {
  dateStyle: "long",
  timeStyle: "medium",
});

/** 悬停出现的消息操作：复制 / 重新生成 / 上下文编辑 / 删除，末尾是时刻。
 *  占位始终在，hover 才可见。 */
function MsgActions({
  text,
  at,
  onRegenerate,
  regenEnabled,
  onEdit,
  onDelete,
  mutateEnabled,
}: {
  text: string;
  /** 消息产生的时刻（Unix 毫秒）。undefined = 老记录没有，不显示。 */
  at?: number;
  onRegenerate?: () => void;
  regenEnabled?: boolean;
  /** 进入编辑态。undefined = 这条不可编辑。 */
  onEdit?: () => void;
  /** 从上下文删除。undefined = 这条不可删。 */
  onDelete?: () => void;
  /** 编辑/删除此刻可用（空闲）。 */
  mutateEnabled?: boolean;
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
      {onEdit ? (
        <button
          type="button"
          className="msg-action"
          title={mutateEnabled ? "编辑（替换上下文里的原文）" : "生成中，结束后才能编辑"}
          aria-label="编辑消息"
          disabled={!mutateEnabled}
          onClick={onEdit}
        >
          <EditIcon />
        </button>
      ) : null}
      {onDelete ? (
        <button
          type="button"
          className="msg-action"
          title={
            mutateEnabled ? "删除这一轮问答（提问连同回复）" : "生成中，结束后才能删除"
          }
          aria-label="删除这一轮问答"
          disabled={!mutateEnabled}
          onClick={onDelete}
        >
          <TrashIcon />
        </button>
      ) : null}
      {at ? <MsgTime at={at} /> : null}
    </div>
  );
}

/** 消息时刻。跟在操作按钮后面，和它们一起 hover 出现。 */
function MsgTime({ at }: { at: number }) {
  const d = new Date(at);
  return (
    <time className="msg-time" dateTime={d.toISOString()} title={FULL_STAMP.format(d)}>
      {HHMM.format(d)}
    </time>
  );
}

/**
 * 消息的内联编辑框（上下文修改）。
 *
 * ⌘/Ctrl+Enter 保存、Esc 取消，和输入框同一套肌肉记忆。保存失败
 * （忙、消息已被压缩、内核拒绝）时编辑框留着 —— 草稿不能丢。
 *
 * 用户消息（`parseChips`）用和输入框同一套 contenteditable 块机械：
 * 文件引用、页面元素在编辑时也是色块，和气泡里看到的一致，而不是一串
 * 裸标记。助手消息保持纯文本 —— 回复里长得像 `@路径` 的字符串是内容，
 * 解析成块再序列化会改写它。
 */
function MsgEditor({
  initial,
  parseChips = false,
  onSave,
  onCancel,
}: {
  initial: string;
  parseChips?: boolean;
  onSave: (text: string) => Promise<boolean>;
  onCancel: () => void;
}) {
  const [saving, setSaving] = useState(false);
  const [hasText, setHasText] = useState(!!initial.trim());
  const ref = useRef<HTMLDivElement>(null);
  // 中文 IME：组字中不能动 DOM（normalize 合并文本节点会打断组字）。
  const imeRef = useRef(false);

  const read = () => (ref.current ? segsToPrompt(readEditor(ref.current)) : "");

  const refresh = () => {
    const el = ref.current;
    if (!el) return;
    if (!imeRef.current) normalizePads(el);
    setHasText(read().trim().length > 0);
  };

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    writeEditor(el, parseChips ? promptToSegs(initial) : [{ kind: "text", value: initial }]);
    el.focus();
    caretToEnd(el);
    // 只在挂载时灌一次：编辑区是非受控的，内容住在 DOM 里。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const save = async () => {
    const text = read().trim();
    if (saving || !text) return;
    setSaving(true);
    try {
      await onSave(text);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="msg-editor">
      <div
        ref={ref}
        className="msg-editbox"
        contentEditable={!saving}
        suppressContentEditableWarning
        role="textbox"
        aria-multiline="true"
        onInput={refresh}
        onCompositionStart={() => {
          imeRef.current = true;
        }}
        onCompositionEnd={() => {
          setTimeout(() => {
            imeRef.current = false;
          }, 0);
          refresh();
        }}
        // 和输入框一致：富文本粘贴一律降级成纯文本。
        onPaste={(e) => {
          e.preventDefault();
          document.execCommand("insertText", false, e.clipboardData.getData("text/plain"));
        }}
        onKeyDown={(e) => {
          if (
            ref.current &&
            !e.nativeEvent.isComposing &&
            !imeRef.current &&
            handleChipKey(e, ref.current)
          ) {
            e.preventDefault();
            refresh();
            return;
          }
          if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
            e.preventDefault();
            void save();
          } else if (e.key === "Escape" && !e.nativeEvent.isComposing) {
            e.preventDefault();
            e.stopPropagation();
            onCancel();
          }
        }}
      />
      <div className="msg-editor-btns">
        <span className="msg-editor-hint">保存后替换上下文里的原文，之后的对话按新内容走</span>
        <button type="button" onClick={onCancel} disabled={saving}>
          取消
        </button>
        <button
          type="button"
          className="msg-editor-save"
          onClick={() => void save()}
          disabled={saving || !hasText}
        >
          {saving ? "保存中…" : "保存"}
        </button>
      </div>
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

function EditIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M9.9 3.1l3 3L6.4 12.6l-3.6.6.6-3.6L9.9 3.1z"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinejoin="round"
      />
      <path d="M8.6 4.4l3 3" stroke="currentColor" strokeWidth="1.3" />
    </svg>
  );
}

function TrashIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M3 4.5h10M6.4 4.5V3.3a.8.8 0 0 1 .8-.8h1.6a.8.8 0 0 1 .8.8v1.2M4.4 4.5l.5 8.2a1 1 0 0 0 1 .95h4.2a1 1 0 0 0 1-.95l.5-8.2"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="M6.7 7.2v3.8M9.3 7.2v3.8"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
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
      out.push(<FileChip key={`${s.path}-${s.index}`} path={s.path} preview />);
      seen.add(s.path);
      last = s.index + s.length;
    }
    if (last < src.length) out.push(src.slice(last));
    const orphans = files.filter((f) => !mentionCovers(seen, f));
    return (
      <>
        {out}
        {orphans.map((p) => (
          <FileChip key={`orphan-${p}`} path={p} preview />
        ))}
      </>
    );
  };

  // 先把"页面元素"标记切出来画成绿色色块（和输入框里一致），标记之间的
  // 普通文本再交给 fileNodes 处理 `@文件` 引用。两层不会互相吞：元素标记
  // 用【】+反引号，文件用 `@`。
  const bodyNodes = (src: string): ReactNode => {
    const marks = extractElemSpans(src);
    if (marks.length === 0) return fileNodes(src);
    const out: React.ReactNode[] = [];
    let last = 0;
    marks.forEach((e, i) => {
      if (e.index > last) {
        out.push(<Fragment key={`t-${i}`}>{fileNodes(src.slice(last, e.index))}</Fragment>);
      }
      out.push(
        <Chip key={`el-${i}`} seg={{ kind: "elem", value: e.selector, label: e.label }} />,
      );
      last = e.index + e.length;
    });
    if (last < src.length) {
      out.push(<Fragment key="t-end">{fileNodes(src.slice(last))}</Fragment>);
    }
    return <>{out}</>;
  };

  if (!cmdName) {
    return <>{bodyNodes(body)}</>;
  }

  return (
    <>
      <Chip seg={{ kind: "cmd", value: cmdName }} />
      {body || files.length > 0 ? <> {bodyNodes(body)}</> : null}
    </>
  );
}

/** 用户消息里附的图。点击全屏放大 —— 附完图想核对细节是常事。 */
function UserImage({ src }: { src: string }) {
  const [viewer, setViewer] = useState(false);
  return (
    <>
      <button
        type="button"
        className="msg-image-btn"
        onClick={() => setViewer(true)}
        aria-label="放大查看图片"
      >
        <img src={src} alt="" />
      </button>
      {viewer ? <ShotViewer src={src} alt="消息附图" onClose={() => setViewer(false)} /> : null}
    </>
  );
}
