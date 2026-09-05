/**
 * 定时任务页（主区，Codex 的「已安排」同款交互）：
 *
 * - 左列：过滤 tab + 搜索 + 任务列表 + 建议。点行在**右侧打开详情**。
 * - 右列（详情面板）：状态与操作在顶部；prompt、运行目标（新会话 /
 *   现有会话 + 会话选择器）、频率（重复 / 时间）都可编辑，有改动时
 *   顶部出现「保存」，失败变「重试保存」。
 *
 * 创建有两条路，「创建」按钮弹菜单让用户选：**手动创建**在右侧栏（详情
 * 面板的位置）展开一张表单（[`ScheduleCreatePanel`]，字段和详情同一套）；
 * **让 Riot 创建**把开头替用户写好、送回输入框，由对话里的 agent 接手。
 * 建议走的是后一条。行上的「…」菜单复用 App 的全局 ContextMenu。
 */

import { useEffect, useMemo, useRef, useState } from "react";

import {
  type MissedRun,
  type RunTargetSpec,
  type SchedulePatch,
  type ScheduledTask,
  scheduleCreate,
  scheduleSetEnabled,
  scheduleUpdate,
  type SessionInfo,
  type WhenSpec,
} from "../bridge";
import { basename } from "../pathDisplay";
import { Chevron } from "./Chevron";
import { FieldSelect } from "./FieldSelect";
import { SidebarReveal } from "./chrome";
import { DateTimePicker, TimePicker } from "./TimePicker";
import { ResizableTextarea } from "./ResizableTextarea";
import { ArrowOutIcon, DotsIcon, PlusIcon } from "./icons";

type Filter = "all" | "enabled" | "paused" | "done";

const TABS: { id: Filter; label: string }[] = [
  { id: "all", label: "全部" },
  { id: "enabled", label: "已开启" },
  { id: "paused", label: "已暂停" },
  { id: "done", label: "已完成" },
];

const WEEKDAY_NAMES = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];

function repeatText(t: ScheduledTask): string {
  const r = t.repeat;
  switch (r.kind) {
    case "once":
      return "一次性";
    case "daily":
      return `每天 ${r.time}`;
    case "weekdays":
      return `工作日 ${r.time}`;
    case "weekly":
      return `每${WEEKDAY_NAMES[r.weekday - 1] ?? "?"} ${r.time}`;
  }
}

/** 一次性任务跑完了（区别于手动暂停）。 */
export function isDoneSchedule(t: ScheduledTask): boolean {
  return t.repeat.kind === "once" && !t.enabled && !t.nextRunMs;
}

/** "下次运行"的相对说法。远了退回绝对时刻（掐掉年份）。 */
function nextText(t: ScheduledTask): string {
  if (!t.enabled) return isDoneSchedule(t) ? "已跑完" : "已暂停";
  if (!t.nextRunMs) return "不再运行";
  const d = t.nextRunMs - Date.now();
  if (d <= 60_000) return "下次运行 1 分钟内";
  if (d < 3_600_000) return `下次运行 ${Math.round(d / 60_000)} 分钟后`;
  if (d < 86_400_000) return `下次运行 ${Math.round(d / 3_600_000)} 小时后`;
  return `下次运行 ${t.nextRunLocal?.slice(5) ?? ""}`;
}

/** 建议模板。点击把整段话送回输入框，让对话里的 agent 接手创建。 */
const SUGGESTIONS = [
  {
    title: "每日晨报",
    when: "工作日 8:00",
    desc: "把项目的最新改动和待办整理成简明的开工简报",
    snippet:
      "帮我设一个定时任务：每个工作日早上 8:00，看看这个项目最近的提交和改动，" +
      "把值得注意的事整理成一份简明的晨间简报。",
  },
  {
    title: "每周回顾",
    when: "星期五 16:00",
    desc: "每周五将你最近的工作整理成简明的状态更新",
    snippet:
      "帮我设一个定时任务：每周五 16:00，把本周这个项目的提交记录和改动" +
      "整理成一份简明的周报。",
  },
];

export function SchedulesPage({
  schedules,
  missed,
  selected,
  onSelect,
  sidebarOpen,
  onToggleSidebar,
  onMenu,
  menuAnchor,
  onCreate,
  onClearDone,
  onSuggest,
  onRerunMissed,
  onDismissMissed,
}: {
  schedules: ScheduledTask[];
  missed: MissedRun[];
  /** 详情面板正看着的任务。详情本体渲染在 App 的系统右侧栏里。 */
  selected: string | null;
  onSelect: (id: string | null) => void;
  /** 侧栏收起时页面顶部要自己放开关；开着时入口在侧栏顶栏，这里不重复。 */
  sidebarOpen: boolean;
  onToggleSidebar: () => void;
  /** 行上与详情面板的「…」：递给 App 的全局菜单。 */
  onMenu: (e: React.MouseEvent, t: ScheduledTask) => void;
  /** 右键 / … 菜单正对着的行。菜单在文档别处，行要靠这个保住 hover。 */
  menuAnchor?: string | null;
  /** 右上「创建」：App 在点击处弹出「手动创建 / 让 Riot 创建」菜单。 */
  onCreate: (e: React.MouseEvent) => void;
  /** 清掉一次性已经跑完的任务。 */
  onClearDone: () => void;
  /** 点一条建议：回到会话、把整段模板写进输入框。 */
  onSuggest: (snippet: string) => void;
  onRerunMissed: (m: MissedRun) => void;
  onDismissMissed: () => void;
}) {
  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return schedules.filter((t) => {
      if (filter === "enabled" && !t.enabled) return false;
      if (filter === "paused" && (t.enabled || isDoneSchedule(t))) return false;
      if (filter === "done" && !isDoneSchedule(t)) return false;
      if (q && !t.name.toLowerCase().includes(q) && !t.prompt.toLowerCase().includes(q)) {
        return false;
      }
      return true;
    });
  }, [schedules, filter, query]);

  const missedOf = (id: string) => missed.find((m) => m.taskId === id);

  return (
    <div className="sched-page">
      <div className="sp-main-col">
        {/* 这页没有 TopBar。侧栏收起时红绿灯悬在左上角，开关给它们让位；
            开着时入口在侧栏顶栏，这里不再放一个。 */}
        <div className="sp-chrome" data-tauri-drag-region>
          <SidebarReveal visible={!sidebarOpen} onToggle={onToggleSidebar} />
        </div>
        <div className="sp-scroll">
        <div className="sp-inner">
          <div className="sp-head">
            <h2>定时任务</h2>
            <p className="sp-sub">让 Riot 到点自动跑一轮 —— 手动填表创建，或在对话里说时间和要做的事</p>
          </div>

          <input
            className="sp-search"
            type="search"
            placeholder="搜索定时任务"
            value={query}
            onChange={(e) => setQuery(e.currentTarget.value)}
          />

          <div className="sp-bar">
            <div className="sp-tabs" role="tablist">
              {TABS.map((tab) => (
                <button
                  key={tab.id}
                  role="tab"
                  aria-selected={filter === tab.id}
                  className={filter === tab.id ? "sp-tab active" : "sp-tab"}
                  onClick={() => setFilter(tab.id)}
                >
                  {tab.label}
                </button>
              ))}
            </div>
            <div className="sp-actions">
              <button
                className="sp-clear"
                disabled={!schedules.some(isDoneSchedule)}
                onClick={onClearDone}
              >
                清理已完成
              </button>
              <button className="sp-create" onClick={onCreate} aria-haspopup="menu">
                <PlusIcon />
                创建
                <Chevron down open={false} />
              </button>
            </div>
          </div>

          {missed.length > 0 ? (
            <div className="sp-missed" role="status">
              <div className="sp-missed-title">App 关着的时候错过了 {missed.length} 个任务</div>
              {missed.map((m) => (
                <div className="sp-missed-row" key={m.taskId}>
                  <span className="sp-missed-name">
                    「{m.name}」错过 {m.count} 次，最后一次 {m.lastLocal}
                  </span>
                  <button className="sp-missed-btn" onClick={() => onRerunMissed(m)}>
                    补跑一次
                  </button>
                </div>
              ))}
              <div className="sp-missed-foot">
                <button className="ghost" onClick={onDismissMissed}>
                  都不用跑
                </button>
              </div>
            </div>
          ) : null}

          <div className="sp-list">
            {shown.length === 0 ? (
              <div className="sp-empty">
                {schedules.length === 0
                  ? "还没有定时任务。点「创建」，或试试下面的建议。"
                  : "没有匹配的任务。"}
              </div>
            ) : (
              shown.map((t) => {
                const m = missedOf(t.id);
                return (
                  <div
                    className={
                      (t.enabled ? "sp-row" : "sp-row paused") +
                      (t.id === selected ? " selected" : "") +
                      (menuAnchor === `schedule:${t.id}` ? " menu-open" : "")
                    }
                    key={t.id}
                    onContextMenu={(e) => onMenu(e, t)}
                  >
                    <button
                      className="sp-row-main"
                      onClick={() => onSelect(t.id === selected ? null : t.id)}
                      title={t.prompt}
                    >
                      <StatusRing t={t} />
                      <span className="sp-row-text">
                        <span className="sp-row-name">
                          {t.name}
                          {m ? <span className="sp-row-missed">错过 {m.count} 次</span> : null}
                        </span>
                        <span className="sp-row-meta">
                          {repeatText(t)} · {nextText(t)}
                          {t.sessionId ? " · 在原会话续跑" : ""}
                        </span>
                      </span>
                    </button>
                    <button className="row-btn" onClick={(e) => onMenu(e, t)} title="任务操作">
                      <DotsIcon />
                    </button>
                  </div>
                );
              })
            )}
          </div>

          <div className="sp-suggest">
            <div className="sp-suggest-caption">建议</div>
            {SUGGESTIONS.map((s) => (
              <button className="sp-suggest-row" key={s.title} onClick={() => onSuggest(s.snippet)}>
                <span className="sp-row-name">
                  {s.title}
                  <span className="sp-suggest-when">{s.when}</span>
                </span>
                <span className="sp-row-meta">{s.desc}</span>
              </button>
            ))}
          </div>
        </div>
        </div>
      </div>

    </div>
  );
}

/** 行首的状态圈：开着 = 空圈，暂停 = 双竖线，跑完 = 对勾。 */
function StatusRing({ t }: { t: ScheduledTask }) {
  return (
    <svg className="sp-ring" width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden>
      <circle cx="8" cy="8" r="6.2" stroke="currentColor" strokeWidth="1.4" />
      {isDoneSchedule(t) ? (
        <path
          d="M5.2 8.2l1.9 1.9 3.7-4"
          stroke="currentColor"
          strokeWidth="1.4"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      ) : !t.enabled ? (
        <path d="M6.6 5.8v4.4M9.4 5.8v4.4" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
      ) : null}
    </svg>
  );
}

/* ── 详情面板 ───────────────────────────────── */

/** 重复选项的扁平表示："once"/"daily"/"weekdays"/"w1".."w7"。 */
type RepeatChoice = "once" | "daily" | "weekdays" | `w${number}`;

function choiceOf(t: ScheduledTask): RepeatChoice {
  switch (t.repeat.kind) {
    case "once":
      return "once";
    case "daily":
      return "daily";
    case "weekdays":
      return "weekdays";
    case "weekly":
      return `w${t.repeat.weekday}`;
  }
}

function timeOf(t: ScheduledTask): string {
  const r = t.repeat;
  if (r.kind === "daily" || r.kind === "weekdays" || r.kind === "weekly") return r.time;
  return "09:00";
}

/**
 * 任务详情。渲染在 **App 的系统右侧栏**（工作台抽屉的位置，全高、
 * 和主区平级），不是任务页内部的一列 —— 宽度与 Resizer 由 App 管。
 */
export function ScheduleDetail({
  task,
  width,
  sessions,
  projects,
  onClose,
  onMenu,
  onError,
  onOpenSession,
}: {
  task: ScheduledTask;
  /** 用户拖出来的宽度。真值和持久化在 App。 */
  width: number;
  sessions: SessionInfo[];
  projects: string[];
  onClose: () => void;
  onMenu: (e: React.MouseEvent, t: ScheduledTask) => void;
  onError: (title: string, e: unknown) => void;
  /** 跳到这条任务绑定的会话。 */
  onOpenSession: (id: string) => void;
}) {
  const [name, setName] = useState(task.name);
  const [prompt, setPrompt] = useState(task.prompt);
  const [runIn, setRunIn] = useState<"new" | "session">(task.sessionId ? "session" : "new");
  const [sessionId, setSessionId] = useState<string | null>(task.sessionId ?? null);
  const [root, setRoot] = useState(task.root);
  const [choice, setChoice] = useState<RepeatChoice>(() => choiceOf(task));
  const [time, setTime] = useState(() => timeOf(task));
  const [onceAt, setOnceAt] = useState(task.nextRunLocal ?? "");
  const [saving, setSaving] = useState(false);
  const [saveFailed, setSaveFailed] = useState(false);

  const done = isDoneSchedule(task);
  const status = task.enabled ? "活跃" : done ? "已完成" : "已暂停";

  /** 各字段相对任务当前值有没有动过。保存成功后 task 更新，dirty 自动消失。 */
  const dirty = useMemo(() => {
    if (name.trim() !== task.name) return true;
    if (prompt.trim() !== task.prompt) return true;
    const wasRunIn = task.sessionId ? "session" : "new";
    if (runIn !== wasRunIn) return true;
    if (runIn === "session" && sessionId !== (task.sessionId ?? null)) return true;
    if (runIn === "new" && root !== task.root) return true;
    if (choice !== choiceOf(task)) return true;
    if (choice !== "once" && time !== timeOf(task)) return true;
    if (choice === "once" && onceAt.trim() !== (task.nextRunLocal ?? "")) return true;
    return false;
  }, [task, name, prompt, runIn, sessionId, root, choice, time, onceAt]);

  const save = async () => {
    const patch: SchedulePatch = {};
    if (name.trim() !== task.name) patch.name = name.trim();
    if (prompt.trim() !== task.prompt) patch.prompt = prompt.trim();

    const wasRunIn = task.sessionId ? "session" : "new";
    if (runIn !== wasRunIn || (runIn === "session" && sessionId !== task.sessionId) || (runIn === "new" && root !== task.root)) {
      if (runIn === "session") {
        if (!sessionId) {
          onError("还没选会话", "「运行于现有会话」需要先选一个会话。");
          return;
        }
        patch.target = { kind: "session", id: sessionId };
      } else {
        patch.target = { kind: "new_session", root };
      }
    }

    const whenChanged =
      choice !== choiceOf(task) ||
      (choice !== "once" && time !== timeOf(task)) ||
      (choice === "once" && onceAt.trim() !== (task.nextRunLocal ?? ""));
    if (whenChanged) {
      patch.when = buildWhen(choice, time, onceAt);
    }

    setSaving(true);
    try {
      await scheduleUpdate(task.id, patch);
      setSaveFailed(false);
    } catch (e) {
      setSaveFailed(true);
      onError("保存失败", e);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="sp-detail" style={{ width }}>
      <div className="sp-d-head">
        <span className={task.enabled ? "sp-d-status live" : "sp-d-status"}>{status}</span>
        <span className="sp-d-space" />
        <button
          className={dirty ? "sp-create sp-d-save" : "sp-create sp-d-save idle"}
          disabled={!dirty || saving}
          tabIndex={dirty ? undefined : -1}
          aria-hidden={!dirty}
          onClick={() => void save()}
        >
          {saving ? "保存中…" : saveFailed ? "重试保存" : "保存"}
        </button>
        <button className="row-btn" onClick={(e) => onMenu(e, task)} title="更多操作">
          <DotsIcon />
        </button>
        {!done ? (
          <button
            className="row-btn"
            title={task.enabled ? "暂停" : "恢复"}
            onClick={() =>
              void scheduleSetEnabled(task.id, !task.enabled).catch((e: unknown) =>
                onError(task.enabled ? "暂停失败" : "恢复失败", e),
              )
            }
          >
            <PauseResumeIcon paused={!task.enabled} />
          </button>
        ) : null}
        <button className="row-btn" onClick={onClose} title="关闭" aria-label="关闭详情">
          <CloseIcon />
        </button>
      </div>

      <input
        className="sp-d-name"
        value={name}
        onChange={(e) => setName(e.currentTarget.value)}
        aria-label="任务名"
        spellCheck={false}
      />

      <ResizableTextarea
        className="preset-body-input"
        value={prompt}
        onChange={(e) => setPrompt(e.currentTarget.value)}
        rows={6}
        aria-label="到点发出的提示词"
        spellCheck={false}
      />

      <div className="sp-d-caption">详情</div>
      <div className="sp-d-group">
        <div className="sp-d-row">
          <span>运行于</span>
          <FieldSelect
            className="sp-d-field"
            value={runIn}
            onChange={(v) => setRunIn(v as "new" | "session")}
            options={[
              { value: "new", label: "新会话" },
              { value: "session", label: "现有会话" },
            ]}
          />
        </div>
        {runIn === "session" ? (
          <div className="sp-d-row">
            <span className="sp-d-label">
              会话
              <button
                className="row-btn"
                disabled={!sessionId || !sessions.some((s) => s.id === sessionId)}
                onClick={() => sessionId && onOpenSession(sessionId)}
                title="打开这个会话"
                aria-label="打开这个会话"
              >
                <ArrowOutIcon />
              </button>
            </span>
            <SessionPicker sessions={sessions} value={sessionId} onPick={setSessionId} />
          </div>
        ) : (
          <div className="sp-d-row">
            <span>项目</span>
            <FieldSelect
              className="sp-d-field"
              value={root}
              onChange={setRoot}
              menuMinWidth={220}
              options={[
                // 任务当前的根可能不在项目列表里（项目被移除过），补一项别丢显示。
                ...(projects.includes(root) ? [] : [{ value: root, label: basename(root) }]),
                ...projects.map((p) => ({ value: p, label: basename(p) })),
              ]}
            />
          </div>
        )}
      </div>

      <div className="sp-d-caption">频率</div>
      <div className="sp-d-group">
        <div className="sp-d-row">
          <span>重复</span>
          <FieldSelect
            className="sp-d-field"
            value={choice}
            onChange={(v) => setChoice(v as RepeatChoice)}
            options={[
              { value: "once", label: "一次性" },
              { value: "daily", label: "每天" },
              { value: "weekdays", label: "工作日" },
              ...WEEKDAY_NAMES.map((w, i) => ({ value: `w${i + 1}`, label: `每${w}` })),
            ]}
          />
        </div>
        {choice === "once" ? (
          <div className="sp-d-row">
            <span>时刻</span>
            <DateTimePicker className="sp-d-field" value={onceAt} onChange={setOnceAt} />
          </div>
        ) : (
          <div className="sp-d-row">
            <span>时间</span>
            <TimePicker className="sp-d-field" value={time} onChange={setTime} />
          </div>
        )}
      </div>

      {task.lastRunLocal ? (
        <div className="sp-d-foot">上次运行 {task.lastRunLocal}</div>
      ) : null}
    </div>
  );
}

/** 表单选择 → 协议的时间说法。 */
function buildWhen(choice: RepeatChoice, time: string, onceAt: string): WhenSpec {
  if (choice === "once") return { kind: "once", at: onceAt.trim() };
  if (choice === "daily") return { kind: "daily", time };
  if (choice === "weekdays") return { kind: "weekdays", time };
  return { kind: "weekly", weekday: Number(choice.slice(1)), time };
}

/* ── 手动创建 ───────────────────────────────── */

/**
 * 手动创建的表单。渲染在**详情面板同一个位置**（App 的系统右侧栏），
 * 壳、字段、间距都和 [`ScheduleDetail`] 一样 —— 建完选中新任务，面板
 * 原地换成详情，用户眼里是同一块地方从"填"变成了"看"。
 *
 * 提交交给宿主校验 —— 时间格式、目录是否存在、会话是否还在都在那边判，
 * 错误原文就是给人看的一句话，直接摆在表单底部。默认值挑"最像会填的"：
 * 每天 09:00、新会话、当前项目。一次性任务的时刻留空让日期选择器自己
 * 落到"明天 9 点"。
 */
export function ScheduleCreatePanel({
  width,
  sessions,
  projects,
  defaultRoot,
  onClose,
  onCreated,
}: {
  /** 和详情面板同一个宽度（用户拖出来的值，真值在 App）。 */
  width: number;
  sessions: SessionInfo[];
  projects: string[];
  /** 「新会话」默认绑定的项目：活跃会话的根，没有就项目列表第一个。 */
  defaultRoot: string | null;
  onClose: () => void;
  /** 建成了。调用方刷新列表、选中它。 */
  onCreated: (t: ScheduledTask) => void;
}) {
  const [name, setName] = useState("");
  const [prompt, setPrompt] = useState("");
  const [runIn, setRunIn] = useState<"new" | "session">("new");
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [root, setRoot] = useState(defaultRoot ?? projects[0] ?? "");
  const [choice, setChoice] = useState<RepeatChoice>("daily");
  const [time, setTime] = useState("09:00");
  const [onceAt, setOnceAt] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const canSubmit =
    name.trim() !== "" &&
    prompt.trim() !== "" &&
    (runIn === "new" ? root !== "" : sessionId !== null) &&
    (choice !== "once" || onceAt.trim() !== "");

  const submit = async () => {
    if (!canSubmit || busy) return;
    const target: RunTargetSpec =
      runIn === "session" && sessionId
        ? { kind: "session", id: sessionId }
        : { kind: "new_session", root };
    setBusy(true);
    setError(null);
    try {
      const t = await scheduleCreate({
        name: name.trim(),
        prompt: prompt.trim(),
        when: buildWhen(choice, time, onceAt),
        target,
      });
      onCreated(t);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="sp-detail" style={{ width }} aria-label="创建定时任务">
      <div className="sp-d-head">
        <span className="sp-d-status">新任务</span>
        <span className="sp-d-space" />
        <button
          className="sp-create sp-d-save"
          disabled={!canSubmit || busy}
          onClick={() => void submit()}
        >
          {busy ? "创建中…" : "创建"}
        </button>
        <button className="row-btn" onClick={onClose} title="取消" aria-label="取消创建">
          <CloseIcon />
        </button>
      </div>

      <input
        className="sp-d-name"
        autoFocus
        value={name}
        onChange={(e) => setName(e.currentTarget.value)}
        placeholder="任务名"
        aria-label="任务名"
        spellCheck={false}
      />

      <ResizableTextarea
        className="preset-body-input"
        value={prompt}
        onChange={(e) => setPrompt(e.currentTarget.value)}
        rows={6}
        placeholder="到点发给 Riot 的话。像写给未来的自己：把背景说全，那时不一定有现在的上下文。"
        aria-label="到点发出的提示词"
        spellCheck={false}
      />

      <div className="sp-d-caption">详情</div>
      <div className="sp-d-group">
        <div className="sp-d-row">
          <span>运行于</span>
          <FieldSelect
            className="sp-d-field"
            value={runIn}
            onChange={(v) => setRunIn(v as "new" | "session")}
            options={[
              { value: "new", label: "新会话" },
              { value: "session", label: "现有会话" },
            ]}
          />
        </div>
        {runIn === "session" ? (
          <div className="sp-d-row">
            <span>会话</span>
            <SessionPicker sessions={sessions} value={sessionId} onPick={setSessionId} />
          </div>
        ) : (
          <div className="sp-d-row">
            <span>项目</span>
            <FieldSelect
              className="sp-d-field"
              value={root}
              onChange={setRoot}
              menuMinWidth={220}
              options={projects.map((p) => ({ value: p, label: basename(p) }))}
            />
          </div>
        )}
      </div>

      <div className="sp-d-caption">频率</div>
      <div className="sp-d-group">
        <div className="sp-d-row">
          <span>重复</span>
          <FieldSelect
            className="sp-d-field"
            value={choice}
            onChange={(v) => setChoice(v as RepeatChoice)}
            options={[
              { value: "once", label: "一次性" },
              { value: "daily", label: "每天" },
              { value: "weekdays", label: "工作日" },
              ...WEEKDAY_NAMES.map((w, i) => ({ value: `w${i + 1}`, label: `每${w}` })),
            ]}
          />
        </div>
        {choice === "once" ? (
          <div className="sp-d-row">
            <span>时刻</span>
            <DateTimePicker className="sp-d-field" value={onceAt} onChange={setOnceAt} />
          </div>
        ) : (
          <div className="sp-d-row">
            <span>时间</span>
            <TimePicker className="sp-d-field" value={time} onChange={setTime} />
          </div>
        )}
      </div>

      {error ? (
        <div className="sp-d-error" role="alert">
          {error}
        </div>
      ) : null}
    </div>
  );
}

/**
 * 会话选择器（Codex 的「选择一个聊天」同款）：点开出搜索框 +
 * 按项目分组的会话列表，点外面收起。
 */
function SessionPicker({
  sessions,
  value,
  onPick,
}: {
  sessions: SessionInfo[];
  value: string | null;
  onPick: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const boxRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (boxRef.current && e.target instanceof Node && !boxRef.current.contains(e.target)) {
        setOpen(false);
      }
    };
    window.addEventListener("pointerdown", onDown);
    return () => window.removeEventListener("pointerdown", onDown);
  }, [open]);

  const current = sessions.find((s) => s.id === value) ?? null;
  const label = current ? (current.title ?? "新会话") : "选择一个会话";

  /** 按项目分组，组内新的在前（和侧栏同序）。 */
  const groups = useMemo(() => {
    const needle = q.trim().toLowerCase();
    const hit = sessions.filter(
      (s) => !needle || (s.title ?? "新会话").toLowerCase().includes(needle),
    );
    const byRoot = new Map<string, SessionInfo[]>();
    for (const s of hit) {
      const list = byRoot.get(s.root) ?? [];
      list.push(s);
      byRoot.set(s.root, list);
    }
    return [...byRoot.entries()].map(([groupRoot, list]) => ({
      root: groupRoot,
      list: [...list].sort((a, b) => b.seq - a.seq),
    }));
  }, [sessions, q]);

  return (
    <div className="sp-picker" ref={boxRef}>
      <button className="sp-d-select sp-picker-btn" onClick={() => setOpen((v) => !v)}>
        <span className="sp-picker-label">{label}</span>
        <Chevron open={open} />
      </button>
      {open ? (
        <div className="sp-picker-pop">
          <input
            className="sp-picker-search"
            autoFocus
            value={q}
            onChange={(e) => setQ(e.currentTarget.value)}
            placeholder="搜索会话"
            spellCheck={false}
          />
          <div className="sp-picker-list">
            {groups.length === 0 ? (
              <div className="sp-picker-empty">没有匹配的会话</div>
            ) : (
              groups.map((g) => (
                <div key={g.root}>
                  <div className="sp-picker-group">{basename(g.root)}</div>
                  {g.list.map((s) => (
                    <button
                      key={s.id}
                      className={s.id === value ? "sp-picker-item picked" : "sp-picker-item"}
                      onClick={() => {
                        onPick(s.id);
                        setOpen(false);
                      }}
                    >
                      {s.title ?? "新会话"}
                    </button>
                  ))}
                </div>
              ))
            )}
          </div>
        </div>
      ) : null}
    </div>
  );
}

function PauseResumeIcon({ paused }: { paused: boolean }) {
  return paused ? (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" aria-hidden>
      <path d="M5.5 3.8v8.4l7-4.2z" />
    </svg>
  ) : (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path d="M5.8 4v8M10.2 4v8" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
    </svg>
  );
}

function CloseIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
    </svg>
  );
}
