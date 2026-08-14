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
