import { buildFileViewerDocumentTextChunks } from './model.js';
import { getCurrentFileViewerDocumentAnchor, resolveFileViewerScrollContainer, scrollToFileViewerDocumentAnchor, } from './dom/index.js';
import { cloneFileViewerSearchState, createFileViewerDomSearchController, createFileViewerDomSearchControllerActionHandlers, } from './search.js';
export const createFileViewerSearchChangeState = (state) => {
    return cloneFileViewerSearchState(state);
};
export const resolveFileViewerLocationChangeAnchor = ({ root, anchors, }) => {
    return getCurrentFileViewerDocumentAnchor(root || null, anchors);
};
export const createFileViewerDocumentChangeSnapshot = ({ root, anchors, searchState, }) => {
    return {
        searchState: createFileViewerSearchChangeState(searchState),
        locationAnchor: resolveFileViewerLocationChangeAnchor({ root, anchors }),
    };
};
export const createFileViewerDocumentFeatureControllerActionHandlers = ({ root, searchTarget, searchOptions, waitForDomUpdate, preferredScrollContainer, getAiOptions, onSearchChange, onLocationChange, }) => {
    let documentActions = null;
    const searchController = createFileViewerDomSearchController({
        root,
        options: searchOptions,
        waitForDomUpdate,
        preferredScrollContainer: () => { var _a, _b; return (_b = (_a = preferredScrollContainer === null || preferredScrollContainer === void 0 ? void 0 : preferredScrollContainer()) !== null && _a !== void 0 ? _a : documentActions === null || documentActions === void 0 ? void 0 : documentActions.getScrollContainer()) !== null && _b !== void 0 ? _b : null; },
    });
    const searchActions = createFileViewerDomSearchControllerActionHandlers(searchTarget, searchController);
    documentActions = createFileViewerDocumentFeatureActions({
        root,
        searchController: {
            getAnchors: () => searchTarget.anchors.value,
            getSearchState: () => searchTarget.state,
            observe: searchActions.observe,
            refreshAnchors: searchActions.refreshAnchors,
            search: searchActions.search,
            clear: searchActions.clear,
            next: searchActions.next,
            previous: searchActions.previous,
        },
        getAiOptions,
        onSearchChange,
        onLocationChange,
    });
    return {
        ...documentActions,
        destroyDocumentFeatures: searchActions.destroy,
    };
};
export const dispatchFileViewerSearchChange = ({ state, onChange, }) => {
    const payload = createFileViewerSearchChangeState(state);
    onChange === null || onChange === void 0 ? void 0 : onChange(payload);
    return true;
};
export const dispatchFileViewerLocationChange = ({ anchor, onChange, }) => {
    onChange === null || onChange === void 0 ? void 0 : onChange(anchor);
    return true;
};
export const createFileViewerDocumentFeatureActions = ({ root, searchController, getAiOptions, onSearchChange, onLocationChange, }) => {
    const getRoot = () => root() || null;
    const getAnchors = () => searchController.getAnchors();
    const getSearchState = () => createFileViewerSearchChangeState(searchController.getSearchState());
    const notifySearchChange = () => {
        const state = getSearchState();
        dispatchFileViewerSearchChange({
            state,
            onChange: onSearchChange,
        });
        return state;
    };
    const getCurrentDocumentAnchor = () => {
        return resolveFileViewerLocationChangeAnchor({
            root: getRoot(),
            anchors: getAnchors(),
        });
    };
    const notifyLocationChange = () => {
        const anchor = getCurrentDocumentAnchor();
        dispatchFileViewerLocationChange({
            anchor,
            onChange: onLocationChange,
        });
        return anchor;
    };
    const maybeNotifyLocationChange = (actionOptions) => {
        if ((actionOptions === null || actionOptions === void 0 ? void 0 : actionOptions.notify) === false) {
            return getCurrentDocumentAnchor();
        }
        return notifyLocationChange();
    };
    const refreshDocumentIndex = async (actionOptions) => {
        searchController.observe();
        const anchors = await searchController.refreshAnchors();
        maybeNotifyLocationChange(actionOptions);
        return anchors;
    };
    const ensureAnchors = async (actionOptions) => {
        if (!getAnchors().length) {
            await refreshDocumentIndex(actionOptions);
        }
        return getAnchors();
    };
    const scrollToLoadedAnchor = (anchor, actionOptions) => {
        const result = scrollToFileViewerDocumentAnchor(getRoot(), anchor);
        maybeNotifyLocationChange(actionOptions);
        return result;
    };
    return {
        refreshDocumentIndex,
        async clearDocumentState() {
            await searchController.clear();
            return getSearchState();
        },
        getScrollContainer() {
            return resolveFileViewerScrollContainer(getRoot());
        },
        async searchDocument(query) {
            await searchController.search(query);
            return notifySearchChange();
        },
        async clearDocumentSearch() {
            await searchController.clear();
            return notifySearchChange();
        },
        async nextSearchResult() {
            await searchController.next();
            notifyLocationChange();
            return notifySearchChange();
        },
        async previousSearchResult() {
            await searchController.previous();
            notifyLocationChange();
            return notifySearchChange();
        },
        getSearchState,
        async collectDocumentAnchors(actionOptions) {
            await refreshDocumentIndex(actionOptions);
            return getAnchors();
        },
        getCurrentDocumentAnchor,
        scrollToLoadedAnchor,
        async scrollToAnchor(anchor, actionOptions) {
            await ensureAnchors(actionOptions);
            return scrollToLoadedAnchor(anchor, actionOptions);
        },
        async scrollToLine(line, actionOptions) {
            await ensureAnchors(actionOptions);
            return scrollToLoadedAnchor(line, actionOptions);
        },
        getDocumentTextChunks(textOptions) {
            return buildFileViewerDocumentTextChunks(getAnchors(), textOptions !== null && textOptions !== void 0 ? textOptions : getAiOptions === null || getAiOptions === void 0 ? void 0 : getAiOptions());
        },
    };
};
