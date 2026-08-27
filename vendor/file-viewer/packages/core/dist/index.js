export { DEFAULT_FILE_VIEWER_ARCHIVE_WORKER_PATH, DEFAULT_FILE_VIEWER_ARCHIVE_WASM_PATH, DEFAULT_FILE_VIEWER_CAD_DWF_WASM_PATH, DEFAULT_FILE_VIEWER_CAD_LIBREDWG_SCRIPT_PATH, DEFAULT_FILE_VIEWER_CAD_LIBREDWG_WASM_PATH, DEFAULT_FILE_VIEWER_CAD_RUNTIME_VERSION, DEFAULT_FILE_VIEWER_CAD_WASM_PATH, DEFAULT_FILE_VIEWER_CAD_WORKER_PATH, DEFAULT_FILE_VIEWER_DATA_SQL_WASM_URL, DEFAULT_FILE_VIEWER_MODEL_RUNTIME_PACKAGE_PATH, DEFAULT_FILE_VIEWER_MODEL_RUNTIME_URL, DEFAULT_FILE_VIEWER_MODEL_IMPORT_LICENSE_PACKAGE_PATH, DEFAULT_FILE_VIEWER_MODEL_IMPORT_LICENSE_URL, DEFAULT_FILE_VIEWER_MODEL_OCCT_LICENSE_PACKAGE_PATH, DEFAULT_FILE_VIEWER_MODEL_OCCT_LICENSE_URL, DEFAULT_FILE_VIEWER_MODEL_WASM_PACKAGE_PATH, DEFAULT_FILE_VIEWER_MODEL_WASM_URL, DEFAULT_FILE_VIEWER_MODEL_WORKER_URL, DEFAULT_FILE_VIEWER_DOCX_WORKER_JSZIP_PATH, DEFAULT_FILE_VIEWER_DOCX_WORKER_PATH, DEFAULT_FILE_VIEWER_DOCX_RUNTIME_VERSION, DEFAULT_FILE_VIEWER_PDF_CMAP_PATH, DEFAULT_FILE_VIEWER_PDF_STANDARD_FONT_PATH, DEFAULT_FILE_VIEWER_PDF_WASM_PATH, DEFAULT_FILE_VIEWER_PDF_WORKER_PATH, DEFAULT_FILE_VIEWER_PPT_FONT_PATH, DEFAULT_FILE_VIEWER_PPT_FRAME_CACHE_PATH, DEFAULT_FILE_VIEWER_PPT_MODULE_PATH, DEFAULT_FILE_VIEWER_PPT_RUNTIME_PATH, DEFAULT_FILE_VIEWER_PPT_RUNTIME_VERSION, DEFAULT_FILE_VIEWER_PPT_WASM_PATH, DEFAULT_FILE_VIEWER_PPT_WORKER_PATH, DEFAULT_FILE_VIEWER_PRESENTATION_WORKER_PATH, DEFAULT_FILE_VIEWER_RENDERER_ASSET_MANIFESTS, DEFAULT_FILE_VIEWER_SPREADSHEET_WORKER_PATH, DEFAULT_FILE_VIEWER_IWORK_WORKER_PATH, DEFAULT_FILE_VIEWER_IWORK_WORKER_PACKAGE_PATH, DEFAULT_FILE_VIEWER_HANGUL_WORKER_PATH, DEFAULT_FILE_VIEWER_HANGUL_WORKER_PACKAGE_PATH, DEFAULT_FILE_VIEWER_WORDPERFECT_WORKER_PATH, DEFAULT_FILE_VIEWER_WORDPERFECT_WORKER_PACKAGE_PATH, DEFAULT_FILE_VIEWER_WORDPERFECT_WASM_PATH, DEFAULT_FILE_VIEWER_WORDPERFECT_WASM_PACKAGE_PATH, DEFAULT_FILE_VIEWER_WORDPERFECT_MODULE_PATH, DEFAULT_FILE_VIEWER_WORDPERFECT_MODULE_PACKAGE_PATH, DEFAULT_FILE_VIEWER_TYPST_COMPILER_WASM_URL, DEFAULT_FILE_VIEWER_TYPST_RENDERER_WASM_URL, DEFAULT_FILE_VIEWER_TYPST_RENDERER_WASM_PACKAGE_PATH, getDefaultFileViewerAssetBaseUrl, getFileViewerRendererAssetManifest, listFileViewerRendererAssetManifests, normalizeFileViewerAssetBaseUrl, resetDefaultFileViewerAssetBaseUrl, resolveFileViewerArchiveWasmUrl, resolveFileViewerArchiveWorkerUrl, resolveFileViewerAssetUrl, resolveFileViewerCadAssetUrls, resolveFileViewerDataSqlWasmUrl, resolveFileViewerDocxWorkerJsZipUrl, resolveFileViewerDocxWorkerUrl, resolveFileViewerDrawioViewerScriptUrl, resolveFileViewerModelAssetUrls, resolveFileViewerPdfAssetUrls, resolveFileViewerPresentationWorkerUrl, resolveFileViewerRendererAssets, resolveFileViewerRuntimeAssetBaseUrl, resolveFileViewerSpreadsheetWorkerUrl, resolveFileViewerIworkWorkerUrl, resolveFileViewerHangulWorkerUrl, resolveFileViewerWordPerfectWorkerUrl, resolveFileViewerWordPerfectWasmUrl, resolveFileViewerTypstCompilerWasmUrl, resolveFileViewerTypstRendererWasmUrl, setDefaultFileViewerAssetBaseUrl, } from './assets.js';
export { ARCHIVE_EXTENSIONS, DEFAULT_REGISTERED_EXTENSIONS, DEFAULT_RENDERER_DEFINITIONS, DEFAULT_STABLE_SUPPORTED_EXTENSIONS, DEFAULT_SUPPORTED_EXTENSIONS, IMAGE_EXTENSIONS, MODEL_EXTENSIONS, TEXT_EXTENSIONS, } from './registry/formats.js';
export { precheckFileViewerSource } from './source/precheck.js';
export { DEFAULT_FILE_VIEWER_TEXT_CHUNK_OVERLAP, DEFAULT_FILE_VIEWER_TEXT_CHUNK_SIZE, DEFAULT_FILE_VIEWER_ZOOM_SCALE, buildFileViewerDocumentTextChunks, createEmptyFileViewerSearchState, createFileViewerZoomState, normalizeFileViewerAiOptions, normalizeFileViewerSearchOptions, } from './features/document/model.js';
export { DEFAULT_FILE_VIEWER_SEARCH_ACTIVE_CLASS, DEFAULT_FILE_VIEWER_SEARCH_MATCH_CLASS, DEFAULT_FILE_VIEWER_SEARCH_MAX_MATCHES, applyFileViewerSearchState, cloneFileViewerSearchState, createFileViewerDomSearchController, createFileViewerDomSearchControllerActionHandlers, destroyFileViewerDomSearchController, observeFileViewerDomSearchController, runFileViewerDomSearchControllerAction, syncFileViewerDomSearchControllerState, } from './features/document/search.js';
export { createFileViewerDocumentFeatureActions, createFileViewerDocumentFeatureControllerActionHandlers, createFileViewerDocumentChangeSnapshot, createFileViewerSearchChangeState, dispatchFileViewerLocationChange, dispatchFileViewerSearchChange, resolveFileViewerLocationChangeAnchor, } from './features/document/events.js';
export { applyFileViewerZoomState, clearFileViewerZoomControllerProvider, createFileViewerZoomChangeEmitter, createFileViewerZoomChangeState, cloneFileViewerZoomState, createFileViewerZoomController, createFileViewerZoomControllerActionHandlers, destroyFileViewerZoomController, observeFileViewerZoomController, refreshFileViewerZoomControllerProvider, runFileViewerZoomControllerAction, syncFileViewerZoomControllerState, } from './features/document/zoom.js';
export { FILE_VIEWER_FIT_MODES, FILE_VIEWER_FIT_RESIZE_MODES, createFileViewerFitController, hasFileViewerExplicitInitialViewState, isFileViewerFitMode, isFileViewerFitResize, normalizeFileViewerFitOptions, resolveFileViewerFitScale, } from './features/document/fit.js';
export { cloneFileViewerViewState, createFileViewerViewStateChange, createFileViewerViewStateChangeEmitter, createFileViewerViewStateController, createFileViewerViewStateControllerActionHandlers, registerFileViewerGenericViewStateProvider, } from './features/document/viewState.js';
export { DEFAULT_FILE_VIEWER_ANCHOR_EXCLUDE_SELECTOR, DEFAULT_FILE_VIEWER_ANCHOR_SELECTOR, DEFAULT_FILE_VIEWER_SCROLL_CONTAINER_CANDIDATE_SELECTOR, DEFAULT_FILE_VIEWER_SCROLL_CONTAINER_SELECTOR, DEFAULT_FILE_VIEWER_SCROLLABLE_OVERFLOW_VALUES, collectFileViewerDocumentAnchors, findFileViewerAnchorForElement, findFileViewerSearchProvider, findFileViewerViewStateProvider, findFileViewerZoomProvider, getCurrentFileViewerDocumentAnchor, getFileViewerScrollableRange, isFileViewerScrollableElement, registerFileViewerSearchProvider, registerFileViewerViewStateProvider, registerFileViewerZoomProvider, resolveFileViewerScrollContainer, scrollToFileViewerDocumentAnchor, unregisterFileViewerSearchProvider, unregisterFileViewerViewStateProvider, unregisterFileViewerZoomProvider, } from './features/document/dom/index.js';
export { buildFileViewerRenderedHtmlDocument, buildExportHtmlDocument, collectDocumentStyles, prepareFileViewerRenderedContentForSnapshot, inlineFileViewerBlobUrlsInHtml, replaceFileViewerCanvasWithImages, resolveFileViewerPrintStyle, triggerFileViewerBlobDownload, triggerFileViewerUrlDownload, waitForFileViewerImages, waitForFileViewerNextPaint, waitForFileViewerPrintWindowReady, } from './output/export.js';
export { applyPrintPageSize, buildPrintPageStyle, formatCssPixels, getElementPrintPageSize, } from './output/printLayout.js';
export { DEFAULT_FILE_VIEWER_DOWNLOAD_FILENAME, DEFAULT_FILE_VIEWER_EXPORT_FILENAME, DEFAULT_FILE_VIEWER_PREVIEW_TITLE, FILE_VIEWER_OPERATION_ACTION_ERROR_PREFIXES, createFileViewerOperationActionHandlers, createFileViewerOriginalSourceState, createFileViewerOriginalSourceStateFromNormalizedSource, createFileViewerPublicOperationActionHandlers, executeFileViewerDownloadOperation, executeFileViewerExportHtmlOperation, executeFileViewerPrintOperation, hasFileViewerOriginalSource, resolveFileViewerDisplayFilename, resolveFileViewerOperationActionErrorMessage, resolveFileViewerOperationFilename, resolveFileViewerOriginalFilename, } from './viewer/operations.js';
export { FILE_VIEWER_BUILTIN_MESSAGES, FILE_VIEWER_DEFAULT_LOCALE, FILE_VIEWER_FALLBACK_LOCALE, FILE_VIEWER_SUPPORTED_LOCALES, createFileViewerTranslator, formatFileViewerMessage, normalizeFileViewerLocale, resolveFileViewerLocale, translateFileViewerMessage, } from './i18n/messages.js';
export { clearFileViewerAutoRendererPresets, collectFileViewerRendererPlugins, createRendererRegistry, findFileViewerAutoRendererPreset, getFileViewerAutoRendererPresetVersion, hasFileViewerRendererPresetName, installFileViewerRendererPlugins, listFileViewerAutoRendererPresetEntries, listFileViewerAutoRendererPresets, registerFileViewerAutoRendererPreset, resolveFileViewerRendererPresetInputs, unregisterFileViewerAutoRendererPreset, } from './registry/registry.js';
export { CORE_LITE_RENDERER_IDS, coreBrowserRendererHandlers, coreLiteBrowserRendererHandlers, coreLiteRendererDefinitions, createFileViewerCoreRendererRegistry, fileViewerCoreRendererDispatcher, fileViewerCoreRendererRegistry, fileViewerCoreRendererRegistryBridge, missingFileViewerCoreRendererHandlers, } from './renderers/index.js';
export const renderFileViewerAudio = async (buffer, target, type) => {
    void buffer;
    void target;
    void type;
    throw new Error('Audio and MIDI rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-media, or use @file-viewer/preset-all.');
};
export const renderFileViewerArchive = async (buffer, target, type, context) => {
    void buffer;
    void target;
    void type;
    void context;
    throw new Error('Archive rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-archive, or use @file-viewer/preset-all.');
};
export const renderFileViewerCad = async (buffer, target, type, context) => {
    void buffer;
    void target;
    void type;
    void context;
    throw new Error('CAD rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-cad, or use @file-viewer/preset-all.');
};
export const renderFileViewerCode = async (buffer, target, type) => {
    void buffer;
    void target;
    void type;
    throw new Error('Code and text rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-text, or use @file-viewer/preset-all.');
};
export const renderFileViewerDataAsset = async (buffer, target, type, context) => {
    void buffer;
    void target;
    void type;
    void context;
    throw new Error('Data asset rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-data, or use @file-viewer/preset-all.');
};
export const renderFileViewerDrawing = async (buffer, target, type, context) => {
    void buffer;
    void target;
    void type;
    void context;
    throw new Error('Draw.io and Excalidraw rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-drawing, or use @file-viewer/preset-all.');
};
export const renderFileViewerEda = async (buffer, target, type, context) => {
    void buffer;
    void target;
    void type;
    void context;
    throw new Error('EDA rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-eda, or use @file-viewer/preset-all.');
};
export const renderFileViewerEmail = async (buffer, target, type, context) => {
    void buffer;
    void target;
    void type;
    void context;
    throw new Error('Email rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-email, or use @file-viewer/preset-all.');
};
export const renderFileViewerEpub = async (buffer, target) => {
    void buffer;
    void target;
    throw new Error('EPUB rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-epub, or use @file-viewer/preset-all.');
};
export const renderFileViewerGeo = async (buffer, target, type) => {
    void buffer;
    void target;
    void type;
    throw new Error('Geospatial rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-geo, or use @file-viewer/preset-all.');
};
export const renderFileViewerImage = async (buffer, target, type) => {
    const { default: renderImage } = await import('./renderers/image.js');
    return renderImage(buffer, target, type);
};
export const renderFileViewerMarkdown = async (buffer, target) => {
    void buffer;
    void target;
    throw new Error('Markdown rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-text, or use @file-viewer/preset-all.');
};
export const renderFileViewerModel = async (buffer, target, type, context) => {
    void buffer;
    void target;
    void type;
    void context;
    throw new Error('3D model rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-3d, or use @file-viewer/preset-all.');
};
export const renderFileViewerOfd = async (buffer, target, context) => {
    void buffer;
    void target;
    void context;
    throw new Error('OFD rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-ofd, or use @file-viewer/preset-all.');
};
export const renderFileViewerOpenDocument = async (buffer, target, type) => {
    void buffer;
    void target;
    void type;
    throw new Error('OpenDocument/RTF rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-word, or use @file-viewer/preset-all.');
};
export const renderFileViewerPdf = async (buffer, target, context) => {
    void buffer;
    void target;
    void context;
    throw new Error('PDF rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-pdf, or use @file-viewer/preset-all.');
};
export const renderFileViewerPptx = async (buffer, target, type, context) => {
    void buffer;
    void target;
    void type;
    void context;
    throw new Error('PPTX rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-presentation, or use @file-viewer/preset-all.');
};
export const renderFileViewerTypst = async (buffer, target, type, context) => {
    void buffer;
    void target;
    void type;
    void context;
    throw new Error('Typst rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-typst, or use @file-viewer/preset-all.');
};
export const renderFileViewerUmd = async (buffer, target) => {
    void buffer;
    void target;
    throw new Error('UMD ebook rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-epub, or use @file-viewer/preset-all.');
};
export const renderFileViewerVideo = async (buffer, target, type, context) => {
    void buffer;
    void target;
    void type;
    void context;
    throw new Error('Video and HLS rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-media, or use @file-viewer/preset-all.');
};
export const renderFileViewerWordDoc = async (buffer, target, context) => {
    void buffer;
    void target;
    void context;
    throw new Error('DOC rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-word, or use @file-viewer/preset-all.');
};
export const renderFileViewerWordDocx = async (buffer, target, context) => {
    void buffer;
    void target;
    void context;
    throw new Error('DOCX rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-word, or use @file-viewer/preset-all.');
};
export const renderFileViewerSpreadsheet = async (buffer, target, type, context) => {
    void buffer;
    void target;
    void type;
    void context;
    throw new Error('Spreadsheet rendering has moved out of @file-viewer/core. Install and pass @file-viewer/renderer-spreadsheet, or use @file-viewer/preset-all.');
};
export const parseEdaFile = async (buffer, type) => {
    void buffer;
    void type;
    throw new Error('EDA parsing has moved out of @file-viewer/core. Import parseEdaFile from @file-viewer/renderer-eda instead.');
};
export { ADAPTER_PRINT_REQUIRED_EXTENSIONS, applyFileViewerZoomAvailability, createUnsupportedAvailability, DEFAULT_OPERATION_AVAILABILITY, DOM_PRINTABLE_EXTENSIONS, getRendererAvailability, isDomPrintableExtension, isKnownNonPrintableExtension, needsDedicatedPrintAdapter, NON_PRINTABLE_EXTENSIONS, resolvePrintAvailability, } from './registry/capabilities.js';
export { FILE_VIEWER_LIFECYCLE_HOOKS, FILE_VIEWER_BEFORE_OPERATION_ERROR_PREFIX, DEFAULT_FILE_VIEWER_TOOLBAR_ORDER, FILE_VIEWER_LIFECYCLE_HOOK_ERROR_MESSAGE_PREFIX, FILE_VIEWER_OPERATION_LABELS, buildFileViewerLifecycleContext, buildFileViewerLifecycleContextFromNormalizedSource, buildFileViewerOperationContext, buildFileViewerOperationContextFromLifecycleState, cloneFileViewerOperationAvailability, createFileViewerLifecycleActions, createFileViewerLifecycleStateController, createFileViewerPublicApi, createFileViewerToolbarActions, createFileViewerToolbarControllerActionHandlers, createFileViewerToolbarZoomSyncSnapshot, DEFAULT_FILE_VIEWER_LIFECYCLE_HOOK_ERROR_LOGGER, DEFAULT_FILE_VIEWER_OPERATION_ERROR_LOGGER, dispatchFileViewerLifecycleEvent, dispatchFileViewerOperationContextEvent, dispatchFileViewerOperationAvailabilityChange, dispatchFileViewerZoomChange, emitFileViewerComponentLifecycleEvent, getFileViewerBeforeOperationHooks, getFileViewerLifecycleHookName, hasVisibleFileViewerToolbarActions, isFileViewerToolbarOperationPermitted, isFileViewerZoomButtonDisabled, normalizeFileViewerToolbar, reportFileViewerLifecycleHookError, reportFileViewerOperationError, resolveFileViewerLifecycleFallbackSource, resolveFileViewerLifecycleHookErrorMessage, resolveFileViewerBeforeOperationErrorMessage, resolveFileViewerOperationAvailability, resolveFileViewerToolbarOrder, resolveFileViewerToolbarState, resolveFileViewerToolbarPosition, resolveVisibleFileViewerToolbar, runFileViewerActiveUnloadComplete, runFileViewerActiveUnloadStart, runFileViewerBeforeOperation, runFileViewerLifecycleHook, runFileViewerToolbarAvailabilitySync, runFileViewerToolbarZoomSync, serializeFileViewerContext, } from './lifecycle/operations.js';
export { FALLBACK_FILE_VIEWER_LOADING_THEME, FILE_VIEWER_LOADING_THEME_MAP, applyFileViewerLoadingState, cloneFileViewerLoadingState, createFileViewerLoadingController, createFileViewerLoadingControllerActionHandlers, createFileViewerLoadingState, createFileViewerLoadingStyleVars, runFileViewerLoadingControllerAction, runFileViewerLoadingExtensionSync, resolveFileViewerLoadingTheme, syncFileViewerLoadingControllerState, } from './viewer/loading.js';
export { createFileViewerLifecycleFacade, } from './lifecycle/facade.js';
export { getFileViewerOptionsSearchParam, normalizeFileViewerUiDensity, normalizeFileViewerTheme, parseFileViewerOptions, resolveFileViewerColorScheme, resolveFileViewerUiDensity, sanitizeFileViewerOptions, serializeFileViewerOptions, setFileViewerOptionsSearchParam, toggleFileViewerColorScheme, } from './config/options.js';
export { resolveFileViewerPresentationState, } from './presentation/state.js';
export { createFileViewerRendererDispatcher, } from './rendering/dispatcher.js';
export { appendFileViewerStyle, buildFileRenderContextFromLoadContext, applyFileViewerRenderSurfaceState, clearFileViewerRenderSurface, createFileRenderHandlerRendererSession, createFileRenderHandlerRegistry, createFileRenderHandlerLoader, createFileViewerRenderSurfaceActionHandlers, createFileViewerRenderReadinessTarget, createFileViewerRenderSurface, createFileViewerRenderSurfaceState, createFileViewerRenderSurfaceStateTarget, createFileViewerRenderTarget, DEFAULT_FILE_VIEWER_RENDER_TARGET_CLASS, FILE_VIEWER_RENDER_SURFACE_BACKGROUND_PROPERTY, DEFAULT_FILE_VIEWER_RENDER_SESSION_DISPOSE_ERROR_LOGGER, FILE_VIEWER_RENDER_SESSION_DISPOSE_ERROR_MESSAGE, disposeActiveFileViewerRendererSession, disposeFileViewerRendered, disposeFileViewerRendererSession, getFileViewerShadowRootForNode, isFileViewerShadowRoot, normalizeFileViewerStyleIsolation, normalizeFileViewerRenderSurfaceBackground, removeFileViewerRenderTarget, reportFileViewerRenderSessionDisposeError, resetFileViewerRenderSurface, resolveFileViewerStyleIsolation, resolveFileViewerRenderSessionDisposeErrorMessage, runFileViewerRenderSurfaceClear, runFileViewerRenderSurfaceMount, syncFileViewerRenderSurfaceBackground, renderFileViewerHandler, } from './rendering/handler.js';
export { DEFAULT_FILE_VIEWER_SOURCE_FILENAME, createFileViewerTextDecoder, decodeFileViewerTextBuffer, decodeFilename, getExtension, normalizeFileExtension, normalizeFilename, isValidFileViewerUtf8, resolveFileViewerTextEncoding, resolveFileViewerSourceFilename, normalizeSource, readFileViewerBuffer, readFileViewerDataUrl, readFileViewerText, wrapFileViewerFileRef, } from './source/index.js';
export { DEFAULT_FILE_VIEWER_STATE_THEME, DEFAULT_FILE_VIEWER_UNSUPPORTED_DESCRIPTION, FILE_VIEWER_PREVIEW_MESSAGES, createFileViewerEmptyState, createFileViewerErrorState, createFileViewerPreviewLoadingState, createFileViewerReadyState, createFileViewerUnsupportedState, formatFileViewerErrorMessage, normalizeFileViewerErrorMessage, resolveFileViewerRendererInstallHint, } from './viewer/state.js';
export { buildFileViewerWatermarkBackgroundImage, buildFileViewerWatermarkInlineStyle, buildFileViewerWatermarkStyle, buildFileViewerWatermarkSvg, normalizeFileViewerWatermark, resolveFileViewerWatermarkPresentationState, } from './features/watermark.js';
export { buildFileViewerPrintMaskOverlayHtml, applyFileViewerPagePrintMasksToHtml, normalizeFileViewerPrintMaskOptions, normalizeFileViewerPrintMaskRegion, normalizeFileViewerPrintStamp, FILE_VIEWER_PRINT_MASK_STYLE, } from './features/printMask.js';
export { openFileViewerPrintMaskDesignerAsync, } from './features/printMaskLoader.js';
export { cancelFileViewerPreviewRequest, DEFAULT_FILE_VIEWER_STREAMING_PDF_FILENAME, DEFAULT_FILE_VIEWER_PREVIEW_LOAD_ERROR_LOGGER, DEFAULT_PDF_RANGE_CHUNK_SIZE, FILE_VIEWER_PREVIEW_LOAD_ERROR_PREFIXES, FILE_VIEWER_REMOTE_MISSING_DATA_ERROR_MESSAGE, applyFileViewerEmptyPreviewState, applyFileViewerPreviewFilenameState, applyFileViewerPreviewSourceUrlState, applyFileViewerReadPreviewState, applyFileViewerRenderReadinessState, applyFileViewerPreviewRequestResetState, commitFileViewerEmptyPreviewResetState, commitFileViewerLoadStartState, commitFileViewerPreviewRequestStartState, commitFileViewerRenderCompleteState, commitFileViewerRemoteDownloadState, createFileViewerEmptyPreviewState, createFileViewerLoadStartState, createFileViewerPreviewStateTarget, createFileViewerSourceLoadingActionHandlers, createFileViewerReadPreviewState, createFileViewerPreviewRequestResetState, createFileViewerRenderCompleteState, createFileViewerRequestController, createFileViewerRequestScope, createFileViewerStreamingPdfPlaceholderFile, finalizeFileViewerPreviewLoadState, hasFileViewerPreviewSource, isFileViewerAbortError, isSameOriginUrl, normalizeFileViewerSourceUrl, normalizePdfStreamingMode, resolveFileViewerFileRefSourcePlan, resolveFileViewerLoadStartMessage, resolveFileViewerMissingRemoteDataErrorMessage, resolveFileViewerPreviewLoadErrorMessage, resolveFileViewerPreviewRequestReason, resolveFileViewerRemoteSourcePlan, resolveFileViewerPageHref, reportFileViewerMissingRemoteData, reportFileViewerPreviewLoadError, runFileViewerLocalFilePreview, runFileViewerPreviewComponentUnmount, runFileViewerPreviewRequest, runFileViewerPreviewSourceChange, runFileViewerRemoteFilePreview, runFileViewerReadAndRenderFile, runFileViewerStreamingPdfPreview, shouldStreamPdfUrl, } from './source/loading.js';
export { createViewer } from './viewer/createViewer.js';
export { WorkerRefImpl, createFileViewerWorkerController, refWorker, } from './platform/worker.js';
