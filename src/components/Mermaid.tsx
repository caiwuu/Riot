import { useEffect, useId, useState } from "react";
import { createPortal } from "react-dom";

import { useTimedFlag } from "../hooks/useTimedFlag";
import { type MessageKey, useT } from "../i18n";
import { type Theme, useTheme } from "../theme";
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
 *
 * 配色随界面主题走。SVG 是静态的，颜色在渲染时就写死在里面，切主题不会
 * 自己变 —— 所以主题进了渲染 effect 的依赖：切换后每张已经画出来的图按新
 * 配色重画一遍（旧图挂着直到新图落地，不闪源码）。
 */

type MermaidApi = {
  initialize: (c: Record<string, unknown>) => void;
  render: (id: string, src: string) => Promise<{ svg: string }>;
};

const FONT_FAMILY = "-apple-system, BlinkMacSystemFont, 'Segoe UI', 'PingFang SC', sans-serif";

/**
 * 两套配色，值抄 styles.css 里对应主题的 token（mermaid 读不了 CSS 变量）：
 * background 是图所在的容器底（--bg-side），primary 是节点底 / 边框
 * （--accent-dim / --accent），文字和连线是 --text / --text-dim，
 * secondary / tertiary 是子图、备注这类次级面（--bg-card / --bg）。
 * 深色用 mermaid 的 dark 主题打底。
 *
 * 浅色用 base 而不是 default：default 的流程图节点底 / 描边是写死的
 * #ECECFF / #9370DB（mainBkg / border1，不从 primaryColor 推），传进去的
 * --accent-dim / --accent 根本落不到节点上，画出来是一片和灰阶界面对不上的
 * 淡紫。base 是 mermaid 专门留给自定义的那套：nodeBkg = primaryColor、
 * nodeBorder = primaryBorderColor、clusterBkg = tertiaryColor、边标签底
 * = secondaryColor，这里给的每个值都会用上。neutral 是纯灰阶，和界面上
 * 蓝色的强调色对不上，所以也不用。
 */
const THEME_CONFIG: Record<Theme, Record<string, unknown>> = {
  dark: {
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
      fontFamily: FONT_FAMILY,
    },
  },
  light: {
    theme: "base",
    themeVariables: {
      darkMode: false,
      background: "#f6f6f6",
      primaryColor: "#dfe9f7",
      primaryTextColor: "#1a1a1a",
      primaryBorderColor: "#3b6fc4",
      lineColor: "#5f5f68",
      secondaryColor: "#f4f4f4",
      tertiaryColor: "#ffffff",
      fontFamily: FONT_FAMILY,
    },
  },
};

let loaded: Promise<MermaidApi> | null = null;
/** 上次 `initialize` 用的主题。切主题后第一张要画的图负责重新 initialize。 */
let configured: Theme | null = null;
let seq = 0;

function load(): Promise<MermaidApi> {
  loaded ??= import("mermaid").then((m) => m.default as MermaidApi);
  return loaded;
}

/**
 * 让 mermaid 的全局配置对上要画的主题。`initialize` 是整体替换，所以每次都
 * 给全套（安全等级也在里面），而不是只补主题那几项。
 */
function configure(api: MermaidApi, theme: Theme) {
  if (configured === theme) return;
  configured = theme;
  api.initialize({ startOnLoad: false, securityLevel: "strict", ...THEME_CONFIG[theme] });
}

/** 读屏和放大按钮要报图的名字。从源码首个关键词猜，猜不出统一叫流程图。 */
const KIND_LABELS: [RegExp, MessageKey][] = [
  [/^(graph|flowchart)\b/, "transcript.mermaid.flowchart"],
  [/^sequenceDiagram/, "transcript.mermaid.sequence"],
  [/^classDiagram/, "transcript.mermaid.class"],
  [/^stateDiagram/, "transcript.mermaid.state"],
  [/^erDiagram/, "transcript.mermaid.er"],
  [/^gantt/, "transcript.mermaid.gantt"],
  [/^pie\b/, "transcript.mermaid.pie"],
];

function kindOf(src: string): MessageKey {
  const head = src.trimStart();
  for (const [re, key] of KIND_LABELS) {
    if (re.test(head)) return key;
  }
  return "transcript.mermaid.flowchart";
}

export function MermaidBlock({ source }: { source: string }) {
  const { t } = useT();
  const theme = useTheme();
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
    // 切主题也走这条路：整屏的图一起排队，180ms 后各画各的。
    const timer = window.setTimeout(() => {
      const id = `mmd-${uid}-${++seq}`;
      void load()
        .then((api) => {
          configure(api, theme);
          return api.render(id, src);
        })
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
      window.clearTimeout(timer);
    };
  }, [source, uid, theme]);

  const label = t(kindOf(source));

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
          aria-label={t("transcript.image.zoomNamed", { name: label })}
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
          {copied === "ok"
            ? t("common.copied")
            : copied === "fail"
              ? t("transcript.md.copyFailed")
              : t("transcript.mermaid.copySource")}
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
        {t("transcript.mermaid.rendering")}
      </div>
    );
  }

  return (
    <div className="codeblock">
      <div className="codeblock-bar">
        <span className="codeblock-lang">mermaid</span>
        {/* 退回源码时得说一声"这本来是张图"，不然像模型就只写了段代码 */}
        {failed ? <span className="codeblock-fail">{t("transcript.mermaid.failed")}</span> : null}
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
  const { t } = useT();
  // Esc 走公共栈 —— 查看器开在权限卡之上时，Esc 只关查看器
  useEscLayer(onClose);

  return createPortal(
    <div
      className="shot-viewer"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <button
        className="shot-viewer-close"
        onClick={onClose}
        type="button"
        aria-label={t("common.close")}
      >
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
        aria-label={t("transcript.image.zoomed", { name: label })}
        onClick={(e) => {
          if (e.target === e.currentTarget) onClose();
        }}
        dangerouslySetInnerHTML={{ __html: svg }}
      />
    </div>,
    document.body,
  );
}
