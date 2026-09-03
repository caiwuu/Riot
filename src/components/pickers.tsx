/**
 * 通用下拉选择件：模式菜单（ModeMenu）、单选下拉（Picker），以及
 * 它们共用的开合/键盘导航逻辑（useDropdown）。从 App.tsx 拆出。
 */

import { useEffect, useRef, useState } from "react";

import type { PermissionMode, ProviderConfig } from "../bridge";
import { Chevron } from "./Chevron";
import { EyeIcon, PlanModeIcon } from "./icons";

const MODE_LABEL: Record<string, string> = {
  default: "每次询问",
  acceptEdits: "编辑放行",
  plan: "Plan",
  auto: "自动判危",
  bypassPermissions: "全部放行",
  unattended: "无人值守",
};

/** 权限档位：执行前问多少。和「工作方式」分组展示，后端仍是同一个 PermissionMode。 */
const PERMISSION_MODES = [
  "default",
  "acceptEdits",
  "auto",
  "bypassPermissions",
  "unattended",
] as const satisfies readonly PermissionMode[];

/** 菜单里跟在模式名后面的警示语。没有就是不需要提醒。 */
const MODE_WARN: Record<string, string> = {
  bypassPermissions: "风险自负",
  unattended: "含危险操作",
};

/** 后端 PermissionMode 里真正表示「执行前问多少」的档位（不含 plan）。 */
export function isExecPermissionMode(m: PermissionMode): boolean {
  return (PERMISSION_MODES as readonly PermissionMode[]).includes(m);
}

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

/**
 * 模式菜单：权限档位（单选）+ 工作方式（规划 / 多任务，互斥单选）。
 *
 * 选中态用 Cursor 同款右侧勾号。规划与多任务互斥；都不选时即普通 agent。
 * 后端仍把 plan 收在 PermissionMode 里；多任务是独立开关，Composer 里做互斥。
 */
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
  warn,
  title,
  onPick,
}: {
  label: string;
  active: boolean;
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
        <span className="menu-pick-label">{label}</span>
        {warn ? <span className="menu-warn">{warn}</span> : null}
      </span>
      <MenuPickCheck active={active} />
    </button>
  );
}

export function ModeMenu({
  mode,
  execMode = "default",
  onChange,
  multitask = false,
  onMultitask,
}: {
  mode: PermissionMode;
  /** 规划模式下 pill 要报的权限档 —— 计划批准后按它执行。 */
  execMode?: PermissionMode;
  onChange: (m: PermissionMode) => void;
  multitask?: boolean;
  onMultitask?: (on: boolean) => void;
}) {
  const [open, setOpen] = useState(false);
  const { rootRef, onKeyDown } = useDropdown(open, setOpen);
  const planning = mode === "plan";
  // 规划模式不是一档权限，pill 上仍报权限档 + 一个规划记号。写成「规划模式」
  // 会把用户设的档位藏起来，而批准计划之后正是按那一档动手的。
  const shown = planning ? execMode : mode;
  // 危险模式常态化后不能和安全模式长得一样 —— pill 要一直带警示色
  const danger = Boolean(MODE_WARN[shown]);
  const label = MODE_LABEL[shown] ?? shown;
  const tip = `${label}${danger ? `（${MODE_WARN[shown]}）` : ""}${planning ? " · 规划模式" : ""}${multitask ? " · 多任务" : ""}`;

  return (
    <div className="mode-menu" ref={rootRef} onKeyDown={onKeyDown}>
      <button
        type="button"
        className={danger ? "pill picker-pill pill-danger" : "pill picker-pill"}
        title={tip}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        {/* 对齐做在内层：WebKit 的 button 会套一层匿名盒，align-items
            落不到子项上，图标就贴着汉字上沿。span 没有这个问题。 */}
        <span className="picker-pill-row">
          {danger ? <span className="pill-danger-dot" aria-hidden /> : null}
          <span className="pick-label">{label}</span>
          {planning ? (
            <span className="pill-plan" aria-label="规划模式开着" title="规划模式">
              <PlanModeIcon />
            </span>
          ) : null}
          {multitask ? (
            <span className="pill-multitask" aria-label="多任务模式开着" title="多任务模式">
              ⑂
            </span>
          ) : null}
          <Chevron down open={open} />
        </span>
      </button>
      {open ? (
        <div className="menu" role="menu">
          <div className="menu-group-title">权限</div>
          {PERMISSION_MODES.map((m) => (
            <ModeMenuItem
              key={m}
              label={MODE_LABEL[m] ?? m}
              {...(MODE_WARN[m] ? { warn: MODE_WARN[m] } : {})}
              active={m === shown}
              onPick={() => {
                onChange(m);
                setOpen(false);
              }}
            />
          ))}
          <div className="menu-sep" role="separator" />
          <div className="menu-group-title">工作方式</div>
          <ModeMenuItem
            label={MODE_LABEL.plan ?? "plan"}
            active={planning}
            title="只读侦察并产出计划，批准后才动手。再点一次退回权限档。"
            onPick={() => {
              // 和多任务一样是可开可关的：再点一次退回它进来前那一档权限。
              onChange(planning ? execMode : "plan");
              setOpen(false);
            }}
          />
          {onMultitask ? (
            <ModeMenuItem
              label="多任务"
              active={multitask}
              title="主 agent 只协调，实质工作交给后台子 agent；委派完就结束回合，做完通知你。适合几分钟起的任务、边等边聊。"
              onPick={() => {
                onMultitask(!multitask);
                setOpen(false);
              }}
            />
          ) : null}
        </div>
      ) : null}
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
            <span className="cap-icon" role="img" aria-label="能收图片" title="能收图片">
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
