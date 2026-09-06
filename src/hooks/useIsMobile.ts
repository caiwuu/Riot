import { useEffect, useState } from "react";

/**
 * 窄屏（手机）判据。和 styles.css 里 `@media (max-width: 720px)` 那组
 * 移动端规则**同一个数** —— 布局在 CSS 里切、行为在这里切，两边对不上
 * 的话会出现"侧栏已经是抽屉了，点会话却不收起"这种半吊子状态。
 */
export const MOBILE_QUERY = "(max-width: 720px)";

/** 此刻是不是窄屏。给 useState 的初值用，避免首帧按桌面布局闪一下。 */
export function isMobileNow(): boolean {
  return typeof window !== "undefined" && window.matchMedia(MOBILE_QUERY).matches;
}

export function useIsMobile(): boolean {
  const [mobile, setMobile] = useState(isMobileNow);
  useEffect(() => {
    const mq = window.matchMedia(MOBILE_QUERY);
    const on = () => setMobile(mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);
  return mobile;
}
