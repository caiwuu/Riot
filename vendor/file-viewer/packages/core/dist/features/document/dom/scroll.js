export const DEFAULT_FILE_VIEWER_SCROLL_CONTAINER_SELECTOR = '[data-viewer-scroll-container], .pdf-wrapper';
export const DEFAULT_FILE_VIEWER_SCROLL_CONTAINER_CANDIDATE_SELECTOR = 'div, section, article, pre';
export const DEFAULT_FILE_VIEWER_SCROLLABLE_OVERFLOW_VALUES = ['auto', 'scroll', 'overlay'];
export const getFileViewerScrollableRange = (element) => {
    return Math.max(0, element.scrollHeight - element.clientHeight);
};
export const isFileViewerScrollableElement = (element, options = {}) => {
    var _a, _b;
    const minScrollRange = (_a = options.minScrollRange) !== null && _a !== void 0 ? _a : 2;
    if (getFileViewerScrollableRange(element) <= minScrollRange) {
        return false;
    }
    const ownerView = (_b = element.ownerDocument) === null || _b === void 0 ? void 0 : _b.defaultView;
    const view = (ownerView === null || ownerView === void 0 ? void 0 : ownerView.getComputedStyle)
        ? ownerView
        : (typeof window !== 'undefined' ? window : undefined);
    if (!(view === null || view === void 0 ? void 0 : view.getComputedStyle)) {
        return false;
    }
    const style = view.getComputedStyle(element);
    const overflowY = style.overflowY || style.overflow;
    const overflowValues = options.overflowValues || DEFAULT_FILE_VIEWER_SCROLLABLE_OVERFLOW_VALUES;
    return overflowValues.includes(overflowY);
};
export const resolveFileViewerScrollContainer = (root, options = {}) => {
    if (!root) {
        return null;
    }
    const preferredSelector = options.preferredSelector || DEFAULT_FILE_VIEWER_SCROLL_CONTAINER_SELECTOR;
    const candidateSelector = options.candidateSelector || DEFAULT_FILE_VIEWER_SCROLL_CONTAINER_CANDIDATE_SELECTOR;
    const preferred = root.querySelector(preferredSelector);
    if (preferred && isFileViewerScrollableElement(preferred, options)) {
        return preferred;
    }
    if (isFileViewerScrollableElement(root, options)) {
        return root;
    }
    const scrollableChildren = Array.from(root.querySelectorAll(candidateSelector))
        .filter(element => isFileViewerScrollableElement(element, options))
        .sort((a, b) => getFileViewerScrollableRange(b) - getFileViewerScrollableRange(a));
    return scrollableChildren[0] || preferred || root;
};
