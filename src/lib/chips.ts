//! 输入框和气泡里那些"色块"的唯一真相：块叫什么、长什么样、在 DOM 上怎么存。
//!
//! 块有两条渲染路径，而且**消不掉** —— 输入框是 contenteditable，节点归
//! 浏览器和光标管，React 一次 re-render 就把插入点冲掉（中文输入法当场
//! 不能用），那边只能命令式建 DOM；气泡和补全菜单那边又必须是 React。
//! 能收拢的是两条路径共用的**知识**，全在这里，各自只剩一层薄渲染。
//!
//! 加一种新块只动两个文件：`Seg` 添一个成员（`ChipKind` 自动跟上），本文件
//! 里的几个 switch 和 `CHIP_KINDS` 各补一支，styles.css 里加一条 `.chip-xxx`
//! 配色。
//!
//! `[约束]` 这里每一处按种类分叉的地方都**编译期强制穷尽**：漏一支 tsc 直接
//! 报错，而不是运行时静默丢掉那种块。别加 `default:` 分支把这层保护关掉。
//! Composer 的编辑器机械（光标、退格、方向键）和 Transcript 的渲染都不用动
//! —— 它们只认 `data-chip`，不认具体是哪一种。

import { basename, isDirRef } from "../pathDisplay";
import type { Seg } from "./promptText";

/** 块的种类。`text` 之外的每种 `Seg` 都是一种块。 */
export type ChipKind = Exclude<Seg["kind"], "text">;
export type ChipSeg = Extract<Seg, { kind: ChipKind }>;

/** 认得的种类，运行时那一份。`Record<ChipKind, true>` 少一个成员编译不过 ——
 *  回读 DOM 要按名字查，而这张表漏一项的表现是那种块被**静默丢掉**。 */
const CHIP_KINDS: Record<ChipKind, true> = { ref: true, cmd: true, elem: true };

function isChipKind(s: string | undefined): s is ChipKind {
  return s != null && s in CHIP_KINDS;
}

/**
 * 这一段是块吗（相对于纯文字）。
 *
 * `[约束]` 所有"要不要保留 / 算不算内容"的判断都走它，别在调用点写
 * `s.kind === "ref" || s.kind === "cmd"`。枚举式写法每加一种块就漏一处，
 * 而漏掉的表现是用户的内容**静默消失** —— 元素块就这么被吞过：取件之后
 * 敲一条斜杠命令，绿块无声无息地没了。
 */
export function isChipSeg(seg: Seg): seg is ChipSeg {
  return seg.kind !== "text";
}

/** 块上的文字超过这个长度就截断。元素描述可以很长，撑变形的是输入框。 */
const MAX_LABEL = 40;

function truncate(s: string): string {
  return s.length > MAX_LABEL ? `${s.slice(0, MAX_LABEL - 1)}…` : s;
}

/** 块上显示的短文本。走 CSS 的 `attr(data-label)`，不进 DOM 文本节点。 */
export function chipLabel(seg: ChipSeg): string {
  switch (seg.kind) {
    case "ref":
      return basename(seg.value) || seg.value;
    case "cmd":
      return `/${seg.value}`;
    case "elem":
      return truncate(seg.label.trim() || seg.value);
  }
}

/** 悬停给的全量信息 —— 块上那行是截断过的，点中的到底是哪个得能看清。 */
export function chipTitle(seg: ChipSeg): string {
  switch (seg.kind) {
    case "ref":
      return seg.value;
    case "cmd":
      return `/${seg.value}`;
    case "elem":
      return `${seg.label}\n${seg.value}`;
  }
}

/** 块的 class。`static` 是气泡/菜单里的只读版（可以选中文字）。 */
export function chipClass(kind: ChipKind, extra = ""): string {
  return extra ? `chip chip-${kind} ${extra}` : `chip chip-${kind}`;
}

/**
 * 块在 DOM 上的全部属性。三种块的节点结构**完全一致**，只差这几个属性 ——
 * 图标和配色都在 CSS 里（见 styles.css 的 `.chip`）。
 *
 * 块里不放任何子节点，文本和 svg 都不放：WebKit 会把插入点塞进
 * `contenteditable=false` 元素内部，退格于是先"走进去"再删。空节点 + 伪
 * 元素是让光标只能停在块**两侧**的办法，也就是让块表现得像一个字符。
 */
export interface ChipAttrs {
  "data-chip": ChipKind;
  "data-value": string;
  "data-label": string;
  "data-extra"?: string;
  title: string;
}

/**
 * `value` 之外还要存的那一份，没有就是 null。
 *
 * 引用块：目录路径以 `/` 结尾（见 `isDirRef`），这里落成 `dir` 给 CSS
 * 换文件夹图标。回读不靠这份 —— 路径本身已经带着约定。
 *
 * 元素块存完整描述：`data-label` 是**截断过**的显示文本，拿它回读会把描述
 * 永久截短 —— 发给模型的 `【…】` 标记也就跟着短了。
 *
 * 返回 `| null` 而不是 `| undefined` 是为了穷尽检查：漏一支时函数尾部隐式
 * 返回的正是 undefined，写进返回类型这层保护就没了。
 */
function chipExtra(seg: ChipSeg): string | null {
  switch (seg.kind) {
    case "ref":
      // 目录块用 `data-extra="dir"` 换图标（见 styles.css 的 `.chip-ref`）。
      // 认不认目录看路径是不是以 `/` 结尾，约定见 `isDirRef`。
      return isDirRef(seg.value) ? "dir" : null;
    case "cmd":
      return null;
    case "elem":
      return seg.label;
  }
}

export function chipAttrs(seg: ChipSeg): ChipAttrs {
  const extra = chipExtra(seg);
  return {
    "data-chip": seg.kind,
    "data-value": seg.value,
    "data-label": chipLabel(seg),
    ...(extra === null ? {} : { "data-extra": extra }),
    title: chipTitle(seg),
  };
}

/** DOM 节点 → 段落。和 [`chipAttrs`] 是一对，改一个必须改另一个。 */
export function chipSegFromEl(el: HTMLElement): ChipSeg | null {
  const kind = el.dataset["chip"];
  const value = el.dataset["value"];
  if (!value || !isChipKind(kind)) return null;
  switch (kind) {
    case "ref":
    case "cmd":
      return { kind, value };
    case "elem":
      return { kind, value, label: el.dataset["extra"] ?? value };
  }
}
