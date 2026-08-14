import { useCallback, useEffect, useRef, useState } from "react";

import type { AgentError, DecisionReason, ToolResultContent } from "../bridge/generated";
import {
  type AgentEvent,
  type ImageInput,
  type Message,
  type PermissionAsk,
  type PermissionMode,
  type PermissionResponse,
  getHistory,
  interrupt as interruptSession,
  queueList,
  queueRemove,
  queueTake,
  respondPermission,
  sendTurn,
  subscribeSession,
} from "../bridge";
import { extractTopLevelStringField } from "../lib/partialJson";

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
   */
  | { kind: "user"; id: string; text: string; images?: string[]; files?: string[] }
  | { kind: "assistant"; id: string; text: string }
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
  | { kind: "notice"; id: string; text: string };

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
   * 待回答的权限请求，按到达顺序排队，弹窗每次只显示队首那个。
   *
   * `[约束]` 必须是队列而不是单个槽位。调度器会把并发安全的工具放在
   * 同一批里并行跑，每个各自征求授权 —— WebSearch + WebFetch 就是这样
   * 一对。用单槽位的话后到的会覆盖先到的，被覆盖那个永远等不到回应，
   * 一直挂到宿主侧 10 分钟超时才按拒绝收场。用户看到的是两个工具卡片
   * 转圈、没有任何弹窗。
   */
  asks: { requestId: string; detail: PermissionAsk }[];
  /** 本会话累计 token 用量。花的是用户的钱，应该让他看得见。 */
  tokens: { input: number; output: number };
  /**
   * 宿主主动切换的权限模式（批准计划时用户选的执行档）。null = 没发生过。
   * Composer 靠它同步显示 —— 不同步的话界面还写着「规划模式」，
   * 宿主已经按「自动接受编辑」放行了。
   */
  hostMode: PermissionMode | null;
  /** 排队面板：模型跑动中发的、还没注入对话的插话。 */
  queued: QueuedItem[];
}

const MAX_TOOL_LINES = 200;

export function useSession(
  sessionId: string,
  opts?: {
    /**
     * 模型开始用内置浏览器时通知界面把抽屉打开。
     * 历史回放不会走这条 —— 只在实时 `tool_use` 上触发。
     */
    onBrowserOpen?: () => void;
  },
) {
  const [state, setState] = useState<SessionState>({
    items: [],
    streaming: "",
    thinking: "",
    streamingPlan: null,
    busy: false,
    asks: [],
    tokens: { input: 0, output: 0 },
    hostMode: null,
    queued: [],
  });

  // delta 先攒在 ref，由 rAF 决定何时 setState。逐条 setState 会让 React
  // 在快速流式输出时掉帧。页面不可见时 WebKit 会节流 rAF，那时直接刷 ——
  // 没人在看，掉帧无所谓，但数据不能积压。
  const pendingText = useRef("");
  const pendingThinking = useRef("");
  const pendingToolJson = useRef<{ id: string; chunk: string }[]>([]);
  const toolJsonById = useRef(new Map<string, string>());
  const rafId = useRef(0);
  const onBrowserOpenRef = useRef(opts?.onBrowserOpen);
  onBrowserOpenRef.current = opts?.onBrowserOpen;

  // 排队面板的权威镜像放 ref 而不是只放 state：事件回调（注入匹配、
  // Done 后接力）跑在 React 渲染周期之外，读 state 拿到的是一拍之前的
  // 值 —— 表现为"刚注入的条目又被接力重发了一遍"。所有修改都过
  // mutateQueued，state 只是它的投影。
  const queuedRef = useRef<QueuedItem[]>([]);
  // 事件回调里要判断"此刻在不在跑"（queueSendNow 分流），同样不能读 state。
  const busyRef = useRef(false);
  useEffect(() => {
    busyRef.current = state.busy;
  }, [state.busy]);
  // send 在订阅 effect 之后才定义，Done 接力经 ref 调它。
  const sendRef = useRef<
    ((text: string, images?: ImageInput[], refs?: string[]) => Promise<boolean>) | null
  >(null);

  const mutateQueued = useCallback((fn: (q: QueuedItem[]) => QueuedItem[]) => {
    queuedRef.current = fn(queuedRef.current);
    const next = queuedRef.current;
    setState((s) => ({ ...s, queued: next }));
  }, []);

  useEffect(() => {
    const flush = () => {
      rafId.current = 0;
      const t = pendingText.current;
      const k = pendingThinking.current;
      const chunks = pendingToolJson.current;
      if (!t && !k && chunks.length === 0) return;
      pendingText.current = "";
      pendingThinking.current = "";
      pendingToolJson.current = [];

      let plan: string | undefined;
      for (const { id, chunk } of chunks) {
        const next = (toolJsonById.current.get(id) ?? "") + chunk;
        toolJsonById.current.set(id, next);
        const extracted = extractTopLevelStringField(next, "plan");
        if (extracted !== null) plan = extracted;
      }

      setState((s) => ({
        ...s,
        streaming: s.streaming + t,
        thinking: s.thinking + k,
        ...(plan !== undefined ? { streamingPlan: plan } : {}),
      }));
    };

    const schedule = () => {
      if (document.hidden) {
        flush();
        return;
      }
      if (!rafId.current) rafId.current = requestAnimationFrame(flush);
    };

    const onSubscribeError = (message: string) => {
      setState((s) => ({
        ...s,
        busy: false,
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
      switch (event.type) {
        case "delta":
          if (event.kind === "text") {
            pendingText.current += event.text;
            schedule();
          } else if (event.kind === "thinking") {
            pendingThinking.current += event.text;
            schedule();
          } else if (event.kind === "tool_input") {
            // 普通工具参数仍等完整 tool_use 再出卡片（JSON 碎片没法读）。
            // 计划是例外：正文就在参数里，必须边写边显示。
            pendingToolJson.current.push({
              id: event.tool_use_id,
              chunk: event.partial_json,
            });
            schedule();
          }
          break;

        case "request_start":
          toolJsonById.current.clear();
          busyRef.current = true;
          setState((s) => ({ ...s, busy: true, streamingPlan: null }));
          break;

        case "message": {
          flush();
          if (event.role === "assistant") {
            for (const c of event.content) {
              if (c.type === "tool_use" && c.name.startsWith("Browser")) {
                onBrowserOpenRef.current?.();
                break;
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
                  },
                ],
              }));
              break;
            }
          }
          setState((s) => applyMessage(s, event));
          break;
        }

        case "progress":
          setState((s) => applyProgress(s, event));
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

        case "compacted":
          // 不提示的话，用户看到的是回答突然变快了、模型偶尔忘了远处的
          // 细节 —— 而他不知道发生过什么。
          flush();
          setState((s) => ({
            ...s,
            items: [
              ...s.items,
              {
                kind: "notice",
                id: `compact-${Date.now()}`,
                text: `上下文已压缩（${fmtK(event.before_tokens)} → ${fmtK(event.after_tokens)} token）。更早的对话已折叠成摘要，继续对话不受影响。`,
              },
            ],
          }));
          break;

        case "done": {
          flush();
          toolJsonById.current.clear();
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

    // 切回会话时重建排队面板：镜像随组件卸载丢了，宿主队列还在。
    // 图片拿不回全量（快照只带张数），但编辑撤回时会从宿主取回原图。
    mutateQueued(() => []);
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

    const sub = subscribeSession(
      sessionId,
      (event) => {
        if (!historyReady) {
          buffered.push(event);
          return;
        }
        handle(event);
      },
      onSubscribeError,
    );

    getHistory(sessionId)
      .then(({ messages, busy }) => {
        if (cancelled) return;
        // busy 要跟着历史一起恢复：轮子在后台跑着，而界面状态随组件
        // 卸载丢了 —— 不恢复的话切回来看到的是发送键，停不下来。
        busyRef.current = busy;
        if (messages.length === 0) {
          setState((s) => ({ ...s, busy }));
          return;
        }
        setState((s) => ({
          ...s,
          busy,
          items: messagesToItems(messages),
          tokens: sumUsage(messages),
        }));
      })
      .catch(() => {
        // 新会话没有历史，拿不到不算错。真正的通信故障会在订阅那边报。
      })
      .finally(() => {
        if (cancelled) return;
        historyReady = true;
        for (const e of buffered) handle(e);
        buffered.length = 0;
      });

    return () => {
      cancelled = true;
      sub.unsubscribe();
      if (rafId.current) cancelAnimationFrame(rafId.current);
    };
  }, [sessionId]);

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
      const localId = `local-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
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
            },
          ],
          }));
        }
        return true;
      } catch (e) {
        mutateQueued((q) => q.filter((x) => x.id !== localId));
        setState((s) => ({
          ...s,
          // 排队发送失败时上一轮还在跑，不能把它标成空闲 ——
          // 否则停止键消失了，输出却还在滚。
          busy: wasBusy,
          items: [
            ...s.items.filter((it) => it.id !== localId),
            { kind: "error", id: `err-${Date.now()}`, text: String(e) },
          ],
        }));
        return false;
      }
    },
    [sessionId, mutateQueued],
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
        void interruptSession(sessionId);
      } else {
        mutateQueued((q) => q.filter((x) => x.id !== id));
        void send(item.text, item.images, item.refs);
      }
    },
    [sessionId, mutateQueued, send],
  );

  const stop = useCallback(() => void interruptSession(sessionId), [sessionId]);

  const answer = useCallback(
    async (response: PermissionResponse, requestId?: string) => {
      // 计划批准卡和权限弹窗可能同时在场（并发工具各自征求授权），
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
    [sessionId, state.asks],
  );

  return { ...state, send, stop, answer, queueDelete, queueEdit, queueSendNow };
}

/**
 * 把持久化的历史翻译成界面条目。切回会话时用。
 *
 * 和 applyMessage 的差别只有一处：历史里的用户文本必须显示（applyMessage
 * 跳过它，因为实时路径上 send 已经乐观插入过了）。
 */
function messagesToItems(msgs: Message[]): Item[] {
  const items: Item[] = [];

  for (const msg of msgs) {
    if (msg.role === "user") {
      // 带文本的合成用户消息（压缩后的续接摘要）不是用户说的话，渲染成
      // 用户气泡会让人以为自己发过一大段总结 —— 折叠成一条中性提示。
      // 只拦带文本的：中断时合成的"已取消"消息只有 tool_result，必须
      // 走下面的正常流程去把工具卡片标成中断。
      if (msg.meta?.synthetic && msg.content.some((c) => c.type === "text")) {
        items.push({
          kind: "notice",
          id: `${msg.id}-synthetic`,
          text: "（更早的对话已压缩成摘要，模型可见）",
        });
        continue;
      }
      // 历史里用户附的图（模型能直接看图时存的是原图）。不回显的话，
      // 切回会话后"自己发过哪张图"就再也看不到了。挂在同一条消息的
      // 第一个文本气泡上 —— user_content 保证每条用户消息都有文本。
      const images = msg.content.flatMap((c) =>
        c.type === "attachment" && c.kind === "image"
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
          items.push({ kind: "assistant", id: `${msg.id}-t${items.length}`, text: c.text });
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

  // 历史里不该有还在转圈的工具。有就是这一轮被中断了，如实说。
  for (let i = 0; i < items.length; i++) {
    const it = items[i];
    if (it && it.kind === "tool" && it.status === "running") {
      items[i] = { ...it, status: "error", result: "未完成（该轮被中断）" };
    }
  }

  return items;
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
    for (const c of msg.content) {
      if (c.type === "text" && c.text.trim()) {
        items.push({ kind: "assistant", id: `${msg.id}-t`, text: c.text });
      } else if (c.type === "thinking" && c.text.trim()) {
        items.push({ kind: "thinking", id: `${msg.id}-k`, text: c.text });
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
    // 流式文本已经落成正式消息，清掉临时区，否则同一段会显示两遍
    const u = msg.usage;
    const tokens = u
      ? {
          input: s.tokens.input + u.input_tokens + u.cache_read_tokens + u.cache_creation_tokens,
          output: s.tokens.output + u.output_tokens,
        }
      : s.tokens;
    return { ...s, items, streaming: "", thinking: "", tokens };
  }

  if (msg.role === "system") {
    items.push({ kind: "error", id: msg.id, text: msg.text });
  }
  return { ...s, items };
}

function applyProgress(
  s: SessionState,
  event: Extract<AgentEvent, { type: "progress" }>,
): SessionState {
  if (event.payload.kind !== "line") return s;
  const i = findLast(s.items, (it) => it.kind === "tool" && it.id === event.tool_use_id);
  if (i < 0) return s;

  const items = [...s.items];
  const t = items[i] as Extract<Item, { kind: "tool" }>;
  // 只留尾部。一个 build 能吐几万行，全留着会让页面卡死，而有用的
  // 信息（错误摘要）总是在最后。
  const output = [...t.output, event.payload.text].slice(-MAX_TOOL_LINES);
  items[i] = { ...t, output };
  return { ...s, items };
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
  return { ...s, items, busy: false, streaming: "", thinking: "", streamingPlan: null, asks: [] };
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

function describeError(e: AgentError): string {
  switch (e.kind) {
    case "provider":
      return e.message;
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
 * 用户该看到的是那张图。
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

function fmtK(n: number): string {
  return n >= 1000 ? `${Math.round(n / 1000)}k` : String(n);
}

function findLast<T>(arr: T[], pred: (x: T) => boolean): number {
  for (let i = arr.length - 1; i >= 0; i--) {
    const v = arr[i];
    if (v !== undefined && pred(v)) return i;
  }
  return -1;
}

/** 重建历史时恢复累计用量，和实时路径的口径一致。 */
function sumUsage(msgs: Message[]): { input: number; output: number } {
  let input = 0;
  let output = 0;
  for (const m of msgs) {
    if (m.role === "assistant" && m.usage) {
      input += m.usage.input_tokens + m.usage.cache_read_tokens + m.usage.cache_creation_tokens;
      output += m.usage.output_tokens;
    }
  }
  return { input, output };
}
