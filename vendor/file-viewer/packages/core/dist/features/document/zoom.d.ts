import type { FileViewerOperationType, FileViewerZoomProvider, FileViewerZoomState } from '../../contracts/types';
export type FileViewerZoomOperation = Extract<FileViewerOperationType, 'zoom-in' | 'zoom-out' | 'zoom-reset'>;
export interface CreateFileViewerZoomControllerOptions {
    root: () => HTMLElement | null | undefined;
    enabled?: () => boolean;
    beforeZoom?: (operation: FileViewerZoomOperation) => Promise<boolean> | boolean;
    onChange?: (state: FileViewerZoomState) => void;
}
export type MutableFileViewerZoomState = FileViewerZoomState;
export interface FileViewerZoomController {
    readonly provider: FileViewerZoomProvider | null;
    readonly state: FileViewerZoomState;
    hasProvider(): boolean;
    refreshProvider(): FileViewerZoomProvider | null;
    observe(): void;
    clearProvider(): void;
    getState(): FileViewerZoomState;
    zoomIn(): Promise<FileViewerZoomState>;
    zoomOut(): Promise<FileViewerZoomState>;
    resetZoom(): Promise<FileViewerZoomState>;
    destroy(): void;
}
export interface FileViewerZoomControllerActionHandlers {
    hasZoomProvider(): boolean;
    refreshZoomProvider(): FileViewerZoomProvider | null;
    startZoomObserver(): FileViewerZoomState;
    stopZoomObserver(): FileViewerZoomState;
    clearZoomProvider(): FileViewerZoomState;
    getZoomState(): FileViewerZoomState;
    zoomIn(): Promise<FileViewerZoomState>;
    zoomOut(): Promise<FileViewerZoomState>;
    resetZoom(): Promise<FileViewerZoomState>;
}
export declare const cloneFileViewerZoomState: (state: FileViewerZoomState) => FileViewerZoomState;
export declare const applyFileViewerZoomState: <Target extends MutableFileViewerZoomState>(target: Target, source?: Partial<FileViewerZoomState> | null) => Target;
export declare const createFileViewerZoomChangeState: (state: FileViewerZoomState) => FileViewerZoomState;
export declare const syncFileViewerZoomControllerState: <Target extends MutableFileViewerZoomState>(target: Target, controller: Pick<FileViewerZoomController, "state">) => Target;
export declare const refreshFileViewerZoomControllerProvider: <Target extends MutableFileViewerZoomState>(target: Target, controller: Pick<FileViewerZoomController, "refreshProvider" | "state">) => FileViewerZoomProvider | null;
export declare const observeFileViewerZoomController: <Target extends MutableFileViewerZoomState>(target: Target, controller: Pick<FileViewerZoomController, "observe" | "state">) => Target;
export declare const clearFileViewerZoomControllerProvider: <Target extends MutableFileViewerZoomState>(target: Target, controller: Pick<FileViewerZoomController, "clearProvider" | "state">) => Target;
export declare const destroyFileViewerZoomController: <Target extends MutableFileViewerZoomState>(target: Target, controller: Pick<FileViewerZoomController, "destroy" | "state">) => Target;
export declare const runFileViewerZoomControllerAction: <Target extends MutableFileViewerZoomState>(target: Target, action: () => Promise<FileViewerZoomState>) => Promise<FileViewerZoomState>;
export declare const createFileViewerZoomControllerActionHandlers: <Target extends MutableFileViewerZoomState>(target: Target, controller: FileViewerZoomController) => FileViewerZoomControllerActionHandlers;
export declare const createFileViewerZoomChangeEmitter: () => {
    emit(): void;
    subscribe(listener: () => void): () => void;
};
export declare const createFileViewerZoomController: ({ root, enabled, beforeZoom, onChange, }: CreateFileViewerZoomControllerOptions) => FileViewerZoomController;
