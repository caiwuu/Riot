import { createFileViewerZoomState } from './model.js';
import { findFileViewerZoomProvider } from './dom/index.js';
export const cloneFileViewerZoomState = (state) => ({
    scale: state.scale,
    label: state.label,
    canZoomIn: state.canZoomIn,
    canZoomOut: state.canZoomOut,
    canReset: state.canReset,
    minScale: state.minScale,
    maxScale: state.maxScale,
});
export const applyFileViewerZoomState = (target, source) => {
    const normalized = createFileViewerZoomState(source || {});
    target.scale = normalized.scale;
    target.label = normalized.label;
    target.canZoomIn = normalized.canZoomIn;
    target.canZoomOut = normalized.canZoomOut;
    target.canReset = normalized.canReset;
    target.minScale = normalized.minScale;
    target.maxScale = normalized.maxScale;
    return target;
};
export const createFileViewerZoomChangeState = (state) => {
    return cloneFileViewerZoomState(state);
};
export const syncFileViewerZoomControllerState = (target, controller) => {
    return applyFileViewerZoomState(target, controller.state);
};
export const refreshFileViewerZoomControllerProvider = (target, controller) => {
    const provider = controller.refreshProvider();
    syncFileViewerZoomControllerState(target, controller);
    return provider;
};
export const observeFileViewerZoomController = (target, controller) => {
    controller.observe();
    return syncFileViewerZoomControllerState(target, controller);
};
export const clearFileViewerZoomControllerProvider = (target, controller) => {
    controller.clearProvider();
    return syncFileViewerZoomControllerState(target, controller);
};
export const destroyFileViewerZoomController = (target, controller) => {
    controller.destroy();
    return syncFileViewerZoomControllerState(target, controller);
};
export const runFileViewerZoomControllerAction = async (target, action) => {
    const nextState = await action();
    applyFileViewerZoomState(target, nextState);
    return createFileViewerZoomChangeState(target);
};
export const createFileViewerZoomControllerActionHandlers = (target, controller) => {
    return {
        hasZoomProvider() {
            const nextProvider = refreshFileViewerZoomControllerProvider(target, controller);
            return !!nextProvider;
        },
        refreshZoomProvider() {
            return refreshFileViewerZoomControllerProvider(target, controller);
        },
        startZoomObserver() {
            return observeFileViewerZoomController(target, controller);
        },
        stopZoomObserver() {
            return destroyFileViewerZoomController(target, controller);
        },
        clearZoomProvider() {
            return clearFileViewerZoomControllerProvider(target, controller);
        },
        getZoomState() {
            return createFileViewerZoomChangeState(target);
        },
        zoomIn() {
            return runFileViewerZoomControllerAction(target, () => controller.zoomIn());
        },
        zoomOut() {
            return runFileViewerZoomControllerAction(target, () => controller.zoomOut());
        },
        resetZoom() {
            return runFileViewerZoomControllerAction(target, () => controller.resetZoom());
        },
    };
};
export const createFileViewerZoomChangeEmitter = () => {
    const listeners = new Set();
    return {
        emit() {
            listeners.forEach(listener => listener());
        },
        subscribe(listener) {
            listeners.add(listener);
            return () => {
                listeners.delete(listener);
            };
        },
    };
};
const getMutationObserverConstructor = (root) => {
    var _a, _b;
    return ((_b = (_a = root === null || root === void 0 ? void 0 : root.ownerDocument) === null || _a === void 0 ? void 0 : _a.defaultView) === null || _b === void 0 ? void 0 : _b.MutationObserver) ||
        (typeof MutationObserver !== 'undefined' ? MutationObserver : undefined);
};
export const createFileViewerZoomController = ({ root, enabled, beforeZoom, onChange, }) => {
    let provider = null;
    let unsubscribe = null;
    let observer = null;
    let runningAction = false;
    const state = createFileViewerZoomState();
    const notifyChange = () => {
        onChange === null || onChange === void 0 ? void 0 : onChange(cloneFileViewerZoomState(state));
    };
    const clearProvider = () => {
        unsubscribe === null || unsubscribe === void 0 ? void 0 : unsubscribe();
        unsubscribe = null;
        provider = null;
        applyFileViewerZoomState(state, null);
    };
    const syncProvider = () => {
        var _a, _b;
        if ((enabled === null || enabled === void 0 ? void 0 : enabled()) === false) {
            clearProvider();
            return null;
        }
        const nextProvider = findFileViewerZoomProvider(root());
        if (nextProvider !== provider) {
            unsubscribe === null || unsubscribe === void 0 ? void 0 : unsubscribe();
            provider = nextProvider;
            unsubscribe = ((_a = nextProvider === null || nextProvider === void 0 ? void 0 : nextProvider.subscribe) === null || _a === void 0 ? void 0 : _a.call(nextProvider, () => {
                applyFileViewerZoomState(state, nextProvider.getState());
                if (!runningAction) {
                    notifyChange();
                }
            })) || null;
        }
        applyFileViewerZoomState(state, ((_b = nextProvider === null || nextProvider === void 0 ? void 0 : nextProvider.getState) === null || _b === void 0 ? void 0 : _b.call(nextProvider)) || null);
        return nextProvider;
    };
    const disconnectObserver = () => {
        observer === null || observer === void 0 ? void 0 : observer.disconnect();
        observer = null;
    };
    const runZoomAction = async (operation, action) => {
        const nextProvider = syncProvider();
        if (!nextProvider) {
            return cloneFileViewerZoomState(state);
        }
        if (beforeZoom && await beforeZoom(operation) === false) {
            return cloneFileViewerZoomState(state);
        }
        runningAction = true;
        try {
            const nextState = await action(nextProvider);
            applyFileViewerZoomState(state, nextState || nextProvider.getState());
            return cloneFileViewerZoomState(state);
        }
        finally {
            runningAction = false;
        }
    };
    return {
        get provider() {
            return provider;
        },
        state,
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
            return cloneFileViewerZoomState(state);
        },
        zoomIn: () => runZoomAction('zoom-in', nextProvider => nextProvider.zoomIn()),
        zoomOut: () => runZoomAction('zoom-out', nextProvider => nextProvider.zoomOut()),
        resetZoom: () => runZoomAction('zoom-reset', nextProvider => nextProvider.resetZoom()),
        destroy() {
            disconnectObserver();
            clearProvider();
        },
    };
};
