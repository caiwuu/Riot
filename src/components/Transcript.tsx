/**
 * 对话流：消息列表、会话内查找、贴底跟随，以及消息行的渲染。
 *
 * 从 App.tsx 拆出的独立职责。滚动语义（非对称迟滞、程序化滚动
 * pinning、每会话位置缓存）全在这里 —— 改贴底/恢复行为只动这个文件。
 * 渲染预算的三层（懒解析 / memo / content-visibility）见
 * ARCHITECTURE §13.1。
 */

import {
  memo,
  type ReactNode,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import type { PermissionAsk, PermissionResponse } from "../bridge";
import type { Item } from "../hooks/useSession";
import { SLASH_HEAD_RE, extractMentionSpans, mentionCovers } from "../lib/promptText";
import { basename, joinRoot, looksAbsPath } from "../pathDisplay";
import { openFilePreview } from "./FilePreview";
import { LazyMarkdown, Markdown, ProjectRootContext } from "./Markdown";
import { AskChoiceCard, PlanApprovalCard, PlanDraft } from "./PermissionDialog";
import { groupBlocks, ProcessGroup, ThinkingBlock } from "./ProcessFold";
import { ShotViewer, ToolCard } from "./ToolCard";

/**
 * 对话流滚到哪、跟不跟随底部。
 *
 * 正式包 WKWebView 一给面板加 `visibility:hidden` / 改 `position`，
 * 会把 scrollTop 清成 0 并冒一次 scroll。dev 的 WebView 常常不这么做，
 * 所以只有打包后才表现为"切回来跳到顶"。记的必须是用户还看得见时的
 * 位置，隐藏之后那次假滚动一律丢掉。
 */
export const transcriptView = new Map<string, { top: number; stick: boolean }>();

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

  // 关闭（含卸载）时清掉高亮，别在页面上留一堆黄块
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
          onKeyDown={(e) => {
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
}) {
  const boxRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  /** 程序化贴底时挡住 onScroll，免得自己把 stick 打成 false。 */
  const pinning = useRef(false);
  /**
   * 向上翻看时的滚动锚点：视口里第一个顶边完整可见的块 + 它到视口顶的距离。
   *
   * 上方的行是懒水合的（content-visibility 按 60px 估高、LazyMarkdown
   * 纯文本占位），第一次往上翻时真实高度陆续落地，scrollHeight 一变
   * 画面就跳。WKWebView 不支持 overflow-anchor 的浏览器原生锚定 ——
   * 只能自己记住「正看着哪个块」，高度变了把 scrollTop 补回去
   * （补偿在下面的 ResizeObserver 里）。贴底时用不上，锚点清空。
   */
  const anchor = useRef<{ el: Element; top: number } | null>(null);
  // 渲染期就写：正式包隐藏面板时 scroll 发生在 commit 里，
  // effect 还没跑，闭包里的 armed 仍是 true，会把清零后的 0 记进去。
  const armedRef = useRef(armed);
  armedRef.current = armed;

  const rememberView = (box: HTMLElement) => {
    if (!armedRef.current) return;
    transcriptView.set(sessionId, { top: box.scrollTop, stick: stick.current });
  };

  const captureAnchor = () => {
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

  const pinBottom = () => {
    const box = boxRef.current;
    if (!box) return;
    pinning.current = true;
    box.scrollTop = box.scrollHeight;
    stick.current = true;
    anchor.current = null;
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
    const gap = box.scrollHeight - box.scrollTop - box.clientHeight;
    setAwayFromBottom(gap > box.clientHeight);
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
      // 隐藏那一帧正式包会把 scrollTop 打成 0。armed 在渲染时已是
      // false，这次滚动不是用户翻的，不能写进缓存、也不能改 stick。
      if (!armedRef.current) return;
      if (pinning.current) return;
      const gap = box.scrollHeight - top - box.clientHeight;
      if (delta < 0 && gap > 1) {
        // gap > 1 挡掉底部橡皮筋回弹：过冲弹回时 scrollTop 也在变小，
        // 但那不是"想往上翻"。
        stick.current = false;
      } else if (delta > 0 && gap < 24) {
        // 只有自己滚回贴底才恢复跟随。阈值收窄到约一行 —— 停在离底
        // 几十像素处阅读时，跟随不该被抢回去。
        stick.current = true;
      }
      rememberView(box);
      setAwayFromBottom(gap > box.clientHeight);
      captureAnchor();
    };
    box.addEventListener("scroll", onScroll, { passive: true });
    return () => box.removeEventListener("scroll", onScroll);
  }, [sessionId]);

  // 切回来把位置还回去。layout 一次不够：正式包揭开 visibility
  // 之后还要再排一次，第二帧再写才能压住 WebKit 的清零。
  useLayoutEffect(() => {
    if (!armed) return;
    restoreView();
    const again = requestAnimationFrame(() => restoreView());
    return () => cancelAnimationFrame(again);
  }, [armed, sessionId]);

  // 正文晚一拍量完（markdown / 图片）时高度还会涨，贴着就跟上。
  useEffect(() => {
    const box = boxRef.current;
    const col = box?.querySelector(".thread-col");
    if (!box || !col) return;
    const ro = new ResizeObserver(() => {
      if (stick.current) {
        pinBottom();
        return;
      }
      // 向上翻看时高度变了（懒水合落地、图片 / mermaid 出图），把锚点
      // 块拉回原位，否则第一次翻历史每水合一块画面就跳一下。
      // ResizeObserver 回调在 layout 之后、paint 之前跑，这里补写
      // scrollTop 用户看不到中间态。pinning 帧（restoreView 正在二次
      // 补写）不掺和；隐藏的保活面板（!armed）量出来的是假布局，也不动。
      const a = anchor.current;
      if (a && a.el.isConnected && armedRef.current && !pinning.current) {
        const dy = a.el.getBoundingClientRect().top - box.getBoundingClientRect().top - a.top;
        if (Math.abs(dy) > 0.5) {
          pinning.current = true;
          box.scrollTop += dy;
          rememberView(box);
          requestAnimationFrame(() => {
            pinning.current = false;
          });
        }
      }
      // 交出跟随后内容还在下面长，gap 变大但不触发 scroll 事件 ——
      // 「回到底部」得靠这里浮出来，不然用户翻上去就找不到回程。
      const gap = box.scrollHeight - box.scrollTop - box.clientHeight;
      setAwayFromBottom(gap > box.clientHeight);
    });
    ro.observe(col);
    return () => ro.disconnect();
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
    <main className="transcript" ref={boxRef}>
      {findOpen && armed ? <FindBar box={boxRef} onClose={() => setFindOpen(false)} /> : null}
      <div className="thread-col">
        {blocks.map((b, i) =>
          b.kind === "row" ? (
            <Row
              key={b.item.id}
              item={b.item}
              hydrate={findOpen || i >= hydrateFrom}
              regenEnabled={!busy}
              {...(onRegenerate ? { onRegenerate } : {})}
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
        {streaming ? (
          <div className="msg assistant">
            <Markdown text={streaming} />
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
      {/* 往上翻了超过一屏才出现 —— 贴底时这按钮只是噪音。点了重新贴底，
          流式输出会继续跟随。 */}
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
    </main>
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
}: {
  item: Item;
  onRegenerate?: (itemId: string) => void;
  regenEnabled?: boolean;
  /** 贴底 / 查找中：立刻解析 markdown 和工具详情。 */
  hydrate?: boolean;
}) {
  switch (item.kind) {
    case "user":
      // 用户输入按原文显示，不走 markdown —— 渲染会篡改他说的话
      return (
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
      );
    case "assistant":
      return (
        <div className="msg assistant">
          <LazyMarkdown text={item.text} eager={!!hydrate} />
          {/* 半截话得说明白它为什么半截 —— 不标的话，用户过一会儿回来
              看到的是一句戛然而止的回答，分不清是自己停的还是模型崩了。 */}
          {item.stopped ? <div className="msg-stopped">已停止生成</div> : null}
          <MsgActions
            text={item.text}
            regenEnabled={!!regenEnabled && !!onRegenerate}
            {...(onRegenerate ? { onRegenerate: () => onRegenerate(item.id) } : {})}
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

/** 悬停出现的消息操作：复制 + 重新生成。占位始终在，hover 才可见。 */
function MsgActions({
  text,
  onRegenerate,
  regenEnabled,
}: {
  text: string;
  onRegenerate?: () => void;
  regenEnabled: boolean;
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

  if (!cmdName) {
    return <>{fileNodes(body)}</>;
  }

  return (
    <>
      <CmdChip name={cmdName} />
      {body || files.length > 0 ? <> {fileNodes(body)}</> : null}
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

/**
 * 文件引用块。`preview` 置真时渲染成按钮、点击打开应用内预览 ——
 * 消息气泡里用；Composer 的 `@` 候选列表里它套在候选按钮内部，
 * 保持纯展示（button 嵌 button 不合法，点击语义也归外层）。
 */
export function FileChip({ path, preview = false }: { path: string; preview?: boolean }) {
  // 引用块记的是项目内相对路径，预览要拼成绝对的。
  const root = useContext(ProjectRootContext);
  if (!preview) {
    return (
      <span className="ref-chip static" title={path}>
        <FileIcon />
        {basename(path)}
      </span>
    );
  }
  const full = looksAbsPath(path) ? path : joinRoot(root, path);
  return (
    <button
      type="button"
      className="ref-chip static clickable"
      title={`预览 ${path}`}
      onClick={() => openFilePreview(full)}
    >
      <FileIcon />
      {basename(path)}
    </button>
  );
}

export function CmdChip({ name }: { name: string }) {
  return (
    <span className="cmd-chip static" title={`/${name}`}>
      /{name}
    </span>
  );
}

function FileIcon() {
  return (
    <svg width="11" height="11" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M9 1.8H4.5a1 1 0 0 0-1 1v10.4a1 1 0 0 0 1 1h7a1 1 0 0 0 1-1V5.3L9 1.8z"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinejoin="round"
      />
      <path d="M8.9 2v3.4h3.4" stroke="currentColor" strokeWidth="1.3" strokeLinejoin="round" />
    </svg>
  );
}
