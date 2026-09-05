//! contenteditable 块编辑器的机械：块节点、光标、守卫字符、键盘行为。
//!
//! 从 Composer 拆出，输入框（Composer）和消息编辑框（Transcript 的
//! MsgEditor）共用 —— 改光标/退格/守卫的行为只动这个文件，两处一起生效。
//! 块的"知识"（种类、属性、样式类）在 `chips.ts`；React 静态渲染在
//! `components/Chip.tsx`；这里只有命令式 DOM —— contenteditable 里
//! React 一 re-render 就冲掉光标，进不来。
//!
//! 行为总纲：**块的光标行为和一个字符完全一致**。落点恰好在块两侧、
//! 方向键一步跨块、退格一次删整块、真实空格是普通字符、换行一次换一行。

import { type ChipSeg, chipAttrs, chipClass, chipSegFromEl, chipVars } from "./chips";
import { SLASH_CH, type Seg } from "./promptText";

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
  for (const [k, v] of Object.entries(chipVars(seg))) span.style.setProperty(k, v);
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
 * 光标不停在守卫**前面**，停在它后面。返回跳过守卫之后的偏移。
 *
 * 守卫前后在语义上是同一个位置，对渲染却不是。WebKit 在 Cocoa 上走
 * CoreText 整形，U+FEFF 这类 default-ignorable 字符会被并进**前一个字母的
 * 字形簇**；簇内部的光标位置 WebKit 按比例插值来画（那是给连字准备的）。
 * 光标停在守卫前面、这时打字，字就长在"字母 + 守卫"这个簇里，光标落在
 * 簇的 1/2 处 —— 画在字母正中间。平时看不到是因为 `normalizePads` 一有
 * 真实字符就把守卫剥了；输入法组字期间不能动 DOM，拼音就一直顶着守卫。
 * 光标停在守卫后面，打进去的字永远跟在守卫**之后**，光标始终在簇边界上。
 * Slate 的零宽占位同样是 U+FEFF，踩的是同一个坑。
 */
function afterPads(text: string, offset: number): number {
  let o = offset;
  while (o < text.length && text[o] === PAD) o++;
  return o;
}

/**
 * 把光标安置到一个没有歧义的位置，返回安置后的位置：
 *
 * 1. 停在 root 边界（node 是编辑区本身、offset 是子节点序号）时进旁边的
 *    文本节点。root 边界光标是各种怪行为的温床：`caretToEnd` 和原生删除都
 *    会留下它，而 WebKit 对紧挨 `contenteditable=false` 元素的边界位置做
 *    归一化时常跳到块的**另一侧** —— 表现是删掉块右边的空格后光标瞬移到
 *    块左边。文本节点内的位置没有这个歧义。
 * 2. 停在守卫前面时挪到守卫后面（原因见 [`afterPads`]）。
 */
function settleCaret(root: HTMLElement): { node: Node; offset: number } | null {
  let cur = caretIn(root);
  if (!cur) return null;
  if (cur.node === root) {
    const prev = root.childNodes[cur.offset - 1];
    const next = root.childNodes[cur.offset];
    if (prev?.nodeType === Node.TEXT_NODE) {
      setCaret(prev, (prev.nodeValue ?? "").length);
    } else if (next?.nodeType === Node.TEXT_NODE) {
      setCaret(next, 0);
    }
    cur = caretIn(root);
    if (!cur) return null;
  }
  if (cur.node.nodeType === Node.TEXT_NODE) {
    const o = afterPads(cur.node.nodeValue ?? "", cur.offset);
    if (o !== cur.offset) {
      setCaret(cur.node, o);
      cur = caretIn(root);
    }
  }
  return cur;
}

/**
 * 块后面那个空行要不要留一个停靠位。
 *
 * pre-wrap 下换行是一个真实的 `\n` 字符，而编辑区**末尾**的 `\n` 不生成
 * 行盒 —— 空行画不出来，光标也停不上去。浏览器自己那套占位（段末再补一个
 * `\n`）在块旁边是坏的（见 [`insertLineBreak`]），这一格于是由守卫顶上，
 * 和块两侧的守卫是同一件事：给光标一个真实的渲染盒。
 *
 * 只认"块后面全是换行"这一种。中间夹着真实文字时（`chip` + `abc\n`）行盒
 * 由那段文字撑着，浏览器的占位也判得对，再垫一个守卫反而会把它那个**不可见
 * 的**收尾换行变成一个看得见的空行 —— 退格删掉换行后空行还赖着不走。
 */
function needsTailPad(node: Node, root: HTMLElement): boolean {
  return (
    node === root.lastChild &&
    isChip(node.previousSibling) &&
    /^\n+$/.test((node.nodeValue ?? "").replace(PAD_RE, ""))
  );
}

/**
 * 维护停靠字符的不变量，光标跟着一起校正：
 *
 * 1. 相邻文本节点已合并、空文本节点已删（`root.normalize()`）；
 * 2. 块的某一侧没有文本节点时，垫一个只含 PAD 的守卫节点；
 * 3. 有真实字符的文本节点里不留 PAD（真实字符本身就是落点，混着守卫
 *    只会多出一个"按一下没动静"的幽灵光标位）—— 例外见 [`needsTailPad`]。
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
      const want = needsTailPad(node, root) ? stripped + PAD : stripped;
      const before =
        cur && cur.node === node ? (v.slice(0, cur.offset).match(PAD_RE) ?? []).length : 0;
      node.nodeValue = want;
      if (cur && cur.node === node) {
        setCaret(node, Math.min(cur.offset - before, want.length));
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

  // 上面那轮只碰已经含 PAD 的节点，块后面新换的那一行还是光的。
  const tail = root.lastChild;
  if (
    tail?.nodeType === Node.TEXT_NODE &&
    !(tail.nodeValue ?? "").includes(PAD) &&
    needsTailPad(tail, root)
  ) {
    tail.nodeValue += PAD;
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

/** 光标放到块右侧。落点必须是**非空**文本节点，空节点画不出光标（见 PAD）；
 *  有守卫时停在守卫**后面**（见 afterPads）。 */
function placeCaretAfter(chip: HTMLElement) {
  let next = chip.nextSibling;
  if (!next || next.nodeType !== Node.TEXT_NODE) {
    next = document.createTextNode(PAD);
    chip.after(next);
  } else if (!(next.nodeValue ?? "").length) {
    next.nodeValue = PAD;
  }
  setCaret(next, afterPads(next.nodeValue ?? "", 0));
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
 *
 * "贴着块"看的是删完之后光标的**两侧**，不只是删除方向那一侧：`11|` 后面
 * 跟着块，退格删掉第二个 1 之后文本节点空了，光标贴着**右边**的块 ——
 * 同一个归一化，光标跑到块右边去。早先只看删除方向，这一种就漏了。
 */
function deleteBesideChip(root: HTMLElement, dir: 1 | -1): boolean {
  const cur = settleCaret(root);
  if (!cur || cur.node.nodeType !== Node.TEXT_NODE) return false;
  const text = cur.node.nodeValue ?? "";
  // 要删的是光标旁边第一个**真实**字符，守卫不算（光标停在守卫后面，
  // 退格方向上紧挨着的可能就是守卫）。
  let at = dir === -1 ? cur.offset - 1 : cur.offset;
  while (at >= 0 && at < text.length && text[at] === PAD) at += dir;
  if (at < 0 || at >= text.length) return false;
  // 删掉这个字符后，光标和某一侧的块之间只剩守卫 —— 这才是归一化会出错的边界。
  const leftBare = text.slice(0, at).replace(PAD_RE, "") === "";
  const rightBare = text.slice(at + 1).replace(PAD_RE, "") === "";
  const touchesChip =
    (leftBare && isChip(skipEmpty(cur.node.previousSibling, -1))) ||
    (rightBare && isChip(skipEmpty(cur.node.nextSibling, 1)));
  if (!touchesChip) return false;
  (cur.node as Text).deleteData(at, 1);
  setCaret(cur.node, at);
  normalizePads(root);
  return true;
}

/**
 * 贴着块的那一次换行自己插。处理了就返回 true，没处理的交回浏览器。
 *
 * pre-wrap 下换行是一个真实的 `\n` 字符，而编辑区末尾的 `\n` 不生成行盒，
 * 所以浏览器插换行时会在**段末**再补一个 `\n` 当占位（占位后面没东西，
 * 不可见）。它判断"段末"看的是可视位置，而紧挨 `contenteditable=false`
 * 元素的位置一律被算作段末 —— 于是块旁边这一下两头都是坏的：
 *
 * - 光标在块**前面**：占位照补，可它后面还跟着块，不再是不可见的收尾，
 *   而是**多出来的一整个空行**。一次换两行，退一次格只回得到中间那行 ——
 *   看着就是"光标回不到上一行"。
 * - 光标在块**后面**：守卫字符（U+FEFF）让它以为后面还有内容、不是段末，
 *   占位不补，空行没有行盒 —— 换行看上去根本没发生。
 *
 * 自己插就没有这个歧义：一个 `\n`，一次一行，新空行的落点由守卫顶上
 * （见 [`needsTailPad`]）。只接管贴着块的这一下，其余仍归原生 —— 和
 * [`deleteBesideChip`] 是同一个取舍。
 *
 * `[约束]` 不能改走 `execCommand("insertText", …, "\n")`。它在 WebKit /
 * Blink 里都不是"插一个字符"：TypingCommand 按 `\n` 切段，走的正是上面
 * 那条 insertParagraphSeparator —— 内容会被包进 `<div>`，块首当其冲。
 */
export function insertLineBreak(root: HTMLElement): boolean {
  if (!adjacentChip(root, "before") && !adjacentChip(root, "after")) return false;
  const cur = settleCaret(root);
  if (!cur) return false;
  if (cur.node.nodeType === Node.TEXT_NODE) {
    (cur.node as Text).insertData(cur.offset, "\n");
    setCaret(cur.node, cur.offset + 1);
  } else if (cur.node === root) {
    // root 边界光标（`settleCaret` 安置不进文本节点时）：自己造一个。
    const nl = document.createTextNode("\n");
    root.insertBefore(nl, root.childNodes[cur.offset] ?? null);
    setCaret(nl, 1);
  } else {
    // 光标落进了块内部。这里的 offset 是块里的序号，拿它当落点会插错
    // 地方 —— 这一下交回浏览器。
    return false;
  }
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

/** 这次原生删除的目标区间会碰到的块。没碰到、或浏览器不给区间时是 null。 */
function chipInTargetRanges(e: InputEvent, root: HTMLElement): HTMLElement | null {
  for (const r of e.getTargetRanges()) {
    const range = document.createRange();
    range.setStart(r.startContainer, r.startOffset);
    range.setEnd(r.endContainer, r.endOffset);
    for (const chip of Array.from(root.children)) {
      if (isChip(chip) && range.intersectsNode(chip)) return chip;
    }
  }
  return null;
}

/**
 * compositionend 之后多长时间内，贴着块的原生删除仍按"输入法的键"处理。
 *
 * 这是"行内原子节点 + 输入法"这个经典问题的经典解法（ProseMirror 的
 * `inOrNearComposition` 用的就是 500ms）。结束组字的那一键在 WebKit 里的
 * 顺序是：输入法先处理 → compositionend → **然后**这一键的 keydown / 默认
 * 动作才到页面。所以"刚结束组字"和"这一键的后续"之间隔的是一次 IPC 往返，
 * 不是零 —— 靠 `setTimeout(0)` 翻标志位盖不住，得看时间窗。
 *
 * 窗口只影响"贴着块的原生删除"这一种输入：正常退格在 keydown 层就被接管，
 * 走不到这里；组完字后 500ms 内要删的也是刚上屏的字，不贴块。
 */
export const IME_TAIL_MS = 500;

/**
 * 原生删除落地前的最后一道闸：会碰到块的删除一律不交给浏览器。
 *
 * [`handleChipKey`] 在 keydown 层拦不全 —— 属于输入法的退格是放过的（拦了
 * 会打断组字），可输入法在缓冲区删空、或它自己的光标停在拼音最前面时会把
 * 这记退格**放行**给 WebKit 执行，而 WebKit 对紧挨光标的 `contenteditable=false`
 * 元素默认整块删掉：块右边直接打拼音再退格，块就没了。到这一步只有
 * beforeinput 还来得及。
 *
 * `imeActive` 说这次删除属于输入法时只拦不删：那一记本来就不该删除任何
 * 东西（拼音已经由输入法删掉了）。不属于时（触控板手势、辅助功能之类不经
 * keydown 的删除）按 handleChipKey 的规矩整块拿掉。
 *
 * `[约束]` `imeActive` 必须把 compositionend 之后的 [`IME_TAIL_MS`] 算进去，
 * 不能只看组字中的标志位 —— 原因见那个常量。早先只看标志位，标志位在
 * compositionend 和这次 beforeinput 之间翻掉，这里就亲手把块删了。
 *
 * 只认 `deleteContent{Backward,Forward}`。输入法自己收缩拼音走的是
 * `insertCompositionText` / `deleteCompositionText`，不会碰到这里。
 *
 * `[约束]` 必须挂原生监听。React 的 `onBeforeInput` 是用 keypress/textInput
 * 拼出来的兼容层，删除根本不会触发它。
 */
export function guardChipDeletes(
  root: HTMLElement,
  imeActive: () => boolean,
  onChange: () => void,
): () => void {
  const onBeforeInput = (e: InputEvent) => {
    const side =
      e.inputType === "deleteContentBackward"
        ? "before"
        : e.inputType === "deleteContentForward"
          ? "after"
          : null;
    if (!side) return;
    const chip = chipInTargetRanges(e, root) ?? adjacentChip(root, side);
    if (!chip) return;
    e.preventDefault();
    if (imeActive()) return;
    removeChip(chip);
    onChange();
  };
  root.addEventListener("beforeinput", onBeforeInput);
  return () => root.removeEventListener("beforeinput", onBeforeInput);
}

/**
 * 这个元素的边界算不算一个换行。
 *
 * 只有块级的才算。WebKit / Blink 在块旁边原生删除时会就地包一层保留样式的
 * `<span>`（`font-family: inherit` 之类的空样式，看不出来），把它也当成换行
 * 的话，草稿里凭空多一行 —— 发出去就是消息里多一个空行。
 *
 * 脱离文档的编辑区拿不到计算样式（`display` 是空串），按块级算 —— 和这条
 * 规则出现之前的行为一致。
 */
function isBlockBox(el: HTMLElement): boolean {
  return !getComputedStyle(el).display.startsWith("inline");
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
          // 边界就是一个换行（行内壳不算，见 isBlockBox）。
          if ((depth > 0 || !first) && isBlockBox(child)) push({ kind: "text", value: "\n" });
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

/** 补全菜单的触发符：`@` 文件、`/` 命令和技能。 */
export type Trigger = "@" | "/";

/**
 * 光标前那段还没敲完的 `触发符查询`。只在行首或空白之后触发（Cursor 同款）：
 * 邮箱、`a@b`、`/usr/bin` 这类写法里的符号不该弹菜单。第 1 组是触发前那个
 * 字符（行首为空），第 2 组是查询。
 */
const TRIGGER_RE: Record<Trigger, RegExp> = {
  "@": /(^|\s)@([^\s@]*)$/,
  "/": new RegExp(`(^|\\s)/(${SLASH_CH}*)$`, "u"),
};

/**
 * 光标所在文本节点里光标之前的内容，去掉守卫。
 *
 * 节点开头不等于行首：前面还挂着块（`[块]@`）的话，块和 `@` 之间没有空格，
 * 不算"行首"—— 垫一个非空白字符让 `^` 匹配不上。
 */
function textBeforeCaret(el: HTMLElement, node: Text, offset: number): string {
  const before = (node.nodeValue ?? "").slice(0, offset).replace(PAD_RE, "");
  return node.previousSibling && node.parentNode === el ? `x${before}` : before;
}

/** 光标前那个还没敲完的 `@查询` / `/查询`。没有就是 undefined（菜单不出）。 */
export function queryAtCaret(el: HTMLElement, trigger: Trigger): string | undefined {
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0) return undefined;
  const r = sel.getRangeAt(0);
  if (!el.contains(r.startContainer) || r.startContainer.nodeType !== Node.TEXT_NODE) {
    return undefined;
  }
  return TRIGGER_RE[trigger].exec(
    textBeforeCaret(el, r.startContainer as Text, r.startOffset),
  )?.[2];
}

/**
 * 在光标处插一个块，块后面补一个空格，光标停在空格后面。给了 `trigger`
 * 就先把光标前那段 `触发符查询` 吃掉（菜单选中就是"把查询换成块"）。
 *
 * 空格是给接着打字用的：块和正文之间本来就该有一格，让用户自己敲等于
 * 每次都多按一下。后面已经有空白（插在句子中间）就不重复补。
 */
export function insertChipAtCaret(el: HTMLElement, seg: ChipSeg, trigger?: Trigger) {
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0) {
    el.appendChild(chipEl(seg));
    el.appendChild(document.createTextNode(" "));
    normalizePads(el);
    caretToEnd(el);
    return;
  }
  if (trigger) dropQueryAtCaret(trigger);
  const range = sel.getRangeAt(0);
  const chip = chipEl(seg);
  range.insertNode(chip);
  // 先补守卫再放光标：块插在句首/句尾时两侧都要有停靠位。
  normalizePads(el);
  placeCaretAfter(chip);
  const cur = caretIn(el);
  if (!cur || cur.node.nodeType !== Node.TEXT_NODE) return;
  const text = cur.node as Text;
  const rest = (text.nodeValue ?? "").slice(cur.offset).replace(PAD_RE, "");
  if (/^\s/.test(rest)) {
    // 后面本来就有空白，光标跳到它后面即可。
    setCaret(text, cur.offset + 1);
  } else {
    text.insertData(cur.offset, " ");
    setCaret(text, cur.offset + 1);
  }
  normalizePads(el);
}

/** 去掉光标前那段 `触发符查询`（Esc 收起菜单、菜单选中换成块时用）。 */
export function dropQueryAtCaret(trigger: Trigger) {
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0) return;
  const range = sel.getRangeAt(0);
  const node = range.startContainer;
  if (node.nodeType !== Node.TEXT_NODE) return;
  const before = (node.nodeValue ?? "").slice(0, range.startOffset);
  const m = TRIGGER_RE[trigger].exec(before);
  if (!m) return;
  // 第 1 组是触发前那个字符（空白），留下；只删 `符号 + 查询`。
  const cut = before.length - (m[0].length - (m[1]?.length ?? 0));
  (node as Text).deleteData(cut, range.startOffset - cut);
  setCaret(node, cut);
}
