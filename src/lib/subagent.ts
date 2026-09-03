/**
 * 子 agent（Task 工具）的嵌套时间线。
 *
 * 内核把子 agent 的整条事件流套在 `Progress { Nested }` 里上转（见
 * `crates/riot-kernel/src/subagent.rs`），这里把那条流折成一份挂在父
 * Task 卡片上的小时间线：子 agent 的思考、正文、每个工具调用及其参数
 * 和输出，以及正在流的半截消息。卡片拿它画出"子任务此刻在干什么"。
 *
 * 全是纯函数。和 useSession 里主时间线的 reducer 是同一套语义，但刻意
 * 简单得多 —— 主时间线要在 tool_start 时把半截正文就地落定、再在完整
 * 消息到达时去重（卡片必须排在它上面那句话的下面）；这里不需要：一条
 * 助手消息里的顺序本就是"思考 → 正文 → 工具"，直播时按这个顺序画，
 * 完整消息到了整体换成落定条目，没有去重问题。
 */

import type {
  AgentEvent,
  AgentEventDelta,
  AgentEventMessage,
  TerminalReason,
  ToolResultContent,
} from "../bridge/generated";
import { extractTopLevelStringFields } from "./partialJson";

/**
 * 一张工具卡片的数据。主时间线的 tool 条目和子时间线里的工具共用这个
 * 形状 —— ToolCard 靠它一份代码画两层。
 */
export interface ToolItemBase {
  kind: "tool";
  id: string;
  name: string;
  input: unknown;
  status: "running" | "ok" | "error";
  result?: string;
  /** 结果里的压缩图（data URL），先显示它，原图到了再换。 */
  resultImage?: string;
  /** 原图的磁盘路径，界面优先按它加载。 */
  resultImagePath?: string;
  /** 实时输出行（Bash 的 stdout 之类），只留尾部。 */
  output: string[];
}

/** 子时间线里的一条。没有 user —— 子 agent 只收一条任务书，它在卡片头部单独画。 */
export type SubItem =
  | ToolItemBase
  | { kind: "assistant"; id: string; text: string }
  | { kind: "thinking"; id: string; text: string }
  | { kind: "error"; id: string; text: string }
  | { kind: "notice"; id: string; text: string };

/**
 * 正在生成、还没落成完整消息的那条助手消息。
 *
 * 工具卡在 tool_start 时就出现（参数还一个字都没有），参数边流边填 ——
 * 和主时间线一样，Write 的整份文件在参数里，生成要几十秒，那段时间
 * 卡片上不能什么都没有。
 */
export interface SubLive {
  thinking: string;
  text: string;
  tools: ToolItemBase[];
  /** 各工具已到达的参数 JSON 片段，按 tool_use id。填卡片用的是从它抽出的字段。 */
  json: Record<string, string>;
}

export interface SubAgent {
  /** 已落定的条目，按到达顺序。 */
  items: SubItem[];
  /** 正在流的那条消息。null = 模型这一刻没在说话（在跑工具，或还没开口）。 */
  live: SubLive | null;
  /** 子 agent 实际用的模型。只读侦察配了便宜档时和主模型不同 —— 用户配完第一个想确认的就是这个。 */
  model?: string;
  /** 已经开始的模型请求轮数（从 1 数）。 */
  turns: number;
  /** 子 agent 的事件流已经结束（Done 到了）。 */
  done: boolean;
  /** 子 agent 自己的 token 用量，累计。 */
  usage: { input: number; output: number };
}

/** 和主时间线一样只留尾部：一个 build 能吐几万行。 */
const MAX_TOOL_LINES = 200;

export function emptySub(): SubAgent {
  return { items: [], live: null, turns: 0, done: false, usage: { input: 0, output: 0 } };
}

function emptyLive(): SubLive {
  return { thinking: "", text: "", tools: [], json: {} };
}

/** 把一条嵌套事件折进子时间线。不认识的事件原样返回（引用不变，memo 才挡得住）。 */
export function reduceSub(sub: SubAgent, ev: AgentEvent): SubAgent {
  switch (ev.type) {
    case "request_start":
      return { ...sub, model: ev.model, turns: ev.turn + 1 };
    case "delta":
      return reduceDelta(sub, ev);
    case "message":
      return reduceMessage(sub, ev);
    case "progress":
      // 子 agent 里的工具输出（Bash 的实时行）。再往里套的不认 —— 子 agent
      // 的注册表里没有 Task，结构上不会出现。
      if (ev.payload.kind !== "line") return sub;
      return appendOutput(sub, ev.tool_use_id, ev.payload.text);
    case "done":
      return finish(sub, ev.reason);
    default:
      return sub;
  }
}

function reduceDelta(sub: SubAgent, ev: AgentEventDelta): SubAgent {
  const live = sub.live ?? emptyLive();
  switch (ev.kind) {
    case "text":
      return { ...sub, live: { ...live, text: live.text + ev.text } };
    case "thinking":
      return { ...sub, live: { ...live, thinking: live.thinking + ev.text } };
    case "tool_start": {
      if (live.tools.some((t) => t.id === ev.tool_use_id)) return sub;
      const card: ToolItemBase = {
        kind: "tool",
        id: ev.tool_use_id,
        name: ev.name,
        input: {},
        status: "running",
        output: [],
      };
      return { ...sub, live: { ...live, tools: [...live.tools, card] } };
    }
    case "tool_input": {
      const json = (live.json[ev.tool_use_id] ?? "") + ev.partial_json;
      // 只认顶层字符串字段；数字和布尔参数等完整消息补上。
      const fields = extractTopLevelStringFields(json);
      return {
        ...sub,
        live: {
          ...live,
          json: { ...live.json, [ev.tool_use_id]: json },
          tools: live.tools.map((t) => (t.id === ev.tool_use_id ? { ...t, input: fields } : t)),
        },
      };
    }
  }
}

function reduceMessage(sub: SubAgent, msg: AgentEventMessage): SubAgent {
  if (msg.role === "assistant") {
    // 完整消息是权威的：顺序、全量参数都以它为准，直播那份整体撤下。
    const items = [...sub.items];
    for (const c of msg.content) {
      if (c.type === "thinking" && c.text.trim()) {
        items.push({ kind: "thinking", id: `${msg.id}-k${items.length}`, text: c.text });
      } else if (c.type === "text" && c.text.trim()) {
        items.push({ kind: "assistant", id: `${msg.id}-t${items.length}`, text: c.text });
      } else if (c.type === "tool_use") {
        // 直播里已有这张卡（tool_start 插的）就接着用它攒下的状态 ——
        // 正常情况下此刻工具还没开跑，这只是保险。
        const prev = sub.live?.tools.find((t) => t.id === c.id);
        items.push({
          kind: "tool",
          id: c.id,
          name: c.name,
          input: c.input,
          status: prev?.status ?? "running",
          output: prev?.output ?? [],
        });
      }
    }
    const u = msg.usage;
    const usage = u
      ? {
          input: sub.usage.input + u.input_tokens + u.cache_read_tokens + u.cache_creation_tokens,
          output: sub.usage.output + u.output_tokens,
        }
      : sub.usage;
    return { ...sub, items, live: null, usage };
  }

  if (msg.role === "user") {
    // 只看工具结果。子 agent 的 user 消息还可能是 hook 反馈之类的合成
    // 提醒 —— 那是给模型看的，不进卡片。
    let items = sub.items;
    for (const c of msg.content) {
      if (c.type !== "tool_result") continue;
      const i = findLast(items, (it) => it.kind === "tool" && it.id === c.tool_use_id);
      if (i < 0) continue;
      if (items === sub.items) items = [...items];
      const t = items[i] as ToolItemBase;
      const view = resultView(c.content);
      items[i] = {
        ...t,
        status: c.is_error ? "error" : "ok",
        ...(view.text !== undefined ? { result: view.text } : {}),
        ...(view.image !== undefined ? { resultImage: view.image } : {}),
        ...(view.imagePath !== undefined ? { resultImagePath: view.imagePath } : {}),
      };
    }
    return items === sub.items ? sub : { ...sub, items };
  }

  return { ...sub, items: [...sub.items, { kind: "error", id: msg.id, text: msg.text }] };
}

/** 工具的实时输出行。卡片可能还在直播区（工具已开跑但消息尚未落定的边角情况），两边都找。 */
function appendOutput(sub: SubAgent, toolId: string, text: string): SubAgent {
  const push = (t: ToolItemBase): ToolItemBase => ({
    ...t,
    output: [...t.output, text].slice(-MAX_TOOL_LINES),
  });
  const i = findLast(sub.items, (it) => it.kind === "tool" && it.id === toolId);
  if (i >= 0) {
    const items = [...sub.items];
    items[i] = push(items[i] as ToolItemBase);
    return { ...sub, items };
  }
  const live = sub.live;
  if (live?.tools.some((t) => t.id === toolId)) {
    return {
      ...sub,
      live: { ...live, tools: live.tools.map((t) => (t.id === toolId ? push(t) : t)) },
    };
  }
  return sub;
}

/**
 * 子 agent 的流结束了。正常结束时直播区早被完整消息清空、工具都有结果；
 * 这里兜的是中断和出错：半截消息落定成条目，还在转的工具标成未完成。
 *
 * 出错和取消不另加一行 —— 父 Task 卡片的结果里已经写着"子任务失败 / 已
 * 取消"，卡片会把它显示出来。步数上限不同：那时父结果是正常的报告，
 * 只在末尾附一句提醒，而卡片默认不重复显示报告，这句提醒得在时间线里。
 */
function finish(sub: SubAgent, reason: TerminalReason): SubAgent {
  const settled = finalizeSub(sub);
  if (reason.reason !== "max_turns") return settled;
  return {
    ...settled,
    items: [
      ...settled.items,
      {
        kind: "notice",
        id: "max-turns",
        text: `子任务跑满了 ${reason.limit} 步的上限，以上可能是未完成的结果。`,
      },
    ],
  };
}

/**
 * 收尾：直播区落定、转圈的工具标成未完成、标记已结束。
 *
 * 除了子 agent 自己的 Done，父会话结束（轮次 Done、用户停止、切回
 * 会话对账发现已空闲）时也要对每张 Task 卡片做一遍 —— 子 agent 的
 * Done 在极端时序下（宿主换出口的间隙）会丢，不能让卡片里永远转圈。
 * 已经结束的原样返回。
 */
export function finalizeSub(sub: SubAgent): SubAgent {
  if (sub.done) return sub;
  const items = [...sub.items];
  const live = sub.live;
  if (live) {
    if (live.thinking.trim()) {
      items.push({ kind: "thinking", id: `live-k${items.length}`, text: live.thinking });
    }
    if (live.text.trim()) {
      items.push({ kind: "assistant", id: `live-t${items.length}`, text: live.text });
    }
    items.push(...live.tools);
  }
  return {
    ...sub,
    live: null,
    done: true,
    items: items.map((it) =>
      it.kind === "tool" && it.status === "running"
        ? { ...it, status: "error" as const, result: it.result ?? "未完成" }
        : it,
    ),
  };
}

/** 子时间线里工具调用的个数（含直播中刚开始的）。卡片头上的"N 步"。 */
export function subToolCount(sub: SubAgent): number {
  let n = 0;
  for (const it of sub.items) if (it.kind === "tool") n++;
  return n + (sub.live?.tools.length ?? 0);
}

/** 子 agent 此刻正在跑的工具（最新那个）。父会话的状态行靠它说清"在等谁"。 */
export function subRunningTool(sub: SubAgent): ToolItemBase | null {
  const live = sub.live;
  if (live) {
    for (let i = live.tools.length - 1; i >= 0; i--) {
      const t = live.tools[i];
      if (t && t.status === "running") return t;
    }
  }
  for (let i = sub.items.length - 1; i >= 0; i--) {
    const it = sub.items[i];
    if (it && it.kind === "tool" && it.status === "running") return it;
  }
  return null;
}

/**
 * 工具结果 → 界面上显示什么。主时间线和子时间线共用。
 *
 * 图片类结果（截图、读图）显示图片本身。described_image 的 text 是写给
 * 模型的转述（带着"当作亲眼所见"之类的内部指示），**不能**摆到界面上 ——
 * 用户该看到的是那张图。marked_image 的 text 则是和图同属一个结果的
 * 正文（编号清单、MCP 结果的文本部分），图文都给用户看。
 *
 * `image` 是消息里的压缩图（data URL），`imagePath` 是落盘原图的路径。
 * 两个都给：压缩图立刻能显示，原图由组件按路径异步加载后替换。
 */
export function resultView(c: ToolResultContent): {
  text?: string;
  image?: string;
  imagePath?: string;
} {
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
