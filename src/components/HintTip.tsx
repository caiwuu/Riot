import { type ReactNode, useRef, useState } from "react";
import { createPortal } from "react-dom";

/**
 * 标题旁的「？」：解释性文字收在这里，悬停才展开。
 * 弹出层用 fixed，避免被设置页的 overflow 裁掉。
 */
export function HintTip({ children }: { children: ReactNode }) {
  const btn = useRef<HTMLButtonElement>(null);
  const [box, setBox] = useState<{ top: number; left: number; up: boolean } | null>(null);

  const show = () => {
    const r = btn.current?.getBoundingClientRect();
    if (!r) return;
    const up = window.innerHeight - r.bottom < 160;
    const left = Math.min(Math.max(8, r.left), window.innerWidth - 328);
    setBox({ top: up ? r.top - 6 : r.bottom + 6, left, up });
  };

  return (
    <span className="hint-tip">
      <button
        ref={btn}
        type="button"
        className="hint-tip-btn"
        aria-label="说明"
        onMouseEnter={show}
        onMouseLeave={() => setBox(null)}
        onFocus={show}
        onBlur={() => setBox(null)}
      >
        ?
      </button>
      {box
        ? createPortal(
            <span
              className="hint-tip-pop"
              role="tooltip"
              style={{
                top: box.top,
                left: box.left,
                transform: box.up ? "translateY(-100%)" : undefined,
              }}
            >
              {children}
            </span>,
            document.body,
          )
        : null}
    </span>
  );
}
