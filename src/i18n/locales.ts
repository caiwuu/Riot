/**
 * 界面语言的清单。
 *
 * 这里只放"有哪几种"和"叫什么"。词典在 `messages/`，查词在 `index.ts`。
 * 加一种语言的步骤：这里加一项 → `messages/` 下照 `en-US` 建一个目录 →
 * `messages/index.ts` 里挂上。少任何一步都过不了 typecheck。
 */

export const LOCALES = ["zh-CN", "en-US", "zh-TW", "ja-JP", "ko-KR"] as const;

export type Locale = (typeof LOCALES)[number];

/** 设置页里语言选项的显示名 —— 用各语言自己的写法，找母语时不用先读懂当前语言。 */
export const LOCALE_NAMES: Record<Locale, string> = {
  "zh-CN": "简体中文",
  "en-US": "English",
  "zh-TW": "繁體中文",
  "ja-JP": "日本語",
  "ko-KR": "한국어",
};

/** 没有一种能对上系统语言时用的那一种。 */
export const FALLBACK_LOCALE: Locale = "en-US";

export function isLocale(v: unknown): v is Locale {
  return typeof v === "string" && (LOCALES as readonly string[]).includes(v);
}

/**
 * 把一条 BCP 47 标签（`navigator.languages` 里的那种）对到我们支持的语言。
 *
 * 中文要看地区/文字：`zh-TW`、`zh-HK`、`zh-MO`、`zh-Hant-*` 归繁体，其余
 * 归简体。别的语言只看主语言子标签 —— `en-GB` 也是 English，`ja` 就是日语。
 */
export function matchLocale(tag: string): Locale | null {
  const parts = tag.toLowerCase().split(/[-_]/);
  const lang = parts[0];
  if (lang === "zh") {
    const rest = parts.slice(1);
    const hant = rest.includes("hant") || rest.includes("tw") || rest.includes("hk") || rest.includes("mo");
    return hant ? "zh-TW" : "zh-CN";
  }
  if (lang === "en") return "en-US";
  if (lang === "ja") return "ja-JP";
  if (lang === "ko") return "ko-KR";
  return null;
}

/** 按系统语言列表挑一种。没有能对上的回落到 [`FALLBACK_LOCALE`]。 */
export function detectLocale(preferred: readonly string[]): Locale {
  for (const tag of preferred) {
    const hit = matchLocale(tag);
    if (hit) return hit;
  }
  return FALLBACK_LOCALE;
}
