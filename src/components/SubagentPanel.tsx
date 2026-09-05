import { useEffect, useMemo, useRef, useState } from "react";

import { type BackgroundTaskView, type TaskHistory, taskCancel, taskHistory } from "../bridge";
import { type Item, messagesToItems } from "../hooks/useSession";
import { SubagentsContext } from "../lib/subagentLink";
import { StopIcon } from "./icons";
import { groupBlocks, ProcessGroup } from "./ProcessFold";
import { kindLabel, statusLabel } from "./TaskPanel";
import { Row } from "./Transcript";

/** 子 agent 还在跑时多久拉一次。它的消息不走主事件流（见 bridge.taskHistory）。 */
const POLL_MS = 1200;

/**
 * 一个子 agent 的会话（右侧抽屉的标签，照 Cursor 的子 agent 视图）。
 *
 * 只读：没有输入框。子 agent 的对话对象是主 agent，不是用户 —— 想给它
 * 追加指令是主 agent 的事（Task 的 resume），用户在主对话里说就行。
 * 这里能做的只有看过程和停掉它。
 *
 * 画法和主对话一致：同一套 messagesToItems → Row / ProcessGroup。第一条
 * user 消息是主 agent 给它的任务书，单独摆在头部当"题目"。
 */
export function SubagentPanel({
  sessionId,
  agentId,
  onTitle,
}: {
  sessionId: string;
  agentId: string;
  /** 拉到视图后把真名报回去（标签栏上可能还只是 id）。 */
  onTitle?: (title: string) => void;
}) {
  const [hist, setHist] = useState<TaskHistory | null>(null);
  const [error, setError] = useState<string | null>(null);
  const onTitleRef = useRef(onTitle);
  onTitleRef.current = onTitle;

  const task = hist?.task ?? null;
  const running = task?.status === "running";

  // 首次立刻拉；跑着就轮询，停了就不再动。轮询的每一次都是全量 ——
  // 子 agent 的会话通常几十条，一次几十 KB，比维护增量协议便宜得多。
  useEffect(() => {
    let alive = true;
    let timer: number | null = null;
    const pull = async () => {
      try {
        const h = await taskHistory(sessionId, agentId);
        if (!alive) return;
        setHist(h);
        setError(null);
        if (h.task) onTitleRef.current?.(h.task.title);
        if (h.task?.status === "running") {
          timer = window.setTimeout(() => void pull(), POLL_MS);
        }
      } catch (e) {
        if (!alive) return;
        setError(e instanceof Error ? e.message : String(e));
      }
    };
    void pull();
    return () => {
      alive = false;
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [sessionId, agentId]);

  const { prompt, items } = useMemo(() => splitPrompt(hist?.messages ?? []), [hist?.messages]);
  const blocks = useMemo(() => groupBlocks(items, running), [items, running]);

  // 跟着尾巴走：跑着的时候新消息不断长出来，用户开这个视图就是想看它
  // 在干什么。用户往上翻了就不再拽 —— 和主对话同一条规矩。
  const scrollRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  useEffect(() => {
    const el = scrollRef.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [items.length, running]);

  if (!hist) {
    return <div className="subagent-panel subagent-empty">{error ?? "加载中…"}</div>;
  }
  if (!task) {
    return (
      <div className="subagent-panel subagent-empty">
        这个子 agent 的记录已经不在内核里（内核重启后旧 id 会失效）。
        <br />
        <code>{agentId}</code>
      </div>
    );
  }

  return (
    <div className="subagent-panel">
      <SubagentHeader
        task={task}
        onStop={running ? () => void taskCancel(sessionId, agentId).catch(() => {}) : null}
      />
      <div
        ref={scrollRef}
        className="subagent-body"
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
      >
        {prompt ? (
          <div className="subagent-prompt">
            <div className="subagent-prompt-label">任务</div>
            <div className="subagent-prompt-text">{prompt}</div>
          </div>
        ) : null}
        {/* 它派出去的子 agent 给它会话里的 Task 卡片认领（嵌套一层层点下去）。 */}
        <SubagentsContext.Provider value={hist.descendants ?? []}>
          <div className="transcript-list subagent-list">
            {blocks.map((b) =>
              b.kind === "row" ? (
                <Row key={b.item.id} item={b.item} hydrate />
              ) : (
                <ProcessGroup key={b.id} items={b.items} live={b.live} />
              ),
            )}
            {running && items.length === 0 ? <div className="msg notice">正在启动…</div> : null}
          </div>
        </SubagentsContext.Provider>
      </div>
    </div>
  );
}

function SubagentHeader({
  task,
  onStop,
}: {
  task: BackgroundTaskView;
  onStop: (() => void) | null;
}) {
  const running = task.status === "running";
  return (
    <div className={`subagent-head subagent-head-${task.status}`}>
      <span className={running ? "task-dot tool-icon-spin" : "task-dot"} aria-hidden>
        {running ? "◐" : task.status === "completed" ? "✓" : "✕"}
      </span>
      <div className="subagent-head-main">
        <div className="subagent-head-title" title={task.title}>
          {task.title}
        </div>
        <div className="subagent-head-meta">
          <span className="task-kind">{kindLabel(task.kind)}</span>
          {task.background ? <span className="task-kind">后台</span> : null}
          <span>{task.model}</span>
          <span>·</span>
          <span>{statusLabel(task.status)}</span>
          {task.tool_uses > 0 ? <span>· {task.tool_uses} 步</span> : null}
          {task.tokens > 0 ? <span>· {fmtK(task.tokens)} tokens</span> : null}
          <span className="subagent-head-id" title={task.id}>
            {task.id}
          </span>
        </div>
      </div>
      {onStop ? (
        <button type="button" className="subagent-stop" onClick={onStop} title="停止这个子 agent">
          <StopIcon />
          <span>停止</span>
        </button>
      ) : null}
    </div>
  );
}

/**
 * 把第一条 user 消息里的任务书拎出来当题目，剩下的按主对话画。
 *
 * 分叉的第一条是"tool_result 补齐 + 分叉说明 + 任务"，题目取 Text 段；
 * 续接过的子 agent 历史里有多条 user 指令，后面的照常画成用户气泡 ——
 * 它们是主 agent 追加的话，出现在时间线里是对的。
 */
function splitPrompt(messages: TaskHistory["messages"]): { prompt: string; items: Item[] } {
  const first = messages[0];
  if (!first || first.role !== "user") {
    return { prompt: "", items: messagesToItems(messages) };
  }
  const prompt = first.content
    .flatMap((c) => (c.type === "text" ? [c.text] : []))
    .join("\n")
    .replace(/^任务：/, "")
    .trim();
  // 第一条里若还夹着 tool_result（分叉补齐的），留给 items 去配对不会有
  // 归宿（那些 tool_use 在父会话里），直接跳过整条。
  return { prompt, items: messagesToItems(messages.slice(1)) };
}

function fmtK(n: number): string {
  return n >= 1000 ? `${(n / 1000).toFixed(n >= 10_000 ? 0 : 1)}k` : String(n);
}
