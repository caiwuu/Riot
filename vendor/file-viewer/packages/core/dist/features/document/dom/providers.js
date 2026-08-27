const searchProviderRegistry = new WeakMap();
const zoomProviderRegistry = new WeakMap();
const viewStateProviderRegistry = new WeakMap();
const isProviderElement = (root) => {
    return !!root && root.nodeType === 1;
};
const queryProviderHosts = (root, selector) => {
    if (!(root === null || root === void 0 ? void 0 : root.querySelectorAll)) {
        return [];
    }
    const hosts = Array.from(root.querySelectorAll(selector));
    if (isProviderElement(root) && root.shadowRoot) {
        hosts.push(...queryProviderHosts(root.shadowRoot, selector));
    }
    const elements = Array.from(root.querySelectorAll('*'));
    for (const element of elements) {
        if (element.shadowRoot) {
            hosts.push(...queryProviderHosts(element.shadowRoot, selector));
        }
    }
    return hosts;
};
export const registerFileViewerSearchProvider = (host, provider) => {
    searchProviderRegistry.set(host, provider);
    host.__flyfishViewerSearchProvider = provider;
};
export const unregisterFileViewerSearchProvider = (host) => {
    if (!host) {
        return;
    }
    searchProviderRegistry.delete(host);
    delete host.__flyfishViewerSearchProvider;
};
export const findFileViewerSearchProvider = (root) => {
    if (!root) {
        return null;
    }
    const direct = isProviderElement(root)
        ? searchProviderRegistry.get(root) || root.__flyfishViewerSearchProvider
        : null;
    if (direct) {
        return direct;
    }
    const host = queryProviderHosts(root, '[data-viewer-search-provider]')[0];
    return host
        ? searchProviderRegistry.get(host) || host.__flyfishViewerSearchProvider || null
        : null;
};
export const registerFileViewerZoomProvider = (host, provider) => {
    zoomProviderRegistry.set(host, provider);
    host.dataset.viewerZoomProvider = host.dataset.viewerZoomProvider || 'custom';
    host.__flyfishViewerZoomProvider = provider;
};
export const unregisterFileViewerZoomProvider = (host) => {
    if (!host) {
        return;
    }
    zoomProviderRegistry.delete(host);
    delete host.dataset.viewerZoomProvider;
    delete host.__flyfishViewerZoomProvider;
};
export const findFileViewerZoomProvider = (root) => {
    if (!root) {
        return null;
    }
    const direct = isProviderElement(root)
        ? zoomProviderRegistry.get(root) || root.__flyfishViewerZoomProvider
        : null;
    if (direct) {
        return direct;
    }
    const host = queryProviderHosts(root, '[data-viewer-zoom-provider]')[0];
    return host
        ? zoomProviderRegistry.get(host) || host.__flyfishViewerZoomProvider || null
        : null;
};
export const registerFileViewerViewStateProvider = (host, provider) => {
    viewStateProviderRegistry.set(host, provider);
    host.dataset.viewerViewStateProvider = host.dataset.viewerViewStateProvider || 'custom';
    host.__flyfishViewerViewStateProvider = provider;
};
export const unregisterFileViewerViewStateProvider = (host) => {
    if (!host) {
        return;
    }
    viewStateProviderRegistry.delete(host);
    delete host.dataset.viewerViewStateProvider;
    delete host.__flyfishViewerViewStateProvider;
};
export const findFileViewerViewStateProvider = (root) => {
    if (!root) {
        return null;
    }
    const direct = isProviderElement(root)
        ? viewStateProviderRegistry.get(root) ||
            root.__flyfishViewerViewStateProvider
        : null;
    if (direct && (!isProviderElement(root) || root.dataset.viewerViewStateProvider !== 'generic')) {
        return direct;
    }
    const hosts = queryProviderHosts(root, '[data-viewer-view-state-provider]');
    const customHost = hosts.find(host => host.dataset.viewerViewStateProvider !== 'generic');
    const genericHost = hosts.find(host => host.dataset.viewerViewStateProvider === 'generic');
    const host = customHost || genericHost;
    if (host) {
        return viewStateProviderRegistry.get(host) || host.__flyfishViewerViewStateProvider || null;
    }
    return direct || null;
};
