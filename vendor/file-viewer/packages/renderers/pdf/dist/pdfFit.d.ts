import type { FileViewerFitRequest } from '@file-viewer/core';
export declare const PDF_FIT_MIN_VIEWPORT_SIZE = 96;
export declare const PDF_FIT_HORIZONTAL_PADDING = 0;
export declare const PDF_PAGE_BORDER_WIDTH = 0;
export interface ResolvePdfFitViewportSizeInput {
    containerWidth: number;
    containerHeight: number;
    fallbackWidth: number;
    fallbackHeight: number;
    request: Pick<FileViewerFitRequest, 'viewportWidth' | 'viewportHeight' | 'padding'>;
}
/**
 * Resolves the PDF page viewport after the navigation pane has been laid out.
 * Core request dimensions already exclude fit.padding; live container and
 * window fallback dimensions do not, so only those branches subtract it.
 */
export declare const resolvePdfFitViewportSize: ({ containerWidth, containerHeight, fallbackWidth, fallbackHeight, request, }: ResolvePdfFitViewportSizeInput) => {
    width: number;
    height: number;
};
