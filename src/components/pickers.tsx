/**
 * 通用下拉选择件：模式菜单（ModeMenu）、单选下拉（Picker），以及
 * 它们共用的开合/键盘导航逻辑（useDropdown）。从 App.tsx 拆出。
 */

import { type ReactNode, useEffect, useRef, useState } from "react";

import type { PermissionMode, ProviderConfig } from "../bridge";
import { type MessageKey, useT } from "../i18n";
import { Chevron } from "./Chevron";
import { ConfirmDialog, type ConfirmRequest } from "./ConfirmDialog";
import { AgentModeIcon, EyeIcon, MultitaskIcon, PlanModeIcon } from "./icons";

/** 权限档名的词典键。渲染时 `t()`，不缓存翻译结果。 */
export const MODE_LABEL_KEY: Record<string, MessageKey> = {
  default: "composer.perm.default",
  acceptEdits: "composer.perm.acceptEdits",
  plan: "composer.perm.plan",
  auto: "composer.perm.auto",
  bypassPermissions: "composer.perm.bypassPermissions",
  unattended: "composer.perm.unattended",
};

/**
 * 权限档位：执行前问多少。在顶栏的权限下拉里选。
 * 后端把 plan 也收在 PermissionMode 里，但它是「工作方式」，不在这一列。
 */
export const PERMISSION_MODES = [
  "default",
  "acceptEdits",
  "auto",
  "bypassPermissions",
  "unattended",
] as const satisfies readonly PermissionMode[];

/** 跟在权限档名后面的警示语（词典键）。没有就是不需要提醒。 */
export const MODE_WARN_KEY: Record<string, MessageKey> = {
  bypassPermissions: "composer.perm.bypassPermissions.warn",
  unattended: "composer.perm.unattended.warn",
};

/** 后端 PermissionMode 里真正表示「执行前问多少」的档位（不含 plan）。 */
export function isExecPermissionMode(m: PermissionMode): boolean {
  return (PERMISSION_MODES as readonly PermissionMode[]).includes(m);
}

/**
 * 工作方式：agent 怎么干活。和权限档是两个维度 —— 权限管"做之前问不问"，
 * 工作方式管"先想还是先做、自己做还是派人做"。互斥单选。
 */
export type WorkMode = "agent" | "plan" | "multitask";

const WORK_MODES: { id: WorkMode; labelKey: MessageKey; titleKey: MessageKey }[] = [
  { id: "agent", labelKey: "composer.mode.agent", titleKey: "composer.mode.agent.hint" },
  { id: "plan", labelKey: "composer.mode.plan", titleKey: "composer.mode.plan.hint" },
  { id: "multitask", labelKey: "composer.mode.multitask", titleKey: "composer.mode.multitask.hint" },
];

/**
 * 每个会话的未发送草稿。挂在模块级：Chat 按会话 id 重挂载，组件内
 * state 活不过切换 —— 用户打了一半的字换个会话再回来就没了，那是
 * 真实的内容损失。进程内存足够，不值得为草稿上持久化。
 */

export function modelLabel(p: ProviderConfig | null, id: string): string {
  return p?.models.find((m) => m.id === id)?.name?.trim() || id;
}

/**
 * 上拉菜单的公共行为：点外面关、Esc 关（焦点还给 pill）、上下键在
 * 菜单项间移动。三个 pill 菜单（模式/服务方/模型）共用，键盘模型
 * 才能一致。
 */
export function useDropdown(open: boolean, setOpen: (v: boolean) => void) {
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [open, setOpen]);

  // 打开后把焦点放到当前选中项，键盘用户不用先按好几下 Tab
  useEffect(() => {
    if (!open) return;
    const root = rootRef.current;
    const target =
      root?.querySelector<HTMLButtonElement>(".menu-item.active") ??
      root?.querySelector<HTMLButtonElement>(".menu-item");
    target?.focus();
  }, [open]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!open) return;
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
      rootRef.current?.querySelector<HTMLButtonElement>(".picker-pill")?.focus();
      return;
    }
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const items = [...(rootRef.current?.querySelectorAll<HTMLButtonElement>(".menu-item") ?? [])];
      if (!items.length) return;
      const cur = items.indexOf(document.activeElement as HTMLButtonElement);
      const n = items.length;
      const next = e.key === "ArrowDown" ? (cur + 1) % n : (cur - 1 + n) % n;
      items[next]?.focus();
    }
  };

  return { rootRef, onKeyDown };
}

function MenuPickCheck({ active }: { active: boolean }) {
  return (
    <svg
      className={active ? "menu-pick-check" : "menu-pick-check menu-pick-check--empty"}
      width="12"
      height="12"
      viewBox="0 0 12 12"
      aria-hidden
    >
      {active ? (
        <path
          d="M2.25 6.1 4.65 8.5 9.75 3.5"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      ) : null}
    </svg>
  );
}

function ModeMenuItem({
  label,
  active,
  icon,
  warn,
  title,
  onPick,
}: {
  label: string;
  active: boolean;
  /** 跟在名字前面的记号，和 pill 上的同一个。 */
  icon?: ReactNode;
  warn?: string;
  title?: string;
  onPick: () => void;
}) {
  return (
    <button
      type="button"
      role="menuitemradio"
      aria-checked={active}
      className={active ? "menu-item menu-pick active" : "menu-item menu-pick"}
      title={title}
      onClick={onPick}
    >
      <span className="menu-pick-body">
        <span className="menu-pick-label">
          {icon}
          {label}
        </span>
        {warn ? <span className="menu-warn">{warn}</span> : null}
      </span>
      <MenuPickCheck active={active} />
    </button>
  );
}

/**
 * 工作方式的记号（Cursor 同款）：Agent 是 ∞，Plan 是清单，多任务是分叉。
 * pill 和菜单项共用，换了方式一眼能对上。
 */
function WorkModeMark({ mode }: { mode: WorkMode }) {
  if (mode === "plan") {
    return (
      <span className="pill-mark pill-plan" aria-hidden>
        <PlanModeIcon />
      </span>
    );
  }
  if (mode === "multitask") {
    return (
      <span className="pill-mark pill-multitask" aria-hidden>
        <MultitaskIcon />
      </span>
    );
  }
  return (
    <span className="pill-mark pill-agent" aria-hidden>
      <AgentModeIcon />
    </span>
  );
}

/**
 * 工作方式菜单：Agent / Plan / 多任务，互斥单选，右侧勾号（Cursor 同款）。
 *
 * 只管工作方式，不管权限 —— 权限是"设一次长期不动"的，混进每条消息都
 * 可能切的菜单里，每次切 Plan 都要扫过五个权限档。后端仍把 plan 收在
 * PermissionMode 里，多任务是独立开关，两者的互斥在 Composer 里做。
 */
export function ModeMenu({
  value,
  onChange,
  canMultitask = true,
}: {
  value: WorkMode;
  onChange: (m: WorkMode) => void;
  canMultitask?: boolean;
}) {
  const { t } = useT();
  const [open, setOpen] = useState(false);
  const { rootRef, onKeyDown } = useDropdown(open, setOpen);
  const items = canMultitask ? WORK_MODES : WORK_MODES.filter((w) => w.id !== "multitask");
  const cur = WORK_MODES.find((w) => w.id === value) ?? WORK_MODES[0]!;
  const curLabel = t(cur.labelKey);

  return (
    <div className="mode-menu" ref={rootRef} onKeyDown={onKeyDown}>
      <button
        type="button"
        // Cursor 的做法：整个 pill 跟着模式换色，不只是图标。Agent 是常态，
        // 保持默认灰；Plan 黄、多任务绿，扫一眼就知道现在在哪种方式里。
        className={`pill picker-pill pill-mode-${value}`}
        title={t("composer.mode.title", { mode: curLabel })}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        {/* 对齐做在内层：WebKit 的 button 会套一层匿名盒，align-items
            落不到子项上，图标就贴着汉字上沿。span 没有这个问题。 */}
        <span className="picker-pill-row">
          <WorkModeMark mode={value} />
          <span className="pick-label">{curLabel}</span>
          <Chevron down open={open} />
        </span>
      </button>
      {open ? (
        <div className="menu" role="menu">
          <div className="menu-group-title">{t("composer.mode.group")}</div>
          {items.map((w) => (
            <ModeMenuItem
              key={w.id}
              label={t(w.labelKey)}
              active={w.id === value}
              icon={<WorkModeMark mode={w.id} />}
              title={t(w.titleKey)}
              onPick={() => {
                onChange(w.id);
                setOpen(false);
              }}
            />
          ))}
        </div>
      ) : null}
    </div>
  );
}

/**
 * 顶栏里跟在会话标题后面的权限下拉：既是状态也是唯一的切换入口。
 *
 * 危险档（全部放行 / 无人值守）常驻警示色和黄点，安全档淡灰 —— 它和
 * 标题同一行，一直在视野里，开着无人值守不至于忘掉。菜单向下展开
 * （输入框里那几个是向上）。无人值守要二次确认：它关掉的是最后一层
 * 保护，不能一次点击就生效。
 */
export function PermissionMenu({
  mode,
  onChange,
}: {
  mode: PermissionMode;
  onChange: (m: PermissionMode) => void;
}) {
  const { t } = useT();
  const [open, setOpen] = useState(false);
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  const { rootRef, onKeyDown } = useDropdown(open, setOpen);
  const labelKey = MODE_LABEL_KEY[mode];
  const label = labelKey ? t(labelKey) : mode;
  const warnKey = MODE_WARN_KEY[mode];
  const warn = warnKey ? t(warnKey) : undefined;

  const pick = (m: PermissionMode) => {
    setOpen(false);
    if (m === mode) return;
    if (m === "unattended") {
      setConfirm({
        title: t("composer.perm.unattended.confirm.title"),
        body: t("composer.perm.unattended.confirm.body"),
        confirmLabel: t("composer.perm.unattended.confirm.ok"),
        action: () => onChange(m),
      });
      return;
    }
    onChange(m);
  };

  return (
    <div className="mode-menu" ref={rootRef} onKeyDown={onKeyDown}>
      <button
        type="button"
        className={warn ? "pill picker-pill perm-flag pill-danger" : "pill picker-pill perm-flag"}
        title={
          warn
            ? t("composer.perm.titleWarn", { mode: label, warn })
            : t("composer.perm.title", { mode: label })
        }
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <span className="picker-pill-row">
          {warn ? <span className="pill-danger-dot" aria-hidden /> : null}
          <span className="pick-label">{label}</span>
          <Chevron down open={open} />
        </span>
      </button>
      {open ? (
        <div className="menu menu-down" role="menu">
          <div className="menu-group-title">{t("composer.perm.group")}</div>
          {PERMISSION_MODES.map((m) => {
            const lk = MODE_LABEL_KEY[m];
            const wk = MODE_WARN_KEY[m];
            return (
              <ModeMenuItem
                key={m}
                label={lk ? t(lk) : m}
                {...(wk ? { warn: t(wk) } : {})}
                active={m === mode}
                onPick={() => pick(m)}
              />
            );
          })}
        </div>
      ) : null}
      {/* portal：顶栏是窄条，遮罩要罩住整个窗口，不能就地渲染在它里面。 */}
      {confirm ? <ConfirmDialog c={confirm} portal onClose={() => setConfirm(null)} /> : null}
    </div>
  );
}

/**
 * 输入框里的内联下拉（服务方 / 模型共用）。样式沿用 ModeMenu 的
 * pill + 上拉菜单，长文本（模型名）截断，避免把整条工具栏撑开。
 */
export interface PickerItem {
  id: string;
  label: string;
  active?: boolean;
  /** 次要说明，靠右。默认淡灰；`warn` 才用黄，留给"未配置 key"这类。 */
  note?: string;
  warn?: boolean;
  /** 能收图片。图标跟在名字后面，不单独占一列。 */
  vision?: boolean;
}

/**
 * 主列表下方的第二组选项，带分隔线和小标题。
 *
 * 模型菜单用它放当前模型的上下文窗口档位。做成同一层的分组而不是二级
 * 弹出菜单：菜单项是 `button`，嵌一个能展开子菜单的按钮进去就成了嵌套
 * `button`（HTML 非法），而绕开它要把整行改成 `div` 自己实现键盘语义。
 * 分组的代价只是菜单长几行，换来的是上下键和读屏行为都不用重写。
 */
export interface PickerSection {
  title: string;
  items: PickerItem[];
  onPick: (id: string) => void;
}

function PickerRow({ item, onPick }: { item: PickerItem; onPick: () => void }) {
  const { t } = useT();
  return (
    <button
      type="button"
      role="menuitemradio"
      aria-checked={Boolean(item.active)}
      className={item.active ? "menu-item menu-pick picker-item active" : "menu-item menu-pick picker-item"}
      onClick={onPick}
    >
      <span className="menu-pick-body">
        <span className="pick-main">
          <span className="pick-label">{item.label}</span>
          {item.vision ? (
            <span
              className="cap-icon"
              role="img"
              aria-label={t("composer.model.vision")}
              title={t("composer.model.vision")}
            >
              <EyeIcon />
            </span>
          ) : null}
        </span>
        {item.note ? (
          <span className={item.warn ? "menu-warn" : "menu-hint"}>{item.note}</span>
        ) : null}
      </span>
      <MenuPickCheck active={Boolean(item.active)} />
    </button>
  );
}

export function Picker({
  label,
  title,
  items,
  onPick,
  emptyHint,
  onEmpty,
  section,
}: {
  label: string;
  title?: string;
  items: PickerItem[];
  onPick: (id: string) => void;
  /** 列表为空时点 pill 的提示与去向（一般是打开设置补模型）。 */
  emptyHint?: string;
  onEmpty?: () => void;
  section?: PickerSection;
}) {
  const [open, setOpen] = useState(false);
  const isEmpty = items.length === 0;
  const { rootRef, onKeyDown } = useDropdown(open, setOpen);

  return (
    <div className="mode-menu" ref={rootRef} onKeyDown={onKeyDown}>
      <button
        type="button"
        className="pill picker-pill"
        title={isEmpty ? (emptyHint ?? title) : label}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => (isEmpty ? onEmpty?.() : setOpen(!open))}
      >
        <span className="picker-pill-row">
          <span className="pick-label">{label}</span>
          <Chevron down open={open} />
        </span>
      </button>
      {open && !isEmpty ? (
        <div className="menu" role="menu">
          {items.map((it) => (
            <PickerRow
              key={it.id}
              item={it}
              onPick={() => {
                onPick(it.id);
                setOpen(false);
              }}
            />
          ))}
          {section && section.items.length ? (
            <>
              <div className="menu-sep" role="separator" />
              <div className="menu-group-title">{section.title}</div>
              {section.items.map((it) => (
                <PickerRow
                  key={it.id}
                  item={it}
                  onPick={() => {
                    section.onPick(it.id);
                    setOpen(false);
                  }}
                />
              ))}
            </>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
