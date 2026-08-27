export declare const DEFAULT_FILE_VIEWER_SCROLL_CONTAINER_SELECTOR = "[data-viewer-scroll-container], .pdf-wrapper";
export declare const DEFAULT_FILE_VIEWER_SCROLL_CONTAINER_CANDIDATE_SELECTOR = "div, section, article, pre";
export declare const DEFAULT_FILE_VIEWER_SCROLLABLE_OVERFLOW_VALUES: readonly ["auto", "scroll", "overlay"];
export interface ResolveFileViewerScrollContainerOptions {
    preferredSelector?: string;
    candidateSelector?: string;
    overflowValues?: readonly string[];
    minScrollRange?: number;
}
export declare const getFileViewerScrollableRange: (element: HTMLElement) => number;
export declare const isFileViewerScrollableElement: (element: HTMLElement, options?: ResolveFileViewerScrollContainerOptions) => boolean;
export declare const resolveFileViewerScrollContainer: (root: HTMLElement | null | undefined, options?: ResolveFileViewerScrollContainerOptions) => HTMLElement | null;
