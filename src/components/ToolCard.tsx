import { memo, startTransition, useContext, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import { type BackgroundTaskView, readImage, renderUiText } from "../bridge";
import type { Item } from "../hooks/useSession";
import { t, tn, useT } from "../i18n";
import { LEGACY_PLAN_TOOL, openPlanPanel, PLAN_TOOL, SWITCH_MODE_TOOL } from "../lib/plan";
import { agentIdFromResult, openSubagent, SubagentsContext } from "../lib/subagentLink";
import { Chevron } from "./Chevron";
import { PlanModeIcon } from "./icons";
import { useEscLayer } from "./Modal";
import { SmoothFold } from "./SmoothFold";

type Tool = Extract<Item, { kind: "tool" }>;

/** TodoWrite 输入里的一项。宽松解析 —— 界面拿到什么画什么。 */
interface TodoInput {
  content?: string;
  status?: string;
  activeForm?: string;
}

function todosOf(tool: Tool): TodoInput[] {
  const i = tool.input as Record<string, unknown>;
  return Array.isArray(i?.todos) ? (i.todos as TodoInput[]) : [];
}

/** 子 agent 结束态的一行：状态词，有步数就带上。 */
function finishedLabel(status: BackgroundTaskView["status"], toolUses: number): string {
  const word =
    status === "completed"
      ? t("transcript.task.status.completed")
      : status === "cancelled"
        ? t("transcript.task.status.cancelled")
        : t("common.failed");
  return toolUses ? `${word} · ${tn("transcript.steps", toolUses)}` : word;
}

/**
 * 一次工具调用。
 *
 * 默认折叠。展开的话，一次 `cargo build` 的输出就会把整个对话冲走 ——
 * 用户来这里是看模型在干什么，不是读日志。摘要行说清"做了什么、成没成"，
 * 想看细节再点开。
 *
 * memo：流式输出时 transcript 每帧重渲染，历史工具卡片不该跟着刷。
 */
export const ToolCard = memo(function ToolCard({
  tool,
  eager = false,
}: {
  tool: Tool;
  /**
   * 贴底的那几条立刻画详情。其余的等进视野再挂 —— Edit/Write 默认
   * 展开，长会话里几十份 diff 一起进 DOM 会把切回卡成白屏。
   */
  eager?: boolean;
}) {
  // 子 agent 有自己的卡：不折叠、不展开输出，点开是它的整个会话。
  if (tool.name === "Task") return <TaskCard tool={tool} />;
  // 计划也有自己的卡：标题 + 概述，点开是右侧抽屉里的计划面板。
  if (tool.name === PLAN_TOOL) return <PlanCard tool={tool} />;
  return <PlainToolCard tool={tool} eager={eager} />;
});

/**
 * CreatePlan 的卡片（照 Cursor：对话里一张计划卡，正文在旁边的面板）。
 *
 * 卡上只放标题和一两句概述 —— 计划是要读的文档，几页纸铺进对话流会把
 * 对话本身冲走；整张卡是打开计划面板的入口。撰写中显示"正在撰写"，
 * 面板那边同步在流。
 */
const PlanCard = memo(function PlanCard({ tool }: { tool: Tool }) {
  const { t } = useT();
  const i = tool.input as Record<string, unknown>;
  const str = (k: string) => (typeof i?.[k] === "string" ? (i[k] as string) : "");
  const name = str("name").trim() || t("transcript.plan.untitled");
  const overview = str("overview").trim();
  const running = tool.status === "running";
  return (
    <div className={`tool tool-${tool.status} tool-plan`}>
      <button
        type="button"
        className="tool-head plan-tool-head"
        onClick={openPlanPanel}
        title={t("transcript.plan.open")}
      >
        <span className="plan-tool-icon" aria-hidden>
          <PlanModeIcon />
        </span>
        <span className="plan-tool-main">
          <span className="plan-tool-badge">
            {running ? t("transcript.plan.drafting") : t("transcript.plan.badge")}
            {running ? <span className="plan-caret" aria-hidden /> : null}
          </span>
          <span className="plan-tool-title">{name}</span>
        </span>
        {/* 概述另起一行、从图标那一列起排：它是整段话，不是标题的附注，
            缩进到标题下面会在图标底下空出一列。 */}
        {overview ? <span className="plan-tool-overview">{overview}</span> : null}
        {tool.status === "error" ? (
          <span className="plan-tool-overview tool-fail">{tool.result ?? t("common.failed")}</span>
        ) : null}
        <span className="task-card-go" aria-hidden>
          ›
        </span>
      </button>
    </div>
  );
});

/**
 * Task 工具的卡片（照 Cursor）：一行"标题 · 模型"，下面一行它此刻在做
 * 什么；整张卡是一个链接，点开右侧抽屉里它的完整会话。
 *
 * 数据来自会话的子 agent 登记表（SubagentsContext，按 tool_use_id 认领），
 * 卡片本身只知道输入参数。登记表跟着会话落盘，重启后照样认得；实在没有
 * 它（记录文件被删了、当时一条消息都没写下）就退回参数里的描述，agent id
 * 从结果文本里捞 —— 点开会看到"找不到记录"，但至少知道它存在过。
 */
const TaskCard = memo(function TaskCard({ tool }: { tool: Tool }) {
  const { t } = useT();
  const tasks = useContext(SubagentsContext);
  const task: BackgroundTaskView | undefined = tasks.find((x) => x.tool_use_id === tool.id);
  const i = tool.input as Record<string, unknown>;
  const str = (k: string) => (typeof i?.[k] === "string" ? (i[k] as string) : "");
  const title = task?.title || str("description") || t("transcript.task.subtask");
  const agentId = task?.id ?? agentIdFromResult(tool.result) ?? null;
  const resume = str("resume");
  const background = task?.background ?? (i?.run_in_background === true || resume === "self");
  const kind = task?.kind ?? (resume === "self" ? "fork" : str("subagent_type") || "general-purpose");
  // 运行状态以登记表为准（后台任务的卡片早就 ok 了，任务还在跑）；没有
  // 登记表时看卡片自己。
  const status: "running" | "ok" | "error" = task
    ? task.status === "running"
      ? "running"
      : task.status === "completed"
        ? "ok"
        : "error"
    : tool.status;
  const activity =
    status === "running"
      ? (task ? renderUiText(task.activity) : "") ||
        tool.output[tool.output.length - 1] ||
        t("transcript.task.starting")
      : task
        ? finishedLabel(task.status, task.tool_uses)
        : tool.status === "error"
          ? t("common.failed")
          : t("transcript.task.status.completed");

  const open = () => {
    if (agentId) openSubagent(agentId, title);
  };
  // 它派出去的子 agent，缩进挂在它下面（照 Cursor 的树形）。只挂直接
  // 孩子：孙子挂在孩子那一行下面，点进孩子的会话看。
  const children = agentId ? tasks.filter((x) => x.parent === agentId) : [];

  return (
    <div className={`tool tool-${status} tool-task`}>
      <button
        type="button"
        className="tool-head task-card-head"
        onClick={open}
        disabled={!agentId}
        title={agentId ? t("transcript.task.openSession") : t("transcript.task.noId")}
      >
        <span className={status === "running" ? "tool-icon tool-icon-spin" : "tool-icon"}>
          {status === "running" ? "◐" : status === "ok" ? "✓" : "✕"}
        </span>
        <span className="task-card-title">{title}</span>
        {task?.model ? <span className="task-card-model">{task.model}</span> : null}
        <span className="task-card-tags">
          <span className="task-kind">
            {kind === "explore"
              ? t("transcript.task.kind.explore")
              : kind === "fork"
                ? t("transcript.task.kind.fork")
                : resume
                  ? t("transcript.task.kind.resume")
                  : t("transcript.task.kind.run")}
          </span>
          {background ? <span className="task-kind">{t("transcript.task.background")}</span> : null}
        </span>
        {agentId ? <span className="task-card-go" aria-hidden>›</span> : null}
      </button>
      <div className="task-card-activity" title={activity}>
        {activity}
      </div>
      {children.length > 0 ? (
        <div className="task-card-children">
          {children.map((c) => (
            <ChildTaskRow key={c.id} task={c} tasks={tasks} depth={1} />
          ))}
        </div>
      ) : null}
    </div>
  );
});

/**
 * 子 agent 派的子 agent：一行标题 · 模型，下面一行动作，再往下递归挂它
 * 自己的孩子。整行可点，打开那个子 agent 的会话。
 */
function ChildTaskRow({
  task,
  tasks,
  depth,
}: {
  task: BackgroundTaskView;
  tasks: BackgroundTaskView[];
  depth: number;
}) {
  const { t } = useT();
  const running = task.status === "running";
  const icon = running ? "◐" : task.status === "completed" ? "✓" : "✕";
  const activity = running
    ? renderUiText(task.activity) || t("transcript.task.starting")
    : finishedLabel(task.status, task.tool_uses);
  const grandchildren = tasks.filter((x) => x.parent === task.id);
  return (
    <div className={`task-child task-child-${task.status}`}>
      <button
        type="button"
        className="task-child-head"
        onClick={() => openSubagent(task.id, task.title)}
        title={t("transcript.task.openSession")}
      >
        <span className={running ? "tool-icon tool-icon-spin" : "tool-icon"}>{icon}</span>
        <span className="task-card-title">{task.title}</span>
        <span className="task-card-model">{task.model}</span>
        <span className="task-card-tags">
          <span className="task-kind">
            {task.kind === "explore"
              ? t("transcript.task.kind.explore")
              : t("transcript.task.kind.run")}
          </span>
        </span>
        <span className="task-card-go" aria-hidden>
          ›
        </span>
      </button>
      <div className="task-card-activity" title={activity}>
        {activity}
      </div>
      {grandchildren.length > 0 && depth < 3 ? (
        <div className="task-card-children">
          {grandchildren.map((g) => (
            <ChildTaskRow key={g.id} task={g} tasks={tasks} depth={depth + 1} />
          ))}
        </div>
      ) : null}
    </div>
  );
}

const PlainToolCard = memo(function PlainToolCard({
  tool,
  eager = false,
}: {
  tool: Tool;
  eager?: boolean;
}) {
  const { t } = useT();
  const [userToggle, setUserToggle] = useState<boolean | null>(null);
  const cardRef = useRef<HTMLDivElement>(null);
  const [near, setNear] = useState(eager);
  // 图片结果（截图、读图）默认展开：截图的意义就是给人看，藏在"展开"
  // 后面的话用户不知道图已经在这里了，会转头让模型"把图贴出来"。
  // Edit / Write 也默认展开：用户要看的就是改了什么、写了什么。Delete 同理，
  // 一个文件没了不该藏在折叠后面。
  // 文本结果维持默认折叠 —— 一次 cargo build 的输出会把对话冲走。
  //
  // TodoWrite 刻意**不**默认展开：进度由输入框上方的常驻面板就地更新，
  // 对话流里的每次调用只是历史快照 —— 都展开的话，一个十步任务会在
  // 对话里铺十张几乎相同的清单。摘要行说清"几之几、正在干什么"，
  // 点开看的是"那一刻清单长什么样"。
  const open =
    userToggle ??
    (Boolean(tool.resultImage || tool.resultImagePath) ||
      tool.name === "Edit" ||
      tool.name === "Write" ||
      tool.name === "Delete");
  const detail = hasDetail(tool);
  const summary = summarize(tool);

  useEffect(() => {
    if (near) return;
    const el = cardRef.current;
    if (!el) return;
    const root = el.closest(".transcript");
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          io.disconnect();
          startTransition(() => setNear(true));
        }
      },
      { root: root instanceof Element ? root : null, rootMargin: "800px 0px" },
    );
    io.observe(el);
    return () => io.disconnect();
  }, [near]);

  useEffect(() => {
    if (eager) setNear(true);
  }, [eager]);

  // 摘要行被 CSS 截成一行，title 兜底让全文悬停可见 —— 长 Bash 命令
  // 在展开区还有完整版（见 renderDetail）。
  const head = (
    <>
      {/* 运行中转起来 —— 静止的 ◐ 和卡死看起来一模一样 */}
      <span className={tool.status === "running" ? "tool-icon tool-icon-spin" : "tool-icon"}>
        {icon(tool.status)}
      </span>
      <span className="tool-name">{tool.name}</span>
      {/* 失败不能只靠 12px 图标变红 —— 扫视时根本发现不了 */}
      {tool.status === "error" ? <span className="tool-fail">{t("common.failed")}</span> : null}
      <span className="tool-summary" title={summary}>
        {summary}
      </span>
    </>
  );

  return (
    <div ref={cardRef} className={`tool tool-${tool.status}`}>
      {detail ? (
        <button
          className="tool-head"
          onClick={() => {
            setNear(true);
            setUserToggle(!open);
          }}
          type="button"
          aria-expanded={open}
        >
          {head}
          <Chevron open={open} />
        </button>
      ) : (
        // 没有详情就别渲染成按钮 —— 可点但点了毫无反应，比不可点更糟
        <div className="tool-head">{head}</div>
      )}

      {detail ? (
        <SmoothFold open={open && near}>
          <div className="tool-detail">{renderDetail(tool)}</div>
        </SmoothFold>
      ) : null}
    </div>
  );
});

function icon(s: Tool["status"]): string {
  if (s === "running") return "◐";
  if (s === "ok") return "✓";
  return "✕";
}

/** 一行说清这次调用在做什么。参数原样 dump 没人看得下去。
 *  导出给过程组的直播头复用 —— 组头滚的就是这句。 */
export function summarize(tool: Tool): string {
  const i = tool.input as Record<string, unknown>;
  const str = (k: string) => (typeof i?.[k] === "string" ? (i[k] as string) : "");
  const num = (k: string) => (typeof i?.[k] === "number" ? (i[k] as number) : 0);
  // 点击/输入的三种定位（ref/selector/text）挑给出的那个显示。
  const target = (textKey = "text") => {
    if (typeof i?.ref === "number") return `[${i.ref as number}]`;
    if (str("selector")) return `\`${str("selector")}\``;
    if (str(textKey)) return `“${str(textKey)}”`;
    return t("transcript.tool.element");
  };

  switch (tool.name) {
    case "Task": {
      const resume = str("resume");
      const bg = i?.run_in_background === true || resume === "self";
      const what =
        resume === "self"
          ? t("transcript.task.kind.fork")
          : resume
            ? t("transcript.tool.task.resume", { id: resume })
            : (str("subagent_type") || "general-purpose") === "explore"
              ? t("transcript.task.kind.explore")
              : t("transcript.task.kind.run");
      const bgTag = bg ? ` · ${t("transcript.task.background")}` : "";
      return `${what}${bgTag} · ${str("description") || t("transcript.task.subtask")}`;
    }
    case "TodoWrite": {
      const todos = todosOf(tool);
      const done = todos.filter((x) => x.status === "completed").length;
      const doing = todos.find((x) => x.status === "in_progress");
      const progress = t("transcript.tool.todo.progress", { done, total: todos.length });
      return `${progress}${doing?.activeForm ? ` · ${doing.activeForm}` : ""}`;
    }
    case "Bash":
      return str("command");
    // 计划有自己的卡（PlanCard）；这条只给过程组的直播头和旧 transcript
    // 里的 ExitPlanMode 用。不写的话会落到 default，把整份计划 dump 进摘要行。
    case PLAN_TOOL:
    case LEGACY_PLAN_TOOL:
      return str("name").trim() || t("transcript.tool.writingPlan");
    case SWITCH_MODE_TOOL:
      return t("transcript.tool.switchMode", { mode: str("target_mode_id") || "?" });
    case "AskUserQuestion":
      return str("question") || t("transcript.tool.question");
    case "Read":
    case "Write":
    case "Edit":
    case "Delete":
    case "PreviewFile":
      return short(str("path") || str("file_path"));
    case "Grep":
      return str("path")
        ? t("transcript.tool.grepIn", { pattern: str("pattern"), path: short(str("path")) })
        : str("pattern");
    // 不带参数。不写这条会落到 default，摘要行是空的。
    case "ShowBrowser":
      return t("transcript.tool.showBrowser");
    case "BrowserNavigate":
      return short(str("url"), 80);
    case "BrowserClick": {
      const key =
        i?.double === true
          ? "transcript.tool.click.double"
          : i?.right === true
            ? "transcript.tool.click.right"
            : "transcript.tool.click.single";
      return t(key, { target: target() });
    }
    case "BrowserType": {
      const typed = t("transcript.tool.type", {
        target: target("target_text"),
        text: clip(str("text"), 40),
      });
      return `${typed}${i?.submit === true ? " ⏎" : ""}`;
    }
    case "BrowserKey":
      return t("transcript.tool.key", { key: str("key") });
    case "BrowserScroll": {
      const d = num("delta_y");
      return d < 0
        ? t("transcript.tool.scrollUp", { px: Math.round(-d) })
        : t("transcript.tool.scrollDown", { px: Math.round(d) });
    }
    case "BrowserHover":
      return t("transcript.tool.hover", { target: target() });
    case "BrowserSelect":
      return t("transcript.tool.select", { target: target(), value: str("value") });
    case "BrowserDrag":
      return t("transcript.tool.drag");
    case "BrowserWaitFor": {
      if (str("selector")) return t("transcript.tool.wait.appear", { selector: str("selector") });
      if (str("selector_gone")) {
        return t("transcript.tool.wait.gone", { selector: str("selector_gone") });
      }
      if (str("text")) return t("transcript.tool.wait.text", { text: str("text") });
      if (str("url_contains")) return t("transcript.tool.wait.url", { text: str("url_contains") });
      if (i?.network_idle === true) return t("transcript.tool.wait.networkIdle");
      return t("transcript.tool.wait.generic");
    }
    case "BrowserGo": {
      const dir = str("direction");
      return dir === "back"
        ? t("transcript.tool.go.back")
        : dir === "forward"
          ? t("transcript.tool.go.forward")
          : dir === "reload"
            ? t("common.refresh")
            : t("transcript.tool.go.history");
    }
    case "BrowserTabs":
      return t("transcript.tool.tabs", { action: str("action") || "list" });
    case "BrowserEvaluate":
      return t("transcript.tool.evaluate", { expr: clip(str("expression"), 60) });
    case "BrowserCookies":
      return t("transcript.tool.cookies");
    case "BrowserNetwork": {
      const head = t("transcript.tool.network", { action: str("action") || "list" });
      return `${head}${str("filter") ? ` (${str("filter")})` : ""}`;
    }
    case "BrowserReplay":
      return t("transcript.tool.replay", {
        method: str("method") || "GET",
        url: short(str("url"), 60),
      });
    case "BrowserIntercept": {
      const head = t("transcript.tool.intercept", { action: str("action") });
      return `${head}${str("url_pattern") ? ` \`${str("url_pattern")}\`` : ""}`;
    }
    case "BrowserSecrets":
      return t("transcript.tool.secrets");
    case "BrowserDiscover":
      return t("transcript.tool.discover");
    case "BrowserFuzz":
      return `fuzz ${short(str("url"), 60)}`;
    case "BrowserUpload": {
      const n = Array.isArray(i?.paths) ? (i.paths as unknown[]).length : 0;
      return tn("transcript.tool.upload", n);
    }
    case "BrowserCrawl":
      return t("transcript.tool.crawl", { url: short(str("url"), 60) });
    case "BrowserReport": {
      const n = Array.isArray(i?.findings) ? (i.findings as unknown[]).length : 0;
      return tn("transcript.tool.report", n);
    }
    default:
      return clip(
        Object.entries(i ?? {})
          .map(([k, v]) => `${k}=${typeof v === "string" ? v : JSON.stringify(v)}`)
          .join(" "),
        120,
      );
  }
}

/** 超长才截，截了要看得出来 —— 没有省略号的硬截像话说了一半。 */
function clip(s: string, max: number): string {
  return s.length <= max ? s : `${s.slice(0, max - 1)}…`;
}

/** 有没有值得展开的内容。只读字段，不建 React 树。 */
function hasDetail(tool: Tool): boolean {
  if (tool.name === "TodoWrite") return todosOf(tool).length > 0;
  if (tool.resultImage || tool.resultImagePath) return true;
  if (tool.output.length > 0) return true;
  if (tool.status !== "running" && tool.result) return true;
  const i = tool.input as Record<string, unknown>;
  if (tool.name === "Bash" && typeof i.command === "string" && i.command) return true;
  if (tool.name === "Edit" && (i.old_string || i.new_string)) return true;
  if (tool.name === "Write" && typeof i.content === "string" && i.content) return true;
  return false;
}

/**
 * 展开后的内容。按工具语义渲染，不是 JSON dump：
 * Edit 给 diff，Write 给内容预览，Bash 给实时输出或结果。
 */
function renderDetail(tool: Tool): React.ReactNode {
  const i = tool.input as Record<string, unknown>;
  const str = (k: string) => (typeof i?.[k] === "string" ? (i[k] as string) : "");

  // 任务清单：从 tool_use 的输入渲染（清单在输入里；结果只是一句固定
  // 确认，显示它反而是噪音）。
  if (tool.name === "TodoWrite") {
    const todos = todosOf(tool);
    if (todos.length === 0) return null;
    return (
      <ul className="todo-list">
        {todos.map((x, n) => (
          <li key={n} className={`todo-item ${x.status ?? "pending"}`}>
            <span className="todo-mark" aria-hidden>
              {x.status === "completed" ? "✓" : x.status === "in_progress" ? "◐" : "○"}
            </span>
            <span className="todo-text">
              {x.status === "in_progress" ? (x.activeForm ?? x.content) : x.content}
            </span>
          </li>
        ))}
      </ul>
    );
  }

  const parts: React.ReactNode[] = [];

  // 长命令的摘要行被截断，全文在这里 —— 审计的核心信息不能在界面上无处可看。
  if (tool.name === "Bash") {
    const cmd = str("command");
    if (cmd) {
      parts.push(
        <pre key="cmd" className="tool-body tool-cmd">
          {cmd}
        </pre>,
      );
    }
  }

  if (tool.name === "Edit") {
    const oldS = str("old_string");
    const newS = str("new_string");
    if (oldS || newS) {
      parts.push(
        <pre key="diff" className="tool-body tool-diff">
          {oldS.split("\n").map((l, n) => (
            <div key={`o${n}`} className="del">
              - {l}
            </div>
          ))}
          {newS.split("\n").map((l, n) => (
            <div key={`n${n}`} className="add">
              + {l}
            </div>
          ))}
        </pre>,
      );
    }
  } else if (tool.name === "Write") {
    const content = str("content");
    if (content) {
      const lines = content.split("\n");
      // 写的过程中跟着尾巴走：定在开头的话，写到第 100 行时画面已经
      // 十几秒没动过了，和卡住一样。落定之后回到开头 —— 那时用户要
      // 确认的是"写了个什么东西"，不是逐行审阅。
      const live = tool.status === "running";
      const from = live ? Math.max(0, lines.length - 30) : 0;
      parts.push(
        <pre key="w" className="tool-body">
          {from > 0 ? `${tn("transcript.tool.write.skipped", from)}\n` : ""}
          {lines.slice(from, from + 30).join("\n")}
          {!live && lines.length > 30
            ? `\n${tn("transcript.tool.write.total", lines.length)}`
            : ""}
        </pre>,
      );
    }
  }

  // 结果里的图（截图、读图）贴出来 —— 这就是用户点开想看的东西
  if (tool.resultImage || tool.resultImagePath) {
    parts.push(
      <ShotImage
        key="img"
        alt={t("transcript.tool.resultImageOf", { tool: tool.name })}
        {...(tool.resultImagePath !== undefined ? { path: tool.resultImagePath } : {})}
        {...(tool.resultImage !== undefined ? { fallback: tool.resultImage } : {})}
      />,
    );
  }

  // 运行中显示实时输出；结束后最终结果更权威（实时行只是进度侧影）
  const live = tool.output.length > 0 ? tool.output.join("\n") : "";
  const result = tool.status === "running" ? live : tool.result || live;
  if (result) {
    parts.push(
      <pre key="r" className="tool-body">
        {result}
      </pre>,
    );
  }

  return parts.length ? parts : null;
}

/**
 * 工具结果里的图。
 *
 * 消息里只带压缩图（给模型的那份，看布局够、看文字糊），原图在磁盘上 ——
 * 先显示压缩图占住位置，按路径把原图读回来后无缝换上。原图读不回来
 * （被清理、超上限）就一直用压缩图，不报错:图能看就行。
 *
 * 整页截图是极端长图，卡片里按容器宽显示、限高纵向滚；点击开查看器
 * 看大图。
 */
function ShotImage({
  path,
  fallback,
  // 每张结果图都叫"工具结果图片"的话，读屏用户分不清哪张是哪次调用的
  alt: altProp,
}: {
  path?: string;
  fallback?: string;
  alt?: string;
}) {
  const { t } = useT();
  const alt = altProp ?? t("transcript.tool.resultImage");
  const [src, setSrc] = useState<string | undefined>(fallback);
  const [viewer, setViewer] = useState(false);

  useEffect(() => {
    if (!path) return;
    let alive = true;
    readImage(path)
      .then((img) => {
        if (alive) setSrc(`data:${img.mediaType};base64,${img.data}`);
      })
      .catch(() => {
        // 原图没了就用压缩图，什么都没有才隐藏。
      });
    return () => {
      alive = false;
    };
  }, [path]);

  if (!src) return null;
  return (
    <>
      {/* button 包一层：键盘可达 + 读屏知道可点开大图，不再是"只能鼠标点" */}
      <button
        type="button"
        className="tool-shot-wrap"
        onClick={() => setViewer(true)}
        aria-label={t("transcript.image.zoomNamed", { name: alt })}
      >
        <img className="tool-shot" src={src} alt={alt} />
      </button>
      {viewer ? <ShotViewer src={src} alt={alt} onClose={() => setViewer(false)} /> : null}
    </>
  );
}

/**
 * 图片查看器:全屏遮罩，图在视口里上下左右居中；超大的缩到视口内。
 *
 * portal 到 body —— 卡片在带 overflow 的滚动容器里，fixed 遮罩留在原地
 * 会被裁掉。导出给聊天区图片和磁盘图片文件的放大查看共用。
 */
export function ShotViewer({
  src,
  alt: altProp,
  onClose,
}: {
  src: string;
  alt?: string;
  onClose: () => void;
}) {
  const { t } = useT();
  const alt = altProp ?? t("transcript.tool.resultImage");
  // Esc 走公共栈 —— 查看器开在权限卡之上时，Esc 只关查看器，
  // 不会顺手把底下的权限请求也拒了。
  useEscLayer(onClose);

  return createPortal(
    // 点空白处（遮罩本身）关闭；点图不关，方便拖滚动条。
    <div
      className="shot-viewer"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <button
        className="shot-viewer-close"
        onClick={onClose}
        type="button"
        aria-label={t("common.close")}
      >
        <svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true">
          <path
            d="M2 2l8 8M10 2L2 10"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
          />
        </svg>
      </button>
      <img src={src} alt={t("transcript.image.original", { name: alt })} />
    </div>,
    document.body,
  );
}

/** 长路径留尾部 —— 文件名比目录前缀有用得多。 */
function short(p: string, max = 48): string {
  return p.length <= max ? p : `…${p.slice(-(max - 1))}`;
}
