import { type FileRenderContext, type FileViewerRenderedInstance } from '@file-viewer/core';
export declare const stripMarkdownFrontmatter: (text: string) => string;
export default function renderMarkdown(buffer: ArrayBuffer, target: HTMLDivElement, context?: FileRenderContext): Promise<FileViewerRenderedInstance>;
