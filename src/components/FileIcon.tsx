/**
 * 文件类型图标。用的是 Cursor / VS Code 内置的 seti 图标字体（拷进
 * assets/fonts，映射生成在 lib/fileIcons.ts）—— 用户在编辑器里天天看的
 * 就是这套，不用重新学一遍"哪个颜色是哪种文件"。
 *
 * 改动列表、文件树、引用块共用这一份；图标只认文件名，不碰磁盘。
 */

import { SETI_BY_EXT, SETI_BY_NAME, SETI_DEFAULT, type SetiIcon } from "../lib/fileIcons";

/** 文件名 → 图标。先按完整文件名（package.json、dockerfile…），
 *  再按后缀从长到短（x.blade.php 先试 blade.php 再试 php）。 */
export function iconFor(path: string): SetiIcon {
  const name = path.slice(Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")) + 1).toLowerCase();
  const byName = SETI_BY_NAME[name];
  if (byName) return byName;
  const parts = name.split(".");
  for (let i = 1; i < parts.length; i++) {
    const icon = SETI_BY_EXT[parts.slice(i).join(".")];
    if (icon) return icon;
  }
  return SETI_DEFAULT;
}

export function FileIcon({ path }: { path: string }) {
  const icon = iconFor(path);
  return (
    <span className="file-icon" style={{ color: icon.color }} aria-hidden>
      {icon.ch}
    </span>
  );
}
