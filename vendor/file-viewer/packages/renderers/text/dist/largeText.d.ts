import { type FileRenderContext, type FileViewerRenderedInstance } from '@file-viewer/core';
export declare const DEFAULT_LARGE_TEXT_THRESHOLD_BYTES: number;
export declare const DEFAULT_LARGE_TEXT_LINE_SEGMENT_BYTES: number;
export declare const DEFAULT_LARGE_TEXT_OVERSCAN_LINES = 12;
export declare const shouldVirtualizeTextBuffer: (buffer: ArrayBuffer, context?: FileRenderContext) => boolean;
export declare const shouldVirtualizeMarkdownBuffer: (buffer: ArrayBuffer, context?: FileRenderContext) => boolean;
export default function renderLargeText(buffer: ArrayBuffer, target: HTMLDivElement, type?: string, context?: FileRenderContext): Promise<FileViewerRenderedInstance>;
