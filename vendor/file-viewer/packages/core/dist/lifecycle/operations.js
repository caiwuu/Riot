// Lifecycle and toolbar operation policy.
//
// This layer builds normalized contexts, runs user hooks, and resolves whether
// actions such as print/download/export/zoom should be available. Concrete file
// operation execution remains in `viewer/operations`.
import { resolvePrintAvailability } from '../registry/capabilities.js';
import { getExtension, normalizeFilename } from '../source/index.js';
import { translateFileViewerMessage, } from '../i18n/messages.js';
import { createFileViewerOriginalSourceState, hasFileViewerOriginalSource, } from '../viewer/operations.js';
export const FILE_VIEWER_LIFECYCLE_HOOKS = {
    'load-start': 'onLoadStart',
    'load-complete': 'onLoadComplete',
    'unload-start': 'onUnloadStart',
    'unload-complete': 'onUnloadComplete',
};
export const FILE_VIEWER_OPERATION_LABELS = {
    download: '下载原始文件',
    print: '打印完整渲染内容',
    'export-html': '导出渲染 HTML',
    'zoom-in': '放大预览',
    'zoom-out': '缩小预览',
    'zoom-reset': '还原预览比例',
};
export const FILE_VIEWER_BEFORE_OPERATION_ERROR_PREFIX = '操作前置校验失败';
export const FILE_VIEWER_LIFECYCLE_HOOK_ERROR_MESSAGE_PREFIX = 'FileViewer';
const FILE_VIEWER_ZOOM_OPERATIONS = ['zoom-in', 'zoom-out', 'zoom-reset'];
const FILE_VIEWER_ZOOM_BUTTON_OPERATIONS = {
    canZoomIn: 'zoom-in',
    canZoomOut: 'zoom-out',
    canReset: 'zoom-reset',
};
export const DEFAULT_FILE_VIEWER_TOOLBAR_ORDER = [
    'search',
    'zoom',
    'download',
    'print',
    'exportHtml',
    'theme',
];
export const buildFileViewerLifecycleContext = ({ phase, version, source, filename, file, url, size, bufferSize, startedAt, duration, timestamp, reason, }) => {
    var _a;
    const resolvedFilename = normalizeFilename((file === null || file === void 0 ? void 0 : file.name) || filename || url || '');
    const now = timestamp !== null && timestamp !== void 0 ? timestamp : Date.now();
    return {
        phase,
        type: getExtension(resolvedFilename),
        filename: resolvedFilename,
        source,
        url,
        file: file || undefined,
        size: (_a = size !== null && size !== void 0 ? size : file === null || file === void 0 ? void 0 : file.size) !== null && _a !== void 0 ? _a : bufferSize,
        version,
        timestamp: now,
        duration: duration !== null && duration !== void 0 ? duration : (phase === 'load-complete' && typeof startedAt === 'number' ? now - startedAt : undefined),
        reason,
    };
};
export const buildFileViewerLifecycleContextFromNormalizedSource = ({ phase, source, version, startedAt, timestamp, reason, }) => {
    const now = timestamp !== null && timestamp !== void 0 ? timestamp : Date.now();
    return buildFileViewerLifecycleContext({
        phase,
        filename: source.filename,
        source: source.kind,
        url: source.url,
        file: typeof File !== 'undefined' && source.file instanceof File ? source.file : undefined,
        size: source.size,
        version,
        timestamp: now,
        duration: phase.endsWith('complete') && typeof startedAt === 'number' ? now - startedAt : undefined,
        reason,
    });
};
export const resolveFileViewerLifecycleFallbackSource = ({ file, url, } = {}) => {
    if (file) {
        return { source: 'file' };
    }
    if (url) {
        return { source: 'url', sourceUrl: url };
    }
    return { source: 'empty' };
};
export const createFileViewerLifecycleStateController = () => {
    let activeDocumentContext = null;
    const loadStartedAt = new Map();
    return {
        markLoadStarted(version, timestamp = Date.now()) {
            loadStartedAt.set(version, timestamp);
        },
        clearLoadStarted(version) {
            loadStartedAt.delete(version);
        },
        getLoadStartedAt(version) {
            return loadStartedAt.get(version);
        },
        getActiveDocumentContext() {
            return activeDocumentContext;
        },
        setActiveDocumentContext(context) {
            activeDocumentContext = context;
        },
        clearActiveDocumentContext() {
            activeDocumentContext = null;
        },
        buildActiveUnloadContext(phase, context, reason = 'replace', timestamp = Date.now()) {
            if (!context) {
                return null;
            }
            return {
                ...context,
                phase,
                timestamp,
                reason,
            };
        },
    };
};
export const buildFileViewerOperationContext = (operation, lifecycleContext, timestamp = Date.now(), i18n) => {
    const { phase: _phase, ...context } = lifecycleContext;
    const labelKey = {
        download: 'operation.download',
        print: 'operation.print',
        'export-html': 'operation.exportHtml',
        'zoom-in': 'operation.zoomIn',
        'zoom-out': 'operation.zoomOut',
        'zoom-reset': 'operation.zoomReset',
    };
    return {
        ...context,
        operation,
        label: translateFileViewerMessage(i18n, labelKey[operation]),
        timestamp,
    };
};
export const buildFileViewerOperationContextFromLifecycleState = ({ operation, lifecycleState, version, filename, bufferSize, currentFile, fallbackFile, fallbackUrl, timestamp, lifecycleTimestamp, i18n, }) => {
    const activeContext = lifecycleState.getActiveDocumentContext();
    const fallbackSource = resolveFileViewerLifecycleFallbackSource({
        file: fallbackFile,
        url: fallbackUrl,
    });
    const baseContext = activeContext || buildFileViewerLifecycleContext({
        phase: 'load-complete',
        version,
        source: fallbackSource.source,
        file: currentFile,
        filename,
        url: fallbackSource.sourceUrl,
        bufferSize,
        startedAt: lifecycleState.getLoadStartedAt(version),
        timestamp: lifecycleTimestamp,
    });
    return buildFileViewerOperationContext(operation, baseContext, timestamp, i18n);
};
export const emitFileViewerComponentLifecycleEvent = (emit, context) => {
    if (context.phase === 'load-start') {
        emit('load-start', context);
        return;
    }
    if (context.phase === 'load-complete') {
        emit('load-complete', context);
        return;
    }
    if (context.phase === 'unload-start') {
        emit('unload-start', context);
        return;
    }
    emit('unload-complete', context);
};
export const resolveFileViewerBeforeOperationErrorMessage = ({ error, formatErrorMessage, prefix, i18n, }) => {
    return formatErrorMessage(prefix || translateFileViewerMessage(i18n, 'error.beforeOperation'), error, i18n);
};
export const resolveFileViewerLifecycleHookErrorMessage = ({ context, prefix = FILE_VIEWER_LIFECYCLE_HOOK_ERROR_MESSAGE_PREFIX, }) => {
    return `${prefix} ${context.phase} hook failed`;
};
export const DEFAULT_FILE_VIEWER_LIFECYCLE_HOOK_ERROR_LOGGER = (message, error) => {
    if (typeof console !== 'undefined' && typeof console.error === 'function') {
        console.error(message, error);
    }
};
export const reportFileViewerLifecycleHookError = ({ error, context, onLogError = DEFAULT_FILE_VIEWER_LIFECYCLE_HOOK_ERROR_LOGGER, prefix, }) => {
    const message = resolveFileViewerLifecycleHookErrorMessage({ context, prefix });
    onLogError === null || onLogError === void 0 ? void 0 : onLogError(message, error, context);
    return message;
};
export const DEFAULT_FILE_VIEWER_OPERATION_ERROR_LOGGER = error => {
    if (typeof console !== 'undefined' && typeof console.error === 'function') {
        console.error(error);
    }
};
export const reportFileViewerOperationError = ({ error, context, onLogError = DEFAULT_FILE_VIEWER_OPERATION_ERROR_LOGGER, }) => {
    onLogError === null || onLogError === void 0 ? void 0 : onLogError(error, context);
    return error;
};
export const runFileViewerActiveUnloadStart = ({ lifecycleState, reason = 'replace', onLifecycle, }) => {
    const context = lifecycleState.getActiveDocumentContext();
    const unloadContext = lifecycleState.buildActiveUnloadContext('unload-start', context, reason);
    if (unloadContext) {
        onLifecycle === null || onLifecycle === void 0 ? void 0 : onLifecycle(unloadContext);
    }
    return {
        reason,
        context,
        unloadContext,
    };
};
export const runFileViewerActiveUnloadComplete = ({ lifecycleState, context = null, reason = 'replace', onLifecycle, }) => {
    const unloadContext = lifecycleState.buildActiveUnloadContext('unload-complete', context, reason);
    if (unloadContext) {
        onLifecycle === null || onLifecycle === void 0 ? void 0 : onLifecycle(unloadContext);
    }
    return {
        reason,
        context,
        unloadContext,
    };
};
export const getFileViewerLifecycleHookName = (phase) => {
    return FILE_VIEWER_LIFECYCLE_HOOKS[phase];
};
export const runFileViewerLifecycleHook = async (context, hooks, onError) => {
    const hook = hooks === null || hooks === void 0 ? void 0 : hooks[getFileViewerLifecycleHookName(context.phase)];
    if (!hook) {
        return;
    }
    try {
        await hook(context);
    }
    catch (error) {
        onError === null || onError === void 0 ? void 0 : onError(error, context);
    }
};
export const getFileViewerBeforeOperationHooks = (options, operation) => {
    const toolbar = options === null || options === void 0 ? void 0 : options.toolbar;
    if (!toolbar || typeof toolbar !== 'object') {
        return [options === null || options === void 0 ? void 0 : options.beforeOperation];
    }
    const specificHook = operation === 'download'
        ? toolbar.beforeDownload
        : operation === 'print'
            ? toolbar.beforePrint
            : operation === 'export-html'
                ? toolbar.beforeExportHtml
                : undefined;
    return [options === null || options === void 0 ? void 0 : options.beforeOperation, toolbar.beforeOperation, specificHook];
};
const isToolbarActionMapAllowed = (map, operation) => (map === null || map === void 0 ? void 0 : map[operation]) !== false;
export const isFileViewerToolbarOperationPermitted = (toolbar, operation) => {
    if (!toolbar || typeof toolbar !== 'object') {
        return true;
    }
    return isToolbarActionMapAllowed(toolbar.permissions, operation);
};
const isFileViewerToolbarOperationVisible = (toolbar, operation) => isToolbarActionMapAllowed(toolbar.items, operation) &&
    isToolbarActionMapAllowed(toolbar.permissions, operation);
const normalizeFileViewerToolbarItem = (item) => {
    if (item === 'search' || item === 'zoom' || item === 'download' || item === 'print') {
        return item;
    }
    if (item === 'exportHtml' || item === 'export-html') {
        return 'exportHtml';
    }
    if (item === 'theme') {
        return 'theme';
    }
    return undefined;
};
export const resolveFileViewerToolbarOrder = (toolbar) => {
    const order = [];
    const seen = new Set();
    const addItem = (item) => {
        const normalized = normalizeFileViewerToolbarItem(item);
        if (!normalized || seen.has(normalized)) {
            return;
        }
        seen.add(normalized);
        order.push(normalized);
    };
    if (Array.isArray(toolbar === null || toolbar === void 0 ? void 0 : toolbar.order)) {
        toolbar.order.forEach(addItem);
    }
    DEFAULT_FILE_VIEWER_TOOLBAR_ORDER.forEach(addItem);
    return order;
};
const hasAnyToolbarZoomOperation = (toolbar) => FILE_VIEWER_ZOOM_OPERATIONS.some(operation => isFileViewerToolbarOperationVisible(toolbar, operation));
const applyToolbarPermissionsToAvailability = (availability, toolbar) => {
    if (!toolbar || typeof toolbar !== 'object' || !toolbar.permissions) {
        return availability;
    }
    const next = cloneFileViewerOperationAvailability(availability);
    if (!isFileViewerToolbarOperationPermitted(toolbar, 'download')) {
        next.download = false;
    }
    if (!isFileViewerToolbarOperationPermitted(toolbar, 'print')) {
        next.print = false;
    }
    if (!isFileViewerToolbarOperationPermitted(toolbar, 'export-html')) {
        next.exportHtml = false;
    }
    if (!isFileViewerToolbarOperationPermitted(toolbar, 'zoom-in')) {
        next.zoomIn = false;
    }
    if (!isFileViewerToolbarOperationPermitted(toolbar, 'zoom-out')) {
        next.zoomOut = false;
    }
    if (!isFileViewerToolbarOperationPermitted(toolbar, 'zoom-reset')) {
        next.zoomReset = false;
    }
    next.zoom = next.zoom && (next.zoomIn || next.zoomOut || next.zoomReset);
    return next;
};
export const runFileViewerBeforeOperation = async ({ context, options, onBefore, onCancel, onError, }) => {
    onBefore === null || onBefore === void 0 ? void 0 : onBefore(context);
    try {
        if (!isFileViewerToolbarOperationPermitted(options === null || options === void 0 ? void 0 : options.toolbar, context.operation)) {
            onCancel === null || onCancel === void 0 ? void 0 : onCancel(context);
            return false;
        }
        for (const hook of getFileViewerBeforeOperationHooks(options, context.operation)) {
            if (!hook) {
                continue;
            }
            const result = await hook(context);
            if (result === false) {
                onCancel === null || onCancel === void 0 ? void 0 : onCancel(context);
                return false;
            }
        }
    }
    catch (error) {
        onError === null || onError === void 0 ? void 0 : onError(error, context);
        onCancel === null || onCancel === void 0 ? void 0 : onCancel(context);
        return false;
    }
    return true;
};
export const serializeFileViewerContext = (context) => {
    const { file: _file, ...serializable } = context;
    return {
        ...serializable,
        hasFile: !!context.file,
    };
};
export const dispatchFileViewerLifecycleEvent = ({ context, hooks, onChange, onError, }) => {
    onChange === null || onChange === void 0 ? void 0 : onChange(context.phase, context);
    void runFileViewerLifecycleHook(context, hooks, onError);
    return true;
};
export const dispatchFileViewerOperationContextEvent = ({ context, onChange, }) => {
    onChange === null || onChange === void 0 ? void 0 : onChange(context);
    return true;
};
export const createFileViewerLifecycleActions = ({ lifecycleState, getOptions = () => undefined, onLifecycleChange, onLifecycleError, onOperationBefore, onOperationCancel, onOperationError, }) => {
    const notifyLifecycle = (context) => {
        var _a;
        return dispatchFileViewerLifecycleEvent({
            context,
            hooks: (_a = getOptions()) === null || _a === void 0 ? void 0 : _a.hooks,
            onChange: onLifecycleChange,
            onError: onLifecycleError,
        });
    };
    return {
        notifyLifecycle,
        notifyActiveUnloadStart(reason = 'replace') {
            return runFileViewerActiveUnloadStart({
                lifecycleState,
                reason,
                onLifecycle: notifyLifecycle,
            }).context;
        },
        notifyActiveUnloadComplete(context, reason = 'replace') {
            return runFileViewerActiveUnloadComplete({
                lifecycleState,
                context,
                reason,
                onLifecycle: notifyLifecycle,
            });
        },
        runBeforeOperation(context) {
            return runFileViewerBeforeOperation({
                context,
                options: getOptions(),
                onBefore: nextContext => {
                    dispatchFileViewerOperationContextEvent({
                        event: 'operation-before',
                        context: nextContext,
                        onChange: onOperationBefore,
                    });
                },
                onCancel: nextContext => {
                    dispatchFileViewerOperationContextEvent({
                        event: 'operation-cancel',
                        context: nextContext,
                        onChange: onOperationCancel,
                    });
                },
                onError: onOperationError,
            });
        },
    };
};
export const dispatchFileViewerOperationAvailabilityChange = ({ availability, onChange, }) => {
    const payload = cloneFileViewerOperationAvailability(availability);
    onChange === null || onChange === void 0 ? void 0 : onChange(payload);
    return true;
};
export const dispatchFileViewerZoomChange = ({ state, onChange, }) => {
    onChange === null || onChange === void 0 ? void 0 : onChange(state);
    return true;
};
export const createFileViewerToolbarActions = ({ getOperationAvailability, getToolbarDisabled = () => false, getZoomState, onOperationAvailabilityChange, onZoomChange, }) => {
    return {
        notifyOperationAvailabilityChange(availability = getOperationAvailability()) {
            return dispatchFileViewerOperationAvailabilityChange({
                availability,
                onChange: onOperationAvailabilityChange,
            });
        },
        notifyZoomChange(state = getZoomState()) {
            return dispatchFileViewerZoomChange({
                state,
                onChange: onZoomChange,
            });
        },
        isZoomButtonDisabled(action) {
            return isFileViewerZoomButtonDisabled({
                action,
                availability: getOperationAvailability(),
                toolbarDisabled: getToolbarDisabled(),
                zoomState: getZoomState(),
            });
        },
    };
};
export const createFileViewerPublicApi = ({ getOperationAvailability, ...api }) => {
    return {
        ...api,
        getOperationAvailability: () => cloneFileViewerOperationAvailability(getOperationAvailability()),
    };
};
export const createFileViewerToolbarZoomSyncSnapshot = (state) => [
    state.scale,
    state.label,
    state.canZoomIn,
    state.canZoomOut,
    state.canReset,
];
export const runFileViewerToolbarAvailabilitySync = ({ toolbarActions, availability, }) => {
    return toolbarActions.notifyOperationAvailabilityChange(availability);
};
export const runFileViewerToolbarZoomSync = ({ toolbarActions, state, }) => {
    return toolbarActions.notifyZoomChange(state);
};
export const createFileViewerToolbarControllerActionHandlers = ({ getAdapter = () => null, getBuffer = () => null, getExtension, getFile = () => null, getHasError = () => false, getLoading = () => false, getOptions, getSearchAvailable = () => true, getSourceUrl = () => null, getToolbar, getRenderedReady, getZoomState, zoomSyncState, onOperationAvailabilityChange, onZoomChange, }) => {
    let currentToolbarState = null;
    const resolveToolbarState = () => {
        var _a, _b, _c, _d;
        currentToolbarState = resolveFileViewerToolbarState({
            extension: getExtension(),
            source: createFileViewerOriginalSourceState({
                buffer: (_a = getBuffer()) !== null && _a !== void 0 ? _a : null,
                file: (_b = getFile()) !== null && _b !== void 0 ? _b : null,
                url: (_c = getSourceUrl()) !== null && _c !== void 0 ? _c : null,
            }),
            renderedReady: getRenderedReady(),
            hasError: getHasError(),
            adapter: (_d = getAdapter()) !== null && _d !== void 0 ? _d : null,
            zoomState: getZoomState(),
            toolbar: getToolbar(),
            options: getOptions === null || getOptions === void 0 ? void 0 : getOptions(),
            searchAvailable: getSearchAvailable(),
            loading: getLoading(),
        });
        return currentToolbarState;
    };
    const getResolvedToolbarState = () => currentToolbarState !== null && currentToolbarState !== void 0 ? currentToolbarState : resolveToolbarState();
    const toolbarActions = createFileViewerToolbarActions({
        getOperationAvailability: () => getResolvedToolbarState().operationAvailability,
        getToolbarDisabled: () => getResolvedToolbarState().toolbarDisabled,
        getZoomState,
        onOperationAvailabilityChange,
        onZoomChange,
    });
    return {
        resolveToolbarState,
        createZoomSyncSnapshot: () => createFileViewerToolbarZoomSyncSnapshot(zoomSyncState !== null && zoomSyncState !== void 0 ? zoomSyncState : getZoomState()),
        syncOperationAvailability: availability => runFileViewerToolbarAvailabilitySync({
            toolbarActions,
            availability,
        }),
        syncZoomChange: state => runFileViewerToolbarZoomSync({
            toolbarActions,
            state,
        }),
        isZoomButtonDisabled: toolbarActions.isZoomButtonDisabled,
    };
};
export const normalizeFileViewerToolbar = (options) => {
    const toolbar = options === null || options === void 0 ? void 0 : options.toolbar;
    if (toolbar === false) {
        return {
            download: false,
            print: false,
            exportHtml: false,
            zoom: false,
            search: false,
            theme: false,
            order: resolveFileViewerToolbarOrder(undefined),
        };
    }
    if (toolbar && typeof toolbar === 'object') {
        return {
            download: toolbar.download !== false && isFileViewerToolbarOperationVisible(toolbar, 'download'),
            print: toolbar.print !== false && isFileViewerToolbarOperationVisible(toolbar, 'print'),
            exportHtml: toolbar.exportHtml !== false && isFileViewerToolbarOperationVisible(toolbar, 'export-html'),
            zoom: toolbar.zoom !== false && hasAnyToolbarZoomOperation(toolbar),
            search: toolbar.search !== false,
            theme: toolbar.theme !== false,
            order: resolveFileViewerToolbarOrder(toolbar),
            items: toolbar.items,
            permissions: toolbar.permissions,
            position: toolbar.position,
            beforeOperation: toolbar.beforeOperation,
            beforeDownload: toolbar.beforeDownload,
            beforePrint: toolbar.beforePrint,
            beforeExportHtml: toolbar.beforeExportHtml,
        };
    }
    return {
        download: true,
        print: true,
        exportHtml: true,
        zoom: true,
        search: true,
        theme: true,
        order: resolveFileViewerToolbarOrder(undefined),
    };
};
export const resolveFileViewerOperationAvailability = ({ extension, hasOriginalSource, renderedReady, hasError = false, adapter, source, zoomState, }) => {
    const hasRenderableOutput = renderedReady && !hasError;
    const hasSource = hasOriginalSource !== null && hasOriginalSource !== void 0 ? hasOriginalSource : (source ? hasFileViewerOriginalSource(source) : false);
    const zoomEnabled = hasRenderableOutput && (zoomState.canZoomIn || zoomState.canZoomOut || zoomState.canReset);
    return {
        download: hasSource,
        print: hasRenderableOutput && resolvePrintAvailability(extension, adapter !== null && adapter !== void 0 ? adapter : null, renderedReady),
        exportHtml: hasRenderableOutput && (adapter === null || adapter === void 0 ? void 0 : adapter.exportHtml) !== false,
        zoom: zoomEnabled,
        zoomIn: zoomEnabled && zoomState.canZoomIn,
        zoomOut: zoomEnabled && zoomState.canZoomOut,
        zoomReset: zoomEnabled && zoomState.canReset,
    };
};
export const cloneFileViewerOperationAvailability = (availability) => ({
    download: availability.download,
    print: availability.print,
    exportHtml: availability.exportHtml,
    zoom: availability.zoom,
    zoomIn: availability.zoomIn,
    zoomOut: availability.zoomOut,
    zoomReset: availability.zoomReset,
});
export const resolveVisibleFileViewerToolbar = (toolbar, availability, searchEnabled = true) => {
    return {
        download: toolbar.download && availability.download,
        print: toolbar.print && availability.print,
        exportHtml: toolbar.exportHtml && availability.exportHtml,
        zoom: toolbar.zoom && availability.zoom,
        search: toolbar.search !== false && searchEnabled,
        theme: toolbar.theme !== false,
    };
};
export const hasVisibleFileViewerToolbarActions = (toolbar) => {
    return !!(toolbar.download ||
        toolbar.print ||
        toolbar.exportHtml ||
        toolbar.zoom ||
        toolbar.search ||
        toolbar.theme);
};
export const isFileViewerZoomButtonDisabled = ({ toolbarDisabled = false, availability, zoomState, action, }) => {
    const operation = FILE_VIEWER_ZOOM_BUTTON_OPERATIONS[action];
    const operationAllowed = operation === 'zoom-in'
        ? availability.zoomIn
        : operation === 'zoom-out'
            ? availability.zoomOut
            : availability.zoomReset;
    return toolbarDisabled || !availability.zoom || !operationAllowed || !zoomState[action];
};
export const isFileViewerToolbarDisabled = ({ loading = false, hasError = false, }) => {
    return !!(loading || hasError);
};
export const resolveFileViewerToolbarState = ({ toolbar, options, searchAvailable = true, loading = false, ...availabilityInput }) => {
    const operationAvailability = applyToolbarPermissionsToAvailability(resolveFileViewerOperationAvailability(availabilityInput), options === null || options === void 0 ? void 0 : options.toolbar);
    const visibleToolbar = resolveVisibleFileViewerToolbar(toolbar, operationAvailability, (options === null || options === void 0 ? void 0 : options.search) !== false && searchAvailable);
    return {
        operationAvailability,
        visibleToolbar,
        toolbarOrder: resolveFileViewerToolbarOrder(toolbar),
        showToolbar: hasVisibleFileViewerToolbarActions(visibleToolbar),
        toolbarPosition: resolveFileViewerToolbarPosition(options, availabilityInput.extension),
        toolbarDisabled: isFileViewerToolbarDisabled({
            loading,
            hasError: availabilityInput.hasError,
        }),
    };
};
export const resolveFileViewerToolbarPosition = (options, extension) => {
    const toolbar = options === null || options === void 0 ? void 0 : options.toolbar;
    const position = toolbar && typeof toolbar === 'object' ? toolbar.position : 'auto';
    if (position === 'top' || position === 'top-center' || position === 'bottom-right') {
        return position;
    }
    return extension === 'pdf' ? 'bottom-right' : 'top';
};
