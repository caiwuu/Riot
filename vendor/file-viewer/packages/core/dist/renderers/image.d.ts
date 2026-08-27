import type { FileRenderContext, FileViewerRenderedInstance } from '../contracts/types';
export default function renderImage(buffer: ArrayBuffer, target: HTMLDivElement, type?: string, context?: FileRenderContext): Promise<FileViewerRenderedInstance>;
