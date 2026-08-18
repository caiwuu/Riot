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
