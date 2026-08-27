import { type HTMLAttributes, type MutableRefObject } from 'react';
import { type ViewerControllerHandle, type ViewerMountOptions, type ViewerEventHandler, type ViewerState } from './controller.js';
export type { FileRef, ViewerAiOptions, ViewerApplyViewStateOptions, ViewerArchiveOptions, ViewerCadOptions, ViewerController, ViewerControllerAccessor, ViewerControllerHandle, ViewerDocxOptions, ViewerEvent, ViewerEventHandler, ViewerEventType, ViewerFetchFile, ViewerFetchInput, ViewerFitMode, ViewerFitOptions, ViewerFitResult, ViewerMountOptions, ViewerOptions, ViewerPdfOptions, ViewerPresentationOptions, ViewerSpreadsheetOptions, ViewerCoreOptions, ViewerSearchOptions, ViewerSourceInput, ViewerThemeMode, ViewerToolbarOptions, ViewerToolbarPosition, ViewerTypstOptions, ViewerUiDensity, ViewerUiOptions, ViewerViewState, ViewerWatermarkOptions, ViewerLifecycleContext, ViewerOperationContext, ViewerState, ViewerStateListener } from './controller.js';
export interface FileViewerHandle extends ViewerControllerHandle {
}
export interface FileViewerProps extends Omit<HTMLAttributes<HTMLDivElement>, 'children'>, ViewerMountOptions {
}
export declare const FileViewer: import("react").ForwardRefExoticComponent<FileViewerProps & import("react").RefAttributes<FileViewerHandle>>;
export interface UseFileViewerStateResult {
    state: ViewerState;
    onStateChange: NonNullable<ViewerMountOptions['onStateChange']>;
    resetState(): void;
}
export declare const useFileViewerState: (onEvent?: ViewerEventHandler) => UseFileViewerStateResult;
export interface UseFileViewerResult {
    ref: MutableRefObject<FileViewerHandle | null>;
    props: ViewerMountOptions;
    state: ViewerState;
    handle: ViewerControllerHandle;
    resetState(): void;
}
export declare const useFileViewer: (options?: ViewerMountOptions) => UseFileViewerResult;
export default FileViewer;
