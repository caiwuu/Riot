import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import { Chevron } from "./Chevron";

export interface FieldOption {
  value: string;
  label: string;
  /** 次要说明。渲染成主标签下方的小号灰字，触发框里不显示。 */
  hint?: string;
}

/**
 * 设置页的下拉。不用原生 `<select>`：WKWebView 里系统弹出层的坐标
 * 经常对不齐触发框（尤其在带 overflow 的弹窗里），菜单会飘到标签上。
 * 这里按按钮的 getBoundingClientRect 用 fixed 贴在正下方，空间不够再翻上去。
 */
export function FieldSelect({
  value,
  onChange,
  options,
  disabled,
  className,
  title,
  menuMinWidth,
}: {
  value: string;
  onChange: (v: string) => void;
  options: FieldOption[];
  disabled?: boolean;
  className?: string;
  title?: string;
  /** 菜单比触发框更宽时用。触发框可以很窄，选项（远程分支名）往往更长。 */
  menuMinWidth?: number;
}) {
  const [open, setOpen] = useState(false);
  const btnRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [box, setBox] = useState<{
    top: number;
    left: number;
    width: number;
    maxH: number;
    up: boolean;
  } | null>(null);

  const place = () => {
    const el = btnRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const gap = 6;
    const below = window.innerHeight - r.bottom - gap - 8;
    const above = r.top - gap - 8;
    const up = below < 160 && above > below;
    const width = Math.min(
      window.innerWidth - 16,
      Math.max(r.width, menuMinWidth ?? 0),
    );
    // 菜单比按钮宽时仍贴左边；贴出窗口就往左收。
    const left = Math.min(r.left, window.innerWidth - width - 8);
    setBox({
      top: up ? r.top - gap : r.bottom + gap,
      left: Math.max(8, left),
      width,
      maxH: Math.min(280, Math.max(120, up ? above : below)),
      up,
    });
  };

  useEffect(() => {
    if (!open) return;
    place();
    const onScroll = () => place();
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (btnRef.current?.contains(t) || menuRef.current?.contains(t)) return;
      setOpen(false);
    };
    window.addEventListener("resize", onScroll);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("mousedown", onDown);
    return () => {
      window.removeEventListener("resize", onScroll);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("mousedown", onDown);
    };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const items = menuRef.current?.querySelectorAll<HTMLButtonElement>("[role=option]");
    const cur = [...(items ?? [])].find((b) => b.getAttribute("aria-selected") === "true");
    (cur ?? items?.[0])?.focus();
  }, [open, box]);

  const current = options.find((o) => o.value === value);

  return (
    <>
      <button
        ref={btnRef}
        type="button"
        className={className ? `field-select ${className}` : "field-select"}
        disabled={disabled}
        title={title}
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => !disabled && setOpen((v) => !v)}
      >
        <span className="field-select-label">{current?.label ?? ""}</span>
        <Chevron down open={open} />
      </button>
      {open && box
        ? createPortal(
            <div
              ref={menuRef}
              className="field-select-menu"
              role="listbox"
              style={{
                left: box.left,
                width: box.width,
                maxHeight: box.maxH,
                ...(box.up
                  ? { bottom: window.innerHeight - box.top }
                  : { top: box.top }),
              }}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  e.preventDefault();
                  e.stopPropagation();
                  setOpen(false);
                  btnRef.current?.focus();
                  return;
                }
                if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
                e.preventDefault();
                const items = [
                  ...(menuRef.current?.querySelectorAll<HTMLButtonElement>("[role=option]") ?? []),
                ];
                if (!items.length) return;
                const i = items.indexOf(document.activeElement as HTMLButtonElement);
                const n = items.length;
                const next = e.key === "ArrowDown" ? (i + 1) % n : (i - 1 + n) % n;
                items[next]?.focus();
              }}
            >
              {options.map((o) => (
                <button
                  key={o.value || "__empty"}
                  type="button"
                  role="option"
                  aria-selected={o.value === value}
                  className={o.value === value ? "menu-item active" : "menu-item"}
                  onClick={() => {
                    onChange(o.value);
                    setOpen(false);
                    btnRef.current?.focus();
                  }}
                >
                  {o.label}
                  {o.hint ? <span className="menu-hint">{o.hint}</span> : null}
                </button>
              ))}
            </div>,
            document.body,
          )
        : null}
    </>
  );
}
