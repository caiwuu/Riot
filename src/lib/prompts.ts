import type { PromptPreset } from "../bridge";

/**
 * 提示词库的共用逻辑。设置里的管理页和会话设置的选择器都要用，
 * 分开写的话两边对"这条叫什么"会给出不同答案。
 */

/** 列表行和下拉标签的截断长度。再长在两处都会被 CSS 省略号切掉。 */
const LABEL_MAX = 48;

/**
 * 这条提示词显示成什么。
 *
 * 没写标题时拿正文首行顶上，而不是显示「未命名」—— 用户从会话里
 * 「存为预设」存下来的那条本来就没有标题，首行至少能认出是哪条。
 */
export function presetLabel(p: PromptPreset): string {
  const title = (p.title ?? "").trim();
  if (title) return title;
  const first = p.body.split("\n").find((l) => l.trim())?.trim() ?? "";
  if (!first) return "空提示词";
  return first.length > LABEL_MAX ? `${first.slice(0, LABEL_MAX)}…` : first;
}

/**
 * 行尾和下拉里的次要说明。
 *
 * 没有标题时首行已经当了标题，再铺一遍正文开头就是同一句话说两次 ——
 * 那种情况改报字数，它至少是条新信息。
 */
export function presetSummary(p: PromptPreset): string {
  const body = p.body.trim();
  if (!body) return "还没写内容";
  if (!(p.title ?? "").trim()) return `${body.length} 字`;
  const flat = body.replace(/\s+/g, " ");
  return flat.length > LABEL_MAX ? `${flat.slice(0, LABEL_MAX)}…` : flat;
}

/** 找出正文和给定文本一致的那条。会话靠内容反查自己用的是哪条预设。 */
export function findPreset(list: PromptPreset[], body: string): PromptPreset | undefined {
  const t = body.trim();
  if (!t) return undefined;
  return list.find((p) => p.body.trim() === t);
}

/** 取一个没被占用的 id。`prompt-N`，让 config.json 读起来还是人话。 */
export function newPresetId(list: PromptPreset[]): string {
  let n = list.length + 1;
  while (list.some((p) => p.id === `prompt-${n}`)) n += 1;
  return `prompt-${n}`;
}
