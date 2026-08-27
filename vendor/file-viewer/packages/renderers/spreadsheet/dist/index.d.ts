import { type FileRenderHandler, type FileViewerRenderedInstance, type FileViewerRendererPlugin, type RendererDefinition } from '@file-viewer/core';
export declare const spreadsheetRendererDefinition: RendererDefinition;
export declare const dbfRendererDefinition: RendererDefinition;
export declare const renderFileViewerSpreadsheet: FileRenderHandler<FileViewerRenderedInstance, HTMLDivElement>;
export declare const spreadsheetRenderer: FileViewerRendererPlugin<FileRenderHandler<FileViewerRenderedInstance, HTMLDivElement>>;
export default spreadsheetRenderer;
