import type { FileViewerDocumentAnchor } from '../../../contracts/types';
export declare const DEFAULT_FILE_VIEWER_ANCHOR_SELECTOR: string;
export declare const DEFAULT_FILE_VIEWER_ANCHOR_EXCLUDE_SELECTOR: string;
export declare const collectFileViewerDocumentAnchors: (root: HTMLElement | null) => FileViewerDocumentAnchor[];
export declare const findFileViewerAnchorForElement: (element: Element | null, anchors: FileViewerDocumentAnchor[], root?: HTMLElement | null) => FileViewerDocumentAnchor | null;
export declare const getCurrentFileViewerDocumentAnchor: (root: HTMLElement | null, anchors: FileViewerDocumentAnchor[]) => FileViewerDocumentAnchor | null;
export declare const scrollToFileViewerDocumentAnchor: (root: HTMLElement | null, anchor: FileViewerDocumentAnchor | string | number | null | undefined) => boolean;
