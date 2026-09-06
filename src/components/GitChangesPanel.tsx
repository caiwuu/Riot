import { useEffect, useState } from "react";

import { type GitChanges, sessionGitChanges } from "../bridge";
import { useT } from "../i18n";
import { FieldSelect } from "./FieldSelect";
import { FileChangeList } from "./FileChangeList";

/**
 * 侧边抽屉的 Git 改动:工作区相对 HEAD 的未提交差异。
 *
 * 和输入框上方的会话改动条分工明确:那边回答"**这个会话**动了什么",
 * commit 之后还在;这边跟着 git 走 —— 用户手改、bash 写盘、重命名
 * 检测都在,commit 之后清零。review 完顺手一提交,这里就该空。
 *
 * 做成抽屉而不是弹窗：review 是拿着 diff 和对话对照着看的，弹窗把对话
 * 挡住，等于逼人记住一边再去看另一边。
 */
export function GitChangesPanel({
  sessionId,
  /** 变一次就重新拉一次。轮次结束时由外层递增 —— 抽屉是常驻的，
   *  不跟着刷新的话，模型改完文件这里还是上一轮的样子。 */
  refreshKey,
}: {
  sessionId: string;
  refreshKey: number;
}) {
  const { t, tn } = useT();
  const [git, setGit] = useState<GitChanges | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  // 手动刷新走同一个 effect —— 两份加载逻辑迟早分叉。
  const [manual, setManual] = useState(0);
  /** 用户选的对比基线。null = 还没选,跟当前分支走。切会话要清掉,
   *  不然上个仓库的分支名会拿去对新仓库 diff。 */
  const [base, setBase] = useState<string | null>(null);

  useEffect(() => {
    setBase(null);
  }, [sessionId]);

  useEffect(() => {
    // alive 防的是快速切会话：先发的请求后返回，会把新会话的改动
    // 覆盖成旧会话的。
    let alive = true;
    setLoading(true);
    sessionGitChanges(sessionId, base ?? undefined)
      .then((g) => {
        if (!alive) return;
        setGit(g);
        setError("");
        // 第一次回包把实际基线记下来,下拉才有选中项。
        setBase((cur) => cur ?? g.base ?? null);
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
  }, [sessionId, refreshKey, manual, base]);

  const changes = git?.repo ? git.changes : null;
  const total = changes?.reduce(
    (acc, c) => ({ added: acc.added + c.added, removed: acc.removed + c.removed }),
    { added: 0, removed: 0 },
  );
  /**
   * 已经问清楚了：这个目录不是 git 仓库。
   *
   * 这时整条头部行都不显示。分支下拉和统计本来就是空的，只剩一个
   * "重新比对" —— 而这里没有任何可比对的东西，按多少次都是同一句话。
   * 一个按不出结果的按钮配一道分割线，比干净的空面板更让人费解。
   * `git init` 之后不用手动点：轮次结束会刷新，切走再切回来也会重问。
   */
  const notRepo = Boolean(git && !git.repo);

  return (
    <div className="changes-panel">
      {notRepo ? null : (
        <div className="changes-head">
          {git?.repo && git.refs?.length ? (
            <FieldSelect
              className="changes-branch"
              title={t("panels.git.base.title")}
              menuMinWidth={240}
              value={base ?? git.base ?? git.refs[0] ?? ""}
              options={git.refs.map((r) =>
                r === git.branch
                  ? { value: r, label: r, hint: t("panels.git.currentBranch") }
                  : { value: r, label: r },
              )}
              onChange={setBase}
              disabled={loading}
            />
          ) : null}
          {changes?.length && total ? (
            <span className="changes-total">
              {tn("common.files", changes.length)} <span className="add">+{total.added}</span>{" "}
              <span className="del">−{total.removed}</span>
            </span>
          ) : null}
          <span className="changes-head-spacer" />
          <button
            className={loading ? "icon loading" : "icon"}
            onClick={() => setManual((n) => n + 1)}
            disabled={loading}
            title={t("panels.git.recompare")}
          >
            <RefreshIcon />
          </button>
        </div>
      )}

      <div className="changes-body">
        {error ? (
          <div className="msg error">
            {t("panels.git.failed", { error })}
            {/* 失败不清旧列表（旧的也比空白有用），但得说清下面是旧的 */}
            {git ? <div className="changes-stale">{t("panels.git.stale")}</div> : null}
          </div>
        ) : null}
        {!git && !error ? <div className="changes-empty">{t("panels.git.comparing")}</div> : null}
        {git && !git.repo ? (
          <div className="changes-empty">
            {t("panels.git.notRepo")}
            <div className="changes-hint">{t("panels.git.notRepo.hint")}</div>
          </div>
        ) : null}
        {changes?.length === 0 ? (
          <div className="changes-empty">
            {git?.base && git.base !== git.branch
              ? t("panels.git.noDiffAgainst", { base: git.base })
              : t("panels.git.clean")}
            <div className="changes-hint" title={t("panels.git.scope.title")}>
              {git?.base
                ? t("panels.git.baseLabel", { base: git.base })
                : t("panels.git.allUncommitted")}
            </div>
          </div>
        ) : null}
        {changes?.length ? <FileChangeList changes={changes} /> : null}
      </div>
    </div>
  );
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

