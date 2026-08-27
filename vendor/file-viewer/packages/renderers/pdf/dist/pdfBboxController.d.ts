import type { FileViewerPdfBoundingBox, FileViewerViewStateChangeSource } from '@file-viewer/core';
interface PdfBoundingBoxPage {
    view?: number[];
    getViewport: (options: {
        scale: number;
        rotation: number;
    }) => {
        width: number;
        height: number;
    };
}
interface PdfBoundingBoxDocument {
    getPage: (pageNumber: number) => Promise<PdfBoundingBoxPage>;
}
export interface CreatePdfBoundingBoxControllerOptions {
    documentRef: Document;
    targetWindow: Window;
    viewerRoot: HTMLElement;
    scrollContainer: HTMLElement;
    initial?: FileViewerPdfBoundingBox | readonly FileViewerPdfBoundingBox[];
    getDocument: () => PdfBoundingBoxDocument | null;
    getPageCount: () => number;
    getCurrentPage: () => number;
    getRotation: () => number;
    goToPage: (page: number, source: FileViewerViewStateChangeSource) => void;
    suppressProgrammaticScrollEvents: () => void;
    waitForPaint: (view?: Window | null) => Promise<void>;
}
export interface PdfBoundingBoxRenderOptions {
    focus?: boolean;
    pageNumber?: number;
    source?: FileViewerViewStateChangeSource;
}
export interface PdfBoundingBoxController {
    hasBoxes(): boolean;
    getStateValue(): FileViewerPdfBoundingBox | FileViewerPdfBoundingBox[] | null;
    set(input: unknown, options?: Pick<PdfBoundingBoxRenderOptions, 'focus' | 'source'>): Promise<boolean>;
    render(options?: PdfBoundingBoxRenderOptions): Promise<void>;
    destroy(): void;
}
export declare const createPdfBoundingBoxController: ({ documentRef, targetWindow, viewerRoot, scrollContainer, initial, getDocument, getPageCount, getCurrentPage, getRotation, goToPage, suppressProgrammaticScrollEvents, waitForPaint, }: CreatePdfBoundingBoxControllerOptions) => PdfBoundingBoxController;
export {};
