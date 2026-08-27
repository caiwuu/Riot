import type { FileViewerRendererDispatcher } from './dispatcher';
import { type FileViewerMutableAccessor, type MutableFileViewerRenderReadinessState } from '../source/loading';
import type { FileRenderExportAdapter, FileRenderThumbnailAdapter, FileRenderContext, FileRenderHandler, FileViewerLifecycleContext, FileViewerOptions, FileViewerStyleIsolation, RenderSurface, RendererDefinition, RendererLoadContext, RendererLoader, RendererRegistry, RendererSession } from '../contracts/types';
export declare const FILE_VIEWER_RENDER_SESSION_DISPOSE_ERROR_MESSAGE = "\u9884\u89C8\u5185\u5BB9\u5378\u8F7D\u5931\u8D25";
export interface ResolveFileViewerRenderSessionDisposeErrorMessageInput {
    message?: string;
}
export type FileViewerRenderSessionDisposeErrorLogger = (message: string, error: unknown) => void;
export interface ReportFileViewerRenderSessionDisposeErrorInput extends ResolveFileViewerRenderSessionDisposeErrorMessageInput {
    error: unknown;
    onLogError?: FileViewerRenderSessionDisposeErrorLogger | null;
}
export interface RenderFileViewerHandlerInput<Rendered = unknown, Target extends HTMLElement = HTMLElement> {
    dispatcher: Pick<FileViewerRendererDispatcher<FileRenderHandler<Rendered, Target>>, 'resolve' | 'handlersByRendererId'>;
    buffer: ArrayBuffer;
    target: Target;
    type?: string;
    context?: FileRenderContext;
    throwOnMissingHandler?: boolean;
}
export interface CreateFileRenderHandlerLoaderOptions<Rendered = unknown, Target extends HTMLElement = HTMLElement> {
    handler: FileRenderHandler<Rendered, Target>;
    rendererId?: string;
    getTarget?: (context: RendererLoadContext) => Target;
    createContext?: (context: RendererLoadContext) => FileRenderContext;
    destroy?: (rendered: Rendered, context: RendererLoadContext) => void | Promise<void>;
}
export interface FileRenderHandlerRendererSession<Rendered = unknown> extends RendererSession {
    rendered: Rendered;
}
export interface DisposeFileViewerRendererSessionOptions {
    onError?: (error: unknown) => void;
}
export interface FileViewerRenderSurfaceState<Session extends RendererSession = RendererSession> {
    session: Session | null;
    exportAdapter: FileRenderExportAdapter | null;
    thumbnailAdapter: FileRenderThumbnailAdapter | null;
}
export type MutableFileViewerRenderSurfaceState<Session extends RendererSession = RendererSession> = FileViewerRenderSurfaceState<Session>;
export interface CreateFileViewerRenderReadinessTargetInput {
    renderedReady: FileViewerMutableAccessor<boolean>;
    progressiveReady: FileViewerMutableAccessor<boolean>;
}
export interface CreateFileViewerRenderSurfaceStateTargetInput<Session extends RendererSession = RendererSession> {
    session: FileViewerMutableAccessor<Session | null>;
    exportAdapter: FileViewerMutableAccessor<FileRenderExportAdapter | null>;
    thumbnailAdapter?: FileViewerMutableAccessor<FileRenderThumbnailAdapter | null>;
}
export interface CreateFileViewerRenderTargetOptions {
    className?: string;
}
export type ResolvedFileViewerStyleIsolation = Exclude<FileViewerStyleIsolation, 'auto'>;
export interface CreateFileViewerRenderSurfaceOptions extends CreateFileViewerRenderTargetOptions {
    styleIsolation?: FileViewerStyleIsolation;
}
export interface FileViewerStyleHandle {
    node?: HTMLStyleElement;
    sheet?: CSSStyleSheet;
    remove(): void;
}
export interface AppendFileViewerStyleOptions {
    /**
     * Constructable stylesheets are useful for long-lived ShadowRoots. Keep this
     * opt-in so renderer styles still clean up naturally with their target node.
     */
    adoptedStyleSheet?: boolean;
}
export interface ResetFileViewerRenderSurfaceInput<Session extends RendererSession = RendererSession> {
    surfaceState: MutableFileViewerRenderSurfaceState<Session>;
    readinessState: MutableFileViewerRenderReadinessState;
    container?: HTMLElement | null;
    disposeOptions?: DisposeFileViewerRendererSessionOptions;
}
export interface FileViewerRenderSurfaceMountContext<Session extends RendererSession = RendererSession> {
    buffer: ArrayBuffer;
    file: File;
    version: number;
    type: string;
    target: HTMLElement;
    filename: string;
    sourceUrl?: string;
    streamUrl?: string;
    onProgressiveRender: () => void;
    registerExportAdapter: (adapter: FileRenderExportAdapter | null) => void;
    surfaceState: MutableFileViewerRenderSurfaceState<Session>;
    readinessState: MutableFileViewerRenderReadinessState;
}
export interface RunFileViewerRenderSurfaceMountInput<Session extends RendererSession = RendererSession> {
    buffer: ArrayBuffer;
    file: File;
    version: number;
    sourceUrl?: string;
    streamUrl?: string;
    getContainer: () => HTMLElement | null | undefined;
    surfaceState: MutableFileViewerRenderSurfaceState<Session>;
    readinessState: MutableFileViewerRenderReadinessState;
    isCurrent: (version: number) => boolean;
    clearRenderedContent: (reason?: FileViewerLifecycleContext['reason']) => void;
    render: (context: FileViewerRenderSurfaceMountContext<Session>) => Promise<Session | undefined>;
    waitForContainer?: () => Promise<unknown> | unknown;
    waitForPaint?: () => Promise<unknown> | unknown;
    disposeSession?: (session?: Session | null) => void;
    onStartZoomObserver?: () => void;
    onStartFitObserver?: () => void;
    onStartViewStateObserver?: () => void;
    onApplyInitialFit?: () => Promise<unknown> | unknown;
    onRefreshDocumentIndex?: () => Promise<unknown> | unknown;
    onRefreshZoomProvider?: () => void;
    onRefreshViewStateProvider?: () => void;
}
export interface RunFileViewerRenderSurfaceClearInput<Session extends RendererSession = RendererSession, UnloadContext = FileViewerLifecycleContext | null> {
    reason?: FileViewerLifecycleContext['reason'];
    surfaceState: MutableFileViewerRenderSurfaceState<Session>;
    readinessState: MutableFileViewerRenderReadinessState;
    container?: HTMLElement | null;
    disposeOptions?: DisposeFileViewerRendererSessionOptions;
    onUnloadStart?: (reason: FileViewerLifecycleContext['reason']) => UnloadContext;
    onUnloadComplete?: (context: UnloadContext | undefined, reason: FileViewerLifecycleContext['reason']) => void;
    onClearActiveDocumentContext?: () => void;
    onClearDocumentState?: () => void;
    onStopZoomObserver?: () => void;
    onClearZoomProvider?: () => void;
    onStopFitObserver?: () => void;
    onStopViewStateObserver?: () => void;
    onClearViewStateProvider?: () => void;
}
export interface FileViewerRenderSurfaceClearState<Session extends RendererSession = RendererSession, UnloadContext = FileViewerLifecycleContext | null> {
    reason: FileViewerLifecycleContext['reason'];
    unloadContext: UnloadContext | undefined;
    session: Session | null | undefined;
}
export interface FileViewerRenderSurfaceActionHandlers<Session extends RendererSession = RendererSession, UnloadContext = FileViewerLifecycleContext | null> {
    destroyRenderSession: (session?: Session | null) => void;
    setActiveRenderSession: (session: Session | null) => MutableFileViewerRenderSurfaceState<Session>;
    clearRenderedContent: (reason?: FileViewerLifecycleContext['reason']) => FileViewerRenderSurfaceClearState<Session, UnloadContext>;
    mountRenderedContent: (buffer: ArrayBuffer, file: File, version: number, sourceUrl?: string, streamUrl?: string) => Promise<Session | undefined>;
}
export interface CreateFileViewerRenderSurfaceActionHandlersInput<Session extends RendererSession = RendererSession, UnloadContext = FileViewerLifecycleContext | null> {
    getContainer: () => HTMLElement | null | undefined;
    surfaceState: MutableFileViewerRenderSurfaceState<Session>;
    readinessState: MutableFileViewerRenderReadinessState;
    isCurrent: (version: number) => boolean;
    render: (context: FileViewerRenderSurfaceMountContext<Session>) => Promise<Session | undefined>;
    waitForContainer?: () => Promise<unknown> | unknown;
    waitForPaint?: () => Promise<unknown> | unknown;
    disposeOptions?: DisposeFileViewerRendererSessionOptions;
    onUnloadStart?: (reason: FileViewerLifecycleContext['reason']) => UnloadContext;
    onUnloadComplete?: (context: UnloadContext | undefined, reason: FileViewerLifecycleContext['reason']) => void;
    onClearActiveDocumentContext?: () => void;
    onClearDocumentState?: () => void;
    onStartZoomObserver?: () => void;
    onStopZoomObserver?: () => void;
    onClearZoomProvider?: () => void;
    onStartFitObserver?: () => void;
    onStopFitObserver?: () => void;
    onStartViewStateObserver?: () => void;
    onStopViewStateObserver?: () => void;
    onClearViewStateProvider?: () => void;
    onApplyInitialFit?: () => Promise<unknown> | unknown;
    onRefreshDocumentIndex?: () => Promise<unknown> | unknown;
    onRefreshZoomProvider?: () => void;
    onRefreshViewStateProvider?: () => void;
}
export declare const DEFAULT_FILE_VIEWER_RENDER_TARGET_CLASS = "file-render";
export declare const FILE_VIEWER_RENDER_SURFACE_BACKGROUND_PROPERTY = "--file-viewer-render-surface-background";
export declare const normalizeFileViewerRenderSurfaceBackground: (value: unknown) => string | null;
/**
 * Keeps the public UI option and the renderer-facing CSS custom property in
 * sync. Custom properties inherit through Shadow DOM, so setting this on the
 * render host covers both scoped and shadow-isolated renderer targets.
 */
export declare const syncFileViewerRenderSurfaceBackground: (target: HTMLElement | null | undefined, options?: Pick<FileViewerOptions, "ui"> | null) => string | null;
export declare const isFileViewerShadowRoot: (value: unknown) => value is ShadowRoot;
export declare const getFileViewerShadowRootForNode: (node: Node | null | undefined) => ShadowRoot | null;
export declare const normalizeFileViewerStyleIsolation: (isolation?: FileViewerStyleIsolation) => FileViewerStyleIsolation;
export declare const resolveFileViewerStyleIsolation: (options: Pick<FileViewerOptions, "styleIsolation"> | undefined, container?: HTMLElement | null) => ResolvedFileViewerStyleIsolation;
export declare const clearFileViewerRenderSurface: (container?: HTMLElement | null) => void;
export declare const createFileViewerRenderTarget: (container: HTMLElement, options?: CreateFileViewerRenderTargetOptions) => HTMLDivElement;
export declare const createFileViewerRenderSurface: (container: HTMLElement, options?: CreateFileViewerRenderSurfaceOptions) => RenderSurface;
export declare const appendFileViewerStyle: (target: HTMLElement | ShadowRoot, css: string, options?: AppendFileViewerStyleOptions) => FileViewerStyleHandle;
export declare const removeFileViewerRenderTarget: (container: HTMLElement, target: HTMLElement) => boolean;
export declare const createFileViewerRenderSurfaceState: <Session extends RendererSession = RendererSession>() => FileViewerRenderSurfaceState<Session>;
export declare const createFileViewerRenderReadinessTarget: ({ renderedReady, progressiveReady, }: CreateFileViewerRenderReadinessTargetInput) => MutableFileViewerRenderReadinessState;
export declare const createFileViewerRenderSurfaceStateTarget: <Session extends RendererSession = RendererSession>({ session, exportAdapter, thumbnailAdapter, }: CreateFileViewerRenderSurfaceStateTargetInput<Session>) => MutableFileViewerRenderSurfaceState<Session>;
export declare const applyFileViewerRenderSurfaceState: <Session extends RendererSession, Target extends MutableFileViewerRenderSurfaceState<Session>>(target: Target, state: Partial<FileViewerRenderSurfaceState<Session>>) => Target;
export declare const createFileRenderHandlerRendererSession: <Rendered = unknown>(rendered: Rendered, destroy?: () => void | Promise<void>) => FileRenderHandlerRendererSession<Rendered>;
export interface CreateFileRenderHandlerRegistryOptions<Rendered = unknown, Target extends HTMLElement = HTMLElement> extends Omit<CreateFileRenderHandlerLoaderOptions<Rendered, Target>, 'handler'> {
    definitions?: readonly RendererDefinition[];
    handlers: Iterable<{
        rendererId: string;
        handler: FileRenderHandler<Rendered, Target>;
    }>;
}
export interface FileRenderHandlerRegistryResult<Rendered = unknown, Target extends HTMLElement = HTMLElement> {
    registry: RendererRegistry;
    dispatcher: FileViewerRendererDispatcher<FileRenderHandler<Rendered, Target>>;
    missingRendererIds: string[];
}
export declare const disposeFileViewerRendered: (rendered?: unknown) => void | Promise<void>;
export declare const resolveFileViewerRenderSessionDisposeErrorMessage: ({ message, }?: ResolveFileViewerRenderSessionDisposeErrorMessageInput) => string;
export declare const DEFAULT_FILE_VIEWER_RENDER_SESSION_DISPOSE_ERROR_LOGGER: FileViewerRenderSessionDisposeErrorLogger;
export declare const reportFileViewerRenderSessionDisposeError: ({ error, onLogError, ...messageInput }: ReportFileViewerRenderSessionDisposeErrorInput) => string;
export declare const disposeFileViewerRendererSession: (session?: RendererSession | null, options?: DisposeFileViewerRendererSessionOptions) => void;
export declare const disposeActiveFileViewerRendererSession: <Session extends RendererSession, Target extends MutableFileViewerRenderSurfaceState<Session>>(target: Target, options?: DisposeFileViewerRendererSessionOptions) => Session | null;
export declare const resetFileViewerRenderSurface: <Session extends RendererSession>({ surfaceState, readinessState, container, disposeOptions, }: ResetFileViewerRenderSurfaceInput<Session>) => Session | null;
export declare const runFileViewerRenderSurfaceClear: <Session extends RendererSession, UnloadContext = FileViewerLifecycleContext | null>({ reason, surfaceState, readinessState, container, disposeOptions, onUnloadStart, onUnloadComplete, onClearActiveDocumentContext, onClearDocumentState, onStopZoomObserver, onClearZoomProvider, onStopFitObserver, onStopViewStateObserver, onClearViewStateProvider, }: RunFileViewerRenderSurfaceClearInput<Session, UnloadContext>) => FileViewerRenderSurfaceClearState<Session, UnloadContext>;
export declare const runFileViewerRenderSurfaceMount: <Session extends RendererSession>({ buffer, file, version, sourceUrl, streamUrl, getContainer, surfaceState, readinessState, isCurrent, clearRenderedContent, render, waitForContainer, waitForPaint, disposeSession, onStartZoomObserver, onStartFitObserver, onStartViewStateObserver, onApplyInitialFit, onRefreshDocumentIndex, onRefreshZoomProvider, onRefreshViewStateProvider, }: RunFileViewerRenderSurfaceMountInput<Session>) => Promise<Session | undefined>;
export declare const createFileViewerRenderSurfaceActionHandlers: <Session extends RendererSession, UnloadContext = FileViewerLifecycleContext | null>({ getContainer, surfaceState, readinessState, isCurrent, render, waitForContainer, waitForPaint, disposeOptions, onUnloadStart, onUnloadComplete, onClearActiveDocumentContext, onClearDocumentState, onStartZoomObserver, onStopZoomObserver, onClearZoomProvider, onStartFitObserver, onStopFitObserver, onStartViewStateObserver, onStopViewStateObserver, onClearViewStateProvider, onApplyInitialFit, onRefreshDocumentIndex, onRefreshZoomProvider, onRefreshViewStateProvider, }: CreateFileViewerRenderSurfaceActionHandlersInput<Session, UnloadContext>) => FileViewerRenderSurfaceActionHandlers<Session, UnloadContext>;
export declare const buildFileRenderContextFromLoadContext: ({ source, surface, options, signal, registerExportAdapter, registerThumbnailAdapter, renderContext, }: RendererLoadContext) => FileRenderContext;
export declare const renderFileViewerHandler: <Rendered = unknown, Target extends HTMLElement = HTMLElement>({ dispatcher, buffer, target, type, context, throwOnMissingHandler, }: RenderFileViewerHandlerInput<Rendered, Target>) => Promise<Rendered | undefined>;
export declare const createFileRenderHandlerLoader: <Rendered = unknown, Target extends HTMLElement = HTMLElement>({ handler, rendererId, getTarget, createContext, destroy, }: CreateFileRenderHandlerLoaderOptions<Rendered, Target>) => RendererLoader;
export declare const createFileRenderHandlerRegistry: <Rendered = unknown, Target extends HTMLElement = HTMLElement>({ definitions, handlers, getTarget, createContext, destroy, }: CreateFileRenderHandlerRegistryOptions<Rendered, Target>) => FileRenderHandlerRegistryResult<Rendered, Target>;
