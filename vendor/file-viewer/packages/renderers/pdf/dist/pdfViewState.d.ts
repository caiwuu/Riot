import type { FileViewerViewState } from '@file-viewer/core';
export type PdfRotation = 0 | 90 | 180 | 270;
export declare const normalizePdfRotation: (rotation: number) => PdfRotation;
export declare const clampPdfScale: (scale: number, minScale: number, maxScale: number) => number;
export declare const resolvePdfViewStateUpdate: (state: FileViewerViewState, current: {
    rotation: number;
    scale: number;
    page: number;
    pageCount: number;
}, limits: {
    minScale: number;
    maxScale: number;
}) => {
    rotation: number | undefined;
    scale: number | undefined;
    page: number | undefined;
};
