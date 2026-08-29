import { useCallback, useEffect, useState } from "react";

import {
  browserState,
  type PanelState,
  type TabInfo,
  watchBrowserTabs,
} from "../bridge";

/** 面板刚起、宿主还没回状态时的样子。 */
export const EMPTY_PANEL: PanelState = { tabs: [], active: 0 };

/**
 * 浏览器的页面状态（页面标签组 + 激活页）提到 App 级持有。
 *
 * 页面标签渲染在工作台统一标签栏里，而浏览器面板只在激活时挂载 ——
 * 状态跟着面板走的话，切去预览标签的那一刻页面标签就全从标签栏上
 * 消失了。
 *
 * 两条更新路：清单变更（开 / 关 / 切页）走宿主推送，收到 ping 立刻
 * 重查 —— 新标签和画面同时出现，不等轮询；标题 / 地址这类没有事件的
 * 渐变靠 1s 轮询兜底（取舍见原 BrowserPanel 的 NAV_POLL_MS 注释）。
 *
 * `[约束]` 内容没变就不 setState。轮询每秒回一个新对象，不比对的话
 * App 每秒整树重渲染一次 —— 之前轮询在面板内部，殃及的只是面板子树，
 * 提上来之后代价范围变了，必须挡在门口。
 */
export function useBrowserPanel(sessionId: string | null, enabled: boolean) {
  const [panel, setPanel] = useState<PanelState>(EMPTY_PANEL);

  /** 覆盖整个状态。轮询、标签操作的回值、openBrowser 的首推都走这里。 */
  const apply = useCallback((s: PanelState) => {
    setPanel((prev) => (panelEq(prev, s) ? prev : s));
  }, []);

  /** 只换一页的信息（前进后退的回值，作用在激活页上）。 */
  const patchTab = useCallback((info: TabInfo) => {
    setPanel((prev) => {
      const idx = prev.tabs.findIndex((t) => t.id === info.id);
      const cur = prev.tabs[idx];
      if (cur === undefined || tabEq(cur, info)) return prev;
      const tabs = [...prev.tabs];
      tabs[idx] = info;
      return { ...prev, tabs };
    });
  }, []);

  useEffect(() => {
    // 没有会话或没有浏览器标签就清空 —— 残留的页面标签属于上一个
    // 上下文，摆在标签栏上会误导。
    if (!sessionId || !enabled) {
      setPanel(EMPTY_PANEL);
      return;
    }
    let alive = true;
    const poll = () => {
      // 失败保持上一次的状态（浏览器可能还没起来），下一拍再问。
      browserState(sessionId)
        .then((s) => {
          if (alive) apply(s);
        })
        .catch(() => {});
    };
    poll();
    const timer = window.setInterval(poll, 1000);
    // 清单变更的即时通道：开 / 关 / 切页的瞬间宿主 ping 过来，马上重查。
    const unwatch = watchBrowserTabs(sessionId, poll);
    return () => {
      alive = false;
      window.clearInterval(timer);
      unwatch();
    };
  }, [sessionId, enabled, apply]);

  return { panel, apply, patchTab };
}

function tabEq(a: TabInfo, b: TabInfo): boolean {
  return (
    a.id === b.id &&
    a.url === b.url &&
    a.title === b.title &&
    a.canBack === b.canBack &&
    a.canForward === b.canForward
  );
}

function panelEq(a: PanelState, b: PanelState): boolean {
  return (
    a.active === b.active &&
    a.tabs.length === b.tabs.length &&
    a.tabs.every((t, i) => {
      const o = b.tabs[i];
      return o !== undefined && tabEq(t, o);
    })
  );
}
