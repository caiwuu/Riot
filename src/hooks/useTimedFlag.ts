import { useCallback, useEffect, useRef, useState } from "react";

/**
 * 一闪即逝的状态：`flash(v)` 置成 `v`，`ms` 之后自己回到 `idle`。
 *
 * 界面上那些「已复制」「打不开」的短提示都是这个形状。之前每处各写一句
 * `setTimeout(() => setX(false), n)`，两个问题：
 *
 * - 卸载不撤。React 18 起对卸载后的 setState 不再警告，所以后果不是报错
 *   而是白跑一次调度 —— 而这些提示恰好长在流式期间成批挂载卸载的消息行、
 *   代码块、预览面板上，一次长回答能攒下几十个只为改一个没人看的状态的
 *   定时器。
 * - 连点不撤。第一次的定时器会把第二次刚亮起的提示提前掐掉，看起来像
 *   "点快了就不认"。
 *
 * 泛型是为了兼容 `"idle" | "ok" | "fail"` 这种三态（复制成功和复制失败
 * 要显示不同的图标）；布尔是它最常见的那一种。
 */
export function useTimedFlag<T>(idle: T, ms: number): [T, (value: T) => void] {
  const [value, setValue] = useState<T>(idle);
  const timer = useRef(0);

  const flash = useCallback(
    (next: T) => {
      setValue(next);
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setValue(idle), ms);
    },
    [idle, ms],
  );

  useEffect(() => () => window.clearTimeout(timer.current), []);

  return [value, flash];
}
