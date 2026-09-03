/**
 * "打开某个子 agent 的会话"这件事的模块级入口（和 FilePreview 的
 * openFilePreview 同一个形状）。
 *
 * 调用点散在三处 —— Task 工具卡片、后台任务面板、回答里的 `agent:` 链接
 * （Markdown 渲染在 memo 过的 Row 里，再穿一个回调 prop 要动四层）——
 * 而接收方只有 App 一个：它把右侧抽屉切到那个子 agent 的标签。
 */

import { createContext } from "react";

import type { BackgroundTaskView } from "../bridge";

export interface SubagentOpenRequest {
  agentId: string;
  /** 标签栏上的名字。不知道就传 id，面板拉到视图后会换成真名。 */
  title: string;
}

let listener: ((req: SubagentOpenRequest) => void) | null = null;

export function openSubagent(agentId: string, title?: string): void {
  listener?.({ agentId, title: title?.trim() || agentId });
}

/** App 订阅（把右侧抽屉切到子 agent 标签）。返回退订函数。 */
export function subscribeSubagentOpen(cb: (req: SubagentOpenRequest) => void): () => void {
  listener = cb;
  return () => {
    if (listener === cb) listener = null;
  };
}

/**
 * 当前会话的全部子 agent 视图。Task 卡片按 `tool_use_id` 认领自己的那个，
 * 直播"标题 · 模型 · 正在做什么"。
 *
 * 用 context 而不是 prop：卡片在 memo 过的 Row 里，穿 prop 要动四层。
 */
export const SubagentsContext = createContext<BackgroundTaskView[]>([]);

/** `agent:agt_xxx` 链接的协议名。模型按 Task 工具提示词里的写法输出。 */
export const AGENT_LINK_SCHEME = "agent:";

/** 从 Task 工具的结果文本里捞 agent id（内核重启后登记表没了时的兜底）。 */
export function agentIdFromResult(text: string | undefined): string | null {
  if (!text) return null;
  const m = /(agt_[A-Za-z0-9_-]{6,})/.exec(text);
  return m?.[1] ?? null;
}
