//! 弹窗的公共外壳：遮罩、dialog 语义、焦点陷阱、Esc 栈、关闭还焦。
//!
//! 之前每个弹窗自己监听 window 的 Esc，叠开时互相打架 —— 模型弹窗按
//! Esc 连设置一起关、图片查看器开着按 Esc 会顺手拒绝权限请求。栈让
//! Esc 永远只落在最上层。

import { type ReactNode, useEffect, useRef } from "react";
import { createPortal } from "react-dom";

/* ── Esc 栈 ─────────────────────────────────── */

interface Layer {
  onEsc: () => void;
}

const layers: Layer[] = [];
let listening = false;

function onWindowKey(e: KeyboardEvent) {
  if (e.key !== "Escape" || e.defaultPrevented) return;
  // 终端把 Esc 交给 shell（vim 靠它活着），这里不抢。
  if (e.target instanceof Element && e.target.closest(".xterm")) return;
  const top = layers[layers.length - 1];
  if (!top) return;
  e.preventDefault();
  top.onEsc();
}

/**
 * 注册一层"Esc 关我"。弹窗、图片查看器、权限卡共用这一个栈 ——
 * 叠开时 Esc 只关最上层，不会一次按键放倒一摞。
 *
 * 元素自己消费 Esc 的地方（地址栏放弃编辑、斜杠菜单收起）记得
 * `preventDefault()`，栈看到 defaultPrevented 就不接手。
 */
export function useEscLayer(onEsc: () => void) {
  const ref = useRef(onEsc);
  ref.current = onEsc;
  useEffect(() => {
    const layer: Layer = { onEsc: () => ref.current() };
    layers.push(layer);
    if (!listening) {
      listening = true;
      window.addEventListener("keydown", onWindowKey);
    }
    return () => {
      const i = layers.indexOf(layer);
      if (i >= 0) layers.splice(i, 1);
      if (layers.length === 0) {
        listening = false;
        window.removeEventListener("keydown", onWindowKey);
      }
    };
  }, []);
}

/* ── 弹窗外壳 ───────────────────────────────── */

/** Tab 循环用的可聚焦元素。disabled 的不算 —— 焦点落上去等于消失。 */
const FOCUSABLE =
  'a[href], button:not(:disabled), input:not(:disabled), select:not(:disabled), ' +
  'textarea:not(:disabled), [tabindex]:not([tabindex="-1"])';

/**
 * 所有居中弹窗的外壳。内容自备（modal-head / 各自的 body），
 * 外壳统一管：
 *
 * - `role="dialog"` + `aria-modal`：读屏能宣告"对话框已打开"并限制朗读范围；
 * - 焦点陷阱：Tab 在弹窗里循环，不会走到遮罩背后的页面上；
 * - 打开时焦点移进来、关闭时还给打开它的控件；
 * - Esc / 点遮罩 = `onClose`。调用方有未保存内容时在 onClose 里拦。
 */
export function Modal({
  className,
  label,
  alert,
  portal,
  onClose,
  children,
}: {
  className?: string;
  /** 读屏宣告的名字。 */
  label: string;
  /** 确认框置真 —— alertdialog 会让读屏立即朗读内容。 */
  alert?: boolean;
  /**
   * 渲染到 `document.body` 而不是就地。**住在开合面板里的弹窗必须置真。**
   *
   * 面板收起时内容会整层淡出（见 styles.css 的 .slide-panel-inner），而
   * 透明度是往下乘到所有后代的 —— 包括 `position: fixed` 的遮罩（它不像
   * containment 那样改包含块，所以不会被壳裁掉，但**会**跟着淡）。留在
   * 壳里就会变成"看不见但还在、还吃点击"的一层。
   *
   * `[约束]` 不能改成默认置真。对话流里的删除确认刻意就地渲染 —— 外面
   * 套一层 `.transcript-confirm` 把全屏 fixed 遮罩改成相对聊天区的
   * absolute，遮罩只罩对话列、不盖侧栏（见 styles.css 那处）。portal
   * 出去那层作用域覆盖就没了。
   */
  portal?: boolean;
  /** 请求关闭（Esc / 点遮罩 / 关闭按钮共用一条路）。 */
  onClose: () => void;
  children: ReactNode;
}) {
  const boxRef = useRef<HTMLDivElement>(null);
  useEscLayer(onClose);

  useEffect(() => {
    const opener =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const box = boxRef.current;
    // autoFocus 的控件（确认框的「取消」）此刻已经拿到焦点，不抢。
    if (box && !box.contains(document.activeElement)) box.focus();
    return () => opener?.focus();
  }, []);

  const trap = (e: React.KeyboardEvent) => {
    if (e.key !== "Tab") return;
    const box = boxRef.current;
    if (!box) return;
    // offsetParent 过滤掉 display:none 的（没显示的分区里的控件）
    const items = [...box.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
      (el) => el.offsetParent !== null,
    );
    if (items.length === 0) {
      e.preventDefault();
      return;
    }
    const first = items[0];
    const last = items[items.length - 1];
    const cur = document.activeElement;
    if (e.shiftKey && (cur === first || cur === box)) {
      e.preventDefault();
      last?.focus();
    } else if (!e.shiftKey && cur === last) {
      e.preventDefault();
      first?.focus();
    }
  };

  const shell = (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => e.target === e.currentTarget && onClose()}
    >
      <div
        ref={boxRef}
        className={className ? `modal ${className}` : "modal"}
        role={alert ? "alertdialog" : "dialog"}
        aria-modal="true"
        aria-label={label}
        tabIndex={-1}
        onKeyDown={trap}
      >
        {children}
      </div>
    </div>
  );

  // Esc 栈和焦点陷阱都不依赖 DOM 位置（栈是模块级的、陷阱走 boxRef），
  // portal 出去两者照旧工作。
  return portal ? createPortal(shell, document.body) : shell;
}
