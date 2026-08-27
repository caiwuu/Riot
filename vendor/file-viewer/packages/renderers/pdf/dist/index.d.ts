import { type FileRenderHandler, type FileViewerRenderedInstance, type FileViewerRendererPlugin, type RendererDefinition } from '@file-viewer/core';
export declare const pdfRendererDefinition: RendererDefinition;
export declare const renderFileViewerPdf: FileRenderHandler<FileViewerRenderedInstance, HTMLDivElement>;
export declare const pdfRenderer: FileViewerRendererPlugin<FileRenderHandler<FileViewerRenderedInstance, HTMLDivElement>>;
export default pdfRenderer;
