//! 任务面板：跑轮期间钉在输入框上方，显示**最新一次** TodoWrite 的清单。
//!
//! 清单是状态不是事件 —— 模型每次更新进度都整表重传一遍，把每次调用都
//! 铺在对话流里，一个十步任务就是十张几乎相同的卡片。这里只画一份、
//! 就地更新（Cursor / Claude Code 同款）；对话流里的每次调用由 ToolCard
//! 降级成单行，点开才看当时的快照。
//!
//! 生命周期由外层（Chat）按 [`hasActiveTodos`] 控制：只在轮子跑着、
//! 且清单还有没做完的条目时挂载 —— 全部完成或轮子结束就整个消失，
//! 把输入框上方那格还给会话改动条；切回已结束的会话也不会再冒出来。
//! 它是进行时的进度条，不是要留档的结果，回看走对话流里的工具卡。

import { memo, useEffect, useMemo, useRef, useState } from "react";

import type { Item } from "../hooks/useSession";
import { Chevron } from "./Chevron";

/** TodoWrite 输入里的一项。宽松解析 —— 拿到什么画什么。 */
interface TodoEntry {
  content?: string;
  status?: string;
  activeForm?: string;
}

/**
 * items 里最后一次带完整清单的 TodoWrite。`callId` 是那次调用的
 * tool_use id —— 手动关闭按它记账（见下）。
 *
 * `[约束]` 必须跳过空清单的调用。正在流式写参数的那次调用，输入暂时
 * 只有边流边解出来的字符串字段（todos 是数组，解不出来）—— 不跳过的话，
 * 模型每次更新进度，面板都会先闪一下空白再恢复。
 */
function latest(items: Item[]): { callId: string; todos: TodoEntry[] } | null {
  for (let i = items.length - 1; i >= 0; i--) {
    const it = items[i];
    if (it?.kind !== "tool" || it.name !== "TodoWrite") continue;
    const input = it.input as Record<string, unknown>;
    if (Array.isArray(input?.todos) && input.todos.length > 0) {
      return { callId: it.id, todos: input.todos as TodoEntry[] };
    }
  }
  return null;
}

/**
 * 清单里还有没做完的活吗。外层用它决定这一格给任务面板还是改动条 ——
 * 全部 completed 的清单没有进行时价值，不该继续占着输入框上方。
 */
export function hasActiveTodos(items: Item[]): boolean {
  const found = latest(items);
  return found !== null && found.todos.some((t) => t.status !== "completed");
}

/**
 * memo 挡不住 items 引用变化（流式输出时每帧都换），真正省的是
 * 内部这次 useMemo —— 但组件本身够便宜，这里主要是和别的卡片保持一致。
 */
export const TodoPanel = memo(function TodoPanel({ items }: { items: Item[] }) {
  const found = useMemo(() => latest(items), [items]);
  /**
   * 手动关闭记的是**那一次调用的 id**，不是一个布尔。模型再次更新清单
   * （新的调用、新的 id）时面板自己回来 —— 用户关掉的是"这份已经看完的
   * 清单"，不是永久放弃这个功能。
   */
  const [dismissedAt, setDismissedAt] = useState<string | null>(null);
  /** 用户手动开合。null = 没碰过，按"完成就收起"的默认走。 */
  const [userOpen, setUserOpen] = useState<boolean | null>(null);
  const listRef = useRef<HTMLUListElement>(null);

  const todos = found?.todos ?? [];
  const done = todos.filter((t) => t.status === "completed").length;
  const doingIdx = todos.findIndex((t) => t.status === "in_progress");
  const doing = doingIdx >= 0 ? todos[doingIdx] : undefined;
  // 全部完成时自动收成一行 —— 任务结束后整列清单继续占着输入框上方，
  // 是用户明确抱怨过的（"完成了还一直挂着"）。想回看点标题再展开。
  const open = userOpen ?? (todos.length > 0 && done < todos.length);

  // 清单限了高会滚动，进度推进时把"进行中"那项带回视野 —— 20 步任务
  // 走到后半段，正在做的事不该藏在滚动条外面。
  useEffect(() => {
    const list = listRef.current;
    const item = list?.querySelector<HTMLElement>(".todo-item.in_progress");
    if (!list || !item) return;
    // 只动清单自己的 scrollTop。scrollIntoView 会沿着祖先链爬，
    // 把对话流也拽走 —— 任务推进时整页跟着跳。
    const top = item.offsetTop;
    const bottom = top + item.offsetHeight;
    if (top < list.scrollTop) list.scrollTop = top;
    else if (bottom > list.scrollTop + list.clientHeight) {
      list.scrollTop = bottom - list.clientHeight;
    }
  }, [doingIdx, done, open]);

  if (!found || found.callId === dismissedAt) return null;
  const { callId } = found;

  return (
    <div className="todo-panel">
      <div className="todo-panel-row">
        <button
          type="button"
          className="todo-panel-head"
          onClick={() => setUserOpen(!open)}
          aria-expanded={open}
        >
          <Chevron open={open} />
          <span className="todo-panel-title">
            任务 {done}/{todos.length}
          </span>
          {/* 收起时把"正在做什么"提到标题上 —— 折叠不该让进度彻底消失。
              展开时清单里那行本来就高亮着，标题再说一遍是重复。 */}
          {!open && doing ? (
            <span className="todo-panel-doing">{doing.activeForm ?? doing.content}</span>
          ) : null}
        </button>
        <button
          type="button"
          className="todo-panel-close"
          onClick={() => setDismissedAt(callId)}
          title="关闭（清单再次更新时会重新出现）"
          aria-label="关闭任务面板"
        >
          ✕
        </button>
      </div>
      {open ? (
        <ul className="todo-list" ref={listRef}>
          {/* key 用下标：清单是整表替换、条目只会在尾部增删，下标键让
              已有条目的 DOM 节点存续 —— 勾选动画靠 class 变化触发，
              节点换新的话每次更新整列都会重新闪一遍。 */}
          {todos.map((t, n) => (
            <li key={n} className={`todo-item ${t.status ?? "pending"}`}>
              <span className="todo-mark" aria-hidden>
                {t.status === "completed" ? "✓" : t.status === "in_progress" ? "◐" : "○"}
              </span>
              <span className="todo-text">
                {t.status === "in_progress" ? (t.activeForm ?? t.content) : t.content}
              </span>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
});
