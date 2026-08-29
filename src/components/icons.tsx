/** 内联 SVG 图标。从 App.tsx 拆出 —— 纯展示，无状态。 */


/** 侧边栏开关：矩形 + 左侧一道竖线。 */
export function SidebarToggleIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden>
      <rect x="1.5" y="2.5" width="13" height="11" rx="2" stroke="currentColor" strokeWidth="1.3" />
      <path d="M6 2.5v11" stroke="currentColor" strokeWidth="1.3" />
    </svg>
  );
}

/**
 * 浏览器：Chrome 的轮廓（内置浏览器跑的就是 Chromium）。
 *
 * 圆形在一排矩形图标里本身就够显眼，不用靠颜色 —— 旁边几个都是矩形
 * 加一道线，只差线的方向，扫一眼分不出谁是谁。
 */
export function BrowserIcon() {
  return (
    <svg
      width="15"
      height="15"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      aria-hidden
    >
      <circle cx="12" cy="12" r="9.5" />
      <circle cx="12" cy="12" r="4" />
      <path d="M21.17 8H12M3.95 6.06 8.54 14M10.88 21.94 15.46 14" />
    </svg>
  );
}

/** 底部终端开关：矩形 + 底部一道横线。 */
export function PanelBottomIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden>
      <rect x="1.5" y="2.5" width="13" height="11" rx="2" stroke="currentColor" strokeWidth="1.3" />
      <path d="M1.5 9.5h13" stroke="currentColor" strokeWidth="1.3" />
    </svg>
  );
}

/** 右侧工作台开关：矩形 + 右侧一道竖线。 */
export function PanelRightIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden>
      <rect x="1.5" y="2.5" width="13" height="11" rx="2" stroke="currentColor" strokeWidth="1.3" />
      <path d="M10.5 2.5v11" stroke="currentColor" strokeWidth="1.3" />
    </svg>
  );
}

/** 改动一览：上面一个加号、下面一道减号 —— diff 的通用符号（octicon 同款）。 */
export function DiffIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M8 2.5v7M4.5 6h7"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
      />
      <path d="M4.5 13h7" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
    </svg>
  );
}

/** 文件预览开关：折角文档 + 两行正文。 */
export function FileDocIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M9.2 1.8H4.5a1 1 0 0 0-1 1v10.4a1 1 0 0 0 1 1h7a1 1 0 0 0 1-1V5.1L9.2 1.8z"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinejoin="round"
      />
      <path d="M9 2v3.3h3.4" stroke="currentColor" strokeWidth="1.3" strokeLinejoin="round" />
      <path
        d="M5.6 9h4.8M5.6 11.4h3.2"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
      />
    </svg>
  );
}

export function PlusIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path d="M8 3v10M3 8h10" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
    </svg>
  );
}

/**
 * 空会话上方的品牌标记：应用图标里那个 R，只留线条。
 *
 * 用的不是 icons/ 里那张位图 —— 那是给 Dock 和安装包用的立体图标，
 * 摆进这块纯色界面里像贴了一张贴纸。这里跟界面其余图标同一套语言：
 * 描边、圆角端点、颜色跟着 currentColor 走。
 *
 * 三笔各自成类，是为了让它动起来：这个 R 拆开看就是个小人 —— 上面
 * 的半圆是头、竖笔是身子、右下那折是腿，腿要能自己甩（见 .riot-mark）。
 */
export function RiotMark() {
  return (
    <svg className="riot-mark" width="42" height="42" viewBox="0 0 24 24" fill="none" aria-hidden>
      <g stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
        <path className="riot-mark-body" d="M7.4 4.2v15.6" />
        <path className="riot-mark-head" d="M7.4 4.2h5A4.2 4.2 0 0 1 12.4 12.6H7.4" />
        <path className="riot-mark-leg" d="M11.6 12.6l5 4.9-2.7 2.3" />
      </g>
    </svg>
  );
}

export function FolderIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M1.5 4a1 1 0 011-1h3l1.5 1.5h6a1 1 0 011 1V12a1 1 0 01-1 1h-11a1 1 0 01-1-1V4z"
        stroke="currentColor"
        strokeWidth="1.2"
      />
    </svg>
  );
}

export function GearIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden>
      <circle cx="12" cy="12" r="3" stroke="currentColor" strokeWidth="2" />
      <path
        d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.38a2 2 0 0 0-.73-2.73l-.15-.1a2 2 0 0 1-1-1.74v-.47a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinejoin="round"
      />
    </svg>
  );
}

export function ArrowUpIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M8 13V3M3.5 7.5L8 3l4.5 4.5"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

export function StopIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden>
      <rect x="1.5" y="1.5" width="9" height="9" rx="1.5" fill="currentColor" />
    </svg>
  );
}

export function PencilIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M11.3 2.2a1.6 1.6 0 0 1 2.3 2.3l-8 8-3.1.8.8-3.1 8-8z"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinejoin="round"
      />
    </svg>
  );
}

export function TrashIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M2.5 4.5h11M5.5 4.5V3a1 1 0 0 1 1-1h3a1 1 0 0 1 1 1v1.5M4 4.5l.7 8.6a1 1 0 0 0 1 .9h4.6a1 1 0 0 0 1-.9l.7-8.6"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

export function DotsIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" aria-hidden>
      <circle cx="3.5" cy="8" r="1.3" />
      <circle cx="8" cy="8" r="1.3" />
      <circle cx="12.5" cy="8" r="1.3" />
    </svg>
  );
}

export function EyeIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M1.8 8s2.4-4.5 6.2-4.5S14.2 8 14.2 8s-2.4 4.5-6.2 4.5S1.8 8 1.8 8z"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinejoin="round"
      />
      <circle cx="8" cy="8" r="1.9" stroke="currentColor" strokeWidth="1.3" />
    </svg>
  );
}

