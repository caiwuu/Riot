import type { PptxPresentationState } from './presentation';
import type { PptxViewerOptions } from './types';
export declare class PptxViewer {
    static open(buffer: ArrayBuffer, target: HTMLElement, options?: PptxViewerOptions): Promise<PptxViewer>;
    readonly target: HTMLElement;
    readonly content: HTMLDivElement;
    readonly scaleBox: HTMLDivElement;
    private readonly buffer;
    private readonly options;
    private worker;
    private resizeObserver;
    private resizeFrame;
    private fitScale;
    private userZoomPercent;
    private currentZoomPercent;
    private thumbnailElement;
    private previewThumbnailDataUrl;
    private slideSize;
    private slideRecords;
    private totalSlides;
    private savedScroll;
    private pendingScrollRestore;
    private slideWindowTarget;
    private slideWindowListeners;
    private slideWindowFrame;
    private charts;
    private readonly chartHandles;
    private readonly mediaRecords;
    private disposed;
    private completed;
    private presentation;
    private readonly handleSlideWindowChange;
    private constructor();
    get zoomPercent(): number;
    get thumbnailDataUrl(): string | null;
    get slideCount(): number;
    get slideDimensions(): {
        width: number;
        height: number;
    } | null;
    get presenting(): boolean;
    get presentationSlideNumber(): number;
    private get styleRoot();
    /**
     * Where the slideshow overlay is mounted. It has to share a root with the injected slide styles,
     * or the engine's scoped CSS stops applying once the slides move into the overlay.
     */
    get presentationRoot(): ShadowRoot | HTMLElement;
    /** Force a slide out of the virtualized window so the slideshow can show it immediately. */
    ensureSlideRendered(slideNumber: number): Element | null;
    refreshLayout(): void;
    /**
     * Remember where the deck was scrolled before the slideshow moves the scale
     * box into the overlay. Moving it collapses the scroller and clamps scrollTop
     * to zero, so the position has to be captured up front and restored on exit.
     */
    saveScrollPosition(): void;
    restoreScrollPosition(): void;
    emitPresentationChange(state: PptxPresentationState): void;
    enterPresentation(slideNumber?: number): Promise<void>;
    exitPresentation(): void;
    togglePresentation(slideNumber?: number): Promise<void>;
    open(): Promise<void>;
    setZoom(percent: number): Promise<void>;
    destroy(): void;
    private startWorker;
    private processMessage;
    private appendGlobalCss;
    private showThumbnail;
    private clearThumbnail;
    private shouldWindowSlides;
    private getSlideWindowOptions;
    private getEstimatedSlideHeight;
    private appendWindowedSlide;
    private createWindowedSlideRecord;
    private getDesignerCanvases;
    private renderSlideRecord;
    private unmountSlideRecord;
    private updateMeasuredSlideHeight;
    private syncWindowedPlaceholderHeights;
    private scheduleSlideWindowUpdate;
    private updateSlideWindow;
    private getWindowedSlideIndexes;
    private getDistanceFromViewport;
    private getSlideViewport;
    private attachSlideWindowListeners;
    private addSlideWindowListener;
    private detachSlideWindowListeners;
    private findSlideWindowTarget;
    private postProcessRenderedContent;
    private postProcessSlideRecord;
    /** Materializes every lazy slide only for the duration of an export snapshot. */
    cloneForExport(): Promise<HTMLElement>;
    private complete;
    private fail;
    private trackChartHandle;
    private releaseChartHandle;
    private releaseCharts;
    private storeMedia;
    private hydrateMedia;
    private releaseMedia;
    private attachResizeObserver;
    private scheduleResize;
    private resize;
    private getMountedSlideElements;
    private isSlideElement;
}
