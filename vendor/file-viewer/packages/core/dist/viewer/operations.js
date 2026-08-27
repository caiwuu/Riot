import { buildFileViewerRenderedHtmlDocument, triggerFileViewerBlobDownload, triggerFileViewerUrlDownload, waitForFileViewerPrintWindowReady, } from '../output/export.js';
import { DEFAULT_FILE_VIEWER_SOURCE_FILENAME } from '../source/index.js';
import { translateFileViewerMessage, } from '../i18n/messages.js';
export const DEFAULT_FILE_VIEWER_PREVIEW_TITLE = 'file-viewer-preview';
export const DEFAULT_FILE_VIEWER_EXPORT_FILENAME = 'preview';
export const DEFAULT_FILE_VIEWER_DOWNLOAD_FILENAME = DEFAULT_FILE_VIEWER_SOURCE_FILENAME;
export const FILE_VIEWER_OPERATION_ACTION_ERROR_PREFIXES = {
    download: '下载失败',
    print: '打印失败',
    'export-html': '导出 HTML 失败',
};
const FILE_VIEWER_OPERATION_ACTION_ERROR_MESSAGE_KEYS = {
    download: 'error.download',
    print: 'error.print',
    'export-html': 'error.exportHtml',
};
const getBlobFilename = (file) => {
    return file && 'name' in file && typeof file.name === 'string' ? file.name : '';
};
const getBlobMimeType = (file) => {
    return file && typeof file.type === 'string' ? file.type : '';
};
export const createFileViewerOriginalSourceState = ({ buffer = null, file = null, url = null, filename = null, mimeType = null, } = {}) => {
    return {
        buffer,
        file,
        url,
        filename,
        mimeType: mimeType || getBlobMimeType(file) || null,
    };
};
export const resolveFileViewerDisplayFilename = (source, fallback = DEFAULT_FILE_VIEWER_EXPORT_FILENAME) => {
    return (source === null || source === void 0 ? void 0 : source.filename) || fallback;
};
export const createFileViewerOriginalSourceStateFromNormalizedSource = (source, fallbackFilename = DEFAULT_FILE_VIEWER_EXPORT_FILENAME) => {
    var _a;
    return createFileViewerOriginalSourceState({
        buffer: source === null || source === void 0 ? void 0 : source.buffer,
        file: source === null || source === void 0 ? void 0 : source.file,
        url: source === null || source === void 0 ? void 0 : source.url,
        filename: resolveFileViewerDisplayFilename(source, fallbackFilename),
        mimeType: (_a = source === null || source === void 0 ? void 0 : source.file) === null || _a === void 0 ? void 0 : _a.type,
    });
};
export const resolveFileViewerOriginalFilename = (source, fallback = 'preview') => {
    return source.filename || getBlobFilename(source.file) || fallback;
};
export const resolveFileViewerOperationFilename = ({ filename, source, fallback = DEFAULT_FILE_VIEWER_PREVIEW_TITLE, }) => {
    return filename || (source ? resolveFileViewerOriginalFilename(source, '') : '') || fallback;
};
export const resolveFileViewerOperationActionErrorMessage = ({ context, formatErrorMessage, prefixes, i18n, }) => {
    var _a;
    return formatErrorMessage((_a = prefixes === null || prefixes === void 0 ? void 0 : prefixes[context.operation]) !== null && _a !== void 0 ? _a : translateFileViewerMessage(i18n, FILE_VIEWER_OPERATION_ACTION_ERROR_MESSAGE_KEYS[context.operation]), context.error, i18n);
};
export const hasFileViewerOriginalSource = (source) => {
    return !!source.buffer || !!source.file || !!source.url;
};
const runBeforeOperation = async (beforeOperation, operation) => {
    if (!beforeOperation) {
        return true;
    }
    return await beforeOperation(operation);
};
const buildRenderedHtmlDocumentFromOperation = async (mode, { source, title, filename, adapter = null, watermarkInlineStyle, mask = null, i18n, }) => {
    if (!source) {
        throw new Error(translateFileViewerMessage(i18n, 'error.noExportContent'));
    }
    return buildFileViewerRenderedHtmlDocument({
        source,
        mode,
        title: resolveFileViewerOperationFilename({
            filename: title || filename,
            fallback: DEFAULT_FILE_VIEWER_PREVIEW_TITLE,
        }),
        adapter,
        watermarkInlineStyle,
        mask: mode === 'print' ? mask : null,
    });
};
export const executeFileViewerDownloadOperation = async ({ source, filename, beforeOperation, i18n, throwOnMissingSource = true, }) => {
    var _a;
    if (!hasFileViewerOriginalSource(source)) {
        if (throwOnMissingSource) {
            throw new Error(translateFileViewerMessage(i18n, 'error.noDownloadSource'));
        }
        return false;
    }
    if (!await runBeforeOperation(beforeOperation, 'download')) {
        return false;
    }
    const resolvedFilename = resolveFileViewerOperationFilename({
        filename,
        source,
        fallback: DEFAULT_FILE_VIEWER_DOWNLOAD_FILENAME,
    });
    // PDF.js transfers ArrayBuffer-backed data to its worker. After a successful
    // render the original buffer can therefore be detached (byteLength === 0),
    // while the File or URL retained by the viewer is still the complete source.
    // Preserve the existing buffer-first contract for non-empty buffers, but do
    // not turn a detached buffer into a zero-byte download.
    if (source.buffer &&
        (source.buffer.byteLength > 0 || (!source.file && !source.url))) {
        triggerFileViewerBlobDownload(new Blob([source.buffer], { type: source.mimeType || ((_a = source.file) === null || _a === void 0 ? void 0 : _a.type) || 'application/octet-stream' }), resolvedFilename);
        return true;
    }
    if (source.file && (source.file.size > 0 || !source.url)) {
        triggerFileViewerBlobDownload(source.file, resolvedFilename);
        return true;
    }
    if (source.url) {
        triggerFileViewerUrlDownload(source.url, resolvedFilename);
        return true;
    }
    // Empty local files are valid sources. These final fallbacks preserve their
    // intentional zero-byte downloads when no complete File or URL is available.
    if (source.file) {
        triggerFileViewerBlobDownload(source.file, resolvedFilename);
        return true;
    }
    triggerFileViewerBlobDownload(new Blob([source.buffer], { type: source.mimeType || 'application/octet-stream' }), resolvedFilename);
    return true;
};
export const executeFileViewerExportHtmlOperation = async ({ download = true, filename, beforeOperation, i18n, ...input }) => {
    if (!await runBeforeOperation(beforeOperation, 'export-html')) {
        return '';
    }
    const html = await buildRenderedHtmlDocumentFromOperation('export', {
        ...input,
        filename,
        i18n,
    });
    if (download !== false) {
        const baseName = resolveFileViewerOperationFilename({
            filename: filename || input.title,
            fallback: DEFAULT_FILE_VIEWER_EXPORT_FILENAME,
        });
        triggerFileViewerBlobDownload(new Blob([html], { type: 'text/html;charset=utf-8' }), `${baseName}.rendered.html`);
    }
    return html;
};
export const executeFileViewerPrintOperation = async ({ autoPrint = true, beforeOperation, i18n, openWindow, printAvailable = true, printWindow, ...input }) => {
    if (!printAvailable) {
        throw new Error(translateFileViewerMessage(i18n, 'error.printUnavailable'));
    }
    if (!await runBeforeOperation(beforeOperation, 'print')) {
        return false;
    }
    const html = await buildRenderedHtmlDocumentFromOperation('print', { ...input, i18n });
    const targetWindow = printWindow ||
        (openWindow === null || openWindow === void 0 ? void 0 : openWindow()) ||
        (typeof window !== 'undefined' ? window.open('', '_blank') : null);
    if (!targetWindow) {
        throw new Error(translateFileViewerMessage(i18n, 'error.printWindowBlocked'));
    }
    targetWindow.document.open();
    targetWindow.document.write(html);
    targetWindow.document.close();
    targetWindow.focus();
    await waitForFileViewerPrintWindowReady(targetWindow);
    if (autoPrint !== false) {
        targetWindow.print();
    }
    return true;
};
const handleFileViewerOperationActionError = (operation, error, { errorPrefixes, formatErrorMessage, getI18n, i18n, onError, onErrorMessage, }) => {
    var _a;
    const context = { operation, error };
    onError === null || onError === void 0 ? void 0 : onError(context);
    if (formatErrorMessage && onErrorMessage) {
        onErrorMessage(resolveFileViewerOperationActionErrorMessage({
            context,
            formatErrorMessage,
            prefixes: errorPrefixes,
            i18n: (_a = getI18n === null || getI18n === void 0 ? void 0 : getI18n()) !== null && _a !== void 0 ? _a : i18n,
        }), context);
    }
};
export const createFileViewerOperationActionHandlers = ({ getBuffer, getFile, getUrl, getI18n, getFilename, getMimeType, getRenderedSource, getAdapter, getWatermarkInlineStyle, getPrintAvailable, beforeOperation, i18n, errorPrefixes, formatErrorMessage, onError, onErrorMessage, }) => {
    const resolveI18n = () => { var _a; return (_a = getI18n === null || getI18n === void 0 ? void 0 : getI18n()) !== null && _a !== void 0 ? _a : i18n; };
    const getOriginalSource = () => {
        var _a, _b, _c, _d;
        const file = (_a = getFile === null || getFile === void 0 ? void 0 : getFile()) !== null && _a !== void 0 ? _a : null;
        return createFileViewerOriginalSourceState({
            buffer: (_b = getBuffer === null || getBuffer === void 0 ? void 0 : getBuffer()) !== null && _b !== void 0 ? _b : null,
            file,
            url: (_c = getUrl === null || getUrl === void 0 ? void 0 : getUrl()) !== null && _c !== void 0 ? _c : null,
            filename: getFilename(),
            mimeType: (_d = getMimeType === null || getMimeType === void 0 ? void 0 : getMimeType()) !== null && _d !== void 0 ? _d : getBlobMimeType(file),
        });
    };
    const getRenderedOperationInput = () => {
        var _a, _b;
        const filename = getFilename() || undefined;
        return {
            source: getRenderedSource(),
            adapter: (_a = getAdapter === null || getAdapter === void 0 ? void 0 : getAdapter()) !== null && _a !== void 0 ? _a : null,
            title: filename,
            filename,
            watermarkInlineStyle: (_b = getWatermarkInlineStyle === null || getWatermarkInlineStyle === void 0 ? void 0 : getWatermarkInlineStyle()) !== null && _b !== void 0 ? _b : undefined,
            beforeOperation,
            i18n: resolveI18n(),
        };
    };
    return {
        async downloadOriginalFile() {
            try {
                return await executeFileViewerDownloadOperation({
                    source: getOriginalSource(),
                    beforeOperation,
                    i18n: resolveI18n(),
                    throwOnMissingSource: false,
                });
            }
            catch (error) {
                handleFileViewerOperationActionError('download', error, {
                    errorPrefixes,
                    formatErrorMessage,
                    getI18n,
                    i18n,
                    onError,
                    onErrorMessage,
                });
                return undefined;
            }
        },
        async exportRenderedHtml() {
            try {
                return await executeFileViewerExportHtmlOperation(getRenderedOperationInput());
            }
            catch (error) {
                handleFileViewerOperationActionError('export-html', error, {
                    errorPrefixes,
                    formatErrorMessage,
                    getI18n,
                    i18n,
                    onError,
                    onErrorMessage,
                });
                return undefined;
            }
        },
        async printRenderedHtml(options = {}) {
            var _a, _b, _c;
            try {
                return await executeFileViewerPrintOperation({
                    ...getRenderedOperationInput(),
                    ...options,
                    watermarkInlineStyle: (_b = (_a = options.watermarkInlineStyle) !== null && _a !== void 0 ? _a : getWatermarkInlineStyle === null || getWatermarkInlineStyle === void 0 ? void 0 : getWatermarkInlineStyle()) !== null && _b !== void 0 ? _b : undefined,
                    printAvailable: (_c = getPrintAvailable === null || getPrintAvailable === void 0 ? void 0 : getPrintAvailable()) !== null && _c !== void 0 ? _c : true,
                });
            }
            catch (error) {
                handleFileViewerOperationActionError('print', error, {
                    errorPrefixes,
                    formatErrorMessage,
                    getI18n,
                    i18n,
                    onError,
                    onErrorMessage,
                });
                return undefined;
            }
        },
        async printWithMask(options = {}) {
            var _a, _b, _c, _d, _e, _f, _g, _h;
            try {
                const source = getRenderedSource();
                if (!source) {
                    throw new Error(translateFileViewerMessage(resolveI18n(), 'error.noExportContent'));
                }
                const { openFileViewerPrintMaskDesignerAsync } = await import('../features/printMaskLoader.js');
                const adapter = (_a = getAdapter === null || getAdapter === void 0 ? void 0 : getAdapter()) !== null && _a !== void 0 ? _a : null;
                const result = await openFileViewerPrintMaskDesignerAsync({
                    root: source,
                    pages: (_b = adapter === null || adapter === void 0 ? void 0 : adapter.getPrintMaskPages) === null || _b === void 0 ? void 0 : _b.call(adapter),
                    i18n: resolveI18n(),
                    color: (_c = options.mask) === null || _c === void 0 ? void 0 : _c.color,
                    initialRegions: (_d = options.mask) === null || _d === void 0 ? void 0 : _d.regions,
                    initialStamps: (_e = options.mask) === null || _e === void 0 ? void 0 : _e.stamps,
                });
                if (!(result === null || result === void 0 ? void 0 : result.mask)) {
                    return undefined;
                }
                return await executeFileViewerPrintOperation({
                    ...getRenderedOperationInput(),
                    ...options,
                    watermarkInlineStyle: (_g = (_f = options.watermarkInlineStyle) !== null && _f !== void 0 ? _f : getWatermarkInlineStyle === null || getWatermarkInlineStyle === void 0 ? void 0 : getWatermarkInlineStyle()) !== null && _g !== void 0 ? _g : undefined,
                    mask: result.mask,
                    printAvailable: (_h = getPrintAvailable === null || getPrintAvailable === void 0 ? void 0 : getPrintAvailable()) !== null && _h !== void 0 ? _h : true,
                });
            }
            catch (error) {
                handleFileViewerOperationActionError('print', error, {
                    errorPrefixes,
                    formatErrorMessage,
                    getI18n,
                    i18n,
                    onError,
                    onErrorMessage,
                });
                return undefined;
            }
        },
    };
};
export const createFileViewerPublicOperationActionHandlers = (input) => {
    const actions = createFileViewerOperationActionHandlers(input);
    return {
        async downloadOriginalFile() {
            await actions.downloadOriginalFile();
        },
        async exportRenderedHtml() {
            await actions.exportRenderedHtml();
        },
        async printRenderedHtml(options) {
            await actions.printRenderedHtml(options);
        },
        async printWithMask(options) {
            await actions.printWithMask(options);
        },
    };
};
