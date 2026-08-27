/** 路径最后一段。Windows 用 `\`，只按 `/` 切会把整条路径当成名字。 */
export function basename(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const i = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return i >= 0 ? trimmed.slice(i + 1) : trimmed || path;
}

/** 去掉最后一段。驱动器根（`D:\`）原样留下。 */
export function parentOf(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const i = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  if (i < 0) return trimmed;
  if (i === 2 && trimmed[1] === ":") return trimmed.slice(0, 3);
  return i === 0 ? "/" : trimmed.slice(0, i);
}

/** 像不像绝对路径（Unix `/`、UNC `\\`、Windows 盘符）。 */
export function looksAbsPath(s: string): boolean {
  return s.startsWith("/") || s.startsWith("\\\\") || /^[A-Za-z]:[\\/]/.test(s);
}

/** 相对路径拼到项目根上，分隔符跟着根走。已是绝对路径的调用方自己先判。 */
export function joinRoot(root: string, rel: string): string {
  const cleaned = rel.replace(/^\.[\\/]+/, "");
  if (!root) return cleaned;
  const sep = root.includes("\\") ? "\\" : "/";
  return `${root.replace(/[\\/]+$/, "")}${sep}${cleaned.replace(/[\\/]+/g, sep)}`;
}

/** 家目录换成 `~`。macOS 是 `/Users/xxx`，Windows 是 `C:\Users\xxx`。 */
export function tildify(path: string): string {
  const unix = /^\/Users\/[^/]+/.exec(path);
  if (unix) return path.slice(unix[0].length) ? `~${path.slice(unix[0].length)}` : "~";
  const win = /^[A-Za-z]:[\\/]Users[\\/][^\\/]+/.exec(path);
  if (win) {
    const rest = path.slice(win[0].length);
    return rest ? `~${rest}` : "~";
  }
  return path;
}
