import { memo, useState } from "react";

import type { BackgroundTaskStatus, BackgroundTaskView } from "../bridge";
import type { Item } from "../hooks/useSession";
import { openSubagent } from "../lib/subagentLink";
import { Chevron } from "./Chevron";
import { StopIcon } from "./icons";
import { Markdown } from "./Markdown";
import { SmoothFold } from "./SmoothFold";

type NoticeItem = Extract<Item, { kind: "task_notice" }>;

export function statusLabel(s: BackgroundTaskStatus): string {
  switch (s) {
    case "running":
      return "运行中";
    case "completed":
      return "完成";
    case "failed":
      return "失败";
    case "cancelled":
      return "已停止";
  }
}

export function kindLabel(kind: string): string {
  switch (kind) {
    case "explore":
      return "侦察";
    case "fork":
      return "分叉";
    default:
      return "执行";
  }
}

/**
 * 后台子 agent 的完成通知卡片。
 *
 * 模型收到的是同一份汇报，用户也该看得到 —— 但它不是用户说的话，不能
 * 画成用户气泡；也不是模型说的话。默认折叠：主 agent 被叫醒之后会接着
 * 综合，汇报原文是"证据"，要看再点开。失败的默认展开 —— 失败原因是
 * 用户当下最想知道的。
 */
export const TaskNoticeCard = memo(function TaskNoticeCard({ item }: { item: NoticeItem }) {
  const [open, setOpen] = useState(item.status === "failed");
  return (
    <div className={`task-notice task-notice-${item.status}`}>
      <button
        type="button"
        className="task-notice-head"
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
      >
        <Chevron open={open} />
        <span className={`task-notice-icon task-notice-icon-${item.status}`} aria-hidden>
          {item.status === "completed" ? "✓" : item.status === "cancelled" ? "◦" : "✕"}
        </span>
        <span className="task-notice-label">后台任务{statusLabel(item.status)}</span>
        <span className="task-notice-title" title={item.title}>
          {item.title}
        </span>
        {/* 点 id 打开那个子 agent 的完整会话（不是展开汇报）。span 而不是
            button：外层已经是 button，button 套 button 是非法 HTML。 */}
        <span
          className="task-notice-id task-link"
          role="link"
          tabIndex={0}
          title="打开这个子 agent 的会话"
          onClick={(e) => {
            e.stopPropagation();
            openSubagent(item.agentId, item.title);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.stopPropagation();
              openSubagent(item.agentId, item.title);
            }
          }}
        >
          查看会话
        </span>
      </button>
      <SmoothFold open={open}>
        <div className="task-notice-body">
          <Markdown text={item.text} breaks />
        </div>
      </SmoothFold>
    </div>
  );
});

/**
 * 后台任务面板：这个会话里在跑 / 刚跑完的子 agent。
 *
 * 和排队面板一样挂在输入框上方 —— 它们回答同一类问题："除了眼前这条
 * 回答，还有什么在进行"。运行中的一行直播它在做什么（最近调的工具、
 * 刚说的第一句），带停止键；结束的留一行状态，用户看过之后自己收掉
 * 整个面板（不逐条关：结束的任务下一条通知消息里就有汇报，面板行只是
 * 状态灯）。
 *
 * 全部结束且用户收起 → 什么都不画。有任务在跑时面板不能被收起来隐藏 ——
 * 那是用户唯一能停掉它们的地方。
 */
export function BackgroundTasksPanel({
  tasks,
  onCancel,
}: {
  tasks: BackgroundTaskView[];
  onCancel: (agentId: string) => void;
}) {
  const [open, setOpen] = useState(true);
  const [dismissedAt, setDismissedAt] = useState<number | null>(null);
  // 只画后台的：同步子 agent 在对话流里有自己的 Task 卡片直播，再进面板是重复。
  const background = tasks.filter((t) => t.background);
  const running = background.filter((t) => t.status === "running");
  // "收起"只收结束的：记下收起那一刻，之后结束的任务照常出现。
  const shown = background.filter(
    (t) =>
      t.status === "running" ||
      dismissedAt === null ||
      (t.finished_at_ms ?? 0) > dismissedAt,
  );
  if (shown.length === 0) return null;

  const head =
    running.length > 0
      ? `${running.length} 个后台任务在跑`
      : `${shown.length} 个后台任务已结束`;

  return (
    <div className="queue-panel task-panel">
      <div className="task-panel-head">
        <button type="button" className="queue-head" onClick={() => setOpen((v) => !v)}>
          <Chevron open={open} />
          {head}
        </button>
        {running.length === 0 ? (
          <button
            type="button"
            className="task-panel-dismiss"
            onClick={() => setDismissedAt(Date.now())}
            title="收起已结束的任务"
          >
            收起
          </button>
        ) : null}
      </div>
      {open
        ? shown.map((t) => (
            <div className={`queue-row task-row task-row-${t.status}`} key={t.id}>
              <span
                className={t.status === "running" ? "task-dot tool-icon-spin" : "task-dot"}
                aria-hidden
              >
                {t.status === "running" ? "◐" : t.status === "completed" ? "✓" : "✕"}
              </span>
              <span className="task-kind">{kindLabel(t.kind)}</span>
              <button
                type="button"
                className="task-title task-link"
                title="打开这个子 agent 的会话"
                onClick={() => openSubagent(t.id, t.title)}
              >
                {t.title}
              </button>
              <span className="task-activity" title={t.activity}>
                {t.status === "running" ? t.activity : statusLabel(t.status)}
              </span>
              <span className="task-meta">
                {t.tool_uses > 0 ? `${t.tool_uses} 步` : ""}
              </span>
              {t.status === "running" ? (
                <span className="queue-actions task-actions">
                  <button
                    type="button"
                    title="停止这个后台任务"
                    aria-label="停止"
                    onClick={() => onCancel(t.id)}
                  >
                    <StopIcon />
                  </button>
                </span>
              ) : null}
            </div>
          ))
        : null}
    </div>
  );
}
