import { useCallback, useEffect, useRef, useState } from "react";

import type { AgentError, DecisionReason, ToolResultContent } from "../bridge/generated";
import {
  type AgentEvent,
  type Message,
  type PermissionAsk,
  type PermissionResponse,
  getHistory,
  interrupt as interruptSession,
  respondPermission,
  sendTurn,
  subscribeSession,
} from "../bridge";

/**
 * 界面上的一条内容。
 *
 * 这不是消息的镜像 —— 一条 assistant 消息里可能同时有思考、正文和三个
 * 工具调用，它们在界面上是分开的四块。事件流到 UI 模型的这层翻译放在
 * 这里，组件就只管画。
 */
export type Item =
  | { kind: "user"; id: string; text: string }
  | { kind: "assistant"; id: string; text: string }
  | { kind: "thinking"; id: string; text: string }
  | {
      kind: "tool";
      id: string;
      name: string;
      input: unknown;
      status: "running" | "ok" | "error";
      result?: string;
      output: string[];
    }
  | { kind: "error"; id: string; text: string };

export interface SessionState {
  items: Item[];
  /** 正在流式输出的正文。 */
  streaming: string;
  /** 正在流式输出的思考过程。 */
  thinking: string;
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
}

const MAX_TOOL_LINES = 200;

export function useSession(sessionId: string) {
  const [state, setState] = useState<SessionState>({
    items: [],
    streaming: "",
    thinking: "",
    busy: false,
    asks: [],
    tokens: { input: 0, output: 0 },
  });

  // delta 先攒在 ref，由 rAF 决定何时 setState。逐条 setState 会让 React
  // 在快速流式输出时掉帧。页面不可见时 WebKit 会节流 rAF，那时直接刷 ——
  // 没人在看，掉帧无所谓，但数据不能积压。
  const pendingText = useRef("");
  const pendingThinking = useRef("");
  const rafId = useRef(0);

  useEffect(() => {
    const flush = () => {
      rafId.current = 0;
      const t = pendingText.current;
      const k = pendingThinking.current;
      if (!t && !k) return;
      pendingText.current = "";
      pendingThinking.current = "";
      setState((s) => ({
        ...s,
        streaming: s.streaming + t,
        thinking: s.thinking + k,
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
          }
          // tool_input 的增量不进正文 —— 那是 JSON 片段，混进去就是乱码。
          // 工具卡片在 tool_use 完整到达时才出现。
          break;

        case "request_start":
          setState((s) => ({ ...s, busy: true }));
          break;

        case "message":
          flush();
          setState((s) => applyMessage(s, event));
          break;

        case "progress":
          setState((s) => applyProgress(s, event));
          break;

        case "permission_request":
          flush();
          setState((s) => {
            // 同一个 request_id 重复到达就不排两次 —— 事件重放（切回
            // 会话）不该让用户连答两遍同一个问题。
            if (s.asks.some((a) => a.requestId === event.request_id)) return s;
            return {
              ...s,
              asks: [...s.asks, { requestId: event.request_id, detail: event.detail }],
            };
          });
          break;

        case "permission_resolved":
          // 宿主那边这个请求已经作废（超时或被中断）。不撤掉弹窗的话，
          // 它会一直挂着，用户点"允许"也毫无反应 —— 操作早就被拒了。
          setState((s) => applyResolved(s, event.request_id, event.reason));
          break;

        case "done":
          flush();
          setState((s) => applyDone(s, event));
          break;

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
      .then((msgs) => {
        if (cancelled || msgs.length === 0) return;
        setState((s) => ({
          ...s,
          items: messagesToItems(msgs),
          tokens: sumUsage(msgs),
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

  const send = useCallback(
    async (text: string) => {
      // 立刻把用户这句话放上去。等宿主回声的话，用户会看到自己输入的
      // 内容凭空消失几百毫秒。
      setState((s) => ({
        ...s,
        busy: true,
        items: [
          ...s.items,
          { kind: "user", id: `local-${Date.now()}`, text },
        ],
      }));
      try {
        await sendTurn(sessionId, text);
      } catch (e) {
        setState((s) => ({
          ...s,
          busy: false,
          items: [...s.items, { kind: "error", id: `err-${Date.now()}`, text: String(e) }],
        }));
      }
    },
    [sessionId],
  );

  const stop = useCallback(() => void interruptSession(sessionId), [sessionId]);

  const answer = useCallback(
    async (response: PermissionResponse) => {
      const ask = state.asks[0];
      if (!ask) return;
      // 先出队，弹窗立刻切到下一个。等 IPC 往返会让按钮看起来没反应，
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

  return { ...state, send, stop, answer };
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
      for (const c of msg.content) {
        if (c.type === "text") {
          items.push({ kind: "user", id: `${msg.id}-u${items.length}`, text: c.text });
        } else if (c.type === "tool_result") {
          const i = findLast(items, (it) => it.kind === "tool" && it.id === c.tool_use_id);
          if (i >= 0) {
            const t = items[i] as Extract<Item, { kind: "tool" }>;
            items[i] = {
              ...t,
              status: c.is_error ? "error" : "ok",
              result: renderResult(c.content),
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
          items[i] = {
            ...t,
            status: c.is_error ? "error" : "ok",
            result: renderResult(c.content),
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
    items.push({
      kind: "error",
      id: `done-${Date.now()}`,
      text: `到达最大轮数（${r.limit}）。再说一句可以让它接着做。`,
    });
  }

  // 这一轮结束了，还排着队的权限请求已经没有意义 —— 它们对应的工具
  // 调用要么被中断要么已超时。留着的话，用户下一轮开口前会先被一个
  // 属于上一轮的弹窗拦住。
  return { ...s, items, busy: false, streaming: "", thinking: "", asks: [] };
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

function renderResult(c: ToolResultContent): string {
  switch (c.type) {
    case "text":
      return c.text;
    case "spilled":
      return `结果过大（${c.total_bytes} 字节），已写入 ${c.path}\n\n${c.preview}`;
    case "cleared":
      return "（历史结果已清理）";
    case "image":
      return `（${c.media_type} 图片）`;
    default:
      return "";
  }
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
