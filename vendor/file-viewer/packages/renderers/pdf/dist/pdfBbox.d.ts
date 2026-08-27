import type { FileViewerPdfBoundingBox } from '@file-viewer/core';
export interface NormalizedPdfBoundingBox {
    id?: string;
    page: number;
    x: number;
    y: number;
    width: number;
    height: number;
    color?: string;
    label?: string;
}
export interface PdfPageBox {
    x: number;
    y: number;
    width: number;
    height: number;
}
export declare const normalizePdfBoundingBoxInput: (input: unknown) => FileViewerPdfBoundingBox[];
export declare const normalizePdfBoundingBox: (input: FileViewerPdfBoundingBox, pageBox: PdfPageBox, fallbackPage?: number) => NormalizedPdfBoundingBox | null;
export declare const rotateNormalizedPdfBoundingBox: (box: NormalizedPdfBoundingBox, rotation: number) => NormalizedPdfBoundingBox;
export declare const serializePdfBoundingBoxes: (input: unknown) => string;
