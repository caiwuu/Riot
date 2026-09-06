import { memo, useState } from "react";

import { type BackgroundTaskStatus, type BackgroundTaskView, renderUiText } from "../bridge";
import type { Item } from "../hooks/useSession";
import { type MessageKey, t, useT } from "../i18n";
import { openSubagent } from "../lib/subagentLink";
import { Chevron } from "./Chevron";
import { StopIcon } from "./icons";
import { Markdown } from "./Markdown";
import { SmoothFold } from "./SmoothFold";

type NoticeItem = Extract<Item, { kind: "task_notice" }>;

/** 状态词。渲染时才查词，调用方所在的组件负责订阅语言。 */
export function statusLabel(s: BackgroundTaskStatus): string {
  switch (s) {
    case "running":
      return t("common.running");
    case "completed":
      return t("transcript.task.status.completed");
    case "failed":
      return t("common.failed");
    case "cancelled":
      return t("transcript.task.status.cancelled");
  }
}

export function kindLabel(kind: string): string {
  switch (kind) {
    case "explore":
      return t("transcript.task.kind.explore");
    case "fork":
      return t("transcript.task.kind.fork");
    default:
      return t("transcript.task.kind.run");
  }
}

/** 通知卡标题："后台任务 + 结束态"，各语言按整句翻。 */
function noticeLabelKey(s: BackgroundTaskStatus): MessageKey {
  switch (s) {
    case "running":
      return "transcript.taskNotice.running";
    case "completed":
      return "transcript.taskNotice.completed";
    case "failed":
      return "transcript.taskNotice.failed";
    case "cancelled":
      return "transcript.taskNotice.cancelled";
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
  const { t } = useT();
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
        <span className="task-notice-label">{t(noticeLabelKey(item.status))}</span>
        <span className="task-notice-title" title={item.title}>
          {item.title}
        </span>
        {/* 点 id 打开那个子 agent 的完整会话（不是展开汇报）。span 而不是
            button：外层已经是 button，button 套 button 是非法 HTML。 */}
        <span
          className="task-notice-id task-link"
          role="link"
          tabIndex={0}
          title={t("transcript.task.openSession")}
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
          {t("transcript.taskNotice.view")}
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
  const { t, tn } = useT();
  const [open, setOpen] = useState(true);
  const [dismissedAt, setDismissedAt] = useState<number | null>(null);
  // 只画后台的：同步子 agent 在对话流里有自己的 Task 卡片直播，再进面板是重复。
  const background = tasks.filter((x) => x.background);
  const running = background.filter((x) => x.status === "running");
  // "收起"只收结束的：记下收起那一刻，之后结束的任务照常出现。
  const shown = background.filter(
    (x) =>
      x.status === "running" ||
      dismissedAt === null ||
      (x.finished_at_ms ?? 0) > dismissedAt,
  );
  if (shown.length === 0) return null;

  const head =
    running.length > 0
      ? tn("transcript.taskPanel.running", running.length)
      : tn("transcript.taskPanel.finished", shown.length);

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
            title={t("transcript.taskPanel.dismissTitle")}
          >
            {t("transcript.taskPanel.dismiss")}
          </button>
        ) : null}
      </div>
      {open
        ? shown.map((task) => (
            <div className={`queue-row task-row task-row-${task.status}`} key={task.id}>
              <span
                className={task.status === "running" ? "task-dot tool-icon-spin" : "task-dot"}
                aria-hidden
              >
                {task.status === "running" ? "◐" : task.status === "completed" ? "✓" : "✕"}
              </span>
              <span className="task-kind">{kindLabel(task.kind)}</span>
              <button
                type="button"
                className="task-title task-link"
                title={t("transcript.task.openSession")}
                onClick={() => openSubagent(task.id, task.title)}
              >
                {task.title}
              </button>
              <span className="task-activity" title={renderUiText(task.activity)}>
                {task.status === "running" ? renderUiText(task.activity) : statusLabel(task.status)}
              </span>
              <span className="task-meta">
                {task.tool_uses > 0 ? tn("transcript.steps", task.tool_uses) : ""}
              </span>
              {task.status === "running" ? (
                <span className="queue-actions task-actions">
                  <button
                    type="button"
                    title={t("transcript.taskPanel.stopTitle")}
                    aria-label={t("common.stop")}
                    onClick={() => onCancel(task.id)}
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
