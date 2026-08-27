import { type FileRenderContext, type FileViewerRenderedInstance } from '@file-viewer/core';
export declare const DEFAULT_FILE_VIEWER_PDF_WORKER_URL = "vendor/pdf/pdf.worker.mjs";
export default function renderPdf(buffer: ArrayBuffer, target: HTMLDivElement, context?: FileRenderContext): Promise<FileViewerRenderedInstance>;
