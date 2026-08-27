import type { FileViewerFitMode, FileViewerFitOptions, FileViewerFitRequest, FileViewerFitResize, FileViewerFitResult, FileViewerViewState, FileViewerViewStateChangeSource } from '../../contracts/types';
export declare const FILE_VIEWER_FIT_MODES: readonly ["auto", "contain", "cover", "width", "height", "actual", "scale-down"];
export declare const FILE_VIEWER_FIT_RESIZE_MODES: readonly ["until-interaction", "always", "initial"];
export interface NormalizedFileViewerFitOptions extends Required<Pick<FileViewerFitOptions, 'mode' | 'resize' | 'padding'>> {
    minScale?: number;
    maxScale?: number;
}
export interface ResolveFileViewerFitScaleInput {
    mode: FileViewerFitMode;
    viewportWidth: number;
    viewportHeight: number;
    contentWidth: number;
    contentHeight: number;
    currentScale?: number;
    minScale?: number;
    maxScale?: number;
}
export interface CreateFileViewerFitControllerOptions {
    root: () => HTMLElement | null | undefined;
    enabled?: () => boolean;
    getFit?: () => FileViewerFitMode | FileViewerFitOptions | null | undefined;
    onFit?: (result: FileViewerFitResult) => void;
}
export interface ApplyInitialFileViewerFitOptions {
    skip?: boolean;
}
export interface RunFileViewerFitOptions {
    source?: FileViewerViewStateChangeSource;
    reason?: FileViewerFitRequest['reason'];
}
export interface FileViewerFitController {
    normalize(fit?: FileViewerFitMode | FileViewerFitOptions | null): NormalizedFileViewerFitOptions | null;
    fit(fit?: FileViewerFitMode | FileViewerFitOptions | null, options?: RunFileViewerFitOptions): Promise<FileViewerFitResult>;
    applyInitialFit(options?: ApplyInitialFileViewerFitOptions): Promise<FileViewerFitResult>;
    scheduleFit(reason?: FileViewerFitRequest['reason']): void;
    observe(): void;
    markUserInteraction(): void;
    resetAutoFit(): void;
    destroy(): void;
}
export declare const isFileViewerFitMode: (value: unknown) => value is FileViewerFitMode;
export declare const isFileViewerFitResize: (value: unknown) => value is FileViewerFitResize;
export declare const normalizeFileViewerFitOptions: (fit?: FileViewerFitMode | FileViewerFitOptions | null) => NormalizedFileViewerFitOptions | null;
export declare const resolveFileViewerFitScale: ({ mode, viewportWidth, viewportHeight, contentWidth, contentHeight, minScale, maxScale, }: ResolveFileViewerFitScaleInput) => number | undefined;
export declare const hasFileViewerExplicitInitialViewState: (state?: FileViewerViewState | null) => boolean;
export declare const createFileViewerFitController: ({ root, enabled, getFit, onFit, }: CreateFileViewerFitControllerOptions) => FileViewerFitController;
