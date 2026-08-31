/**
 * 时间与日期时间选择器（定时任务详情用）。
 *
 * 不用原生 `<input type=time>` / `<select>` / `<input type=date>`：
 * WKWebView 里那套是系统皮肤（灰胶囊、系统弹层、聚焦光环），和深色
 * 界面不是一家人。触发框长着 FieldSelect 的样子，弹层用同一个壳
 * （.field-select-menu），定位思路也一致：fixed 贴触发框，下方不够翻上去。
 *
 * - [`TimePicker`]：HH:MM。两列（时 / 分），点分即选完关闭。
 * - [`DateTimePicker`]：YYYY-MM-DD HH:MM。月历 + 时 / 分两列；
 *   过去的日期不可选（定时任务没有过去时）。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import { Chevron } from "./Chevron";

const pad2 = (n: number) => String(n).padStart(2, "0");

/* ── 共享：弹层开合与定位 ───────────────────── */

function usePop(popW: number, popH: number) {
  const btnRef = useRef<HTMLButtonElement>(null);
  const popRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [box, setBox] = useState<React.CSSProperties | null>(null);

  const place = useCallback(() => {
    const el = btnRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const gap = 6;
    const below = window.innerHeight - r.bottom - gap - 8;
    const up = below < popH && r.top > below;
    // 行内控件靠右，弹层贴触发框右缘；出屏就往回收。
    const left = Math.max(8, Math.min(r.right - popW, window.innerWidth - popW - 8));
    setBox({
      left,
      width: popW,
      ...(up
        ? { bottom: window.innerHeight - r.top + gap }
        : { top: r.bottom + gap }),
    });
  }, [popW, popH]);

  useEffect(() => {
    if (!open) return;
    place();
    const onScroll = () => place();
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (btnRef.current?.contains(t) || popRef.current?.contains(t)) return;
      setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      e.preventDefault();
      setOpen(false);
      btnRef.current?.focus();
    };
    window.addEventListener("resize", onScroll);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("resize", onScroll);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open, place]);

  return { btnRef, popRef, open, setOpen, box };
}

/** 一列可滚的刻度。打开时选中项滚到视野中间。 */
function ScrollCol({
  caption,
  items,
  picked,
  onPick,
}: {
  caption: string;
  items: string[];
  picked: string;
  onPick: (v: string) => void;
}) {
  const listRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    listRef.current
      ?.querySelector(".tp-item.picked")
      ?.scrollIntoView({ block: "center" });
  }, []);
  return (
    <div className="tp-col">
      <div className="tp-col-caption">{caption}</div>
      <div className="tp-col-list" ref={listRef}>
        {items.map((it) => (
          <button
            key={it}
            type="button"
            className={it === picked ? "tp-item picked" : "tp-item"}
            onClick={() => onPick(it)}
          >
            {it}
          </button>
        ))}
      </div>
    </div>
  );
}

const HOURS = Array.from({ length: 24 }, (_, i) => pad2(i));

/** 分钟刻度：5 分钟一格；当前值不在格点上（模型定的 08:37）就插进去。 */
function minuteItems(cur: number): string[] {
  const base = Array.from({ length: 12 }, (_, i) => i * 5);
  if (!base.includes(cur)) base.push(cur);
  return base.sort((a, b) => a - b).map(pad2);
}

/* ── TimePicker：HH:MM ──────────────────────── */

export function TimePicker({
  value,
  onChange,
  className,
}: {
  /** "HH:MM"。 */
  value: string;
  onChange: (v: string) => void;
  className?: string;
}) {
  const { btnRef, popRef, open, setOpen, box } = usePop(148, 300);
  const [hh = "09", mm = "00"] = value.split(":");

  return (
    <>
      <button
        ref={btnRef}
        type="button"
        className={className ? `field-select ${className}` : "field-select"}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        <span className="field-select-label">{value || "选择时间"}</span>
        <Chevron down open={open} />
      </button>
      {open && box
        ? createPortal(
            <div ref={popRef} className="field-select-menu tp-pop" style={box}>
              <ScrollCol
                caption="时"
                items={HOURS}
                picked={hh}
                onPick={(h) => onChange(`${h}:${mm}`)}
              />
              <ScrollCol
                caption="分"
                items={minuteItems(Number(mm) || 0)}
                picked={mm}
                onPick={(m) => {
                  onChange(`${hh}:${m}`);
                  setOpen(false);
                }}
              />
            </div>,
            document.body,
          )
        : null}
    </>
  );
}

/* ── DateTimePicker：YYYY-MM-DD HH:MM ───────── */

interface Dt {
  y: number;
  m: number; // 1-12
  d: number;
  hh: number;
  mm: number;
}

function parseDt(s: string): Dt | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})[ T](\d{2}):(\d{2})/.exec(s.trim());
  if (!m) return null;
  return {
    y: Number(m[1]),
    m: Number(m[2]),
    d: Number(m[3]),
    hh: Number(m[4]),
    mm: Number(m[5]),
  };
}

function fmtDt(v: Dt): string {
  return `${v.y}-${pad2(v.m)}-${pad2(v.d)} ${pad2(v.hh)}:${pad2(v.mm)}`;
}

/** 默认基准：明天 09:00 —— 空值打开时从一个合理的未来时刻开始。 */
function tomorrowNine(): Dt {
  const t = new Date();
  t.setDate(t.getDate() + 1);
  return { y: t.getFullYear(), m: t.getMonth() + 1, d: t.getDate(), hh: 9, mm: 0 };
}

const DOW = ["一", "二", "三", "四", "五", "六", "日"];

export function DateTimePicker({
  value,
  onChange,
  className,
}: {
  /** "YYYY-MM-DD HH:MM"；空或读不懂就当没选。 */
  value: string;
  onChange: (v: string) => void;
  className?: string;
}) {
  const { btnRef, popRef, open, setOpen, box } = usePop(332, 320);
  const sel = parseDt(value);
  const base = sel ?? tomorrowNine();
  /** 日历正在看的年月。打开跟着选中值走。 */
  const [view, setView] = useState({ y: base.y, m: base.m });
  useEffect(() => {
    if (open) setView({ y: base.y, m: base.m });
    // eslint-disable-next-line react-hooks/exhaustive-deps -- 只在开合时回位
  }, [open]);

  const today = new Date();
  const todayKey = today.getFullYear() * 10_000 + (today.getMonth() + 1) * 100 + today.getDate();

  // 月网格：周一起始。
  const firstCol = (new Date(view.y, view.m - 1, 1).getDay() + 6) % 7;
  const daysInMonth = new Date(view.y, view.m, 0).getDate();

  const shiftMonth = (d: number) => {
    setView((v) => {
      const total = v.y * 12 + (v.m - 1) + d;
      return { y: Math.floor(total / 12), m: (total % 12) + 1 };
    });
  };

  return (
    <>
      <button
        ref={btnRef}
        type="button"
        className={className ? `field-select ${className}` : "field-select"}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        <span className="field-select-label">{sel ? fmtDt(sel) : "选择时刻"}</span>
        <Chevron down open={open} />
      </button>
      {open && box
        ? createPortal(
            <div ref={popRef} className="field-select-menu tp-pop dtp" style={box}>
              <div className="dtp-cal">
                <div className="dtp-head">
                  <button type="button" className="dtp-nav" onClick={() => shiftMonth(-1)} aria-label="上个月">
                    ‹
                  </button>
                  <span className="dtp-title">
                    {view.y} 年 {view.m} 月
                  </span>
                  <button type="button" className="dtp-nav" onClick={() => shiftMonth(1)} aria-label="下个月">
                    ›
                  </button>
                </div>
                <div className="dtp-grid">
                  {DOW.map((w) => (
                    <span key={w} className="dtp-dow">
                      {w}
                    </span>
                  ))}
                  {Array.from({ length: firstCol }, (_, i) => (
                    <span key={`pad${i}`} />
                  ))}
                  {Array.from({ length: daysInMonth }, (_, i) => {
                    const d = i + 1;
                    const key = view.y * 10_000 + view.m * 100 + d;
                    const isPast = key < todayKey;
                    const isPicked =
                      sel !== null && sel.y === view.y && sel.m === view.m && sel.d === d;
                    return (
                      <button
                        key={d}
                        type="button"
                        disabled={isPast}
                        className={
                          "dtp-day" +
                          (isPicked ? " picked" : "") +
                          (key === todayKey ? " today" : "")
                        }
                        onClick={() =>
                          onChange(fmtDt({ ...base, y: view.y, m: view.m, d }))
                        }
                      >
                        {d}
                      </button>
                    );
                  })}
                </div>
              </div>
              <ScrollCol
                caption="时"
                items={HOURS}
                picked={pad2(base.hh)}
                onPick={(h) => onChange(fmtDt({ ...base, hh: Number(h) }))}
              />
              <ScrollCol
                caption="分"
                items={minuteItems(base.mm)}
                picked={pad2(base.mm)}
                onPick={(m) => {
                  onChange(fmtDt({ ...base, mm: Number(m) }));
                  setOpen(false);
                }}
              />
            </div>,
            document.body,
          )
        : null}
    </>
  );
}
