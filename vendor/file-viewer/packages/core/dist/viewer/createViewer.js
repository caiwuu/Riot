// Imperative viewer composition root.
//
// `createViewer` wires source normalization, lifecycle hooks, renderer
// sessions, document features, watermarking, and public operations into a
// framework-neutral controller. Framework packages should wrap this API instead
// of reimplementing the orchestration.
import { createEmptyFileViewerSearchState, } from '../features/document/model.js';
import { createFileViewerDocumentFeatureControllerActionHandlers, } from '../features/document/events.js';
import { createFileViewerZoomController } from '../features/document/zoom.js';
import { createFileViewerViewStateController } from '../features/document/viewState.js';
import { createFileViewerFitController, hasFileViewerExplicitInitialViewState, } from '../features/document/fit.js';
import { DEFAULT_FILE_VIEWER_DOWNLOAD_FILENAME, DEFAULT_FILE_VIEWER_EXPORT_FILENAME, DEFAULT_FILE_VIEWER_PREVIEW_TITLE, createFileViewerOriginalSourceStateFromNormalizedSource, executeFileViewerDownloadOperation, executeFileViewerExportHtmlOperation, executeFileViewerPrintOperation, resolveFileViewerDisplayFilename, resolveFileViewerOperationFilename, } from './operations.js';
import { openFileViewerPrintMaskDesignerAsync } from '../features/printMaskLoader.js';
import { applyFileViewerZoomAvailability, createUnsupportedAvailability, getRendererAvailability, } from '../registry/capabilities.js';
import { buildFileViewerLifecycleContextFromNormalizedSource, buildFileViewerOperationContext, runFileViewerBeforeOperation, runFileViewerLifecycleHook, } from '../lifecycle/operations.js';
import { collectFileViewerRendererPlugins, createRendererRegistry, getFileViewerAutoRendererPresetVersion, hasFileViewerRendererPresetName, installFileViewerRendererPlugins, listFileViewerAutoRendererPresets, resolveFileViewerRendererPresetInputs, } from '../registry/registry.js';
import { createFileRenderHandlerLoader, applyFileViewerRenderSurfaceState, createFileViewerRenderSurfaceState, createFileViewerRenderSurface, removeFileViewerRenderTarget, syncFileViewerRenderSurfaceBackground, } from '../rendering/handler.js';
import { createFileViewerCoreRendererRegistry } from '../renderers/index.js';
import { createFileViewerRequestScope } from '../source/loading.js';
import { normalizeSource } from '../source/index.js';
import { buildFileViewerWatermarkInlineStyle } from '../features/watermark.js';
import { createFileViewerUnsupportedState } from './state.js';
const emitLifecycle = async (options, onEvent, phase, source, version, startedAt, reason) => {
    const now = Date.now();
    const context = buildFileViewerLifecycleContextFromNormalizedSource({
        phase,
        source,
        version,
        timestamp: now,
        startedAt,
        reason,
    });
    await runFileViewerLifecycleHook(context, options.hooks, error => {
        throw error;
    });
    onEvent === null || onEvent === void 0 ? void 0 : onEvent({ type: phase, payload: context });
};
const createBaseRendererRegistry = (createOptions, options) => {
    if (options.rendererMode === 'replace') {
        return createRendererRegistry([]);
    }
    if (createOptions.registry) {
        return createOptions.registry;
    }
    return createFileViewerCoreRendererRegistry({
        builtinRenderers: options.builtinRenderers,
    }).registry;
};
const resolveAutoRenderersEnabled = (options) => {
    const setting = options.autoRenderers;
    if (typeof setting === 'boolean') {
        return setting;
    }
    if ((setting === null || setting === void 0 ? void 0 : setting.enabled) !== undefined) {
        return setting.enabled;
    }
    return (options.rendererMode || 'extend') !== 'replace';
};
const createFileViewerLoadSignal = (controller, signals) => {
    if (!controller) {
        return signals.find(signal => signal);
    }
    const removeListeners = [];
    const cleanup = () => removeListeners.splice(0).forEach(remove => remove());
    controller.signal.addEventListener('abort', cleanup, { once: true });
    for (const signal of signals) {
        if (!signal) {
            continue;
        }
        if (signal.aborted) {
            controller.abort(signal.reason);
            break;
        }
        const onAbort = () => controller.abort(signal.reason);
        signal.addEventListener('abort', onAbort, { once: true });
        removeListeners.push(() => signal.removeEventListener('abort', onAbort));
    }
    return controller.signal;
};
const renderMissingRendererState = (container, type, options) => {
    const documentRef = container.ownerDocument;
    const state = createFileViewerUnsupportedState(type, undefined, options);
    const wrapper = documentRef.createElement('div');
    wrapper.className = 'file-viewer-missing-renderer';
    wrapper.style.cssText = [
        'display:flex',
        'min-height:260px',
        'height:100%',
        'align-items:center',
        'justify-content:center',
        'padding:32px',
        'box-sizing:border-box',
        'text-align:center',
        'color:#64748b',
        'font:14px/1.6 system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif',
    ].join(';');
    const content = documentRef.createElement('div');
    const title = documentRef.createElement('strong');
    title.textContent = state.message;
    title.style.cssText = 'display:block;margin-bottom:8px;color:#172033;font-size:16px;';
    const description = documentRef.createElement('p');
    description.textContent = state.description || state.title;
    description.style.cssText = 'max-width:520px;margin:0;';
    content.append(title, description);
    wrapper.append(content);
    container.replaceChildren(wrapper);
};
export const createViewer = (container, createOptions = {}) => {
    let options = createOptions.options || {};
    let registry = createBaseRendererRegistry(createOptions, options);
    let installedRendererInput = undefined;
    let installedPresetInput = undefined;
    let installedPresetsInput = undefined;
    let installedRendererMode = options.rendererMode || 'extend';
    let installedBuiltinRenderers = options.builtinRenderers || 'all';
    let installedAutoRenderersEnabled = resolveAutoRenderersEnabled(options);
    let installedAutoRendererVersion = -1;
    let currentSource = null;
    let currentRenderTarget = null;
    let currentDocumentRoot = null;
    const renderSurfaceState = createFileViewerRenderSurfaceState();
    const requestScope = createFileViewerRequestScope();
    const documentTarget = {
        anchors: { value: [] },
        state: createEmptyFileViewerSearchState(),
    };
    let watermarkEl = null;
    let forcedWatermarkContainerPosition = false;
    let watermarkResizeObserver = null;
    let watermarkMutationObserver = null;
    let watermarkFrame = null;
    const getContainerWindow = () => container.ownerDocument.defaultView || (typeof window !== 'undefined' ? window : undefined);
    const cancelWatermarkFrame = () => {
        if (watermarkFrame === null) {
            return;
        }
        const view = getContainerWindow();
        if (view === null || view === void 0 ? void 0 : view.cancelAnimationFrame) {
            view.cancelAnimationFrame(watermarkFrame);
        }
        watermarkFrame = null;
    };
    const updateWatermarkOverlaySize = () => {
        watermarkFrame = null;
        if (!watermarkEl || watermarkEl.parentElement !== container) {
            return;
        }
        const previousDisplay = watermarkEl.style.display;
        watermarkEl.style.display = 'none';
        const width = Math.max(container.scrollWidth, container.clientWidth);
        const height = Math.max(container.scrollHeight, container.clientHeight);
        watermarkEl.style.display = previousDisplay;
        watermarkEl.style.width = `${width}px`;
        watermarkEl.style.height = `${height}px`;
    };
    const scheduleWatermarkSizeUpdate = () => {
        if (!watermarkEl) {
            return;
        }
        cancelWatermarkFrame();
        const view = getContainerWindow();
        if (view === null || view === void 0 ? void 0 : view.requestAnimationFrame) {
            watermarkFrame = view.requestAnimationFrame(updateWatermarkOverlaySize);
            return;
        }
        updateWatermarkOverlaySize();
    };
    const stopWatermarkObservers = () => {
        cancelWatermarkFrame();
        watermarkResizeObserver === null || watermarkResizeObserver === void 0 ? void 0 : watermarkResizeObserver.disconnect();
        watermarkResizeObserver = null;
        watermarkMutationObserver === null || watermarkMutationObserver === void 0 ? void 0 : watermarkMutationObserver.disconnect();
        watermarkMutationObserver = null;
    };
    const startWatermarkObservers = () => {
        stopWatermarkObservers();
        const view = getContainerWindow();
        if (view === null || view === void 0 ? void 0 : view.ResizeObserver) {
            watermarkResizeObserver = new view.ResizeObserver(() => scheduleWatermarkSizeUpdate());
            watermarkResizeObserver.observe(container);
        }
        if (view === null || view === void 0 ? void 0 : view.MutationObserver) {
            watermarkMutationObserver = new view.MutationObserver(mutations => {
                const onlyWatermarkChanged = mutations.every(mutation => watermarkEl && (mutation.target === watermarkEl ||
                    watermarkEl.contains(mutation.target)));
                if (!onlyWatermarkChanged) {
                    scheduleWatermarkSizeUpdate();
                }
            });
            watermarkMutationObserver.observe(container, {
                attributes: true,
                childList: true,
                subtree: true,
            });
        }
        scheduleWatermarkSizeUpdate();
    };
    const removeWatermarkOverlay = () => {
        stopWatermarkObservers();
        watermarkEl === null || watermarkEl === void 0 ? void 0 : watermarkEl.remove();
        watermarkEl = null;
        if (forcedWatermarkContainerPosition) {
            container.style.position = '';
            forcedWatermarkContainerPosition = false;
        }
    };
    const removeRenderTarget = (target = currentRenderTarget) => {
        if (!target) {
            return;
        }
        removeFileViewerRenderTarget(container, target);
        if (currentRenderTarget === target) {
            currentRenderTarget = null;
        }
    };
    const createRenderTarget = (fillHeight = true) => {
        const surface = createFileViewerRenderSurface(container, {
            styleIsolation: options.styleIsolation,
        });
        const target = surface.host || surface.container;
        if (fillHeight) {
            target.style.height = '100%';
            surface.container.style.height = '100%';
        }
        syncFileViewerRenderSurfaceBackground(target, options);
        currentRenderTarget = target;
        return surface;
    };
    const disposeStaleSession = async (session, target) => {
        var _a;
        try {
            await ((_a = session === null || session === void 0 ? void 0 : session.destroy) === null || _a === void 0 ? void 0 : _a.call(session));
        }
        finally {
            removeRenderTarget(target);
        }
    };
    const syncWatermarkOverlay = () => {
        const inlineStyle = buildFileViewerWatermarkInlineStyle(options.watermark);
        if (!inlineStyle) {
            removeWatermarkOverlay();
            return;
        }
        if (!watermarkEl || watermarkEl.ownerDocument !== container.ownerDocument) {
            watermarkEl = container.ownerDocument.createElement('div');
            watermarkEl.className = 'viewer-watermark';
            watermarkEl.setAttribute('aria-hidden', 'true');
        }
        watermarkEl.style.cssText = inlineStyle;
        const view = getContainerWindow();
        if ((view === null || view === void 0 ? void 0 : view.getComputedStyle) && view.getComputedStyle(container).position === 'static' && !container.style.position) {
            container.style.position = 'relative';
            forcedWatermarkContainerPosition = true;
        }
        if (watermarkEl.parentElement !== container) {
            container.appendChild(watermarkEl);
        }
        startWatermarkObservers();
    };
    const ensureRendererPluginsInstalled = async () => {
        const nextMode = options.rendererMode || 'extend';
        const nextRendererInput = options.renderers;
        const nextPresetInput = options.preset;
        const nextPresetsInput = options.presets;
        const nextBuiltinRenderers = options.builtinRenderers || 'all';
        const nextAutoRenderersEnabled = resolveAutoRenderersEnabled(options);
        const needsAutoPresetBucket = nextAutoRenderersEnabled ||
            hasFileViewerRendererPresetName(nextPresetInput) ||
            hasFileViewerRendererPresetName(nextPresetsInput);
        const nextAutoRendererVersion = needsAutoPresetBucket
            ? getFileViewerAutoRendererPresetVersion()
            : 0;
        if (nextMode === installedRendererMode &&
            nextRendererInput === installedRendererInput &&
            nextPresetInput === installedPresetInput &&
            nextPresetsInput === installedPresetsInput &&
            nextBuiltinRenderers === installedBuiltinRenderers &&
            nextAutoRenderersEnabled === installedAutoRenderersEnabled &&
            nextAutoRendererVersion === installedAutoRendererVersion) {
            return;
        }
        registry = createBaseRendererRegistry(createOptions, options);
        installedRendererMode = nextMode;
        installedRendererInput = nextRendererInput;
        installedPresetInput = nextPresetInput;
        installedPresetsInput = nextPresetsInput;
        installedBuiltinRenderers = nextBuiltinRenderers;
        installedAutoRenderersEnabled = nextAutoRenderersEnabled;
        installedAutoRendererVersion = nextAutoRendererVersion;
        const rendererInputs = [];
        if (nextAutoRenderersEnabled) {
            rendererInputs.push(...listFileViewerAutoRendererPresets());
        }
        rendererInputs.push(...resolveFileViewerRendererPresetInputs(nextPresetInput), ...resolveFileViewerRendererPresetInputs(nextPresetsInput));
        if (nextRendererInput) {
            rendererInputs.push(nextRendererInput);
        }
        const plugins = collectFileViewerRendererPlugins(rendererInputs);
        if (!plugins.length) {
            return;
        }
        const registerHandler = (registration) => {
            const definition = registry.getById(registration.rendererId);
            if (!definition) {
                return;
            }
            registry.register({
                ...definition,
                load: createFileRenderHandlerLoader({
                    handler: registration.handler,
                    rendererId: definition.id,
                    getTarget: context => context.surface.container,
                }),
            });
        };
        await installFileViewerRendererPlugins({
            registry,
            plugins,
            registerHandler,
        });
    };
    const buildCurrentLifecycleContext = () => {
        const source = currentSource || normalizeSource({});
        return buildFileViewerLifecycleContextFromNormalizedSource({
            phase: 'load-complete',
            source,
            version: requestScope.getCurrentVersion(),
            timestamp: Date.now(),
        });
    };
    const runBeforeViewerOperation = async (operation) => {
        const context = buildFileViewerOperationContext(operation, buildCurrentLifecycleContext());
        return runFileViewerBeforeOperation({
            context,
            options,
            onBefore: nextContext => {
                var _a;
                (_a = createOptions.onEvent) === null || _a === void 0 ? void 0 : _a.call(createOptions, { type: 'operation-before', payload: nextContext });
            },
            onCancel: nextContext => {
                var _a;
                (_a = createOptions.onEvent) === null || _a === void 0 ? void 0 : _a.call(createOptions, { type: 'operation-cancel', payload: nextContext });
            },
            onError(error) {
                throw error;
            },
        });
    };
    const getWatermarkInlineStyle = (override) => {
        if (typeof override === 'string') {
            return override;
        }
        return buildFileViewerWatermarkInlineStyle(options.watermark);
    };
    const getCapabilitiesForExtension = (extension) => {
        const targetExtension = extension || (currentSource === null || currentSource === void 0 ? void 0 : currentSource.extension) || '';
        const renderer = registry.getByExtension(targetExtension);
        const zoomState = zoomController.getState();
        if (!renderer) {
            return applyFileViewerZoomAvailability(createUnsupportedAvailability(targetExtension), zoomState);
        }
        return applyFileViewerZoomAvailability(getRendererAvailability(renderer, renderSurfaceState.session), zoomState);
    };
    const emitOperationAvailabilityChange = () => {
        var _a;
        (_a = createOptions.onEvent) === null || _a === void 0 ? void 0 : _a.call(createOptions, {
            type: 'operation-availability-change',
            payload: getCapabilitiesForExtension(),
        });
    };
    const emitZoomChange = (state = zoomController.getState()) => {
        var _a;
        (_a = createOptions.onEvent) === null || _a === void 0 ? void 0 : _a.call(createOptions, {
            type: 'zoom-change',
            payload: state,
        });
    };
    const emitZoomAndOperationAvailabilityChange = (state = zoomController.getState()) => {
        emitZoomChange(state);
        emitOperationAvailabilityChange();
    };
    const emitFitChange = (result) => {
        var _a;
        (_a = createOptions.onEvent) === null || _a === void 0 ? void 0 : _a.call(createOptions, {
            type: 'fit-change',
            payload: result,
        });
    };
    const zoomController = createFileViewerZoomController({
        root: () => currentDocumentRoot || container,
        beforeZoom: runBeforeViewerOperation,
        onChange: state => emitZoomAndOperationAvailabilityChange(state),
    });
    const fitController = createFileViewerFitController({
        root: () => currentDocumentRoot || container,
        getFit: () => options.fit,
        onFit: result => {
            zoomController.refreshProvider();
            viewStateController.refreshProvider();
            emitZoomAndOperationAvailabilityChange();
            emitFitChange(result);
        },
    });
    const viewStateController = createFileViewerViewStateController({
        root: () => currentDocumentRoot || container,
        onChange: change => {
            var _a;
            if ((change.source === 'user' || change.source === 'api') &&
                change.action !== 'fit') {
                fitController.markUserInteraction();
            }
            (_a = createOptions.onEvent) === null || _a === void 0 ? void 0 : _a.call(createOptions, { type: 'view-state-change', payload: change });
        },
    });
    const documentActions = createFileViewerDocumentFeatureControllerActionHandlers({
        root: () => currentDocumentRoot || container,
        searchTarget: documentTarget,
        searchOptions: () => options.search,
        getAiOptions: () => options.ai,
        onSearchChange: state => {
            var _a;
            (_a = createOptions.onEvent) === null || _a === void 0 ? void 0 : _a.call(createOptions, { type: 'search-change', payload: state });
        },
        onLocationChange: anchor => {
            var _a;
            (_a = createOptions.onEvent) === null || _a === void 0 ? void 0 : _a.call(createOptions, { type: 'location-change', payload: anchor });
        },
    });
    zoomController.observe();
    viewStateController.observe();
    fitController.observe();
    const destroyCurrent = async (reason = 'replace') => {
        var _a;
        const session = renderSurfaceState.session;
        const target = currentRenderTarget;
        if (!currentSource && !session && !target) {
            return;
        }
        const source = currentSource;
        const startedAt = Date.now();
        const version = requestScope.getCurrentVersion();
        if (source) {
            await emitLifecycle(options, createOptions.onEvent, 'unload-start', source, version, startedAt, reason);
        }
        await ((_a = session === null || session === void 0 ? void 0 : session.destroy) === null || _a === void 0 ? void 0 : _a.call(session));
        removeRenderTarget(target || undefined);
        currentSource = null;
        currentDocumentRoot = null;
        applyFileViewerRenderSurfaceState(renderSurfaceState, {
            session: null,
            exportAdapter: null,
            thumbnailAdapter: null,
        });
        removeWatermarkOverlay();
        await documentActions.clearDocumentState();
        zoomController.clearProvider();
        viewStateController.clearProvider();
        fitController.resetAutoFit();
        emitZoomAndOperationAvailabilityChange();
        if (source) {
            await emitLifecycle(options, createOptions.onEvent, 'unload-complete', source, version, startedAt, reason);
        }
    };
    const instance = {
        container,
        async prepare() {
            await ensureRendererPluginsInstalled();
        },
        async load(source, loadOptions = {}) {
            const version = requestScope.requestController.createVersion();
            const requestAbortController = requestScope.requestController.createAbortController();
            const loadSignal = createFileViewerLoadSignal(requestAbortController, [
                createOptions.signal,
                loadOptions.signal,
            ]);
            await destroyCurrent('replace');
            await ensureRendererPluginsInstalled();
            if (!requestScope.isCurrentRequest(version)) {
                return null;
            }
            const normalized = normalizeSource(source);
            currentSource = normalized;
            const renderer = registry.getByExtension(normalized.extension);
            const startedAt = Date.now();
            await emitLifecycle(options, createOptions.onEvent, 'load-start', normalized, version, startedAt);
            const surface = createRenderTarget((renderer === null || renderer === void 0 ? void 0 : renderer.id) !== 'office-presentation' && (renderer === null || renderer === void 0 ? void 0 : renderer.id) !== 'office-presentation-binary');
            const target = surface.container;
            const targetHost = surface.host || target;
            currentDocumentRoot = target;
            if (!(renderer === null || renderer === void 0 ? void 0 : renderer.load)) {
                renderMissingRendererState(target, normalized.extension, options);
                applyFileViewerRenderSurfaceState(renderSurfaceState, { session: null });
                syncWatermarkOverlay();
                emitZoomAndOperationAvailabilityChange();
                await emitLifecycle(options, createOptions.onEvent, 'load-complete', normalized, version, startedAt);
                return null;
            }
            let session;
            try {
                session = await renderer.load({
                    source: normalized,
                    surface,
                    options,
                    signal: loadSignal,
                    registerExportAdapter: adapter => {
                        if (requestScope.isCurrentRequest(version)) {
                            applyFileViewerRenderSurfaceState(renderSurfaceState, { exportAdapter: adapter });
                        }
                    },
                    registerThumbnailAdapter: adapter => {
                        if (requestScope.isCurrentRequest(version)) {
                            applyFileViewerRenderSurfaceState(renderSurfaceState, { thumbnailAdapter: adapter });
                        }
                    },
                    renderContext: {
                        renderPurpose: createOptions.renderPurpose || 'preview',
                    },
                });
            }
            catch (error) {
                if (!requestScope.isCurrentRequest(version)) {
                    removeRenderTarget(targetHost);
                    return null;
                }
                removeRenderTarget(targetHost);
                throw error;
            }
            if (!requestScope.isCurrentRequest(version)) {
                await disposeStaleSession(session, targetHost);
                return null;
            }
            applyFileViewerRenderSurfaceState(renderSurfaceState, { session });
            syncWatermarkOverlay();
            zoomController.refreshProvider();
            viewStateController.refreshProvider();
            await documentActions.refreshDocumentIndex({ notify: false });
            await fitController.applyInitialFit({
                skip: hasFileViewerExplicitInitialViewState(options.initialViewState),
            });
            zoomController.refreshProvider();
            viewStateController.refreshProvider();
            emitZoomAndOperationAvailabilityChange();
            await emitLifecycle(options, createOptions.onEvent, 'load-complete', normalized, version, startedAt);
            return session;
        },
        async unload(reason = 'replace') {
            requestScope.requestController.createVersion();
            await destroyCurrent(reason);
        },
        async destroy(reason = 'component-unmount') {
            requestScope.requestController.createVersion();
            await destroyCurrent(reason);
            documentActions.destroyDocumentFeatures();
            zoomController.destroy();
            viewStateController.destroy();
            fitController.destroy();
            removeWatermarkOverlay();
        },
        updateOptions(nextOptions) {
            const previousFit = options.fit;
            options = {
                ...options,
                ...nextOptions,
            };
            syncFileViewerRenderSurfaceBackground(currentRenderTarget, options);
            syncWatermarkOverlay();
            if ('fit' in nextOptions && nextOptions.fit !== previousFit) {
                fitController.resetAutoFit();
                fitController.scheduleFit('resize');
            }
        },
        getCapabilities(extension) {
            return getCapabilitiesForExtension(extension);
        },
        getRenderer(extension) {
            return registry.getByExtension(extension || (currentSource === null || currentSource === void 0 ? void 0 : currentSource.extension) || '');
        },
        getSource() {
            return currentSource;
        },
        registerExportAdapter(adapter) {
            applyFileViewerRenderSurfaceState(renderSurfaceState, { exportAdapter: adapter });
        },
        getExportAdapter() {
            return renderSurfaceState.exportAdapter;
        },
        registerThumbnailAdapter(adapter) {
            applyFileViewerRenderSurfaceState(renderSurfaceState, { thumbnailAdapter: adapter });
        },
        getThumbnailAdapter() {
            return renderSurfaceState.thumbnailAdapter;
        },
        async download(downloadOptions = {}) {
            const source = createFileViewerOriginalSourceStateFromNormalizedSource(currentSource);
            await executeFileViewerDownloadOperation({
                source,
                filename: downloadOptions.filename || resolveFileViewerOperationFilename({
                    source,
                    fallback: DEFAULT_FILE_VIEWER_DOWNLOAD_FILENAME,
                }),
                beforeOperation: runBeforeViewerOperation,
            });
        },
        async exportHtml(exportOptions = {}) {
            return executeFileViewerExportHtmlOperation({
                source: container,
                adapter: renderSurfaceState.exportAdapter,
                download: exportOptions.download,
                filename: exportOptions.filename || resolveFileViewerOperationFilename({
                    filename: resolveFileViewerDisplayFilename(currentSource),
                    fallback: DEFAULT_FILE_VIEWER_EXPORT_FILENAME,
                }),
                title: exportOptions.title || resolveFileViewerOperationFilename({
                    filename: resolveFileViewerDisplayFilename(currentSource),
                    fallback: DEFAULT_FILE_VIEWER_PREVIEW_TITLE,
                }),
                watermarkInlineStyle: getWatermarkInlineStyle(exportOptions.watermarkInlineStyle),
                beforeOperation: runBeforeViewerOperation,
            });
        },
        async print(printOptions = {}) {
            await executeFileViewerPrintOperation({
                source: container,
                adapter: renderSurfaceState.exportAdapter,
                autoPrint: printOptions.autoPrint,
                openWindow: printOptions.openWindow,
                printWindow: printOptions.printWindow,
                title: printOptions.title || resolveFileViewerOperationFilename({
                    filename: resolveFileViewerDisplayFilename(currentSource),
                    fallback: DEFAULT_FILE_VIEWER_PREVIEW_TITLE,
                }),
                watermarkInlineStyle: getWatermarkInlineStyle(printOptions.watermarkInlineStyle),
                mask: printOptions.mask,
                printAvailable: getCapabilitiesForExtension().print,
                beforeOperation: runBeforeViewerOperation,
            });
        },
        async printWithMask(printOptions = {}) {
            var _a, _b, _c, _d, _e;
            const result = await openFileViewerPrintMaskDesignerAsync({
                root: container,
                pages: (_b = (_a = renderSurfaceState.exportAdapter) === null || _a === void 0 ? void 0 : _a.getPrintMaskPages) === null || _b === void 0 ? void 0 : _b.call(_a),
                i18n: options,
                color: (_c = printOptions.mask) === null || _c === void 0 ? void 0 : _c.color,
                initialRegions: (_d = printOptions.mask) === null || _d === void 0 ? void 0 : _d.regions,
                initialStamps: (_e = printOptions.mask) === null || _e === void 0 ? void 0 : _e.stamps,
            });
            if (!(result === null || result === void 0 ? void 0 : result.mask)) {
                return;
            }
            await instance.print({
                ...printOptions,
                mask: result.mask,
            });
        },
        async zoomIn() {
            fitController.markUserInteraction();
            const state = await zoomController.zoomIn();
            emitZoomAndOperationAvailabilityChange(state);
            return state;
        },
        async zoomOut() {
            fitController.markUserInteraction();
            const state = await zoomController.zoomOut();
            emitZoomAndOperationAvailabilityChange(state);
            return state;
        },
        async resetZoom() {
            fitController.markUserInteraction();
            const state = await zoomController.resetZoom();
            emitZoomAndOperationAvailabilityChange(state);
            return state;
        },
        async fitToView(fit) {
            const result = await fitController.fit(fit, { source: 'api', reason: 'api' });
            zoomController.refreshProvider();
            viewStateController.refreshProvider();
            emitZoomAndOperationAvailabilityChange();
            return result;
        },
        getZoomState() {
            return zoomController.getState();
        },
        getViewState() {
            return viewStateController.getState();
        },
        applyViewState(state, applyOptions) {
            fitController.markUserInteraction();
            return viewStateController.applyState(state, applyOptions);
        },
        search(query) {
            return documentActions.searchDocument(query);
        },
        nextSearchResult() {
            return documentActions.nextSearchResult();
        },
        previousSearchResult() {
            return documentActions.previousSearchResult();
        },
        clearSearch() {
            return documentActions.clearDocumentSearch();
        },
        getSearchState() {
            return documentActions.getSearchState();
        },
        collectDocumentAnchors() {
            return documentActions.collectDocumentAnchors({ notify: false });
        },
        getCurrentDocumentAnchor() {
            return documentActions.getCurrentDocumentAnchor();
        },
        scrollToDocumentAnchor(anchor) {
            return documentActions.scrollToLoadedAnchor(anchor);
        },
        scrollToLine(line) {
            return documentActions.scrollToLine(line);
        },
        getDocumentTextChunks(textOptions) {
            return documentActions.getDocumentTextChunks(textOptions);
        },
    };
    return instance;
};
