import { useCallback, useEffect, useState } from "react";

import { browserScopeList, browserScopeRevoke } from "../bridge";

/**
 * 渗透授权范围（scope）管理。
 *
 * 侵入性渗透工具（改包、重放、fuzzing、爬虫）只对这里列出的目标放行 ——
 * 每个都是用户在权限弹窗里点过"总是允许"授权进来的。这个面板让那份授权
 * **看得见、可撤销**:一次性把整个会话的攻击目标摆在眼前，比翻聊天记录找
 * "我到底授权了哪些站"强得多。
 *
 * `[取舍]` 轮询而不是订阅。scope 变化只发生在用户点弹窗那一下，频率极低，
 * 为它单开一条事件通道不值得;两秒一次的轻查询（就是读一个内存 Vec）足够。
 *
 * 空的时候整块不渲染 —— 没在做渗透的会话不该看到一块空面板占地方。
 */
export function ScopePanel({ sessionId }: { sessionId: string }) {
  const [hosts, setHosts] = useState<string[]>([]);

  const refresh = useCallback(() => {
    browserScopeList(sessionId)
      .then(setHosts)
      .catch(() => {
        // 会话刚建好、浏览器还没起来时查询可能失败，忽略即可 —— 下一拍再来。
      });
  }, [sessionId]);

  useEffect(() => {
    refresh();
    const t = setInterval(refresh, 2000);
    return () => clearInterval(t);
  }, [refresh]);

  const revoke = (host: string) => {
    // 乐观移除:先从界面拿掉，再落到宿主。失败了下一拍轮询会把它补回来。
    setHosts((hs) => hs.filter((h) => h !== host));
    browserScopeRevoke(sessionId, host).catch(refresh);
  };

  if (hosts.length === 0) return null;

  return (
    <div className="scope-panel">
      <div className="scope-head">
        <span className="scope-title">渗透授权范围</span>
        <span className="scope-count">{hosts.length}</span>
      </div>
      <ul className="scope-list">
        {hosts.map((h) => (
          <li key={h} className="scope-item">
            <span className="scope-host" title={h}>
              {h}
            </span>
            <button
              type="button"
              className="scope-revoke"
              title={`撤销对 ${h} 的授权`}
              onClick={() => revoke(h)}
            >
              撤销
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
