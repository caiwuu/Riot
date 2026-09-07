import { useEffect, useRef, useState } from "react";

import { readFileBytes } from "../bridge";
import { useT } from "../i18n";
import type { PlanView } from "../lib/plan";
import { joinRoot } from "../pathDisplay";
import { openFilePreview } from "./FilePreview";
import { PlanModeIcon } from "./icons";
import { Markdown } from "./Markdown";

/**
 * 右侧抽屉的「计划」标签（照 Cursor：计划在对话旁边的一页，不在对话里）。
 *
 * 三个状态一张面板：
 * - 撰写中：模型正在往 CreatePlan 的参数里流正文，这里边到边渲染
 *   （`streaming`）—— 计划要几十秒才写完，空着的面板和卡死没区别；
 * - 已就绪：读磁盘上的 `.plan.md`。文件才是计划：用户提了意见，模型是
 *   去 Edit 那个文件，面板要跟着刷（`refreshKey` 每有一次编辑落盘就变）；
 * - 读不到文件（被删了、目录换了）：退回工具输入里那份正文。
 *
 * 没有输入框、没有「构建」键：意见在下方的输入框里说，构建键也在那里 ——
 * 一件事一个入口。底部一行提示把人指过去。
 */
export function PlanPanel({
  root,
  plan,
  streaming,
  refreshKey,
  canBuild,
}: {
  /** 会话的项目根，计划文件的相对路径按它拼。 */
  root: string;
  plan: PlanView;
  /** 正在流的计划正文；不在流时为 null。 */
  streaming: string | null;
  /** 变一次就重读文件一次。 */
  refreshKey: string;
  /** 输入框那边此刻有没有「构建」键（还在规划模式、回合已结束）。 */
  canBuild: boolean;
}) {
  const { t } = useT();
  const running = plan.status === "running";
  const abs = plan.path ? joinRoot(root, plan.path) : null;
  /** 磁盘上的文件内容。null = 还没读到 / 读不到，用输入里的正文兜底。 */
  const [file, setFile] = useState<{ path: string; text: string } | null>(null);

  useEffect(() => {
    if (!abs || running) return;
    let stale = false;
    readFileBytes(abs)
      .then((buf) => {
        if (stale) return;
        setFile({ path: abs, text: new TextDecoder("utf-8", { fatal: false }).decode(buf) });
      })
      .catch(() => {
        if (!stale) setFile((f) => (f?.path === abs ? f : null));
      });
    return () => {
      stale = true;
    };
  }, [abs, running, refreshKey]);

  // 撰写中跟着尾巴滚，用户往上翻了就不再拽 —— 和对话流同一条规矩。
  const bodyRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  useEffect(() => {
    if (!running) return;
    const el = bodyRef.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [streaming, running]);
  // 换了一份计划从头看。
  useEffect(() => {
    stick.current = true;
    if (bodyRef.current) bodyRef.current.scrollTop = 0;
  }, [plan.id]);

  const fromFile = !running && file !== null && file.path === abs;
  const text = running
    ? (streaming ?? plan.body)
    : fromFile
      ? stripTitle(file.text, plan.name)
      : plan.body;
  // 概述只在正文里没有它的时候单独画：文件版开头就是概述（CreatePlan 写
  // 进去的），再画一遍就是同一段话叠两次；流式草稿和输入兜底里没有。
  const showOverview = Boolean(plan.overview) && !fromFile;
  const title = plan.name || t("transcript.plan.untitled");

  return (
    <div className="plan-panel">
      <div className="plan-panel-head">
        <span className={running ? "plan-panel-icon plan-panel-icon-live" : "plan-panel-icon"}>
          <PlanModeIcon />
        </span>
        <div className="plan-panel-head-main">
          <div className="plan-panel-title" title={title}>
            {title}
          </div>
          <div className="plan-panel-meta">
            {running ? (
              <span className="plan-panel-live">
                {t("transcript.plan.drafting")}
                <span className="plan-caret" aria-hidden />
              </span>
            ) : plan.status === "error" ? (
              <span className="tool-fail">{t("common.failed")}</span>
            ) : (
              <span>{t("transcript.plan.ready")}</span>
            )}
            {plan.path ? (
              <>
                <span>·</span>
                <button
                  type="button"
                  className="plan-panel-path"
                  title={t("transcript.plan.openFile")}
                  onClick={() => abs && openFilePreview(abs)}
                >
                  {plan.path}
                </button>
              </>
            ) : null}
          </div>
        </div>
      </div>
      <div
        ref={bodyRef}
        className="plan-panel-body"
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
      >
        {showOverview ? <p className="plan-panel-overview">{plan.overview}</p> : null}
        <div className="plan-panel-doc">
          {text.trim() ? <Markdown text={text} /> : running ? null : (
            <p className="plan-panel-empty">{t("transcript.plan.empty")}</p>
          )}
        </div>
      </div>
      {!running ? (
        <div className="plan-panel-foot">
          {canBuild ? t("transcript.plan.footBuild") : t("transcript.plan.footDone")}
        </div>
      ) : null}
    </div>
  );
}

/**
 * 文件开头那行 `# 标题` 去掉 —— 头部已经把标题画出来了，正文里再来一遍
 * 是两个大标题叠着。只认第一行、只认和面板标题一致的（用户自己改过
 * 标题就照文件的来）。
 */
function stripTitle(doc: string, name: string): string {
  const nl = doc.indexOf("\n");
  const first = (nl < 0 ? doc : doc.slice(0, nl)).trim();
  if (!first.startsWith("# ")) return doc;
  if (name && first.slice(2).trim() !== name.trim()) return doc;
  return nl < 0 ? "" : doc.slice(nl + 1).replace(/^\s*\n/, "");
}
