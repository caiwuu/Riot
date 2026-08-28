/**
 * 设置导航的分区图标。
 *
 * 一套 16×16 的线描，`currentColor` 跟着选中态走。图标是给**扫视**用的：
 * 十个分区纯靠文字，切页时每次都得从头读一遍标签。所以形状要互相拉开
 * 距离 —— 别再多一个"矩形加一道线"。
 */

const S = {
  width: 16,
  height: 16,
  viewBox: "0 0 16 16",
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.3,
  strokeLinecap: "round",
  strokeLinejoin: "round",
} as const;

/** 服务方：两层机架。 */
export function ProviderIcon() {
  return (
    <svg {...S} aria-hidden>
      <rect x="2" y="2.6" width="12" height="4.6" rx="1.4" />
      <rect x="2" y="8.8" width="12" height="4.6" rx="1.4" />
      <path d="M4.4 4.9h.01M4.4 11.1h.01" />
    </svg>
  );
}

/** 联网：地球。 */
export function GlobeIcon() {
  return (
    <svg {...S} aria-hidden>
      <circle cx="8" cy="8" r="6" />
      <path d="M2.3 6.4h11.4M2.3 9.6h11.4" />
      <path d="M8 2a10 10 0 0 1 0 12A10 10 0 0 1 8 2z" />
    </svg>
  );
}

/** 提示词：书签夹着两行字。收起来的文本 —— 和「书页」的折角矩形拉得开。 */
export function BookmarkIcon() {
  return (
    <svg {...S} aria-hidden>
      <path d="M3.7 2.5h8.6v11.2L8 10.9l-4.3 2.8V2.5z" />
      <path d="M6 5.4h4M6 7.6h2.6" />
    </svg>
  );
}

/** 权限：盾牌。 */
export function ShieldIcon() {
  return (
    <svg {...S} aria-hidden>
      <path d="M8 1.9l5 1.9v4.1c0 3-2.1 5.4-5 6.2-2.9-.8-5-3.2-5-6.2V3.8l5-1.9z" />
      <path d="M5.9 8.1L7.4 9.6l2.9-3" />
    </svg>
  );
}

/** MCP：插头。 */
export function PlugIcon() {
  return (
    <svg {...S} aria-hidden>
      <path d="M6 1.9v3.3M10 1.9v3.3" />
      <path d="M3.9 5.2h8.2v2.5a4.1 4.1 0 0 1-8.2 0V5.2z" />
      <path d="M8 11.8v2.3" />
    </svg>
  );
}

/** 能力包：箱子。 */
export function PackageIcon() {
  return (
    <svg {...S} aria-hidden>
      <path d="M8 1.9l5.4 2.7v6.8L8 14.1l-5.4-2.7V4.6L8 1.9z" />
      <path d="M2.6 4.6L8 7.3l5.4-2.7M8 7.3v6.8" />
    </svg>
  );
}

/** Skills：书签书页。 */
export function SkillIcon() {
  return (
    <svg {...S} aria-hidden>
      <path d="M3.4 2.6h6.2l3 3v7.8a.6.6 0 0 1-.6.6H3.4a.6.6 0 0 1-.6-.6V3.2a.6.6 0 0 1 .6-.6z" />
      <path d="M9.4 2.6v3.2h3.2" />
      <path d="M5.4 9.4l1.3 1.3 2.6-2.6" />
    </svg>
  );
}

/** 命令：终端提示符。 */
export function TerminalIcon() {
  return (
    <svg {...S} aria-hidden>
      <rect x="1.9" y="2.9" width="12.2" height="10.2" rx="1.6" />
      <path d="M4.8 6.4l2 1.7-2 1.7M8.6 10.2h2.9" />
    </svg>
  );
}

/** Hooks：钩子。 */
export function HookIcon() {
  return (
    <svg {...S} aria-hidden>
      <path d="M10.4 2.2v5.2a3 3 0 0 1-6 0v-.9" />
      <circle cx="10.4" cy="12.1" r="1.9" />
      <path d="M10.4 10.2V8.6" />
    </svg>
  );
}

/** 关于：信息圈。 */
export function InfoIcon() {
  return (
    <svg {...S} aria-hidden>
      <circle cx="8" cy="8" r="6" />
      <path d="M8 7.3v3.6M8 5.2h.01" />
    </svg>
  );
}
