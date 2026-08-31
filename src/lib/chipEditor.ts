//! contenteditable 块编辑器的机械：块节点、光标、守卫字符、键盘行为。
//!
//! 从 Composer 拆出，输入框（Composer）和消息编辑框（Transcript 的
//! MsgEditor）共用 —— 改光标/退格/守卫的行为只动这个文件，两处一起生效。
//! 块的"知识"（种类、属性、样式类）在 `chips.ts`；React 静态渲染在
//! `components/Chip.tsx`；这里只有命令式 DOM —— contenteditable 里
//! React 一 re-render 就冲掉光标，进不来。
//!
//! 行为总纲：**块的光标行为和一个字符完全一致**。落点恰好在块两侧、
//! 方向键一步跨块、退格一次删整块、真实空格是普通字符。

import { type ChipSeg, chipAttrs, chipClass, chipSegFromEl } from "./chips";
import type { Seg } from "./promptText";

/**
 * 造一个块。`contenteditable=false` 让它在编辑器里是一个整体。
 *
 * 三种块的节点一模一样，只差 `chipAttrs` 那几个属性 —— 图标和配色都在
 * CSS 里，块里**不放任何子节点**：WebKit 否则会把光标塞进块内部，退格
 * 先"走进去"再删。
 */
function chipEl(seg: ChipSeg): HTMLElement {
  const span = document.createElement("span");
  span.className = chipClass(seg.kind);
  span.contentEditable = "false";
  for (const [k, v] of Object.entries(chipAttrs(seg))) {
    if (v !== undefined) span.setAttribute(k, v);
  }
  return bindChip(span);
}

/** 单一标记，不枚举种类 —— 加一种块时光标行为自动跟上（漏认的块会退回
 *  WebKit 默认：光标钻进块里、退格先走进去）。 */
function isChip(node: Node | null): node is HTMLElement {
  return node instanceof HTMLElement && node.dataset["chip"] != null;
}

/**
 * 块两侧的光标停靠字符（零宽不换行空格）。
 *
 * 块左边没有文字时（块在句首、两块相邻），光标只能落进一个**空**文本
 * 节点 —— 空节点没有渲染盒，光标画不出来，看上去就是"过不去"。垫一个
 * 零宽字符给光标一个真实的落点，Slate 给行内不可编辑节点做的是同一件事。
 *
 * 选 U+FEFF 而不是 U+200B：JS 的 `trim()` 和 `\s` 把 FEFF 当空白，
 * `adjacentChip` 的空白判断、`@` 补全的边界正则全都自动兼容。它永远
 * 不进数据 —— `readEditor` 输出前剥掉。
 */
const PAD = "\uFEFF";
const PAD_RE = /\uFEFF/g;

function setCaret(node: Node, offset: number) {
  const sel = window.getSelection();
  if (!sel) return;
  const r = document.createRange();
  r.setStart(node, offset);
  r.collapse(true);
  sel.removeAllRanges();
  sel.addRange(r);
}

/** 编辑区内折叠状态的光标位置。选区在别处 / 非折叠时是 null。 */
function caretIn(root: HTMLElement): { node: Node; offset: number } | null {
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0 || !sel.isCollapsed) return null;
  const r = sel.getRangeAt(0);
  if (!root.contains(r.startContainer)) return null;
  return { node: r.startContainer, offset: r.startOffset };
}

/**
 * 光标停在 root 边界（node 是编辑区本身、offset 是子节点序号）时安置进
 * 旁边的文本节点，返回安置后的位置。
 *
 * root 边界光标是各种怪行为的温床：`caretToEnd` 和原生删除都会留下它，
 * 而 WebKit 对紧挨 `contenteditable=false` 元素的边界位置做归一化时常跳
 * 到块的**另一侧** —— 表现是删掉块右边的空格后光标瞬移到块左边。文本
 * 节点内的位置没有这个歧义。
 */
function settleCaret(root: HTMLElement): { node: Node; offset: number } | null {
  const cur = caretIn(root);
  if (!cur || cur.node !== root) return cur;
  const prev = root.childNodes[cur.offset - 1];
  const next = root.childNodes[cur.offset];
  if (prev?.nodeType === Node.TEXT_NODE) {
    setCaret(prev, (prev.nodeValue ?? "").length);
  } else if (next?.nodeType === Node.TEXT_NODE) {
    setCaret(next, 0);
  }
  return caretIn(root);
}

/**
 * 维护停靠字符的不变量，光标跟着一起校正：
 *
 * 1. 相邻文本节点已合并、空文本节点已删（`root.normalize()`）；
 * 2. 块的某一侧没有文本节点时，垫一个只含 PAD 的守卫节点；
 * 3. 有真实字符的文本节点里不留 PAD（真实字符本身就是落点，混着守卫
 *    只会多出一个"按一下没动静"的幽灵光标位）。
 *
 * 每次 sync 都跑：退格、剪切、原生删除都可能把守卫吃掉或留下孤儿，
 * 逐个调用点去补漏就是当初 elem 块被吞的老路。IME 组字中**不要**调 ——
 * normalize 合并文本节点会打断组字。
 */
export function normalizePads(root: HTMLElement) {
  root.normalize();
  let cur = caretIn(root);

  for (const node of Array.from(root.childNodes)) {
    if (node.nodeType !== Node.TEXT_NODE) continue;
    const v = node.nodeValue ?? "";
    if (!v.includes(PAD)) continue;
    const stripped = v.replace(PAD_RE, "");
    if (stripped) {
      // 混进真实文字的守卫剥掉，光标位按剥掉的字符数回退。
      const before =
        cur && cur.node === node ? (v.slice(0, cur.offset).match(PAD_RE) ?? []).length : 0;
      node.nodeValue = stripped;
      if (cur && cur.node === node) {
        setCaret(node, Math.min(cur.offset - before, stripped.length));
      }
    } else if (isChip(node.previousSibling) || isChip(node.nextSibling)) {
      // 名正言顺的守卫，收敛成单个字符。
      if (v !== PAD) {
        node.nodeValue = PAD;
        if (cur && cur.node === node) setCaret(node, Math.min(cur.offset, 1));
      }
    } else {
      // 块没了守卫还在（退格删块、剪切）。留着就是一个看不见的幽灵字符。
      if (cur && cur.node === node) {
        const prev = node.previousSibling;
        const next = node.nextSibling;
        if (prev?.nodeType === Node.TEXT_NODE) setCaret(prev, (prev.nodeValue ?? "").length);
        else if (next?.nodeType === Node.TEXT_NODE) setCaret(next, 0);
        else setCaret(root, Array.from(root.childNodes).indexOf(node));
      }
      (node as ChildNode).remove();
    }
    cur = caretIn(root);
  }

  for (const chip of Array.from(root.children)) {
    if (!isChip(chip)) continue;
    if (chip.previousSibling?.nodeType !== Node.TEXT_NODE) {
      chip.before(document.createTextNode(PAD));
    }
    if (chip.nextSibling?.nodeType !== Node.TEXT_NODE) {
      chip.after(document.createTextNode(PAD));
    }
  }

  settleCaret(root);
}

function chipAround(node: Node | null, root: HTMLElement): HTMLElement | null {
  let n: Node | null = node;
  while (n && n !== root) {
    if (isChip(n)) return n;
    n = n.parentNode;
  }
  return null;
}

function skipEmpty(node: Node | null, dir: 1 | -1): Node | null {
  let n = node;
  while (n && n.nodeType === Node.TEXT_NODE && !(n.nodeValue ?? "").length) {
    n = dir === 1 ? n.nextSibling : n.previousSibling;
  }
  return n;
}

/** 光标放到块右侧。落点必须是**非空**文本节点，空节点画不出光标（见 PAD）。 */
function placeCaretAfter(chip: HTMLElement) {
  let next = chip.nextSibling;
  if (!next || next.nodeType !== Node.TEXT_NODE) {
    next = document.createTextNode(PAD);
    chip.after(next);
  } else if (!(next.nodeValue ?? "").length) {
    next.nodeValue = PAD;
  }
  setCaret(next, 0);
}

/** 光标放到块左侧。同上，守卫字符保证有真实落点。 */
function placeCaretBefore(chip: HTMLElement) {
  let prev = chip.previousSibling;
  if (!prev || prev.nodeType !== Node.TEXT_NODE) {
    prev = document.createTextNode(PAD);
    chip.before(prev);
  } else if (!(prev.nodeValue ?? "").length) {
    prev.nodeValue = PAD;
  }
  setCaret(prev, (prev.nodeValue ?? "").length);
}

/** 点在块上时把光标放到外侧，不要让 WebKit 把插入点放进边框里。 */
function bindChip(span: HTMLElement): HTMLElement {
  span.addEventListener("mousedown", (e) => {
    e.preventDefault();
    const root = span.parentElement;
    if (!root) return;
    root.focus();
    const mid = span.getBoundingClientRect().left + span.getBoundingClientRect().width / 2;
    if (e.clientX < mid) placeCaretBefore(span);
    else placeCaretAfter(span);
  });
  return span;
}

/** 只删块本身，旁边的普通字符归下一次退格管；孤儿守卫由 normalizePads
 *  收走，不然留下一个看不见的幽灵字符。 */
function removeChip(chip: HTMLElement) {
  const root = chip.parentElement;
  placeCaretBefore(chip);
  chip.remove();
  if (root) normalizePads(root);
}

/**
 * 光标紧挨着的块。`before` = 块在光标前面（退格要删的那个）。
 *
 * "紧挨着"只允许隔着守卫字符（U+FEFF，不可见的光标停靠位）——它不是
 * 内容，光标语义上必须穿透它。**真实空格是普通字符**：退格先删空格、
 * 再退一次才删块，方向键在空格上也停一步。曾经把"块+尾随空格"当一个
 * 复合单元（一次退格全带走），和"块的光标行为和一个字符完全一致"的
 * 原则冲突，按后者改掉了。
 */
function adjacentChip(root: HTMLElement, side: "before" | "after"): HTMLElement | null {
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0 || !sel.isCollapsed) return null;
  const range = sel.getRangeAt(0);
  if (!root.contains(range.startContainer)) return null;

  const inside = chipAround(range.startContainer, root);
  if (inside) return inside;

  const node = range.startContainer;
  const offset = range.startOffset;

  if (node === root) {
    // 光标停在 root 层（`caretToEnd` 折叠到内容末尾就是这样，程序化插块后
    // 常见），childNodes[idx] 可能是块旁边的守卫节点而不是块本身 ——
    // 跳过守卫再判，否则退格定位不到块，WebKit 就默认"跨过"它（光标左移
    // 但不删）。只跳守卫，不跳真实空白。
    let idx = side === "before" ? offset - 1 : offset;
    let child = root.childNodes[idx] ?? null;
    while (
      child &&
      child.nodeType === Node.TEXT_NODE &&
      (child.nodeValue ?? "").replace(PAD_RE, "") === ""
    ) {
      idx += side === "before" ? -1 : 1;
      child = root.childNodes[idx] ?? null;
    }
    return isChip(child) ? child : null;
  }

  if (node.nodeType !== Node.TEXT_NODE) return null;
  const text = node.nodeValue ?? "";
  if (side === "before") {
    if (text.slice(0, offset).replace(PAD_RE, "") !== "") return null;
    const prev = skipEmpty(node.previousSibling, -1);
    return isChip(prev) ? prev : null;
  }
  if (text.slice(offset).replace(PAD_RE, "") !== "") return null;
  const next = skipEmpty(node.nextSibling, 1);
  return isChip(next) ? next : null;
}

/**
 * 光标到编辑区边缘之间是否只剩守卫字符 —— 这个方向已经顶到"墙"了。
 *
 * 不吃掉这类按键的话，方向键会在守卫字符上走出一格看不见的移动，退格
 * 会把守卫删掉（normalize 又补回来）—— 两者的观感都是"按了一下卡住"。
 */
function atGuardWall(root: HTMLElement, dir: 1 | -1): boolean {
  const cur = settleCaret(root);
  if (!cur || cur.node.nodeType !== Node.TEXT_NODE) return false;
  const text = cur.node.nodeValue ?? "";
  const rest = dir === -1 ? text.slice(0, cur.offset) : text.slice(cur.offset);
  if (rest.replace(PAD_RE, "") !== "") return false;
  return (dir === -1 ? cur.node.previousSibling : cur.node.nextSibling) === null;
}

/**
 * 贴着块的那**一个**字符自己删，不走原生删除。
 *
 * 原生删除完成后 WebKit 会对紧挨 `contenteditable=false` 元素的光标位置
 * 做归一化，而且常常归一化到块的**另一侧**：删掉块右边的空格，光标瞬移
 * 到块左边，下一次退格被当成顶墙吃掉 —— 块怎么都删不掉。自己删、自己
 * 放光标，归一化根本不参与。只接管边界上这一个字符，其余仍归原生。
 */
function deleteBesideChip(root: HTMLElement, dir: 1 | -1): boolean {
  const cur = settleCaret(root);
  if (!cur || cur.node.nodeType !== Node.TEXT_NODE) return false;
  const text = cur.node.nodeValue ?? "";
  const at = dir === -1 ? cur.offset - 1 : cur.offset;
  if (at < 0 || at >= text.length) return false;
  // 删掉这个字符后，光标和块之间只剩守卫 —— 这才是归一化会出错的边界。
  const rest = dir === -1 ? text.slice(0, at) : text.slice(at + 1);
  if (rest.replace(PAD_RE, "") !== "") return false;
  const beside =
    dir === -1
      ? skipEmpty(cur.node.previousSibling, -1)
      : skipEmpty(cur.node.nextSibling, 1);
  if (!isChip(beside)) return false;
  (cur.node as Text).deleteData(at, 1);
  setCaret(cur.node, at);
  normalizePads(root);
  return true;
}

/** 方向键跨过整块，退格/删除一次拿掉整块。处理了就返回 true。 */
export function handleChipKey(
  e: { key: string; altKey: boolean; metaKey: boolean; ctrlKey: boolean },
  root: HTMLElement,
): boolean {
  const key = e.key;
  if (key === "Backspace") {
    const chip = adjacentChip(root, "before");
    if (!chip) return atGuardWall(root, -1) || deleteBesideChip(root, -1);
    removeChip(chip);
    return true;
  }
  if (key === "Delete") {
    const chip = adjacentChip(root, "after");
    if (!chip) return atGuardWall(root, 1) || deleteBesideChip(root, 1);
    removeChip(chip);
    return true;
  }
  if (key === "ArrowLeft" && !e.altKey && !e.metaKey && !e.ctrlKey) {
    const chip =
      chipAround(window.getSelection()?.anchorNode ?? null, root) ?? adjacentChip(root, "before");
    if (!chip) return atGuardWall(root, -1);
    placeCaretBefore(chip);
    return true;
  }
  if (key === "ArrowRight" && !e.altKey && !e.metaKey && !e.ctrlKey) {
    const chip =
      chipAround(window.getSelection()?.anchorNode ?? null, root) ?? adjacentChip(root, "after");
    if (!chip) return atGuardWall(root, 1);
    placeCaretAfter(chip);
    return true;
  }
  return false;
}

/** 把编辑区的 DOM 读成段落序列。 */
export function readEditor(el: HTMLElement): Seg[] {
  const out: Seg[] = [];
  const push = (s: Seg) => {
    const last = out[out.length - 1];
    if (s.kind === "text" && last?.kind === "text") last.value += s.value;
    else if (s.kind !== "text" || s.value) out.push(s);
  };
  const walk = (node: Node, depth: number) => {
    let first = true;
    for (const child of Array.from(node.childNodes)) {
      if (child.nodeType === Node.TEXT_NODE) {
        // 守卫字符只属于 DOM，不属于内容 —— 落进草稿或消息就是脏数据。
        push({ kind: "text", value: (child.nodeValue ?? "").replace(PAD_RE, "") });
      } else if (child instanceof HTMLElement) {
        const chip = chipSegFromEl(child);
        if (chip) {
          push(chip);
        } else if (child.tagName === "BR") {
          push({ kind: "text", value: "\n" });
        } else {
          // 浏览器在换行/粘贴时会包一层 div。除了第一层，块级元素的
          // 边界就是一个换行。
          if (depth > 0 || !first) push({ kind: "text", value: "\n" });
          walk(child, depth + 1);
        }
      }
      first = false;
    }
  };
  walk(el, 0);
  return out;
}

/** 用段落序列重建编辑区（切会话、发送失败回滚、程序化改写时用）。 */
export function writeEditor(el: HTMLElement, segs: Seg[]) {
  el.replaceChildren();
  for (const s of segs) {
    el.appendChild(s.kind === "text" ? document.createTextNode(s.value) : chipEl(s));
  }
  // 句首/相邻的块马上就有守卫，光标不用等下一次 sync 才能停到块的两侧。
  normalizePads(el);
}

/** 把光标放到编辑区末尾（安置进文本节点，不留 root 边界光标）。 */
export function caretToEnd(el: HTMLElement) {
  const sel = window.getSelection();
  if (!sel) return;
  const r = document.createRange();
  r.selectNodeContents(el);
  r.collapse(false);
  sel.removeAllRanges();
  sel.addRange(r);
  settleCaret(el);
}

/** 光标前那个还没敲完的 `@查询`。没有就是 undefined（菜单不出）。 */
export function queryAtCaret(el: HTMLElement): string | undefined {
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0) return undefined;
  const r = sel.getRangeAt(0);
  if (!el.contains(r.startContainer) || r.startContainer.nodeType !== Node.TEXT_NODE) {
    return undefined;
  }
  const before = (r.startContainer.nodeValue ?? "").slice(0, r.startOffset);
  // 边界规则与内核一致：中文后面直接敲 `@` 也要出菜单（"读下@" 是中文
  // 用户的常态写法，要求先打个空格等于让他们用不了这个菜单）。
  return /(?:^|[^A-Za-z0-9._%+-])@([^\s@]*)$/.exec(before)?.[1];
}

/**
 * 在光标处把 `@查询` 换成一个块。
 *
 * 光标贴在块的右缘，接着打字就是正常续写，不用再点一下输入框。
 * 不自动补空格 —— 空格是内容，要不要由用户自己敲；文件引用后面直接跟
 * 中文也没事，`mentionToken` 认不回裸写法时会落引号形式。
 */
export function insertChipAtCaret(el: HTMLElement, seg: ChipSeg) {
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0) {
    el.appendChild(chipEl(seg));
    normalizePads(el);
    caretToEnd(el);
    return;
  }
  const range = sel.getRangeAt(0);
  const node = range.startContainer;
  if (node.nodeType === Node.TEXT_NODE) {
    const before = (node.nodeValue ?? "").slice(0, range.startOffset);
    const m = /(^|[^A-Za-z0-9._%+-])@[^\s@]*$/.exec(before);
    if (m) {
      const cut = before.length - (m[0].length - (m[1]?.length ?? 0));
      (node as Text).deleteData(cut, range.startOffset - cut);
      range.setStart(node, cut);
      range.collapse(true);
    }
  }
  const chip = chipEl(seg);
  range.insertNode(chip);
  // 先补守卫再放光标：块插在句首/句尾时两侧都要有停靠位。
  normalizePads(el);
  placeCaretAfter(chip);
}

/** 去掉光标前那段 `@查询`（Esc 收起文件菜单时用）。 */
export function dropQueryAtCaret() {
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0) return;
  const range = sel.getRangeAt(0);
  const node = range.startContainer;
  if (node.nodeType !== Node.TEXT_NODE) return;
  const before = (node.nodeValue ?? "").slice(0, range.startOffset);
  const m = /@[^\s@]*$/.exec(before);
  if (!m) return;
  const cut = before.length - m[0].length;
  (node as Text).deleteData(cut, range.startOffset - cut);
}
