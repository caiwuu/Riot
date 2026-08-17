import { useCallback, useEffect, useRef, useState } from "react";

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
  /** 撤销被宿主拒绝。乐观移除会被轮询默默补回来 —— 用户以为权限
   *  已收回而实际仍生效，这在安全面板上必须说出声。 */
  const [revokeError, setRevokeError] = useState("");
  const errTimer = useRef<number | undefined>(undefined);
  /** 撤掉最后一项后先亮一拍"已全部撤销"再消失 —— 面板瞬间蒸发的话，
   *  用户来不及确认那一下点上了没有。 */
  const [farewell, setFarewell] = useState(false);
  const farewellTimer = useRef<number | undefined>(undefined);

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
    const next = hosts.filter((h) => h !== host);
    setHosts(next);
    if (next.length === 0) {
      setFarewell(true);
      window.clearTimeout(farewellTimer.current);
      farewellTimer.current = window.setTimeout(() => setFarewell(false), 1500);
    }
    browserScopeRevoke(sessionId, host).catch(() => {
      refresh();
      setRevokeError(`撤销 ${host} 失败，授权仍然有效`);
      window.clearTimeout(errTimer.current);
      errTimer.current = window.setTimeout(() => setRevokeError(""), 4000);
    });
  };

  if (hosts.length === 0) {
    if (!farewell) return null;
    return (
      <div className="scope-panel">
        <div className="scope-head">
          <span className="scope-title">已全部撤销</span>
        </div>
      </div>
    );
  }

  return (
    <div className="scope-panel">
      <div className="scope-head">
        <span className="scope-title">渗透授权范围</span>
        <span
          className="scope-info"
          title="这些站点是你在权限弹窗里点过「总是允许」的侵入性渗透目标。撤销之后，模型再对它做侵入操作会重新询问。"
        >
          ⓘ
        </span>
        <span className="scope-count">{hosts.length}</span>
      </div>
      {revokeError ? <div className="scope-error">{revokeError}</div> : null}
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
