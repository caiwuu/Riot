import { type PointerEvent, type TextareaHTMLAttributes, useRef, useState } from "react";

const H = { min: 72, max: 720 };

/**
 * 可拖高度的多行输入。不用 CSS `resize`：WKWebView 在 `appearance: none`
 * 下把系统拉伸角标吃掉，拖了等于没拖。右下角那块才是真的拖高度。
 */
export function ResizableTextarea({
  className,
  minHeight = H.min,
  maxHeight = H.max,
  style,
  ...props
}: TextareaHTMLAttributes<HTMLTextAreaElement> & {
  minHeight?: number;
  maxHeight?: number;
}) {
  const ref = useRef<HTMLTextAreaElement>(null);
  const [h, setH] = useState<number>();
  const drag = useRef<{ y: number; h: number } | null>(null);

  const onGripDown = (e: PointerEvent<HTMLButtonElement>) => {
    if (e.button !== 0) return;
    // 拦住默认：否则焦点从 textarea 跑到按钮上，失焦提交会在拖的半途开火。
    e.preventDefault();
    const el = ref.current;
    if (!el) return;
    drag.current = { y: e.clientY, h: el.offsetHeight };
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      /* 合成事件没有活跃指针 */
    }
  };

  const onGripMove = (e: PointerEvent<HTMLButtonElement>) => {
    const d = drag.current;
    if (!d) return;
    setH(Math.min(maxHeight, Math.max(minHeight, d.h + (e.clientY - d.y))));
  };

  const onGripUp = () => {
    drag.current = null;
  };

  return (
    <div className="ta-resize">
      <textarea
        {...props}
        ref={ref}
        className={className}
        style={h != null ? { ...style, height: h } : style}
      />
      <button
        type="button"
        className="ta-resize-grip"
        tabIndex={-1}
        aria-label="拖动调整高度"
        title="拖动调整高度"
        onPointerDown={onGripDown}
        onPointerMove={onGripMove}
        onPointerUp={onGripUp}
        onPointerCancel={onGripUp}
      />
    </div>
  );
}
