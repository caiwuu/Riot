import type { FileViewerSearchProvider, FileViewerViewStateProvider, FileViewerZoomProvider } from '../../../contracts/types';
export interface FileViewerSearchProviderHost extends HTMLElement {
    __flyfishViewerSearchProvider?: FileViewerSearchProvider;
}
export interface FileViewerZoomProviderHost extends HTMLElement {
    __flyfishViewerZoomProvider?: FileViewerZoomProvider;
}
export interface FileViewerViewStateProviderHost extends HTMLElement {
    __flyfishViewerViewStateProvider?: FileViewerViewStateProvider;
}
type FileViewerProviderSearchRoot = HTMLElement | ShadowRoot | null | undefined;
export declare const registerFileViewerSearchProvider: (host: HTMLElement, provider: FileViewerSearchProvider) => void;
export declare const unregisterFileViewerSearchProvider: (host: HTMLElement | null | undefined) => void;
export declare const findFileViewerSearchProvider: (root: FileViewerProviderSearchRoot) => FileViewerSearchProvider | null;
export declare const registerFileViewerZoomProvider: (host: HTMLElement, provider: FileViewerZoomProvider) => void;
export declare const unregisterFileViewerZoomProvider: (host: HTMLElement | null | undefined) => void;
export declare const findFileViewerZoomProvider: (root: FileViewerProviderSearchRoot) => FileViewerZoomProvider | null;
export declare const registerFileViewerViewStateProvider: (host: HTMLElement, provider: FileViewerViewStateProvider) => void;
export declare const unregisterFileViewerViewStateProvider: (host: HTMLElement | null | undefined) => void;
export declare const findFileViewerViewStateProvider: (root: FileViewerProviderSearchRoot) => FileViewerViewStateProvider | null;
export {};
