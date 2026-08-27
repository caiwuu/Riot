import { findFileViewerViewStateProvider, findFileViewerZoomProvider, } from './dom/index.js';
export const FILE_VIEWER_FIT_MODES = [
    'auto',
    'contain',
    'cover',
    'width',
    'height',
    'actual',
    'scale-down',
];
export const FILE_VIEWER_FIT_RESIZE_MODES = [
    'until-interaction',
    'always',
    'initial',
];
const DEFAULT_FILE_VIEWER_FIT_RESIZE = 'until-interaction';
const DEFAULT_FILE_VIEWER_FIT_PADDING = 0;
const MAX_FILE_VIEWER_FIT_RETRIES = 8;
const isRecord = (value) => {
    return !!value && typeof value === 'object' && !Array.isArray(value);
};
export const isFileViewerFitMode = (value) => {
    return typeof value === 'string' && FILE_VIEWER_FIT_MODES.includes(value);
};
export const isFileViewerFitResize = (value) => {
    return typeof value === 'string' &&
        FILE_VIEWER_FIT_RESIZE_MODES.includes(value);
};
const normalizePositiveNumber = (value) => {
    const nextValue = Number(value);
    return Number.isFinite(nextValue) && nextValue > 0 ? nextValue : undefined;
};
const normalizePadding = (value) => {
    const nextValue = Number(value);
    return Number.isFinite(nextValue) && nextValue > 0 ? nextValue : DEFAULT_FILE_VIEWER_FIT_PADDING;
};
export const normalizeFileViewerFitOptions = (fit) => {
    if (fit === undefined || fit === null || fit === false) {
        return null;
    }
    if (isFileViewerFitMode(fit)) {
        return {
            mode: fit,
            resize: DEFAULT_FILE_VIEWER_FIT_RESIZE,
            padding: DEFAULT_FILE_VIEWER_FIT_PADDING,
        };
    }
    if (!isRecord(fit)) {
        return null;
    }
    const minScale = normalizePositiveNumber(fit.minScale);
    const rawMaxScale = normalizePositiveNumber(fit.maxScale);
    const maxScale = minScale !== undefined && rawMaxScale !== undefined && rawMaxScale < minScale
        ? minScale
        : rawMaxScale;
    return {
        mode: isFileViewerFitMode(fit.mode) ? fit.mode : 'auto',
        resize: isFileViewerFitResize(fit.resize) ? fit.resize : DEFAULT_FILE_VIEWER_FIT_RESIZE,
        padding: normalizePadding(fit.padding),
        minScale,
        maxScale,
    };
};
const clampScale = (scale, minScale, maxScale) => {
    let nextScale = scale;
    if (Number.isFinite(minScale)) {
        nextScale = Math.max(Number(minScale), nextScale);
    }
    if (Number.isFinite(maxScale)) {
        nextScale = Math.min(Number(maxScale), nextScale);
    }
    return nextScale;
};
export const resolveFileViewerFitScale = ({ mode, viewportWidth, viewportHeight, contentWidth, contentHeight, minScale, maxScale, }) => {
    if (viewportWidth <= 0 ||
        viewportHeight <= 0 ||
        contentWidth <= 0 ||
        contentHeight <= 0) {
        return undefined;
    }
    const widthScale = viewportWidth / contentWidth;
    const heightScale = viewportHeight / contentHeight;
    const containScale = Math.min(widthScale, heightScale);
    const coverScale = Math.max(widthScale, heightScale);
    const nextScale = (() => {
        switch (mode) {
            case 'cover':
                return coverScale;
            case 'width':
                return widthScale;
            case 'height':
                return heightScale;
            case 'actual':
                return 1;
            case 'scale-down':
                return Math.min(1, containScale);
            case 'auto':
            case 'contain':
            default:
                return containScale;
        }
    })();
    if (!Number.isFinite(nextScale) || nextScale <= 0) {
        return undefined;
    }
    return clampScale(nextScale, minScale, maxScale);
};
export const hasFileViewerExplicitInitialViewState = (state) => {
    var _a, _b;
    if (!state) {
        return false;
    }
    const scale = Number((_a = state.scale) !== null && _a !== void 0 ? _a : (_b = state.zoom) === null || _b === void 0 ? void 0 : _b.scale);
    if (Number.isFinite(scale) && scale > 0) {
        return true;
    }
    if (Number.isFinite(state.page) || Number.isFinite(state.rotation)) {
        return true;
    }
    if (state.navigation && Object.keys(state.navigation).length > 0) {
        return true;
    }
    if (state.extra && Object.keys(state.extra).length > 0) {
        return true;
    }
    const scroll = state.scroll || {};
    return ['top', 'left', 'topRatio', 'leftRatio'].some(key => {
        const value = scroll[key];
        return Number.isFinite(value);
    });
};
const createNoopFitResult = (fit, reason, source = 'viewer', provider = 'none') => ({
    applied: false,
    mode: fit.mode,
    resize: fit.resize,
    source,
    reason,
    provider,
});
const withProviderResult = (result, request, provider) => ({
    applied: !!(result === null || result === void 0 ? void 0 : result.applied),
    mode: (result === null || result === void 0 ? void 0 : result.mode) || request.mode,
    resize: (result === null || result === void 0 ? void 0 : result.resize) || request.resize,
    scale: result === null || result === void 0 ? void 0 : result.scale,
    source: (result === null || result === void 0 ? void 0 : result.source) || request.source,
    reason: result === null || result === void 0 ? void 0 : result.reason,
    provider: (result === null || result === void 0 ? void 0 : result.provider) || provider,
    state: result === null || result === void 0 ? void 0 : result.state,
});
const isRetriableFitReason = (reason) => {
    return reason === 'pending' ||
        reason === 'retry' ||
        reason === 'image-not-ready' ||
        reason === 'unmeasurable';
};
const readViewportSize = (root) => {
    var _a;
    const rect = (_a = root.getBoundingClientRect) === null || _a === void 0 ? void 0 : _a.call(root);
    return {
        width: Math.max(0, (rect === null || rect === void 0 ? void 0 : rect.width) || root.clientWidth || 0),
        height: Math.max(0, (rect === null || rect === void 0 ? void 0 : rect.height) || root.clientHeight || 0),
    };
};
export const createFileViewerFitController = ({ root, enabled, getFit = () => undefined, onFit, }) => {
    let resizeObserver = null;
    let resizeFrame = null;
    let retryTimer = null;
    let userInteracted = false;
    let retryCount = 0;
    const getWindow = () => {
        var _a;
        const currentRoot = root();
        return ((_a = currentRoot === null || currentRoot === void 0 ? void 0 : currentRoot.ownerDocument) === null || _a === void 0 ? void 0 : _a.defaultView) ||
            (typeof window !== 'undefined' ? window : undefined);
    };
    const cancelFrame = () => {
        if (resizeFrame === null) {
            return;
        }
        const view = getWindow();
        if (view === null || view === void 0 ? void 0 : view.cancelAnimationFrame) {
            view.cancelAnimationFrame(resizeFrame);
        }
        resizeFrame = null;
    };
    const cancelRetry = () => {
        if (retryTimer) {
            clearTimeout(retryTimer);
        }
        retryTimer = null;
    };
    const resolveFit = (fit, reason) => {
        const configured = fit === undefined ? getFit() : fit;
        if (configured !== undefined && configured !== null) {
            return normalizeFileViewerFitOptions(configured);
        }
        return reason === 'api' ? normalizeFileViewerFitOptions('auto') : null;
    };
    const scheduleRetry = (fit, source, reason) => {
        if (retryCount >= MAX_FILE_VIEWER_FIT_RETRIES) {
            return;
        }
        cancelRetry();
        retryCount += 1;
        retryTimer = setTimeout(() => {
            retryTimer = null;
            void runFit(fit, { source, reason: reason === 'api' ? 'api' : 'retry' });
        }, 32 * retryCount);
    };
    const runFit = async (fit, runOptions = {}) => {
        const source = runOptions.source || 'api';
        const reason = runOptions.reason || 'api';
        const normalized = resolveFit(fit, reason);
        if (!normalized) {
            return {
                applied: false,
                mode: 'auto',
                resize: DEFAULT_FILE_VIEWER_FIT_RESIZE,
                source,
                reason: 'not-configured',
                provider: 'none',
            };
        }
        if ((enabled === null || enabled === void 0 ? void 0 : enabled()) === false) {
            return createNoopFitResult(normalized, 'disabled', source);
        }
        const currentRoot = root();
        if (!currentRoot) {
            scheduleRetry(fit, source, reason);
            return createNoopFitResult(normalized, 'no-root', source);
        }
        const viewport = readViewportSize(currentRoot);
        if (viewport.width <= 0 || viewport.height <= 0) {
            scheduleRetry(fit, source, reason);
            return createNoopFitResult(normalized, 'zero-size', source);
        }
        const request = {
            ...normalized,
            source,
            reason,
            viewportWidth: Math.max(0, viewport.width - normalized.padding * 2),
            viewportHeight: Math.max(0, viewport.height - normalized.padding * 2),
            container: currentRoot,
        };
        const viewStateProvider = findFileViewerViewStateProvider(currentRoot);
        let lastProviderResult = null;
        if (viewStateProvider === null || viewStateProvider === void 0 ? void 0 : viewStateProvider.fit) {
            const result = withProviderResult(await viewStateProvider.fit(request), request, 'view-state');
            if (result.applied) {
                retryCount = 0;
                onFit === null || onFit === void 0 ? void 0 : onFit(result);
                return result;
            }
            lastProviderResult = result;
        }
        const zoomProvider = findFileViewerZoomProvider(currentRoot);
        if (zoomProvider === null || zoomProvider === void 0 ? void 0 : zoomProvider.fit) {
            const result = withProviderResult(await zoomProvider.fit(request), request, 'zoom');
            if (result.applied) {
                retryCount = 0;
                onFit === null || onFit === void 0 ? void 0 : onFit(result);
                return result;
            }
            lastProviderResult = result;
        }
        const noop = createNoopFitResult(normalized, viewStateProvider || zoomProvider
            ? 'unsupported'
            : 'no-provider', source);
        if (noop.reason === 'no-provider' || isRetriableFitReason(lastProviderResult === null || lastProviderResult === void 0 ? void 0 : lastProviderResult.reason)) {
            scheduleRetry(fit, source, reason);
        }
        return noop;
    };
    const scheduleFit = (reason = 'resize') => {
        const normalized = normalizeFileViewerFitOptions(getFit());
        if (!normalized || normalized.resize === 'initial') {
            return;
        }
        if (normalized.resize === 'until-interaction' && userInteracted) {
            return;
        }
        cancelFrame();
        const view = getWindow();
        const run = () => {
            resizeFrame = null;
            void runFit(undefined, { source: 'viewer', reason });
        };
        if (view === null || view === void 0 ? void 0 : view.requestAnimationFrame) {
            resizeFrame = view.requestAnimationFrame(run);
            return;
        }
        resizeFrame = setTimeout(run, 16);
    };
    const disconnectObserver = () => {
        cancelFrame();
        resizeObserver === null || resizeObserver === void 0 ? void 0 : resizeObserver.disconnect();
        resizeObserver = null;
    };
    const controller = {
        normalize: normalizeFileViewerFitOptions,
        fit: async (fit, options) => {
            cancelRetry();
            userInteracted = false;
            return runFit(fit, {
                source: (options === null || options === void 0 ? void 0 : options.source) || 'api',
                reason: (options === null || options === void 0 ? void 0 : options.reason) || 'api',
            });
        },
        applyInitialFit: async (initialOptions = {}) => {
            cancelRetry();
            retryCount = 0;
            userInteracted = false;
            if (initialOptions.skip) {
                const normalized = normalizeFileViewerFitOptions(getFit()) ||
                    normalizeFileViewerFitOptions('auto');
                return createNoopFitResult(normalized, 'initial-view-state', 'initial');
            }
            return runFit(undefined, { source: 'initial', reason: 'initial' });
        },
        scheduleFit,
        observe() {
            var _a, _b;
            disconnectObserver();
            const currentRoot = root();
            const ResizeObserverCtor = ((_b = (_a = currentRoot === null || currentRoot === void 0 ? void 0 : currentRoot.ownerDocument) === null || _a === void 0 ? void 0 : _a.defaultView) === null || _b === void 0 ? void 0 : _b.ResizeObserver) ||
                (typeof ResizeObserver !== 'undefined' ? ResizeObserver : undefined);
            if (!currentRoot || !ResizeObserverCtor) {
                return;
            }
            resizeObserver = new ResizeObserverCtor(() => scheduleFit('resize'));
            resizeObserver.observe(currentRoot);
        },
        markUserInteraction() {
            userInteracted = true;
            cancelRetry();
        },
        resetAutoFit() {
            userInteracted = false;
            retryCount = 0;
        },
        destroy() {
            disconnectObserver();
            cancelRetry();
            retryCount = 0;
        },
    };
    return controller;
};
