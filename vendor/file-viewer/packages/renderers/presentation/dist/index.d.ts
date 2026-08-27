import { type FileRenderHandler, type FileViewerRenderedInstance, type FileViewerRendererPlugin, type RendererDefinition } from '@file-viewer/core';
export declare const binaryPresentationRendererDefinition: RendererDefinition;
export declare const presentationRendererDefinition: RendererDefinition;
export declare const renderFileViewerBinaryPresentation: FileRenderHandler<FileViewerRenderedInstance, HTMLDivElement>;
export declare const renderFileViewerPresentation: FileRenderHandler<FileViewerRenderedInstance, HTMLDivElement>;
export declare const presentationRenderer: FileViewerRendererPlugin<FileRenderHandler<FileViewerRenderedInstance, HTMLDivElement>>;
export default presentationRenderer;
