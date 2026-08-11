import { useEffect, useRef } from "react";

import type { PermissionAsk, PermissionResponse } from "../bridge";

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
