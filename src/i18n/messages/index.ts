/**
 * 词典总表。
 *
 * `zh-CN` 是基准：它定义键集（[`MessageKey`]），别的语言按它逐键对齐。
 * 每种语言一个目录，目录里按界面区域分文件（`common`、`settings`、
 * `composer`……），一个文件对应一片界面 —— 一千多条键塞进一个文件没法
 * 维护，也没法几个人同时改。
 *
 * 非基准语言的每个分文件都写 `satisfies Dict<typeof zh模块>`：漏键、
 * 多键都是编译错误，见 [`Dict`]。
 */

import type { Locale } from "../locales";
import enUS from "./en-US";
import jaJP from "./ja-JP";
import koKR from "./ko-KR";
import zhCN from "./zh-CN";
import zhTW from "./zh-TW";

export type MessageKey = keyof typeof zhCN;

/**
 * 非基准语言分文件的类型：基准的每个键都得有；另外允许 `key#one` 这类
 * 复数变体作为额外项（英文才用得上，中日韩没有复数）。
 */
export type Dict<Z> = Record<keyof Z, string> & {
  [k: `${string}#zero` | `${string}#one` | `${string}#two` | `${string}#few` | `${string}#many` | `${string}#other`]: string;
};

export const MESSAGES: Record<Locale, Record<MessageKey, string>> = {
  "zh-CN": zhCN,
  "en-US": enUS,
  "zh-TW": zhTW,
  "ja-JP": jaJP,
  "ko-KR": koKR,
};
