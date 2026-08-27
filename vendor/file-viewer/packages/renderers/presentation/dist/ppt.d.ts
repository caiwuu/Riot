import type { FileRenderContext, FileRenderHandler, FileViewerRenderedInstance, FileViewerRendererPlugin, RendererDefinition } from '@file-viewer/core';
export default function renderPpt(buffer: ArrayBuffer, target: HTMLDivElement, _type?: string, context?: FileRenderContext): Promise<FileViewerRenderedInstance>;
export declare const binaryPresentationRendererDefinition: RendererDefinition;
export declare const renderFileViewerBinaryPresentation: FileRenderHandler<FileViewerRenderedInstance, HTMLDivElement>;
export declare const pptRenderer: FileViewerRendererPlugin<FileRenderHandler<FileViewerRenderedInstance, HTMLDivElement>>;
