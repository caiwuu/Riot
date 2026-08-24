//! 输入文本的纯解析层：段落（Seg）、斜杠命令、`@` 文件引用。
//!
//! 从 App.tsx 拆出。这里只有字符串进字符串出的纯函数 —— 输入框的
//! contenteditable DOM 操作在 Composer 里，气泡渲染在 Transcript 里，
//! 两边共用这一份解析规则：**解析和渲染分居两处时，规则必须只有一份**，
//! 否则"发出去的引用"和"画回来的引用"迟早对不上。

/**
 * 输入框里的一段内容：一截文字、一个文件引用块，或一条斜杠命令/技能块。
 *
 * 输入框是 contenteditable 而不是 textarea —— 引用块要和文字**排在
 * 同一行**（用户是在句子中间点名文件的："打开 [index.html] 看看"），
 * 而 textarea 只能装纯文本，块只能堆到框外面去，读起来就和正文脱节了。
 *
 * 命令/技能同样不能是一段可被改坏的 `/compact` 字符串：选中之后变成
 * 色块，退格整块删掉，和普通输入一眼能分开。
 */
export type Seg =
  | { kind: "text"; value: string }
  | { kind: "ref"; value: string }
  | { kind: "cmd"; value: string };

/** 斜杠名：字母数字、中文、冒号命名空间。名字里不含 `/`，免得把 /usr/bin 认成命令。 */
const SLASH_CH = String.raw`[\w\p{L}\p{N}:-]`;
export const SLASH_QUERY_RE = new RegExp(`^/(${SLASH_CH}*)$`, "u");
export const SLASH_LEAD_RE = new RegExp(`^/(${SLASH_CH}+)(\\s)([\\s\\S]*)$`, "u");
export const SLASH_SUBMIT_RE = new RegExp(`^/(${SLASH_CH}+)\\s*([\\s\\S]*)$`, "u");
export const SLASH_HEAD_RE = new RegExp(`^/(${SLASH_CH}+)(?=\\s|$)`, "u");

/** 段落序列里的纯文字部分（补全菜单、空判断用）。 */
export function segsText(segs: Seg[]): string {
  return segs.map((s) => (s.kind === "text" ? s.value : "")).join("");
}

/**
 * 段落序列 → 发出去的消息文本：引用块在**原位**留下 `@路径`。
 *
 * `[约束]` 不能把块的位置丢掉。"把 @a.css 的样式抄给 @b.css" 抹掉标记
 * 之后是"把 的样式抄给"，模型看到的是一句指代不明的话 —— 附件里有那
 * 两个文件也救不回来，它不知道谁抄给谁。顺带，界面重建气泡时也是靠
 * 这些标记把块画回原来的位置。
 */
export function segsToPrompt(segs: Seg[]): string {
  return segs
    .map((s, i) => {
      if (s.kind === "text") return s.value;
      if (s.kind === "cmd") return `/${s.value}`;
      const next = segs[i + 1];
      return mentionToken(s.value, next?.kind === "text" ? next.value : "");
    })
    .join("");
}

/** 把开头的 `/已知命令 ` 收成色块。已经有块、或名字还不完整，原样返回。 */
export function promoteLeadingCmd(segs: Seg[], known: Set<string>): Seg[] | null {
  if (segs.some((s) => s.kind === "cmd")) return null;
  const first = segs[0];
  if (first?.kind !== "text") return null;
  const m = SLASH_LEAD_RE.exec(first.value);
  const [, name, gap, rest] = m ?? [];
  if (name === undefined || gap === undefined || rest === undefined) return null;
  if (!known.has(name)) return null;
  return [{ kind: "cmd", value: name }, { kind: "text", value: gap + rest }, ...segs.slice(1)];
}

/**
 * 引用块 → 正文里的 `@路径`。裸写法认不回来就加引号。
 *
 * 断在哪里由解析器说了算，所以这里直接**拿解析器试一遍**：路径带空格
 * （`@/tmp/报表 (1).xlsx`）、或者后面紧跟着别的字（`@src/a.rs然后改` ——
 * 中文不写空格，这很常见）都会被吞掉半截，只有引号形式才回得来。
 */
export function mentionToken(path: string, after = ""): string {
  const bare = `@${path}`;
  const [span] = extractMentionSpans(bare + after);
  const intact = span?.path === path && span.index === 0 && span.length === bare.length;
  return intact ? bare : `@"${path}"`;
}

/** 与 `mentions.rs` 的 `is_stop_punct` 对齐：这些字符在路径里几乎不会出现。 */
const MENTION_STOP = new Set("，。；：、！？）（「」《》“”");

/**
 * `@` 前面这个字符算不算边界（与 `mentions.rs` 的 `is_mention_boundary`
 * 对齐 —— 内核认不认和界面画不画必须是同一条规则）。
 *
 * 反着定义：只有 ASCII 标识符字符才**不是**边界，那正是 `me@example.com`
 * 的形状。中文不写空格，"读下@src/a.rs" 里的 `下` 必须算边界。
 */
const MENTION_GLUE = /[A-Za-z0-9._%+-]/;

/**
 * 正文里一段 `@路径` 标记。`index`/`length` 覆盖整段 token（含 `@` 和引号）。
 *
 * 规则跟内核 `mentions::extract` 对齐：邮箱、行内代码、中文口语不当引用。
 */
export interface MentionSpan {
  path: string;
  index: number;
  length: number;
}

/** 长得像路径才画成块 —— `@这里` 这种口语必须留在原文里。 */
export function mentionLooksLikePath(s: string): boolean {
  if (!s) return false;
  if (s.includes("/") || s.includes("\\") || s.startsWith(".") || s.startsWith("~")) return true;
  return /^[A-Za-z0-9_.-]+$/.test(s);
}

export function mentionTrimPunct(s: string): string {
  return s.replace(/[.,;:!?)"']+$/, "");
}

/**
 * 从用户气泡正文里挑出 `@路径` 标记，好把块画回原位。
 *
 * 发送时乐观气泡带着 `files`；切会话后界面按历史重画。二进制、目录、
 * 读失败的引用不会落成 `user_file` 附件，`files` 就是空的 —— 但标记还
 * 在正文里（见 segsToPrompt），靠它重建，不能只拿附件当白名单。
 */
export function extractMentionSpans(text: string): MentionSpan[] {
  const spans: MentionSpan[] = [];
  let inFence = false;
  let offset = 0;
  const lines = text.split("\n");
  for (let li = 0; li < lines.length; li++) {
    const line = lines[li] ?? "";
    if (line.trimStart().startsWith("```")) {
      inFence = !inFence;
    } else if (!inFence) {
      let i = 0;
      let inTick = false;
      let prevBoundary = true;
      while (i < line.length) {
        const c = line[i] ?? "";
        if (c === "`") {
          inTick = !inTick;
          prevBoundary = true;
          i += 1;
          continue;
        }
        if (inTick || c !== "@" || !prevBoundary) {
          prevBoundary = !MENTION_GLUE.test(c);
          i += 1;
          continue;
        }
        if (line[i + 1] === '"') {
          const start = i + 2;
          const end = line.indexOf('"', start);
          if (end >= 0) {
            const raw = line.slice(start, end);
            if (raw.trim()) {
              spans.push({ path: raw, index: offset + i, length: end + 1 - i });
            }
            i = end + 1;
            prevBoundary = false;
            continue;
          }
        }
        let j = i + 1;
        while (j < line.length) {
          const ch = line[j] ?? "";
          if (/\s/u.test(ch) || MENTION_STOP.has(ch)) break;
          j += 1;
        }
        const raw = line.slice(i + 1, j);
        const cleaned = mentionTrimPunct(raw);
        if (mentionLooksLikePath(cleaned)) {
          spans.push({ path: cleaned, index: offset + i, length: 1 + cleaned.length });
        }
        i += 1 + raw.length;
        prevBoundary = false;
      }
    }
    offset += line.length + 1;
  }
  return spans;
}

/**
 * 发出去的正文 → 输入框里的段落：`@路径` 标记原位还原成引用块。
 *
 * 放回输入框的是**发出去的那一份文本**，块在原位留下了 `@路径`（见
 * `segsToPrompt`）。不还原的话，用户看到的是一句夹着裸路径的话，而且
 * 再发一次会连块带标记发出两份同样的引用。
 */
export function promptToSegs(text: string, refs: string[] = [], skip: string[] = []): Seg[] {
  const segs: Seg[] = [];
  const seen = new Set<string>();
  let last = 0;
  for (const s of extractMentionSpans(text)) {
    if (s.index > last) segs.push({ kind: "text", value: text.slice(last, s.index) });
    segs.push({ kind: "ref", value: s.path });
    seen.add(s.path);
    last = s.index + s.length;
  }
  if (last < text.length) segs.push({ kind: "text", value: text.slice(last) });
  // 正文里没留下标记的引用（老消息、用户把标记删了）补在末尾 ——
  // 丢掉的话模型就看不到那个文件了。
  for (const r of refs) {
    if (!mentionCovers(seen, r) && !skip.includes(r)) segs.push({ kind: "ref", value: r });
  }
  return segs;
}

/** 历史附件里的绝对路径，和正文里的相对 `@src/a.rs` 算同一个文件。 */
export function mentionCovers(seen: Set<string>, file: string): boolean {
  if (seen.has(file)) return true;
  for (const p of seen) {
    if (file.endsWith(`/${p}`) || file.endsWith(`\\${p}`) || p.endsWith(`/${file}`) || p.endsWith(`\\${file}`)) {
      return true;
    }
  }
  return false;
}

