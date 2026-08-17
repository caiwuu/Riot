import { useEffect, useRef, useState } from "react";

import { type FileChange, sessionChanges } from "../bridge";
import { Chevron } from "./Chevron";

/**
 * 本次会话的改动一览。
 *
 * 回答的是"这个会话到底动了哪些行、有没有手滑多改" —— 逐条 Edit 卡片
 * 答不了：同一个文件被改五次、中间还有改了又撤的，看卡片等于让人在脑子
 * 里做一遍归并。这里给的是净效果。
 *
 * 和 `git diff` 的区别在于范围：那个是工作区相对 HEAD 的全部差异，混着
 * 用户自己没提交的改动；这里只有本会话经工具落下的那些。
 *
 * 做成抽屉而不是弹窗：review 是拿着 diff 和对话对照着看的，弹窗把对话
 * 挡住，等于逼人记住一边再去看另一边。
 */
const STATUS_LABEL: Record<FileChange["status"], string> = {
  created: "新增",
  modified: "修改",
  deleted: "删除",
};

export function ChangesPanel({
  sessionId,
  /** 变一次就重新拉一次。轮次结束时由外层递增 —— 抽屉是常驻的，
   *  不跟着刷新的话，模型改完文件这里还是上一轮的样子。 */
  refreshKey,
  onClose,
}: {
  sessionId: string;
  refreshKey: number;
  onClose: () => void;
}) {
  const [changes, setChanges] = useState<FileChange[] | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  // 默认折叠：文件一多，十五条 diff 叠在一起比摘要还难扫。要点哪份
  // 自己点开。新出现的文件也走这个默认 —— 集合里没有的就是收着的。
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  // 手动刷新走同一个 effect —— 两份加载逻辑迟早分叉。
  const [manual, setManual] = useState(0);
  /** 刚复制过路径的那一行。给"复制路径"按钮一个"已复制"的确认拍。 */
  const [copied, setCopied] = useState<string | null>(null);
  const copiedTimer = useRef<number | undefined>(undefined);

  useEffect(() => {
    // alive 防的是快速切会话：先发的请求后返回，会把新会话的改动
    // 覆盖成旧会话的。
    let alive = true;
    setLoading(true);
    sessionChanges(sessionId)
      .then((c) => {
        if (!alive) return;
        setChanges(c);
        setError("");
      })
      .catch((e: unknown) => {
        if (alive) setError(String(e));
      })
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [sessionId, refreshKey, manual]);

  const copyPath = (path: string) => {
    void navigator.clipboard.writeText(path).then(() => {
      setCopied(path);
      window.clearTimeout(copiedTimer.current);
      copiedTimer.current = window.setTimeout(() => setCopied(null), 1500);
    });
  };

  const toggle = (path: string) =>
    setExpanded((s) => {
      const next = new Set(s);
      if (!next.delete(path)) next.add(path);
      return next;
    });

  const total = changes?.reduce(
    (acc, c) => ({ added: acc.added + c.added, removed: acc.removed + c.removed }),
    { added: 0, removed: 0 },
  );

  return (
    <div className="changes-panel">
      <div className="changes-head">
        <span className="changes-title">本次改动</span>
        {changes?.length && total ? (
          <span className="changes-total">
            {changes.length} 个文件 <span className="add">+{total.added}</span>{" "}
            <span className="del">−{total.removed}</span>
          </span>
        ) : null}
        <span className="changes-head-spacer" />
        <button
          className={loading ? "icon loading" : "icon"}
          onClick={() => setManual((n) => n + 1)}
          disabled={loading}
          title="重新比对"
        >
          <RefreshIcon />
        </button>
        <button className="icon" onClick={onClose} title="收起面板">
          <PanelIcon />
        </button>
      </div>

      <div className="changes-body">
        {error ? (
          <div className="msg error">
            比对失败：{error}
            {/* 失败不清旧列表（旧的也比空白有用），但得说清下面是旧的 */}
            {changes ? <div className="changes-stale">下方显示的是上次的结果。</div> : null}
          </div>
        ) : null}
        {!changes && !error ? <div className="changes-empty">正在比对…</div> : null}
        {changes?.length === 0 ? (
          <div className="changes-empty">
            这个会话还没有改过文件。
            <div
              className="changes-hint"
              title="只统计模型经编辑工具落盘的写入；用 Bash 重定向写的文件绕过了工具层，这里看不到。"
            >
              只统计模型用编辑工具改的文件
            </div>
          </div>
        ) : null}

        {changes?.map((c) => {
          const open = expanded.has(c.path);
          return (
            <div className="change" key={c.path}>
              <button
                className="change-head"
                onClick={() => toggle(c.path)}
                type="button"
                aria-expanded={open}
              >
                <Chevron open={open} />
                <span className={`change-status ${c.status}`}>{STATUS_LABEL[c.status]}</span>
                <span className="change-path" title={c.path}>
                  {midTruncate(c.path)}
                </span>
                {/* span 而不是嵌套 button（button 套 button 是非法 HTML）。
                    打开文件涉及 bridge 权限暂不做，复制路径是够到文件的最短路。 */}
                <span
                  className={copied === c.path ? "change-copy done" : "change-copy"}
                  role="button"
                  tabIndex={0}
                  title="复制完整路径"
                  onClick={(e) => {
                    e.stopPropagation();
                    copyPath(c.path);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      e.stopPropagation();
                      copyPath(c.path);
                    }
                  }}
                >
                  {copied === c.path ? "已复制" : "复制路径"}
                </span>
                <span className="change-stat">
                  <span className="add">+{c.added}</span>{" "}
                  <span className="del">−{c.removed}</span>
                </span>
              </button>

              {open ? (
                <div className="change-diff">
                  {c.hunks.map((h, i) => (
                    <div className="hunk" key={i}>
                      <div className="hunk-head">{h.header}</div>
                      {h.lines.map((l, j) => (
                        <div className={`hunk-line ${l.kind}`} key={j}>
                          <span className="hunk-sign" aria-hidden>
                            {l.kind === "add" ? "+" : l.kind === "del" ? "−" : " "}
                          </span>
                          {l.text || "\u00a0"}
                        </div>
                      ))}
                    </div>
                  ))}
                  {c.truncated ? (
                    <div className="hunk-more">
                      改动太大，只显示了前面一截。完整内容请看文件本身。
                    </div>
                  ) : null}
                </div>
              ) : null}
            </div>
          );
        })}
      </div>
    </div>
  );
}

/**
 * 长路径中间截断：头留一截认目录，尾优先保文件名 —— 尾部截断恰好吃掉
 * 的就是最有信息量的那段。不用 CSS 的 direction:rtl 截断（styles.css
 * 里警告过：它会把开头的标点甩到另一头）。title 仍给全路径。
 */
function midTruncate(path: string): string {
  const MAX = 48;
  if (path.length <= MAX) return path;
  const name = path.slice(path.lastIndexOf("/") + 1);
  // 尾段至少装下文件名；文件名本身超长就退化成固定尾长，保住扩展名
  const tail = Math.min(Math.max(name.length, 24), MAX - 8);
  return `${path.slice(0, MAX - tail - 1)}…${path.slice(-tail)}`;
}

function RefreshIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M13.5 8a5.5 5.5 0 1 1-1.6-3.9"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
      />
      <path
        d="M13.5 2v3h-3"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/** 关闭面板。画的是"右边那一栏收起来"，和浏览器面板同一个手势。 */
function PanelIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <rect x="1.5" y="2.5" width="13" height="11" rx="2" stroke="currentColor" strokeWidth="1.3" />
      <path d="M10.5 2.5v11" stroke="currentColor" strokeWidth="1.3" />
    </svg>
  );
}
