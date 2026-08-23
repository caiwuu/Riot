import { useEffect, useState } from "react";

import { type FileChange, sessionChanges } from "../bridge";
import { Chevron } from "./Chevron";
import { FileChangeList } from "./FileChangeList";

/**
 * 输入框上方的"本次会话改动"条(Cursor 同款位置)。
 *
 * 回答的是"这个会话到底动了哪些行、有没有手滑多改" —— 基线是会话
 * 自己记的,不跟 git 走:commit 了它还在,直到会话结束都能回看。
 * 工作区维度的未提交改动在右侧抽屉的 Git 面板。
 *
 * 收起时只占一行(N 个文件 +x −y);点开在原地向上展开列表。没有
 * 改动时整条不出现 —— 一条常驻的"0 个文件"只是噪音。
 */
export function SessionChangesBar({
  sessionId,
  /** 变一次就重新拉一次。外层在每次编辑工具落盘时递增 ——
   *  跑轮中的改动要实时长出来,不能等到轮子结束。 */
  refreshKey,
  paused = false,
}: {
  sessionId: string;
  refreshKey: number;
  /** 保活但不可见时别轮询。切回来 refreshKey 会再推一次。 */
  paused?: boolean;
}) {
  const [changes, setChanges] = useState<FileChange[] | null>(null);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (paused) return;
    // alive 防的是快速切会话:先发的请求后返回,会把新会话的改动
    // 覆盖成旧会话的。
    let alive = true;
    sessionChanges(sessionId)
      .then((c) => {
        if (alive) setChanges(c);
      })
      .catch(() => {
        // 拉不到就保持上一次的列表。这是常驻小条,不值得为它弹错误 ——
        // 需要诊断时抽屉的 Git 面板会把错误显出来。
      });
    return () => {
      alive = false;
    };
  }, [sessionId, refreshKey, paused]);

  if (!changes?.length) return null;

  const total = changes.reduce(
    (acc, c) => ({ added: acc.added + c.added, removed: acc.removed + c.removed }),
    { added: 0, removed: 0 },
  );

  return (
    <div className="changes-bar">
      {open ? (
        <div className="changes-bar-list">
          <FileChangeList changes={changes} />
        </div>
      ) : null}
      <div className="changes-bar-row">
        <button
          type="button"
          className="changes-bar-head"
          onClick={() => setOpen(!open)}
          aria-expanded={open}
          title="本次会话经编辑工具落盘的净改动。提交到 git 之后这里依然保留,直到会话结束。"
        >
          <Chevron open={open} />
          <span className="changes-bar-title">本次改动 {changes.length} 个文件</span>
          <span className="changes-bar-stat">
            <span className="add">+{total.added}</span> <span className="del">−{total.removed}</span>
          </span>
        </button>
      </div>
    </div>
  );
}
