import { DEFAULT_RENDERER_DEFINITIONS, createFileViewerTranslator, createFileViewerZoomChangeEmitter, normalizeFileViewerErrorMessage, registerFileViewerZoomProvider, resolveFileViewerAssetUrl, resolveFileViewerFitScale, resolveFileViewerRuntimeAssetBaseUrl, unregisterFileViewerZoomProvider, waitForFileViewerNextPaint, } from '@file-viewer/core';
const MIN_ZOOM = 0.25;
const MAX_ZOOM = 3;
const ZOOM_STEP = 0.15;
const MAX_NATIVE_SCALE = 1.5;
const MIN_NATIVE_SCALE = 0.75;
const RERENDER_DELAY_MS = 160;
const pptStyle = `
.ppt-binary-shell{position:relative;box-sizing:border-box;min-height:100%;padding:24px 20px;background:var(--file-viewer-render-surface-background,#eef3f8);color:#1f2d3b;font-family:Aptos,'Segoe UI','PingFang SC','Microsoft YaHei',sans-serif}
.ppt-binary-loading{position:sticky;top:12px;z-index:3;box-sizing:border-box;display:flex;width:max-content;max-width:calc(100% - 24px);align-items:center;gap:10px;margin:0 auto 18px;padding:10px 14px;border:1px solid rgba(42,94,144,.14);border-radius:999px;background:rgba(255,255,255,.92);color:#41556b;box-shadow:0 12px 32px rgba(24,44,64,.12);backdrop-filter:blur(18px)}
.ppt-binary-loading[hidden],.ppt-binary-error[hidden]{display:none!important}
.ppt-binary-loading-dot{width:9px;height:9px;flex:0 0 auto;border-radius:999px;background:#1f9d67;box-shadow:0 0 0 6px rgba(31,157,103,.13)}
.ppt-binary-error{box-sizing:border-box;width:min(680px,calc(100% - 32px));margin:48px auto;padding:24px;border:1px solid rgba(28,43,58,.12);border-radius:16px;background:#fff;color:#1f2d3b;box-shadow:0 16px 42px rgba(25,42,54,.08)}
.ppt-binary-error strong{display:block;margin-bottom:10px;font-size:18px}.ppt-binary-error p{margin:0;color:#607282;line-height:1.7}.ppt-binary-error small{display:block;margin-top:12px;color:#7a8b99;line-height:1.6}
.ppt-binary-slides{display:grid;box-sizing:border-box;width:max-content;min-width:100%;justify-items:center;gap:22px;margin:0 auto}
.ppt-binary-page{position:relative;box-sizing:border-box;overflow:hidden;background:#fff;box-shadow:0 10px 34px rgba(15,23,42,.14);transform-origin:top center;transition:width .18s ease}
.ppt-binary-page canvas{display:block;max-width:none;height:auto;background:#fff}
.ppt-binary-page-error{position:absolute;inset:0;display:grid;place-items:center;padding:24px;background:#fff;color:#8b3a3a;text-align:center}
[data-viewer-theme='dark'] .ppt-binary-shell{background:var(--file-viewer-render-surface-background,#101820);color:#e5eef8}
[data-viewer-theme='dark'] .ppt-binary-loading{border-color:rgba(148,163,184,.18);background:rgba(15,23,42,.9);color:#cbd5e1;box-shadow:0 18px 44px rgba(0,0,0,.26)}
[data-viewer-theme='dark'] .ppt-binary-error{border-color:rgba(148,163,184,.18);background:#151f2b;color:#f8fafc;box-shadow:0 22px 56px rgba(0,0,0,.32)}
[data-viewer-theme='dark'] .ppt-binary-error p,[data-viewer-theme='dark'] .ppt-binary-error small{color:#94a3b8}
[data-viewer-theme='dark'] .ppt-binary-page{color-scheme:only light;forced-color-adjust:none;box-shadow:0 16px 44px rgba(0,0,0,.34)}
@media (prefers-color-scheme:dark){[data-viewer-theme='system'] .ppt-binary-shell{background:var(--file-viewer-render-surface-background,#101820);color:#e5eef8}[data-viewer-theme='system'] .ppt-binary-loading{border-color:rgba(148,163,184,.18);background:rgba(15,23,42,.9);color:#cbd5e1}[data-viewer-theme='system'] .ppt-binary-error{border-color:rgba(148,163,184,.18);background:#151f2b;color:#f8fafc}[data-viewer-theme='system'] .ppt-binary-error p,[data-viewer-theme='system'] .ppt-binary-error small{color:#94a3b8}}
@media (max-width:640px){.ppt-binary-shell{padding:14px 10px}.ppt-binary-slides{gap:14px}.ppt-binary-loading{top:8px;margin-bottom:12px;padding:8px 12px;font-size:13px}}
@media (prefers-reduced-motion:reduce){.ppt-binary-page{transition:none}}
`;
const pptPrintStyle = `
  .ppt-binary-shell { background: #fff !important; padding: 0 !important; }
  .ppt-binary-loading, .ppt-binary-error { display: none !important; }
  .ppt-binary-slides { display: block !important; width: 100% !important; min-width: 0 !important; }
  .ppt-binary-page { width: 100% !important; margin: 0 auto !important; box-shadow: none !important; break-after: page; page-break-after: always; }
  .ppt-binary-page:last-child { break-after: auto; page-break-after: auto; }
  .ppt-binary-page canvas, .ppt-binary-page img { display: block; width: 100% !important; height: auto !important; }
`;
const clampZoom = (value) => Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, value));
const createElement = (documentRef, tagName, className, text) => {
    const element = documentRef.createElement(tagName);
    if (className)
        element.className = className;
    if (text !== undefined)
        element.textContent = text;
    return element;
};
const loadPptModule = async (moduleUrl) => {
    const module = moduleUrl
        // Both hints are intentional. Vite/Rolldown and legacy webpack 4 must
        // leave the configured browser URL as a native dynamic import instead of
        // turning it into a build-time context module.
        ? await import(/* webpackIgnore: true */ /* @vite-ignore */ moduleUrl)
        : await import('@file-viewer/ppt');
    if (!module || typeof module.createPptViewer !== 'function') {
        throw new Error('The configured PPT module does not export createPptViewer().');
    }
    return module;
};
const loadPptRuntime = async (moduleUrl, options) => {
    const module = await loadPptModule(moduleUrl);
    // @file-viewer/ppt 0.3 owns one active document per Worker runtime. A fresh
    // runtime per File Viewer instance prevents concurrent viewers from sharing
    // (and closing) each other's Worker/document lifecycle.
    return module.createPptViewer(options);
};
const canvasToBlob = (canvas) => new Promise(resolve => {
    if (typeof canvas.toBlob === 'function') {
        try {
            canvas.toBlob(resolve, 'image/png');
            return;
        }
        catch {
            // A Worker-owned HTMLCanvasElement cannot be encoded on the main thread.
            // The renderer's direct snapshot path handles that case below.
        }
    }
    try {
        const dataUrl = canvas.toDataURL('image/png');
        fetch(dataUrl).then(response => response.blob()).then(resolve, () => resolve(null));
    }
    catch {
        resolve(null);
    }
});
const buildExportAdapter = (pages, targetWindow, captureSlideDataUrls) => ({
    print: true,
    exportHtml: true,
    includeDocumentStyles: false,
    beforeSnapshot: () => waitForFileViewerNextPaint(targetWindow || undefined),
    getPrintMaskPages: () => pages,
    printStyle: pptPrintStyle,
    toHtml: async () => {
        const dataUrls = await captureSlideDataUrls();
        const slides = dataUrls.map((dataUrl, index) => {
            return `<section class="ppt-binary-page" data-viewer-print-page-index="${index}"><img src="${dataUrl}" alt="Slide ${index + 1}"></section>`;
        }).join('');
        return `<div class="ppt-binary-shell"><div class="ppt-binary-slides">${slides}</div></div>`;
    },
});
export default async function renderPpt(buffer, target, _type, context) {
    var _a, _b, _c, _d, _e, _f, _g, _h, _j, _k, _l, _m, _o;
    const t = createFileViewerTranslator(context === null || context === void 0 ? void 0 : context.options);
    const documentRef = target.ownerDocument || document;
    const targetWindow = documentRef.defaultView || (typeof window !== 'undefined' ? window : null);
    const zoomEmitter = createFileViewerZoomChangeEmitter();
    const style = createElement(documentRef, 'style');
    style.textContent = pptStyle;
    const shell = createElement(documentRef, 'div', 'ppt-binary-shell');
    shell.dataset.viewerZoomProvider = 'ppt';
    const loading = createElement(documentRef, 'div', 'ppt-binary-loading');
    loading.setAttribute('role', 'status');
    loading.setAttribute('aria-live', 'polite');
    const loadingLabel = createElement(documentRef, 'span', undefined, t('presentation.ppt.state.loading'));
    loading.append(createElement(documentRef, 'span', 'ppt-binary-loading-dot'), loadingLabel);
    const error = createElement(documentRef, 'div', 'ppt-binary-error');
    error.hidden = true;
    const errorTitle = createElement(documentRef, 'strong', undefined, t('presentation.ppt.error.title'));
    const errorMessage = createElement(documentRef, 'p');
    const errorHint = createElement(documentRef, 'small', undefined, t('presentation.ppt.error.assetHint'));
    error.append(errorTitle, errorMessage, errorHint);
    const slides = createElement(documentRef, 'div', 'ppt-binary-slides');
    shell.append(loading, error, slides);
    target.replaceChildren(style, shell);
    let pptRuntime = null;
    let pptDocument = null;
    let mountedViewer = null;
    let resourceClosePromise = null;
    let pages = [];
    const canvases = [];
    let canvasStateObserver = null;
    let snapshotRuntimeOptions = null;
    let snapshotModuleUrl;
    let currentScale = 1;
    let disposed = false;
    let ready = false;
    let renderGeneration = 0;
    let rerenderTimer = null;
    let statsRefreshTimer = null;
    let statsRefreshGeneration = 0;
    const throwIfAborted = () => {
        var _a, _b;
        if (disposed || ((_a = context === null || context === void 0 ? void 0 : context.signal) === null || _a === void 0 ? void 0 : _a.aborted)) {
            throw ((_b = context === null || context === void 0 ? void 0 : context.signal) === null || _b === void 0 ? void 0 : _b.reason) || new DOMException('PPT rendering aborted.', 'AbortError');
        }
    };
    const clearRerenderTimer = () => {
        if (rerenderTimer !== null) {
            if (targetWindow)
                targetWindow.clearTimeout(rerenderTimer);
            else
                clearTimeout(rerenderTimer);
            rerenderTimer = null;
        }
    };
    const clearStatsRefreshTimer = () => {
        if (statsRefreshTimer !== null) {
            if (targetWindow)
                targetWindow.clearTimeout(statsRefreshTimer);
            else
                clearTimeout(statsRefreshTimer);
            statsRefreshTimer = null;
        }
    };
    const refreshRuntimeStats = async () => {
        const mounted = mountedViewer;
        if (!mounted || disposed)
            return;
        const generation = ++statsRefreshGeneration;
        try {
            const stats = await mounted.cacheStats();
            if (generation !== statsRefreshGeneration || mounted !== mountedViewer || disposed)
                return;
            const publishNumber = (key, value) => {
                if (Number.isFinite(value))
                    shell.dataset[key] = String(value);
            };
            shell.dataset.pptCacheEnabled = String(stats.enabled);
            shell.dataset.pptCacheAvailable = String(stats.available);
            publishNumber('pptCacheEntries', stats.entries);
            publishNumber('pptCacheBytes', stats.bytes);
            publishNumber('pptAttachedCanvases', stats.attachedCanvases);
            publishNumber('pptActiveCanvases', stats.activeCanvases);
            publishNumber('pptWasmMemoryBytes', stats.wasmMemoryBytes);
        }
        catch {
            // Diagnostics must never interfere with rendering or cleanup.
        }
    };
    const scheduleRuntimeStatsRefresh = () => {
        clearStatsRefreshTimer();
        const run = () => {
            statsRefreshTimer = null;
            void refreshRuntimeStats();
        };
        statsRefreshTimer = targetWindow
            ? targetWindow.setTimeout(run, 200)
            : setTimeout(run, 200);
    };
    const closeResources = () => {
        if (resourceClosePromise)
            return resourceClosePromise;
        const mountedToClose = mountedViewer;
        const documentToClose = pptDocument;
        const runtimeToClose = pptRuntime;
        mountedViewer = null;
        pptDocument = null;
        pptRuntime = null;
        canvasStateObserver === null || canvasStateObserver === void 0 ? void 0 : canvasStateObserver.disconnect();
        canvasStateObserver = null;
        clearStatsRefreshTimer();
        if (!mountedToClose && !documentToClose && !runtimeToClose)
            return Promise.resolve();
        resourceClosePromise = (async () => {
            try {
                if (mountedToClose) {
                    await mountedToClose.close();
                }
                else if (documentToClose && !documentToClose.closed) {
                    await documentToClose.close();
                }
            }
            finally {
                await (runtimeToClose === null || runtimeToClose === void 0 ? void 0 : runtimeToClose.close());
            }
        })();
        return resourceClosePromise;
    };
    const disposeResources = () => {
        void closeResources().catch(closeError => {
            console.warn('PPT runtime cleanup warning:', closeError);
        });
    };
    const abort = () => {
        renderGeneration += 1;
        clearRerenderTimer();
        clearStatsRefreshTimer();
        // The signal can fire while runtime.mount() is still awaiting its first
        // Worker response. Let the surrounding try/catch attach the returned
        // document before closing it; eagerly closing only the runtime here would
        // otherwise leave that late document outside the owned lifecycle.
    };
    (_a = context === null || context === void 0 ? void 0 : context.signal) === null || _a === void 0 ? void 0 : _a.addEventListener('abort', abort, { once: true });
    const getZoomState = () => ({
        scale: currentScale,
        label: `${Math.round(currentScale * 100)}%`,
        canZoomIn: currentScale < MAX_ZOOM,
        canZoomOut: currentScale > MIN_ZOOM,
        canReset: Math.abs(currentScale - 1) > 0.001,
        minScale: MIN_ZOOM,
        maxScale: MAX_ZOOM,
    });
    const applyCssZoom = () => {
        if (!pptDocument)
            return;
        const width = Math.max(1, Math.round(pptDocument.width * currentScale));
        const height = Math.max(1, Math.round(pptDocument.height * currentScale));
        pages.forEach((page, index) => {
            page.style.width = `${width}px`;
            page.style.height = `${height}px`;
            page.style.maxWidth = 'none';
            const canvas = canvases[index];
            if (canvas) {
                canvas.style.width = `${width}px`;
                canvas.style.height = `${height}px`;
                canvas.style.maxWidth = 'none';
            }
        });
    };
    const renderActiveSlides = async (generation) => {
        var _a, _b, _c, _d;
        const document = pptDocument;
        const mounted = mountedViewer;
        if (!document || !mounted)
            return;
        const pixelRatio = Math.min(2, Math.max(1, (targetWindow === null || targetWindow === void 0 ? void 0 : targetWindow.devicePixelRatio) || 1));
        const nativeScale = Math.min(MAX_NATIVE_SCALE, Math.max(MIN_NATIVE_SCALE, currentScale * pixelRatio));
        const activeIndices = canvases
            .map((canvas, index) => ({ index, state: canvas.dataset.renderState }))
            .filter(item => item.state === 'rendering' || item.state === 'rendered')
            .map(item => item.index);
        for (const index of activeIndices) {
            throwIfAborted();
            if (generation !== renderGeneration)
                return;
            const canvas = canvases[index];
            const page = pages[index];
            try {
                if (canvas.dataset.renderState === 'rendering') {
                    await mounted.renderSlide(index);
                    if (disposed || ((_a = context === null || context === void 0 ? void 0 : context.signal) === null || _a === void 0 ? void 0 : _a.aborted) || generation !== renderGeneration)
                        return;
                }
                if (canvas.dataset.renderState !== 'rendered')
                    continue;
                const result = await document.renderSlide(index, canvas, {
                    scale: nativeScale,
                    pixelRatio: 1,
                });
                if (disposed || ((_b = context === null || context === void 0 ? void 0 : context.signal) === null || _b === void 0 ? void 0 : _b.aborted) || generation !== renderGeneration)
                    return;
                if (result.cancelled)
                    continue;
                if (!page.isConnected)
                    return;
                (_c = page.querySelector('.ppt-binary-page-error')) === null || _c === void 0 ? void 0 : _c.remove();
            }
            catch (slideError) {
                if (disposed || ((_d = context === null || context === void 0 ? void 0 : context.signal) === null || _d === void 0 ? void 0 : _d.aborted) || generation !== renderGeneration || !page.isConnected) {
                    return;
                }
                const message = createElement(documentRef, 'div', 'ppt-binary-page-error', `${t('presentation.ppt.error.parseFailed')} (${index + 1})`);
                page.append(message);
                console.warn(`PPT slide ${index + 1} render warning:`, slideError);
            }
            applyCssZoom();
            if (index + 1 < canvases.length) {
                await waitForFileViewerNextPaint(targetWindow || undefined);
            }
        }
    };
    const scheduleQualityRender = () => {
        clearRerenderTimer();
        const generation = renderGeneration;
        const run = () => {
            rerenderTimer = null;
            void renderActiveSlides(generation).catch(renderError => {
                var _a;
                if (!disposed && !((_a = context === null || context === void 0 ? void 0 : context.signal) === null || _a === void 0 ? void 0 : _a.aborted)) {
                    console.warn('PPT quality rerender warning:', renderError);
                }
            });
        };
        rerenderTimer = targetWindow
            ? targetWindow.setTimeout(run, RERENDER_DELAY_MS)
            : setTimeout(run, RERENDER_DELAY_MS);
    };
    const setZoom = (scale) => {
        currentScale = clampZoom(scale);
        renderGeneration += 1;
        applyCssZoom();
        zoomEmitter.emit();
        scheduleQualityRender();
        return getZoomState();
    };
    const fit = (request) => {
        var _a, _b;
        if (!pptDocument) {
            return {
                applied: false,
                mode: request.mode,
                resize: request.resize,
                source: request.source,
                reason: 'ppt-not-ready',
                provider: 'zoom',
            };
        }
        const scale = resolveFileViewerFitScale({
            mode: request.mode,
            viewportWidth: Math.max(1, (request.viewportWidth || shell.clientWidth || 0) - request.padding * 2),
            viewportHeight: Math.max(1, (request.viewportHeight || shell.clientHeight || 0) - request.padding * 2),
            contentWidth: pptDocument.width,
            contentHeight: pptDocument.height,
            currentScale,
            minScale: (_a = request.minScale) !== null && _a !== void 0 ? _a : MIN_ZOOM,
            maxScale: (_b = request.maxScale) !== null && _b !== void 0 ? _b : MAX_ZOOM,
        });
        if (!scale) {
            return {
                applied: false,
                mode: request.mode,
                resize: request.resize,
                source: request.source,
                reason: 'unmeasurable',
                provider: 'zoom',
            };
        }
        const state = setZoom(scale);
        return {
            applied: true,
            mode: request.mode,
            resize: request.resize,
            scale: state.scale,
            source: request.source,
            provider: 'zoom',
        };
    };
    const captureDirectSlides = async (slideIndices, encode) => {
        const runtimeOptions = snapshotRuntimeOptions;
        if (!runtimeOptions)
            throw new Error(t('presentation.ppt.error.parseFailed'));
        const runtime = await loadPptRuntime(snapshotModuleUrl, {
            ...runtimeOptions,
            worker: false,
            cache: false,
        });
        let document = null;
        try {
            document = await runtime.open(buffer.slice(0));
            const output = [];
            for (let position = 0; position < slideIndices.length; position += 1) {
                const slideIndex = slideIndices[position];
                throwIfAborted();
                const canvas = createElement(documentRef, 'canvas');
                try {
                    await document.renderSlide(slideIndex, canvas, {
                        scale: MAX_NATIVE_SCALE,
                        pixelRatio: 1,
                    });
                    throwIfAborted();
                    output.push(await encode(canvas));
                }
                finally {
                    canvas.width = 1;
                    canvas.height = 1;
                }
                // Direct-mode snapshots run on the main thread. Yield in bounded
                // batches so a long deck does not monopolize input and painting.
                if (position + 1 < slideIndices.length && (position + 1) % 4 === 0) {
                    await waitForFileViewerNextPaint(targetWindow || undefined);
                }
            }
            return output;
        }
        finally {
            if (document && !document.closed)
                await document.close().catch(() => { });
            await runtime.close().catch(() => { });
        }
    };
    try {
        throwIfAborted();
        const presentationOptions = (_b = context === null || context === void 0 ? void 0 : context.options) === null || _b === void 0 ? void 0 : _b.presentation;
        const runtimeBaseUrl = resolveFileViewerRuntimeAssetBaseUrl(documentRef);
        const moduleUrl = (presentationOptions === null || presentationOptions === void 0 ? void 0 : presentationOptions.pptModuleUrl)
            ? resolveFileViewerAssetUrl(presentationOptions.pptModuleUrl, String(presentationOptions.pptModuleUrl), { documentBaseUrl: runtimeBaseUrl })
            : undefined;
        const resolvePptAssetUrl = (value) => value
            ? resolveFileViewerAssetUrl(value, String(value), { documentBaseUrl: runtimeBaseUrl })
            : undefined;
        const runtimeOptions = {
            worker: (_c = presentationOptions === null || presentationOptions === void 0 ? void 0 : presentationOptions.pptWorker) !== null && _c !== void 0 ? _c : 'auto',
            cache: presentationOptions === null || presentationOptions === void 0 ? void 0 : presentationOptions.pptCache,
            wasmUrl: resolvePptAssetUrl(presentationOptions === null || presentationOptions === void 0 ? void 0 : presentationOptions.pptWasmUrl),
            fontUrl: resolvePptAssetUrl(presentationOptions === null || presentationOptions === void 0 ? void 0 : presentationOptions.pptFontUrl),
            workerUrl: resolvePptAssetUrl(presentationOptions === null || presentationOptions === void 0 ? void 0 : presentationOptions.pptWorkerUrl),
        };
        snapshotModuleUrl = moduleUrl;
        snapshotRuntimeOptions = runtimeOptions;
        const runtime = await loadPptRuntime(moduleUrl, runtimeOptions);
        pptRuntime = runtime;
        shell.dataset.pptRuntimeMode = runtime.mode;
        throwIfAborted();
        slides.className = 'ppt-binary-mount';
        const mounted = await runtime.mount(slides, buffer, {
            replace: true,
            className: 'ppt-binary-slides',
            scale: 1,
            pixelRatio: 1,
            virtualize: (presentationOptions === null || presentationOptions === void 0 ? void 0 : presentationOptions.pptVirtualize) !== false,
            rootMargin: presentationOptions === null || presentationOptions === void 0 ? void 0 : presentationOptions.pptVirtualRootMargin,
            releaseDelayMs: presentationOptions === null || presentationOptions === void 0 ? void 0 : presentationOptions.pptReleaseDelayMs,
        });
        mountedViewer = mounted;
        pptDocument = mounted.document;
        shell.dataset.pptVirtualized = String(mounted.virtualized);
        throwIfAborted();
        // @file-viewer/ppt 0.3 supplies standalone defaults as inline styles.
        // File Viewer owns the surrounding scroll/zoom geometry, so normalize the
        // generated root before sizing pages. Otherwise the runtime's 20px padding
        // and max-width constraints squash slides while leaving their CSS height at
        // the requested zoom, especially on mobile and above 100%.
        Object.assign(mounted.root.style, {
            width: 'max-content',
            minWidth: '100%',
            maxWidth: 'none',
            padding: '0px',
            background: 'transparent',
            justifyContent: '',
            gap: '',
        });
        pages = Array.from(mounted.root.children).map((element, index) => {
            const page = element;
            page.classList.add('ppt-binary-page');
            page.dataset.slideIndex = String(index);
            page.dataset.viewerPrintPageIndex = String(index);
            page.style.maxWidth = 'none';
            return page;
        });
        canvases.push(...mounted.canvases);
        canvases.forEach((canvas, index) => {
            canvas.setAttribute('role', 'img');
            canvas.setAttribute('aria-label', `Slide ${index + 1}`);
            canvas.style.maxWidth = 'none';
        });
        const shellStyle = (_d = targetWindow === null || targetWindow === void 0 ? void 0 : targetWindow.getComputedStyle) === null || _d === void 0 ? void 0 : _d.call(targetWindow, shell);
        const horizontalPadding = (Number.parseFloat((shellStyle === null || shellStyle === void 0 ? void 0 : shellStyle.paddingLeft) || '') || 0) +
            (Number.parseFloat((shellStyle === null || shellStyle === void 0 ? void 0 : shellStyle.paddingRight) || '') || 0);
        const availableWidth = Math.max(0, (shell.clientWidth || target.clientWidth || 0) - horizontalPadding);
        currentScale = availableWidth > 0
            ? clampZoom(Math.min(1, availableWidth / pptDocument.width))
            : 1;
        applyCssZoom();
        registerFileViewerZoomProvider(shell, {
            zoomIn: () => setZoom(currentScale + ZOOM_STEP),
            zoomOut: () => setZoom(currentScale - ZOOM_STEP),
            resetZoom: () => setZoom(1),
            setZoom,
            fit,
            getState: getZoomState,
            subscribe: zoomEmitter.subscribe,
        });
        const MutationObserverCtor = targetWindow === null || targetWindow === void 0 ? void 0 : targetWindow.MutationObserver;
        if (MutationObserverCtor) {
            canvasStateObserver = new MutationObserverCtor(records => {
                if (records.some(record => {
                    var _a;
                    return (((_a = record.target.dataset) === null || _a === void 0 ? void 0 : _a.renderState) === 'rendered');
                })) {
                    scheduleQualityRender();
                }
                scheduleRuntimeStatsRefresh();
            });
            canvases.forEach(canvas => {
                canvasStateObserver === null || canvasStateObserver === void 0 ? void 0 : canvasStateObserver.observe(canvas, {
                    attributes: true,
                    attributeFilter: ['data-render-state'],
                });
            });
        }
        ready = true;
        loading.hidden = true;
        void refreshRuntimeStats();
        (_e = context === null || context === void 0 ? void 0 : context.onProgressiveRender) === null || _e === void 0 ? void 0 : _e.call(context);
        (_f = context === null || context === void 0 ? void 0 : context.registerExportAdapter) === null || _f === void 0 ? void 0 : _f.call(context, buildExportAdapter(pages, targetWindow, () => captureDirectSlides(canvases.map((_, index) => index), canvas => canvas.toDataURL('image/png'))));
        (_g = context === null || context === void 0 ? void 0 : context.registerThumbnailAdapter) === null || _g === void 0 ? void 0 : _g.call(context, {
            captureSource: 'rendered',
            beforeCapture: () => {
                if (!ready)
                    throw new Error(t('presentation.ppt.error.parseFailed'));
            },
            capture: async () => (await captureDirectSlides([0], canvasToBlob))[0] || null,
            getTarget: () => pages[0] || null,
        });
        zoomEmitter.emit();
    }
    catch (renderError) {
        if (disposed || ((_h = context === null || context === void 0 ? void 0 : context.signal) === null || _h === void 0 ? void 0 : _h.aborted)) {
            clearRerenderTimer();
            (_j = context === null || context === void 0 ? void 0 : context.registerExportAdapter) === null || _j === void 0 ? void 0 : _j.call(context, null);
            (_k = context === null || context === void 0 ? void 0 : context.registerThumbnailAdapter) === null || _k === void 0 ? void 0 : _k.call(context, null);
            unregisterFileViewerZoomProvider(shell);
            await closeResources().catch(() => { });
            throw renderError;
        }
        loading.hidden = true;
        error.hidden = false;
        errorMessage.textContent = normalizeFileViewerErrorMessage(renderError, context === null || context === void 0 ? void 0 : context.options) ||
            t('presentation.ppt.error.parseFailed');
        (_l = context === null || context === void 0 ? void 0 : context.registerExportAdapter) === null || _l === void 0 ? void 0 : _l.call(context, null);
        (_m = context === null || context === void 0 ? void 0 : context.registerThumbnailAdapter) === null || _m === void 0 ? void 0 : _m.call(context, null);
        unregisterFileViewerZoomProvider(shell);
        await closeResources().catch(() => { });
        throw renderError;
    }
    finally {
        (_o = context === null || context === void 0 ? void 0 : context.signal) === null || _o === void 0 ? void 0 : _o.removeEventListener('abort', abort);
    }
    return {
        $el: shell,
        unmount() {
            var _a, _b;
            disposed = true;
            renderGeneration += 1;
            clearRerenderTimer();
            clearStatsRefreshTimer();
            (_a = context === null || context === void 0 ? void 0 : context.registerExportAdapter) === null || _a === void 0 ? void 0 : _a.call(context, null);
            (_b = context === null || context === void 0 ? void 0 : context.registerThumbnailAdapter) === null || _b === void 0 ? void 0 : _b.call(context, null);
            unregisterFileViewerZoomProvider(shell);
            disposeResources();
            target.replaceChildren();
        },
    };
}
const binaryPresentationDefinition = DEFAULT_RENDERER_DEFINITIONS.find(definition => definition.id === 'office-presentation-binary');
if (!binaryPresentationDefinition) {
    throw new Error('@file-viewer/renderer-presentation/ppt could not locate the core PPT renderer definition.');
}
export const binaryPresentationRendererDefinition = binaryPresentationDefinition;
export const renderFileViewerBinaryPresentation = renderPpt;
export const pptRenderer = {
    id: 'file-viewer-renderer-presentation-ppt',
    label: 'Flyfish File Viewer PPT renderer',
    definitions: [binaryPresentationRendererDefinition],
    handlers: [
        {
            rendererId: binaryPresentationRendererDefinition.id,
            handler: renderFileViewerBinaryPresentation,
        },
    ],
};
