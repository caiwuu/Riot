/**
 * 模型上下文窗口的档位、显示和换算。
 *
 * 用户填的是**窗口**（模型文档上查得到的客观数字），真正决定何时压缩的
 * 阈值由窗口减去两笔预留算出来。换算在宿主侧（`riot-kernel/src/config.rs`
 * 的 `compact_threshold_for_window`）执行，这里的同名实现只用来在界面上
 * 预览"填这个窗口会在哪儿触发压缩"。
 *
 * `[约束]` 下面三个常量和公式必须和 Rust 侧逐字对应。漂移了不会报错，
 * 表现是设置页写着"约 167k 触发"而实际在别处触发 —— 用户照着界面调参
 * 会一直调不准，且没有任何线索指向这里。
 */

/** 单次回复要留出的空间上限。见 Rust 侧 `OUTPUT_RESERVE_CAP`。 */
const OUTPUT_RESERVE_CAP = 20_000;
/** 阈值到窗口上限之间的缓冲。见 Rust 侧 `COMPACT_BUFFER`。 */
const COMPACT_BUFFER = 13_000;

/** 见 Rust 侧 `MIN_COMPACT_THRESHOLD` / `MAX_COMPACT_THRESHOLD`。 */
export const MIN_COMPACT_THRESHOLD = 8_000;
export const MAX_COMPACT_THRESHOLD = 1_000_000;
/** 没填窗口的模型走这个数。见 Rust 侧 `default_compact_threshold_tokens`。 */
export const DEFAULT_COMPACT_THRESHOLD = 100_000;

/** 见 Rust 侧 `MIN_CONTEXT_WINDOW` / `MAX_CONTEXT_WINDOW`。 */
export const MIN_CONTEXT_WINDOW = 8_000;
export const MAX_CONTEXT_WINDOW = 10_000_000;

/**
 * 选择器里列出的窗口档位。
 *
 * 只放两档。这个菜单是"临时换个跑法"的地方，不是模型参数表 —— 每多一项，
 * 真正想选的那个就更难挑。其余尺寸（128k、256k 之类）在「设置 → 服务方 →
 * 模型」里填精确值，填过的值也会被带进这个菜单，不会因为不在档位里就丢。
 */
export const WINDOW_PRESETS = [300_000, 1_000_000] as const;

/** `128000` → `128K`，`1000000` → `1M`。整数不拖 `.0`。 */
export function fmtTokens(n: number): string {
  if (n >= 1_000_000) {
    const m = n / 1_000_000;
    return `${Number.isInteger(m) ? m : m.toFixed(1)}M`;
  }
  if (n >= 1_000) {
    const k = n / 1_000;
    return `${Number.isInteger(k) ? k : k.toFixed(1)}K`;
  }
  return String(n);
}

/**
 * 从窗口推压缩阈值。和 Rust 侧同一个公式（见文件头的约束）。
 *
 * `maxOutput` 是这个模型配的最大输出 token，没配就按预留上限算。
 */
export function compactThresholdForWindow(window: number, maxOutput?: number): number {
  const reserve = Math.min(maxOutput ?? OUTPUT_RESERVE_CAP, OUTPUT_RESERVE_CAP);
  const derived = Math.max(window - reserve - COMPACT_BUFFER, Math.floor(window / 2));
  return Math.min(Math.max(derived, MIN_COMPACT_THRESHOLD), MAX_COMPACT_THRESHOLD);
}
