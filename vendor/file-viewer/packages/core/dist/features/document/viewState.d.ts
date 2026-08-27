import type { FileViewerApplyViewStateOptions, FileViewerViewState, FileViewerViewStateChange, FileViewerViewStateProvider } from '../../contracts/types';
export interface RegisterFileViewerGenericViewStateProviderOptions {
    host: HTMLElement;
    renderer?: string;
    scrollTarget?: HTMLElement | (() => HTMLElement | null | undefined) | null;
}
export interface FileViewerGenericViewStateProviderRegistration {
    provider: FileViewerViewStateProvider;
    destroy: () => void;
}
export interface CreateFileViewerViewStateControllerOptions {
    root: () => HTMLElement | null | undefined;
    enabled?: () => boolean;
    onChange?: (change: FileViewerViewStateChange) => void;
}
export interface FileViewerViewStateController {
    readonly provider: FileViewerViewStateProvider | null;
    readonly state: FileViewerViewState | null;
    hasProvider(): boolean;
    refreshProvider(): FileViewerViewStateProvider | null;
    observe(): void;
    clearProvider(): FileViewerViewState | null;
    getState(): FileViewerViewState | null;
    applyState(state: FileViewerViewState, options?: FileViewerApplyViewStateOptions): Promise<FileViewerViewState | null>;
    destroy(): void;
}
export interface FileViewerViewStateControllerActionHandlers {
    hasViewStateProvider(): boolean;
    refreshViewStateProvider(): FileViewerViewStateProvider | null;
    startViewStateObserver(): FileViewerViewState | null;
    stopViewStateObserver(): FileViewerViewState | null;
    clearViewStateProvider(): FileViewerViewState | null;
    getViewState(): FileViewerViewState | null;
    applyViewState(state: FileViewerViewState, options?: FileViewerApplyViewStateOptions): Promise<FileViewerViewState | null>;
}
export declare const cloneFileViewerViewState: (state?: FileViewerViewState | null) => FileViewerViewState | null;
export declare const createFileViewerViewStateChange: (state: FileViewerViewState, patch?: Partial<Omit<FileViewerViewStateChange, "state">>) => FileViewerViewStateChange;
export declare const createFileViewerViewStateChangeEmitter: () => {
    emit(change: FileViewerViewStateChange): void;
    subscribe(listener: (change: FileViewerViewStateChange) => void): () => void;
};
export declare const registerFileViewerGenericViewStateProvider: ({ host, renderer, scrollTarget, }: RegisterFileViewerGenericViewStateProviderOptions) => FileViewerGenericViewStateProviderRegistration;
export declare const createFileViewerViewStateController: ({ root, enabled, onChange, }: CreateFileViewerViewStateControllerOptions) => FileViewerViewStateController;
export declare const createFileViewerViewStateControllerActionHandlers: (controller: FileViewerViewStateController) => FileViewerViewStateControllerActionHandlers;
