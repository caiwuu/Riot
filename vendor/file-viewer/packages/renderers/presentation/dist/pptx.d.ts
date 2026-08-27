import type { FileRenderContext, FileRenderHandler, FileViewerRenderedInstance, FileViewerRendererPlugin, RendererDefinition } from '@file-viewer/core';
export declare const resolvePptxPreviewErrorMessage: (error: unknown, fallback: string, context?: FileRenderContext) => string;
export default function renderPptx(buffer: ArrayBuffer, target: HTMLDivElement, _type?: string, context?: FileRenderContext): Promise<FileViewerRenderedInstance>;
export declare const presentationRendererDefinition: RendererDefinition;
export declare const renderFileViewerPresentation: FileRenderHandler<FileViewerRenderedInstance, HTMLDivElement>;
export declare const pptxRenderer: FileViewerRendererPlugin<FileRenderHandler<FileViewerRenderedInstance, HTMLDivElement>>;
