import { useEffect, useState } from "react";

import { onHostReconnect } from "../bridge";

/**
 * 每次与宿主的连接**重新**建立就加一。
 *
 * 给那些"订阅一次、靠宿主一直推"的 effect 当依赖：把它放进依赖数组，
 * 重连后 effect 自动重跑 —— 先 cleanup（对已作废的旧连接无害）再重新
 * 订阅。终端面板、浏览器面板都是这个形状。桌面上永远是 0。
 */
export function useHostReconnectTick(): number {
  const [tick, setTick] = useState(0);
  useEffect(() => onHostReconnect(() => setTick((t) => t + 1)), []);
  return tick;
}
