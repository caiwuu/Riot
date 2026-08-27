import { buildFileViewerLifecycleContext, } from '../lifecycle/operations.js';
import { resolveFileViewerPreviewMessages, } from '../viewer/state.js';
import { translateFileViewerMessage } from '../i18n/messages.js';
import { DEFAULT_FILE_VIEWER_SOURCE_FILENAME, getExtension, normalizeFilename, readFileViewerBuffer, resolveFileViewerSourceFilename, wrapFileViewerFileRef, } from './index.js';
export const DEFAULT_PDF_RANGE_CHUNK_SIZE = 64 * 1024;
export const DEFAULT_FILE_VIEWER_STREAMING_PDF_FILENAME = 'preview.pdf';
export const FILE_VIEWER_REMOTE_MISSING_DATA_ERROR_MESSAGE = '文件下载失败';
export const FILE_VIEWER_PREVIEW_LOAD_ERROR_PREFIXES = {
    local: '读取文件异常',
    load: '加载文件异常',
    stream: '加载 PDF 流式预览异常',
};
export const resolveFileViewerPreviewLoadErrorMessage = ({ kind, error, formatErrorMessage, prefixes, i18n, }) => {
    var _a;
    const fallbackPrefix = kind === 'local'
        ? translateFileViewerMessage(i18n, 'error.localRead')
        : kind === 'stream'
            ? translateFileViewerMessage(i18n, 'error.stream')
            : translateFileViewerMessage(i18n, 'error.load');
    return formatErrorMessage((_a = prefixes === null || prefixes === void 0 ? void 0 : prefixes[kind]) !== null && _a !== void 0 ? _a : fallbackPrefix, error, i18n);
};
export const resolveFileViewerMissingRemoteDataErrorMessage = ({ message, i18n, } = {}) => message || translateFileViewerMessage(i18n, 'error.remoteDownload');
export const DEFAULT_FILE_VIEWER_PREVIEW_LOAD_ERROR_LOGGER = error => {
    if (typeof console !== 'undefined' && typeof console.error === 'function') {
        console.error(error);
    }
};
export const reportFileViewerPreviewLoadError = ({ onLogError = DEFAULT_FILE_VIEWER_PREVIEW_LOAD_ERROR_LOGGER, onErrorMessage, ...messageInput }) => {
    onLogError === null || onLogError === void 0 ? void 0 : onLogError(messageInput.error);
    const message = resolveFileViewerPreviewLoadErrorMessage(messageInput);
    onErrorMessage === null || onErrorMessage === void 0 ? void 0 : onErrorMessage(message);
    return message;
};
export const reportFileViewerMissingRemoteData = ({ onErrorMessage, ...messageInput } = {}) => {
    const message = resolveFileViewerMissingRemoteDataErrorMessage(messageInput);
    onErrorMessage === null || onErrorMessage === void 0 ? void 0 : onErrorMessage(message);
    return message;
};
export const createFileViewerRequestController = () => {
    let version = 0;
    let activeAbortController = null;
    return {
        get version() {
            return version;
        },
        createVersion() {
            version += 1;
            activeAbortController === null || activeAbortController === void 0 ? void 0 : activeAbortController.abort();
            activeAbortController = null;
            return version;
        },
        isCurrent(nextVersion) {
            return nextVersion === version;
        },
        createAbortController() {
            activeAbortController = typeof AbortController === 'function'
                ? new AbortController()
                : null;
            return activeAbortController;
        },
        clearAbortController(controller) {
            if (activeAbortController === controller) {
                activeAbortController = null;
            }
        },
        abort() {
            activeAbortController === null || activeAbortController === void 0 ? void 0 : activeAbortController.abort();
            activeAbortController = null;
        },
    };
};
export const createFileViewerRequestScope = (requestController = createFileViewerRequestController()) => {
    return {
        requestController,
        getCurrentVersion: () => requestController.version,
        isCurrentRequest: version => requestController.isCurrent(version),
    };
};
export const isFileViewerAbortError = (error) => {
    if (typeof DOMException !== 'undefined' && error instanceof DOMException && error.name === 'AbortError') {
        return true;
    }
    if (!error || typeof error !== 'object') {
        return false;
    }
    const candidate = error;
    return candidate.__CANCEL__ === true ||
        candidate.code === 'ERR_CANCELED' ||
        candidate.name === 'AbortError' ||
        candidate.name === 'CanceledError';
};
export const hasFileViewerPreviewSource = ({ file, url, } = {}) => {
    return !!file || !!url;
};
export const resolveFileViewerPreviewRequestReason = (input = {}) => {
    return hasFileViewerPreviewSource(input) ? 'replace' : 'reset';
};
const FILE_VIEWER_HIERARCHICAL_URL_PATTERN = /^[a-z][a-z0-9+.-]*:\/\//i;
const FILE_VIEWER_SCHEME_URL_PATTERN = /^[a-z][a-z0-9+.-]*:/i;
const findFileViewerUrlSuffixIndex = (value) => {
    const queryIndex = value.indexOf('?');
    const hashIndex = value.indexOf('#');
    if (queryIndex === -1) {
        return hashIndex;
    }
    if (hashIndex === -1) {
        return queryIndex;
    }
    return Math.min(queryIndex, hashIndex);
};
const encodeFileViewerUrlPathSegment = (segment) => {
    if (!segment) {
        return segment;
    }
    let decoded = segment;
    try {
        decoded = decodeURIComponent(segment);
    }
    catch {
        // Invalid percent escapes are treated as literal filename characters.
    }
    return encodeURIComponent(decoded)
        .replace(/%3A/gi, ':')
        .replace(/%40/gi, '@')
        .replace(/%24/g, '$')
        .replace(/%26/g, '&')
        .replace(/%2B/gi, '+')
        .replace(/%2C/gi, ',')
        .replace(/%3B/gi, ';')
        .replace(/%3D/gi, '=');
};
const normalizeFileViewerUrlPathname = (pathname) => {
    return pathname
        .split('/')
        .map(encodeFileViewerUrlPathSegment)
        .join('/');
};
const splitFileViewerUrlPathPrefix = (pathPart) => {
    if (pathPart.startsWith('//')) {
        const pathStart = pathPart.indexOf('/', 2);
        return pathStart === -1
            ? { prefix: pathPart, pathname: '' }
            : { prefix: pathPart.slice(0, pathStart), pathname: pathPart.slice(pathStart) };
    }
    const hierarchical = pathPart.match(FILE_VIEWER_HIERARCHICAL_URL_PATTERN);
    if (hierarchical) {
        const pathStart = pathPart.indexOf('/', hierarchical[0].length);
        return pathStart === -1
            ? { prefix: pathPart, pathname: '' }
            : { prefix: pathPart.slice(0, pathStart), pathname: pathPart.slice(pathStart) };
    }
    if (FILE_VIEWER_SCHEME_URL_PATTERN.test(pathPart)) {
        return { prefix: pathPart, pathname: '' };
    }
    return { prefix: '', pathname: pathPart };
};
export const normalizeFileViewerSourceUrl = (sourceUrl) => {
    const trimmed = sourceUrl === null || sourceUrl === void 0 ? void 0 : sourceUrl.trim();
    if (!trimmed) {
        return null;
    }
    const suffixIndex = findFileViewerUrlSuffixIndex(trimmed);
    const pathPart = suffixIndex === -1 ? trimmed : trimmed.slice(0, suffixIndex);
    const suffix = suffixIndex === -1 ? '' : trimmed.slice(suffixIndex);
    const { prefix, pathname } = splitFileViewerUrlPathPrefix(pathPart);
    if (!pathname) {
        return `${prefix}${suffix}`;
    }
    return `${prefix}${normalizeFileViewerUrlPathname(pathname)}${suffix}`;
};
export const createFileViewerEmptyPreviewState = () => {
    return {
        filename: '',
        file: null,
        buffer: null,
        sourceUrl: null,
        renderedReady: false,
        progressiveReady: false,
    };
};
export const createFileViewerPreviewRequestResetState = () => {
    return {
        file: null,
        buffer: null,
        sourceUrl: null,
        progressiveReady: false,
    };
};
export const createFileViewerPreviewStateTarget = ({ filename, file, buffer, sourceUrl, renderedReady, progressiveReady, }) => {
    return {
        get filename() {
            return filename.get();
        },
        set filename(value) {
            filename.set(value);
        },
        get file() {
            return file.get();
        },
        set file(value) {
            file.set(value);
        },
        get buffer() {
            return buffer.get();
        },
        set buffer(value) {
            buffer.set(value);
        },
        get sourceUrl() {
            return sourceUrl.get();
        },
        set sourceUrl(value) {
            sourceUrl.set(value);
        },
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
export const applyFileViewerPreviewRequestResetState = (target, state = createFileViewerPreviewRequestResetState()) => {
    target.file = state.file;
    target.buffer = state.buffer;
    target.sourceUrl = state.sourceUrl;
    target.progressiveReady = state.progressiveReady;
    return target;
};
export const commitFileViewerPreviewRequestStartState = ({ reason = 'replace', requestController, previewTarget, onClearRenderedContent, onClearError, }) => {
    const version = requestController.createVersion();
    onClearRenderedContent === null || onClearRenderedContent === void 0 ? void 0 : onClearRenderedContent(reason);
    applyFileViewerPreviewRequestResetState(previewTarget);
    onClearError === null || onClearError === void 0 ? void 0 : onClearError();
    return version;
};
export const cancelFileViewerPreviewRequest = ({ reason = 'component-unmount', requestController, previewTarget, onClearRenderedContent, onClearError, }) => {
    return commitFileViewerPreviewRequestStartState({
        reason,
        requestController,
        previewTarget,
        onClearRenderedContent,
        onClearError,
    });
};
export const runFileViewerPreviewSourceChange = ({ onRefreshPreview, } = {}) => {
    return onRefreshPreview === null || onRefreshPreview === void 0 ? void 0 : onRefreshPreview();
};
export const runFileViewerPreviewComponentUnmount = ({ reason = 'component-unmount', onCancelPreview, onClearRenderedContent, onResetLoading, onStopZoomObserver, onStopFitObserver, onStopViewStateObserver, } = {}) => {
    onCancelPreview === null || onCancelPreview === void 0 ? void 0 : onCancelPreview(reason);
    if (!onCancelPreview) {
        onClearRenderedContent === null || onClearRenderedContent === void 0 ? void 0 : onClearRenderedContent(reason);
    }
    onResetLoading === null || onResetLoading === void 0 ? void 0 : onResetLoading();
    onStopZoomObserver === null || onStopZoomObserver === void 0 ? void 0 : onStopZoomObserver();
    onStopFitObserver === null || onStopFitObserver === void 0 ? void 0 : onStopFitObserver();
    onStopViewStateObserver === null || onStopViewStateObserver === void 0 ? void 0 : onStopViewStateObserver();
    return {
        reason,
    };
};
export const applyFileViewerEmptyPreviewState = (target, state = createFileViewerEmptyPreviewState()) => {
    target.filename = state.filename;
    target.renderedReady = state.renderedReady;
    applyFileViewerPreviewRequestResetState(target, state);
    return target;
};
export const commitFileViewerEmptyPreviewResetState = ({ previewTarget, state, reason, onClearRenderedContent, onResetLoading, }) => {
    applyFileViewerEmptyPreviewState(previewTarget, state);
    onClearRenderedContent === null || onClearRenderedContent === void 0 ? void 0 : onClearRenderedContent(reason);
    onResetLoading === null || onResetLoading === void 0 ? void 0 : onResetLoading();
    return previewTarget;
};
export const runFileViewerPreviewRequest = async ({ file, url, reason = resolveFileViewerPreviewRequestReason({ file, url }), requestController, previewTarget, onPreviewLocalFile, onPreviewRemoteFile, onClearRenderedContent, onClearError, onResetLoading, }) => {
    const version = commitFileViewerPreviewRequestStartState({
        reason,
        requestController,
        previewTarget,
        onClearRenderedContent,
        onClearError,
    });
    if (file) {
        const result = await onPreviewLocalFile(file, version);
        return {
            status: 'file',
            version,
            reason,
            file,
            url: null,
            result,
        };
    }
    if (url) {
        const result = await onPreviewRemoteFile(url, version);
        return {
            status: 'url',
            version,
            reason,
            file: null,
            url,
            result,
        };
    }
    const result = commitFileViewerEmptyPreviewResetState({
        previewTarget,
        onClearRenderedContent,
        onResetLoading,
    });
    return {
        status: 'reset',
        version,
        reason,
        file: null,
        url: null,
        result,
    };
};
export const createFileViewerReadPreviewState = ({ file, buffer, sourceUrl, fallbackFilename = '', }) => ({
    filename: resolveFileViewerSourceFilename({ file, fallback: fallbackFilename }),
    file,
    buffer,
    sourceUrl: normalizeFileViewerSourceUrl(sourceUrl),
});
export const applyFileViewerReadPreviewState = (target, state) => {
    target.filename = state.filename;
    target.file = state.file;
    target.buffer = state.buffer;
    target.sourceUrl = state.sourceUrl;
    return target;
};
export const applyFileViewerPreviewSourceUrlState = (target, sourceUrl) => {
    target.sourceUrl = normalizeFileViewerSourceUrl(sourceUrl);
    return target;
};
export const applyFileViewerPreviewFilenameState = (target, filename, fallbackFilename = '') => {
    target.filename = resolveFileViewerSourceFilename({ filename, fallback: fallbackFilename });
    return target;
};
export const applyFileViewerRenderReadinessState = (target, state) => {
    if (typeof state.renderedReady === 'boolean') {
        target.renderedReady = state.renderedReady;
    }
    if (typeof state.progressiveReady === 'boolean') {
        target.progressiveReady = state.progressiveReady;
    }
    return target;
};
export const commitFileViewerRenderCompleteState = ({ version, session, buildState, readinessTarget, onSession, onActiveDocumentContext, onLifecycle, onClearLoadStarted, }) => {
    onSession === null || onSession === void 0 ? void 0 : onSession(session !== null && session !== void 0 ? session : null);
    const completeState = buildState();
    applyFileViewerRenderReadinessState(readinessTarget, completeState.readiness);
    onActiveDocumentContext === null || onActiveDocumentContext === void 0 ? void 0 : onActiveDocumentContext(completeState.lifecycleContext);
    onLifecycle === null || onLifecycle === void 0 ? void 0 : onLifecycle(completeState.lifecycleContext);
    onClearLoadStarted === null || onClearLoadStarted === void 0 ? void 0 : onClearLoadStarted(version);
    return completeState;
};
export const runFileViewerReadAndRenderFile = async ({ file, version, sourceUrl, source = sourceUrl ? 'url' : 'file', fallbackFilename = '', previewTarget, isCurrent, mountRenderedContent, destroyRenderSession, buildRenderCompleteState, onSession, onActiveDocumentContext, onLifecycle, onClearLoadStarted, }) => {
    const buffer = await readFileViewerBuffer(file);
    if (!isCurrent(version)) {
        return {
            stale: true,
            buffer,
            session: null,
            complete: null,
        };
    }
    applyFileViewerReadPreviewState(previewTarget, createFileViewerReadPreviewState({
        file,
        buffer,
        sourceUrl,
        fallbackFilename,
    }));
    const session = await mountRenderedContent(buffer, file, version, sourceUrl);
    if (!isCurrent(version)) {
        destroyRenderSession === null || destroyRenderSession === void 0 ? void 0 : destroyRenderSession(session);
        return {
            stale: true,
            buffer,
            session,
            complete: null,
        };
    }
    const complete = commitFileViewerRenderCompleteState({
        version,
        session,
        readinessTarget: previewTarget,
        buildState: () => buildRenderCompleteState({
            version,
            source,
            file,
            sourceUrl,
        }),
        onSession,
        onActiveDocumentContext,
        onLifecycle,
        onClearLoadStarted,
    });
    return {
        stale: false,
        buffer,
        session,
        complete,
    };
};
export const runFileViewerStreamingPdfPreview = async ({ url, version, filename, previewTarget, isCurrent, mountRenderedContent, destroyRenderSession, buildRenderCompleteState, loadingMessage, i18n, onStartLoading, onSession, onActiveDocumentContext, onLifecycle, onClearLoadStarted, onStopLoading, onError, }) => {
    let placeholderFile = null;
    onStartLoading === null || onStartLoading === void 0 ? void 0 : onStartLoading(loadingMessage || resolveFileViewerPreviewMessages(i18n).streamingPdf);
    try {
        placeholderFile = createFileViewerStreamingPdfPlaceholderFile(filename);
        applyFileViewerPreviewSourceUrlState(previewTarget, url);
        const session = await mountRenderedContent(new ArrayBuffer(0), placeholderFile, version, url, url);
        if (!isCurrent(version)) {
            destroyRenderSession === null || destroyRenderSession === void 0 ? void 0 : destroyRenderSession(session);
            return {
                status: 'stale',
                placeholderFile,
                session,
                complete: null,
                error: null,
            };
        }
        const complete = commitFileViewerRenderCompleteState({
            version,
            session,
            readinessTarget: previewTarget,
            buildState: () => buildRenderCompleteState({
                version,
                source: 'url',
                sourceUrl: url,
            }),
            onSession,
            onActiveDocumentContext,
            onLifecycle,
            onClearLoadStarted,
        });
        return {
            status: 'ready',
            placeholderFile,
            session,
            complete,
            error: null,
        };
    }
    catch (error) {
        if (!isCurrent(version)) {
            return {
                status: 'stale',
                placeholderFile,
                session: null,
                complete: null,
                error: null,
            };
        }
        onError === null || onError === void 0 ? void 0 : onError(error);
        return {
            status: 'error',
            placeholderFile,
            session: null,
            complete: null,
            error,
        };
    }
    finally {
        finalizeFileViewerPreviewLoadState({
            version,
            isCurrent,
            onClearLoadStarted,
            onStopLoading,
        });
    }
};
export const runFileViewerLocalFilePreview = async ({ source, version, currentFilename, fallbackFilename, previewTarget, isCurrent, mountRenderedContent, destroyRenderSession, buildLoadStartState, buildRenderCompleteState, onMarkLoadStarted, onStartLoading, onSession, onActiveDocumentContext, onLifecycle, onClearLoadStarted, onStopLoading, onError, }) => {
    const localSource = resolveFileViewerFileRefSourcePlan({
        source,
        currentFilename,
        fallbackFilename,
    });
    const { file } = localSource;
    commitFileViewerLoadStartState({
        version,
        filename: localSource.filename,
        filenameTarget: previewTarget,
        buildState: () => buildLoadStartState({
            version,
            source: 'file',
            file,
        }),
        onMarkLoadStarted,
        onLifecycle,
        onStartLoading,
    });
    try {
        const read = await runFileViewerReadAndRenderFile({
            file,
            version,
            source: 'file',
            previewTarget,
            isCurrent,
            mountRenderedContent,
            destroyRenderSession,
            buildRenderCompleteState: input => buildRenderCompleteState({
                version: input.version,
                source: 'file',
                file,
            }),
            onSession,
            onActiveDocumentContext,
            onLifecycle,
            onClearLoadStarted,
        });
        if (read.stale) {
            return {
                status: 'stale',
                source: localSource,
                read,
                error: null,
            };
        }
        return {
            status: 'ready',
            source: localSource,
            read,
            error: null,
        };
    }
    catch (error) {
        if (!isCurrent(version)) {
            return {
                status: 'stale',
                source: localSource,
                read: null,
                error: null,
            };
        }
        onError === null || onError === void 0 ? void 0 : onError(error);
        return {
            status: 'error',
            source: localSource,
            read: null,
            error,
        };
    }
    finally {
        finalizeFileViewerPreviewLoadState({
            version,
            isCurrent,
            onClearLoadStarted,
            onStopLoading,
        });
    }
};
export const runFileViewerRemoteFilePreview = async ({ url, version, pageHref, streaming, previewTarget, requestController, isCurrent, downloadFile, mountRenderedContent, destroyRenderSession, buildLoadStartState, buildRenderCompleteState, i18n, onMarkLoadStarted, onStartLoading, onSetLoadingMessage, onSession, onActiveDocumentContext, onLifecycle, onClearLoadStarted, onStopLoading, onMissingData, onError, }) => {
    const remoteSource = resolveFileViewerRemoteSourcePlan({
        pageHref,
        streaming,
        url,
    });
    const sourceUrl = remoteSource.url;
    commitFileViewerLoadStartState({
        version,
        filename: remoteSource.filename,
        filenameTarget: previewTarget,
        buildState: () => buildLoadStartState({
            version,
            source: 'url',
            sourceUrl,
        }),
        onMarkLoadStarted,
        onLifecycle,
        onStartLoading,
    });
    if (remoteSource.streamPdf) {
        const stream = await runFileViewerStreamingPdfPreview({
            url: sourceUrl,
            version,
            filename: remoteSource.filename,
            previewTarget,
            isCurrent,
            mountRenderedContent,
            destroyRenderSession,
            buildRenderCompleteState: input => buildRenderCompleteState({
                version: input.version,
                source: 'url',
                sourceUrl,
            }),
            i18n,
            onStartLoading,
            onSession,
            onActiveDocumentContext,
            onLifecycle,
            onClearLoadStarted,
            onStopLoading,
            onError: error => onError === null || onError === void 0 ? void 0 : onError(error, 'stream'),
        });
        if (stream.status === 'ready') {
            return {
                status: 'stream',
                remoteSource,
                download: null,
                read: null,
                stream,
                error: null,
            };
        }
        if (stream.status === 'error') {
            return {
                status: 'error',
                remoteSource,
                download: null,
                read: null,
                stream,
                error: stream.error,
            };
        }
        return {
            status: 'stale',
            remoteSource,
            download: null,
            read: null,
            stream,
            error: null,
        };
    }
    const controller = requestController.createAbortController();
    try {
        const data = await downloadFile({
            url: sourceUrl,
            signal: controller === null || controller === void 0 ? void 0 : controller.signal,
        });
        const download = commitFileViewerRemoteDownloadState({
            version,
            data,
            currentFilename: remoteSource.filename,
            isCurrent,
            i18n,
            onMissingData,
            onSetLoadingMessage,
        });
        if (download.stale) {
            return {
                status: 'stale',
                remoteSource,
                download,
                read: null,
                stream: null,
                error: null,
            };
        }
        if (download.missing) {
            return {
                status: 'missing',
                remoteSource,
                download,
                read: null,
                stream: null,
                error: null,
            };
        }
        const read = await runFileViewerReadAndRenderFile({
            file: download.source.file,
            version,
            source: 'url',
            sourceUrl,
            previewTarget,
            isCurrent,
            mountRenderedContent,
            destroyRenderSession,
            buildRenderCompleteState: input => buildRenderCompleteState({
                version: input.version,
                source: 'url',
                file: input.file,
                sourceUrl,
            }),
            onSession,
            onActiveDocumentContext,
            onLifecycle,
            onClearLoadStarted,
        });
        if (read.stale) {
            return {
                status: 'stale',
                remoteSource,
                download,
                read,
                stream: null,
                error: null,
            };
        }
        return {
            status: 'ready',
            remoteSource,
            download,
            read,
            stream: null,
            error: null,
        };
    }
    catch (error) {
        if (!isCurrent(version) || isFileViewerAbortError(error)) {
            return {
                status: 'stale',
                remoteSource,
                download: null,
                read: null,
                stream: null,
                error: null,
            };
        }
        onError === null || onError === void 0 ? void 0 : onError(error, 'load');
        return {
            status: 'error',
            remoteSource,
            download: null,
            read: null,
            stream: null,
            error,
        };
    }
    finally {
        requestController.clearAbortController(controller);
        finalizeFileViewerPreviewLoadState({
            version,
            isCurrent,
            onClearLoadStarted,
            onStopLoading,
        });
    }
};
export const createFileViewerSourceLoadingActionHandlers = ({ getFile, getUrl, getCurrentFilename, getPdfStreaming, getI18n, getPageHref, previewTarget, requestController, downloadFile, mountRenderedContent, destroyRenderSession, buildLoadStartState, buildRenderCompleteState, formatErrorMessage, onMarkLoadStarted, onClearLoadStarted, onStartLoading, onSetLoadingMessage, onStopLoading, onShowError, onClearError, onResetLoading, onClearRenderedContent, onSession, onActiveDocumentContext, onLifecycle, }) => {
    const isCurrentRequest = (version) => requestController.isCurrent(version);
    const previewLocalFile = async (source, version) => {
        var _a;
        return await runFileViewerLocalFilePreview({
            source,
            version,
            currentFilename: (_a = getCurrentFilename === null || getCurrentFilename === void 0 ? void 0 : getCurrentFilename()) !== null && _a !== void 0 ? _a : previewTarget.filename,
            previewTarget,
            isCurrent: isCurrentRequest,
            mountRenderedContent,
            destroyRenderSession,
            buildLoadStartState: input => buildLoadStartState({
                version: input.version,
                source: 'file',
                file: input.file,
            }),
            buildRenderCompleteState: input => buildRenderCompleteState({
                version: input.version,
                source: 'file',
                file: input.file,
            }),
            onMarkLoadStarted,
            onStartLoading,
            onSession,
            onActiveDocumentContext,
            onLifecycle,
            onClearLoadStarted,
            onStopLoading,
            onError: error => {
                reportFileViewerPreviewLoadError({
                    kind: 'local',
                    error,
                    formatErrorMessage,
                    i18n: getI18n === null || getI18n === void 0 ? void 0 : getI18n(),
                    onErrorMessage: onShowError,
                });
            },
        });
    };
    const previewRemoteFile = async (url, version) => {
        return await runFileViewerRemoteFilePreview({
            url,
            version,
            pageHref: getPageHref === null || getPageHref === void 0 ? void 0 : getPageHref(),
            streaming: getPdfStreaming === null || getPdfStreaming === void 0 ? void 0 : getPdfStreaming(),
            i18n: getI18n === null || getI18n === void 0 ? void 0 : getI18n(),
            previewTarget,
            requestController,
            isCurrent: isCurrentRequest,
            downloadFile,
            mountRenderedContent,
            destroyRenderSession,
            buildLoadStartState: input => buildLoadStartState({
                version: input.version,
                source: 'url',
                sourceUrl: input.sourceUrl,
            }),
            buildRenderCompleteState: input => buildRenderCompleteState({
                version: input.version,
                source: 'url',
                file: input.file,
                sourceUrl: input.sourceUrl,
            }),
            onMarkLoadStarted,
            onStartLoading,
            onSetLoadingMessage,
            onSession,
            onActiveDocumentContext,
            onLifecycle,
            onClearLoadStarted,
            onStopLoading,
            onMissingData: () => {
                reportFileViewerMissingRemoteData({
                    i18n: getI18n === null || getI18n === void 0 ? void 0 : getI18n(),
                    onErrorMessage: onShowError,
                });
            },
            onError: (error, kind) => {
                reportFileViewerPreviewLoadError({
                    kind,
                    error,
                    formatErrorMessage,
                    i18n: getI18n === null || getI18n === void 0 ? void 0 : getI18n(),
                    onErrorMessage: onShowError,
                });
            },
        });
    };
    const resetViewer = (reason) => {
        return commitFileViewerEmptyPreviewResetState({
            previewTarget,
            reason,
            onClearRenderedContent,
            onResetLoading,
        });
    };
    const refreshPreview = async () => {
        return await runFileViewerPreviewRequest({
            file: getFile(),
            url: getUrl(),
            requestController,
            previewTarget,
            onPreviewLocalFile: previewLocalFile,
            onPreviewRemoteFile: previewRemoteFile,
            onClearRenderedContent,
            onClearError,
            onResetLoading,
        });
    };
    const cancelPreview = (reason = 'component-unmount') => {
        return cancelFileViewerPreviewRequest({
            reason,
            requestController,
            previewTarget,
            onClearRenderedContent,
            onClearError,
        });
    };
    return {
        isCurrentRequest,
        previewLocalFile,
        previewRemoteFile,
        resetViewer,
        refreshPreview,
        cancelPreview,
    };
};
export const finalizeFileViewerPreviewLoadState = ({ version, isCurrent, onClearLoadStarted, onStopLoading, }) => {
    onClearLoadStarted === null || onClearLoadStarted === void 0 ? void 0 : onClearLoadStarted(version);
    if (isCurrent(version)) {
        onStopLoading === null || onStopLoading === void 0 ? void 0 : onStopLoading();
    }
};
export const resolveFileViewerLoadStartMessage = (source, i18n) => {
    const messages = resolveFileViewerPreviewMessages(i18n);
    return source === 'url'
        ? messages.downloading
        : messages.reading;
};
export const commitFileViewerLoadStartState = ({ version, filename, fallbackFilename, filenameTarget, buildState, onMarkLoadStarted, onLifecycle, onStartLoading, }) => {
    if (filenameTarget) {
        applyFileViewerPreviewFilenameState(filenameTarget, filename, fallbackFilename);
    }
    onMarkLoadStarted === null || onMarkLoadStarted === void 0 ? void 0 : onMarkLoadStarted(version);
    const loadStartState = buildState();
    onLifecycle === null || onLifecycle === void 0 ? void 0 : onLifecycle(loadStartState.lifecycleContext);
    onStartLoading === null || onStartLoading === void 0 ? void 0 : onStartLoading(loadStartState.loadingMessage);
    return loadStartState;
};
export const createFileViewerLoadStartState = ({ version, source, filename, file, sourceUrl, bufferSize, loadingMessage, i18n, timestamp, }) => {
    return {
        loadingMessage: loadingMessage || resolveFileViewerLoadStartMessage(source, i18n),
        lifecycleContext: buildFileViewerLifecycleContext({
            phase: 'load-start',
            version,
            source,
            file,
            filename,
            url: normalizeFileViewerSourceUrl(sourceUrl) || undefined,
            bufferSize,
            timestamp,
        }),
    };
};
export const createFileViewerRenderCompleteState = ({ version, source, filename, file, sourceUrl, bufferSize, startedAt, timestamp, lifecycleState, }) => {
    return {
        readiness: {
            renderedReady: true,
            progressiveReady: false,
        },
        lifecycleContext: buildFileViewerLifecycleContext({
            phase: 'load-complete',
            version,
            source,
            file,
            filename,
            url: normalizeFileViewerSourceUrl(sourceUrl) || undefined,
            bufferSize,
            startedAt: startedAt !== null && startedAt !== void 0 ? startedAt : lifecycleState === null || lifecycleState === void 0 ? void 0 : lifecycleState.getLoadStartedAt(version),
            timestamp,
        }),
    };
};
export const resolveFileViewerFileRefSourcePlan = ({ source, currentFilename, fallbackFilename = DEFAULT_FILE_VIEWER_SOURCE_FILENAME, }) => {
    const file = wrapFileViewerFileRef(source, currentFilename || fallbackFilename);
    return {
        file,
        filename: resolveFileViewerSourceFilename({ file, fallback: fallbackFilename }),
    };
};
export const normalizePdfStreamingMode = (mode) => {
    if (mode === true || mode === false || mode === 'same-origin') {
        return mode;
    }
    return 'same-origin';
};
export const isSameOriginUrl = (url, pageHref) => {
    try {
        const target = new URL(url, pageHref);
        const page = new URL(pageHref);
        return target.origin === page.origin;
    }
    catch {
        return false;
    }
};
export const shouldStreamPdfUrl = ({ extension, pageHref, streaming, url, }) => {
    if (extension.toLowerCase() !== 'pdf') {
        return false;
    }
    const mode = normalizePdfStreamingMode(streaming);
    if (mode === false) {
        return false;
    }
    if (mode === true) {
        return true;
    }
    return isSameOriginUrl(url, pageHref);
};
export const resolveFileViewerPageHref = (locationLike) => {
    return (locationLike === null || locationLike === void 0 ? void 0 : locationLike.href) || undefined;
};
export const resolveFileViewerRemoteSourcePlan = ({ filename, fallbackFilename = DEFAULT_FILE_VIEWER_SOURCE_FILENAME, pageHref, streaming, url, }) => {
    const sourceUrl = normalizeFileViewerSourceUrl(url) || url;
    const nextFilename = normalizeFilename(filename || url, fallbackFilename);
    const extension = getExtension(nextFilename);
    return {
        url: sourceUrl,
        filename: nextFilename,
        extension,
        streamPdf: pageHref
            ? shouldStreamPdfUrl({
                extension,
                pageHref,
                streaming,
                url: sourceUrl,
            })
            : false,
    };
};
export const commitFileViewerRemoteDownloadState = ({ version, data, currentFilename, fallbackFilename, isCurrent, i18n, onMissingData, onSetLoadingMessage, }) => {
    if (!isCurrent(version)) {
        return {
            stale: true,
            missing: false,
            source: null,
        };
    }
    if (!data) {
        onMissingData === null || onMissingData === void 0 ? void 0 : onMissingData();
        return {
            stale: false,
            missing: true,
            source: null,
        };
    }
    onSetLoadingMessage === null || onSetLoadingMessage === void 0 ? void 0 : onSetLoadingMessage(resolveFileViewerPreviewMessages(i18n).reading);
    return {
        stale: false,
        missing: false,
        source: resolveFileViewerFileRefSourcePlan({
            source: data,
            currentFilename,
            fallbackFilename,
        }),
    };
};
export const createFileViewerStreamingPdfPlaceholderFile = (filename) => {
    if (typeof Blob === 'undefined') {
        throw new Error('Blob is not available in the current execution environment.');
    }
    return wrapFileViewerFileRef(new Blob([], { type: 'application/pdf' }), normalizeFilename(filename, DEFAULT_FILE_VIEWER_STREAMING_PDF_FILENAME));
};
