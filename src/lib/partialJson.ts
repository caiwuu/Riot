/**
 * 从尚未闭合的 JSON 对象里抽出顶层字符串字段。
 *
 * 给流式 tool_input 用：ExitPlanMode 的整份计划在 `{"plan":"..."}` 里，
 * 完整 JSON 到齐之前用户就该看见正文在长，而不是对着三个点干等。
 *
 * 只认**第一个**顶层键，避免 Bash/Write 的参数值里碰巧出现 `"plan"`。
 * 字段还没出现或值还没开始时返回 `null`；键已经对上、值还是空的，返回 `""`。
 */
export function extractTopLevelStringField(
  partial: string,
  field: string,
): string | null {
  const t = partial.trimStart();
  if (!t.startsWith("{")) return null;
  let i = 1;
  i = skipWs(t, i);
  const key = `"${field}"`;
  if (!t.startsWith(key, i)) return null;
  i += key.length;
  i = skipWs(t, i);
  if (i >= t.length) return "";
  if (t[i] !== ":") return null;
  i += 1;
  i = skipWs(t, i);
  if (i >= t.length) return "";
  if (t[i] !== '"') return null;
  return unescapeJsonStringPrefix(t.slice(i + 1));
}

/**
 * 从尚未闭合的 JSON 对象里抽出已出现的顶层字符串字段。
 *
 * Write 的参数是 `{"path":"...","content":"..."}`，正文在第二个键里，
 * 而且会流很久。只看第一个键的话，用户对着三个点干等整份文件写完。
 * 值还在写的那个字段带上已写出的前缀。
 */
export function extractTopLevelStringFields(partial: string): Record<string, string> {
  const t = partial.trimStart();
  if (!t.startsWith("{")) return {};
  const out: Record<string, string> = {};
  let i = 1;
  while (i < t.length) {
    i = skipWs(t, i);
    if (i >= t.length) break;
    if (t[i] === "}") break;
    if (t[i] === ",") {
      i += 1;
      continue;
    }
    if (t[i] !== '"') break;
    const key = readClosedJsonString(t, i);
    if (!key) break;
    i = key.end;
    i = skipWs(t, i);
    if (i >= t.length) break;
    if (t[i] !== ":") break;
    i += 1;
    i = skipWs(t, i);
    if (i >= t.length) {
      out[key.value] = "";
      break;
    }
    if (t[i] !== '"') break;
    const val = unescapeJsonStringPrefix(t.slice(i + 1));
    out[key.value] = val;
    const closed = jsonStringClosed(t, i + 1);
    if (!closed) break;
    i = closed;
  }
  return out;
}

/** 读一个已经闭合的 JSON 字符串（含开头引号）。没闭合就当键还没写完。 */
function readClosedJsonString(s: string, start: number): { value: string; end: number } | null {
  if (s[start] !== '"') return null;
  const value = unescapeJsonStringPrefix(s.slice(start + 1));
  const end = jsonStringClosed(s, start + 1);
  if (end === null) return null;
  return { value, end };
}

/** 从字符串内容起点找到闭合引号之后的下标；还没闭合返回 null。 */
function jsonStringClosed(s: string, from: number): number | null {
  for (let i = from; i < s.length; i++) {
    const c = s[i];
    if (c === '"') return i + 1;
    if (c !== "\\") continue;
    if (i + 1 >= s.length) return null;
    const n = s[++i]!;
    if (n === "u") {
      if (i + 4 >= s.length) return null;
      i += 4;
    }
  }
  return null;
}

function skipWs(s: string, i: number): number {
  while (i < s.length) {
    const c = s[i];
    if (c !== " " && c !== "\n" && c !== "\r" && c !== "\t") break;
    i += 1;
  }
  return i;
}

const ESC: Record<string, string> = {
  '"': '"',
  "\\": "\\",
  "/": "/",
  b: "\b",
  f: "\f",
  n: "\n",
  r: "\r",
  t: "\t",
};

/** 解开一段可能尚未闭合的 JSON 字符串（不含开头的引号）。 */
function unescapeJsonStringPrefix(s: string): string {
  let out = "";
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (c === '"') break;
    if (c !== "\\") {
      out += c;
      continue;
    }
    if (i + 1 >= s.length) break;
    const n = s[++i]!;
    if (n === "u") {
      if (i + 4 >= s.length) break;
      const hex = s.slice(i + 1, i + 5);
      if (!/^[0-9a-fA-F]{4}$/.test(hex)) break;
      out += String.fromCharCode(Number.parseInt(hex, 16));
      i += 4;
      continue;
    }
    out += ESC[n] ?? n;
  }
  return out;
}
