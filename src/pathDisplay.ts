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

/**
 * 引用块认目录的办法：路径以分隔符结尾。
 *
 * 图标、发出去的 `@路径/`、气泡回读都走这一份约定 —— 不另开字段，
 * 免得输入框、正文、历史三条路各记各的，迟早对不上。
 */
export function isDirRef(path: string): boolean {
  return /[\\/]$/.test(path);
}

/** 给目录路径补上结尾 `/`（已有分隔符的原样）。块上的短名仍走 basename。 */
export function asDirRef(path: string): string {
  return isDirRef(path) ? path : `${path.replace(/[\\/]+$/, "")}/`;
}

/** 相对路径拼到项目根上，分隔符跟着根走。已是绝对路径的调用方自己先判。 */
export function joinRoot(root: string, rel: string): string {
  const cleaned = rel.replace(/^\.[\\/]+/, "");
  if (!root) return cleaned;
  const sep = root.includes("\\") ? "\\" : "/";
  return `${root.replace(/[\\/]+$/, "")}${sep}${cleaned.replace(/[\\/]+/g, sep)}`;
}

/**
 * `path` 在 `root` 下的相对路径（`/` 分隔，根本身是空串）；不在就 null。
 * 按路径段比较：`/work` 不是 `/workspace/a` 的根。
 */
export function relativeTo(root: string, path: string): string | null {
  if (!root) return null;
  const base = root.replace(/[\\/]+$/, "");
  if (!path.startsWith(base)) return null;
  const rest = path.slice(base.length);
  if (rest && !/^[\\/]/.test(rest)) return null;
  return rest.replace(/^[\\/]+/, "").replace(/\\/g, "/");
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
