/**
 * 色块的 React 版：气泡、补全菜单里那些只读的块。
 *
 * 和输入框里那份共用 `lib/chips.ts` 的属性与 CSS —— 用户看到自己发出去的
 * 和刚才打的长得一样。输入框里那份为什么不能也是 React 组件，见 chips.ts
 * 的文件头（一句话：contenteditable 里 re-render 会冲掉光标）。
 */

import { useContext } from "react";

import { type ChipSeg, chipAttrs, chipClass } from "../lib/chips";
import { isDirRef, joinRoot, looksAbsPath } from "../pathDisplay";
import { openFilePreview } from "./FilePreview";
import { ProjectRootContext } from "./Markdown";

/**
 * 一个块。`onClick` 给出去就渲染成按钮。
 *
 * 三种块共用这一个渲染 —— 结构完全一致，差别全在 `chipAttrs` 的属性和
 * CSS 的配色里。
 */
export function Chip({
  seg,
  onClick,
  title,
}: {
  seg: ChipSeg;
  onClick?: () => void;
  title?: string;
}) {
  const attrs = { ...chipAttrs(seg), ...(title ? { title } : {}) };
  if (!onClick) return <span className={chipClass(seg.kind, "static")} {...attrs} />;
  return (
    <button
      type="button"
      className={chipClass(seg.kind, "static clickable")}
      {...attrs}
      onClick={onClick}
    />
  );
}

/**
 * 文件引用块。`preview` 置真时点击打开应用内预览 —— 消息气泡里用；
 * Composer 的 `@` 候选列表里它套在候选按钮内部，保持纯展示
 * （button 嵌 button 不合法，点击语义也归外层）。
 */
export function FileChip({ path, preview = false }: { path: string; preview?: boolean }) {
  // 引用块记的是项目内相对路径，预览要拼成绝对的。
  const root = useContext(ProjectRootContext);
  const seg: ChipSeg = { kind: "ref", value: path };
  // 目录没有单文件内容可预览，点开只会落到"打不开"。
  if (!preview || isDirRef(path)) return <Chip seg={seg} />;
  const full = looksAbsPath(path) ? path : joinRoot(root, path);
  return <Chip seg={seg} title={`预览 ${path}`} onClick={() => openFilePreview(full)} />;
}
