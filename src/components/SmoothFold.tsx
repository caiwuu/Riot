import { useEffect, useLayoutEffect, useState, type ReactNode, type TransitionEvent } from "react";

/**
 * 高度动画的折叠。收起时**不挂载**孩子。
 *
 * 长对话里几十组过程、上百张工具卡如果为了一次开合动画一直占着 DOM，
 * 切回会话会把主线程卡死，表现为白屏。打开：先挂上（0fr）再在下一帧
 * 加 `.open`（1fr），才能播展开动画。收起：先去掉 `.open`，等
 * `transitionend` 再卸掉孩子。
 */

/** transitionend 收不到时的兜底。要比 styles.css 的 `--dur-3` 长。 */
const FOLD_FALLBACK_MS = 340;
export function SmoothFold({
  open,
  children,
}: {
  open: boolean;
  children: ReactNode;
}) {
  const [mounted, setMounted] = useState(open);
  const [shown, setShown] = useState(open);

  useLayoutEffect(() => {
    if (open) {
      setMounted(true);
      if (prefersReducedMotion()) {
        setShown(true);
        return;
      }
      // 两帧：第一帧把孩子画在 0fr 里，第二帧才加 open。一帧的话
      // 浏览器经常把两步合成一次，动画没了。
      let inner = 0;
      const outer = requestAnimationFrame(() => {
        inner = requestAnimationFrame(() => setShown(true));
      });
      return () => {
        cancelAnimationFrame(outer);
        cancelAnimationFrame(inner);
      };
    }
    setShown(false);
    if (prefersReducedMotion()) setMounted(false);
  }, [open]);

  // transitionend 在父级 `display:none`（切走会话）时可能不来。
  // 超时兜底，别让关过的内容永远占着。
  //
  // `[约束]` 必须比 styles.css 的 `--dur-3` 长（.smooth-fold 的过渡走
  // 那一档）。短了的话兜底会赶在动画播完之前把孩子卸掉，收起动画在
  // 半路上被砍断 —— 而 transitionend 那条正常路径根本不会走到。
  useEffect(() => {
    if (open || !mounted) return;
    const t = window.setTimeout(() => setMounted(false), FOLD_FALLBACK_MS);
    return () => window.clearTimeout(t);
  }, [open, mounted]);

  const onTransitionEnd = (e: TransitionEvent<HTMLDivElement>) => {
    if (e.target !== e.currentTarget) return;
    if (e.propertyName !== "grid-template-rows") return;
    if (!open) setMounted(false);
  };

  if (!mounted) return null;

  return (
    <div
      className={shown ? "smooth-fold open" : "smooth-fold"}
      inert={!shown}
      aria-hidden={!shown}
      onTransitionEnd={onTransitionEnd}
    >
      <div className="smooth-fold-inner">{children}</div>
    </div>
  );
}

function prefersReducedMotion(): boolean {
  return (
    typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}
