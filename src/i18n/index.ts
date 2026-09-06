/**
 * 界面文案的查词层。
 *
 * `[取舍]` 自己写而不是引 i18next：桌面应用不需要按需加载语言包、不需要
 * 服务端渲染，中/英/日/韩也没有复杂的复数与性别规则 —— 那套库里九成功能
 * 用不上，剩下一成就是这个文件。哪天要接翻译平台再换，`t(key, vars)` 的
 * 调用形状是一样的。
 *
 * `[约束]` 组件里的文案一律从这里取，不许写死中文 —— eslint 的
 * `no-restricted-syntax` 拦 `src/` 里所有含汉字的字面量（词典目录除外）。
 * 词典以 `messages/zh-CN` 为基准定义键集，其余语言由 TypeScript 逐键对齐：
 * 漏一条就是编译错误，而不是运行时回落成另一种语言。
 *
 * 语言的选择存 localStorage：它是纯界面偏好，和布局尺寸一样不值得进宿主
 * 配置；网页版（远程访问）也因此能各自记住自己的语言。没存过就跟系统走。
 */

import { Fragment, type ReactNode, createElement, useSyncExternalStore } from "react";

import { type Locale, FALLBACK_LOCALE, detectLocale, isLocale } from "./locales";
import { MESSAGES, type MessageKey } from "./messages";

export { LOCALES, LOCALE_NAMES, type Locale, isLocale } from "./locales";
export type { MessageKey } from "./messages";

const LS_KEY = "riot.locale";

/** 用户的显式选择；`null` = 跟随系统。 */
export type LocaleChoice = Locale | null;

function readChoice(): LocaleChoice {
  if (typeof localStorage === "undefined") return null;
  const v = localStorage.getItem(LS_KEY);
  return isLocale(v) ? v : null;
}

function systemLocale(): Locale {
  if (typeof navigator === "undefined") return FALLBACK_LOCALE;
  return detectLocale(navigator.languages?.length ? navigator.languages : [navigator.language]);
}

let choice: LocaleChoice = readChoice();
let current: Locale = choice ?? systemLocale();
const listeners = new Set<() => void>();

function applyLang() {
  if (typeof document !== "undefined") document.documentElement.lang = current;
}
applyLang();

/** 此刻生效的语言。非组件代码（hooks、lib、bridge）直接用。 */
export function getLocale(): Locale {
  return current;
}

/** 用户的显式选择（设置页显示用）。`null` = 跟随系统。 */
export function getLocaleChoice(): LocaleChoice {
  return choice;
}

/**
 * 改语言。`null` 表示回到跟随系统。立即生效：订阅了 [`useLocale`] 的
 * 组件全部重渲染，不重挂载 —— 输入框里的草稿、正在流式生成的回复都保住。
 *
 * 选择变了就通知，哪怕生效语言没变（系统就是简体中文时，"跟随系统"和
 * "简体中文"来回切）：[`useLocaleChoice`] 和 [`useLocale`] 共用一组订阅者，
 * 不通知的话设置页的下拉会停在旧项。
 */
export function setLocaleChoice(next: LocaleChoice): void {
  const choiceChanged = next !== choice;
  choice = next;
  if (typeof localStorage !== "undefined") {
    if (next) localStorage.setItem(LS_KEY, next);
    else localStorage.removeItem(LS_KEY);
  }
  const resolved = next ?? systemLocale();
  const localeChanged = resolved !== current;
  if (localeChanged) {
    current = resolved;
    applyLang();
  }
  if (choiceChanged || localeChanged) {
    for (const notify of listeners) notify();
  }
}

function subscribe(notify: () => void): () => void {
  listeners.add(notify);
  return () => {
    listeners.delete(notify);
  };
}

/**
 * 组件里订阅当前语言。凡是渲染文案的组件都要调一次（直接调它或调
 * [`useT`]），否则切语言时那块界面停在旧语言上，直到下一次别的原因重渲染。
 */
export function useLocale(): Locale {
  return useSyncExternalStore(subscribe, getLocale, getLocale);
}

/** 语言的选择项（含"跟随系统"），给设置页。 */
export function useLocaleChoice(): LocaleChoice {
  return useSyncExternalStore(subscribe, getLocaleChoice, getLocaleChoice);
}

export type Vars = Record<string, string | number>;

/** 空对象。参数为空时不再逐条 `Object.entries`。 */
const NO_VARS: Vars = {};

function interpolate(template: string, vars: Vars): string {
  if (vars === NO_VARS) return template;
  return template.replace(/\{(\w+)\}/g, (m, name: string) => {
    const v = vars[name];
    return v === undefined ? m : String(v);
  });
}

function lookup(key: string, locale: Locale): string | undefined {
  const dict = MESSAGES[locale] as Record<string, string>;
  return dict[key] ?? (MESSAGES[FALLBACK_LOCALE] as Record<string, string>)[key];
}

/**
 * 查一条文案。`{name}` 占位符用 `vars` 填。
 *
 * 键在词典里一定存在（类型保证），所以这里不做"找不到就显示键名"那类
 * 兜底 —— 兜底只会让漏翻译在界面上悄悄过去。
 */
export function t(key: MessageKey, vars: Vars = NO_VARS): string {
  return interpolate(lookup(key, current) ?? key, vars);
}

/**
 * 带数量的文案。按当前语言的复数规则挑变体：先找 `key#one` / `key#other`
 * 这类带形式后缀的键，没有就用 `key` 本身。`{count}` 自动填进去。
 *
 * 中日韩没有复数变化，词典里只写 `key`；英文在需要时多写一条 `key#one`。
 * 词典类型允许这类后缀键作为额外项存在（见 `messages/index.ts`）。
 */
export function tn(key: MessageKey, count: number, vars: Vars = NO_VARS): string {
  const form = pluralRules(current).select(count);
  const template = lookup(`${key}#${form}`, current) ?? lookup(key, current) ?? key;
  return interpolate(template, { ...vars, count });
}

/**
 * 占位符要填进 React 节点（`<code>`、`<a>`、按钮）时用这个。返回节点数组，
 * 直接放进 JSX。词序由各语言的模板自己定，代码里不拼接。
 */
export function tx(key: MessageKey, vars: Record<string, ReactNode>): ReactNode[] {
  const template = lookup(key, current) ?? key;
  const out: ReactNode[] = [];
  const re = /\{(\w+)\}/g;
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(template))) {
    if (m.index > last) out.push(template.slice(last, m.index));
    const name = m[1] ?? "";
    const v = vars[name];
    out.push(v === undefined ? m[0] : createElement(Fragment, { key: `${name}${m.index}` }, v));
    last = m.index + m[0].length;
  }
  if (last < template.length) out.push(template.slice(last));
  return out;
}

const PLURAL_CACHE = new Map<Locale, Intl.PluralRules>();
function pluralRules(locale: Locale): Intl.PluralRules {
  let r = PLURAL_CACHE.get(locale);
  if (!r) {
    r = new Intl.PluralRules(locale);
    PLURAL_CACHE.set(locale, r);
  }
  return r;
}

/**
 * 组件用这个拿 `t`：它顺带订阅了语言，切换时组件会重渲染。
 * 返回的就是模块级的 `t`，不会因为重渲染产生新函数身份。
 */
export function useT(): { t: typeof t; tn: typeof tn; tx: typeof tx; locale: Locale } {
  const locale = useLocale();
  return { t, tn, tx, locale };
}

/**
 * 按当前语言的日期时间格式化器。同一份选项在同一语言下复用实例 ——
 * `Intl.DateTimeFormat` 的构造不便宜，对话流里每条消息都要格式化一次。
 */
const DTF_CACHE = new Map<string, Intl.DateTimeFormat>();
export function dateTimeFormat(options: Intl.DateTimeFormatOptions): Intl.DateTimeFormat {
  const key = `${current}|${JSON.stringify(options)}`;
  let f = DTF_CACHE.get(key);
  if (!f) {
    f = new Intl.DateTimeFormat(current, options);
    DTF_CACHE.set(key, f);
  }
  return f;
}

const NF_CACHE = new Map<string, Intl.NumberFormat>();
export function numberFormat(options: Intl.NumberFormatOptions = {}): Intl.NumberFormat {
  const key = `${current}|${JSON.stringify(options)}`;
  let f = NF_CACHE.get(key);
  if (!f) {
    f = new Intl.NumberFormat(current, options);
    NF_CACHE.set(key, f);
  }
  return f;
}
