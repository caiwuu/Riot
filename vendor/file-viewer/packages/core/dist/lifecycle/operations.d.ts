import { type FileViewerI18nInput } from '../i18n/messages';
import type { FileViewerErrorMessageFormatter } from '../viewer/state';
import { type FileViewerOriginalSourceState } from '../viewer/operations';
import type { FileRenderExportAdapter, FileViewerBeforeOperation, FileViewerFileRef, FileViewerLifecycleContext, FileViewerLifecycleHooks, FileViewerLifecyclePhase, FileViewerOperationAvailability, FileViewerOperationContext, FileViewerOperationType, FileViewerOptions, FileViewerResolvedToolbarItem, FileViewerSourceKind, FileViewerToolbarOptions, FileViewerToolbarPosition, FileViewerPublicApi, FileViewerZoomState, NormalizedFileViewerSource } from '../contracts/types';
export declare const FILE_VIEWER_LIFECYCLE_HOOKS: {
    readonly 'load-start': "onLoadStart";
    readonly 'load-complete': "onLoadComplete";
    readonly 'unload-start': "onUnloadStart";
    readonly 'unload-complete': "onUnloadComplete";
};
export declare const FILE_VIEWER_OPERATION_LABELS: {
    readonly download: "下载原始文件";
    readonly print: "打印完整渲染内容";
    readonly 'export-html': "导出渲染 HTML";
    readonly 'zoom-in': "放大预览";
    readonly 'zoom-out': "缩小预览";
    readonly 'zoom-reset': "还原预览比例";
};
export declare const FILE_VIEWER_BEFORE_OPERATION_ERROR_PREFIX = "\u64CD\u4F5C\u524D\u7F6E\u6821\u9A8C\u5931\u8D25";
export declare const FILE_VIEWER_LIFECYCLE_HOOK_ERROR_MESSAGE_PREFIX = "FileViewer";
export declare const DEFAULT_FILE_VIEWER_TOOLBAR_ORDER: readonly ["search", "zoom", "download", "print", "exportHtml", "theme"];
export interface FileViewerLifecycleComponentEmit {
    (event: 'load-start', context: FileViewerLifecycleContext): void;
    (event: 'load-complete', context: FileViewerLifecycleContext): void;
    (event: 'unload-start', context: FileViewerLifecycleContext): void;
    (event: 'unload-complete', context: FileViewerLifecycleContext): void;
}
export interface BuildFileViewerLifecycleContextInput<Source extends string = FileViewerSourceKind> {
    phase: FileViewerLifecyclePhase;
    version: number;
    source: Source;
    filename?: string;
    file?: File | null;
    url?: string;
    size?: number;
    bufferSize?: number;
    startedAt?: number;
    duration?: number;
    timestamp?: number;
    reason?: FileViewerLifecycleContext['reason'];
}
export interface BuildFileViewerLifecycleContextFromNormalizedSourceInput {
    phase: FileViewerLifecyclePhase;
    source: NormalizedFileViewerSource;
    version: number;
    startedAt?: number;
    timestamp?: number;
    reason?: FileViewerLifecycleContext['reason'];
}
export interface ResolveFileViewerLifecycleFallbackSourceInput {
    file?: FileViewerFileRef | null;
    url?: string | null;
}
export interface ResolvedFileViewerLifecycleFallbackSource {
    source: FileViewerLifecycleContext['source'];
    sourceUrl?: string;
}
export type BuiltFileViewerLifecycleContext<Source extends string = FileViewerSourceKind> = Omit<FileViewerLifecycleContext, 'source'> & {
    source: Source;
};
export type BuiltFileViewerOperationContext<Source extends string = FileViewerSourceKind> = Omit<BuiltFileViewerLifecycleContext<Source>, 'phase'> & {
    operation: FileViewerOperationType;
    label: string;
};
export type SerializedFileViewerContext<Context extends FileViewerLifecycleContext | FileViewerOperationContext> = Omit<Context, 'file'> & {
    hasFile: boolean;
};
export interface ResolveFileViewerOperationAvailabilityInput {
    extension: string;
    hasOriginalSource?: boolean;
    source?: FileViewerOriginalSourceState | null;
    renderedReady: boolean;
    hasError?: boolean;
    adapter?: FileRenderExportAdapter | null;
    zoomState: FileViewerZoomState;
}
export interface ResolveFileViewerToolbarStateInput extends ResolveFileViewerOperationAvailabilityInput {
    toolbar: FileViewerToolbarOptions;
    options?: Pick<FileViewerOptions, 'toolbar' | 'search'>;
    searchAvailable?: boolean;
    loading?: boolean;
}
export interface FileViewerToolbarState {
    operationAvailability: FileViewerOperationAvailability;
    visibleToolbar: FileViewerToolbarOptions;
    toolbarOrder: FileViewerResolvedToolbarItem[];
    showToolbar: boolean;
    toolbarPosition: FileViewerToolbarPosition;
    toolbarDisabled: boolean;
}
export interface RunFileViewerBeforeOperationInput<Context extends FileViewerOperationContext = FileViewerOperationContext> {
    context: Context;
    options?: FileViewerOptions;
    onBefore?: (context: Context) => void;
    onCancel?: (context: Context) => void;
    onError?: (error: unknown, context: Context) => void;
}
export interface ResolveFileViewerBeforeOperationErrorMessageInput {
    error: unknown;
    formatErrorMessage: FileViewerErrorMessageFormatter;
    prefix?: string;
    i18n?: FileViewerI18nInput;
}
export interface ResolveFileViewerLifecycleHookErrorMessageInput {
    context: Pick<FileViewerLifecycleContext, 'phase'>;
    prefix?: string;
}
export type FileViewerLifecycleHookErrorLogger = (message: string, error: unknown, context: FileViewerLifecycleContext) => void;
export interface ReportFileViewerLifecycleHookErrorInput extends ResolveFileViewerLifecycleHookErrorMessageInput {
    error: unknown;
    context: FileViewerLifecycleContext;
    onLogError?: FileViewerLifecycleHookErrorLogger | null;
}
export type FileViewerOperationErrorLogger = (error: unknown, context: FileViewerOperationContext) => void;
export interface ReportFileViewerOperationErrorInput {
    error: unknown;
    context: FileViewerOperationContext;
    onLogError?: FileViewerOperationErrorLogger | null;
}
export interface CreateFileViewerLifecycleActionsInput<OperationContext extends FileViewerOperationContext = FileViewerOperationContext> {
    lifecycleState: FileViewerLifecycleStateController;
    getOptions?: () => FileViewerOptions | undefined;
    onLifecycleChange?: (event: FileViewerLifecyclePhase, context: FileViewerLifecycleContext) => void;
    onLifecycleError?: (error: unknown, context: FileViewerLifecycleContext) => void;
    onOperationBefore?: (context: OperationContext) => void;
    onOperationCancel?: (context: OperationContext) => void;
    onOperationError?: (error: unknown, context: OperationContext) => void;
}
export interface FileViewerLifecycleActions<OperationContext extends FileViewerOperationContext = FileViewerOperationContext> {
    notifyLifecycle(context: FileViewerLifecycleContext): boolean;
    notifyActiveUnloadStart(reason?: FileViewerLifecycleContext['reason']): FileViewerLifecycleContext | null;
    notifyActiveUnloadComplete(context: FileViewerLifecycleContext | null, reason?: FileViewerLifecycleContext['reason']): FileViewerActiveUnloadState;
    runBeforeOperation(context: OperationContext): Promise<boolean>;
}
export interface DispatchFileViewerLifecycleEventInput<Context extends FileViewerLifecycleContext = FileViewerLifecycleContext> {
    context: Context;
    hooks?: FileViewerLifecycleHooks;
    onChange?: (event: FileViewerLifecyclePhase, context: Context) => void;
    onError?: (error: unknown, context: Context) => void;
}
export interface DispatchFileViewerOperationContextEventInput<Context extends FileViewerOperationContext = FileViewerOperationContext> {
    event: 'operation-before' | 'operation-cancel';
    context: Context;
    onChange?: (context: Context) => void;
}
export interface DispatchFileViewerOperationAvailabilityChangeInput {
    availability: FileViewerOperationAvailability;
    onChange?: (availability: FileViewerOperationAvailability) => void;
}
export interface DispatchFileViewerZoomChangeInput {
    state: FileViewerZoomState;
    onChange?: (state: FileViewerZoomState) => void;
}
export type FileViewerZoomButtonAction = keyof Pick<FileViewerZoomState, 'canZoomIn' | 'canZoomOut' | 'canReset'>;
export interface CreateFileViewerToolbarActionsInput {
    getOperationAvailability: () => FileViewerOperationAvailability;
    getToolbarDisabled?: () => boolean;
    getZoomState: () => FileViewerZoomState;
    onOperationAvailabilityChange?: (availability: FileViewerOperationAvailability) => void;
    onZoomChange?: (state: FileViewerZoomState) => void;
}
export interface FileViewerToolbarActions {
    notifyOperationAvailabilityChange(availability?: FileViewerOperationAvailability): boolean;
    notifyZoomChange(state?: FileViewerZoomState): boolean;
    isZoomButtonDisabled(action: FileViewerZoomButtonAction): boolean;
}
export interface CreateFileViewerPublicApiInput extends Omit<FileViewerPublicApi, 'getOperationAvailability'> {
    getOperationAvailability: () => FileViewerOperationAvailability;
}
export type FileViewerToolbarZoomSyncSnapshot = readonly [
    scale: FileViewerZoomState['scale'],
    label: FileViewerZoomState['label'],
    canZoomIn: FileViewerZoomState['canZoomIn'],
    canZoomOut: FileViewerZoomState['canZoomOut'],
    canReset: FileViewerZoomState['canReset']
];
export interface RunFileViewerToolbarAvailabilitySyncInput {
    toolbarActions: Pick<FileViewerToolbarActions, 'notifyOperationAvailabilityChange'>;
    availability?: FileViewerOperationAvailability;
}
export interface RunFileViewerToolbarZoomSyncInput {
    toolbarActions: Pick<FileViewerToolbarActions, 'notifyZoomChange'>;
    state?: FileViewerZoomState;
}
export interface FileViewerToolbarControllerActionHandlers {
    resolveToolbarState(): FileViewerToolbarState;
    createZoomSyncSnapshot(): FileViewerToolbarZoomSyncSnapshot;
    syncOperationAvailability(availability?: FileViewerOperationAvailability): boolean;
    syncZoomChange(state?: FileViewerZoomState): boolean;
    isZoomButtonDisabled(action: FileViewerZoomButtonAction): boolean;
}
export interface CreateFileViewerToolbarControllerActionHandlersInput {
    getAdapter?: () => FileRenderExportAdapter | null | undefined;
    getBuffer?: () => ArrayBuffer | null | undefined;
    getExtension: () => string;
    getFile?: () => File | null | undefined;
    getHasError?: () => boolean;
    getLoading?: () => boolean;
    getOptions?: () => FileViewerOptions | undefined;
    getSearchAvailable?: () => boolean;
    getSourceUrl?: () => string | null | undefined;
    getToolbar: () => FileViewerToolbarOptions;
    getRenderedReady: () => boolean;
    getZoomState: () => FileViewerZoomState;
    zoomSyncState?: FileViewerZoomState;
    onOperationAvailabilityChange?: (availability: FileViewerOperationAvailability) => void;
    onZoomChange?: (state: FileViewerZoomState) => void;
}
export interface FileViewerLifecycleStateController {
    markLoadStarted(version: number, timestamp?: number): void;
    clearLoadStarted(version: number): void;
    getLoadStartedAt(version: number): number | undefined;
    getActiveDocumentContext(): FileViewerLifecycleContext | null;
    setActiveDocumentContext(context: FileViewerLifecycleContext): void;
    clearActiveDocumentContext(): void;
    buildActiveUnloadContext(phase: Extract<FileViewerLifecyclePhase, 'unload-start' | 'unload-complete'>, context: FileViewerLifecycleContext | null, reason?: FileViewerLifecycleContext['reason'], timestamp?: number): FileViewerLifecycleContext | null;
}
export interface BuildFileViewerOperationContextFromLifecycleStateInput {
    operation: FileViewerOperationType;
    lifecycleState: Pick<FileViewerLifecycleStateController, 'getActiveDocumentContext' | 'getLoadStartedAt'>;
    version: number;
    filename?: string;
    bufferSize?: number;
    currentFile?: File | null;
    fallbackFile?: FileViewerFileRef | null;
    fallbackUrl?: string | null;
    timestamp?: number;
    lifecycleTimestamp?: number;
    i18n?: FileViewerI18nInput;
}
export interface RunFileViewerActiveUnloadStartInput {
    lifecycleState: Pick<FileViewerLifecycleStateController, 'getActiveDocumentContext' | 'buildActiveUnloadContext'>;
    reason?: FileViewerLifecycleContext['reason'];
    onLifecycle?: (context: FileViewerLifecycleContext) => void;
}
export interface RunFileViewerActiveUnloadCompleteInput {
    lifecycleState: Pick<FileViewerLifecycleStateController, 'buildActiveUnloadContext'>;
    context?: FileViewerLifecycleContext | null;
    reason?: FileViewerLifecycleContext['reason'];
    onLifecycle?: (context: FileViewerLifecycleContext) => void;
}
export interface FileViewerActiveUnloadState {
    reason: FileViewerLifecycleContext['reason'];
    context: FileViewerLifecycleContext | null;
    unloadContext: FileViewerLifecycleContext | null;
}
export declare const buildFileViewerLifecycleContext: <Source extends string = FileViewerSourceKind>({ phase, version, source, filename, file, url, size, bufferSize, startedAt, duration, timestamp, reason, }: BuildFileViewerLifecycleContextInput<Source>) => BuiltFileViewerLifecycleContext<Source>;
export declare const buildFileViewerLifecycleContextFromNormalizedSource: ({ phase, source, version, startedAt, timestamp, reason, }: BuildFileViewerLifecycleContextFromNormalizedSourceInput) => FileViewerLifecycleContext;
export declare const resolveFileViewerLifecycleFallbackSource: ({ file, url, }?: ResolveFileViewerLifecycleFallbackSourceInput) => ResolvedFileViewerLifecycleFallbackSource;
export declare const createFileViewerLifecycleStateController: () => FileViewerLifecycleStateController;
export declare const buildFileViewerOperationContext: <Source extends string = FileViewerSourceKind>(operation: FileViewerOperationType, lifecycleContext: BuiltFileViewerLifecycleContext<Source>, timestamp?: number, i18n?: FileViewerI18nInput) => BuiltFileViewerOperationContext<Source>;
export declare const buildFileViewerOperationContextFromLifecycleState: ({ operation, lifecycleState, version, filename, bufferSize, currentFile, fallbackFile, fallbackUrl, timestamp, lifecycleTimestamp, i18n, }: BuildFileViewerOperationContextFromLifecycleStateInput) => FileViewerOperationContext;
export declare const emitFileViewerComponentLifecycleEvent: (emit: FileViewerLifecycleComponentEmit, context: FileViewerLifecycleContext) => void;
export declare const resolveFileViewerBeforeOperationErrorMessage: ({ error, formatErrorMessage, prefix, i18n, }: ResolveFileViewerBeforeOperationErrorMessageInput) => string;
export declare const resolveFileViewerLifecycleHookErrorMessage: ({ context, prefix, }: ResolveFileViewerLifecycleHookErrorMessageInput) => string;
export declare const DEFAULT_FILE_VIEWER_LIFECYCLE_HOOK_ERROR_LOGGER: FileViewerLifecycleHookErrorLogger;
export declare const reportFileViewerLifecycleHookError: ({ error, context, onLogError, prefix, }: ReportFileViewerLifecycleHookErrorInput) => string;
export declare const DEFAULT_FILE_VIEWER_OPERATION_ERROR_LOGGER: FileViewerOperationErrorLogger;
export declare const reportFileViewerOperationError: ({ error, context, onLogError, }: ReportFileViewerOperationErrorInput) => unknown;
export declare const runFileViewerActiveUnloadStart: ({ lifecycleState, reason, onLifecycle, }: RunFileViewerActiveUnloadStartInput) => FileViewerActiveUnloadState;
export declare const runFileViewerActiveUnloadComplete: ({ lifecycleState, context, reason, onLifecycle, }: RunFileViewerActiveUnloadCompleteInput) => FileViewerActiveUnloadState;
export declare const getFileViewerLifecycleHookName: (phase: FileViewerLifecyclePhase) => "onLoadStart" | "onLoadComplete" | "onUnloadStart" | "onUnloadComplete";
export declare const runFileViewerLifecycleHook: <Context extends FileViewerLifecycleContext>(context: Context, hooks?: FileViewerLifecycleHooks, onError?: (error: unknown, context: Context) => void) => Promise<void>;
export declare const getFileViewerBeforeOperationHooks: (options: FileViewerOptions | undefined, operation: FileViewerOperationType) => Array<FileViewerBeforeOperation | undefined>;
export declare const isFileViewerToolbarOperationPermitted: (toolbar: FileViewerOptions["toolbar"] | undefined, operation: FileViewerOperationType) => boolean;
export declare const resolveFileViewerToolbarOrder: (toolbar: Pick<FileViewerToolbarOptions, "order"> | undefined) => FileViewerResolvedToolbarItem[];
export declare const runFileViewerBeforeOperation: <Context extends FileViewerOperationContext>({ context, options, onBefore, onCancel, onError, }: RunFileViewerBeforeOperationInput<Context>) => Promise<boolean>;
export declare const serializeFileViewerContext: <Context extends FileViewerLifecycleContext | FileViewerOperationContext>(context: Context) => SerializedFileViewerContext<Context>;
export declare const dispatchFileViewerLifecycleEvent: <Context extends FileViewerLifecycleContext>({ context, hooks, onChange, onError, }: DispatchFileViewerLifecycleEventInput<Context>) => boolean;
export declare const dispatchFileViewerOperationContextEvent: <Context extends FileViewerOperationContext>({ context, onChange, }: DispatchFileViewerOperationContextEventInput<Context>) => boolean;
export declare const createFileViewerLifecycleActions: <OperationContext extends FileViewerOperationContext = FileViewerOperationContext>({ lifecycleState, getOptions, onLifecycleChange, onLifecycleError, onOperationBefore, onOperationCancel, onOperationError, }: CreateFileViewerLifecycleActionsInput<OperationContext>) => FileViewerLifecycleActions<OperationContext>;
export declare const dispatchFileViewerOperationAvailabilityChange: ({ availability, onChange, }: DispatchFileViewerOperationAvailabilityChangeInput) => boolean;
export declare const dispatchFileViewerZoomChange: ({ state, onChange, }: DispatchFileViewerZoomChangeInput) => boolean;
export declare const createFileViewerToolbarActions: ({ getOperationAvailability, getToolbarDisabled, getZoomState, onOperationAvailabilityChange, onZoomChange, }: CreateFileViewerToolbarActionsInput) => FileViewerToolbarActions;
export declare const createFileViewerPublicApi: ({ getOperationAvailability, ...api }: CreateFileViewerPublicApiInput) => FileViewerPublicApi;
export declare const createFileViewerToolbarZoomSyncSnapshot: (state: FileViewerZoomState) => FileViewerToolbarZoomSyncSnapshot;
export declare const runFileViewerToolbarAvailabilitySync: ({ toolbarActions, availability, }: RunFileViewerToolbarAvailabilitySyncInput) => boolean;
export declare const runFileViewerToolbarZoomSync: ({ toolbarActions, state, }: RunFileViewerToolbarZoomSyncInput) => boolean;
export declare const createFileViewerToolbarControllerActionHandlers: ({ getAdapter, getBuffer, getExtension, getFile, getHasError, getLoading, getOptions, getSearchAvailable, getSourceUrl, getToolbar, getRenderedReady, getZoomState, zoomSyncState, onOperationAvailabilityChange, onZoomChange, }: CreateFileViewerToolbarControllerActionHandlersInput) => FileViewerToolbarControllerActionHandlers;
export declare const normalizeFileViewerToolbar: (options: Pick<FileViewerOptions, "toolbar"> | undefined) => FileViewerToolbarOptions;
export declare const resolveFileViewerOperationAvailability: ({ extension, hasOriginalSource, renderedReady, hasError, adapter, source, zoomState, }: ResolveFileViewerOperationAvailabilityInput) => FileViewerOperationAvailability;
export declare const cloneFileViewerOperationAvailability: (availability: FileViewerOperationAvailability) => FileViewerOperationAvailability;
export declare const resolveVisibleFileViewerToolbar: (toolbar: FileViewerToolbarOptions, availability: FileViewerOperationAvailability, searchEnabled?: boolean) => FileViewerToolbarOptions;
export declare const hasVisibleFileViewerToolbarActions: (toolbar: FileViewerToolbarOptions) => boolean;
export declare const isFileViewerZoomButtonDisabled: ({ toolbarDisabled, availability, zoomState, action, }: {
    toolbarDisabled?: boolean;
    availability: FileViewerOperationAvailability;
    zoomState: FileViewerZoomState;
    action: keyof Pick<FileViewerZoomState, "canZoomIn" | "canZoomOut" | "canReset">;
}) => boolean;
export declare const isFileViewerToolbarDisabled: ({ loading, hasError, }: {
    loading?: boolean;
    hasError?: boolean;
}) => boolean;
export declare const resolveFileViewerToolbarState: ({ toolbar, options, searchAvailable, loading, ...availabilityInput }: ResolveFileViewerToolbarStateInput) => FileViewerToolbarState;
export declare const resolveFileViewerToolbarPosition: (options: Pick<FileViewerOptions, "toolbar"> | undefined, extension: string) => FileViewerToolbarPosition;
