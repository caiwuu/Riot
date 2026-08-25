import { type KeyboardEvent, useEffect, useRef } from "react";

import type { ConfirmRequest } from "../ConfirmDialog";

/**
 * 设置各分区共用的小件。只放「两个以上分区都要」的东西 ——
 * 单一分区自己的辅助函数留在各自文件里。
 */

/** 分区向壳请求二次确认的回调形状。 */
export type AskConfirm = (req: ConfirmRequest) => void;

/**
 * 离开当前分区前的拦截：返回 null 放行，返回确认内容则先问一句。
 * 目前只有 MCP 的 JSON 视图用 —— 那里可能躺着用户刚粘贴、还没保存的
 * 一整段配置，Esc/点遮罩/切标签任何一条路都不该无声地丢掉它。
 */
export type LeaveGuard = () => Omit<ConfirmRequest, "action"> | null;

/** "失焦提交"的单行输入统一支持回车：Enter → blur，提交仍走 onBlur 一条路。 */
export function blurOnEnter(e: KeyboardEvent<HTMLInputElement>) {
  if (e.key === "Enter") e.currentTarget.blur();
}

/** 底部错误行。出现时滚进视野 —— 长页面里它可能在两屏之外，等于没报。 */
export function FormError({ text }: { text: string }) {
  const ref = useRef<HTMLParagraphElement>(null);
  useEffect(() => {
    ref.current?.scrollIntoView({ block: "nearest" });
  }, [text]);
  return (
    <p ref={ref} className="form-error">
      {text}
    </p>
  );
}

export function Toggle({ on, onChange, label }: { on: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <button className="toggle-row" onClick={() => onChange(!on)} role="switch" aria-checked={on}>
      <span className={on ? "toggle-track on" : "toggle-track"}>
        <span className="toggle-knob" />
      </span>
      <span className="toggle-label">{label}</span>
    </button>
  );
}
