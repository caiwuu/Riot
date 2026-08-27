import { type FileRenderHandler, type FileViewerRenderedInstance, type FileViewerRendererPlugin, type RendererDefinition } from '@file-viewer/core';
export declare const textRendererDefinitions: RendererDefinition[];
export declare const renderFileViewerCode: FileRenderHandler<FileViewerRenderedInstance, HTMLDivElement>;
export declare const renderFileViewerMarkdown: FileRenderHandler<FileViewerRenderedInstance, HTMLDivElement>;
export declare const textRenderer: FileViewerRendererPlugin<FileRenderHandler<FileViewerRenderedInstance, HTMLDivElement>>;
export default textRenderer;
