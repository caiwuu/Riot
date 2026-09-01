import { useEffect, useId, useState } from "react";
import { createPortal } from "react-dom";

import { useTimedFlag } from "../hooks/useTimedFlag";
import { useEscLayer } from "./Modal";

/**
 * 把 ```mermaid 围栏画成图。
 *
 * 模型已经会写 mermaid，以前当代码块原样甩出来，看起来像「只会出字」。
 * 画失败（流式还没写完、语法坏了）就退回源码 —— 有字比空白好；
 * 但之前成功过的话保留上一张图，流式期间图/源码来回切会让对话流上下跳。
 *
 * mermaid 很大，动态加载，没图的对话不付这份体积。
 * `securityLevel: "strict"`：图里的 HTML / 点击事件一律不执行。
 */

type MermaidApi = {
  initialize: (c: Record<string, unknown>) => void;
  render: (id: string, src: string) => Promise<{ svg: string }>;
};

let loaded: Promise<MermaidApi> | null = null;
let seq = 0;

function load(): Promise<MermaidApi> {
  loaded ??= import("mermaid").then((m) => {
    const api = m.default as MermaidApi;
    api.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      theme: "dark",
      themeVariables: {
        darkMode: true,
        background: "#121212",
        primaryColor: "#2a3d5c",
        primaryTextColor: "#ececf1",
        primaryBorderColor: "#5a8dd6",
        lineColor: "#a2a2ad",
        secondaryColor: "#212121",
        tertiaryColor: "#181818",
        fontFamily:
          "-apple-system, BlinkMacSystemFont, 'Segoe UI', 'PingFang SC', sans-serif",
      },
    });
    return api;
  });
  return loaded;
}

/** 读屏和放大按钮要报图的名字。从源码首个关键词猜，猜不出统一叫流程图。 */
const KIND_LABELS: [RegExp, string][] = [
  [/^(graph|flowchart)\b/, "流程图"],
  [/^sequenceDiagram/, "时序图"],
  [/^classDiagram/, "类图"],
  [/^stateDiagram/, "状态图"],
  [/^erDiagram/, "ER 图"],
  [/^gantt/, "甘特图"],
  [/^pie\b/, "饼图"],
];

function kindOf(src: string): string {
  const head = src.trimStart();
  for (const [re, label] of KIND_LABELS) {
    if (re.test(head)) return label;
  }
  return "流程图";
}

export function MermaidBlock({ source }: { source: string }) {
  const uid = useId().replace(/[^a-zA-Z0-9]/g, "");
  const [svg, setSvg] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  // 首个渲染结果落地前既不显示源码也不显示空白 —— mermaid 库首次加载
  // 要一两秒，这期间闪现源码再切成图，对话流会跳一下。
  const [settled, setSettled] = useState(false);
  const [copied, flashCopied] = useTimedFlag<"idle" | "ok" | "fail">("idle", 1500);
  const [viewer, setViewer] = useState(false);

  useEffect(() => {
    const src = source.trim();
    if (!src) {
      setSvg(null);
      setFailed(false);
      return;
    }
    let alive = true;
    // 流式时每个 token 都重跑。短延迟等一小截写完再画，别每个字符都渲染。
    const t = window.setTimeout(() => {
      const id = `mmd-${uid}-${++seq}`;
      void load()
        .then((api) => api.render(id, src))
        .then((out) => {
          if (!alive) return;
          setSvg(out.svg);
          setFailed(false);
          setSettled(true);
        })
        .catch(() => {
          // 只标失败、不清 svg —— 之前成功过就继续挂着上一张图
          if (!alive) return;
          setFailed(true);
          setSettled(true);
        });
    }, 180);
    return () => {
      alive = false;
      window.clearTimeout(t);
    };
  }, [source, uid]);

  const label = kindOf(source);

  const copySrc = () => {
    navigator.clipboard.writeText(source).then(
      () => flashCopied("ok"),
      () => flashCopied("fail"),
    );
  };

  if (svg) {
    return (
      <div className="md-mermaid-wrap">
        {/* 图被压在对话列宽里，大图的节点文字缩成蚂蚁 —— 点开全屏看 */}
        <button
          type="button"
          className="md-mermaid-zoom"
          onClick={() => setViewer(true)}
          aria-label={`放大查看${label}`}
        >
          <div
            className="md-mermaid"
            role="img"
            aria-label={label}
            dangerouslySetInnerHTML={{ __html: svg }}
          />
        </button>
        {/* 渲染成图之后源码就没处看了，hover 给条复制的路 */}
        <button type="button" className="md-mermaid-copy" onClick={copySrc}>
          {copied === "ok" ? "已复制" : copied === "fail" ? "复制失败" : "复制源码"}
        </button>
        {viewer ? (
          <MermaidViewer svg={svg} label={label} onClose={() => setViewer(false)} />
        ) : null}
      </div>
    );
  }

  if (!settled && source.trim()) {
    return (
      <div className="md-mermaid-loading" role="status">
        图渲染中…
      </div>
    );
  }

  return (
    <div className="codeblock">
      <div className="codeblock-bar">
        <span className="codeblock-lang">mermaid</span>
        {/* 退回源码时得说一声"这本来是张图"，不然像模型就只写了段代码 */}
        {failed ? <span className="codeblock-fail">图渲染失败</span> : null}
      </div>
      <pre>
        <code>{source}</code>
      </pre>
    </div>
  );
}

/**
 * 全屏图查看器。portal 到 body —— 卡片在带 overflow 的滚动容器里，
 * fixed 遮罩留在原地会被裁掉。遮罩样式复用 ShotViewer 的 .shot-viewer。
 */
function MermaidViewer({
  svg,
  label,
  onClose,
}: {
  svg: string;
  label: string;
  onClose: () => void;
}) {
  // Esc 走公共栈 —— 查看器开在权限卡之上时，Esc 只关查看器
  useEscLayer(onClose);

  return createPortal(
    <div
      className="shot-viewer"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <button className="shot-viewer-close" onClick={onClose} type="button" aria-label="关闭">
        <svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true">
          <path
            d="M2 2l8 8M10 2L2 10"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
          />
        </svg>
      </button>
      <div
        className="mermaid-viewer-body"
        role="img"
        aria-label={`${label}（放大）`}
        onClick={(e) => {
          if (e.target === e.currentTarget) onClose();
        }}
        dangerouslySetInnerHTML={{ __html: svg }}
      />
    </div>,
    document.body,
  );
}
