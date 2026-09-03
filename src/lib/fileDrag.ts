//! 应用内把一个文件拖到别处（文件树 → 输入框）。
//!
//! 没走 HTML5 拖放，也不能走：Tauri 的原生拖放在 webview 层把整个拖放
//! 目标协议接管了（wry 的 draggingEntered / performDragOperation 收下事件
//! 就直接返回，不再交给 WebKit），页面里 dragstart 之后 dragover / drop
//! 一个都收不到。两条路互斥，而原生那条给的是磁盘**绝对路径** —— 关掉它，
//! 从访达拖文件进来就再也变不成引用块和图片附件了（见 bridge 的
//! `subscribeDragDrop`）。所以应用内这条自己用 pointer 事件做：自己画
//! 拖影、自己命中落点。
//!
//! 落点用 DOM 注册而不是 React context：拖拽的两头（文件树、输入框）在
//! 组件树上隔着大半个应用，为它们架一条 context 只会让中间每一层都知道
//! 有这么回事。

/** 拖着的东西。绝对路径 —— 落点那边和访达拖进来的走同一条收件路径。 */
export interface FileDragItem {
  abs: string;
}

/** 一个落点。`el` 用来做命中测试，两个回调分别管高亮和收件。 */
export interface FileDropZone {
  el: HTMLElement;
  onOver: (over: boolean) => void;
  onDrop: (item: FileDragItem) => void;
}

const zones = new Set<FileDropZone>();

/** 注册一个落点，返回注销函数。 */
export function registerFileDrop(zone: FileDropZone): () => void {
  zones.add(zone);
  return () => {
    zones.delete(zone);
  };
}

/**
 * 按下之后要移开这么多像素才算"在拖"。
 *
 * 树里点一个文件是打开预览，手抖两像素不该变成拖拽。反过来门槛太高更糟：
 * 用户已经拖了一段却还没有拖影，会当成这里拖不动。
 */
const SLOP = 5;

/** 拖影贴着指针的偏移。压在指针正下方会挡住"落点亮没亮"。 */
const GHOST_DX = 14;
const GHOST_DY = 12;

/**
 * 开始拖一个文件。挂在源头的 `onPointerDown` 上 —— 到底算点击还是算拖，
 * 由指针有没有走出 SLOP 决定，源头不用管。
 *
 * `row` 是被抓住的那个 DOM 节点，克隆一份当拖影。必须在事件处理函数里
 * **同步**取（`e.currentTarget` 派发完就被 React 置空了）。
 */
export function startFileDrag(
  e: { button: number; clientX: number; clientY: number },
  item: FileDragItem,
  row: HTMLElement,
) {
  if (e.button !== 0) return;
  const from = { x: e.clientX, y: e.clientY };
  const listeners = new AbortController();
  let ghost: HTMLElement | null = null;
  let hot: FileDropZone | null = null;

  const setHot = (z: FileDropZone | null) => {
    if (z === hot) return;
    hot?.onOver(false);
    hot = z;
    hot?.onOver(true);
    document.body.classList.toggle("file-drop-hot", hot !== null);
  };

  /**
   * 指针底下的落点。走 `elementFromPoint` 而不是逐个比 rect：落点上面压着
   * 弹窗时，比 rect 会把够不着的那个判成命中。
   */
  const zoneAt = (x: number, y: number): FileDropZone | null => {
    const el = document.elementFromPoint(x, y);
    if (!el) return null;
    for (const z of zones) {
      if (z.el.contains(el)) return z;
    }
    return null;
  };

  const finish = (deliver: boolean) => {
    listeners.abort();
    const target = deliver ? hot : null;
    setHot(null);
    if (ghost) {
      ghost.remove();
      document.body.classList.remove("file-dragging");
      // 拖过了，紧跟着的那次 click 就不再是"点了这一行"。不吃掉的话，
      // 拖到一半反悔、松手落回原处，会顺手把这个文件开成预览标签。
      swallowNextClick();
    }
    target?.onDrop(item);
  };

  const { signal } = listeners;
  // 按下的这一瞬就把"起选区"这条路堵死，不等走出 SLOP。
  //
  // WebKit 在 `user-select: none` 的元素上按下再拖，会往上找最近的**可选**
  // 位置当锚点 —— 那个位置常常在半个文档之外，于是拖一行文件树，整条对话
  // 流跟着被刷蓝。selectstart 是专为这件事留的口子：拦住它选区根本不会
  // 开始，而焦点和 click 都不受影响。拦 mousedown 也挡得住选区，但会顺手
  // 把焦点也挡掉，树的键盘光标环就没了。
  //
  // 拖起来之后 body 上那条 `user-select: none`（见 styles.css 的
  // `.file-dragging`）管的是另一半：指针扫过别处时不让它再起念头。
  document.addEventListener("selectstart", preventDefault, { signal });
  window.addEventListener(
    "pointermove",
    (ev) => {
      if (!ghost) {
        if (Math.abs(ev.clientX - from.x) + Math.abs(ev.clientY - from.y) < SLOP) return;
        ghost = makeGhost(row);
        document.body.classList.add("file-dragging");
      }
      ghost.style.transform = `translate(${ev.clientX + GHOST_DX}px, ${ev.clientY + GHOST_DY}px)`;
      setHot(zoneAt(ev.clientX, ev.clientY));
    },
    { signal },
  );
  window.addEventListener("pointerup", () => finish(true), { signal });
  window.addEventListener("pointercancel", () => finish(false), { signal });
  window.addEventListener(
    "keydown",
    (ev) => {
      // 拖到一半反悔。只在真的拖起来之后才认 —— 否则"按住不动顺手按了
      // 一下 Esc"会把别处的 Esc（关弹窗、停止当前轮）连带吃掉。
      if (ev.key !== "Escape" || !ghost) return;
      ev.preventDefault();
      ev.stopPropagation();
      finish(false);
    },
    { capture: true, signal },
  );
}

function preventDefault(ev: Event) {
  ev.preventDefault();
}

/**
 * 拖影：把抓住的那一行原样克隆一份。
 *
 * 另画一个"文件卡片"要把图标、截断规则再实现一遍，而用户抓住的本来就是
 * 这一行 —— 克隆既省事，跟手的感觉也更实。
 */
function makeGhost(row: HTMLElement): HTMLElement {
  const box = document.createElement("div");
  box.className = "file-drag-ghost";
  box.setAttribute("aria-hidden", "true");
  const clone = row.cloneNode(true) as HTMLElement;
  // id 会和原件撞车；选中态 / 键盘光标是树里的状态，拖影上没有意义。
  clone.removeAttribute("id");
  clone.classList.remove("selected", "cursor");
  // 缩进是行上的内联样式（见 FileTree 的 indent），拖影不该带着它。
  clone.style.paddingLeft = "";
  box.appendChild(clone);
  document.body.appendChild(box);
  return box;
}

/** 吃掉紧随其后的那一次 click。 */
function swallowNextClick() {
  const eat = (ev: MouseEvent) => {
    ev.preventDefault();
    ev.stopPropagation();
  };
  window.addEventListener("click", eat, true);
  // click 和松手在同一轮事件里派发，setTimeout(0) 稳稳排在它后面。拆监听
  // 这一步不能省：落在别处松手时压根没有 click，留着的话用户接下来的
  // 第一次点击会凭空失灵。
  setTimeout(() => window.removeEventListener("click", eat, true), 0);
}
