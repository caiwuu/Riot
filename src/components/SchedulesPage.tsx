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
  type ScheduleRunRecord,
  renderUiError,
  type ScheduledTask,
  scheduleCreate,
  scheduleSetEnabled,
  scheduleUpdate,
  type SessionInfo,
  type WhenSpec,
} from "../bridge";
import { type MessageKey, dateTimeFormat, t, tn, useT } from "../i18n";
import { basename } from "../pathDisplay";
import { Chevron } from "./Chevron";
import { FieldSelect } from "./FieldSelect";
import { SidebarReveal } from "./chrome";
import { DateTimePicker, TimePicker } from "./TimePicker";
import { ResizableTextarea } from "./ResizableTextarea";
import { ArrowOutIcon, DotsIcon, PlusIcon } from "./icons";

type Filter = "all" | "enabled" | "paused" | "done";

const TABS: { id: Filter; label: MessageKey }[] = [
  { id: "all", label: "schedules.tab.all" },
  { id: "enabled", label: "schedules.tab.enabled" },
  { id: "paused", label: "schedules.status.paused" },
  { id: "done", label: "schedules.status.done" },
];

/** 星期名（1 = 周一 … 7 = 周日），按当前界面语言。2024-01-01 是周一。 */
function weekdayName(weekday: number): string {
  if (weekday < 1 || weekday > 7) return "?";
  return dateTimeFormat({ weekday: "long" }).format(new Date(2024, 0, weekday));
}

/** "每 N 分钟"：整小时 / 整天的说成小时 / 天，念着顺。 */
function everyText(minutes: number): string {
  if (minutes % 1440 === 0) return tn("schedules.repeat.everyDays", minutes / 1440);
  if (minutes % 60 === 0) return tn("schedules.repeat.everyHours", minutes / 60);
  return tn("schedules.repeat.everyMinutes", minutes);
}

function repeatText(task: ScheduledTask): string {
  const r = task.repeat;
  switch (r.kind) {
    case "once":
      return t("schedules.repeat.once");
    case "every":
      return everyText(r.minutes);
    case "daily":
      return t("schedules.repeat.dailyAt", { time: r.time });
    case "weekdays":
      return t("schedules.repeat.weekdaysAt", { time: r.time });
    case "weekly":
      return t("schedules.repeat.weeklyAt", { weekday: weekdayName(r.weekday), time: r.time });
  }
}

/** 一次性任务跑完了（区别于手动暂停）。 */
export function isDoneSchedule(t: ScheduledTask): boolean {
  return t.repeat.kind === "once" && !t.enabled && !t.nextRunMs;
}

/** "下次运行"的相对说法。远了退回绝对时刻（掐掉年份）。 */
function nextText(task: ScheduledTask): string {
  if (!task.enabled) return t(isDoneSchedule(task) ? "schedules.next.finished" : "schedules.status.paused");
  if (!task.nextRunMs) return t("schedules.next.never");
  const d = task.nextRunMs - Date.now();
  if (d <= 60_000) return t("schedules.next.withinMinute");
  if (d < 3_600_000) return tn("schedules.next.inMinutes", Math.round(d / 60_000));
  if (d < 86_400_000) return tn("schedules.next.inHours", Math.round(d / 3_600_000));
  return t("schedules.next.at", { time: task.nextRunLocal?.slice(5) ?? "" });
}

/** 建议模板。点击把整段话送回输入框，让对话里的 agent 接手创建。 */
const SUGGESTIONS: { id: string; title: MessageKey; when: MessageKey; desc: MessageKey; snippet: MessageKey }[] = [
  {
    id: "morning",
    title: "schedules.suggest.morning.title",
    when: "schedules.suggest.morning.when",
    desc: "schedules.suggest.morning.desc",
    snippet: "schedules.suggest.morning.snippet",
  },
  {
    id: "weekly",
    title: "schedules.suggest.weekly.title",
    when: "schedules.suggest.weekly.when",
    desc: "schedules.suggest.weekly.desc",
    snippet: "schedules.suggest.weekly.snippet",
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
  const { t, tn } = useT();
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
            <h2>{t("schedules.title")}</h2>
            <p className="sp-sub">{t("schedules.subtitle")}</p>
          </div>

          <input
            className="sp-search"
            type="search"
            placeholder={t("schedules.searchPlaceholder")}
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
                  {t(tab.label)}
                </button>
              ))}
            </div>
            <div className="sp-actions">
              <button
                className="sp-clear"
                disabled={!schedules.some(isDoneSchedule)}
                onClick={onClearDone}
              >
                {t("schedules.clearDone")}
              </button>
              <button className="sp-create" onClick={onCreate} aria-haspopup="menu">
                <PlusIcon />
                {t("schedules.create")}
                <Chevron down open={false} />
              </button>
            </div>
          </div>

          {missed.length > 0 ? (
            <div className="sp-missed" role="status">
              <div className="sp-missed-title">{tn("schedules.missed.title", missed.length)}</div>
              {missed.map((m) => (
                <div className="sp-missed-row" key={m.taskId}>
                  <span className="sp-missed-name">
                    {tn("schedules.missed.row", m.count, { name: m.name, time: m.lastLocal })}
                  </span>
                  <button className="sp-missed-btn" onClick={() => onRerunMissed(m)}>
                    {t("schedules.missed.rerun")}
                  </button>
                </div>
              ))}
              <div className="sp-missed-foot">
                <button className="ghost" onClick={onDismissMissed}>
                  {t("schedules.missed.dismiss")}
                </button>
              </div>
            </div>
          ) : null}

          <div className="sp-list">
            {shown.length === 0 ? (
              <div className="sp-empty">
                {schedules.length === 0 ? t("schedules.empty.none") : t("schedules.empty.noMatch")}
              </div>
            ) : (
              shown.map((task) => {
                const m = missedOf(task.id);
                return (
                  <div
                    className={
                      (task.enabled ? "sp-row" : "sp-row paused") +
                      (task.id === selected ? " selected" : "") +
                      (menuAnchor === `schedule:${task.id}` ? " menu-open" : "")
                    }
                    key={task.id}
                    onContextMenu={(e) => onMenu(e, task)}
                  >
                    <button
                      className="sp-row-main"
                      onClick={() => onSelect(task.id === selected ? null : task.id)}
                      title={task.prompt}
                    >
                      <StatusRing t={task} />
                      <span className="sp-row-text">
                        <span className="sp-row-name">
                          {task.name}
                          {m ? (
                            <span className="sp-row-missed">{tn("schedules.row.missed", m.count)}</span>
                          ) : null}
                        </span>
                        <span className="sp-row-meta">
                          {repeatText(task)} · {nextText(task)}
                          {task.sessionId ? ` · ${t("schedules.row.continueInSession")}` : ""}
                        </span>
                      </span>
                    </button>
                    <button className="row-btn" onClick={(e) => onMenu(e, task)} title={t("schedules.row.actions")}>
                      <DotsIcon />
                    </button>
                  </div>
                );
              })
            )}
          </div>

          <div className="sp-suggest">
            <div className="sp-suggest-caption">{t("schedules.suggest.caption")}</div>
            {SUGGESTIONS.map((s) => (
              <button className="sp-suggest-row" key={s.id} onClick={() => onSuggest(t(s.snippet))}>
                <span className="sp-row-name">
                  {t(s.title)}
                  <span className="sp-suggest-when">{t(s.when)}</span>
                </span>
                <span className="sp-row-meta">{t(s.desc)}</span>
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

/** 重复选项的扁平表示："once"/"every"/"daily"/"weekdays"/"w1".."w7"。 */
type RepeatChoice = "once" | "every" | "daily" | "weekdays" | `w${number}`;

const REPEAT_FIXED: { value: RepeatChoice; label: MessageKey }[] = [
  { value: "once", label: "schedules.repeat.once" },
  { value: "every", label: "schedules.repeat.every" },
  { value: "daily", label: "schedules.repeat.daily" },
  { value: "weekdays", label: "schedules.repeat.weekdays" },
];

/** 渲染时现算：标签跟着当前语言走，星期名由 Intl 给。 */
function repeatOptions(): { value: string; label: string }[] {
  return [
    ...REPEAT_FIXED.map((o) => ({ value: o.value, label: t(o.label) })),
    ...Array.from({ length: 7 }, (_, i) => ({
      value: `w${i + 1}`,
      label: t("schedules.repeat.weekly", { weekday: weekdayName(i + 1) }),
    })),
  ];
}

function choiceOf(t: ScheduledTask): RepeatChoice {
  switch (t.repeat.kind) {
    case "once":
      return "once";
    case "every":
      return "every";
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

/** 间隔的表单表示：数值 + 单位。整小时的任务用小时显示，不然 120 分钟看着别扭。 */
type Interval = { n: string; unit: "min" | "hour" };

function intervalOf(t: ScheduledTask): Interval {
  if (t.repeat.kind !== "every") return { n: "30", unit: "min" };
  const m = t.repeat.minutes;
  return m % 60 === 0 ? { n: String(m / 60), unit: "hour" } : { n: String(m), unit: "min" };
}

/** 表单的间隔 → 分钟数。非法（空、非数字、≤0）返回 null。 */
function intervalMinutes(iv: Interval): number | null {
  const n = Number(iv.n);
  if (!Number.isInteger(n) || n <= 0) return null;
  return iv.unit === "hour" ? n * 60 : n;
}

/** 频率组里"间隔"那一行：数值输入 + 单位下拉。 */
function IntervalRow({ value, onChange }: { value: Interval; onChange: (v: Interval) => void }) {
  const { t } = useT();
  return (
    <div className="sp-d-row">
      <span>{t("schedules.form.interval")}</span>
      <span className="sp-d-interval">
        <input
          className="sp-d-input sp-d-interval-n"
          type="number"
          min={1}
          step={1}
          inputMode="numeric"
          value={value.n}
          onChange={(e) => onChange({ ...value, n: e.currentTarget.value })}
          aria-label={t("schedules.form.intervalValue")}
        />
        <FieldSelect
          className="sp-d-field"
          value={value.unit}
          onChange={(u) => onChange({ ...value, unit: u as Interval["unit"] })}
          options={[
            { value: "min", label: t("schedules.unit.minutes") },
            { value: "hour", label: t("schedules.unit.hours") },
          ]}
        />
      </span>
    </div>
  );
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
  const [every, setEvery] = useState<Interval>(() => intervalOf(task));
  const [onceAt, setOnceAt] = useState(task.nextRunLocal ?? "");
  const [saving, setSaving] = useState(false);
  const [saveFailed, setSaveFailed] = useState(false);
  const { t } = useT();

  const done = isDoneSchedule(task);
  const status = t(
    task.enabled ? "schedules.status.active" : done ? "schedules.status.done" : "schedules.status.paused",
  );

  /** 频率相对任务当前值动过没有。三种形态各看自己那一项。 */
  const whenChanged =
    choice !== choiceOf(task) ||
    (choice === "once"
      ? onceAt.trim() !== (task.nextRunLocal ?? "")
      : choice === "every"
        ? intervalMinutes(every) !== (task.repeat.kind === "every" ? task.repeat.minutes : null)
        : time !== timeOf(task));

  /** 各字段相对任务当前值有没有动过。保存成功后 task 更新，dirty 自动消失。 */
  const dirty = useMemo(() => {
    if (name.trim() !== task.name) return true;
    if (prompt.trim() !== task.prompt) return true;
    const wasRunIn = task.sessionId ? "session" : "new";
    if (runIn !== wasRunIn) return true;
    if (runIn === "session" && sessionId !== (task.sessionId ?? null)) return true;
    if (runIn === "new" && root !== task.root) return true;
    return whenChanged;
  }, [task, name, prompt, runIn, sessionId, root, whenChanged]);

  const save = async () => {
    const patch: SchedulePatch = {};
    if (name.trim() !== task.name) patch.name = name.trim();
    if (prompt.trim() !== task.prompt) patch.prompt = prompt.trim();

    const wasRunIn = task.sessionId ? "session" : "new";
    if (runIn !== wasRunIn || (runIn === "session" && sessionId !== task.sessionId) || (runIn === "new" && root !== task.root)) {
      if (runIn === "session") {
        if (!sessionId) {
          onError(t("schedules.err.noSession.title"), t("schedules.err.noSession.body"));
          return;
        }
        patch.target = { kind: "session", id: sessionId };
      } else {
        patch.target = { kind: "new_session", root };
      }
    }

    if (whenChanged) {
      const when = buildWhen(choice, time, onceAt, every);
      if (!when) {
        onError(t("schedules.err.badInterval.title"), t("schedules.err.badInterval.body"));
        return;
      }
      patch.when = when;
    }

    setSaving(true);
    try {
      await scheduleUpdate(task.id, patch);
      setSaveFailed(false);
    } catch (e) {
      setSaveFailed(true);
      onError(t("schedules.saveFailed"), e);
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
          {saving ? t("common.saving") : saveFailed ? t("schedules.retrySave") : t("common.save")}
        </button>
        <button className="row-btn" onClick={(e) => onMenu(e, task)} title={t("schedules.moreActions")}>
          <DotsIcon />
        </button>
        {!done ? (
          <button
            className="row-btn"
            title={task.enabled ? t("schedules.pause") : t("schedules.resume")}
            onClick={() =>
              void scheduleSetEnabled(task.id, !task.enabled).catch((e: unknown) =>
                onError(task.enabled ? t("schedules.pauseFailed") : t("schedules.resumeFailed"), e),
              )
            }
          >
            <PauseResumeIcon paused={!task.enabled} />
          </button>
        ) : null}
        <button className="row-btn" onClick={onClose} title={t("common.close")} aria-label={t("schedules.closeDetail")}>
          <CloseIcon />
        </button>
      </div>

      <input
        className="sp-d-name"
        value={name}
        onChange={(e) => setName(e.currentTarget.value)}
        aria-label={t("schedules.form.name")}
        spellCheck={false}
      />

      <ResizableTextarea
        className="preset-body-input"
        value={prompt}
        onChange={(e) => setPrompt(e.currentTarget.value)}
        rows={6}
        aria-label={t("schedules.form.prompt")}
        spellCheck={false}
      />

      <div className="sp-d-caption">{t("schedules.section.details")}</div>
      <div className="sp-d-group">
        <div className="sp-d-row">
          <span>{t("schedules.form.runIn")}</span>
          <FieldSelect
            className="sp-d-field"
            value={runIn}
            onChange={(v) => setRunIn(v as "new" | "session")}
            options={[
              { value: "new", label: t("schedules.newSession") },
              { value: "session", label: t("schedules.existingSession") },
            ]}
          />
        </div>
        {runIn === "session" ? (
          <div className="sp-d-row">
            <span className="sp-d-label">
              {t("schedules.form.session")}
              <button
                className="row-btn"
                disabled={!sessionId || !sessions.some((s) => s.id === sessionId)}
                onClick={() => sessionId && onOpenSession(sessionId)}
                title={t("schedules.openSession")}
                aria-label={t("schedules.openSession")}
              >
                <ArrowOutIcon />
              </button>
            </span>
            <SessionPicker sessions={sessions} value={sessionId} onPick={setSessionId} />
          </div>
        ) : (
          <div className="sp-d-row">
            <span>{t("schedules.form.project")}</span>
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

      <div className="sp-d-caption">{t("schedules.section.frequency")}</div>
      <div className="sp-d-group">
        <div className="sp-d-row">
          <span>{t("schedules.form.repeat")}</span>
          <FieldSelect
            className="sp-d-field"
            value={choice}
            onChange={(v) => setChoice(v as RepeatChoice)}
            options={repeatOptions()}
          />
        </div>
        {choice === "once" ? (
          <div className="sp-d-row">
            <span>{t("schedules.form.datetime")}</span>
            <DateTimePicker className="sp-d-field" value={onceAt} onChange={setOnceAt} />
          </div>
        ) : choice === "every" ? (
          <IntervalRow value={every} onChange={setEvery} />
        ) : (
          <div className="sp-d-row">
            <span>{t("schedules.form.time")}</span>
            <TimePicker className="sp-d-field" value={time} onChange={setTime} />
          </div>
        )}
      </div>

      <RunHistory runs={task.runs ?? []} sessions={sessions} onOpenSession={onOpenSession} />
    </div>
  );
}

/**
 * 运行历史（Codex 的「运行历史记录」同款）：每次到点执行一行 —— 状态点、
 * 时刻、会话、相对时间。点行跳到那次运行的会话看结果。
 *
 * 这是"一个周期任务反复跑"的另一半：任务列表里只有一条，跑过哪几次、
 * 哪次失败了，都在这里。
 */
function RunHistory({
  runs,
  sessions,
  onOpenSession,
}: {
  runs: ScheduleRunRecord[];
  sessions: SessionInfo[];
  onOpenSession: (id: string) => void;
}) {
  const { t } = useT();
  if (runs.length === 0) return null;
  return (
    <>
      <div className="sp-d-caption">{t("schedules.runs.title")}</div>
      <div className="sp-d-group sp-runs">
        {runs.map((r) => {
          const state = r.error ? "failed" : r.finishedAtMs ? "ok" : "running";
          const session = r.sessionId ? sessions.find((s) => s.id === r.sessionId) : undefined;
          // 会话还在就能点过去；已删（或开跑就失败没会话）只展示。
          const canOpen = Boolean(session);
          const errorText = r.error ? renderUiError(r.error) : null;
          const title =
            errorText ?? t(state === "running" ? "schedules.runs.running" : "schedules.status.done");
          return (
            <button
              key={`${r.startedAtMs}:${r.sessionId ?? ""}`}
              className={canOpen ? "sp-run" : "sp-run static"}
              disabled={!canOpen}
              onClick={() => r.sessionId && onOpenSession(r.sessionId)}
              title={title}
            >
              <span className={`sp-run-dot ${state}`} aria-label={title} />
              <span className="sp-run-text">
                <span className="sp-run-when">{r.startedAtLocal}</span>
                {session ? (
                  <span className="sp-run-session">{session.title ?? t("schedules.newSession")}</span>
                ) : errorText ? (
                  <span className="sp-run-session err">{errorText}</span>
                ) : null}
              </span>
              <span className="sp-run-ago">{agoText(r.startedAtMs)}</span>
            </button>
          );
        })}
      </div>
    </>
  );
}

/** "多久之前"：分钟 / 小时 / 天，再远给日期。 */
function agoText(ms: number): string {
  const d = Date.now() - ms;
  if (d < 60_000) return t("common.justNow");
  if (d < 3_600_000) return tn("common.minutesAgo", Math.round(d / 60_000));
  if (d < 86_400_000) return tn("common.hoursAgo", Math.round(d / 3_600_000));
  if (d < 30 * 86_400_000) return tn("common.daysAgo", Math.round(d / 86_400_000));
  return dateTimeFormat({ year: "numeric", month: "numeric", day: "numeric" }).format(new Date(ms));
}

/** 表单选择 → 协议的时间说法。间隔非法时返回 null，调用方提示。 */
function buildWhen(
  choice: RepeatChoice,
  time: string,
  onceAt: string,
  interval: Interval,
): WhenSpec | null {
  if (choice === "once") return { kind: "once", at: onceAt.trim() };
  if (choice === "every") {
    const minutes = intervalMinutes(interval);
    return minutes === null ? null : { kind: "every", minutes };
  }
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
  const [every, setEvery] = useState<Interval>({ n: "30", unit: "min" });
  const [onceAt, setOnceAt] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { t } = useT();

  const when = buildWhen(choice, time, onceAt, every);
  const canSubmit =
    name.trim() !== "" &&
    prompt.trim() !== "" &&
    (runIn === "new" ? root !== "" : sessionId !== null) &&
    (choice !== "once" || onceAt.trim() !== "") &&
    when !== null;

  const submit = async () => {
    if (!canSubmit || busy || !when) return;
    const target: RunTargetSpec =
      runIn === "session" && sessionId
        ? { kind: "session", id: sessionId }
        : { kind: "new_session", root };
    setBusy(true);
    setError(null);
    try {
      const created = await scheduleCreate({
        name: name.trim(),
        prompt: prompt.trim(),
        when,
        target,
      });
      onCreated(created);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="sp-detail" style={{ width }} aria-label={t("schedules.createPanel")}>
      <div className="sp-d-head">
        <span className="sp-d-status">{t("schedules.newTask")}</span>
        <span className="sp-d-space" />
        <button
          className="sp-create sp-d-save"
          disabled={!canSubmit || busy}
          onClick={() => void submit()}
        >
          {busy ? t("schedules.creating") : t("schedules.create")}
        </button>
        <button className="row-btn" onClick={onClose} title={t("common.cancel")} aria-label={t("schedules.cancelCreate")}>
          <CloseIcon />
        </button>
      </div>

      <input
        className="sp-d-name"
        autoFocus
        value={name}
        onChange={(e) => setName(e.currentTarget.value)}
        placeholder={t("schedules.form.name")}
        aria-label={t("schedules.form.name")}
        spellCheck={false}
      />

      <ResizableTextarea
        className="preset-body-input"
        value={prompt}
        onChange={(e) => setPrompt(e.currentTarget.value)}
        rows={6}
        placeholder={t("schedules.form.promptPlaceholder")}
        aria-label={t("schedules.form.prompt")}
        spellCheck={false}
      />

      <div className="sp-d-caption">{t("schedules.section.details")}</div>
      <div className="sp-d-group">
        <div className="sp-d-row">
          <span>{t("schedules.form.runIn")}</span>
          <FieldSelect
            className="sp-d-field"
            value={runIn}
            onChange={(v) => setRunIn(v as "new" | "session")}
            options={[
              { value: "new", label: t("schedules.newSession") },
              { value: "session", label: t("schedules.existingSession") },
            ]}
          />
        </div>
        {runIn === "session" ? (
          <div className="sp-d-row">
            <span>{t("schedules.form.session")}</span>
            <SessionPicker sessions={sessions} value={sessionId} onPick={setSessionId} />
          </div>
        ) : (
          <div className="sp-d-row">
            <span>{t("schedules.form.project")}</span>
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

      <div className="sp-d-caption">{t("schedules.section.frequency")}</div>
      <div className="sp-d-group">
        <div className="sp-d-row">
          <span>{t("schedules.form.repeat")}</span>
          <FieldSelect
            className="sp-d-field"
            value={choice}
            onChange={(v) => setChoice(v as RepeatChoice)}
            options={repeatOptions()}
          />
        </div>
        {choice === "once" ? (
          <div className="sp-d-row">
            <span>{t("schedules.form.datetime")}</span>
            <DateTimePicker className="sp-d-field" value={onceAt} onChange={setOnceAt} />
          </div>
        ) : choice === "every" ? (
          <IntervalRow value={every} onChange={setEvery} />
        ) : (
          <div className="sp-d-row">
            <span>{t("schedules.form.time")}</span>
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
  const { t } = useT();
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
  const untitled = t("schedules.newSession");
  const label = current ? (current.title ?? untitled) : t("schedules.picker.placeholder");

  /** 按项目分组，组内新的在前（和侧栏同序）。 */
  const groups = useMemo(() => {
    const needle = q.trim().toLowerCase();
    const hit = sessions.filter(
      (s) => !needle || (s.title ?? untitled).toLowerCase().includes(needle),
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
  }, [sessions, q, untitled]);

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
            placeholder={t("schedules.picker.search")}
            spellCheck={false}
          />
          <div className="sp-picker-list">
            {groups.length === 0 ? (
              <div className="sp-picker-empty">{t("schedules.picker.empty")}</div>
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
                      {s.title ?? untitled}
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
