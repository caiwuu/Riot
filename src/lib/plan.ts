/**
 * 计划（规划模式的产物）在前端的形状、派生，以及"打开计划面板"的入口。
 *
 * 计划不是一条独立的会话状态：它就是对话流里最近一次 CreatePlan 调用 ——
 * 标题、概述、正文在工具输入里，文件路径在工具结果的首行。从条目里派生
 * 而不另存，切会话、重启、回退历史之后自然就是对的（条目本来就会跟着
 * 重建）。
 */

import type { Item } from "../hooks/useSession";

/** 内核里交计划的工具名（`riot_tools::tools::names::CREATE_PLAN`）。 */
export const PLAN_TOOL = "CreatePlan";

/** 旧 transcript 里的名字：计划整份在 `plan` 参数里，没有文件。 */
export const LEGACY_PLAN_TOOL = "ExitPlanMode";

/** 内核里建议换模式的工具名（`riot_tools::tools::names::SWITCH_MODE`）。 */
export const SWITCH_MODE_TOOL = "SwitchMode";

/** 待办清单的工具名（`riot_tools::tools::names::TODO_WRITE`）。 */
export const TODO_TOOL = "TodoWrite";

/**
 * CreatePlan 结果首行的前缀，后面是计划文件相对项目根的路径。
 *
 * `[约束]` 和 `crates/riot-tools/src/tools/plan.rs` 的 `PLAN_FILE_LINE_PREFIX`
 * 对齐。工具的 ui_payload 到不了前端，路径又是内核现编的（标题 + 调用
 * id），两边只能约定一个可解析的行。改一边必须改另一边。
 */
const PLAN_FILE_LINE_PREFIX = "Plan file: ";

export interface PlanView {
  /** 那次 CreatePlan 调用的 tool_use id。变了就是一份新计划。 */
  id: string;
  name: string;
  overview: string;
  /** 工具输入里的正文（模型写下的那份）。文件读不到时的兜底。 */
  body: string;
  /** 计划文件相对项目根的路径。工具还没落定、或结果里解析不出时为 null。 */
  path: string | null;
  status: "running" | "ok" | "error";
  /** 计划自带的待办措辞（CreatePlan 的 `todos[].content`），按顺序。
   *  没给就是空 —— 旧计划、纯调研的计划都没有。 */
  todos: string[];
}

/** 计划面板进度区里的一项。状态归并成清单的三态，认不出的算 pending。 */
export interface PlanTodoView {
  content: string;
  status: "pending" | "in_progress" | "completed";
  /** 进行式措辞（TodoWrite 的 activeForm）。计划初稿里没有。 */
  activeForm?: string;
}

/** 一个会话的计划状态，由 Chat 上报给 App（右侧抽屉的面板画它）。 */
export interface PlanState {
  plan: PlanView | null;
  /** 正在流的计划正文；不在流时为 null。 */
  streaming: string | null;
  /** 输入框那边此刻有没有「构建」键（规划模式、空闲、计划已落定）。 */
  canBuild: boolean;
  /** 本会话落盘的 Edit / Write 次数。面板据此重读计划文件 —— 用户提了
   *  意见，模型改的是文件，不刷新的话面板还停在旧版。 */
  edits: number;
  /** 计划的待办及其进度（见 [`planTodos`]）。 */
  todos: PlanTodoView[];
}

/**
 * 两份计划视图内容相同。`latestPlan` 每次对话条目变化都产出新对象（流式
 * 期间每帧一次），按内容比才能让 App 那头的状态不跟着每帧翻一遍。
 */
export function samePlan(a: PlanView | null, b: PlanView | null): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  return (
    a.id === b.id &&
    a.status === b.status &&
    a.path === b.path &&
    a.name === b.name &&
    a.overview === b.overview &&
    a.body === b.body &&
    a.todos.length === b.todos.length &&
    a.todos.every((t, i) => t === b.todos[i])
  );
}

/** 两份进度内容相同。理由同 `samePlan`。 */
export function sameTodos(a: PlanTodoView[], b: PlanTodoView[]): boolean {
  if (a === b) return true;
  return (
    a.length === b.length &&
    a.every((t, i) => {
      const o = b[i];
      return (
        o !== undefined &&
        t.content === o.content &&
        t.status === o.status &&
        t.activeForm === o.activeForm
      );
    })
  );
}

/** 计划自带的待办措辞。宽松解析：拿到什么算什么，空的跳过。 */
function todosOf(input: Record<string, unknown> | null): string[] {
  const raw = input?.todos;
  if (!Array.isArray(raw)) return [];
  const out: string[] = [];
  for (const t of raw) {
    const c = (t as { content?: unknown } | null)?.content;
    if (typeof c === "string" && c.trim()) out.push(c.trim());
  }
  return out;
}

/**
 * 计划的待办及其进度。
 *
 * 和内核 `riot_kernel::plan` 同一条规则：计划的待办是清单的**初稿**，直到
 * 模型开始用 TodoWrite 跟踪 —— 计划之后最近一次带清单的 TodoWrite 就是
 * 进度（构建时模型照着计划的待办、同一措辞落成清单，之后逐项翻状态）；
 * 还没有的话，初稿全部 pending。不按措辞逐条配对：模型改了措辞、加了一
 * 项，面板照样显示它真正在跟踪的那份，而不是一列永远 pending 的初稿。
 * 计划**之前**的 TodoWrite 是别的活的清单，不算。
 */
export function planTodos(items: Item[], plan: PlanView | null): PlanTodoView[] {
  if (!plan) return [];
  const at = items.findIndex((it) => it.kind === "tool" && it.id === plan.id);
  for (let i = items.length - 1; i > at; i--) {
    const it = items[i];
    if (!it || it.kind !== "tool" || it.name !== TODO_TOOL) continue;
    const raw = (it.input as { todos?: unknown } | null)?.todos;
    // 正在流式写参数的那次调用，数组还解不出来 —— 跳过，别让面板闪一下空白。
    if (!Array.isArray(raw) || raw.length === 0) continue;
    return raw.flatMap((t) => {
      const e = t as { content?: unknown; status?: unknown; activeForm?: unknown } | null;
      if (typeof e?.content !== "string") return [];
      const status =
        e.status === "completed" || e.status === "in_progress" ? e.status : "pending";
      return [
        {
          content: e.content,
          status,
          ...(typeof e.activeForm === "string" ? { activeForm: e.activeForm } : {}),
        },
      ];
    });
  }
  return plan.todos.map((content) => ({ content, status: "pending" as const }));
}

/** 从 CreatePlan 的结果文本里取计划文件路径。 */
export function planPathFromResult(result: string | undefined): string | null {
  const first = result?.split("\n", 1)[0]?.trim();
  if (!first?.startsWith(PLAN_FILE_LINE_PREFIX)) return null;
  const p = first.slice(PLAN_FILE_LINE_PREFIX.length).trim();
  return p || null;
}

/**
 * 对话里最近的那份计划。没有就是 null。
 *
 * 只认新工具：旧 transcript 里的 ExitPlanMode 条目在工具卡里还能展开看
 * 正文，但不再驱动侧栏面板和「构建」键 —— 那套批准流程已经不存在了。
 */
export function latestPlan(items: Item[]): PlanView | null {
  for (let i = items.length - 1; i >= 0; i--) {
    const it = items[i];
    if (!it || it.kind !== "tool" || it.name !== PLAN_TOOL) continue;
    const input = it.input as Record<string, unknown> | null;
    const str = (k: string) => (typeof input?.[k] === "string" ? (input[k] as string) : "");
    return {
      id: it.id,
      name: str("name").trim(),
      overview: str("overview").trim(),
      body: str("plan"),
      path: it.status === "ok" ? planPathFromResult(it.result) : null,
      status: it.status,
      todos: todosOf(input),
    };
  }
  return null;
}

let listener: (() => void) | null = null;

/** 把右侧抽屉切到「计划」标签。对话流里的计划卡点它。 */
export function openPlanPanel(): void {
  listener?.();
}

/** App 订阅。返回退订函数。 */
export function subscribePlanOpen(cb: () => void): () => void {
  listener = cb;
  return () => {
    if (listener === cb) listener = null;
  };
}
