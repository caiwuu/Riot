import { useCallback, useEffect, useRef, useState } from "react";

import {
  type BrowserFrame,
  type BrowserInput,
  browserInput,
  browserNavigate,
  closeBrowser,
  openBrowser,
} from "../bridge";

/**
 * 内置浏览器面板。
 *
 * 画面是宿主推来的 JPEG 帧，输入原路打回页面。看起来像内嵌了一个浏览器，
 * 实际是另一个进程在渲染 —— 那个进程跑的是 CEF，而它必须活在自己的
 * `.app` 里（见 crates/riot-browser）。
 *
 * # 为什么不用 iframe
 *
 * iframe 只能显示，读不到跨域页面的 DOM、截不了图、也拿不到 console。
 * 而这个面板存在的意义正是"你和模型看同一个东西" —— 模型那边靠 CDP，
 * 面板这边靠帧流，两者看的是同一个渲染结果。
 */
export function BrowserPanel({
  sessionId,
  onClose,
}: {
  sessionId: string;
  onClose: () => void;
}) {
  const [frame, setFrame] = useState<BrowserFrame | null>(null);
  const [address, setAddress] = useState("");
  const [busy, setBusy] = useState(false);
  const viewRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const sub = openBrowser(sessionId, setFrame);
    return () => {
      sub.unsubscribe();
      // 让宿主也停下来。只取消本地订阅的话，另一头还在编码 JPEG。
      void closeBrowser(sessionId);
    };
  }, [sessionId]);

  /**
   * DOM 坐标 → 页面坐标。
   *
   * `[约束]` 必须按帧的真实尺寸换算，不能直接用鼠标事件的 offsetX/Y。
   * 画面是等比缩放后铺在面板里的，两者的比例通常不是 1 —— 不换算的话
   * 点击会系统性地偏，而且窗口越窄偏得越多。
   */
  const toPage = useCallback(
    (e: React.MouseEvent): { x: number; y: number } | null => {
      const img = viewRef.current?.querySelector("img");
      if (!img || !frame) return null;
      const r = img.getBoundingClientRect();
      if (r.width === 0 || r.height === 0) return null;
      return {
        x: ((e.clientX - r.left) / r.width) * frame.width,
        y: ((e.clientY - r.top) / r.height) * frame.height,
      };
    },
    [frame],
  );

  const send = useCallback(
    (input: BrowserInput) => void browserInput(sessionId, input).catch(() => {}),
    [sessionId],
  );

  const go = async () => {
    const url = normalize(address);
    if (!url) return;
    setBusy(true);
    try {
      await browserNavigate(sessionId, url);
      setAddress(url);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="browser-panel">
      <div className="browser-bar">
        <input
          value={address}
          onChange={(e) => setAddress(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void go();
          }}
          placeholder="输入地址，回车打开"
          spellCheck={false}
        />
        <button onClick={() => void go()} disabled={busy || !address.trim()}>
          {busy ? "打开中…" : "打开"}
        </button>
        <button className="ghost" onClick={onClose} title="关闭面板">
          ✕
        </button>
      </div>

      <div
        className="browser-view"
        ref={viewRef}
        // 键盘要先能聚焦到这里。没有 tabIndex 的话 div 收不到 keydown，
        // 表现是"点了输入框但打不了字"。
        tabIndex={0}
        onClick={(e) => {
          const p = toPage(e);
          if (p) send({ kind: "click", ...p, button: "left" });
          viewRef.current?.focus();
        }}
        onMouseMove={(e) => {
          const p = toPage(e);
          if (p) send({ kind: "move", ...p });
        }}
        onWheel={(e) => {
          const p = toPage(e as unknown as React.MouseEvent);
          if (p) send({ kind: "scroll", ...p, deltaY: e.deltaY });
        }}
        onKeyDown={(e) => {
          // 单字符走 insertText —— 中文、emoji 没有键码，逐字符发不出去。
          // 功能键走 keyDown/keyUp，否则回车不提交、退格不删字符。
          if (e.key.length === 1 && !e.metaKey && !e.ctrlKey) {
            e.preventDefault();
            send({ kind: "text", text: e.key });
          } else if (FUNCTION_KEYS.has(e.key)) {
            e.preventDefault();
            send({ kind: "key", key: e.key });
          }
        }}
      >
        {frame ? (
          <img
            src={`data:image/jpeg;base64,${frame.data}`}
            alt=""
            draggable={false}
          />
        ) : (
          <div className="browser-empty">
            <p className="hint">浏览器启动中…</p>
          </div>
        )}
      </div>
    </div>
  );
}

/** 交给宿主处理的功能键。列表外的组合键留给系统。 */
const FUNCTION_KEYS = new Set([
  "Enter",
  "Backspace",
  "Tab",
  "Escape",
  "ArrowLeft",
  "ArrowUp",
  "ArrowRight",
  "ArrowDown",
  "Delete",
  "Home",
  "End",
  "PageUp",
  "PageDown",
]);

/**
 * 补全用户输入的地址。
 *
 * 直接把 `example.com` 传下去的话，CDP 会当成相对路径而导航到一个不存在
 * 的地方，页面白屏但不报错。
 */
function normalize(raw: string): string | null {
  const s = raw.trim();
  if (!s) return null;
  if (/^[a-z][a-z0-9+.-]*:/i.test(s)) return s;
  return `https://${s}`;
}
