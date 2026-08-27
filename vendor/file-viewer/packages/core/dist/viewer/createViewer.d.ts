import type { FileViewerEventHandler, FileViewerInstance, FileViewerOptions, FileViewerRenderPurpose, RendererRegistry } from '../contracts/types';
export interface CreateViewerOptions {
    registry?: RendererRegistry;
    options?: FileViewerOptions;
    signal?: AbortSignal;
    onEvent?: FileViewerEventHandler;
    renderPurpose?: FileViewerRenderPurpose;
}
export declare const createViewer: (container: HTMLElement, createOptions?: CreateViewerOptions) => FileViewerInstance;
