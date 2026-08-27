import { findFileViewerViewStateProvider, findFileViewerZoomProvider, registerFileViewerViewStateProvider, resolveFileViewerScrollContainer, unregisterFileViewerViewStateProvider, } from './dom/index.js';
import { resolveFileViewerFitScale } from './fit.js';
export const cloneFileViewerViewState = (state) => {
    if (!state) {
        return null;
    }
    return {
        ...state,
        zoom: state.zoom ? { ...state.zoom } : undefined,
        scroll: state.scroll ? { ...state.scroll } : undefined,
        navigation: state.navigation ? { ...state.navigation } : undefined,
        extra: state.extra ? { ...state.extra } : undefined,
    };
};
export const createFileViewerViewStateChange = (state, patch = {}) => ({
    state: cloneFileViewerViewState(state) || {},
    action: patch.action || 'init',
    source: patch.source || 'viewer',
    timestamp: patch.timestamp || Date.now(),
});
export const createFileViewerViewStateChangeEmitter = () => {
    const listeners = new Set();
    return {
        emit(change) {
            listeners.forEach(listener => listener(change));
        },
        subscribe(listener) {
            listeners.add(listener);
            return () => {
                listeners.delete(listener);
            };
        },
    };
};
const getViewStateWindow = (host) => {
    var _a;
    return ((_a = host.ownerDocument) === null || _a === void 0 ? void 0 : _a.defaultView) || (typeof window !== 'undefined' ? window : undefined);
};
const waitForViewStateFrame = (host) => {
    const ownerWindow = getViewStateWindow(host);
    return new Promise(resolve => {
        if (ownerWindow === null || ownerWindow === void 0 ? void 0 : ownerWindow.requestAnimationFrame) {
            ownerWindow.requestAnimationFrame(() => resolve());
            return;
        }
        setTimeout(resolve, 16);
    });
};
const readFileViewerScrollState = (element) => {
    const maxTop = Math.max(0, element.scrollHeight - element.clientHeight);
    const maxLeft = Math.max(0, element.scrollWidth - element.clientWidth);
    return {
        top: element.scrollTop || 0,
        left: element.scrollLeft || 0,
        width: element.scrollWidth || 0,
        height: element.scrollHeight || 0,
        clientWidth: element.clientWidth || 0,
        clientHeight: element.clientHeight || 0,
        topRatio: maxTop > 0 ? (element.scrollTop || 0) / maxTop : 0,
        leftRatio: maxLeft > 0 ? (element.scrollLeft || 0) / maxLeft : 0,
    };
};
const resolveScrollValue = (value, ratio, maxValue) => {
    if (Number.isFinite(value)) {
        return Number(value);
    }
    if (Number.isFinite(ratio)) {
        return Number(ratio) * maxValue;
    }
    return undefined;
};
export const registerFileViewerGenericViewStateProvider = ({ host, renderer, scrollTarget, }) => {
    var _a, _b;
    const emitter = createFileViewerViewStateChangeEmitter();
    const ownerWindow = getViewStateWindow(host);
    let destroyed = false;
    let scrollFrame = 0;
    let suppressScrollEventUntil = 0;
    let unsubscribeZoom = null;
    const resolveScrollTarget = () => {
        if (typeof scrollTarget === 'function') {
            return scrollTarget() || host;
        }
        return scrollTarget || resolveFileViewerScrollContainer(host) || host;
    };
    const getZoomProvider = () => findFileViewerZoomProvider(host);
    const getState = () => {
        var _a, _b;
        const zoom = (_b = (_a = getZoomProvider()) === null || _a === void 0 ? void 0 : _a.getState) === null || _b === void 0 ? void 0 : _b.call(_a);
        const scrollElement = resolveScrollTarget();
        return {
            renderer,
            scale: zoom === null || zoom === void 0 ? void 0 : zoom.scale,
            zoom,
            scroll: scrollElement ? readFileViewerScrollState(scrollElement) : undefined,
        };
    };
    const emit = (action, source = 'viewer') => {
        if (destroyed) {
            return getState();
        }
        const state = getState();
        emitter.emit(createFileViewerViewStateChange(state, { action, source }));
        return state;
    };
    const suppressProgrammaticScrollEvents = () => {
        suppressScrollEventUntil = Math.max(suppressScrollEventUntil, Date.now() + 180);
    };
    const applyScrollState = (scroll) => {
        if (!scroll) {
            return;
        }
        const scrollElement = resolveScrollTarget();
        const maxTop = Math.max(0, scrollElement.scrollHeight - scrollElement.clientHeight);
        const maxLeft = Math.max(0, scrollElement.scrollWidth - scrollElement.clientWidth);
        const top = resolveScrollValue(scroll.top, scroll.topRatio, maxTop);
        const left = resolveScrollValue(scroll.left, scroll.leftRatio, maxLeft);
        suppressProgrammaticScrollEvents();
        if (top !== undefined) {
            scrollElement.scrollTop = Math.min(Math.max(0, top), maxTop);
        }
        if (left !== undefined) {
            scrollElement.scrollLeft = Math.min(Math.max(0, left), maxLeft);
        }
    };
    const fit = async (request) => {
        var _a, _b, _c, _d;
        const zoomProvider = getZoomProvider();
        if (zoomProvider === null || zoomProvider === void 0 ? void 0 : zoomProvider.fit) {
            return zoomProvider.fit(request);
        }
        if (!(zoomProvider === null || zoomProvider === void 0 ? void 0 : zoomProvider.setZoom)) {
            return {
                applied: false,
                mode: request.mode,
                resize: request.resize,
                source: request.source,
                reason: 'zoom-provider-unavailable',
                provider: 'view-state',
            };
        }
        const zoomState = zoomProvider.getState();
        const currentScale = Number(zoomState.scale) > 0 ? Number(zoomState.scale) : 1;
        const scrollElement = resolveScrollTarget();
        const viewportWidth = request.viewportWidth || scrollElement.clientWidth || host.clientWidth;
        const viewportHeight = request.viewportHeight || scrollElement.clientHeight || host.clientHeight;
        const contentWidth = Math.max(scrollElement.scrollWidth || 0, host.scrollWidth || 0, ((_a = scrollElement.getBoundingClientRect) === null || _a === void 0 ? void 0 : _a.call(scrollElement).width) || 0);
        const contentHeight = Math.max(scrollElement.scrollHeight || 0, host.scrollHeight || 0, ((_b = scrollElement.getBoundingClientRect) === null || _b === void 0 ? void 0 : _b.call(scrollElement).height) || 0);
        const scale = resolveFileViewerFitScale({
            mode: request.mode,
            viewportWidth,
            viewportHeight,
            contentWidth: contentWidth / currentScale,
            contentHeight: contentHeight / currentScale,
            currentScale,
            minScale: (_c = request.minScale) !== null && _c !== void 0 ? _c : zoomState.minScale,
            maxScale: (_d = request.maxScale) !== null && _d !== void 0 ? _d : zoomState.maxScale,
        });
        if (!scale) {
            return {
                applied: false,
                mode: request.mode,
                resize: request.resize,
                source: request.source,
                reason: 'unmeasurable',
                provider: 'view-state',
            };
        }
        suppressProgrammaticScrollEvents();
        await zoomProvider.setZoom(scale);
        await waitForViewStateFrame(host);
        const state = emit('fit', request.source);
        return {
            applied: true,
            mode: request.mode,
            resize: request.resize,
            scale,
            source: request.source,
            provider: 'view-state',
            state,
        };
    };
    const scheduleScrollChange = () => {
        if (destroyed || Date.now() < suppressScrollEventUntil || scrollFrame) {
            return;
        }
        const requestFrame = (ownerWindow === null || ownerWindow === void 0 ? void 0 : ownerWindow.requestAnimationFrame) || ((callback) => {
            return setTimeout(() => callback(Date.now()), 16);
        });
        scrollFrame = requestFrame(() => {
            scrollFrame = 0;
            emit('scroll', 'user');
        });
    };
    const provider = {
        getState,
        async applyState(state, options = {}) {
            var _a, _b;
            const source = options.source || 'api';
            const action = options.action || 'restore';
            const notify = options.notify !== false;
            const zoomProvider = getZoomProvider();
            const nextScale = Number((_a = state.scale) !== null && _a !== void 0 ? _a : (_b = state.zoom) === null || _b === void 0 ? void 0 : _b.scale);
            suppressProgrammaticScrollEvents();
            if (Number.isFinite(nextScale) && (zoomProvider === null || zoomProvider === void 0 ? void 0 : zoomProvider.setZoom)) {
                await zoomProvider.setZoom(nextScale);
            }
            await waitForViewStateFrame(host);
            applyScrollState(state.scroll);
            await waitForViewStateFrame(host);
            applyScrollState(state.scroll);
            if (notify) {
                return emit(action, source);
            }
            return getState();
        },
        fit,
        subscribe: emitter.subscribe,
    };
    host.dataset.viewerViewStateProvider = 'generic';
    registerFileViewerViewStateProvider(host, provider);
    (_a = host.addEventListener) === null || _a === void 0 ? void 0 : _a.call(host, 'scroll', scheduleScrollChange, {
        capture: true,
        passive: true,
    });
    const zoomProvider = getZoomProvider();
    unsubscribeZoom = ((_b = zoomProvider === null || zoomProvider === void 0 ? void 0 : zoomProvider.subscribe) === null || _b === void 0 ? void 0 : _b.call(zoomProvider, () => {
        emit('zoom-change', 'viewer');
    })) || null;
    return {
        provider,
        destroy() {
            var _a;
            destroyed = true;
            unsubscribeZoom === null || unsubscribeZoom === void 0 ? void 0 : unsubscribeZoom();
            unsubscribeZoom = null;
            if (scrollFrame && (ownerWindow === null || ownerWindow === void 0 ? void 0 : ownerWindow.cancelAnimationFrame)) {
                ownerWindow.cancelAnimationFrame(scrollFrame);
            }
            scrollFrame = 0;
            (_a = host.removeEventListener) === null || _a === void 0 ? void 0 : _a.call(host, 'scroll', scheduleScrollChange, true);
            unregisterFileViewerViewStateProvider(host);
        },
    };
};
const getMutationObserverConstructor = (root) => {
    var _a, _b;
    return ((_b = (_a = root === null || root === void 0 ? void 0 : root.ownerDocument) === null || _a === void 0 ? void 0 : _a.defaultView) === null || _b === void 0 ? void 0 : _b.MutationObserver) ||
        (typeof MutationObserver !== 'undefined' ? MutationObserver : undefined);
};
export const createFileViewerViewStateController = ({ root, enabled, onChange, }) => {
    let provider = null;
    let unsubscribe = null;
    let observer = null;
    let state = null;
    const readProviderState = () => { var _a; return cloneFileViewerViewState(((_a = provider === null || provider === void 0 ? void 0 : provider.getState) === null || _a === void 0 ? void 0 : _a.call(provider)) || null); };
    const commitState = (nextState, change) => {
        state = cloneFileViewerViewState(nextState);
        if (change && state) {
            onChange === null || onChange === void 0 ? void 0 : onChange({
                ...change,
                state: cloneFileViewerViewState(change.state) || state,
            });
        }
        return cloneFileViewerViewState(state);
    };
    const clearProvider = () => {
        unsubscribe === null || unsubscribe === void 0 ? void 0 : unsubscribe();
        unsubscribe = null;
        provider = null;
        return commitState(null);
    };
    const syncProvider = () => {
        var _a;
        if ((enabled === null || enabled === void 0 ? void 0 : enabled()) === false) {
            clearProvider();
            return null;
        }
        const nextProvider = findFileViewerViewStateProvider(root());
        if (nextProvider !== provider) {
            unsubscribe === null || unsubscribe === void 0 ? void 0 : unsubscribe();
            provider = nextProvider;
            unsubscribe = ((_a = nextProvider === null || nextProvider === void 0 ? void 0 : nextProvider.subscribe) === null || _a === void 0 ? void 0 : _a.call(nextProvider, change => {
                commitState(change.state, change);
            })) || null;
        }
        commitState(readProviderState());
        return nextProvider;
    };
    const disconnectObserver = () => {
        observer === null || observer === void 0 ? void 0 : observer.disconnect();
        observer = null;
    };
    return {
        get provider() {
            return provider;
        },
        get state() {
            return cloneFileViewerViewState(state);
        },
        hasProvider() {
            return !!syncProvider();
        },
        refreshProvider: syncProvider,
        observe() {
            disconnectObserver();
            const currentRoot = root();
            const MutationObserverCtor = getMutationObserverConstructor(currentRoot);
            if (!currentRoot || !MutationObserverCtor) {
                syncProvider();
                return;
            }
            observer = new MutationObserverCtor(() => {
                syncProvider();
            });
            observer.observe(currentRoot, {
                childList: true,
                subtree: true,
            });
            syncProvider();
        },
        clearProvider,
        getState() {
            syncProvider();
            return cloneFileViewerViewState(state);
        },
        async applyState(nextState, options) {
            const nextProvider = syncProvider();
            if (!(nextProvider === null || nextProvider === void 0 ? void 0 : nextProvider.applyState)) {
                return cloneFileViewerViewState(state);
            }
            const appliedState = await nextProvider.applyState(nextState, options);
            commitState(appliedState || nextProvider.getState());
            return cloneFileViewerViewState(state);
        },
        destroy() {
            disconnectObserver();
            clearProvider();
        },
    };
};
export const createFileViewerViewStateControllerActionHandlers = (controller) => {
    return {
        hasViewStateProvider() {
            return controller.hasProvider();
        },
        refreshViewStateProvider() {
            return controller.refreshProvider();
        },
        startViewStateObserver() {
            controller.observe();
            return controller.getState();
        },
        stopViewStateObserver() {
            controller.destroy();
            return null;
        },
        clearViewStateProvider() {
            return controller.clearProvider();
        },
        getViewState() {
            return controller.getState();
        },
        applyViewState(state, options) {
            return controller.applyState(state, options);
        },
    };
};
