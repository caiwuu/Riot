import type { InputHTMLAttributes } from "react";

/**
 * 数字框。不用 `type="number"`：系统自带的步进箭头在 macOS / Windows
 * 上完全两套皮肤，WKWebView 里还经常和旁边的文字框对不齐。
 * 校验和夹紧仍由调用方在 blur 时做。
 */
export function FieldNumber(props: Omit<InputHTMLAttributes<HTMLInputElement>, "type">) {
  return <input type="text" inputMode="decimal" autoComplete="off" spellCheck={false} {...props} />;
}
