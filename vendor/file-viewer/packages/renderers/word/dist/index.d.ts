import { type FileRenderHandler, type FileViewerRenderedInstance, type FileViewerRendererPlugin, type RendererDefinition } from '@file-viewer/core';
export declare const wordRendererDefinitions: RendererDefinition[];
export declare const renderFileViewerWordDocx: FileRenderHandler<FileViewerRenderedInstance, HTMLDivElement>;
/**
 * A surprising number of legacy systems keep the `.doc` suffix after saving
 * an OOXML document. Route those ZIP-based files through the DOCX engine while
 * leaving genuine OLE/CFB `.doc` files on the binary parser.
 */
export declare const resolveFileViewerWordContainer: (buffer: ArrayBuffer) => "openxml" | "binary";
export declare const renderFileViewerWordDoc: FileRenderHandler<FileViewerRenderedInstance, HTMLDivElement>;
export declare const renderFileViewerOpenDocument: FileRenderHandler<FileViewerRenderedInstance, HTMLDivElement>;
export declare const wordRenderer: FileViewerRendererPlugin<FileRenderHandler<FileViewerRenderedInstance, HTMLDivElement>>;
export default wordRenderer;
