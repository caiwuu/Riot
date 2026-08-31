/**
 * 布局面板（侧栏 / 抽屉 / 终端）的开合动画壳。
 *
 * - 尺寸动画只发生在壳上（width/height 在 0 与定值之间过渡），内容躺在
 *   **固定尺寸**的内层里被裁切 —— 不跟着重排：面板内部不变形，终端的
 *   xterm 也不会在动画中反复 fit。
 * - 关闭后拖一拍再卸载（退出动画要看得见）；期间渲染最后一帧的内容 ——
 *   调用方在关闭时往往已经拿不出内容了（详情面板的任务被取消选中）。
 * - transition 只在开合瞬间挂上、动画走完就摘 —— 拖宽调的是同一个
 *   width，常挂 transition 会让宽度追着鼠标发飘。
 */

import { useEffect, useLayoutEffect, useRef, useState } from "react";

/** 动画时长。壳的 transition 和延迟卸载共用，改要一起改。 */
const SLIDE_MS = 240;

/** open 翻 false 后再撑一拍（给退出动画留时间）。 */
export function usePresence(open: boolean, ms: number = SLIDE_MS + 40): boolean {
  const [present, setPresent] = useState(open);
  useEffect(() => {
    if (open) {
      setPresent(true);
      return;
    }
    const t = window.setTimeout(() => setPresent(false), ms);
    return () => window.clearTimeout(t);
  }, [open, ms]);
  return present || open;
}

export function SlidePanel({
  open,
  size,
  axis,
  anchor = "start",
  keepMounted,
  className,
  onVisualOpen,
  children,
}: {
  open: boolean;
  /** 展开后的宽（axis=x）或高（axis=y）。拖宽时它高频变化，瞬时跟随。 */
  size: number;
  axis: "x" | "y";
  /** 裁哪一边。右侧抽屉 / 底栏必须 `end`：壳变窄时贴着窗口外缘，
   *  裁的是靠主区的那一侧。默认 `start` 是左侧栏。 */
  anchor?: "start" | "end";
  /** 关死后也不卸载内容（终端要保住 xterm 的回滚缓冲），只把壳收到 0。 */
  keepMounted?: boolean;
  className?: string;
  /** 壳真正开始改尺寸的那一拍。顶栏红绿灯让位必须跟这一拍对齐，
   *  跟 `open` 对齐会先挤到右边（侧栏还在，让位已经加上）。 */
  onVisualOpen?: (open: boolean) => void;
  children: React.ReactNode;
}) {
  const present = usePresence(open);
  /** 展开的目标状态。初值 = open：启动时就开着的面板不播入场动画。 */
  const [shown, setShown] = useState(open);
  /** 开合瞬间才挂 transition，走完摘掉（拖宽不能有）。 */
  const [animating, setAnimating] = useState(false);
  const first = useRef(true);
  const shellRef = useRef<HTMLDivElement>(null);
  const onVisualOpenRef = useRef(onVisualOpen);
  onVisualOpenRef.current = onVisualOpen;
  const last = useRef(children);
  if (open && children != null) last.current = children;

  useLayoutEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    const el = shellRef.current;
    // `[约束]` transition 必须已经在计算样式里，下一拍的宽/高才会过渡。
    // 同一次 commit 里既加 .animating 又改 width，按规范这次变化不产生
    // 过渡，面板瞬跳。先写 class、强制重排，再让 React 改尺寸。
    if (el) {
      el.classList.add("animating");
      void el.offsetWidth;
    }
    setAnimating(true);
    setShown(open);
    onVisualOpenRef.current?.(open);
    const anim = window.setTimeout(() => setAnimating(false), SLIDE_MS + 40);
    return () => window.clearTimeout(anim);
  }, [open]);

  if (!present && !keepMounted) return null;

  const cls = [
    "slide-panel",
    axis === "x" ? "ax" : "ay",
    anchor === "end" ? "end" : "",
    animating ? "animating" : "",
    className ?? "",
  ]
    .filter(Boolean)
    .join(" ");
  const shellStyle =
    axis === "x" ? { width: shown ? size : 0 } : { height: shown ? size : 0 };
  const innerStyle = axis === "x" ? { width: size } : { height: size };
  // 有现场 children 就用（侧栏 / 终端 keepMounted 一直有）。
  // 父级已经卸掉内容时（抽屉、任务详情关了），退出动画用最后一帧。
  const live = children != null && (open || keepMounted);
  const body = live ? children : present ? last.current : null;

  return (
    <div ref={shellRef} className={cls} style={shellStyle}>
      <div className="slide-panel-inner" style={innerStyle}>
        {body}
      </div>
    </div>
  );
}
