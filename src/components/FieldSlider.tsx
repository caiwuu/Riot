import { useRef } from "react";

import type { Sampling } from "../bridge";
import {
  type SamplingDraft,
  type SamplingField,
  type SamplingKey,
  SAMPLING_FIELDS,
  formatSamplingValue,
  ratioToValue,
  snapSamplingValue,
  valueToRatio,
} from "../lib/sampling";
import { FieldNumber } from "./FieldNumber";
import { HintTip } from "./HintTip";

const SLIDER_STEPS = 1000;

/** 「不发这个参数」在界面上的说法。三处设置共用一个词。 */
const DEFAULT_LABEL = "模型默认";

function parseDraft(raw: string): number | null {
  const t = raw.trim();
  if (!t) return null;
  const n = Number(t);
  return Number.isFinite(n) ? n : null;
}

/**
 * 带范围的采样滑块。主交互是拖，右边留一个窄数字格给想敲精确值的人。
 *
 * 三态，靠滑块下方那行文字说清自己是哪个：
 *
 * - **自定义** —— 本层填了数字。轨道实色，右边有 × 清掉。
 * - **继承 x** —— 本层没填，用上层的 x。轨道淡色，停在 x 上。
 * - **模型默认** —— 这个参数一个都不发，让模型用它自己的默认值。
 *
 * `[取舍]` 后两个必须分开，哪怕上层没设值时它们表现一样。
 *
 * 只有两态时"没填"兼着"继承"和"不发"，于是服务方一设 temperature，
 * 这家的推理模型就再也回不到"别发"—— 而那正是它需要的。顶层同理：
 * 数字格里显示一个继承来的 1，和用户自己设了 1 长得一模一样。
 *
 * 上层没有值可继承时（顶层的服务方面板就是这样），"继承"和"模型默认"
 * 是同一件事，那行文字就固定说「模型默认」，不给切换 —— 那里没有第二个选择。
 */
export function FieldSlider({
  field,
  value,
  inherited,
  hint,
  onChange,
  onCommit,
}: {
  field: SamplingField;
  /** `""` = 继承，`null` = 模型默认，其它 = 编辑中的数字文本。 */
  value: string | null;
  /** 上层这个字段的三态值。`undefined`/`null` 都表示上层最终也不发。 */
  inherited?: number | null | undefined;
  hint?: boolean | undefined;
  onChange: (next: string | null) => void;
  /** 带上刚写下的值，避免父组件 setState 还没落地就拿旧草稿去提交。 */
  onCommit?: ((next: string | null) => void) | undefined;
}) {
  const off = value === null;
  const set = off ? null : parseDraft(value);
  /** 上层最终会发的值。没有就是 null —— 此时继承和模型默认合流。 */
  const base = inherited ?? null;
  const inheritLabel = base == null ? null : formatSamplingValue(field, base);
  // 模型默认态不能再指向继承值：那个值已经被这一层否掉了。
  const visual = set ?? (off ? field.typical : (base ?? field.typical));
  const ratio = valueToRatio(field, visual);
  const pct = `${Math.round(ratio * 100)}%`;
  const sliderPos = Math.round(ratio * SLIDER_STEPS);
  // 滑块念出来是什么。没设值时光报数字，会把停在继承值上的位置念成"用户设了这个"。
  const spoken =
    set != null
      ? formatSamplingValue(field, visual)
      : off || inheritLabel == null
        ? DEFAULT_LABEL
        : `继承 ${inheritLabel}`;

  const pending = useRef<string | null>(null);

  const write = (n: number) => {
    const next = formatSamplingValue(field, n);
    pending.current = next;
    onChange(next);
  };

  const flush = () => {
    if (pending.current == null) return;
    const next = pending.current;
    pending.current = null;
    onCommit?.(next);
  };

  const put = (next: string | null) => {
    pending.current = null;
    onChange(next);
    onCommit?.(next);
  };

  return (
    <div className={set == null ? "field-slider unset" : "field-slider"}>
      <div className="field-slider-head">
        <label>
          {field.label}
          {hint ? <HintTip>{field.hint}</HintTip> : null}
        </label>
        {/* 继承值走 placeholder 而不是 value：框里留空，用户点进去直接敲。
            填成真值的话，想设的数恰好等于继承值时敲不出变化 —— 而
            "0."、"-" 这类中间态也会被当成非法值抹掉。 */}
        <FieldNumber
          className="field-slider-num"
          value={value ?? ""}
          placeholder={off || inheritLabel == null ? DEFAULT_LABEL : inheritLabel}
          onChange={(e) => onChange(e.target.value)}
          onBlur={() => {
            if (value === null) return;
            const n = parseDraft(value);
            if (n == null) {
              if (value !== "") onChange("");
              onCommit?.("");
              return;
            }
            const snapped = formatSamplingValue(field, snapSamplingValue(field, n));
            if (snapped !== value) onChange(snapped);
            onCommit?.(snapped);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") (e.target as HTMLInputElement).blur();
          }}
          aria-label={field.label}
        />
        {set != null ? (
          <button
            type="button"
            className="field-slider-clear"
            onClick={() => put("")}
            aria-label={`清除${field.label}，回到${inheritLabel ?? DEFAULT_LABEL}`}
            title={inheritLabel ?? DEFAULT_LABEL}
          >
            ×
          </button>
        ) : (
          <span className="field-slider-clear-slot" aria-hidden />
        )}
      </div>
      <div className="field-slider-track">
        <div className="field-slider-rail" aria-hidden>
          <div className="field-slider-fill" style={{ width: pct }} />
        </div>
        <input
          type="range"
          className="field-slider-range"
          min={0}
          max={SLIDER_STEPS}
          step={1}
          value={sliderPos}
          aria-valuemin={field.min}
          aria-valuemax={field.max}
          aria-valuenow={visual}
          aria-valuetext={spoken}
          aria-label={field.label}
          onInput={(e) => {
            const t = Number((e.target as HTMLInputElement).value) / SLIDER_STEPS;
            write(ratioToValue(field, t));
          }}
          onPointerUp={flush}
          onKeyUp={(e) => {
            if (
              e.key === "ArrowLeft" ||
              e.key === "ArrowRight" ||
              e.key === "ArrowUp" ||
              e.key === "ArrowDown" ||
              e.key === "Home" ||
              e.key === "End"
            ) {
              flush();
            }
          }}
        />
      </div>
      <div className="field-slider-bounds">
        <span aria-hidden>{formatBound(field, field.min)}</span>
        <StateTag
          field={field}
          set={set != null}
          off={off}
          inheritLabel={inheritLabel}
          onPick={put}
        />
        <span aria-hidden>{formatBound(field, field.max)}</span>
      </div>
    </div>
  );
}

/**
 * 滑块下方那行状态字，兼「继承 ⇄ 模型默认」的开关。
 *
 * 位置固定、三态都有词：这个格子里现在到底会发什么，不用推。
 */
function StateTag({
  field,
  set,
  off,
  inheritLabel,
  onPick,
}: {
  field: SamplingField;
  set: boolean;
  off: boolean;
  /** 上层的值，`null` = 上层也不发（此时没得选）。 */
  inheritLabel: string | null;
  onPick: (next: string | null) => void;
}) {
  if (set) return <span className="field-slider-state">自定义</span>;
  // 顶层没有第二个选择：这是缺省状态，不是谁选出来的，别比刻度更抢眼。
  if (inheritLabel == null) {
    return <span className="field-slider-state">{DEFAULT_LABEL}</span>;
  }
  return off ? (
    <button
      type="button"
      className="field-slider-state on"
      onClick={() => onPick("")}
      aria-label={`${field.label} 当前是${DEFAULT_LABEL}，点击改回继承 ${inheritLabel}`}
      title={`改回继承（${inheritLabel}）`}
    >
      {DEFAULT_LABEL}
    </button>
  ) : (
    <button
      type="button"
      className="field-slider-state"
      onClick={() => onPick(null)}
      aria-label={`${field.label} 当前继承 ${inheritLabel}，点击改用${DEFAULT_LABEL}`}
      title={`改用${DEFAULT_LABEL}：这一项不发给服务方，由模型自己定`}
    >
      继承 {inheritLabel}
    </button>
  );
}

function formatBound(field: SamplingField, n: number): string {
  if (field.integer && n >= 1000 && n % 1000 === 0) return `${n / 1000}k`;
  return formatSamplingValue(field, n);
}

/** 四个采样滑块的两列网格。三处设置共用同一套范围。 */
export function SamplingSliders({
  draft,
  inherited,
  hint,
  onChange,
  onCommit,
}: {
  draft: SamplingDraft;
  inherited?: Sampling | undefined;
  hint?: boolean | undefined;
  onChange: (key: SamplingKey, value: string | null) => void;
  onCommit?: ((next: SamplingDraft) => void) | undefined;
}) {
  return (
    <div className="samp-grid">
      {SAMPLING_FIELDS.map((f) => {
        // 草稿里的 null 是「模型默认」，不能被 ?? 顺手抹成继承。
        const v = draft[f.key];
        return (
          <FieldSlider
            key={f.key}
            field={f}
            value={v === undefined ? "" : v}
            inherited={inherited?.[f.key]}
            hint={hint}
            onChange={(next) => onChange(f.key, next)}
            onCommit={onCommit ? (next) => onCommit({ ...draft, [f.key]: next }) : undefined}
          />
        );
      })}
    </div>
  );
}
