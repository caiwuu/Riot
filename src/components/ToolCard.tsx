import { memo, useState } from "react";

import type { Item } from "../hooks/useSession";

type Tool = Extract<Item, { kind: "tool" }>;

/**
 * 一次工具调用。
 *
 * 默认折叠。展开的话，一次 `cargo build` 的输出就会把整个对话冲走 ——
 * 用户来这里是看模型在干什么，不是读日志。摘要行说清"做了什么、成没成"，
 * 想看细节再点开。
 *
 * memo：流式输出时 transcript 每帧重渲染，历史工具卡片不该跟着刷。
 */
export const ToolCard = memo(function ToolCard({ tool }: { tool: Tool }) {
  const [open, setOpen] = useState(false);
  const detail = renderDetail(tool);

  return (
    <div className={`tool tool-${tool.status}`}>
      <button className="tool-head" onClick={() => setOpen(!open)} type="button">
        <span className="tool-icon">{icon(tool.status)}</span>
        <span className="tool-name">{tool.name}</span>
        <span className="tool-summary">{summarize(tool)}</span>
        {detail ? <span className="tool-chevron">{open ? "收起" : "展开"}</span> : null}
      </button>

      {open && detail ? <div className="tool-detail">{detail}</div> : null}
    </div>
  );
});

function icon(s: Tool["status"]): string {
  if (s === "running") return "◐";
  if (s === "ok") return "✓";
  return "✕";
}

/** 一行说清这次调用在做什么。参数原样 dump 没人看得下去。 */
function summarize(t: Tool): string {
  const i = t.input as Record<string, unknown>;
  const str = (k: string) => (typeof i?.[k] === "string" ? (i[k] as string) : "");

  switch (t.name) {
    case "Bash":
      return str("command");
    case "Read":
    case "Write":
    case "Edit":
      return short(str("path") || str("file_path"));
    case "Grep":
      return `${str("pattern")}${str("path") ? ` 在 ${short(str("path"))}` : ""}`;
    default:
      return Object.entries(i ?? {})
        .map(([k, v]) => `${k}=${typeof v === "string" ? v : JSON.stringify(v)}`)
        .join(" ")
        .slice(0, 120);
  }
}

/**
 * 展开后的内容。按工具语义渲染，不是 JSON dump：
 * Edit 给 diff，Write 给内容预览，Bash 给实时输出或结果。
 */
function renderDetail(t: Tool): React.ReactNode {
  const i = t.input as Record<string, unknown>;
  const str = (k: string) => (typeof i?.[k] === "string" ? (i[k] as string) : "");

  const parts: React.ReactNode[] = [];

  if (t.name === "Edit") {
    const oldS = str("old_string");
    const newS = str("new_string");
    if (oldS || newS) {
      parts.push(
        <pre key="diff" className="tool-body tool-diff">
          {oldS.split("\n").map((l, n) => (
            <div key={`o${n}`} className="del">
              - {l}
            </div>
          ))}
          {newS.split("\n").map((l, n) => (
            <div key={`n${n}`} className="add">
              + {l}
            </div>
          ))}
        </pre>,
      );
    }
  } else if (t.name === "Write") {
    const content = str("content");
    if (content) {
      // 预览开头就够了 —— 用户要确认的是"写了个什么东西"，不是逐行审阅
      const lines = content.split("\n");
      const preview = lines.slice(0, 30).join("\n");
      parts.push(
        <pre key="w" className="tool-body">
          {preview}
          {lines.length > 30 ? `\n… 共 ${lines.length} 行` : ""}
        </pre>,
      );
    }
  }

  // 运行中显示实时输出；结束后最终结果更权威（实时行只是进度侧影）
  const live = t.output.length > 0 ? t.output.join("\n") : "";
  const result = t.status === "running" ? live : t.result || live;
  if (result) {
    parts.push(
      <pre key="r" className="tool-body">
        {result}
      </pre>,
    );
  }

  return parts.length ? parts : null;
}

/** 长路径留尾部 —— 文件名比目录前缀有用得多。 */
function short(p: string, max = 48): string {
  return p.length <= max ? p : `…${p.slice(-(max - 1))}`;
}
