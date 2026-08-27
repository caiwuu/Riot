import type { FileViewerFileRef, FileViewerOptions, FileViewerThemeMode, FileViewerToolbarOptions, FileViewerUiDensity } from '../contracts/types';
export interface ResolveFileViewerPresentationStateInput {
    filename?: string;
    file?: FileViewerFileRef;
    url?: string;
    options?: FileViewerOptions;
}
export interface FileViewerPresentationState {
    displayFilename: string;
    extension: string;
    toolbar: FileViewerToolbarOptions;
    theme: FileViewerThemeMode;
    density: FileViewerUiDensity;
}
export declare const resolveFileViewerPresentationState: ({ filename, file, url, options, }: ResolveFileViewerPresentationStateInput) => FileViewerPresentationState;
