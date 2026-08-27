import type { FileViewerDocumentAnchor, FileViewerSearchMatch, FileViewerSearchOptions, FileViewerSearchState } from '../../contracts/types';
export interface CreateFileViewerDomSearchControllerOptions {
    root: () => HTMLElement | null | undefined;
    options?: () => boolean | FileViewerSearchOptions | undefined;
    waitForDomUpdate?: () => Promise<void> | void;
    preferredScrollContainer?: () => HTMLElement | null | undefined;
}
export interface FileViewerInternalSearchMatch extends FileViewerSearchMatch {
    element: HTMLElement;
}
export interface FileViewerDomSearchController {
    readonly anchors: FileViewerDocumentAnchor[];
    readonly state: FileViewerSearchState;
    getInternalMatches(): FileViewerInternalSearchMatch[];
    observe(): void;
    refreshAnchors(): Promise<FileViewerDocumentAnchor[]>;
    search(query: string): Promise<FileViewerSearchState>;
    next(): Promise<FileViewerSearchState>;
    previous(): Promise<FileViewerSearchState>;
    clear(): Promise<FileViewerSearchState>;
    destroy(): void;
}
export interface FileViewerDocumentAnchorsTarget {
    value: FileViewerDocumentAnchor[];
}
export interface FileViewerDomSearchControllerStateTarget {
    anchors: FileViewerDocumentAnchorsTarget;
    state: MutableFileViewerSearchState;
}
export interface FileViewerDomSearchControllerActionHandlers {
    observe(): FileViewerSearchState;
    refreshAnchors(): Promise<FileViewerDocumentAnchor[]>;
    search(query: string): Promise<FileViewerSearchState>;
    next(): Promise<FileViewerSearchState>;
    previous(): Promise<FileViewerSearchState>;
    clear(): Promise<FileViewerSearchState>;
    destroy(): FileViewerSearchState;
}
export declare const DEFAULT_FILE_VIEWER_SEARCH_MATCH_CLASS = "flyfish-search-match";
export declare const DEFAULT_FILE_VIEWER_SEARCH_ACTIVE_CLASS = "flyfish-search-match--active";
export declare const DEFAULT_FILE_VIEWER_SEARCH_MAX_MATCHES = 1000;
export declare const cloneFileViewerSearchState: (state: FileViewerSearchState) => FileViewerSearchState;
export type MutableFileViewerSearchState = FileViewerSearchState;
export declare const applyFileViewerSearchState: <Target extends MutableFileViewerSearchState>(target: Target, source: FileViewerSearchState) => Target;
export declare const syncFileViewerDomSearchControllerState: <Target extends FileViewerDomSearchControllerStateTarget>(target: Target, controller: Pick<FileViewerDomSearchController, "anchors" | "state">) => FileViewerSearchState;
export declare const observeFileViewerDomSearchController: <Target extends FileViewerDomSearchControllerStateTarget>(target: Target, controller: Pick<FileViewerDomSearchController, "observe" | "anchors" | "state">) => FileViewerSearchState;
export declare const runFileViewerDomSearchControllerAction: <Target extends FileViewerDomSearchControllerStateTarget, Result>(target: Target, controller: Pick<FileViewerDomSearchController, "anchors" | "state">, action: () => Result | Promise<Result>) => Promise<FileViewerSearchState>;
export declare const destroyFileViewerDomSearchController: <Target extends FileViewerDomSearchControllerStateTarget>(target: Target, controller: Pick<FileViewerDomSearchController, "destroy" | "anchors" | "state">) => FileViewerSearchState;
export declare const createFileViewerDomSearchControllerActionHandlers: <Target extends FileViewerDomSearchControllerStateTarget>(target: Target, controller: FileViewerDomSearchController) => FileViewerDomSearchControllerActionHandlers;
export declare const createFileViewerDomSearchController: ({ root, options, waitForDomUpdate, preferredScrollContainer, }: CreateFileViewerDomSearchControllerOptions) => FileViewerDomSearchController;
