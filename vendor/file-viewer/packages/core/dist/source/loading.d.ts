import type { FileViewerFileRef, FileViewerLifecycleContext, FileViewerPdfOptions } from '../contracts/types';
import { type FileViewerLifecycleStateController } from '../lifecycle/operations';
import { type FileViewerErrorMessageFormatter } from '../viewer/state';
import type { FileViewerI18nInput } from '../i18n/messages';
export declare const DEFAULT_PDF_RANGE_CHUNK_SIZE: number;
export declare const DEFAULT_FILE_VIEWER_STREAMING_PDF_FILENAME = "preview.pdf";
export declare const FILE_VIEWER_REMOTE_MISSING_DATA_ERROR_MESSAGE = "\u6587\u4EF6\u4E0B\u8F7D\u5931\u8D25";
export declare const FILE_VIEWER_PREVIEW_LOAD_ERROR_PREFIXES: {
    readonly local: "读取文件异常";
    readonly load: "加载文件异常";
    readonly stream: "加载 PDF 流式预览异常";
};
export type FileViewerPreviewLoadErrorKind = 'local' | FileViewerRemoteFilePreviewErrorKind;
export type FileViewerPreviewLoadErrorPrefixes = Partial<Record<FileViewerPreviewLoadErrorKind, string>>;
export interface ResolveFileViewerPreviewLoadErrorMessageInput {
    kind: FileViewerPreviewLoadErrorKind;
    error: unknown;
    formatErrorMessage: FileViewerErrorMessageFormatter;
    prefixes?: FileViewerPreviewLoadErrorPrefixes;
    i18n?: FileViewerI18nInput;
}
export interface ResolveFileViewerMissingRemoteDataErrorMessageInput {
    message?: string;
    i18n?: FileViewerI18nInput;
}
export type FileViewerPreviewLoadErrorLogger = (error: unknown) => void;
export interface ReportFileViewerPreviewLoadErrorInput extends ResolveFileViewerPreviewLoadErrorMessageInput {
    onLogError?: FileViewerPreviewLoadErrorLogger | null;
    onErrorMessage?: (message: string) => void;
}
export interface ReportFileViewerMissingRemoteDataInput extends ResolveFileViewerMissingRemoteDataErrorMessageInput {
    onErrorMessage?: (message: string) => void;
}
export interface FileViewerRequestController {
    readonly version: number;
    createVersion(): number;
    isCurrent(version: number): boolean;
    createAbortController(): AbortController | null;
    clearAbortController(controller: AbortController | null): void;
    abort(): void;
}
export interface FileViewerRequestScope {
    requestController: FileViewerRequestController;
    getCurrentVersion(): number;
    isCurrentRequest(version: number): boolean;
}
export interface ResolveFileViewerPreviewRequestReasonInput {
    file?: FileViewerFileRef | null;
    url?: string | null;
}
export interface CommitFileViewerPreviewRequestStartStateInput {
    reason?: FileViewerLifecycleContext['reason'];
    requestController: Pick<FileViewerRequestController, 'createVersion'>;
    previewTarget: MutableFileViewerPreviewRequestState;
    onClearRenderedContent?: (reason?: FileViewerLifecycleContext['reason']) => void;
    onClearError?: () => void;
}
export interface FileViewerEmptyPreviewState {
    filename: '';
    file: null;
    buffer: null;
    sourceUrl: null;
    renderedReady: false;
    progressiveReady: false;
}
export type FileViewerPreviewRequestResetState = Pick<FileViewerEmptyPreviewState, 'file' | 'buffer' | 'sourceUrl' | 'progressiveReady'>;
export interface MutableFileViewerPreviewRequestState {
    file: File | null;
    buffer: ArrayBuffer | null;
    sourceUrl: string | null;
    progressiveReady: boolean;
}
export interface MutableFileViewerPreviewState extends MutableFileViewerPreviewRequestState {
    filename: string;
    renderedReady: boolean;
}
export interface FileViewerMutableAccessor<Value> {
    get(): Value;
    set(value: Value): void;
}
export interface CreateFileViewerPreviewStateTargetInput {
    filename: FileViewerMutableAccessor<string>;
    file: FileViewerMutableAccessor<File | null>;
    buffer: FileViewerMutableAccessor<ArrayBuffer | null>;
    sourceUrl: FileViewerMutableAccessor<string | null>;
    renderedReady: FileViewerMutableAccessor<boolean>;
    progressiveReady: FileViewerMutableAccessor<boolean>;
}
export interface CommitFileViewerEmptyPreviewResetStateInput {
    previewTarget: MutableFileViewerPreviewState;
    state?: FileViewerEmptyPreviewState;
    reason?: FileViewerLifecycleContext['reason'];
    onClearRenderedContent?: (reason?: FileViewerLifecycleContext['reason']) => void;
    onResetLoading?: () => void;
}
export interface FileViewerReadPreviewState {
    filename: string;
    file: File;
    buffer: ArrayBuffer;
    sourceUrl: string | null;
}
export interface FileViewerFileRefSourcePlan {
    file: File;
    filename: string;
}
export interface ResolveFileViewerFileRefSourcePlanInput {
    source: FileViewerFileRef;
    currentFilename?: string;
    fallbackFilename?: string;
}
export interface CreateFileViewerReadPreviewStateInput {
    file: File;
    buffer: ArrayBuffer;
    sourceUrl?: string | null;
    fallbackFilename?: string;
}
export type MutableFileViewerReadPreviewState = Pick<MutableFileViewerPreviewState, 'filename' | 'file' | 'buffer' | 'sourceUrl'>;
export type MutableFileViewerPreviewSourceUrlState = Pick<MutableFileViewerPreviewState, 'sourceUrl'>;
export type MutableFileViewerPreviewFilenameState = Pick<MutableFileViewerPreviewState, 'filename'>;
export type FileViewerRenderReadinessState = Pick<MutableFileViewerPreviewState, 'renderedReady' | 'progressiveReady'>;
export type MutableFileViewerRenderReadinessState = FileViewerRenderReadinessState;
export interface FileViewerLoadStartState {
    loadingMessage: string;
    lifecycleContext: FileViewerLifecycleContext;
}
export interface CreateFileViewerLoadStartStateInput {
    version: number;
    source: FileViewerLifecycleContext['source'];
    filename?: string;
    file?: File | null;
    sourceUrl?: string | null;
    bufferSize?: number;
    loadingMessage?: string;
    i18n?: FileViewerI18nInput;
    timestamp?: number;
}
export interface CommitFileViewerLoadStartStateInput {
    version: number;
    filename?: string;
    fallbackFilename?: string;
    filenameTarget?: MutableFileViewerPreviewFilenameState;
    buildState: () => FileViewerLoadStartState;
    onMarkLoadStarted?: (version: number) => void;
    onLifecycle?: (context: FileViewerLifecycleContext) => void;
    onStartLoading?: (message: string) => void;
}
export interface FileViewerRenderCompleteState {
    readiness: FileViewerRenderReadinessState;
    lifecycleContext: FileViewerLifecycleContext;
}
export interface CreateFileViewerRenderCompleteStateInput {
    version: number;
    source: FileViewerLifecycleContext['source'];
    filename?: string;
    file?: File | null;
    sourceUrl?: string | null;
    bufferSize?: number;
    startedAt?: number;
    timestamp?: number;
    lifecycleState?: Pick<FileViewerLifecycleStateController, 'getLoadStartedAt'>;
}
export interface CommitFileViewerRenderCompleteStateInput<Session = unknown> {
    version: number;
    session?: Session | null;
    buildState: () => FileViewerRenderCompleteState;
    readinessTarget: MutableFileViewerRenderReadinessState;
    onSession?: (session: Session | null) => void;
    onActiveDocumentContext?: (context: FileViewerLifecycleContext) => void;
    onLifecycle?: (context: FileViewerLifecycleContext) => void;
    onClearLoadStarted?: (version: number) => void;
}
export interface RunFileViewerReadAndRenderFileInput<Session = unknown> {
    file: File;
    version: number;
    source?: FileViewerLifecycleContext['source'];
    sourceUrl?: string;
    fallbackFilename?: string;
    previewTarget: MutableFileViewerReadPreviewState & MutableFileViewerRenderReadinessState;
    isCurrent: (version: number) => boolean;
    mountRenderedContent: (buffer: ArrayBuffer, file: File, version: number, sourceUrl?: string) => Promise<Session | undefined>;
    destroyRenderSession?: (session?: Session | null) => void;
    buildRenderCompleteState: (input: {
        version: number;
        source: FileViewerLifecycleContext['source'];
        file?: File | null;
        sourceUrl?: string | null;
    }) => FileViewerRenderCompleteState;
    onSession?: (session: Session | null) => void;
    onActiveDocumentContext?: (context: FileViewerLifecycleContext) => void;
    onLifecycle?: (context: FileViewerLifecycleContext) => void;
    onClearLoadStarted?: (version: number) => void;
}
export type FileViewerReadAndRenderFileState<Session = unknown> = {
    readonly stale: true;
    readonly buffer: ArrayBuffer | null;
    readonly session: Session | null | undefined;
    readonly complete: null;
} | {
    readonly stale: false;
    readonly buffer: ArrayBuffer;
    readonly session: Session | undefined;
    readonly complete: FileViewerRenderCompleteState;
};
export interface RunFileViewerStreamingPdfPreviewInput<Session = unknown> {
    url: string;
    version: number;
    filename?: string;
    previewTarget: MutableFileViewerPreviewSourceUrlState & MutableFileViewerRenderReadinessState;
    isCurrent: (version: number) => boolean;
    mountRenderedContent: (buffer: ArrayBuffer, file: File, version: number, sourceUrl?: string, streamUrl?: string) => Promise<Session | undefined>;
    destroyRenderSession?: (session?: Session | null) => void;
    buildRenderCompleteState: (input: {
        version: number;
        source: 'url';
        sourceUrl?: string | null;
    }) => FileViewerRenderCompleteState;
    loadingMessage?: string;
    i18n?: FileViewerI18nInput;
    onStartLoading?: (message: string) => void;
    onSession?: (session: Session | null) => void;
    onActiveDocumentContext?: (context: FileViewerLifecycleContext) => void;
    onLifecycle?: (context: FileViewerLifecycleContext) => void;
    onClearLoadStarted?: (version: number) => void;
    onStopLoading?: () => void;
    onError?: (error: unknown) => void;
}
export type FileViewerStreamingPdfPreviewState<Session = unknown> = {
    readonly status: 'ready';
    readonly placeholderFile: File;
    readonly session: Session | undefined;
    readonly complete: FileViewerRenderCompleteState;
    readonly error: null;
} | {
    readonly status: 'stale';
    readonly placeholderFile: File | null;
    readonly session: Session | null | undefined;
    readonly complete: null;
    readonly error: null;
} | {
    readonly status: 'error';
    readonly placeholderFile: File | null;
    readonly session: null;
    readonly complete: null;
    readonly error: unknown;
};
export interface RunFileViewerLocalFilePreviewInput<Session = unknown> {
    source: FileViewerFileRef;
    version: number;
    currentFilename?: string;
    fallbackFilename?: string;
    previewTarget: MutableFileViewerPreviewState;
    isCurrent: (version: number) => boolean;
    mountRenderedContent: (buffer: ArrayBuffer, file: File, version: number, sourceUrl?: string) => Promise<Session | undefined>;
    destroyRenderSession?: (session?: Session | null) => void;
    buildLoadStartState: (input: {
        version: number;
        source: 'file';
        file: File;
    }) => FileViewerLoadStartState;
    buildRenderCompleteState: (input: {
        version: number;
        source: 'file';
        file: File;
    }) => FileViewerRenderCompleteState;
    onMarkLoadStarted?: (version: number) => void;
    onStartLoading?: (message: string) => void;
    onSession?: (session: Session | null) => void;
    onActiveDocumentContext?: (context: FileViewerLifecycleContext) => void;
    onLifecycle?: (context: FileViewerLifecycleContext) => void;
    onClearLoadStarted?: (version: number) => void;
    onStopLoading?: () => void;
    onError?: (error: unknown) => void;
}
export type FileViewerLocalFilePreviewState<Session = unknown> = {
    readonly status: 'ready';
    readonly source: FileViewerFileRefSourcePlan;
    readonly read: Extract<FileViewerReadAndRenderFileState<Session>, {
        stale: false;
    }>;
    readonly error: null;
} | {
    readonly status: 'stale';
    readonly source: FileViewerFileRefSourcePlan | null;
    readonly read: FileViewerReadAndRenderFileState<Session> | null;
    readonly error: null;
} | {
    readonly status: 'error';
    readonly source: FileViewerFileRefSourcePlan;
    readonly read: null;
    readonly error: unknown;
};
export interface FileViewerRemoteFileDownloadInput {
    url: string;
    signal?: AbortSignal;
}
export type FileViewerRemoteFilePreviewErrorKind = 'stream' | 'load';
export declare const resolveFileViewerPreviewLoadErrorMessage: ({ kind, error, formatErrorMessage, prefixes, i18n, }: ResolveFileViewerPreviewLoadErrorMessageInput) => string;
export declare const resolveFileViewerMissingRemoteDataErrorMessage: ({ message, i18n, }?: ResolveFileViewerMissingRemoteDataErrorMessageInput) => string;
export declare const DEFAULT_FILE_VIEWER_PREVIEW_LOAD_ERROR_LOGGER: FileViewerPreviewLoadErrorLogger;
export declare const reportFileViewerPreviewLoadError: ({ onLogError, onErrorMessage, ...messageInput }: ReportFileViewerPreviewLoadErrorInput) => string;
export declare const reportFileViewerMissingRemoteData: ({ onErrorMessage, ...messageInput }?: ReportFileViewerMissingRemoteDataInput) => string;
export interface RunFileViewerRemoteFilePreviewInput<Session = unknown> {
    url: string;
    version: number;
    pageHref?: string;
    streaming?: FileViewerPdfOptions['streaming'];
    previewTarget: MutableFileViewerPreviewState;
    requestController: Pick<FileViewerRequestController, 'createAbortController' | 'clearAbortController'>;
    isCurrent: (version: number) => boolean;
    downloadFile: (input: FileViewerRemoteFileDownloadInput) => Promise<FileViewerFileRef | null | undefined>;
    mountRenderedContent: (buffer: ArrayBuffer, file: File, version: number, sourceUrl?: string, streamUrl?: string) => Promise<Session | undefined>;
    destroyRenderSession?: (session?: Session | null) => void;
    buildLoadStartState: (input: {
        version: number;
        source: 'url';
        sourceUrl: string;
    }) => FileViewerLoadStartState;
    buildRenderCompleteState: (input: {
        version: number;
        source: 'url';
        file?: File | null;
        sourceUrl?: string | null;
    }) => FileViewerRenderCompleteState;
    i18n?: FileViewerI18nInput;
    onMarkLoadStarted?: (version: number) => void;
    onStartLoading?: (message: string) => void;
    onSetLoadingMessage?: (message: string) => void;
    onSession?: (session: Session | null) => void;
    onActiveDocumentContext?: (context: FileViewerLifecycleContext) => void;
    onLifecycle?: (context: FileViewerLifecycleContext) => void;
    onClearLoadStarted?: (version: number) => void;
    onStopLoading?: () => void;
    onMissingData?: () => void;
    onError?: (error: unknown, kind: FileViewerRemoteFilePreviewErrorKind) => void;
}
export type FileViewerRemoteFilePreviewState<Session = unknown> = {
    readonly status: 'ready';
    readonly remoteSource: FileViewerRemoteSourcePlan;
    readonly download: Extract<FileViewerRemoteDownloadState, {
        stale: false;
        missing: false;
    }>;
    readonly read: Extract<FileViewerReadAndRenderFileState<Session>, {
        stale: false;
    }>;
    readonly stream: null;
    readonly error: null;
} | {
    readonly status: 'stream';
    readonly remoteSource: FileViewerRemoteSourcePlan;
    readonly download: null;
    readonly read: null;
    readonly stream: Extract<FileViewerStreamingPdfPreviewState<Session>, {
        status: 'ready';
    }>;
    readonly error: null;
} | {
    readonly status: 'missing';
    readonly remoteSource: FileViewerRemoteSourcePlan;
    readonly download: Extract<FileViewerRemoteDownloadState, {
        stale: false;
        missing: true;
    }>;
    readonly read: null;
    readonly stream: null;
    readonly error: null;
} | {
    readonly status: 'stale';
    readonly remoteSource: FileViewerRemoteSourcePlan;
    readonly download: FileViewerRemoteDownloadState | null;
    readonly read: FileViewerReadAndRenderFileState<Session> | null;
    readonly stream: Extract<FileViewerStreamingPdfPreviewState<Session>, {
        status: 'stale';
    }> | null;
    readonly error: null;
} | {
    readonly status: 'error';
    readonly remoteSource: FileViewerRemoteSourcePlan;
    readonly download: null;
    readonly read: null;
    readonly stream: Extract<FileViewerStreamingPdfPreviewState<Session>, {
        status: 'error';
    }> | null;
    readonly error: unknown;
};
export interface RunFileViewerPreviewRequestInput<LocalResult = unknown, RemoteResult = unknown> {
    file?: FileViewerFileRef | null;
    url?: string | null;
    reason?: FileViewerLifecycleContext['reason'];
    requestController: Pick<FileViewerRequestController, 'createVersion'>;
    previewTarget: MutableFileViewerPreviewState;
    onPreviewLocalFile: (source: FileViewerFileRef, version: number) => Promise<LocalResult>;
    onPreviewRemoteFile: (url: string, version: number) => Promise<RemoteResult>;
    onClearRenderedContent?: (reason?: FileViewerLifecycleContext['reason']) => void;
    onClearError?: () => void;
    onResetLoading?: () => void;
}
export type FileViewerPreviewRequestRunState<LocalResult = unknown, RemoteResult = unknown> = {
    readonly status: 'file';
    readonly version: number;
    readonly reason: FileViewerLifecycleContext['reason'];
    readonly file: FileViewerFileRef;
    readonly url: null;
    readonly result: LocalResult;
} | {
    readonly status: 'url';
    readonly version: number;
    readonly reason: FileViewerLifecycleContext['reason'];
    readonly file: null;
    readonly url: string;
    readonly result: RemoteResult;
} | {
    readonly status: 'reset';
    readonly version: number;
    readonly reason: FileViewerLifecycleContext['reason'];
    readonly file: null;
    readonly url: null;
    readonly result: MutableFileViewerPreviewState;
};
export interface CancelFileViewerPreviewRequestInput {
    reason?: FileViewerLifecycleContext['reason'];
    requestController: Pick<FileViewerRequestController, 'createVersion'>;
    previewTarget: MutableFileViewerPreviewRequestState;
    onClearRenderedContent?: (reason?: FileViewerLifecycleContext['reason']) => void;
    onClearError?: () => void;
}
export interface RunFileViewerPreviewSourceChangeInput {
    onRefreshPreview?: () => Promise<unknown> | unknown;
}
export interface RunFileViewerPreviewComponentUnmountInput {
    reason?: FileViewerLifecycleContext['reason'];
    onCancelPreview?: (reason: FileViewerLifecycleContext['reason']) => void;
    onClearRenderedContent?: (reason: FileViewerLifecycleContext['reason']) => void;
    onResetLoading?: () => void;
    onStopZoomObserver?: () => void;
    onStopFitObserver?: () => void;
    onStopViewStateObserver?: () => void;
}
export interface FileViewerPreviewComponentUnmountState {
    reason: FileViewerLifecycleContext['reason'];
}
export interface FileViewerSourceLoadingActionHandlers<Session = unknown> {
    isCurrentRequest: (version: number) => boolean;
    previewLocalFile: (source: FileViewerFileRef, version: number) => Promise<FileViewerLocalFilePreviewState<Session>>;
    previewRemoteFile: (url: string, version: number) => Promise<FileViewerRemoteFilePreviewState<Session>>;
    resetViewer: (reason?: FileViewerLifecycleContext['reason']) => MutableFileViewerPreviewState;
    refreshPreview: () => Promise<FileViewerPreviewRequestRunState<FileViewerLocalFilePreviewState<Session>, FileViewerRemoteFilePreviewState<Session>>>;
    cancelPreview: (reason?: FileViewerLifecycleContext['reason']) => number;
}
export interface CreateFileViewerSourceLoadingActionHandlersInput<Session = unknown> {
    getFile: () => FileViewerFileRef | null | undefined;
    getUrl: () => string | null | undefined;
    getCurrentFilename?: () => string | undefined;
    getPdfStreaming?: () => FileViewerPdfOptions['streaming'] | undefined;
    getI18n?: () => FileViewerI18nInput;
    getPageHref?: () => string | undefined;
    previewTarget: MutableFileViewerPreviewState;
    requestController: FileViewerRequestController;
    downloadFile: (input: FileViewerRemoteFileDownloadInput) => Promise<FileViewerFileRef | null | undefined>;
    mountRenderedContent: (buffer: ArrayBuffer, file: File, version: number, sourceUrl?: string, streamUrl?: string) => Promise<Session | undefined>;
    destroyRenderSession?: (session?: Session | null) => void;
    buildLoadStartState: (input: {
        version: number;
        source: FileViewerLifecycleContext['source'];
        file?: File | null;
        sourceUrl?: string | null;
    }) => FileViewerLoadStartState;
    buildRenderCompleteState: (input: {
        version: number;
        source: FileViewerLifecycleContext['source'];
        file?: File | null;
        sourceUrl?: string | null;
    }) => FileViewerRenderCompleteState;
    formatErrorMessage: FileViewerErrorMessageFormatter;
    onMarkLoadStarted?: (version: number) => void;
    onClearLoadStarted?: (version: number) => void;
    onStartLoading?: (message: string) => void;
    onSetLoadingMessage?: (message: string) => void;
    onStopLoading?: () => void;
    onShowError?: (message: string) => void;
    onClearError?: () => void;
    onResetLoading?: () => void;
    onClearRenderedContent?: (reason?: FileViewerLifecycleContext['reason']) => void;
    onSession?: (session: Session | null) => void;
    onActiveDocumentContext?: (context: FileViewerLifecycleContext) => void;
    onLifecycle?: (context: FileViewerLifecycleContext) => void;
}
export interface FinalizeFileViewerPreviewLoadStateInput {
    version: number;
    isCurrent: (version: number) => boolean;
    onClearLoadStarted?: (version: number) => void;
    onStopLoading?: () => void;
}
export declare const createFileViewerRequestController: () => FileViewerRequestController;
export declare const createFileViewerRequestScope: (requestController?: FileViewerRequestController) => FileViewerRequestScope;
export declare const isFileViewerAbortError: (error: unknown) => boolean;
export declare const hasFileViewerPreviewSource: ({ file, url, }?: ResolveFileViewerPreviewRequestReasonInput) => boolean;
export declare const resolveFileViewerPreviewRequestReason: (input?: ResolveFileViewerPreviewRequestReasonInput) => FileViewerLifecycleContext["reason"];
export declare const normalizeFileViewerSourceUrl: (sourceUrl?: string | null) => string | null;
export declare const createFileViewerEmptyPreviewState: () => FileViewerEmptyPreviewState;
export declare const createFileViewerPreviewRequestResetState: () => FileViewerPreviewRequestResetState;
export declare const createFileViewerPreviewStateTarget: ({ filename, file, buffer, sourceUrl, renderedReady, progressiveReady, }: CreateFileViewerPreviewStateTargetInput) => MutableFileViewerPreviewState;
export declare const applyFileViewerPreviewRequestResetState: <Target extends MutableFileViewerPreviewRequestState>(target: Target, state?: FileViewerPreviewRequestResetState) => Target;
export declare const commitFileViewerPreviewRequestStartState: ({ reason, requestController, previewTarget, onClearRenderedContent, onClearError, }: CommitFileViewerPreviewRequestStartStateInput) => number;
export declare const cancelFileViewerPreviewRequest: ({ reason, requestController, previewTarget, onClearRenderedContent, onClearError, }: CancelFileViewerPreviewRequestInput) => number;
export declare const runFileViewerPreviewSourceChange: ({ onRefreshPreview, }?: RunFileViewerPreviewSourceChangeInput) => unknown;
export declare const runFileViewerPreviewComponentUnmount: ({ reason, onCancelPreview, onClearRenderedContent, onResetLoading, onStopZoomObserver, onStopFitObserver, onStopViewStateObserver, }?: RunFileViewerPreviewComponentUnmountInput) => FileViewerPreviewComponentUnmountState;
export declare const applyFileViewerEmptyPreviewState: <Target extends MutableFileViewerPreviewState>(target: Target, state?: FileViewerEmptyPreviewState) => Target;
export declare const commitFileViewerEmptyPreviewResetState: ({ previewTarget, state, reason, onClearRenderedContent, onResetLoading, }: CommitFileViewerEmptyPreviewResetStateInput) => MutableFileViewerPreviewState;
export declare const runFileViewerPreviewRequest: <LocalResult = unknown, RemoteResult = unknown>({ file, url, reason, requestController, previewTarget, onPreviewLocalFile, onPreviewRemoteFile, onClearRenderedContent, onClearError, onResetLoading, }: RunFileViewerPreviewRequestInput<LocalResult, RemoteResult>) => Promise<FileViewerPreviewRequestRunState<LocalResult, RemoteResult>>;
export declare const createFileViewerReadPreviewState: ({ file, buffer, sourceUrl, fallbackFilename, }: CreateFileViewerReadPreviewStateInput) => FileViewerReadPreviewState;
export declare const applyFileViewerReadPreviewState: <Target extends MutableFileViewerReadPreviewState>(target: Target, state: FileViewerReadPreviewState) => Target;
export declare const applyFileViewerPreviewSourceUrlState: <Target extends MutableFileViewerPreviewSourceUrlState>(target: Target, sourceUrl?: string | null) => Target;
export declare const applyFileViewerPreviewFilenameState: <Target extends MutableFileViewerPreviewFilenameState>(target: Target, filename?: string, fallbackFilename?: string) => Target;
export declare const applyFileViewerRenderReadinessState: <Target extends MutableFileViewerRenderReadinessState>(target: Target, state: Partial<FileViewerRenderReadinessState>) => Target;
export declare const commitFileViewerRenderCompleteState: <Session = unknown>({ version, session, buildState, readinessTarget, onSession, onActiveDocumentContext, onLifecycle, onClearLoadStarted, }: CommitFileViewerRenderCompleteStateInput<Session>) => FileViewerRenderCompleteState;
export declare const runFileViewerReadAndRenderFile: <Session = unknown>({ file, version, sourceUrl, source, fallbackFilename, previewTarget, isCurrent, mountRenderedContent, destroyRenderSession, buildRenderCompleteState, onSession, onActiveDocumentContext, onLifecycle, onClearLoadStarted, }: RunFileViewerReadAndRenderFileInput<Session>) => Promise<FileViewerReadAndRenderFileState<Session>>;
export declare const runFileViewerStreamingPdfPreview: <Session = unknown>({ url, version, filename, previewTarget, isCurrent, mountRenderedContent, destroyRenderSession, buildRenderCompleteState, loadingMessage, i18n, onStartLoading, onSession, onActiveDocumentContext, onLifecycle, onClearLoadStarted, onStopLoading, onError, }: RunFileViewerStreamingPdfPreviewInput<Session>) => Promise<FileViewerStreamingPdfPreviewState<Session>>;
export declare const runFileViewerLocalFilePreview: <Session = unknown>({ source, version, currentFilename, fallbackFilename, previewTarget, isCurrent, mountRenderedContent, destroyRenderSession, buildLoadStartState, buildRenderCompleteState, onMarkLoadStarted, onStartLoading, onSession, onActiveDocumentContext, onLifecycle, onClearLoadStarted, onStopLoading, onError, }: RunFileViewerLocalFilePreviewInput<Session>) => Promise<FileViewerLocalFilePreviewState<Session>>;
export declare const runFileViewerRemoteFilePreview: <Session = unknown>({ url, version, pageHref, streaming, previewTarget, requestController, isCurrent, downloadFile, mountRenderedContent, destroyRenderSession, buildLoadStartState, buildRenderCompleteState, i18n, onMarkLoadStarted, onStartLoading, onSetLoadingMessage, onSession, onActiveDocumentContext, onLifecycle, onClearLoadStarted, onStopLoading, onMissingData, onError, }: RunFileViewerRemoteFilePreviewInput<Session>) => Promise<FileViewerRemoteFilePreviewState<Session>>;
export declare const createFileViewerSourceLoadingActionHandlers: <Session = unknown>({ getFile, getUrl, getCurrentFilename, getPdfStreaming, getI18n, getPageHref, previewTarget, requestController, downloadFile, mountRenderedContent, destroyRenderSession, buildLoadStartState, buildRenderCompleteState, formatErrorMessage, onMarkLoadStarted, onClearLoadStarted, onStartLoading, onSetLoadingMessage, onStopLoading, onShowError, onClearError, onResetLoading, onClearRenderedContent, onSession, onActiveDocumentContext, onLifecycle, }: CreateFileViewerSourceLoadingActionHandlersInput<Session>) => FileViewerSourceLoadingActionHandlers<Session>;
export declare const finalizeFileViewerPreviewLoadState: ({ version, isCurrent, onClearLoadStarted, onStopLoading, }: FinalizeFileViewerPreviewLoadStateInput) => void;
export declare const resolveFileViewerLoadStartMessage: (source: FileViewerLifecycleContext["source"], i18n?: FileViewerI18nInput) => string;
export declare const commitFileViewerLoadStartState: ({ version, filename, fallbackFilename, filenameTarget, buildState, onMarkLoadStarted, onLifecycle, onStartLoading, }: CommitFileViewerLoadStartStateInput) => FileViewerLoadStartState;
export declare const createFileViewerLoadStartState: ({ version, source, filename, file, sourceUrl, bufferSize, loadingMessage, i18n, timestamp, }: CreateFileViewerLoadStartStateInput) => FileViewerLoadStartState;
export declare const createFileViewerRenderCompleteState: ({ version, source, filename, file, sourceUrl, bufferSize, startedAt, timestamp, lifecycleState, }: CreateFileViewerRenderCompleteStateInput) => FileViewerRenderCompleteState;
export declare const resolveFileViewerFileRefSourcePlan: ({ source, currentFilename, fallbackFilename, }: ResolveFileViewerFileRefSourcePlanInput) => FileViewerFileRefSourcePlan;
export declare const normalizePdfStreamingMode: (mode: FileViewerPdfOptions["streaming"]) => true | false | "same-origin";
export declare const isSameOriginUrl: (url: string, pageHref: string) => boolean;
export declare const shouldStreamPdfUrl: ({ extension, pageHref, streaming, url, }: {
    extension: string;
    pageHref: string;
    streaming?: FileViewerPdfOptions["streaming"];
    url: string;
}) => boolean;
export interface FileViewerRemoteSourcePlan {
    readonly url: string;
    readonly filename: string;
    readonly extension: string;
    readonly streamPdf: boolean;
}
export interface CommitFileViewerRemoteDownloadStateInput {
    version: number;
    data?: FileViewerFileRef | null;
    currentFilename?: string;
    fallbackFilename?: string;
    isCurrent: (version: number) => boolean;
    i18n?: FileViewerI18nInput;
    onMissingData?: () => void;
    onSetLoadingMessage?: (message: string) => void;
}
export type FileViewerRemoteDownloadState = {
    readonly stale: true;
    readonly missing: false;
    readonly source: null;
} | {
    readonly stale: false;
    readonly missing: true;
    readonly source: null;
} | {
    readonly stale: false;
    readonly missing: false;
    readonly source: FileViewerFileRefSourcePlan;
};
export interface FileViewerLocationLike {
    href?: string | null;
}
export declare const resolveFileViewerPageHref: (locationLike?: FileViewerLocationLike) => string | undefined;
export declare const resolveFileViewerRemoteSourcePlan: ({ filename, fallbackFilename, pageHref, streaming, url, }: {
    filename?: string;
    fallbackFilename?: string;
    pageHref?: string;
    streaming?: FileViewerPdfOptions["streaming"];
    url: string;
}) => FileViewerRemoteSourcePlan;
export declare const commitFileViewerRemoteDownloadState: ({ version, data, currentFilename, fallbackFilename, isCurrent, i18n, onMissingData, onSetLoadingMessage, }: CommitFileViewerRemoteDownloadStateInput) => FileViewerRemoteDownloadState;
export declare const createFileViewerStreamingPdfPlaceholderFile: (filename?: string) => File;
