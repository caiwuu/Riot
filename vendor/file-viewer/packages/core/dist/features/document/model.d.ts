import type { FileViewerAiOptions, FileViewerDocumentAnchor, FileViewerDocumentChunk, FileViewerSearchOptions, FileViewerSearchState, FileViewerZoomState } from '../../contracts/types';
export declare const DEFAULT_FILE_VIEWER_ZOOM_SCALE = 1;
export declare const DEFAULT_FILE_VIEWER_TEXT_CHUNK_SIZE = 1200;
export declare const DEFAULT_FILE_VIEWER_TEXT_CHUNK_OVERLAP = 160;
export declare const createFileViewerZoomState: (patch?: Partial<FileViewerZoomState>) => FileViewerZoomState;
export declare const normalizeFileViewerSearchOptions: (options?: boolean | FileViewerSearchOptions) => FileViewerSearchOptions;
export declare const createEmptyFileViewerSearchState: (query?: string) => FileViewerSearchState;
export declare const normalizeFileViewerAiOptions: (options?: boolean | FileViewerAiOptions) => FileViewerAiOptions;
export declare const buildFileViewerDocumentTextChunks: (anchors: FileViewerDocumentAnchor[], options?: boolean | FileViewerAiOptions) => FileViewerDocumentChunk[];
