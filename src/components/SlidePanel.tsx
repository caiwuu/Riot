/**
 * 布局面板（侧栏 / 抽屉 / 终端）的开合动画壳。
 *
 * - 尺寸动画只发生在壳上（width/height 在 0 与定值之间过渡），内容躺在
 *   **固定尺寸**的内层里被裁切 —— 不跟着重排：面板内部不变形，终端的
 *   xterm 也不会在动画中反复 fit。
 * - 关闭后拖一拍再卸载（退出动画要看得见）；期间渲染最后一帧的内容 ——
 *   调用方在关闭时往往已经拿不出内容了（详情面板的任务被取消选中）。
 * - transition 挂在 CSS 里、常驻，拖动时由 `.rz.dragging` / `[data-resizing]`
 *   摘掉（见 styles.css 的 .slide-panel）。所以这里只管尺寸，不碰 class、
 *   不强制重排 —— 尺寸在哪一拍变，过渡就在哪一拍起。
 */

import { useEffect, useLayoutEffect, useRef, useState } from "react";

/** 壳的过渡时长。必须和 styles.css 的 `--dur-3` 一致 —— 短了的话
 *  内容会在收起动画播完之前就被卸掉，看到的是空壳在滑。 */
const SLIDE_MS = 300;

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
  /** 壳改尺寸的那一拍。顶栏红绿灯让位跟着它走，两边同一帧起步、
   *  同一条曲线，中间不会有"侧栏还在、让位已经加上"的错帧。 */
  onVisualOpen?: (open: boolean) => void;
  children: React.ReactNode;
}) {
  const present = usePresence(open);
  const onVisualOpenRef = useRef(onVisualOpen);
  onVisualOpenRef.current = onVisualOpen;
  const last = useRef(children);
  if (open && children != null) last.current = children;

  // 壳的尺寸和 `open` 同一拍变（下面直接读 open），所以这一拍就是"真正
  // 开始改尺寸"的那一拍 —— 顶栏让位跟着它走才不会先挤到一边再弹回来。
  // layout effect 里通知：调用方的重渲染仍在 paint 之前，画面上同帧。
  //
  // 挂载时也会通知一次。调用方的初值本来就等于 open（见 App 的
  // sidebarVisual / drawerVisual），同值 setState 被 React 挡掉，
  // 所以不用像早先那样专门留一个 first ref 去跳过首次。
  useLayoutEffect(() => {
    onVisualOpenRef.current?.(open);
  }, [open]);

  if (!present && !keepMounted) return null;

  const cls = [
    "slide-panel",
    axis === "x" ? "ax" : "ay",
    anchor === "end" ? "end" : "",
    className ?? "",
  ]
    .filter(Boolean)
    .join(" ");
  // 直接读 open，不再经一层 state 延后一拍。新挂载的元素没有"变化前"的
  // 值，不会播过渡 —— 启动时就开着的面板照旧安静地在那儿。
  const shellStyle = axis === "x" ? { width: open ? size : 0 } : { height: open ? size : 0 };
  const innerStyle = axis === "x" ? { width: size } : { height: size };
  // 有现场 children 就用（侧栏 / 终端 keepMounted 一直有）。
  // 父级已经卸掉内容时（抽屉、任务详情关了），退出动画用最后一帧。
  const live = children != null && (open || keepMounted);
  const body = live ? children : present ? last.current : null;

  return (
    <div className={cls} style={shellStyle}>
      <div className="slide-panel-inner" style={innerStyle}>
        {body}
      </div>
    </div>
  );
}
