import type { Sampling } from "../bridge";

/**
 * 采样参数的范围和展示。三处 UI（会话覆盖、服务方默认、模型覆盖）共用，
 * 免得一边改了范围另一边还是裸输入框。
 *
 * `[约束]` 这里的上下限是**交互范围**，不是协议校验。服务端仍可能拒
 * 掉组合（比如思考模式 + temperature）。超范围的存量值只夹紧滑块位置，
 * 不在打开面板时偷偷改掉已保存的数字。
 */
export type SamplingKey = keyof Sampling;

export type SamplingScale = "linear" | "log";

export interface SamplingField {
  key: SamplingKey;
  label: string;
  hint: string;
  min: number;
  max: number;
  step: number;
  integer?: boolean;
  scale?: SamplingScale;
  /** 未设置且没有继承值时，滑块停在这儿。不写入。 */
  typical: number;
}

export const SAMPLING_FIELDS: SamplingField[] = [
  {
    key: "temperature",
    label: "temperature",
    hint: "0–2。越高越发散。",
    min: 0,
    max: 2,
    step: 0.05,
    typical: 1,
  },
  {
    key: "topP",
    label: "top_p",
    hint: "0–1。核采样。一般不与 temperature 同调。",
    min: 0,
    max: 1,
    step: 0.05,
    typical: 1,
  },
  {
    key: "topK",
    label: "top_k",
    hint: "仅 Anthropic 协议发送。",
    min: 1,
    max: 100,
    step: 1,
    integer: true,
    typical: 40,
  },
  {
    key: "maxOutputTokens",
    label: "max tokens",
    hint: "单次回复的输出上限。",
    min: 256,
    max: 128_000,
    step: 256,
    integer: true,
    scale: "log",
    typical: 4096,
  },
];

export function clamp(n: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, n));
}

/** 把任意数字收进该字段的步长和范围。 */
export function snapSamplingValue(field: SamplingField, n: number): number {
  const raw = field.integer ? Math.round(n) : n;
  const snapped = Math.round(raw / field.step) * field.step;
  const v = field.integer
    ? Math.round(snapped)
    : Number(snapped.toFixed(field.step >= 0.1 ? 1 : 2));
  return clamp(v, field.min, field.max);
}

export function formatSamplingValue(field: SamplingField, n: number): string {
  if (field.integer) return String(Math.round(n));
  const digits = field.step >= 0.1 ? 1 : 2;
  return n.toFixed(digits).replace(/\.?0+$/, "") || "0";
}

/** 值 → 滑条 0–1。超出范围的存量值夹到两端，不改原值。 */
export function valueToRatio(field: SamplingField, value: number): number {
  const v = clamp(value, field.min, field.max);
  if (field.scale === "log") {
    return Math.log(v / field.min) / Math.log(field.max / field.min);
  }
  return (v - field.min) / (field.max - field.min);
}

export function ratioToValue(field: SamplingField, ratio: number): number {
  const t = clamp(ratio, 0, 1);
  const raw =
    field.scale === "log"
      ? field.min * (field.max / field.min) ** t
      : field.min + t * (field.max - field.min);
  return snapSamplingValue(field, raw);
}

export function samplingDraft(s: Sampling): Record<string, string> {
  return Object.fromEntries(
    SAMPLING_FIELDS.map((f) => [f.key, s[f.key] != null ? String(s[f.key]) : ""]),
  );
}

/** 空/非法 = null（不设置 / 继承）。不夹范围：存量超范围值原样留下，夹紧只发生在拖滑块或改数字的那一刻。 */
export function parseSampling(draft: Record<string, string>): Sampling {
  const num = (key: SamplingKey, integer?: boolean) => {
    const t = (draft[key] ?? "").trim();
    if (!t) return null;
    const v = Number(t);
    if (!Number.isFinite(v)) return null;
    return integer ? Math.round(v) : v;
  };
  return {
    temperature: num("temperature"),
    topP: num("topP"),
    topK: num("topK", true),
    maxOutputTokens: num("maxOutputTokens", true),
  };
}

export function sameSampling(a: Sampling, b: Sampling): boolean {
  return SAMPLING_FIELDS.every((f) => (a[f.key] ?? null) === (b[f.key] ?? null));
}
