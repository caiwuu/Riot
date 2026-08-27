import { type FileRenderContext, type FileViewerRenderedInstance as AppWrapper } from '@file-viewer/core';
/**
 * 渲染 doc 文件
 */
export default function render(buffer: ArrayBuffer, target: HTMLDivElement, context?: FileRenderContext): Promise<AppWrapper>;
