import { useCallback, useEffect, useRef, useState } from "react";

import type {
  AgentError,
  DecisionReason,
  MessageMeta,
  ToolResultContent,
  Usage,
} from "../bridge/generated";
import {
  type AgentEvent,
  type HistorySnapshot,
  type ImageInput,
  type Message,
  type PendingAsk,
  type PermissionAsk,
  type PermissionMode,
  type PermissionResponse,
  deleteMessage as deleteMessageBridge,
  editMessage as editMessageBridge,
  getHistory,
  interrupt as interruptSession,
  isIpcTimeout,
  queueList,
  queueRemove,
  queueTake,
  regenerateTurn,
  respondPermission,
  sendTurn,
  subscribeSession,
  subscribeWindowFocus,
} from "../bridge";
import { extractTopLevelStringField, extractTopLevelStringFields } from "../lib/partialJson";

/**
 * 界面上的一条内容。
 *
 * 这不是消息的镜像 —— 一条 assistant 消息里可能同时有思考、正文和三个
 * 工具调用，它们在界面上是分开的四块。事件流到 UI 模型的这层翻译放在
 * 这里，组件就只管画。
 */
export type Item =
  /**
   * `images` 是 data URL，只给界面回显自己发过什么用。
   * `files` 是消息里 `@` 引用过的文件路径（内容进了模型，界面只列路径）。
   * `at` 是消息产生的时刻（Unix 毫秒，见 MessageMeta.created_at_ms）；
   * undefined = 本字段之前的老记录，界面那里不显示时间。
   */
  | { kind: "user"; id: string; text: string; images?: string[]; files?: string[]; at?: number }
  /** `stopped` = 用户按停止截断的半截回答（内核定稿的，见 finalize_partial）。 */
  | { kind: "assistant"; id: string; text: string; stopped?: boolean; at?: number }
  | { kind: "thinking"; id: string; text: string }
  | {
      kind: "tool";
      id: string;
      name: string;
      input: unknown;
      status: "running" | "ok" | "error";
      result?: string;
      /**
       * 结果里的图片（截图、读图），data URL。这是消息里自带的**压缩图**
       * （给模型的那份），先显示它，原图到了再换。
       */
      resultImage?: string;
      /**
       * 原图的磁盘路径（截图落盘的文件、或被读的图片本身）。
       * 界面优先按它加载原图 —— 压缩图看布局够，看文字不行。
       */
      resultImagePath?: string;
      output: string[];
    }
  | { kind: "error"; id: string; text: string }
  /** 中性告知（不是出错）：到达轮数上限之类，只是提示"该歇口气了"。 */
  | { kind: "notice"; id: string; text: string }
  /** 压缩边界：上面是压缩前的记录，下面是压缩后的对话。 */
  | { kind: "compact"; id: string };

/**
 * 排队面板里的一条插话（还没被内核注入的用户消息）。
 *
 * `id` 是宿主队列条目 id —— 注入后回流的消息用同一个 id，面板条目
 * 由此转成对话气泡。sendTurn 还没返回的短暂窗口里是本地临时 id。
 */
export interface QueuedItem {
  id: string;
  text: string;
  images: ImageInput[];
  /** `@` 引用的文件路径（输入框里的那些块）。 */
  refs: string[];
}

/**
 * 一条被撤回的提问：用户在模型开口之前按了停止，内核把它从历史里删了。
 *
 * 界面要把它放回输入框 —— 那句话从没被回答过，用户按停止的意思是
 * "重说一遍"，而不是"扔掉我刚打的字"。
 */
export interface WithdrawnPrompt {
  /** 内核那条消息的 id。对账/排查用，界面按位置认气泡。 */
  id: string;
  text: string;
  images: ImageInput[];
  refs: string[];
  /** 撤完这个会话一条消息都不剩了。侧栏那句自动标题也该跟着撤。 */
  sessionEmpty: boolean;
}

export interface SessionState {
  items: Item[];
  /** 正在流式输出的正文。 */
  streaming: string;
  /** 正在流式输出的思考过程。 */
  thinking: string;
  /**
   * 正在流式写出的计划正文（ExitPlanMode 的 `plan` 参数）。
   * `null` = 没在写计划。空字符串 = 已经认出是计划、正文还没到。
   * 计划整段塞在工具参数里，等 tool_use 完整到达才显示的话，
   * 用户会对着三个点干等几十秒，以为对话卡住了。
   */
  streamingPlan: string | null;
  busy: boolean;
  /**
   * 宿主正在压缩上下文。
   *
   * 压缩要真调一次模型做摘要，几十秒 —— 而那段时间界面上原本只有那三个点，
   * 和"模型正在回答"分不出来，用户看到的是应答莫名变慢。
   *
   * 宿主不发"压缩结束"，只发压缩**成功**的 `compacted`。失败的那一半
   * （摘要请求本身出错）没有事件，所以这个标志必须由后续动作清掉，
   * 见 handle 里 `request_start` 和 `done` 的处理。
   */
  compacting: boolean;
  /**
   * 待回答的权限请求，按到达顺序排队，弹窗每次只显示队首那个。
   *
   * `[约束]` 必须是队列而不是单个槽位。调度器会把并发安全的工具放在
   * 同一批里并行跑，每个各自征求授权 —— WebSearch + WebFetch 就是这样
   * 一对。用单槽位的话后到的会覆盖先到的，被覆盖那个永远等不到回应，
   * 一直挂到宿主侧 10 分钟超时才按拒绝收场。用户看到的是两个工具卡片
   * 转圈、没有任何弹窗。
   */
  asks: { requestId: string; detail: PermissionAsk }[];
  /**
   * 本会话的 token 用量。花的是用户的钱，应该让他看得见。
   *
   * `input`/`output` 是**累计**（每轮相加，只增不减）；`context` 是
   * **当前占用**的快照 —— 最近一次请求真实发出去的量，压缩之后会掉下来。
   * 两个口径混用会让人以为"聊了半天上下文才用了 5%"（累计 ÷ 窗口），
   * 或者"压缩完怎么还是这么多"（快照当累计看）。
   */
  tokens: { input: number; output: number; context: number };
  /**
   * 宿主主动切换的权限模式（批准计划时用户选的执行档）。null = 没发生过。
   * Composer 靠它同步显示 —— 不同步的话界面还写着「规划模式」，
   * 宿主已经按「自动接受编辑」放行了。
   */
  hostMode: PermissionMode | null;
  /** 排队面板：模型跑动中发的、还没注入对话的插话。 */
  queued: QueuedItem[];
  /**
   * 刚被撤回、等着放回输入框的那条提问。输入框消费掉之后置回 null
   * （见 `clearWithdrawn`）。
   */
  withdrawn: WithdrawnPrompt | null;
}

const MAX_TOOL_LINES = 200;

/**
 * Channel 沉默超过此时长，视为 IPC 已经死了。
 *
 * `[约束]` Tauri `Channel::send` 在 JS 那头已经不听时**不会失败**
 * （见 `AppState::attach_sink`）。笔记本睡眠、下午再打开、WKWebView
 * 把后台页掐掉，都是同一件事：内核照常跑完、历史照常落盘，界面却永远
 * 停在「正在生成」。切走再切回来能看见完整回复，是因为换会话会重新
 * 订阅并拉历史 —— 不换会话就没人做这两步。
 */
const SINK_STALE_MS = 30_000;
/** 忙碌/压缩中这么久收不到事件，主动换出口并对一下历史。 */
const BUSY_SILENCE_MS = 12_000;
/** 刚发送的那几秒不要用快照盖界面：乐观气泡还没进宿主历史。 */
const SEND_GRACE_MS = 8_000;
/** 切到别的 app 不到这么久、事件还在流，不要重订阅（避免把正在流的正文闪掉）。 */
const AWAY_RESYNC_MS = 30_000;
/**
 * 一次"换出口并对历史"最多占住多久。
 *
 * 比 bridge 给单条命令的期限短：那边是"这条命令算不算失败"，这里是
 * "还要不要挡着别人重连"。等待期间发消息的用户被这条挡在乐观气泡
 * 之前 —— 他打的字既没上屏也没报错，那是最不该出现的一种失败。
 */
const ENSURE_LIVE_DEADLINE_MS = 20_000;

/**
 * 等 `p`，但最多等 `ms`。到点就当它完成了（不抛）—— 调用方要的是
 * "别再等下去"，不是"报个错"。
 *
 * 定时器在 `p` 先落地时清掉：这个函数每次重连都调一次，留着就是一串
 * 挂到会话关闭的空转。
 */
function settleWithin(p: Promise<void>, ms: number): Promise<void> {
  return new Promise<void>((resolve) => {
    const timer = window.setTimeout(resolve, ms);
    const done = () => {
      window.clearTimeout(timer);
      resolve();
    };
    p.then(done, done);
  });
}

/**
 * 宿主的历史快照。
 *
 * `[约束]` 类型只有 `bridge` 里那一份（对着 Rust 的 `HistoryOut`）。
 * 这里以前另写了一份把四个字段标成可选的副本，于是满地 `?? false` /
 * `?? []` 兜着永远不会发生的情况，而真出现字段增删时两份都不会红。
 */
type HistorySnap = HistorySnapshot;

/**
 * 每个会话当前这轮等待的起点（epoch ms）。
 *
 * 挂在模块级：Chat 按会话 id 重挂载，组件内的计时起点活不过切换 ——
 * 切走再切回，状态行的秒数从 0 重数，明明已经等了两分钟却显示 3s。
 * 协议消息不带时间戳，历史水合恢复不出真实起点，只能在"等待开始"的
 * 时刻由前端记下来。应用重启后拿不回起点，从水合时刻起数（诚实的下界）。
 */
const waitStartAt = new Map<string, number>();

const EMPTY_STATE: SessionState = {
  items: [],
  streaming: "",
  thinking: "",
  streamingPlan: null,
  busy: false,
  compacting: false,
  asks: [],
  tokens: { input: 0, output: 0, context: 0 },
  hostMode: null,
  queued: [],
  withdrawn: null,
};

/**
 * 切走会话后界面树可能被卸掉，条目先记在这里。下一次挂上这个 id
 * 时立刻还原，不用干等 getHistory —— 等的那段时间主区是空的，长
 * 会话看起来就是白屏。
 *
 * `[约束]` 必须有上限。`items` 里的图片是 base64 data URL（一张 Retina
 * 截图 base64 之后一兆多），不淘汰的话，今天切过的每个会话都把自己
 * 的整份对话连图留在内存里，直到用户显式删除它 —— 而正常使用根本
 * 不会去删会话。挂载中的界面树由 App 的 KEEP_CHATS 管，这里放宽一档：
 * 淘汰的代价只是下次切回要等一次 getHistory，不是白屏。
 */
const SESSION_CACHE_MAX = 8;
const sessionCache = new Map<string, SessionState>();

/** 写缓存，并把这个会话顶到最近使用的一端。超限时淘汰最久没碰的。 */
function cacheSession(id: string, state: SessionState) {
  // Map 保持插入顺序：先删再插就是"移到队尾"。
  sessionCache.delete(id);
  sessionCache.set(id, state);
  if (sessionCache.size > SESSION_CACHE_MAX) {
    const oldest = sessionCache.keys().next();
    if (!oldest.done) sessionCache.delete(oldest.value);
  }
}

/** 读缓存并顺手续命。只读不续的话，一直空闲的会话会被自己的邻居挤掉。 */
function touchSession(id: string): SessionState | undefined {
  const hit = sessionCache.get(id);
  if (hit) cacheSession(id, hit);
  return hit;
}

/** 状态行计时的起点。null = 此刻没有在等的东西。 */
export function waitStartedAt(sessionId: string): number | null {
  return waitStartAt.get(sessionId) ?? null;
}

/** 会话删掉时清掉缓存，免得占着一份已经没了的对话。 */
export function forgetSession(id: string) {
  sessionCache.delete(id);
  waitStartAt.delete(id);
}

export function useSession(
  sessionId: string,
  opts?: {
    /**
     * 把浏览器面板打开给用户看。
     *
     * `[约束]` 只有模型**明说**要给用户看时才走这条：`ShowBrowser`，以及
     * 请用户在面板里亲自操作的 `BrowserHandoff`。别的 `Browser*` 工具一律
     * 不弹 —— 早先按名字前缀猜，结果抓包、扫描这类纯后台分析也在抢屏幕。
     *
     * 历史回放不会走这条 —— 只在实时事件上触发。
     */
    onBrowserOpen?: () => void;
    /**
     * 模型的 PreviewFile 工具**成功**后，把这个文件在预览面板里展示给
     * 用户。路径是模型传的原文（可能是相对路径），由调用方解析。
     * 同 onBrowserOpen：历史回放不走，只在实时事件上触发。
     */
    onPreviewFile?: (path: string) => void;
  },
) {
  const [state, setState] = useState<SessionState>(() => touchSession(sessionId) ?? EMPTY_STATE);
  /** 历史快照到过（或缓存里已有）。没到之前不要把长会话画成空招呼页。 */
  const [ready, setReady] = useState(() => sessionCache.has(sessionId));

  // 缓存跟着**提交后**的状态走，不写在 updater 里。
  //
  // `[约束]` updater 必须是纯函数。React 19 的并发渲染允许重复调用甚至
  // 整个丢弃一次 updater —— 在里面写外部缓存，等于让一份跨会话存活的
  // 数据跟着渲染的中间态跑。这里以前那样写没出事，靠的是"每次渲染都从
  // base state 重放整个队列、最后一次调用的值恰好是对的"，同一个文件在
  // tool_start 和 send 两处已经为 StrictMode 双跑各留过一个坑。
  useEffect(() => {
    cacheSession(sessionId, state);
  }, [sessionId, state]);

  // delta 先攒在 ref，由 rAF 决定何时 setState。逐条 setState 会让 React
  // 在快速流式输出时掉帧。页面不可见时 WebKit 会节流 rAF，那时直接刷 ——
  // 没人在看，掉帧无所谓，但数据不能积压。
  const pendingText = useRef("");
  const pendingThinking = useRef("");
  const pendingToolJson = useRef<{ id: string; chunk: string }[]>([]);
  /** 工具进度行也要合批：`cargo build` 一秒能吐几百行，逐条 setState
   *  等于逐行重渲染整棵树。走和 delta 同一个 rAF 出口。 */
  const pendingProgress = useRef<{ id: string; text: string }[]>([]);
  const toolJsonById = useRef(new Map<string, string>());
  /**
   * 工具卡片出现时就地落定的流式内容。
   *
   * 卡片必须排在模型那句话的**下面**（"先写文件："在上，卡片在下），所以
   * 工具一开始就把当时的流式文本/思考落成条目。完整消息随后会把同样的块
   * 再给一遍 —— 靠这里去重，否则同一句话在界面上出现两次。
   */
  const settled = useRef<{ text: string[]; thinking: string[] }>({ text: [], thinking: [] });
  const rafId = useRef(0);
  const onBrowserOpenRef = useRef(opts?.onBrowserOpen);
  onBrowserOpenRef.current = opts?.onBrowserOpen;
  const onPreviewFileRef = useRef(opts?.onPreviewFile);
  onPreviewFileRef.current = opts?.onPreviewFile;
  /** 等结果的 PreviewFile 调用：tool_use id → 路径。成功的结果一到才开
   *  预览 —— 在调用时就开的话，同一批里"先 Write 再 Preview"会抢在文件
   *  落盘之前打开一个报错的标签；失败的调用（文件不存在）则根本不该开。 */
  const pendingPreviews = useRef(new Map<string, string>());
  /** 等结果的 ShowBrowser 调用。同 pendingPreviews 的理由：浏览器里一页
   *  都没有时这个工具会失败，那时弹出来的是一个空面板。 */
  const pendingShowBrowser = useRef(new Set<string>());

  // 排队面板的权威镜像放 ref 而不是只放 state：事件回调（注入匹配、
  // Done 后接力）跑在 React 渲染周期之外，读 state 拿到的是一拍之前的
  // 值 —— 表现为"刚注入的条目又被接力重发了一遍"。所有修改都过
  // mutateQueued，state 只是它的投影。
  const queuedRef = useRef<QueuedItem[]>([]);
  // 事件回调里要判断"此刻在不在跑"（queueSendNow 分流），同样不能读 state。
  const busyRef = useRef(false);
  const compactingRef = useRef(false);
  useEffect(() => {
    busyRef.current = state.busy;
    compactingRef.current = state.compacting;
  }, [state.busy, state.compacting]);
  // send 在订阅 effect 之后才定义，Done 接力经 ref 调它。
  const sendRef = useRef<
    ((text: string, images?: ImageInput[], refs?: string[]) => Promise<boolean>) | null
  >(null);
  /** 最近一次从这条会话的 Channel 收到事件（或刚挂上新出口）。 */
  const lastHeardAt = useRef(Date.now());
  const lastSendAt = useRef(0);
  /** 换一条活 Channel，并在宿主已空闲时用历史把界面追上。 */
  const ensureLiveRef = useRef<(() => Promise<void>) | null>(null);

  const mutateQueued = useCallback(
    (fn: (q: QueuedItem[]) => QueuedItem[]) => {
      queuedRef.current = fn(queuedRef.current);
      const next = queuedRef.current;
      setState((s) => ({ ...s, queued: next }));
    },
    [setState],
  );

  useEffect(() => {
    const flush = () => {
      rafId.current = 0;
      const t = pendingText.current;
      const k = pendingThinking.current;
      const chunks = pendingToolJson.current;
      const lines = pendingProgress.current;
      if (!t && !k && chunks.length === 0 && lines.length === 0) return;
      pendingText.current = "";
      pendingThinking.current = "";
      pendingToolJson.current = [];
      pendingProgress.current = [];

      let plan: string | undefined;
      // 工具参数边流边填进卡片：id → 此刻已经到齐的那些字段。
      const partial = new Map<string, Record<string, string>>();
      for (const { id, chunk } of chunks) {
        const next = (toolJsonById.current.get(id) ?? "") + chunk;
        toolJsonById.current.set(id, next);
        const extracted = extractTopLevelStringField(next, "plan");
        if (extracted !== null) plan = extracted;
        partial.set(id, extractTopLevelStringFields(next));
      }

      setState((s) => {
        // 两处都改 items，必须串起来改：各自从 s.items 出发的话，
        // 后写的那份会把先写的覆盖掉。
        let items = s.items;
        if (partial.size) items = fillToolInput(items, partial);
        if (lines.length) items = appendToolOutput(items, lines);
        return {
          ...s,
          streaming: s.streaming + t,
          thinking: s.thinking + k,
          ...(plan !== undefined ? { streamingPlan: plan } : {}),
          ...(items !== s.items ? { items } : {}),
        };
      });
    };

    const schedule = () => {
      if (document.hidden) {
        flush();
        return;
      }
      if (!rafId.current) rafId.current = requestAnimationFrame(flush);
    };

    const onSubscribeError = (message: string) => {
      waitStartAt.delete(sessionId);
      setState((s) => ({
        ...s,
        busy: false,
        // 收不到事件了，那个"压缩中"再也不会有人来清。
        compacting: false,
        items: [
          ...s.items,
          {
            kind: "error",
            id: `sub-${Date.now()}`,
            text: `事件流订阅失败，这个会话收不到回复：${message}`,
          },
        ],
      }));
    };

    const handle = (event: AgentEvent) => {
      lastHeardAt.current = Date.now();
      switch (event.type) {
        case "delta":
          if (event.kind === "text") {
            pendingText.current += event.text;
            schedule();
          } else if (event.kind === "thinking") {
            pendingThinking.current += event.text;
            schedule();
          } else if (event.kind === "tool_start") {
            // 卡片立刻出现，不等参数流完。Write 的整份文件在参数里，
            // 生成要几十秒 —— 那段时间屏幕上不能什么都没有。
            flush();
            setState((s) => {
              if (s.items.some((it) => it.kind === "tool" && it.id === event.tool_use_id)) {
                return s;
              }
              const items = [...s.items];
              // 工具块开始 = 前面那段话已经说完了。就地落定，卡片才能
              // 排在它下面 —— 反过来是"卡片在前、话在后"。
              // `[约束]` 记 settled 用 includes 判重:StrictMode 把 updater
              // 跑两遍，直接 push 会攒下两份同样的文本。
              if (s.thinking.trim()) {
                items.push({
                  kind: "thinking",
                  id: `${event.tool_use_id}-k`,
                  text: s.thinking,
                });
                if (!settled.current.thinking.includes(s.thinking)) {
                  settled.current.thinking.push(s.thinking);
                }
              }
              if (s.streaming.trim()) {
                items.push({
                  kind: "assistant",
                  id: `${event.tool_use_id}-t`,
                  text: s.streaming,
                  // 这段话没有对应的完整消息（它会从消息里被 dropSettled
                  // 摘掉），拿不到内核的戳 —— 就地记一个。切回会话时整份
                  // 重建，那时用的是内核那份。
                  at: Date.now(),
                });
                if (!settled.current.text.includes(s.streaming)) {
                  settled.current.text.push(s.streaming);
                }
              }
              items.push({
                kind: "tool",
                id: event.tool_use_id,
                name: event.name,
                input: {},
                status: "running",
                output: [],
              });
              return { ...s, items, streaming: "", thinking: "" };
            });
          } else if (event.kind === "tool_input") {
            pendingToolJson.current.push({
              id: event.tool_use_id,
              chunk: event.partial_json,
            });
            schedule();
          }
          break;

        case "request_start":
          toolJsonById.current.clear();
          // 上一轮的残留会让这一轮的同样一句话被误删。
          settled.current = { text: [], thinking: [] };
          busyRef.current = true;
          // 只补缺不重置：一轮里每次模型调用都发 request_start，
          // 中途重置会让状态行的秒数在轮内清零。
          if (!waitStartAt.has(sessionId)) waitStartAt.set(sessionId, Date.now());
          // 请求开始意味着压缩这一段结束了 —— 成功如此，失败也如此
          // （失败没有事件，只有日志，但轮次照常用完整历史往下走）。
          setState((s) => ({
            ...s,
            busy: true,
            streamingPlan: null,
            compacting: false,
          }));
          break;

        case "message": {
          flush();
          if (event.role === "assistant") {
            for (const c of event.content) {
              if (c.type !== "tool_use") continue;
              if (c.name === "ShowBrowser") {
                pendingShowBrowser.current.add(c.id);
              } else if (c.name === "BrowserHandoff") {
                // 唯一在**调用时**就弹的工具。它请用户在面板里亲自做一件事
                // （登录、过验证码），而它的结果要等他做完才回来 —— 等结果
                // 再弹，等于让他先对着一个看不见的页面动手。
                onBrowserOpenRef.current?.();
              } else if (c.name === "PreviewFile") {
                const p = (c.input as { path?: unknown } | null)?.path;
                if (typeof p === "string" && p.trim()) {
                  pendingPreviews.current.set(c.id, p);
                }
              }
            }
          }
          // 成功的结果到了才开面板 —— 这时内核已经确认过文件真的存在、
          // 浏览器里真的有一页，开出来的不会是报错标签或空面板。
          if (event.role === "user") {
            for (const c of event.content) {
              if (c.type !== "tool_result") continue;
              const p = pendingPreviews.current.get(c.tool_use_id);
              if (p !== undefined) {
                pendingPreviews.current.delete(c.tool_use_id);
                if (!c.is_error) onPreviewFileRef.current?.(p);
              }
              if (pendingShowBrowser.current.delete(c.tool_use_id) && !c.is_error) {
                onBrowserOpenRef.current?.();
              }
            }
          }
          // 排队的插话被内核注入了 —— 面板条目转成对话气泡。先按 id 配
          //（宿主入队时定的、注入原样带回），配不上再按原文兜底（sendTurn
          // 返回和注入事件跨两条通道，极端时序下 id 还没换成宿主的）。
          if (event.role === "user") {
            const text = event.content.find((c) => c.type === "text")?.text;
            const hit =
              queuedRef.current.find((q) => q.id === event.id) ??
              (text !== undefined ? queuedRef.current.find((q) => q.text === text) : undefined);
            if (hit) {
              mutateQueued((q) => q.filter((x) => x !== hit));
              setState((s) => ({
                ...s,
                items: [
                  ...s.items,
                  {
                    kind: "user",
                    id: event.id,
                    text: hit.text,
                    images: hit.images.map((i) => `data:${i.mediaType};base64,${i.data}`),
                    ...(hit.refs.length ? { files: hit.refs } : {}),
                    ...stampOf(event.meta),
                  },
                ],
              }));
              break;
            }
          }
          // 去重在 updater 外面做：StrictMode 会把 updater 跑两遍，
          // 而 settled 的消费是一次性的。
          const deduped = dropSettled(event, settled.current);
          setState((s) => applyMessage(s, deduped));
          break;
        }

        case "progress":
          if (event.payload.kind === "line") {
            pendingProgress.current.push({
              id: event.tool_use_id,
              text: event.payload.text,
            });
            schedule();
          }
          break;

        case "permission_request":
          flush();
          setState((s) => {
            // 同一个 request_id 重复到达就不排两次 —— 事件重放（切回
            // 会话）不该让用户连答两遍同一个问题。
            if (s.asks.some((a) => a.requestId === event.request_id)) return s;
            const isPlan = event.detail.suggestions.some((x) => x.type === "set_mode");
            return {
              ...s,
              asks: [...s.asks, { requestId: event.request_id, detail: event.detail }],
              // 批准卡接手之后草稿可以撤 —— 两份同一份计划叠在一起。
              ...(isPlan ? { streamingPlan: null } : {}),
            };
          });
          break;

        case "permission_resolved":
          // 宿主那边这个请求已经作废（超时或被中断）。不撤掉弹窗的话，
          // 它会一直挂着，用户点"允许"也毫无反应 —— 操作早就被拒了。
          setState((s) => applyResolved(s, event.request_id, event.reason));
          break;

        case "mode_changed":
          setState((s) => ({ ...s, hostMode: event.mode }));
          break;

        case "compacting":
          // 这段时间界面上只有那三个点在动。不说的话用户以为是模型变慢了，
          // 而实际上系统在做一件必要的事。
          // 手动 /compact 不开轮次、没有 request_start，等待起点在这里补。
          if (!waitStartAt.has(sessionId)) waitStartAt.set(sessionId, Date.now());
          setState((s) => ({ ...s, compacting: true }));
          break;

        case "compacted":
          // 手动压缩（不在轮内）到此等待结束；轮内自动压缩则轮子还在跑。
          if (!busyRef.current) waitStartAt.delete(sessionId);
          // 旧消息还在界面上，划一条线就够了。再出一块提示等于把记录盖住。
          flush();
          setState((s) => ({
            ...s,
            compacting: false,
            items: [...s.items, { kind: "compact", id: `compact-${Date.now()}` }],
          }));
          // 和宿主对一次账。长压缩（>12s 静默）期间看门狗拉过快照，把
          // running 被占的 busy=true 吸了进来 —— 而手动压缩结束没有 Done，
          // 这个 busy 没有别的事件会来清，状态行卡在假的"正在生成"直到下
          // 一个看门狗周期。内核保证发这个事件时 running 已释放（见
          // compact_now），所以空闲场景快照回 busy=false、这里立刻清干净；
          // 轮内场景快照回 busy=true，catchUp=false 只换出口不盖正在流的
          // 内容 —— 两种场景以宿主为准，前端不猜。
          void ensureLive({ catchUp: false });
          break;

        case "prompt_withdrawn":
          // 模型一个字都没给出就被停了，内核把这条提问从历史里删了。
          // 界面跟着撤气泡，并把原文交给输入框（Composer 消费 withdrawn）。
          flush();
          setState((s) => applyWithdrawn(s, event.message_id, event.session_empty));
          break;

        case "done": {
          flush();
          toolJsonById.current.clear();
          waitStartAt.delete(sessionId);
          // 立刻同步 ref —— 下面的接力在 React 提交之前就要按"已空闲"
          // 决定开新轮还是排队。
          busyRef.current = false;
          setState((s) => applyDone(s, event));
          // 接力：轮子停了（中断/满轮/竞态漏网）面板里还排着插话 ——
          // 自动按顺序重发，第一条开新轮、其余重新排队。这正是 CC/Cursor
          // 的语义：排队的消息是用户明确说过的话，不该因为一次停止而
          // 人间蒸发。出错例外：provider 都坏了，自动重发只会连环报错，
          // 条目留在面板里等用户处置。
          if (event.reason.reason !== "error" && queuedRef.current.length > 0) {
            const pending = [...queuedRef.current];
            mutateQueued(() => []);
            void (async () => {
              for (const it of pending) {
                // 每条之间都要重查：逐条 await 期间用户可能切走会话
                // （超出 KEEP_CHATS 就卸载了），剩下的不该继续发出去。
                if (cancelled) {
                  mutateQueued((q) => [...q, it]);
                  continue;
                }
                // 重发被拒（hook 拦了、模型没配好）就放回面板 ——
                // 接力的前提是"这些话用户还想说"，发不出去更不该丢。
                const ok = await sendRef.current?.(it.text, it.images, it.refs);
                if (!ok) mutateQueued((q) => [...q, it]);
              }
            })();
          }
          break;
        }

        default:
          break;
      }
    };

    // 历史和实时事件的衔接：订阅先建立（不能丢事件），事件先进缓冲；
    // 历史落位之后再回放缓冲。反过来做（先拉历史再订阅）会在两次调用
    // 的间隙丢事件，表现为切回会话时最后半句话不见了。
    let cancelled = false;
    let historyReady = false;
    const buffered: AgentEvent[] = [];

    const listen = (event: AgentEvent) => {
      if (!historyReady) {
        buffered.push(event);
        return;
      }
      handle(event);
    };

    const restoreQueue = () => {
      void queueList(sessionId)
        .then((list) => {
          if (cancelled || list.length === 0) return;
          mutateQueued((q) => [
            ...list
              .filter((x) => !q.some((y) => y.id === x.id))
              .map((x) => ({
                id: x.id,
                text: x.text,
                images: [] as ImageInput[],
                refs: x.refs,
              })),
            ...q,
          ]);
        })
        .catch(() => {
          // 会话可能刚被删。面板空着就空着，不值得报错。
        });
    };

    // 切回会话时重建排队面板：镜像随组件卸载丢了，宿主队列还在。
    // 图片拿不回全量（快照只带张数），但编辑撤回时会从宿主取回原图。
    mutateQueued(() => []);
    restoreQueue();

    let sub = subscribeSession(sessionId, listen, onSubscribeError);

    const applySnap = (hist: HistorySnap) => {
      const busy = hist.busy;
      const compacting = hist.compacting;
      // 等待起点跟着快照对齐：还在等而起点丢了（应用重启）就从现在
      // 起数；已经空闲就清掉，免得下一轮从陈旧起点开始。
      if (busy || compacting) {
        if (!waitStartAt.has(sessionId)) waitStartAt.set(sessionId, Date.now());
      } else {
        waitStartAt.delete(sessionId);
      }
      busyRef.current = busy;
      compactingRef.current = compacting;
      setState((s) => applyHistorySnap(s, hist));
    };

    // 先等出口挂上再拉历史。并行的话，结束事件可能还打在已经没人听的
    // 旧 channel 上，而快照里 busy 仍是 true —— 停止键就摘不掉。
    void sub.ready
      .then(() => {
        if (cancelled) return undefined;
        return getHistory(sessionId);
      })
      .then((hist) => {
        if (cancelled || !hist) return;
        // 订阅之后、快照落地之前的 text/thinking delta 已经在内核缓冲里
        // 一份，也在这里的 buffer 里一份。两头都用会把前缀拼两遍，字数
        // 翻倍；丢掉 buffer 又会丢快照之后才到的那几个 token。按重叠
        // 拼接（见 mergeLive）。
        const extra = liveDeltasOf(buffered);
        applySnap({
          ...hist,
          liveText: mergeLive(hist.liveText, extra.text),
          liveThinking: mergeLive(hist.liveThinking, extra.thinking),
        });
      })
      .catch(() => {
        // 新会话没有历史，拿不到不算错。真正的通信故障会在订阅那边报。
      })
      .finally(() => {
        if (cancelled) return;
        historyReady = true;
        setReady(true);
        for (const e of buffered) {
          // text/thinking 已经折进 liveText/liveThinking，再 handle 会重拼。
          if (isLiveDelta(e)) continue;
          handle(e);
        }
        buffered.length = 0;
      });

    lastHeardAt.current = Date.now();
    let ensureInflight: Promise<void> | null = null;
    /**
     * `catchUp`：用历史覆盖条目。看门狗在轮子还在跑时只换出口，
     * 不要每十几秒把正在流的正文和工具输出盖掉。
     */
    const ensureLive = (opts?: { catchUp?: boolean }): Promise<void> => {
      if (cancelled || !historyReady) return Promise.resolve();
      if (ensureInflight) return ensureInflight;
      const catchUp = opts?.catchUp !== false;
      lastHeardAt.current = Date.now();
      const run = (async () => {
        try {
          sub.unsubscribe();
          sub = subscribeSession(sessionId, listen, onSubscribeError);
          await sub.ready;
          if (cancelled) return;
          const hist = await getHistory(sessionId);
          if (cancelled || !hist) return;
          if (!hist.busy || catchUp) {
            applySnap(hist);
          } else {
            busyRef.current = true;
            compactingRef.current = hist.compacting;
            setState((s) => ({
              ...s,
              busy: true,
              compacting: hist.compacting,
              // 弹窗也要对账：睡眠期间到的询问，事件早发进死通道了，
              // 只有快照里有它。
              asks: reconcileAsks(s.asks, hist.pendingAsks),
              // 通道死掉的那段思考/正文只在内核缓冲里。不 catchUp 条目
              // 以免盖掉正在流的工具输出，但半截流要接上，否则字数停住。
              streaming: mergeLive(hist.liveText, s.streaming),
              thinking: mergeLive(hist.liveThinking, s.thinking),
            }));
          }
          restoreQueue();
        } catch {
          // 订阅/历史失败：onSubscribeError 已经报过，这里别再铺一条。
        }
      })();
      // 这把锁必须自己会开。bridge 已经给每条命令上了期限，但那道保险
      // 管不了"宿主回了、这里的某个 await 却没往下走"的形态 —— 而锁一旦
      // 卡住，看门狗、切回前台、窗口聚焦三条重连路径**同时**失效，界面
      // 从此停在「正在生成」，且不报任何错。宁可放一次重复的重连进来
      // （重连本身幂等：换出口 + 拉快照），也不能永久上锁。
      const guarded = settleWithin(run, ENSURE_LIVE_DEADLINE_MS);
      ensureInflight = guarded;
      void guarded.then(() => {
        // 只清自己那一把 —— 期限到时早退的话，后面可能已经换了新的。
        if (ensureInflight === guarded) ensureInflight = null;
      });
      return guarded;
    };
    ensureLiveRef.current = () => ensureLive({ catchUp: true });

    let hiddenAt = 0;
    const onVisibility = () => {
      if (document.hidden) {
        hiddenAt = Date.now();
        return;
      }
      const away = hiddenAt ? Date.now() - hiddenAt : 0;
      hiddenAt = 0;
      if (away < AWAY_RESYNC_MS && Date.now() - lastHeardAt.current < SINK_STALE_MS) {
        return;
      }
      void ensureLive({ catchUp: true });
    };
    document.addEventListener("visibilitychange", onVisibility);

    const onWindowFocus = () => {
      // 睡眠唤醒有时不翻 visibility（窗口一直"可见"），只给一个 focus。
      if (Date.now() - lastHeardAt.current < SINK_STALE_MS) return;
      void ensureLive({ catchUp: true });
    };
    window.addEventListener("focus", onWindowFocus);

    const unlistenFocus = subscribeWindowFocus((focused) => {
      if (!focused || cancelled) return;
      if (Date.now() - lastHeardAt.current < SINK_STALE_MS) return;
      void ensureLive({ catchUp: true });
    });

    const watchdog = window.setInterval(() => {
      if (cancelled) return;
      if (!busyRef.current && !compactingRef.current) return;
      if (Date.now() - lastHeardAt.current < BUSY_SILENCE_MS) return;
      if (Date.now() - lastSendAt.current < SEND_GRACE_MS) return;
      void ensureLive({ catchUp: false });
    }, 4_000);

    return () => {
      cancelled = true;
      ensureLiveRef.current = null;
      sub.unsubscribe();
      document.removeEventListener("visibilitychange", onVisibility);
      window.removeEventListener("focus", onWindowFocus);
      unlistenFocus();
      window.clearInterval(watchdog);
      if (rafId.current) cancelAnimationFrame(rafId.current);
    };
  }, [sessionId, mutateQueued, setState]);

  /**
   * 发一条消息。返回 false = 宿主没收下（UserPromptSubmit hook 拦了、
   * 模型没配好、会话没了），调用方应该把用户打的字放回输入框 ——
   * 失败时这里会撤掉乐观气泡，不放回的话那段文字就彻底没了。
   */
  const send = useCallback(
    async (
      text: string,
      images: ImageInput[] = [],
      refs: string[] = [],
    ): Promise<boolean> => {
      lastSendAt.current = Date.now();
      // 上午聊完、下午再发：Channel 往往已经死了。先换出口并对历史，
      // 否则这一轮的事件（包括 Done）继续丢进没人听的旧通道，界面
      // 永远停在「正在生成」，切走再切回来却能看见完整回复。
      //
      // `[约束]` 这一步只能延后发送，不能挡死它。它排在乐观气泡**之前**，
      // 卡在这里的表现是用户打的字凭空消失 —— 没有气泡、没有排队项、
      // 也没有报错。ensureLive 自带期限且从不抛（见 settleWithin），
      // 换出口失败就带着一条可能已经死掉的通道往下走：消息照发，最坏
      // 情况是这一轮的事件收不到，而那个有看门狗兜。
      if (Date.now() - lastHeardAt.current >= SINK_STALE_MS) {
        await ensureLiveRef.current?.();
      }
      // 乐观气泡上的时刻。内核那份（MessageMeta.created_at_ms）要等消息
      // 定稿才回来，而气泡现在就要显示 —— 差的只是一次 IPC 往返。
      const sentAt = Date.now();
      const localId = `local-${sentAt}-${Math.random().toString(36).slice(2, 8)}`;
      const dataUrls = images.map((i) => `data:${i.mediaType};base64,${i.data}`);
      // 立刻放上去（等宿主回声的话，用户会看到自己输入的内容凭空消失
      // 几百毫秒）：空闲 → 对话气泡；忙 → 排队面板。排队的插话在被
      // 内核注入之前**不进对话流** —— 气泡的位置意味着"模型看到了这句
      // 话"，而它此刻还没看到。
      //
      // `[约束]` queuedRef 的修改都在 setState 外面：StrictMode 会把
      // reducer 跑两遍，塞在里面的追加会翻倍。
      const wasBusy = busyRef.current;
      if (wasBusy) {
        mutateQueued((q) => [...q, { id: localId, text, images, refs }]);
      } else {
        busyRef.current = true;
        waitStartAt.set(sessionId, Date.now());
        setState((s) => ({
          ...s,
          busy: true,
          items: [
            ...s.items,
            {
              kind: "user",
              id: localId,
              text,
              images: dataUrls,
              ...(refs.length ? { files: refs } : {}),
              at: sentAt,
            },
          ],
        }));
      }
      try {
        const queuedId = await sendTurn(sessionId, text, images, refs);
        const inPanel = queuedRef.current.some((q) => q.id === localId);
        // 宿主的裁决可能和乐观放置相反（提交瞬间轮子恰好结束/恰好开跑），
        // 以返回值为准挪位置。
        if (queuedId != null) {
          if (inPanel) {
            // 面板条目换成宿主 id —— 注入回流的消息按它配对。
            mutateQueued((q) => q.map((x) => (x.id === localId ? { ...x, id: queuedId } : x)));
          } else if (!wasBusy) {
            // 乐观放的是气泡，宿主却说排队了（并发发送的竞态）：挪去面板。
            mutateQueued((q) => [...q, { id: queuedId, text, images, refs }]);
            setState((s) => ({ ...s, items: s.items.filter((it) => it.id !== localId) }));
          } else {
            // 面板条目已经没了。两种可能：注入事件抢在返回之前按原文配上
            // 了（remove 落空，无害）；或者用户趁宿主 id 没就位时把它删了/
            // 编辑撤走了 —— 这时必须把宿主队列里的那份也清掉，否则用户
            // 刚删的消息过会儿又被注入进对话。
            void queueRemove(sessionId, queuedId).catch(() => {});
          }
        } else if (inPanel) {
          // 乐观放进了面板，宿主却直接开轮了：转成对话气泡。
          mutateQueued((q) => q.filter((x) => x.id !== localId));
          busyRef.current = true;
          waitStartAt.set(sessionId, Date.now());
          setState((s) => ({
            ...s,
            busy: true,
            items: [
            ...s.items,
            {
              kind: "user",
              id: localId,
              text,
              images: dataUrls,
              ...(refs.length ? { files: refs } : {}),
              at: sentAt,
            },
          ],
          }));
        }
        return true;
      } catch (e) {
        mutateQueued((q) => q.filter((x) => x.id !== localId));
        // 乐观记下的等待起点一并回滚（轮子没开起来）。
        if (!wasBusy) waitStartAt.delete(sessionId);
        setState((s) => ({
          ...s,
          // 排队发送失败时上一轮还在跑，不能把它标成空闲 ——
          // 否则停止键消失了，输出却还在滚。
          busy: wasBusy,
          items: [
            ...s.items.filter((it) => it.id !== localId),
            { kind: "error", id: `err-${Date.now()}`, text: sendFailureText(e) },
          ],
        }));
        return false;
      }
    },
    [sessionId, mutateQueued, setState],
  );
  sendRef.current = send;

  /** 删一条排队插话（面板的垃圾桶）。 */
  const queueDelete = useCallback(
    (id: string) => {
      mutateQueued((q) => q.filter((x) => x.id !== id));
      // 宿主那边也删。false（已注入/已删）无妨 —— 面板以事件流为准。
      void queueRemove(sessionId, id).catch(() => {});
    },
    [sessionId, mutateQueued],
  );

  /**
   * 撤回一条排队插话去编辑：从宿主队列拿回原始输入（含图），面板条目
   * 消掉。返回 null = 条目已经不在（刚被注入），没东西可编。
   *
   * 条目还是本地 id（sendTurn 没返回，宿主必然说没有）时用镜像内容 ——
   * 在途的那份由 send 的对账兜底清掉，不会双份。
   */
  const queueEdit = useCallback(
    async (id: string): Promise<{ text: string; images: ImageInput[]; refs: string[] } | null> => {
      const local = queuedRef.current.find((q) => q.id === id);
      if (!local) return null;
      let took: { text: string; images: ImageInput[]; refs: string[] } | null = null;
      let takeFailed = false;
      try {
        took = await queueTake(sessionId, id);
      } catch {
        takeFailed = true; // IPC 抖了，用镜像内容兜底
      }
      if (!took && !takeFailed && !id.startsWith("local-")) {
        // 宿主明确说没有：它已经被注入进对话了，再放回输入框就是重复。
        return null;
      }
      mutateQueued((q) => q.filter((x) => x.id !== id));
      return took ?? { text: local.text, images: local.images, refs: local.refs };
    },
    [sessionId, mutateQueued],
  );

  /**
   * 立即发送一条排队插话（面板的 ↑）：不等安全点。
   *
   * 在跑 → 先从宿主队列撤下（防止停止前的最后一个安全点把它注入，
   * 出现"注入 + 接力"两条），挪到面板最前，停掉当前轮 —— Done 后的
   * 接力会让它第一个发出去。空闲（出错残留）→ 直接重发。
   */
  const queueSendNow = useCallback(
    async (id: string) => {
      const local = queuedRef.current.find((q) => q.id === id);
      if (!local) return;
      let took: { text: string; images: ImageInput[]; refs: string[] } | null = null;
      let takeFailed = false;
      try {
        took = await queueTake(sessionId, id);
      } catch {
        takeFailed = true;
      }
      if (!took && !takeFailed && !id.startsWith("local-")) {
        // 宿主明确说没有：刚被注入，消息已经在对话里了，别再发一遍。
        mutateQueued((q) => q.filter((x) => x.id !== id));
        return;
      }
      const item: QueuedItem = took ? { ...local, ...took } : local;
      if (busyRef.current) {
        mutateQueued((q) => [item, ...q.filter((x) => x.id !== id)]);
        void interruptSession(sessionId).then((cancelled) => {
          if (cancelled) return;
          // 宿主已经闲着，忙碌是残留。直接发，别等一个永远不来的 Done。
          busyRef.current = false;
          mutateQueued((q) => q.filter((x) => x.id !== id));
          void send(item.text, item.images, item.refs);
        });
      } else {
        mutateQueued((q) => q.filter((x) => x.id !== id));
        void send(item.text, item.images, item.refs);
      }
    },
    [sessionId, mutateQueued, send],
  );

  /**
   * 重新生成一条助手回复：丢掉它及之后的条目，从前面那条用户消息再跑。
   * 忙着的时候不做事 —— 按钮那时是禁用的。
   */
  const regenerate = useCallback(
    async (itemId: string) => {
      if (busyRef.current) return;
      const messageId = assistantMessageId(itemId);
      busyRef.current = true;
      waitStartAt.set(sessionId, Date.now());
      mutateQueued(() => []);
      setState((s) => ({
        ...s,
        items: trimAfterUserPrompt(s.items, itemId),
        streaming: "",
        thinking: "",
        streamingPlan: null,
        busy: true,
        asks: [],
        queued: [],
      }));
      try {
        await regenerateTurn(sessionId, messageId);
      } catch (e) {
        waitStartAt.delete(sessionId);
        busyRef.current = false;
        try {
          const hist = await getHistory(sessionId);
          setState((s) => {
            const restored = applyHistorySnap(s, hist);
            return {
              ...restored,
              busy: false,
              items: [
                ...restored.items,
                { kind: "error", id: `err-${Date.now()}`, text: humanizeError(e) },
              ],
            };
          });
        } catch {
          setState((s) => ({
            ...s,
            busy: false,
            items: [
              ...s.items,
              { kind: "error", id: `err-${Date.now()}`, text: humanizeError(e) },
            ],
          }));
        }
      }
    },
    [sessionId, mutateQueued, setState],
  );

  /**
   * 上下文修改（编辑/删除）的共用骨架：定位消息 → 执行 → 用快照对账。
   *
   * 操作前先拉一次快照，两个理由：
   * - 确认宿主真的空闲。本地 busy 只是镜像，忙时内核也会拒绝，
   *   提前拦下省一次注定失败的往返；
   * - 把界面条目换算成内核消息 id —— 乐观气泡（`local-*`）和流式期间
   *   就地落定的块（前缀是 tool_use id）带的都不是消息 id，得按
   *   角色 + 原文在快照里找到真身。
   *
   * 成功后再拉一次快照整份对齐（空心消息整条消失、多段文本合并成一段，
   * 这些边界自己在前端模拟一遍，等于把内核逻辑抄一份）。
   */
  const mutateHistory = useCallback(
    async (item: TextItem, op: (messageId: string) => Promise<void>): Promise<boolean> => {
      if (busyRef.current) return false;
      try {
        const hist = await getHistory(sessionId);
        if (hist.busy) return false;
        const messageId = locateMessage(hist.messages, item);
        if (!messageId) {
          throw new Error("这条消息已经不在当前上下文里（可能已被压缩进摘要）。");
        }
        await op(messageId);
        const after = await getHistory(sessionId);
        setState((s) => rebuildFromSnap(s, after));
        return true;
      } catch (e) {
        setState((s) => ({
          ...s,
          items: [
            ...s.items,
            { kind: "error", id: `err-${Date.now()}`, text: humanizeError(e) },
          ],
        }));
        return false;
      }
    },
    [sessionId, setState],
  );

  /**
   * 上下文编辑：把这条气泡对应消息的文本换掉。之后的轮次模型看到的
   * 就是改过的历史。返回 false = 没改成（忙、消息没了、内核拒绝），
   * 编辑框应保留草稿。
   */
  const editEntry = useCallback(
    (item: TextItem, text: string) =>
      mutateHistory(item, (messageId) => editMessageBridge(sessionId, messageId, text)),
    [sessionId, mutateHistory],
  );

  /** 上下文删除：按轮成对删（这条气泡所属的提问连同全部回应）。 */
  const deleteEntry = useCallback(
    (item: TextItem) =>
      mutateHistory(item, (messageId) => deleteMessageBridge(sessionId, messageId)),
    [sessionId, mutateHistory],
  );

  const stop = useCallback(() => {
    void interruptSession(sessionId).then((cancelled) => {
      if (cancelled) return;
      // 宿主已经闲着。结束事件在换订阅时丢过，界面还转圈 ——
      // 再点停止必须把残留忙碌清掉，否则停止键永远摘不下来。
      waitStartAt.delete(sessionId);
      busyRef.current = false;
      setState((s) => ({
        ...s,
        busy: false,
        compacting: false,
        streaming: "",
        thinking: "",
        streamingPlan: null,
        items: s.items.map((it) =>
          it.kind === "tool" && it.status === "running"
            ? { ...it, status: "error" as const, result: "未完成" }
            : it,
        ),
      }));
    });
  }, [sessionId, setState]);

  /** 输入框收下了撤回的那条提问。不清的话切走再回来它会被再放一次。 */
  const clearWithdrawn = useCallback(() => {
    setState((s) => (s.withdrawn ? { ...s, withdrawn: null } : s));
  }, [setState]);

  const answer = useCallback(
    async (response: PermissionResponse, requestId?: string) => {
      // 计划卡、选择题卡和权限弹窗可能同时在场（并发工具各自征求授权），
      // 必须按 id 应答 —— 拿队首猜的话，点计划卡的"批准"可能答给了
      // 旁边排队的某次 Bash 确认。
      const ask = requestId
        ? state.asks.find((a) => a.requestId === requestId)
        : state.asks[0];
      if (!ask) return;
      // 先出队，界面立刻切到下一个。等 IPC 往返会让按钮看起来没反应，
      // 用户会连点。按 id 移除而不是 slice(1)：万一同一个请求被答了
      // 两次，也只会移除它自己，不会顶掉后面排队的那个。
      setState((s) => ({
        ...s,
        asks: s.asks.filter((a) => a.requestId !== ask.requestId),
      }));
      await respondPermission(sessionId, ask.requestId, response);
    },
    [sessionId, state.asks, setState],
  );

  return {
    ...state,
    ready,
    send,
    stop,
    answer,
    regenerate,
    editEntry,
    deleteEntry,
    queueDelete,
    queueEdit,
    queueSendNow,
    clearWithdrawn,
  };
}

/** 能做上下文修改的条目：用户气泡和助手文本气泡。 */
export type TextItem = Extract<Item, { kind: "user" } | { kind: "assistant" }>;

/**
 * 上下文修改（编辑/删除）成功后的对账：快照就是真相，整份重建。
 *
 * 不走 applyHistorySnap —— 那是"切回活会话"的对齐语义，带两个照顾：
 * 空历史时保留旧条目、把不在快照里的乐观气泡（`local-*`）拼回来。
 * 这两个照顾在"刚刚确定性地删掉/改掉了什么"的场景里恰好是反效果：
 * 删掉唯一一轮后界面纹丝不动（看起来像没删，再点一次就报"消息不在
 * 上下文里"），编辑乐观气泡后旧文本又被拼回来显示成两条。
 */
function rebuildFromSnap(s: SessionState, hist: HistorySnap): SessionState {
  const messages = hist.messages;
  const archived = hist.archived;
  return {
    ...s,
    busy: hist.busy,
    compacting: hist.compacting,
    items: historyToItems(messages, archived, false),
    tokens: sumUsage([...archived, ...messages]),
    streaming: "",
    thinking: "",
    streamingPlan: null,
    asks: [],
  };
}

/**
 * 界面条目 → 内核消息 id。
 *
 * 历史水合的条目 id 是 `msg_x-t3` / `msg_x-u1` / `msg_x-k2`，剥掉后缀就是
 * 消息 id；实时路径还有两种带不了消息 id 的形态 —— 乐观用户气泡
 * （`local-*`）和工具卡出现时就地落定的块（前缀是 tool_use id）——
 * 按角色 + 原文在快照里倒序找（同文出现多次时取最近那条）。
 */
function locateMessage(messages: Message[], item: TextItem): string | null {
  const bare = item.id.replace(/-[tuk]\d*$/, "");
  if (messages.some((m) => m.id === bare)) return bare;
  const role = item.kind === "user" ? "user" : "assistant";
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i];
    if (!m || m.role !== role) continue;
    if (m.content.some((c) => c.type === "text" && c.text === item.text)) return m.id;
  }
  return null;
}

/** 活历史 + 压缩前归档 → 界面条目。归档画在分割线上面。 */
function historyToItems(live: Message[], archived: Message[], liveTurn = false): Item[] {
  const past = messagesToItems(archived, true);
  const now = messagesToItems(live, true);
  const items: Item[] =
    past.length === 0 ? now : [...past, { kind: "compact", id: "compact-boundary" }, ...now];
  if (liveTurn) return items;
  // 历史里不该有还在转圈的工具。有就是这一轮被中断了，如实说。
  // 轮子还在跑时（liveTurn）不能标 —— 切到正在跑的会话会把活工具
  // 画成「未完成」，下一秒事件又改回来，闪一下像出错了。
  return finalizeIdleItems(items, "未完成（该轮被中断）");
}

/**
 * 用宿主快照对齐界面。
 *
 * 空闲：整份替换并清掉忙碌残留（这就是下午回来还在「正在生成」的修复）。
 * 忙碌：条目追上已落盘的消息，但保留还没进历史的流式正文和工具输出。
 */
function applyHistorySnap(s: SessionState, hist: HistorySnap): SessionState {
  const messages = hist.messages;
  const archived = hist.archived;
  const busy = hist.busy;
  const compacting = hist.compacting;

  if (messages.length === 0 && archived.length === 0) {
    return {
      ...s,
      busy,
      compacting,
      ...(!busy
        ? {
            streaming: "",
            thinking: "",
            streamingPlan: null,
            asks: [],
            items: finalizeIdleItems(s.items, "未完成"),
          }
        : {
            asks: reconcileAsks(s.asks, hist.pendingAsks),
            streaming: mergeLive(
              hist.liveText,
              keepIfNotSettled(s.streaming, s.items, "assistant"),
            ),
            thinking: mergeLive(
              hist.liveThinking,
              keepIfNotSettled(s.thinking, s.items, "thinking"),
            ),
          }),
    };
  }

  const fromHist = historyToItems(messages, archived, busy);
  const items = mergeLiveTools(mergeOptimisticUser(fromHist, s.items), s.items);
  const tokens = sumUsage([...archived, ...messages]);
  if (busy) {
    return {
      ...s,
      busy: true,
      compacting,
      items,
      tokens,
      // 半截流以快照为准：内核收齐了每一条增量，本地这份在切走/通道
      // 断开期间是缺头的。快照为空（老内核或刚好没在流）才退回本地。
      streaming: mergeLive(
        hist.liveText,
        keepIfNotSettled(s.streaming, items, "assistant"),
      ),
      thinking: mergeLive(
        hist.liveThinking,
        keepIfNotSettled(s.thinking, items, "thinking"),
      ),
      asks: reconcileAsks(s.asks, hist.pendingAsks),
    };
  }
  return {
    ...s,
    busy: false,
    compacting,
    items,
    tokens,
    streaming: "",
    thinking: "",
    streamingPlan: null,
    asks: [],
  };
}

/** 还没进历史的乐观气泡留下，已经在快照里的丢掉。 */
function mergeOptimisticUser(fromHist: Item[], current: Item[]): Item[] {
  const extras = current.filter((it) => {
    if (it.kind !== "user" || !it.id.startsWith("local-")) return false;
    return !fromHist.some((h) => h.kind === "user" && h.text === it.text);
  });
  return extras.length === 0 ? fromHist : [...fromHist, ...extras];
}

/**
 * 用快照里的挂起询问重建弹窗队列。
 *
 * `permission_request` 事件只发一次：切走再切回（组件重挂载）、睡眠唤醒
 * 后换通道，事件都已经发进没人听的旧出口，弹窗全靠快照重建。快照为准 ——
 * 不在快照里的已经被解决（超时/分类器），留着就是一个点了没反应的僵尸
 * 弹窗；已在队列里的保住原对象，正在看的弹窗不因为一次对账重建。快照
 * 之后新到的询问由事件流补上（permission_request 的处理按 request_id
 * 去重，晚到的重复无害）。
 */
function reconcileAsks(
  current: { requestId: string; detail: PermissionAsk }[],
  snap: PendingAsk[],
): { requestId: string; detail: PermissionAsk }[] {
  return snap.map(
    (p) =>
      current.find((a) => a.requestId === p.request_id) ?? {
        requestId: p.request_id,
        detail: p.detail,
      },
  );
}

/**
 * 把内核快照里的半截流和本地/缓冲里的增量拼起来。
 *
 * 订阅之后到快照落地之间，同一段 delta 会同时出现在两边。哪边是前缀
 * 就取长的；有重叠就按后缀对齐再接上快照之后才到的那截。对不上就
 * 以快照为准再追加 —— 总比从 0 重数、缺一截头好。
 */
function mergeLive(snap: string | undefined, extra: string): string {
  const a = snap ?? "";
  if (!extra) return a;
  if (!a) return extra;
  if (extra.startsWith(a)) return extra;
  if (a.startsWith(extra)) return a;
  const max = Math.min(a.length, extra.length);
  for (let k = max; k >= 0; k--) {
    if (a.endsWith(extra.slice(0, k))) return a + extra.slice(k);
  }
  return a + extra;
}

function isLiveDelta(event: AgentEvent): boolean {
  return event.type === "delta" && (event.kind === "text" || event.kind === "thinking");
}

function liveDeltasOf(events: AgentEvent[]): { text: string; thinking: string } {
  let text = "";
  let thinking = "";
  for (const e of events) {
    if (e.type !== "delta") continue;
    if (e.kind === "text") text += e.text;
    else if (e.kind === "thinking") thinking += e.text;
  }
  return { text, thinking };
}

/** 历史没有进度行，把界面上还在跑的那张卡的输出接回去。 */
function mergeLiveTools(fromHist: Item[], current: Item[]): Item[] {
  return fromHist.map((it) => {
    if (it.kind !== "tool" || it.status !== "running") return it;
    const prev = current.find((c) => c.kind === "tool" && c.id === it.id);
    if (!prev || prev.kind !== "tool") return it;
    return {
      ...it,
      output: prev.output.length > 0 ? prev.output : it.output,
      input: hasToolInput(it.input) ? it.input : prev.input,
      ...(prev.resultImage ? { resultImage: prev.resultImage } : {}),
      ...(prev.resultImagePath ? { resultImagePath: prev.resultImagePath } : {}),
    };
  });
}

function hasToolInput(input: unknown): boolean {
  return !!input && typeof input === "object" && Object.keys(input as object).length > 0;
}

function keepIfNotSettled(
  live: string,
  items: Item[],
  kind: "assistant" | "thinking",
): string {
  if (!live.trim()) return "";
  if (items.some((it) => it.kind === kind && textsOverlap(it.text, live))) return "";
  return live;
}

function textsOverlap(a: string, b: string): boolean {
  return a === b || a.startsWith(b) || b.startsWith(a);
}

/** 界面条目 id（`msg_xxx-t` / `msg_xxx-t3`）→ 内核消息 id。 */
function assistantMessageId(itemId: string): string {
  return itemId.replace(/-t\d*$/, "");
}

/** 丢掉这条助手回复及之后的一切，保留到它前面那条用户气泡。 */
function trimAfterUserPrompt(items: Item[], assistantItemId: string): Item[] {
  const ast = items.findIndex((it) => it.id === assistantItemId);
  if (ast < 0) return items;
  for (let i = ast - 1; i >= 0; i--) {
    if (items[i]?.kind === "user") return items.slice(0, i + 1);
  }
  return items.slice(0, ast);
}

function finalizeIdleItems(items: Item[], result: string): Item[] {
  let hit = false;
  const out = items.map((it) => {
    if (it.kind !== "tool" || it.status !== "running") return it;
    hit = true;
    return { ...it, status: "error" as const, result: it.result ?? result };
  });
  return hit ? out : items;
}

/**
 * 消息上的时刻，摊成一段可展开的属性。
 *
 * 老 transcript 没有这个字段（也不该编一个出来），展开成空 —— 气泡上
 * 就没有时间，而不是标成"刚刚"。
 */
function stampOf(meta: MessageMeta | null | undefined): { at?: number } {
  const at = meta?.created_at_ms;
  return at ? { at } : {};
}

function messagesToItems(msgs: Message[], skipSynthetic = false): Item[] {
  const items: Item[] = [];

  for (const msg of msgs) {
    if (msg.role === "user") {
      // 带文本的合成用户消息（压缩后的续接摘要）不是用户说的话。
      // 分割线已经说明"上面被压缩了"，这里不再画一块提示把记录盖住。
      // 只拦带文本的：中断时合成的"已取消"消息只有 tool_result，必须
      // 走下面的正常流程去把工具卡片标成中断。
      if (msg.meta?.synthetic && msg.content.some((c) => c.type === "text")) {
        if (!skipSynthetic) {
          items.push({ kind: "compact", id: `${msg.id}-synthetic` });
        }
        continue;
      }
      // 历史里用户附的图。不回显的话，切回会话后"自己发过哪张图"就再也
      // 看不到了。挂在同一条消息的第一个文本气泡上 —— user_content 保证
      // 每条用户消息都有文本。
      //
      // 两种附件都是图：`image` 是模型自己能看图时存的原图，
      // `described_image` 是走视觉兼容时存的（模型读里面的转述，图给界面）。
      // 只认前者的话，纯文本模型下的图片全都不显示。
      const images = msg.content.flatMap((c) =>
        c.type === "attachment" && (c.kind === "image" || c.kind === "described_image")
          ? [`data:${c.media_type};base64,${c.data}`]
          : [],
      );
      // `@` 引用过的文件。内容进了模型，界面只列路径 —— 把整份文件
      // 铺在气泡里，一次引用就能把对话流淹掉。
      const files = msg.content.flatMap((c) =>
        c.type === "attachment" && c.kind === "user_file" ? [c.path] : [],
      );
      let imagesShown = false;
      for (const c of msg.content) {
        if (c.type === "text") {
          items.push({
            kind: "user",
            id: `${msg.id}-u${items.length}`,
            text: c.text,
            ...(images.length && !imagesShown ? { images } : {}),
            ...(files.length && !imagesShown ? { files } : {}),
            ...stampOf(msg.meta),
          });
          imagesShown = true;
        } else if (c.type === "tool_result") {
          const i = findLast(items, (it) => it.kind === "tool" && it.id === c.tool_use_id);
          if (i >= 0) {
            const t = items[i] as Extract<Item, { kind: "tool" }>;
            const view = resultView(c.content);
            items[i] = {
              ...t,
              status: c.is_error ? "error" : "ok",
              ...(view.text !== undefined ? { result: view.text } : {}),
              ...(view.image !== undefined ? { resultImage: view.image } : {}),
              ...(view.imagePath !== undefined ? { resultImagePath: view.imagePath } : {}),
            };
          }
        }
      }
    } else if (msg.role === "assistant") {
      for (const c of msg.content) {
        if (c.type === "text" && c.text.trim()) {
          items.push({
            kind: "assistant",
            id: `${msg.id}-t${items.length}`,
            text: c.text,
            ...(msg.meta?.interrupted ? { stopped: true as const } : {}),
            ...stampOf(msg.meta),
          });
        } else if (c.type === "thinking" && c.text.trim()) {
          items.push({ kind: "thinking", id: `${msg.id}-k${items.length}`, text: c.text });
        } else if (c.type === "tool_use") {
          items.push({
            kind: "tool",
            id: c.id,
            name: c.name,
            input: c.input,
            status: "running",
            output: [],
          });
        }
      }
    } else {
      items.push({ kind: "error", id: msg.id, text: msg.text });
    }
  }

  return items;
}

/**
 * 把流式到达的工具参数填进对应卡片。
 *
 * 参数还没齐，能显示多少显示多少 —— Write 的正文就是这样一行行长出来的。
 * 解析只认顶层字符串字段，数字和布尔参数要等完整的 tool_use 才补上。
 */
function fillToolInput(items: Item[], partial: Map<string, Record<string, string>>): Item[] {
  let hit = false;
  const out = items.map((it) => {
    if (it.kind !== "tool") return it;
    const fields = partial.get(it.id);
    if (!fields) return it;
    hit = true;
    return { ...it, input: fields };
  });
  return hit ? out : items;
}

/**
 * 去掉工具卡片出现时已经就地落定的那些块。
 *
 * 完整消息会把同一段话再给一遍，不去掉的话界面上出现两次。按内容配对
 * 而不是按位置 —— 一条消息里可能有好几个文本块，只有落定过的那些要走。
 */
function dropSettled(
  event: Extract<AgentEvent, { type: "message" }>,
  settled: { text: string[]; thinking: string[] },
): Extract<AgentEvent, { type: "message" }> {
  if (event.role !== "assistant") return event;
  if (settled.text.length === 0 && settled.thinking.length === 0) return event;
  const content = event.content.filter((c) => {
    if (c.type !== "text" && c.type !== "thinking") return true;
    const pool = c.type === "text" ? settled.text : settled.thinking;
    const at = pool.indexOf(c.text);
    if (at < 0) return true;
    pool.splice(at, 1);
    return false;
  });
  return { ...event, content };
}

function applyMessage(s: SessionState, event: Extract<AgentEvent, { type: "message" }>): SessionState {
  const items = [...s.items];
  const msg = event;

  if (msg.role === "user") {
    for (const c of msg.content) {
      if (c.type === "tool_result") {
        // 找到对应的工具卡片填结果。倒着找 —— 同一个工具在一次会话里
        // 会被调用很多次，最近那次才是它。
        const i = findLast(items, (it) => it.kind === "tool" && it.id === c.tool_use_id);
        if (i >= 0) {
          const t = items[i] as Extract<Item, { kind: "tool" }>;
          const view = resultView(c.content);
          items[i] = {
            ...t,
            status: c.is_error ? "error" : "ok",
            ...(view.text !== undefined ? { result: view.text } : {}),
            ...(view.image !== undefined ? { resultImage: view.image } : {}),
            ...(view.imagePath !== undefined ? { resultImagePath: view.imagePath } : {}),
          };
        }
      } else if (c.type === "text") {
        // 用户消息在 send 时已经乐观插入过了，这里跳过避免重复。
        // 内核合成的消息（如取消提示）不会走 text 这一支。
      }
    }
    return { ...s, items };
  }

  if (msg.role === "assistant") {
    const stopped = msg.meta?.interrupted ? { stopped: true as const } : {};
    const at = stampOf(msg.meta);
    for (const c of msg.content) {
      if (c.type === "text" && c.text.trim()) {
        items.push({ kind: "assistant", id: `${msg.id}-t`, text: c.text, ...stopped, ...at });
      } else if (c.type === "thinking" && c.text.trim()) {
        items.push({ kind: "thinking", id: `${msg.id}-k`, text: c.text });
      } else if (c.type === "tool_use") {
        // 卡片多半已经在了（tool_start 时就插好、参数边流边填）。这里补
        // 上完整参数 —— 再 push 一张的话，一次调用会显示成两张卡。
        const at = items.findIndex((it) => it.kind === "tool" && it.id === c.id);
        const card = {
          kind: "tool" as const,
          id: c.id,
          name: c.name,
          input: c.input,
          status: "running" as const,
          output: [] as string[],
        };
        if (at >= 0) {
          const prev = items[at] as Extract<Item, { kind: "tool" }>;
          items[at] = { ...card, status: prev.status, output: prev.output };
        } else {
          items.push(card);
        }
      }
    }
    // 流式文本已经落成正式消息，清掉临时区，否则同一段会显示两遍
    const u = msg.usage;
    const tokens = u
      ? {
          input: s.tokens.input + u.input_tokens + u.cache_read_tokens + u.cache_creation_tokens,
          output: s.tokens.output + u.output_tokens,
          context: contextOf(u),
        }
      : s.tokens;
    return { ...s, items, streaming: "", thinking: "", tokens };
  }

  if (msg.role === "system") {
    items.push({ kind: "error", id: msg.id, text: msg.text });
  }
  return { ...s, items };
}

/**
 * 把一帧里攒下的进度行追加到各自的工具卡片。
 *
 * 按工具分组后每张卡只重建一次：同一个 build 一帧内来几十行是常态，
 * 逐行复制 items 数组等于把开销乘上行数。找不到卡片的行直接丢 ——
 * 那是 tool_start 还没到（或已经被历史覆盖）的进度，没有归宿。
 */
function appendToolOutput(items: Item[], lines: { id: string; text: string }[]): Item[] {
  const byTool = new Map<string, string[]>();
  for (const { id, text } of lines) {
    const bucket = byTool.get(id);
    if (bucket) bucket.push(text);
    else byTool.set(id, [text]);
  }

  let out = items;
  for (const [id, texts] of byTool) {
    const i = findLast(out, (it) => it.kind === "tool" && it.id === id);
    if (i < 0) continue;
    // 第一次命中才复制，全都没命中时保持引用不变（memo 才挡得住）。
    if (out === items) out = [...items];
    const t = out[i] as Extract<Item, { kind: "tool" }>;
    // 只留尾部。一个 build 能吐几万行，全留着会让页面卡死，而有用的
    // 信息（错误摘要）总是在最后。
    out[i] = { ...t, output: [...t.output, ...texts].slice(-MAX_TOOL_LINES) };
  }
  return out;
}

function applyDone(s: SessionState, event: Extract<AgentEvent, { type: "done" }>): SessionState {
  const items = [...s.items];

  // 收尾时把还挂着 running 的工具卡片改掉。否则界面上会永远转圈，
  // 而后台其实什么都没在跑了。
  for (let i = 0; i < items.length; i++) {
    const it = items[i];
    if (it && it.kind === "tool" && it.status === "running") {
      items[i] = { ...it, status: "error", result: "未完成" };
    }
  }

  const r = event.reason;
  if (r.reason === "error") {
    items.push({ kind: "error", id: `done-${Date.now()}`, text: describeError(r.error) });
  } else if (r.reason === "max_turns") {
    // 不是报错:模型这一轮自主跑满了步数上限，停下来等指示。用中性
    // 提示，别用红色 error 样式吓人。文案说清"没坏、接着说就行"。
    items.push({
      kind: "notice",
      id: `done-${Date.now()}`,
      text: `这一轮忙活了 ${r.limit} 步，先停下来喘口气。要继续的话说一声（比如“继续”）就接着做；这个步数上限可以在设置里调。`,
    });
  }

  // 这一轮结束了，还排着队的权限请求已经没有意义 —— 它们对应的工具
  // 调用要么被中断要么已超时。留着的话，用户下一轮开口前会先被一个
  // 属于上一轮的弹窗拦住。
  return {
    ...s,
    items,
    busy: false,
    compacting: false,
    streaming: "",
    thinking: "",
    streamingPlan: null,
    asks: [],
  };
}

/**
 * 撤掉最后那条用户气泡，把内容交给输入框。
 *
 * 按位置找而不是按 id：气泡通常还是发送时的乐观条目（`local-*`），内核
 * 的消息 id 从没到过界面这一侧。撤回只发生在"这一轮什么都没产出"的时候，
 * 所以往回数第一条用户消息必然就是它 —— 但它不一定是**最后一条条目**：
 * 主动压缩的分割线会排在它后面。
 */
function applyWithdrawn(s: SessionState, id: string, sessionEmpty: boolean): SessionState {
  let at = -1;
  for (let i = s.items.length - 1; i >= 0; i--) {
    if (s.items[i]?.kind === "user") {
      at = i;
      break;
    }
  }
  const bubble = at < 0 ? undefined : s.items[at];
  // 没有气泡可撤（历史刚被别的路径重建过）：内核那边已经删了，界面
  // 保持现状就行，下一次快照会对齐。
  if (!bubble || bubble.kind !== "user") return s;

  const items = [...s.items];
  items.splice(at, 1);
  return {
    ...s,
    items,
    withdrawn: {
      id,
      text: bubble.text,
      images: (bubble.images ?? []).flatMap((u) => {
        const img = imageFromDataUrl(u);
        return img ? [img] : [];
      }),
      refs: bubble.files ?? [],
      sessionEmpty,
    },
  };
}

/** `data:image/png;base64,……` → 重新发送时要的图片输入。认不出就丢掉。 */
function imageFromDataUrl(url: string): ImageInput | null {
  const m = /^data:([^;,]+);base64,([\s\S]*)$/.exec(url);
  return m?.[1] && m[2] ? { mediaType: m[1], data: m[2] } : null;
}

/**
 * 撤掉一个已经作废的权限请求。
 *
 * 超时会额外留一行。不说的话用户只会以为程序卡了 —— 而这一步确实
 * 没执行，后面的结果都建立在"少做了一件事"之上。
 */
function applyResolved(s: SessionState, requestId: string, reason: DecisionReason): SessionState {
  if (!s.asks.some((a) => a.requestId === requestId)) return s;
  const asks = s.asks.filter((a) => a.requestId !== requestId);
  if (reason.kind !== "timeout") return { ...s, asks };
  return {
    ...s,
    asks,
    items: [
      ...s.items,
      {
        kind: "error",
        id: `ask-timeout-${requestId}`,
        text: "等待授权超时，这一步没有执行。",
      },
    ],
  };
}

/**
 * 发送失败时摆在对话流里的那一行。
 *
 * 宿主超时要和"宿主拒绝"分开说。拒绝是确定的：这条没进队列，原文回到
 * 输入框，重发就行。超时的结果**不确定** —— 命令可能已经落地、轮子已经
 * 在跑，只是回执没回来。这时催用户重发，就是在制造两条一模一样的消息。
 */
function sendFailureText(e: unknown): string {
  if (isIpcTimeout(e)) {
    return (
      "宿主一直没有回应，这条消息有没有发出去不好说 —— 原文已经放回输入框。" +
      "先看看这个会话过一会儿有没有自己动起来，再决定要不要重发。"
    );
  }
  return humanizeError(e);
}

/**
 * 把一串技术错误链翻成一句人话。识别不了的原样保留 —— 编出来的
 * 解释比看不懂的原文更糟。原文压缩在括号里，报 bug 时用得上。
 */
function humanizeError(e: unknown): string {
  // 宿主超时要抢在下面那条 timeout 规则前面：那句话说的是"网络或服务方
  // 没按时响应"，而这一类根本没走到网络 —— 是宿主自己没回话。
  if (isIpcTimeout(e)) return `${e.message}这一步做没做成不好说，先别急着重试。`;
  const raw = String(e);
  const lower = raw.toLowerCase();
  const known: [RegExp, string][] = [
    [/timed?\s*out|timeout/, "请求超时了，网络或服务方没有按时响应。稍等重试一般就好。"],
    [/dns|name not resolved|nodename/, "域名解析失败 —— 检查网络连接或服务方地址。"],
    [/connection refused|connect error|network|fetch failed/, "连不上服务方 —— 检查网络或代理设置。"],
    [/401|unauthorized|invalid.*key|authentication/, "服务方拒绝了 API key，去设置里检查一下。"],
    [/429|rate.?limit|overloaded/, "服务方限流了，稍等一会儿再发。"],
    // 额度类只认带额度语境的措辞。裸的 "insufficient" 会误伤 ——
    // DeepSeek 的 tool_calls 校验报错里就有 "insufficient tool messages"，
    // 真实余额没问题的用户被这句话带去查账单（生产事故）。
    // 覆盖的真实文案：DeepSeek "Insufficient Balance"、OpenAI
    // "insufficient_quota" / "You exceeded your current quota"、
    // Anthropic "credit balance is too low"、Kimi "balance is insufficient"。
    [
      /insufficient[_\s]+(quota|balance|funds|credits)|(credit\s+)?balance\s+is\s+(too\s+low|insufficient)|exceeded.{0,40}quota|quota.{0,40}exceeded|out\s+of\s+(quota|credits)|余额不足|欠费/,
      "服务方账户额度不足。",
    ],
  ];
  for (const [re, msg] of known) {
    if (re.test(lower)) return `${msg}（${raw.length > 200 ? `${raw.slice(0, 200)}…` : raw}）`;
  }
  return raw;
}

function describeError(e: AgentError): string {
  switch (e.kind) {
    case "provider":
      // 服务方的报错常是一整条 HTTP 错误链，先过一遍人话转换
      return humanizeError(e.message);
    case "context_exhausted":
      return `上下文超限且压缩无效（用了 ${e.used}，上限 ${e.limit}）。开个新会话吧。`;
    case "compact_circuit_open":
      return `压缩连续失败 ${e.attempts} 次，已停止重试。`;
    case "internal":
      return `内部错误：${e.message}`;
    default:
      return "未知错误";
  }
}

/**
 * 工具结果 → 界面上显示什么。
 *
 * 图片类结果（截图、读图）显示图片本身。described_image 的 text 是写给
 * 模型的转述（带着"当作亲眼所见"之类的内部指示），**不能**摆到界面上 ——
 * 用户该看到的是那张图。marked_image 的 text 则是和图同属一个结果的
 * 正文（编号清单、MCP 结果的文本部分），图文都给用户看。
 *
 * `image` 是消息里的压缩图（data URL），`imagePath` 是落盘原图的路径。
 * 两个都给：压缩图立刻能显示，原图由组件按路径异步加载后替换。
 */
function resultView(c: ToolResultContent): { text?: string; image?: string; imagePath?: string } {
  switch (c.type) {
    case "text":
      return { text: c.text };
    case "spilled":
      return { text: `结果过大（${c.total_bytes} 字节），已写入 ${c.path}\n\n${c.preview}` };
    case "cleared":
      return { text: "（历史结果已清理）" };
    case "marked_image":
      return {
        text: c.text,
        image: `data:${c.media_type};base64,${c.data}`,
        ...(c.path ? { imagePath: c.path } : {}),
      };
    case "image":
    case "described_image":
      return {
        image: `data:${c.media_type};base64,${c.data}`,
        ...(c.path ? { imagePath: c.path } : {}),
      };
    default:
      return {};
  }
}

function findLast<T>(arr: T[], pred: (x: T) => boolean): number {
  for (let i = arr.length - 1; i >= 0; i--) {
    const v = arr[i];
    if (v !== undefined && pred(v)) return i;
  }
  return -1;
}

/** 重建历史时恢复用量，和实时路径的口径一致。 */
function sumUsage(msgs: Message[]): { input: number; output: number; context: number } {
  let input = 0;
  let output = 0;
  // 每遇到一条就整个覆盖，循环结束后剩的就是最后一条 —— 那一次请求发出去
  // 的量就是此刻上下文的真实大小（服务方报的数，不是估的）。
  let context = 0;
  for (const m of msgs) {
    if (m.role === "assistant" && m.usage) {
      input += m.usage.input_tokens + m.usage.cache_read_tokens + m.usage.cache_creation_tokens;
      output += m.usage.output_tokens;
      context = contextOf(m.usage);
    }
  }
  return { input, output, context };
}

/** 一次请求占了多少上下文：发进去的（含命中缓存的）加上吐出来的。 */
function contextOf(u: Usage): number {
  return u.input_tokens + u.cache_read_tokens + u.cache_creation_tokens + u.output_tokens;
}
