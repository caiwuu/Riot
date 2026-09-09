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

/**
 * 从 Task 工具的结果文本里捞它自己的 agent id（登记表里没有这条时的兜底）。
 *
 * 先认两处锚点 —— 同步结果末尾的脚注 `[子任务 agt_…：…]`、后台结果开头的
 * `agent id：agt_…`：汇报正文里可能提到**别的**子 agent（它自己派的那些，
 * 写成 `agent:` 链接），见谁先抓谁会抓错。和内核 `tasks::find_agent_id`
 * 同一条规则。
 */
export function agentIdFromResult(text: string | undefined): string | null {
  if (!text) return null;
  // 脚注在末尾，取最后一处。
  const footers = [...text.matchAll(SYNC_FOOTER_ID)];
  const footer = footers[footers.length - 1];
  if (footer?.[1]) return footer[1];
  const launched = BACKGROUND_LAUNCHED_ID.exec(text);
  if (launched?.[1]) return launched[1];
  return LOOSE_ID.exec(text)?.[1] ?? null;
}

// 下面两个锚点是内核 `subagent::TaskTool::call` 拼进结果文本的协议标记
// （和 `useSession.ts` 的 NOTICE_REPORT_MARK 一样），不是界面文案，不进词典。
const SYNC_FOOTER_ID = /\[子任务 (agt_[A-Za-z0-9_-]{6,})/g;
const BACKGROUND_LAUNCHED_ID = /agent id：(agt_[A-Za-z0-9_-]{6,})/;
const LOOSE_ID = /(agt_[A-Za-z0-9_-]{6,})/;
