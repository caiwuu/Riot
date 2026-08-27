// Rendering orchestration layer.
//
// This module adapts registry entries into disposable renderer sessions. It is
// the only place that should bridge raw buffers, DOM targets, render contexts,
// export adapters, and readiness state.
import { DEFAULT_RENDERER_DEFINITIONS } from '../registry/formats.js';
import { waitForFileViewerNextPaint } from '../output/export.js';
import { createFileViewerRendererDispatcher } from './dispatcher.js';
import { resolveFileViewerRendererRedirectId } from './dispatcher.js';
import { createRendererRegistry } from '../registry/registry.js';
import { getExtension, normalizeFileExtension } from '../source/index.js';
import { applyFileViewerRenderReadinessState, } from '../source/loading.js';
import { findFileViewerViewStateProvider } from '../features/document/dom/index.js';
import { registerFileViewerGenericViewStateProvider } from '../features/document/viewState.js';
export const FILE_VIEWER_RENDER_SESSION_DISPOSE_ERROR_MESSAGE = '预览内容卸载失败';
export const DEFAULT_FILE_VIEWER_RENDER_TARGET_CLASS = 'file-render';
export const FILE_VIEWER_RENDER_SURFACE_BACKGROUND_PROPERTY = '--file-viewer-render-surface-background';
const FILE_VIEWER_RENDER_HOST_CLASS = 'file-render-host';
const FILE_VIEWER_SHADOW_RENDER_TARGET_CLASS = 'file-render-shadow-target';
export const normalizeFileViewerRenderSurfaceBackground = (value) => {
    if (typeof value !== 'string') {
        return null;
    }
    const normalized = value.trim();
    return normalized && normalized.toLowerCase() !== 'auto' ? normalized : null;
};
/**
 * Keeps the public UI option and the renderer-facing CSS custom property in
 * sync. Custom properties inherit through Shadow DOM, so setting this on the
 * render host covers both scoped and shadow-isolated renderer targets.
 */
export const syncFileViewerRenderSurfaceBackground = (target, options) => {
    var _a;
    if (!target) {
        return null;
    }
    const background = normalizeFileViewerRenderSurfaceBackground((_a = options === null || options === void 0 ? void 0 : options.ui) === null || _a === void 0 ? void 0 : _a.surfaceBackground);
    if (background) {
        target.style.setProperty(FILE_VIEWER_RENDER_SURFACE_BACKGROUND_PROPERTY, background);
    }
    else {
        target.style.removeProperty(FILE_VIEWER_RENDER_SURFACE_BACKGROUND_PROPERTY);
    }
    return background;
};
export const isFileViewerShadowRoot = (value) => {
    if (!value || typeof value !== 'object') {
        return false;
    }
    if (typeof ShadowRoot !== 'undefined' && value instanceof ShadowRoot) {
        return true;
    }
    return value.nodeType === 11 &&
        !!value.host;
};
const getElementRootNode = (element) => {
    var _a;
    return ((_a = element === null || element === void 0 ? void 0 : element.getRootNode) === null || _a === void 0 ? void 0 : _a.call(element)) || null;
};
export const getFileViewerShadowRootForNode = (node) => {
    var _a;
    if (!node) {
        return null;
    }
    if (isFileViewerShadowRoot(node)) {
        return node;
    }
    const root = (_a = node.getRootNode) === null || _a === void 0 ? void 0 : _a.call(node);
    return isFileViewerShadowRoot(root) ? root : null;
};
export const normalizeFileViewerStyleIsolation = (isolation) => {
    return isolation === 'shadow' || isolation === 'scoped' || isolation === 'none'
        ? isolation
        : 'auto';
};
export const resolveFileViewerStyleIsolation = (options, container) => {
    const isolation = normalizeFileViewerStyleIsolation(options === null || options === void 0 ? void 0 : options.styleIsolation);
    if (isolation === 'auto') {
        return isFileViewerShadowRoot(getElementRootNode(container)) ? 'scoped' : 'none';
    }
    if (isolation === 'shadow' && isFileViewerShadowRoot(getElementRootNode(container))) {
        return 'scoped';
    }
    return isolation;
};
const isPromiseLike = (value) => {
    return !!value &&
        (typeof value === 'object' || typeof value === 'function') &&
        typeof value.then === 'function';
};
export const clearFileViewerRenderSurface = (container) => {
    if (!container) {
        return;
    }
    while (container.firstChild) {
        container.removeChild(container.firstChild);
    }
};
export const createFileViewerRenderTarget = (container, options = {}) => {
    const target = container.ownerDocument.createElement('div');
    target.className = options.className || DEFAULT_FILE_VIEWER_RENDER_TARGET_CLASS;
    target.style.width = '100%';
    target.style.minWidth = '0';
    target.style.minHeight = '0';
    container.appendChild(target);
    return target;
};
export const createFileViewerRenderSurface = (container, options = {}) => {
    const styleIsolation = resolveFileViewerStyleIsolation({
        styleIsolation: options.styleIsolation,
    }, container);
    const className = options.className || DEFAULT_FILE_VIEWER_RENDER_TARGET_CLASS;
    if (styleIsolation === 'shadow' && typeof container.attachShadow === 'function') {
        const host = container.ownerDocument.createElement('div');
        host.className = `${FILE_VIEWER_RENDER_HOST_CLASS} ${className}`;
        host.dataset.fileViewerStyleIsolation = 'shadow';
        host.style.width = '100%';
        host.style.minWidth = '0';
        host.style.minHeight = '0';
        const shadowRoot = host.attachShadow({ mode: 'open' });
        const target = container.ownerDocument.createElement('div');
        target.className = `${FILE_VIEWER_SHADOW_RENDER_TARGET_CLASS} ${className}`;
        target.dataset.fileViewerStyleIsolation = 'shadow';
        target.style.width = '100%';
        target.style.minWidth = '0';
        target.style.minHeight = '0';
        shadowRoot.appendChild(target);
        container.appendChild(host);
        return {
            host,
            container: target,
            shadowRoot,
            styleIsolation,
        };
    }
    const target = createFileViewerRenderTarget(container, { className });
    target.dataset.fileViewerStyleIsolation = styleIsolation;
    return {
        host: target,
        container: target,
        styleIsolation,
    };
};
const appendStyleElement = (target, css) => {
    const documentRef = target.ownerDocument;
    const style = documentRef.createElement('style');
    style.textContent = css;
    target.appendChild(style);
    return style;
};
const getConstructableStyleSheet = (root) => {
    var _a, _b;
    const constructor = (_b = (_a = root.ownerDocument.defaultView) === null || _a === void 0 ? void 0 : _a.CSSStyleSheet) !== null && _b !== void 0 ? _b : (typeof CSSStyleSheet !== 'undefined' ? CSSStyleSheet : undefined);
    return 'adoptedStyleSheets' in root &&
        constructor &&
        typeof constructor.prototype.replaceSync === 'function'
        ? constructor
        : null;
};
export const appendFileViewerStyle = (target, css, options = {}) => {
    const shadowRoot = getFileViewerShadowRootForNode(target);
    const StyleSheet = shadowRoot ? getConstructableStyleSheet(shadowRoot) : null;
    if (options.adoptedStyleSheet && shadowRoot && StyleSheet) {
        // Use the target document's constructor. A stylesheet constructed in the
        // parent window cannot be adopted by a ShadowRoot owned by an iframe.
        const sheet = new StyleSheet();
        sheet.replaceSync(css);
        shadowRoot.adoptedStyleSheets = [...shadowRoot.adoptedStyleSheets, sheet];
        return {
            sheet,
            remove() {
                shadowRoot.adoptedStyleSheets = shadowRoot.adoptedStyleSheets.filter(item => item !== sheet);
            },
        };
    }
    const styleTarget = isFileViewerShadowRoot(target) ? target : target;
    const node = appendStyleElement(styleTarget, css);
    return {
        node,
        remove() {
            node.remove();
        },
    };
};
export const removeFileViewerRenderTarget = (container, target) => {
    if (target.parentNode !== container) {
        return false;
    }
    container.removeChild(target);
    return true;
};
export const createFileViewerRenderSurfaceState = () => ({
    session: null,
    exportAdapter: null,
    thumbnailAdapter: null,
});
export const createFileViewerRenderReadinessTarget = ({ renderedReady, progressiveReady, }) => {
    return {
        get renderedReady() {
            return renderedReady.get();
        },
        set renderedReady(value) {
            renderedReady.set(value);
        },
        get progressiveReady() {
            return progressiveReady.get();
        },
        set progressiveReady(value) {
            progressiveReady.set(value);
        },
    };
};
export const createFileViewerRenderSurfaceStateTarget = ({ session, exportAdapter, thumbnailAdapter, }) => {
    let localThumbnailAdapter = null;
    return {
        get session() {
            return session.get();
        },
        set session(value) {
            session.set(value);
        },
        get exportAdapter() {
            return exportAdapter.get();
        },
        set exportAdapter(value) {
            exportAdapter.set(value);
        },
        get thumbnailAdapter() {
            var _a;
            return (_a = thumbnailAdapter === null || thumbnailAdapter === void 0 ? void 0 : thumbnailAdapter.get()) !== null && _a !== void 0 ? _a : localThumbnailAdapter;
        },
        set thumbnailAdapter(value) {
            localThumbnailAdapter = value;
            thumbnailAdapter === null || thumbnailAdapter === void 0 ? void 0 : thumbnailAdapter.set(value);
        },
    };
};
export const applyFileViewerRenderSurfaceState = (target, state) => {
    var _a, _b, _c;
    if ('session' in state) {
        target.session = (_a = state.session) !== null && _a !== void 0 ? _a : null;
    }
    if ('exportAdapter' in state) {
        target.exportAdapter = (_b = state.exportAdapter) !== null && _b !== void 0 ? _b : null;
    }
    if ('thumbnailAdapter' in state) {
        target.thumbnailAdapter = (_c = state.thumbnailAdapter) !== null && _c !== void 0 ? _c : null;
    }
    return target;
};
export const createFileRenderHandlerRendererSession = (rendered, destroy) => ({
    rendered,
    destroy: () => {
        if (destroy) {
            return destroy();
        }
        return disposeFileViewerRendered(rendered);
    },
});
export const disposeFileViewerRendered = (rendered) => {
    if (!rendered || typeof rendered !== 'object') {
        return;
    }
    const disposable = rendered;
    if (typeof disposable.unmount === 'function') {
        return disposable.unmount();
    }
    if (typeof disposable.$destroy === 'function') {
        return disposable.$destroy();
    }
    if (typeof disposable.destroy === 'function') {
        return disposable.destroy();
    }
};
export const resolveFileViewerRenderSessionDisposeErrorMessage = ({ message = FILE_VIEWER_RENDER_SESSION_DISPOSE_ERROR_MESSAGE, } = {}) => message;
export const DEFAULT_FILE_VIEWER_RENDER_SESSION_DISPOSE_ERROR_LOGGER = (message, error) => {
    if (typeof console !== 'undefined' && typeof console.warn === 'function') {
        console.warn(message, error);
    }
};
export const reportFileViewerRenderSessionDisposeError = ({ error, onLogError = DEFAULT_FILE_VIEWER_RENDER_SESSION_DISPOSE_ERROR_LOGGER, ...messageInput }) => {
    const message = resolveFileViewerRenderSessionDisposeErrorMessage(messageInput);
    onLogError === null || onLogError === void 0 ? void 0 : onLogError(message, error);
    return message;
};
export const disposeFileViewerRendererSession = (session, options = {}) => {
    if (!(session === null || session === void 0 ? void 0 : session.destroy)) {
        return;
    }
    const handleError = (error) => {
        var _a;
        (_a = options.onError) === null || _a === void 0 ? void 0 : _a.call(options, error);
    };
    try {
        const result = session.destroy();
        if (isPromiseLike(result)) {
            void Promise.resolve(result).catch(handleError);
        }
    }
    catch (error) {
        handleError(error);
    }
};
export const disposeActiveFileViewerRendererSession = (target, options = {}) => {
    const session = target.session;
    try {
        disposeFileViewerRendererSession(session, options);
    }
    finally {
        target.session = null;
    }
    return session;
};
export const resetFileViewerRenderSurface = ({ surfaceState, readinessState, container, disposeOptions, }) => {
    const session = disposeActiveFileViewerRendererSession(surfaceState, disposeOptions);
    applyFileViewerRenderSurfaceState(surfaceState, { exportAdapter: null, thumbnailAdapter: null });
    applyFileViewerRenderReadinessState(readinessState, {
        renderedReady: false,
        progressiveReady: false,
    });
    clearFileViewerRenderSurface(container);
    return session;
};
export const runFileViewerRenderSurfaceClear = ({ reason = 'replace', surfaceState, readinessState, container, disposeOptions, onUnloadStart, onUnloadComplete, onClearActiveDocumentContext, onClearDocumentState, onStopZoomObserver, onClearZoomProvider, onStopFitObserver, onStopViewStateObserver, onClearViewStateProvider, }) => {
    const unloadContext = onUnloadStart === null || onUnloadStart === void 0 ? void 0 : onUnloadStart(reason);
    let session;
    try {
        session = resetFileViewerRenderSurface({
            surfaceState,
            readinessState,
            container,
            disposeOptions,
        });
    }
    finally {
        onClearActiveDocumentContext === null || onClearActiveDocumentContext === void 0 ? void 0 : onClearActiveDocumentContext();
        onClearDocumentState === null || onClearDocumentState === void 0 ? void 0 : onClearDocumentState();
        onStopZoomObserver === null || onStopZoomObserver === void 0 ? void 0 : onStopZoomObserver();
        onClearZoomProvider === null || onClearZoomProvider === void 0 ? void 0 : onClearZoomProvider();
        onStopFitObserver === null || onStopFitObserver === void 0 ? void 0 : onStopFitObserver();
        onStopViewStateObserver === null || onStopViewStateObserver === void 0 ? void 0 : onStopViewStateObserver();
        onClearViewStateProvider === null || onClearViewStateProvider === void 0 ? void 0 : onClearViewStateProvider();
    }
    onUnloadComplete === null || onUnloadComplete === void 0 ? void 0 : onUnloadComplete(unloadContext, reason);
    return {
        reason,
        unloadContext,
        session,
    };
};
export const runFileViewerRenderSurfaceMount = async ({ buffer, file, version, sourceUrl, streamUrl, getContainer, surfaceState, readinessState, isCurrent, clearRenderedContent, render, waitForContainer, waitForPaint = waitForFileViewerNextPaint, disposeSession = disposeFileViewerRendererSession, onStartZoomObserver, onStartFitObserver, onStartViewStateObserver, onApplyInitialFit, onRefreshDocumentIndex, onRefreshZoomProvider, onRefreshViewStateProvider, }) => {
    if (!getContainer()) {
        await (waitForContainer === null || waitForContainer === void 0 ? void 0 : waitForContainer());
    }
    const container = getContainer();
    if (!container || !isCurrent(version)) {
        return undefined;
    }
    clearRenderedContent('replace');
    const target = createFileViewerRenderTarget(container);
    onStartZoomObserver === null || onStartZoomObserver === void 0 ? void 0 : onStartZoomObserver();
    onStartFitObserver === null || onStartFitObserver === void 0 ? void 0 : onStartFitObserver();
    onStartViewStateObserver === null || onStartViewStateObserver === void 0 ? void 0 : onStartViewStateObserver();
    await (waitForContainer === null || waitForContainer === void 0 ? void 0 : waitForContainer());
    await waitForPaint();
    if (!isCurrent(version)) {
        removeFileViewerRenderTarget(container, target);
        return undefined;
    }
    const registerExportAdapter = (adapter) => {
        applyFileViewerRenderSurfaceState(surfaceState, { exportAdapter: adapter });
    };
    const onProgressiveRender = () => {
        if (isCurrent(version)) {
            applyFileViewerRenderReadinessState(readinessState, { progressiveReady: true });
        }
    };
    try {
        const session = await render({
            buffer,
            file,
            version,
            type: getExtension(file.name),
            target,
            filename: file.name,
            sourceUrl,
            streamUrl,
            onProgressiveRender,
            registerExportAdapter,
            surfaceState,
            readinessState,
        });
        if (!isCurrent(version)) {
            disposeSession(session);
            removeFileViewerRenderTarget(container, target);
            return undefined;
        }
        void (onRefreshDocumentIndex === null || onRefreshDocumentIndex === void 0 ? void 0 : onRefreshDocumentIndex());
        onRefreshZoomProvider === null || onRefreshZoomProvider === void 0 ? void 0 : onRefreshZoomProvider();
        onRefreshViewStateProvider === null || onRefreshViewStateProvider === void 0 ? void 0 : onRefreshViewStateProvider();
        if (onApplyInitialFit) {
            await onApplyInitialFit();
            onRefreshZoomProvider === null || onRefreshZoomProvider === void 0 ? void 0 : onRefreshZoomProvider();
            onRefreshViewStateProvider === null || onRefreshViewStateProvider === void 0 ? void 0 : onRefreshViewStateProvider();
        }
        return session;
    }
    catch (error) {
        removeFileViewerRenderTarget(container, target);
        throw error;
    }
};
export const createFileViewerRenderSurfaceActionHandlers = ({ getContainer, surfaceState, readinessState, isCurrent, render, waitForContainer, waitForPaint, disposeOptions, onUnloadStart, onUnloadComplete, onClearActiveDocumentContext, onClearDocumentState, onStartZoomObserver, onStopZoomObserver, onClearZoomProvider, onStartFitObserver, onStopFitObserver, onStartViewStateObserver, onStopViewStateObserver, onClearViewStateProvider, onApplyInitialFit, onRefreshDocumentIndex, onRefreshZoomProvider, onRefreshViewStateProvider, }) => {
    const handleDisposeError = (error) => {
        if (disposeOptions === null || disposeOptions === void 0 ? void 0 : disposeOptions.onError) {
            disposeOptions.onError(error);
            return;
        }
        reportFileViewerRenderSessionDisposeError({ error });
    };
    const mergedDisposeOptions = {
        ...disposeOptions,
        onError: handleDisposeError,
    };
    const destroyRenderSession = (session) => {
        disposeFileViewerRendererSession(session, mergedDisposeOptions);
    };
    const setActiveRenderSession = (session) => {
        return applyFileViewerRenderSurfaceState(surfaceState, { session });
    };
    const clearRenderedContent = (reason = 'replace') => {
        return runFileViewerRenderSurfaceClear({
            reason,
            surfaceState,
            readinessState,
            container: getContainer(),
            disposeOptions: mergedDisposeOptions,
            onUnloadStart,
            onUnloadComplete,
            onClearActiveDocumentContext,
            onClearDocumentState,
            onStopZoomObserver,
            onClearZoomProvider,
            onStopFitObserver,
            onStopViewStateObserver,
            onClearViewStateProvider,
        });
    };
    const mountRenderedContent = async (buffer, file, version, sourceUrl, streamUrl) => {
        return await runFileViewerRenderSurfaceMount({
            buffer,
            file,
            version,
            sourceUrl,
            streamUrl,
            getContainer,
            surfaceState,
            readinessState,
            isCurrent,
            clearRenderedContent,
            render,
            waitForContainer,
            waitForPaint,
            disposeSession: destroyRenderSession,
            onStartZoomObserver,
            onStartFitObserver,
            onStartViewStateObserver,
            onApplyInitialFit,
            onRefreshDocumentIndex,
            onRefreshZoomProvider,
            onRefreshViewStateProvider,
        });
    };
    return {
        destroyRenderSession,
        setActiveRenderSession,
        clearRenderedContent,
        mountRenderedContent,
    };
};
export const buildFileRenderContextFromLoadContext = ({ source, surface, options, signal, registerExportAdapter, registerThumbnailAdapter, renderContext, }) => ({
    filename: source.filename,
    url: source.url,
    streamUrl: source.url,
    signal,
    options,
    surface,
    registerExportAdapter,
    registerThumbnailAdapter,
    ...renderContext,
});
export const renderFileViewerHandler = async ({ dispatcher, buffer, target, type = '', context, throwOnMissingHandler = false, }) => {
    var _a, _b, _c;
    const normalizedType = normalizeFileExtension(type);
    const handler = dispatcher.resolve(normalizedType);
    if (!handler) {
        if (throwOnMissingHandler) {
            throw new Error(`No file viewer renderer is registered for "${normalizedType}".`);
        }
        return undefined;
    }
    try {
        return await handler(buffer, target, normalizedType, context);
    }
    catch (error) {
        const redirectedRendererId = resolveFileViewerRendererRedirectId(error);
        const redirectedHandler = redirectedRendererId
            ? dispatcher.handlersByRendererId.get(redirectedRendererId)
            : undefined;
        if (!redirectedHandler || redirectedHandler === handler) {
            throw error;
        }
        const redirectedType = ((_a = DEFAULT_RENDERER_DEFINITIONS
            .find(definition => definition.id === redirectedRendererId)) === null || _a === void 0 ? void 0 : _a.extensions[0]) || normalizedType;
        (_c = (_b = context === null || context === void 0 ? void 0 : context.options) === null || _b === void 0 ? void 0 : _b.onDiagnostic) === null || _c === void 0 ? void 0 : _c.call(_b, {
            code: 'renderer-content-signature-redirect',
            level: 'warning',
            message: `File extension .${normalizedType} contains the ${redirectedRendererId} container and was routed as .${redirectedType}.`,
            detail: {
                declaredExtension: normalizedType,
                detectedRendererId: redirectedRendererId,
                routedExtension: redirectedType,
            },
        });
        return await redirectedHandler(buffer, target, redirectedType, context);
    }
};
export const createFileRenderHandlerLoader = ({ handler, rendererId, getTarget = context => context.surface.container, createContext = buildFileRenderContextFromLoadContext, destroy, }) => {
    return async (context) => {
        const { source } = context;
        if (!source.buffer) {
            throw new Error('FileRenderHandler renderer requires an ArrayBuffer source.');
        }
        const target = getTarget(context);
        const renderContext = createContext(context);
        const rendered = await handler(source.buffer, target, source.extension, renderContext);
        const existingProvider = findFileViewerViewStateProvider(target);
        const genericViewStateProvider = existingProvider
            ? null
            : registerFileViewerGenericViewStateProvider({
                host: target,
                renderer: rendererId || source.extension,
            });
        const viewStateProvider = existingProvider || (genericViewStateProvider === null || genericViewStateProvider === void 0 ? void 0 : genericViewStateProvider.provider) || null;
        if (context.options.initialViewState && (viewStateProvider === null || viewStateProvider === void 0 ? void 0 : viewStateProvider.applyState)) {
            await viewStateProvider.applyState(context.options.initialViewState, {
                action: 'restore',
                source: 'initial',
            });
        }
        return createFileRenderHandlerRendererSession(rendered, () => {
            const destroyRendered = () => {
                if (destroy) {
                    return destroy(rendered, context);
                }
                return disposeFileViewerRendered(rendered);
            };
            try {
                genericViewStateProvider === null || genericViewStateProvider === void 0 ? void 0 : genericViewStateProvider.destroy();
            }
            finally {
                return destroyRendered();
            }
        });
    };
};
export const createFileRenderHandlerRegistry = ({ definitions = DEFAULT_RENDERER_DEFINITIONS, handlers, getTarget, createContext, destroy, }) => {
    const baseRegistry = createRendererRegistry(definitions);
    const dispatcher = createFileViewerRendererDispatcher({
        registry: baseRegistry,
        handlers,
    });
    const definitionsWithLoaders = baseRegistry.list().map(definition => {
        const handler = dispatcher.handlersByRendererId.get(definition.id);
        if (!handler) {
            return definition;
        }
        return {
            ...definition,
            load: createFileRenderHandlerLoader({
                handler: (buffer, target, type, context) => renderFileViewerHandler({
                    dispatcher,
                    buffer,
                    target,
                    type,
                    context,
                    throwOnMissingHandler: true,
                }),
                rendererId: definition.id,
                getTarget,
                createContext,
                destroy,
            }),
        };
    });
    return {
        registry: createRendererRegistry(definitionsWithLoaders),
        dispatcher,
        missingRendererIds: dispatcher.missingRendererIds,
    };
};
