import { useEffect, useRef, useState } from "react";

import type { PermissionAsk, PermissionMode, PermissionResponse } from "../bridge";
import { Markdown } from "./Markdown";

interface Props {
  ask: PermissionAsk;
  /** 含这一个在内，还有几个请求排着队。 */
  pendingCount: number;
  onAnswer: (r: PermissionResponse) => void;
}

/**
 * 权限确认。
 *
 * `[约束]` 必须显示**将要执行的原文** —— 完整命令、完整路径、完整 diff。
 * 显示摘要等于让用户盲签：他点"允许"时以为批准的是摘要里那件事。
 *
 * 没有"关闭"按钮，Esc 等于拒绝。给一个语义模糊的退出口，用户会用它来
 * 跳过自己没看懂的东西，而没看懂正是最该拒绝的情况。
 */
export function PermissionDialog({ ask, pendingCount, onAnswer }: Props) {
  const denyRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    // 焦点落在"拒绝"上。一个习惯性的回车不应该批准一次删除。
    denyRef.current?.focus();

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onAnswer({ decision: "deny" });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onAnswer]);

  // 内核给出了"可以记住"的规则建议时才显示"总是允许"。没有建议
  // 却显示这个按钮，等于许诺一个不会兑现的行为。
  const rememberable = ask.suggestions.filter((s) => s.type === "add_rule");

  return (
    <div className="modal-backdrop">
      <div className="modal" role="dialog" aria-modal="true" aria-label="权限确认">
        <div className="modal-head">
          <span className="modal-tool">{ask.tool_name}</span>
          <span className="modal-title">{ask.summary}</span>
          {/* 并发工具会一次问好几个。不说还剩几个的话，用户答完一个
              又冒出一个，会以为是自己点错了或者程序在重复询问。 */}
          {pendingCount > 1 ? (
            <span className="modal-queue">还有 {pendingCount - 1} 个待确认</span>
          ) : null}
        </div>

        <Preview preview={ask.preview} />

        <div className="modal-actions">
          <button ref={denyRef} className="btn-deny" onClick={() => onAnswer({ decision: "deny" })}>
            拒绝
          </button>
          {rememberable.length > 0 ? (
            <button
              className="btn-allow-always"
              title={rememberable
                .map((s) => `${s.tool}${s.pattern ? `(${s.pattern})` : ""}`)
                .join("、")}
              onClick={() => onAnswer({ decision: "allow", remember: rememberable })}
            >
              总是允许
              <span className="allow-always-sub">
                {rememberable
                  .map((s) => `${s.tool}${s.pattern ? `(${s.pattern})` : ""}`)
                  .join("、")}
              </span>
            </button>
          ) : null}
          <button className="btn-allow" onClick={() => onAnswer({ decision: "allow" })}>
            允许一次
          </button>
        </div>
      </div>
    </div>
  );
}

/** 批准后切到哪个档，按钮上要写清楚 —— 这是批准动作的一部分，不是细节。 */
const APPROVE_LABEL: Partial<Record<PermissionMode, { label: string; sub: string }>> = {
  acceptEdits: { label: "批准，自动接受编辑", sub: "文件修改直接放行，命令仍询问" },
  default: { label: "批准，逐步确认", sub: "每个写操作都再问一次" },
};

/**
 * 计划还在往 tool_input 里写的时候用。外观跟批准卡同一套，
 * 没有按钮 —— 写完才轮到用户审。
 */
export function PlanDraft({ text }: { text: string }) {
  const bodyRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = bodyRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [text]);

  return (
    <div className="plan-card plan-draft" role="status" aria-label="正在撰写计划">
      <div className="plan-card-head">
        <span className="plan-card-badge">计划</span>
        <span className="plan-card-title">正在撰写…</span>
      </div>
      <div className="plan-body" ref={bodyRef}>
        {text ? <Markdown text={text} /> : null}
        <span className="plan-caret" aria-hidden />
      </div>
    </div>
  );
}

/**
 * 计划批准卡（对照 Claude Code 的 "Ready to code?"，但**长在对话流里**）。
 *
 * 内联而不是弹窗：计划在模型侦察几分钟之后才到，弹窗会突然糊在脸上；
 * 而它本来就是对话的一部分 —— 跟在 ExitPlanMode 的工具卡后面，随流
 * 滚动，答完就地消失、工具卡随之落定结果。
 *
 * 三个交互决策：计划按 Markdown 渲染（它是要读的文档）；批准按钮按
 * 执行档分两个（批准之后再逐个确认编辑，等于把刚做的决定再问一遍，
 * 所以"自动接受编辑"是主选项）；打回可以带反馈 —— 不带的话模型只
 * 知道"被拒了"，不知道往哪改。
 */
export function PlanApprovalCard({
  ask,
  onAnswer,
}: {
  ask: PermissionAsk;
  onAnswer: (r: PermissionResponse) => void;
}) {
  const [feedback, setFeedback] = useState("");
  const modes = ask.suggestions.flatMap((s) => (s.type === "set_mode" ? [s.mode] : []));
  const plan = ask.preview.kind === "plain" ? ask.preview.text : "";

  const approve = (mode: PermissionMode) => {
    const chosen = ask.suggestions.find((s) => s.type === "set_mode" && s.mode === mode);
    onAnswer({ decision: "allow", remember: chosen ? [chosen] : [] });
  };

  return (
    <div className="plan-card" role="region" aria-label="计划批准">
      <div className="plan-card-head">
        <span className="plan-card-badge">计划</span>
        <span className="plan-card-title">审阅后选择怎么执行</span>
      </div>

      <div className="plan-body">
        <Markdown text={plan || "（计划为空）"} />
      </div>

      <textarea
        className="plan-feedback"
        value={feedback}
        onChange={(e) => setFeedback(e.target.value)}
        placeholder="要打回的话，告诉它往哪改（可留空）"
        rows={2}
        spellCheck={false}
      />

      <div className="plan-card-actions">
        <button
          className="btn-deny"
          onClick={() =>
            onAnswer({
              decision: "deny",
              ...(feedback.trim() ? { message: feedback.trim() } : {}),
            })
          }
        >
          打回，继续规划
        </button>
        <span className="plan-card-spacer" />
        {modes.map((m, i) => {
          const label = APPROVE_LABEL[m] ?? { label: `批准（${m}）`, sub: "" };
          return (
            <button
              key={m}
              className={i === 0 ? "btn-allow" : "btn-allow-always"}
              onClick={() => approve(m)}
            >
              {label.label}
              {label.sub ? <span className="allow-always-sub">{label.sub}</span> : null}
            </button>
          );
        })}
      </div>
    </div>
  );
}

function Preview({ preview }: { preview: PermissionAsk["preview"] }) {
  switch (preview.kind) {
    case "command":
      return (
        <div className="preview">
          <div className="preview-label">将在 {preview.cwd} 执行</div>
          <pre className="preview-cmd">{preview.command}</pre>
        </div>
      );

    case "file_write":
      return (
        <div className="preview">
          <div className="preview-label">写入文件（{preview.bytes} 字节）</div>
          <pre className="preview-cmd">{preview.path}</pre>
        </div>
      );

    case "file_edit":
      return (
        <div className="preview">
          <div className="preview-label">修改 {preview.path}</div>
          <pre className="preview-diff">
            {preview.diff.split("\n").map((line, i) => (
              <div key={i} className={line.startsWith("+") ? "add" : line.startsWith("-") ? "del" : ""}>
                {line}
              </div>
            ))}
          </pre>
        </div>
      );

    case "network_fetch":
      return (
        <div className="preview">
          <div className="preview-label">访问网络</div>
          <pre className="preview-cmd">{preview.url}</pre>
        </div>
      );

    default:
      return (
        <div className="preview">
          <pre className="preview-cmd">{preview.text}</pre>
        </div>
      );
  }
}
