/**
 * 通用下拉选择件：模式菜单（ModeMenu）、单选下拉（Picker），以及
 * 它们共用的开合/键盘导航逻辑（useDropdown）。从 App.tsx 拆出。
 */

import { useEffect, useRef, useState } from "react";

import type { PermissionMode, ProviderConfig } from "../bridge";
import { Chevron } from "./Chevron";
import { EyeIcon } from "./icons";

const MODE_LABEL: Record<string, string> = {
  default: "每次询问",
  acceptEdits: "自动接受编辑",
  plan: "规划模式",
  auto: "自动判危",
  bypassPermissions: "全部放行",
  unattended: "无人值守",
};

/** 菜单里跟在模式名后面的警示语。没有就是不需要提醒。 */
const MODE_WARN: Record<string, string> = {
  bypassPermissions: "风险自负",
  unattended: "含危险操作",
};

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

/** 权限模式的上拉菜单。原生 select 样式改不动，自己画一个。 */
export function ModeMenu({
  mode,
  onChange,
}: {
  mode: PermissionMode;
  onChange: (m: PermissionMode) => void;
}) {
  const [open, setOpen] = useState(false);
  const { rootRef, onKeyDown } = useDropdown(open, setOpen);
  // 危险模式常态化后不能和安全模式长得一样 —— pill 要一直带警示色
  const danger = Boolean(MODE_WARN[mode]);

  return (
    <div className="mode-menu" ref={rootRef} onKeyDown={onKeyDown}>
      <button
        type="button"
        className={danger ? "pill picker-pill pill-danger" : "pill picker-pill"}
        title={danger ? `${MODE_LABEL[mode]}（${MODE_WARN[mode]}）` : (MODE_LABEL[mode] ?? mode)}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        {danger ? <span className="pill-danger-dot" aria-hidden /> : null}
        <span className="pick-label">{MODE_LABEL[mode] ?? mode}</span>
        <Chevron down open={open} />
      </button>
      {open ? (
        <div className="menu" role="menu">
          {(Object.keys(MODE_LABEL) as PermissionMode[]).map((m) => (
            <button
              key={m}
              type="button"
              role="menuitemradio"
              aria-checked={m === mode}
              className={m === mode ? "menu-item active" : "menu-item"}
              onClick={() => {
                onChange(m);
                setOpen(false);
              }}
            >
              {MODE_LABEL[m]}
              {MODE_WARN[m] ? <span className="menu-warn">{MODE_WARN[m]}</span> : null}
            </button>
          ))}
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
      className={item.active ? "menu-item picker-item active" : "menu-item picker-item"}
      onClick={onPick}
    >
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
        <span className="pick-label">{label}</span>
        <Chevron down open={open} />
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
