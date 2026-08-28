import { useRef } from "react";

import type { Sampling } from "../bridge";
import {
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

function parseDraft(raw: string): number | null {
  const t = raw.trim();
  if (!t) return null;
  const n = Number(t);
  return Number.isFinite(n) ? n : null;
}

/**
 * 带范围的采样滑块。主交互是拖，右边留一个窄数字格给想敲精确值的人。
 *
 * 空值 = 不设置（继承 / 服务端默认）。滑块和数字格都停在继承值或 typical
 * 上，轨道是淡的 —— 拖一下或改数字才算写入。
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
  value: string;
  inherited?: number | null | undefined;
  hint?: boolean | undefined;
  onChange: (next: string) => void;
  /** 带上刚写下的字符串，避免父组件 setState 还没落地就拿旧草稿去提交。 */
  onCommit?: ((next: string) => void) | undefined;
}) {
  const set = parseDraft(value);
  const ghost = inherited ?? field.typical;
  const visual = set ?? ghost;
  const shown = set != null ? value : formatSamplingValue(field, ghost);
  const ratio = valueToRatio(field, visual);
  const pct = `${Math.round(ratio * 100)}%`;
  const sliderPos = Math.round(ratio * SLIDER_STEPS);
  const restoreLabel = formatSamplingValue(field, ghost);

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

  const clear = () => {
    pending.current = null;
    onChange("");
    onCommit?.("");
  };

  return (
    <div className={set == null ? "field-slider unset" : "field-slider"}>
      <div className="field-slider-head">
        <label>
          {field.label}
          {hint ? <HintTip>{field.hint}</HintTip> : null}
        </label>
        <FieldNumber
          className="field-slider-num"
          value={shown}
          onChange={(e) => onChange(e.target.value)}
          onBlur={() => {
            const n = parseDraft(value);
            if (n == null) {
              if (value.trim() !== "") onChange("");
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
            onClick={clear}
            aria-label={`恢复${field.label}为${restoreLabel}`}
            title={restoreLabel}
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
          aria-valuetext={formatSamplingValue(field, visual)}
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
      <div className="field-slider-bounds" aria-hidden>
        <span>{formatBound(field, field.min)}</span>
        <span>{formatBound(field, field.max)}</span>
      </div>
    </div>
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
  draft: Record<string, string>;
  inherited?: Sampling | undefined;
  hint?: boolean | undefined;
  onChange: (key: SamplingKey, value: string) => void;
  onCommit?: ((next: Record<string, string>) => void) | undefined;
}) {
  return (
    <div className="samp-grid">
      {SAMPLING_FIELDS.map((f) => (
        <FieldSlider
          key={f.key}
          field={f}
          value={draft[f.key] ?? ""}
          inherited={inherited?.[f.key]}
          hint={hint}
          onChange={(next) => onChange(f.key, next)}
          onCommit={onCommit ? (next) => onCommit({ ...draft, [f.key]: next }) : undefined}
        />
      ))}
    </div>
  );
}
