import { type FileViewerDomSearchControllerStateTarget } from './search';
import type { FileViewerAiOptions, FileViewerDocumentAnchor, FileViewerDocumentChunk, FileViewerSearchOptions, FileViewerSearchState } from '../../contracts/types';
export interface ResolveFileViewerLocationChangeAnchorInput {
    root: HTMLElement | null | undefined;
    anchors: FileViewerDocumentAnchor[];
}
export interface CreateFileViewerDocumentChangeSnapshotInput extends ResolveFileViewerLocationChangeAnchorInput {
    searchState: FileViewerSearchState;
}
export interface FileViewerDocumentChangeSnapshot {
    searchState: FileViewerSearchState;
    locationAnchor: FileViewerDocumentAnchor | null;
}
export interface DispatchFileViewerSearchChangeInput {
    state: FileViewerSearchState;
    onChange?: (state: FileViewerSearchState) => void;
}
export interface DispatchFileViewerLocationChangeInput {
    anchor: FileViewerDocumentAnchor | null;
    onChange?: (anchor: FileViewerDocumentAnchor | null) => void;
}
export interface FileViewerDocumentFeatureActionOptions {
    notify?: boolean;
}
export interface FileViewerDocumentFeatureSearchController {
    getAnchors(): FileViewerDocumentAnchor[];
    getSearchState(): FileViewerSearchState;
    observe(): void;
    refreshAnchors(): Promise<FileViewerDocumentAnchor[]>;
    search(query: string): Promise<unknown>;
    clear(): Promise<unknown>;
    next(): Promise<unknown>;
    previous(): Promise<unknown>;
}
export interface CreateFileViewerDocumentFeatureActionsInput {
    root: () => HTMLElement | null | undefined;
    searchController: FileViewerDocumentFeatureSearchController;
    getAiOptions?: () => boolean | FileViewerAiOptions | undefined;
    onSearchChange?: (state: FileViewerSearchState) => void;
    onLocationChange?: (anchor: FileViewerDocumentAnchor | null) => void;
}
export interface FileViewerDocumentFeatureActions {
    refreshDocumentIndex(options?: FileViewerDocumentFeatureActionOptions): Promise<FileViewerDocumentAnchor[]>;
    clearDocumentState(): Promise<FileViewerSearchState>;
    getScrollContainer(): HTMLElement | null;
    searchDocument(query: string): Promise<FileViewerSearchState>;
    clearDocumentSearch(): Promise<FileViewerSearchState>;
    nextSearchResult(): Promise<FileViewerSearchState>;
    previousSearchResult(): Promise<FileViewerSearchState>;
    getSearchState(): FileViewerSearchState;
    collectDocumentAnchors(options?: FileViewerDocumentFeatureActionOptions): Promise<FileViewerDocumentAnchor[]>;
    getCurrentDocumentAnchor(): FileViewerDocumentAnchor | null;
    scrollToLoadedAnchor(anchor: FileViewerDocumentAnchor | string | number | null | undefined, options?: FileViewerDocumentFeatureActionOptions): boolean;
    scrollToAnchor(anchor: FileViewerDocumentAnchor | string | number | null | undefined, options?: FileViewerDocumentFeatureActionOptions): Promise<boolean>;
    scrollToLine(line: number, options?: FileViewerDocumentFeatureActionOptions): Promise<boolean>;
    getDocumentTextChunks(options?: boolean | FileViewerAiOptions): FileViewerDocumentChunk[];
}
export interface CreateFileViewerDocumentFeatureControllerActionHandlersInput extends Omit<CreateFileViewerDocumentFeatureActionsInput, 'searchController'> {
    searchTarget: FileViewerDomSearchControllerStateTarget;
    searchOptions?: () => boolean | FileViewerSearchOptions | undefined;
    waitForDomUpdate?: () => Promise<void> | void;
    preferredScrollContainer?: () => HTMLElement | null | undefined;
}
export interface FileViewerDocumentFeatureControllerActionHandlers extends FileViewerDocumentFeatureActions {
    destroyDocumentFeatures(): FileViewerSearchState;
}
export declare const createFileViewerSearchChangeState: (state: FileViewerSearchState) => FileViewerSearchState;
export declare const resolveFileViewerLocationChangeAnchor: ({ root, anchors, }: ResolveFileViewerLocationChangeAnchorInput) => FileViewerDocumentAnchor | null;
export declare const createFileViewerDocumentChangeSnapshot: ({ root, anchors, searchState, }: CreateFileViewerDocumentChangeSnapshotInput) => FileViewerDocumentChangeSnapshot;
export declare const createFileViewerDocumentFeatureControllerActionHandlers: ({ root, searchTarget, searchOptions, waitForDomUpdate, preferredScrollContainer, getAiOptions, onSearchChange, onLocationChange, }: CreateFileViewerDocumentFeatureControllerActionHandlersInput) => FileViewerDocumentFeatureControllerActionHandlers;
export declare const dispatchFileViewerSearchChange: ({ state, onChange, }: DispatchFileViewerSearchChangeInput) => boolean;
export declare const dispatchFileViewerLocationChange: ({ anchor, onChange, }: DispatchFileViewerLocationChangeInput) => boolean;
export declare const createFileViewerDocumentFeatureActions: ({ root, searchController, getAiOptions, onSearchChange, onLocationChange, }: CreateFileViewerDocumentFeatureActionsInput) => FileViewerDocumentFeatureActions;
