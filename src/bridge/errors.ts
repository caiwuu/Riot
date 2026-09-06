/**
 * 宿主错误在前端的形态。
 *
 * 宿主（Tauri IPC 和远程 WebSocket 两条线）报错时给的是 riot-protocol 的
 * `UiError`：`{ key, args?, detail? }` —— 一个词典键、占位参数、一段不翻译
 * 的技术细节。这里把它包成 [`HostError`]，`message` 按当前界面语言查词。
 *
 * `[约束]` `toString()` 只回文案本身，不带 `Error:` 前缀 —— 调用方普遍用
 * `String(e)` 直接上屏。
 */

import { type MessageKey, t } from "../i18n";
import { MESSAGES } from "../i18n/messages";

/** 宿主/内核发来的一句话（riot-protocol `UiText` 的 JSON）。 */
export interface UiTextPayload {
  key: string;
  args?: Record<string, string>;
}

/** 宿主线上的错误载荷（riot-protocol `UiError` 的 JSON）。 */
export interface UiErrorPayload extends UiTextPayload {
  detail?: string | null;
}

/**
 * 把宿主/内核给的 `UiText` 按当前语言渲染成一句话。
 * 键不认识时退回键名 —— 这只会发生在 Rust 与词典脱节的时候，而那由
 * riot-protocol 的对齐测试拦着。
 */
export function renderUiText(p: UiTextPayload): string {
  return isKnownKey(p.key) ? t(p.key, p.args ?? {}) : p.key;
}

function isUiErrorPayload(v: unknown): v is UiErrorPayload {
  return (
    typeof v === "object" &&
    v !== null &&
    typeof (v as { key?: unknown }).key === "string" &&
    (v as { key: string }).key.length > 0
  );
}

/** 键在词典里有没有。宿主的键集由 Rust 侧测试对齐，这里只是最后一道保险。 */
function isKnownKey(key: string): key is MessageKey {
  return Object.prototype.hasOwnProperty.call(MESSAGES["zh-CN"], key);
}

export class HostError extends Error {
  readonly key: string;
  readonly args: Record<string, string>;
  readonly detail: string | null;

  constructor(payload: UiErrorPayload) {
    super(HostError.render(payload));
    this.name = "HostError";
    this.key = payload.key;
    this.args = payload.args ?? {};
    this.detail = payload.detail ?? null;
  }

  /** 是不是某个具体错误。比对 `String(e)` 的文案脆弱，按键判。 */
  is(key: MessageKey): boolean {
    return this.key === key;
  }

  override toString(): string {
    return this.message;
  }

  /**
   * 拼成一句话。模板自己写了 `{detail}` 的按模板；否则有细节就按
   * `errors.withDetail` 追加在后面。键不认识时只剩细节可给。
   */
  static render(p: UiErrorPayload): string {
    const detail = p.detail ?? "";
    if (!isKnownKey(p.key)) return detail || p.key;
    const vars = { ...(p.args ?? {}), detail };
    const text = t(p.key, vars);
    if (!detail || /\{detail\}/.test(MESSAGES["zh-CN"][p.key])) return text;
    return t("errors.withDetail", { text, detail });
  }
}

/**
 * 把宿主线上收到的任意拒绝值规整成前端能用的错误。
 *
 * - `UiError` JSON → [`HostError`]
 * - 旧式的一句话（字符串）→ 包成 `host.legacy`，原话当细节
 * - 已经是 `Error` 的原样返回
 */
export function toHostError(raw: unknown): unknown {
  if (isUiErrorPayload(raw)) return new HostError(raw);
  if (typeof raw === "string") return new HostError({ key: "host.legacy", detail: raw });
  return raw;
}

/** 结构里带着的 `UiError`（不是 throw 出来的）→ 一句话。带细节的按 `errors.withDetail` 拼。 */
export function renderUiError(p: UiErrorPayload): string {
  return HostError.render(p);
}

export function isHostError(e: unknown): e is HostError {
  return e instanceof HostError;
}

/**
 * 任何 catch 到的东西 → 一句给人看的话。
 * `Error` 取 `message`（[`HostError`] 的已经是译文），字符串原样，其余兜底。
 */
export function describeError(e: unknown): string {
  if (e instanceof Error) return e.message || t("errors.unknown");
  if (typeof e === "string") return e || t("errors.unknown");
  return t("errors.unknown");
}
