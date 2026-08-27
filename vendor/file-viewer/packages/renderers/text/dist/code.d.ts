import { type FileRenderContext, type FileViewerRenderedInstance } from '@file-viewer/core';
/**
 * Framework-neutral text/code renderer.
 *
 * highlight.js core and language definitions are loaded lazily by format. HTML
 * and XML are highlighted as escaped source text, never executed as real DOM.
 * @param buffer 文本二进制内容
 * @param target 目标
 * @param type 文件扩展名，用于选择 highlight.js 语言
 */
export default function renderText(buffer: ArrayBuffer, target: HTMLDivElement, type?: string, context?: FileRenderContext): Promise<FileViewerRenderedInstance>;
