import { createFileViewerWorkerController, createFileViewerTranslator, createFileViewerZoomChangeEmitter as createZoomChangeEmitter, registerFileViewerZoomProvider, resolveFileViewerColorScheme, resolveFileViewerFitScale, resolveFileViewerRuntimeAssetBaseUrl, resolveFileViewerSpreadsheetWorkerUrl, unregisterFileViewerZoomProvider, } from '@file-viewer/core';
import { renderSpreadsheetChart } from './spreadsheet/chartRenderer.js';
import { createSpreadsheetImageSourceResolver } from './spreadsheet/imageSource.js';
import { buildRows, clampWindowStart, collectWindowStarts, createEmptyVirtualState, DEFAULT_SHEET_DEFAULTS, displayCellKey, getDataKey, INDEX_COLUMN_KEY, markWindowState, ROW_STATE_FIELD, RowState, WINDOW_SIZE, } from './spreadsheet/state.js';
import { buildColumns, createTableConfig, detectIndexOffset, getDisplayColumns, getRowHeight, HEADER_HEIGHT, INDEX_COLUMN_WIDTH, normalizeCellStyle, normalizeRowHeight, RESIZABLE_COLUMN_MIN_WIDTH, RESIZABLE_ROW_MIN_HEIGHT, SPREADSHEET_MAX_ZOOM, SPREADSHEET_MIN_ZOOM, } from './spreadsheet/view.js';
const EXCEL_IMAGE_SCROLLBAR_GUARD = 18;
const DEFAULT_SPREADSHEET_WORKER_AUTO_THRESHOLD = 1 * 1024 * 1024;
const E_VIRT_TABLE_STYLE_MARKERS = [
    '.e-virt-table-container',
    '.e-virt-table-overlayer',
    '.e-virt-table-editor',
    '.e-virt-table-context-menu',
];
const SPREADSHEET_STYLE_SCOPE = '.excel-wrapper[data-file-viewer-spreadsheet-root]';
const E_VIRT_TABLE_ROTATE_KEYFRAME = 'file-viewer-e-virt-table-rotate';
export const enableEVirtTableShadowEventTargeting = (context) => {
    const isNativeTarget = context.isTarget.bind(context);
    context.isTarget = (event) => {
        if (isNativeTarget(event)) {
            return true;
        }
        const path = typeof event.composedPath === 'function' ? event.composedPath() : [];
        return path.includes(context.containerElement);
    };
};
const spreadsheetStyle = `
.excel-wrapper{position:relative;width:100%;height:100%;display:flex;flex-direction:column;background:var(--file-viewer-render-surface-background,#fff);color:#172033;font-family:Aptos,'Segoe UI','PingFang SC','Microsoft YaHei',sans-serif}
.excel-wrapper *{box-sizing:border-box}
.excel-wrapper .table-wrapper{position:relative;width:100%;flex:1;min-height:0;background:#fff;overflow:hidden}
.excel-wrapper .table-host{position:absolute;inset:0}
.excel-wrapper .table-target{width:100%;height:100%}
.excel-wrapper .table-host .e-virt-table-container,.excel-wrapper .table-host .e-virt-table-stage{width:100%!important}
.excel-wrapper .table-host .e-virt-table-container{height:100%!important}
.excel-wrapper .table-host .e-virt-table-stage{overflow:hidden}
.excel-wrapper .sheet-loading{position:absolute;right:18px;bottom:18px;z-index:20;display:inline-flex;align-items:center;gap:8px;padding:10px 14px;border-radius:14px;background:rgba(33,163,102,.1);border:1px solid rgba(33,163,102,.2);box-shadow:0 8px 20px rgba(33,163,102,.12);color:#1a7f50;font-size:12px;font-weight:700;pointer-events:none}
.excel-wrapper .sheet-loading-dot{width:8px;height:8px;flex-shrink:0;border-radius:999px;background:#21a366;box-shadow:0 0 0 6px rgba(33,163,102,.12);animation:file-viewer-spreadsheet-loading-pulse 1.2s ease-in-out infinite}
.excel-wrapper .sheet-loading-summary{color:#5f6368}
.excel-wrapper .excel-image-viewport{position:absolute;z-index:35;overflow:hidden;pointer-events:none}
.excel-wrapper .excel-image-layer{position:absolute;inset:0 auto auto 0;width:0;height:0;transform-origin:0 0;will-change:transform}
.excel-wrapper .excel-image{position:absolute;display:block;max-width:none;height:auto;object-fit:contain;user-select:none}
.excel-wrapper .excel-chart{position:absolute;display:block;max-width:none;overflow:hidden;background:#fff;box-shadow:0 1px 3px rgba(15,23,42,.08);user-select:none}
.excel-wrapper .excel-chart svg{display:block;width:100%;height:100%;overflow:visible}
.excel-wrapper .excel-image-lightbox{position:absolute;inset:0;z-index:1200;display:flex;align-items:center;justify-content:center;padding:clamp(16px,4vw,48px);background:rgba(15,23,42,.9);box-sizing:border-box;opacity:0;visibility:hidden;pointer-events:none;transition:opacity .18s ease,visibility 0s linear .18s}
.excel-wrapper .excel-image-lightbox[data-open='true']{opacity:1;visibility:visible;pointer-events:auto;transition-delay:0s}
.excel-wrapper .excel-image-lightbox img{display:block;max-width:100%;max-height:100%;object-fit:contain;background:#fff;box-shadow:0 30px 80px rgba(0,0,0,.4);transform:scale(.985);transition:transform .18s ease;user-select:none}
.excel-wrapper .excel-image-lightbox[data-open='true'] img{transform:scale(1)}
.excel-wrapper .excel-image-lightbox button{position:absolute;top:16px;right:16px;display:grid;width:40px;height:40px;place-items:center;padding:0;border:1px solid rgba(255,255,255,.7);border-radius:999px;background:rgba(255,255,255,.96);color:#172033;font:400 27px/1 Arial,sans-serif;cursor:pointer;box-shadow:0 12px 28px rgba(0,0,0,.24);transition:background-color .14s ease,transform .14s ease}
.excel-wrapper .excel-image-lightbox button:hover{background:#fff;transform:scale(1.04)}
.excel-wrapper .excel-image-lightbox button:focus-visible{outline:3px solid #60a5fa;outline-offset:2px}
.excel-wrapper .loading{position:absolute;inset:0;display:flex;align-items:center;justify-content:center;background:rgba(255,255,255,.96);z-index:999;backdrop-filter:blur(6px)}
.excel-wrapper .loading-card{width:min(100%,460px);display:flex;align-items:center;gap:18px;padding:22px;border-radius:24px;background:rgba(255,255,255,.92);border:1px solid rgba(33,163,102,.1);box-shadow:0 22px 48px rgba(18,36,27,.12)}
.excel-wrapper .loading-brand{flex-shrink:0;width:78px;height:78px;display:flex;align-items:center;justify-content:center;border-radius:22px;background:linear-gradient(135deg,rgba(33,163,102,.14),rgba(33,163,102,.04));color:#1a7f50;font-size:18px;font-weight:900;letter-spacing:0}
.excel-wrapper .loading-copy{min-width:0;flex:1}
.excel-wrapper .loading-kicker{display:block;color:#21a366;font-size:12px;font-weight:800;letter-spacing:.08em;text-transform:uppercase}
.excel-wrapper .loading-copy strong{display:block;margin-top:6px;color:#183828;font-size:20px;line-height:1.3}
.excel-wrapper .loading-copy p{margin:8px 0 0;color:#64748b;font-size:13px;line-height:1.5}
.excel-wrapper .loading-spinner{width:28px;height:28px;border-radius:999px;border:3px solid rgba(33,163,102,.16);border-top-color:#21a366;animation:file-viewer-spreadsheet-loading-spin .8s linear infinite}
.excel-wrapper .error{position:absolute;left:50%;top:50%;z-index:1000;transform:translate(-50%,-50%);max-width:min(520px,calc(100% - 48px));padding:16px 18px;border-radius:16px;background:#fff7ed;color:#9a3412;border:1px solid rgba(234,88,12,.18);box-shadow:0 18px 42px rgba(154,52,18,.12);font-size:14px;line-height:1.6}
.excel-wrapper .toolbar{min-height:44px;display:flex;align-items:center;justify-content:space-between;gap:12px;padding:8px 12px;border-top:1px solid #e5e7eb;background:#f8fafc}
.excel-wrapper .btn-group{min-width:0;max-width:100%;flex:1 1 auto;display:flex;align-items:center;gap:6px;overflow-x:auto;overflow-y:hidden;scrollbar-gutter:stable;scrollbar-width:thin;overscroll-behavior-x:contain}
.excel-wrapper .sheet-tab{flex:0 0 auto;width:max-content;min-width:72px;max-width:min(260px,70vw);height:30px;border:1px solid transparent;border-radius:8px;padding:0 12px;background:transparent;color:#526173;font:inherit;font-size:12px;font-weight:700;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;cursor:pointer}
.excel-wrapper .sheet-tab:hover{background:#edf2f7}
.excel-wrapper .sheet-tab.active{border-color:rgba(33,163,102,.28);background:rgba(33,163,102,.12);color:#137347}
.excel-wrapper .summary{flex:0 0 auto;max-width:42%;overflow:hidden;color:#64748b;font-size:12px;font-weight:700;white-space:nowrap;text-overflow:ellipsis}
.excel-wrapper .hidden{display:none!important}
.excel-wrapper[data-spreadsheet-theme='dark']{color-scheme:dark;background:var(--file-viewer-render-surface-background,#0f172a);color:#e5e7eb}
.excel-wrapper[data-spreadsheet-theme='dark'] .table-wrapper{background:#111827}
.excel-wrapper[data-spreadsheet-theme='dark'] .toolbar{background:#111827;border-color:rgba(148,163,184,.22)}
.excel-wrapper[data-spreadsheet-theme='dark'] .sheet-tab{color:#cbd5e1}
.excel-wrapper[data-spreadsheet-theme='dark'] .sheet-tab:hover{background:#1f2937}
.excel-wrapper[data-spreadsheet-theme='dark'] .sheet-tab.active{border-color:rgba(52,211,153,.38);background:rgba(33,163,102,.2);color:#6ee7b7}
.excel-wrapper[data-spreadsheet-theme='dark'] .summary,.excel-wrapper[data-spreadsheet-theme='dark'] .sheet-loading-summary{color:#94a3b8}
.excel-wrapper[data-spreadsheet-theme='dark'] .loading{background:rgba(2,6,23,.9)}
.excel-wrapper[data-spreadsheet-theme='dark'] .loading-card{border-color:rgba(52,211,153,.2);background:rgba(17,24,39,.96);box-shadow:0 24px 58px rgba(0,0,0,.42)}
.excel-wrapper[data-spreadsheet-theme='dark'] .loading-copy strong{color:#f0fdf4}
.excel-wrapper[data-spreadsheet-theme='dark'] .loading-copy p{color:#94a3b8}
.excel-wrapper[data-spreadsheet-theme='dark'] .error{border-color:rgba(251,146,60,.28);background:#2a1710;color:#fdba74;box-shadow:0 20px 48px rgba(0,0,0,.36)}
@keyframes file-viewer-spreadsheet-loading-spin{to{transform:rotate(360deg)}}
@keyframes file-viewer-spreadsheet-loading-pulse{0%,100%{opacity:.55;transform:scale(.9)}50%{opacity:1;transform:scale(1)}}
@media (max-width:720px){.excel-wrapper .toolbar{align-items:stretch;flex-direction:column}.excel-wrapper .btn-group{flex:0 0 auto}.excel-wrapper .summary{max-width:none;white-space:normal}.excel-wrapper .sheet-loading{left:12px;right:12px;bottom:58px;justify-content:center}.excel-wrapper .loading-card{margin:18px;flex-direction:column;text-align:center}.excel-wrapper .excel-image-lightbox button{top:12px;right:12px}}
@media (prefers-reduced-motion:reduce){.excel-wrapper .excel-image-lightbox,.excel-wrapper .excel-image-lightbox img,.excel-wrapper .excel-image-lightbox button{transition:none}}
`;
const scopedSpreadsheetStyle = spreadsheetStyle.replace(/\.excel-wrapper/g, SPREADSHEET_STYLE_SCOPE);
const isEVirtTableConstructor = (value) => {
    return typeof value === 'function';
};
const asModuleRecord = (value) => {
    return value && typeof value === 'object'
        ? value
        : null;
};
const getEVirtTableGlobalCandidate = () => {
    const globalRecord = typeof globalThis === 'undefined'
        ? null
        : globalThis;
    return globalRecord === null || globalRecord === void 0 ? void 0 : globalRecord.EVirtTable;
};
export const resolveEVirtTableConstructor = (module) => {
    const record = asModuleRecord(module);
    const defaultRecord = asModuleRecord(record === null || record === void 0 ? void 0 : record.default);
    const moduleExportsRecord = asModuleRecord(record === null || record === void 0 ? void 0 : record['module.exports']);
    const globalCandidate = getEVirtTableGlobalCandidate();
    const candidates = [
        module,
        record === null || record === void 0 ? void 0 : record.default,
        record === null || record === void 0 ? void 0 : record.EVirtTable,
        record === null || record === void 0 ? void 0 : record['module.exports'],
        defaultRecord === null || defaultRecord === void 0 ? void 0 : defaultRecord.default,
        defaultRecord === null || defaultRecord === void 0 ? void 0 : defaultRecord.EVirtTable,
        moduleExportsRecord === null || moduleExportsRecord === void 0 ? void 0 : moduleExportsRecord.default,
        moduleExportsRecord === null || moduleExportsRecord === void 0 ? void 0 : moduleExportsRecord.EVirtTable,
        globalCandidate,
    ];
    const constructor = candidates.find(isEVirtTableConstructor);
    if (!constructor) {
        const keys = record ? Object.keys(record).join(', ') : typeof module;
        throw new Error(`Unable to resolve e-virt-table constructor from module exports: ${keys}`);
    }
    return constructor;
};
const isEVirtTableStyleText = (value) => {
    return E_VIRT_TABLE_STYLE_MARKERS.every(marker => value.includes(marker));
};
const collectEVirtTableStyleDocuments = (documentRef) => [
    documentRef,
    typeof document === 'undefined' ? null : document,
].filter((value, index, values) => (!!value && values.indexOf(value) === index));
const collectStyleElements = (documents) => documents.flatMap(candidateDocument => { var _a; return Array.from(((_a = candidateDocument.head) === null || _a === void 0 ? void 0 : _a.querySelectorAll('style')) || []); });
let loadedEVirtTableStyleText = '';
// e-virt-table bundles its complete stylesheet into the ESM entry and injects
// unscoped :root, .dark, and table selectors into document.head. Capture that
// exact version-matched CSS, remove only the node created by our import, and
// install a scoped copy beside the renderer for both light and shadow DOM.
// Independent layout, overlay, editor, and menu markers avoid mistaking an
// app's partial compatibility overrides for the complete vendor sheet.
export const resolveEVirtTableStyleText = (documentRef) => {
    var _a;
    const styles = collectStyleElements(collectEVirtTableStyleDocuments(documentRef));
    for (let index = styles.length - 1; index >= 0; index -= 1) {
        const cssText = ((_a = styles[index]) === null || _a === void 0 ? void 0 : _a.textContent) || '';
        if (isEVirtTableStyleText(cssText)) {
            return cssText;
        }
    }
    return '';
};
const loadEVirtTable = async (documentRef) => {
    const documents = collectEVirtTableStyleDocuments(documentRef);
    const stylesBeforeImport = new Set(collectStyleElements(documents));
    const module = await import('e-virt-table/dist/index.es.js');
    const injectedStyles = collectStyleElements(documents).filter(style => (!stylesBeforeImport.has(style) && isEVirtTableStyleText(style.textContent || '')));
    const injectedStyle = injectedStyles[injectedStyles.length - 1];
    if (injectedStyle) {
        loadedEVirtTableStyleText = injectedStyle.textContent || '';
    }
    injectedStyles.forEach(style => style.remove());
    return {
        constructor: resolveEVirtTableConstructor(module),
        // The vendor entry injects CSS while its dynamic import is evaluated.
        // Prefer that newly-created node so an app's independently loaded table
        // version cannot be mistaken for the renderer-owned dependency.
        styleText: loadedEVirtTableStyleText || resolveEVirtTableStyleText(documentRef),
    };
};
export const scopeEVirtTableStyleText = (cssText, shadow) => {
    const rootScope = shadow
        ? `:host,${SPREADSHEET_STYLE_SCOPE}`
        : SPREADSHEET_STYLE_SCOPE;
    const darkScope = shadow
        ? `:host(.dark),${SPREADSHEET_STYLE_SCOPE}[data-spreadsheet-theme='dark']`
        : `${SPREADSHEET_STYLE_SCOPE}[data-spreadsheet-theme='dark']`;
    const scopeSelector = (selector) => {
        const normalized = selector.trim();
        if (normalized === ':root') {
            return rootScope;
        }
        if (normalized === '.dark') {
            return darkScope;
        }
        return `${SPREADSHEET_STYLE_SCOPE} ${normalized}`;
    };
    return cssText
        .replace(/@keyframes\s+rotate\b/g, `@keyframes ${E_VIRT_TABLE_ROTATE_KEYFRAME}`)
        .replace(/animation:\s*rotate\b/g, `animation:${E_VIRT_TABLE_ROTATE_KEYFRAME}`)
        .replace(/(^|})\s*(:root|\.dark|\.e-virt-table-[^{}]+)\{/g, (_match, boundary, selectorList) => {
        const scopedSelectors = selectorList
            .split(',')
            .map(scopeSelector)
            .join(',');
        return `${boundary}${scopedSelectors}{`;
    });
};
const getTargetWindow = (target) => {
    return target.ownerDocument.defaultView;
};
const getDocumentBaseUrl = (target) => {
    return resolveFileViewerRuntimeAssetBaseUrl(target.ownerDocument);
};
const callListener = (listener, event) => {
    if (!listener) {
        return;
    }
    if (typeof listener === 'function') {
        listener(event);
        return;
    }
    listener.handleEvent(event);
};
class MainThreadSpreadsheetWorker {
    constructor(targetWindow) {
        this.onmessage = null;
        this.onerror = null;
        this.destroyed = false;
        this.listeners = new Map();
        this.parserPromise = null;
        this.context = null;
        this.targetWindow = targetWindow;
    }
    addEventListener(type, listener) {
        var _a;
        if (!this.listeners.has(type)) {
            this.listeners.set(type, new Set());
        }
        (_a = this.listeners.get(type)) === null || _a === void 0 ? void 0 : _a.add(listener);
    }
    removeEventListener(type, listener) {
        var _a;
        (_a = this.listeners.get(type)) === null || _a === void 0 ? void 0 : _a.delete(listener);
    }
    terminate() {
        this.destroyed = true;
        this.listeners.clear();
    }
    postMessage(message) {
        void this.handleMessage(message);
    }
    async loadParser() {
        if (!this.parserPromise) {
            this.parserPromise = import('./spreadsheet/worker/sheetjs/parser.js');
        }
        const parser = await this.parserPromise;
        if (!this.context) {
            this.context = parser.createSpreadsheetParserContext();
        }
        return parser;
    }
    dispatch(type, event) {
        var _a;
        (_a = this.listeners.get(type)) === null || _a === void 0 ? void 0 : _a.forEach(listener => callListener(listener, event));
    }
    dispatchMessage(data) {
        var _a;
        const targetGlobal = this.targetWindow;
        const MessageEventCtor = (targetGlobal === null || targetGlobal === void 0 ? void 0 : targetGlobal.MessageEvent) ||
            (typeof MessageEvent !== 'undefined' ? MessageEvent : undefined);
        const event = MessageEventCtor
            ? new MessageEventCtor('message', { data })
            : { type: 'message', data };
        (_a = this.onmessage) === null || _a === void 0 ? void 0 : _a.call(this, event);
        this.dispatch('message', event);
    }
    dispatchError(error) {
        var _a;
        const message = error instanceof Error ? error.message : String(error);
        const targetGlobal = this.targetWindow;
        const ErrorEventCtor = (targetGlobal === null || targetGlobal === void 0 ? void 0 : targetGlobal.ErrorEvent) ||
            (typeof ErrorEvent !== 'undefined' ? ErrorEvent : undefined);
        const event = ErrorEventCtor
            ? new ErrorEventCtor('error', { message, error })
            : { type: 'error', message, error };
        (_a = this.onerror) === null || _a === void 0 ? void 0 : _a.call(this, event);
        this.dispatch('error', event);
    }
    async handleMessage(message) {
        if (this.destroyed) {
            return;
        }
        try {
            const parser = await this.loadParser();
            const responses = await parser.handleSpreadsheetWorkerRequest(this.context || parser.createSpreadsheetParserContext(), message);
            responses.forEach(response => {
                if (!this.destroyed) {
                    this.dispatchMessage(response);
                }
            });
        }
        catch (error) {
            if (!this.destroyed) {
                this.dispatchError(error);
            }
        }
    }
}
class AutoFallbackSpreadsheetWorker {
    constructor(primary, createFallback) {
        this.createFallback = createFallback;
        this.onmessage = null;
        this.onerror = null;
        this.destroyed = false;
        this.usingFallback = false;
        this.hasPrimaryMessage = false;
        this.pendingMessages = [];
        this.listeners = new Map();
        this.handleMessage = (event) => {
            var _a;
            if (this.destroyed) {
                return;
            }
            if (!this.usingFallback) {
                this.hasPrimaryMessage = true;
                this.pendingMessages.length = 0;
            }
            (_a = this.onmessage) === null || _a === void 0 ? void 0 : _a.call(this, event);
            this.dispatch('message', event);
        };
        this.handleError = (event) => {
            var _a;
            if (this.destroyed) {
                return;
            }
            if (!this.usingFallback && !this.hasPrimaryMessage) {
                this.switchToFallback(event);
                return;
            }
            (_a = this.onerror) === null || _a === void 0 ? void 0 : _a.call(this, event);
            this.dispatch('error', event);
        };
        this.active = primary;
        this.attach(primary);
    }
    addEventListener(type, listener) {
        var _a;
        if (!this.listeners.has(type)) {
            this.listeners.set(type, new Set());
        }
        (_a = this.listeners.get(type)) === null || _a === void 0 ? void 0 : _a.add(listener);
    }
    removeEventListener(type, listener) {
        var _a;
        (_a = this.listeners.get(type)) === null || _a === void 0 ? void 0 : _a.delete(listener);
    }
    terminate() {
        this.destroyed = true;
        this.detach(this.active);
        this.active.terminate();
        this.pendingMessages.length = 0;
        this.listeners.clear();
    }
    postMessage(message) {
        if (this.destroyed) {
            return;
        }
        if (!this.usingFallback && !this.hasPrimaryMessage) {
            this.pendingMessages.push(message);
        }
        this.active.postMessage(message);
    }
    attach(worker) {
        worker.addEventListener('message', this.handleMessage);
        worker.addEventListener('error', this.handleError);
    }
    detach(worker) {
        worker.removeEventListener('message', this.handleMessage);
        worker.removeEventListener('error', this.handleError);
    }
    dispatch(type, event) {
        var _a;
        (_a = this.listeners.get(type)) === null || _a === void 0 ? void 0 : _a.forEach(listener => callListener(listener, event));
    }
    switchToFallback(event) {
        var _a;
        const messages = this.pendingMessages.splice(0);
        this.detach(this.active);
        this.active.terminate();
        this.usingFallback = true;
        this.hasPrimaryMessage = false;
        try {
            this.active = this.createFallback();
            this.attach(this.active);
            console.warn('[file-viewer] Spreadsheet Worker 自动模式启动失败，已回退到主线程解析。', event.message || event.type);
            messages.forEach(message => this.active.postMessage(message));
        }
        catch (fallbackError) {
            const targetGlobal = typeof window !== 'undefined' ? window : undefined;
            const ErrorEventCtor = (targetGlobal === null || targetGlobal === void 0 ? void 0 : targetGlobal.ErrorEvent) ||
                (typeof ErrorEvent !== 'undefined' ? ErrorEvent : undefined);
            const message = fallbackError instanceof Error ? fallbackError.message : String(fallbackError);
            const nextEvent = ErrorEventCtor
                ? new ErrorEventCtor('error', { message, error: fallbackError })
                : { type: 'error', message, error: fallbackError };
            (_a = this.onerror) === null || _a === void 0 ? void 0 : _a.call(this, nextEvent);
            this.dispatch('error', nextEvent);
        }
    }
}
const createMainThreadSpreadsheetWorker = (target) => {
    return new MainThreadSpreadsheetWorker(getTargetWindow(target));
};
const getSpreadsheetWorkerMode = (context) => {
    var _a, _b, _c;
    return (_c = (_b = (_a = context === null || context === void 0 ? void 0 : context.options) === null || _a === void 0 ? void 0 : _a.spreadsheet) === null || _b === void 0 ? void 0 : _b.worker) !== null && _c !== void 0 ? _c : 'auto';
};
const shouldUseSpreadsheetWorker = (byteLength, context) => {
    var _a;
    const spreadsheetOptions = (_a = context === null || context === void 0 ? void 0 : context.options) === null || _a === void 0 ? void 0 : _a.spreadsheet;
    const workerMode = getSpreadsheetWorkerMode(context);
    if (workerMode === true) {
        return true;
    }
    if (workerMode === false) {
        return false;
    }
    const threshold = typeof (spreadsheetOptions === null || spreadsheetOptions === void 0 ? void 0 : spreadsheetOptions.workerAutoThreshold) === 'number' &&
        Number.isFinite(spreadsheetOptions.workerAutoThreshold) &&
        spreadsheetOptions.workerAutoThreshold >= 0
        ? spreadsheetOptions.workerAutoThreshold
        : DEFAULT_SPREADSHEET_WORKER_AUTO_THRESHOLD;
    return byteLength >= threshold;
};
const wrapAutoSpreadsheetWorker = (worker, target, context) => {
    if (getSpreadsheetWorkerMode(context) !== 'auto') {
        return worker;
    }
    return new AutoFallbackSpreadsheetWorker(worker, () => createMainThreadSpreadsheetWorker(target));
};
const createSpreadsheetWorkerFactory = (target, bufferByteLength, context) => {
    return () => {
        var _a;
        if (!shouldUseSpreadsheetWorker(bufferByteLength, context)) {
            return createMainThreadSpreadsheetWorker(target);
        }
        const view = getTargetWindow(target);
        const WorkerCtor = (view === null || view === void 0 ? void 0 : view.Worker) ||
            (typeof Worker !== 'undefined' ? Worker : undefined);
        if (!WorkerCtor) {
            return createMainThreadSpreadsheetWorker(target);
        }
        const workerUrl = resolveFileViewerSpreadsheetWorkerUrl((_a = context === null || context === void 0 ? void 0 : context.options) === null || _a === void 0 ? void 0 : _a.spreadsheet, getDocumentBaseUrl(target));
        try {
            return wrapAutoSpreadsheetWorker(new WorkerCtor(workerUrl, { type: 'module' }), target, context);
        }
        catch (moduleWorkerError) {
            try {
                return wrapAutoSpreadsheetWorker(new WorkerCtor(workerUrl), target, context);
            }
            catch (classicWorkerError) {
                console.warn('[file-viewer] Spreadsheet Worker 无法创建，已回退到主线程解析。', classicWorkerError || moduleWorkerError);
                return createMainThreadSpreadsheetWorker(target);
            }
        }
    };
};
const createStyle = (documentRef, cssText = scopedSpreadsheetStyle) => {
    const style = documentRef.createElement('style');
    style.textContent = cssText;
    return style;
};
const createEVirtTableStyle = (documentRef, target, cssText) => {
    var _a;
    if (!cssText) {
        throw new Error('Unable to resolve the e-virt-table stylesheet for the spreadsheet render surface.');
    }
    const rootNode = target.getRootNode();
    const ShadowRootCtor = (_a = target.ownerDocument.defaultView) === null || _a === void 0 ? void 0 : _a.ShadowRoot;
    const shadow = !!ShadowRootCtor && rootNode instanceof ShadowRootCtor;
    const style = createStyle(documentRef, scopeEVirtTableStyleText(cssText, shadow));
    style.dataset.fileViewerVendorStyle = 'e-virt-table';
    return style;
};
const setHidden = (element, hidden) => {
    element.classList.toggle('hidden', hidden);
};
const clampZoom = (value) => {
    return Math.min(SPREADSHEET_MAX_ZOOM, Math.max(SPREADSHEET_MIN_ZOOM, Number(value.toFixed(2))));
};
const serializeSpreadsheetCopyCell = (value) => {
    if (value === null || value === undefined) {
        return '';
    }
    const text = `${value}`;
    return /[\t\r\n"]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
};
const serializeSpreadsheetCopyData = (data) => {
    if (!Array.isArray(data)) {
        return serializeSpreadsheetCopyCell(data);
    }
    return data.map(row => {
        if (!Array.isArray(row)) {
            return serializeSpreadsheetCopyCell(row);
        }
        return row.map(serializeSpreadsheetCopyCell).join('\t');
    }).join('\n');
};
const copyTextWithTextareaFallback = (documentRef, text) => {
    var _a;
    const body = documentRef.body;
    if (!body || typeof documentRef.execCommand !== 'function') {
        return false;
    }
    const activeElement = documentRef.activeElement;
    const textarea = documentRef.createElement('textarea');
    textarea.value = text;
    textarea.setAttribute('readonly', 'true');
    textarea.setAttribute('aria-hidden', 'true');
    Object.assign(textarea.style, {
        position: 'fixed',
        top: '0',
        left: '0',
        width: '1px',
        height: '1px',
        padding: '0',
        border: '0',
        opacity: '0',
        pointerEvents: 'none',
    });
    body.appendChild(textarea);
    textarea.focus({ preventScroll: true });
    textarea.select();
    textarea.setSelectionRange(0, textarea.value.length);
    let copied = false;
    try {
        copied = documentRef.execCommand('copy');
    }
    finally {
        textarea.remove();
        (_a = activeElement === null || activeElement === void 0 ? void 0 : activeElement.focus) === null || _a === void 0 ? void 0 : _a.call(activeElement, { preventScroll: true });
    }
    return copied;
};
const writeSpreadsheetClipboard = async (documentRef, text) => {
    var _a;
    const targetWindow = documentRef.defaultView;
    const clipboard = (_a = targetWindow === null || targetWindow === void 0 ? void 0 : targetWindow.navigator) === null || _a === void 0 ? void 0 : _a.clipboard;
    const useAsyncClipboard = !!(targetWindow === null || targetWindow === void 0 ? void 0 : targetWindow.isSecureContext) && typeof (clipboard === null || clipboard === void 0 ? void 0 : clipboard.writeText) === 'function';
    if (useAsyncClipboard) {
        try {
            await clipboard.writeText(text);
            return true;
        }
        catch {
            return copyTextWithTextareaFallback(documentRef, text);
        }
    }
    return copyTextWithTextareaFallback(documentRef, text);
};
const renderFileViewerSpreadsheet = async (buffer, target, type, context) => {
    var _a, _b, _c, _d, _e, _f, _g, _h, _j, _k, _l;
    const documentRef = target.ownerDocument;
    const loadedEVirtTable = await loadEVirtTable(documentRef);
    const EVirtTable = loadedEVirtTable.constructor;
    const t = createFileViewerTranslator(context === null || context === void 0 ? void 0 : context.options);
    const zoomEmitter = createZoomChangeEmitter();
    const systemDark = (_c = (_b = (_a = documentRef.defaultView) === null || _a === void 0 ? void 0 : _a.matchMedia) === null || _b === void 0 ? void 0 : _b.call(_a, '(prefers-color-scheme: dark)').matches) !== null && _c !== void 0 ? _c : false;
    const darkMode = resolveFileViewerColorScheme((_d = context === null || context === void 0 ? void 0 : context.options) === null || _d === void 0 ? void 0 : _d.theme, systemDark) === 'dark';
    const root = documentRef.createElement('div');
    root.className = 'excel-wrapper';
    root.dataset.fileViewerSpreadsheetRoot = 'true';
    root.dataset.spreadsheetTheme = darkMode ? 'dark' : 'light';
    root.dataset.viewerZoomProvider = 'xlsx';
    const loading = documentRef.createElement('div');
    loading.className = 'loading';
    const loadingCard = documentRef.createElement('div');
    loadingCard.className = 'loading-card';
    const loadingBrand = documentRef.createElement('div');
    loadingBrand.className = 'loading-brand';
    loadingBrand.textContent = 'XLSX';
    const loadingCopy = documentRef.createElement('div');
    loadingCopy.className = 'loading-copy';
    const loadingKicker = documentRef.createElement('span');
    loadingKicker.className = 'loading-kicker';
    loadingKicker.textContent = t('spreadsheet.loading.kicker');
    const loadingTitle = documentRef.createElement('strong');
    loadingTitle.dataset.loadingTitle = 'true';
    loadingTitle.textContent = t('spreadsheet.loading.title');
    const loadingHint = documentRef.createElement('p');
    loadingHint.textContent = t('spreadsheet.loading.hint');
    loadingCopy.append(loadingKicker, loadingTitle, loadingHint);
    const loadingSpinner = documentRef.createElement('span');
    loadingSpinner.className = 'loading-spinner';
    loadingCard.append(loadingBrand, loadingCopy, loadingSpinner);
    loading.appendChild(loadingCard);
    const error = documentRef.createElement('div');
    error.className = 'error hidden';
    const tableWrapper = documentRef.createElement('div');
    tableWrapper.className = 'table-wrapper';
    const sheetLoading = documentRef.createElement('div');
    sheetLoading.className = 'sheet-loading hidden';
    const sheetLoadingDot = documentRef.createElement('span');
    sheetLoadingDot.className = 'sheet-loading-dot';
    const sheetLoadingText = documentRef.createElement('span');
    sheetLoadingText.textContent = t('spreadsheet.loading.streaming');
    const sheetLoadingSummary = documentRef.createElement('span');
    sheetLoadingSummary.className = 'sheet-loading-summary';
    sheetLoading.append(sheetLoadingDot, sheetLoadingText, sheetLoadingSummary);
    const tableHostShell = documentRef.createElement('div');
    tableHostShell.className = 'table-host';
    const tableHost = documentRef.createElement('div');
    tableHost.className = 'table-target';
    const imageViewport = documentRef.createElement('div');
    imageViewport.className = 'excel-image-viewport hidden';
    const imageLayer = documentRef.createElement('div');
    imageLayer.className = 'excel-image-layer';
    imageViewport.appendChild(imageLayer);
    tableHostShell.append(tableHost, imageViewport);
    tableWrapper.append(sheetLoading, tableHostShell);
    const imageLightbox = documentRef.createElement('div');
    imageLightbox.className = 'excel-image-lightbox';
    imageLightbox.dataset.open = 'false';
    imageLightbox.setAttribute('role', 'dialog');
    imageLightbox.setAttribute('aria-modal', 'true');
    imageLightbox.setAttribute('aria-hidden', 'true');
    const lightboxImage = documentRef.createElement('img');
    lightboxImage.alt = t('image.lightbox.alt');
    lightboxImage.draggable = false;
    const lightboxCloseButton = documentRef.createElement('button');
    lightboxCloseButton.type = 'button';
    lightboxCloseButton.setAttribute('aria-label', t('image.lightbox.close'));
    lightboxCloseButton.textContent = '×';
    imageLightbox.append(lightboxImage, lightboxCloseButton);
    const toolbar = documentRef.createElement('div');
    toolbar.className = 'toolbar';
    const sheetTabsBar = documentRef.createElement('div');
    sheetTabsBar.className = 'btn-group';
    sheetTabsBar.setAttribute('aria-label', t('spreadsheet.tabs.ariaLabel'));
    const summary = documentRef.createElement('div');
    summary.className = 'summary';
    toolbar.append(sheetTabsBar, summary);
    root.append(loading, error, tableWrapper, toolbar, imageLightbox);
    target.replaceChildren(createEVirtTableStyle(documentRef, target, loadedEVirtTable.styleText), createStyle(documentRef), root);
    let sheets = [];
    let sheetIndex = 0;
    let errorMessage = '';
    let totalRows = 0;
    let totalCols = 0;
    let sheetDefaults = { ...DEFAULT_SHEET_DEFAULTS };
    let sheetInitializing = true;
    let hasInitialWindow = false;
    let loadedWindowCount = 0;
    let loadingWindowCount = 0;
    let sheetImages = [];
    let sheetCharts = [];
    let zoom = 1;
    let imageViewportState = {
        scrollX: 0,
        scrollY: 0,
        width: 0,
        height: 0,
    };
    let loadingState = true;
    let virtualState = createEmptyVirtualState();
    const sheetStateCache = new Map();
    const sheetImageCache = new Map();
    const sheetChartCache = new Map();
    let table = null;
    let resizeObserver = null;
    let resizeFrame = 0;
    let scrollFrame = 0;
    let layoutRefreshToken = 0;
    const layoutRefreshTimers = [];
    let viewportRange = { start: 0, end: 0 };
    let scrollDirection = 1;
    let lastScrollY = 0;
    let sheetSessionId = 0;
    let disposed = false;
    const imageSourceResolver = createSpreadsheetImageSourceResolver(documentRef);
    let imageLightboxPreviousFocus = null;
    let hasNotifiedFirstPaint = false;
    let hasAppliedDefaultInitialFit = false;
    const resizableColumns = ((_f = (_e = context === null || context === void 0 ? void 0 : context.options) === null || _e === void 0 ? void 0 : _e.spreadsheet) === null || _f === void 0 ? void 0 : _f.resizableColumns) === true;
    const resizableRows = ((_h = (_g = context === null || context === void 0 ? void 0 : context.options) === null || _g === void 0 ? void 0 : _g.spreadsheet) === null || _h === void 0 ? void 0 : _h.resizableRows) === true;
    const controller = createFileViewerWorkerController(createSpreadsheetWorkerFactory(target, buffer.byteLength, context), { logErrors: false });
    const getActiveSheet = () => sheets.find(sheet => sheet.id === sheetIndex);
    const getSheetTabs = () => {
        const visible = sheets.filter(sheet => !sheet.hidden);
        return visible.length ? visible : sheets;
    };
    const getActiveSheetId = () => { var _a; return sheetIndex !== null && sheetIndex !== void 0 ? sheetIndex : (_a = sheets[0]) === null || _a === void 0 ? void 0 : _a.id; };
    const getHostHeight = () => tableHost.clientHeight || 0;
    const showBlockingLoading = () => !errorMessage && !hasInitialWindow && (loadingState || sheetInitializing);
    const showStreamingLoading = () => !showBlockingLoading() &&
        !errorMessage &&
        hasInitialWindow &&
        loadingWindowCount > 0;
    const scalePx = (value) => Math.max(1, Math.round(value * zoom));
    const scaleRowHeight = (value) => Math.max(0.1, Math.round(value * zoom));
    const closeImageLightbox = () => {
        if (imageLightbox.dataset.open !== 'true') {
            return;
        }
        imageLightbox.dataset.open = 'false';
        imageLightbox.setAttribute('aria-hidden', 'true');
        delete imageLightbox.dataset.imageId;
        if (imageLightboxPreviousFocus === null || imageLightboxPreviousFocus === void 0 ? void 0 : imageLightboxPreviousFocus.isConnected) {
            imageLightboxPreviousFocus.focus({ preventScroll: true });
        }
        imageLightboxPreviousFocus = null;
    };
    const openImageLightbox = (image) => {
        imageLightboxPreviousFocus = documentRef.activeElement instanceof HTMLElement
            ? documentRef.activeElement
            : null;
        lightboxImage.src = image.src;
        void imageSourceResolver.resolve(image).then((source) => {
            if (!disposed && imageLightbox.dataset.imageId === image.id) {
                lightboxImage.src = source;
            }
        });
        imageLightbox.dataset.imageId = image.id;
        imageLightbox.dataset.open = 'true';
        imageLightbox.setAttribute('aria-hidden', 'false');
        lightboxCloseButton.focus({ preventScroll: true });
    };
    const containsViewportPoint = (item, x, y) => {
        const left = scalePx(item.left) - imageViewportState.scrollX;
        const top = scalePx(item.top) - imageViewportState.scrollY;
        const right = left + scalePx(item.width);
        const bottom = top + scalePx(item.height);
        return x >= left && x <= right && y >= top && y <= bottom;
    };
    const findImageAtViewportPoint = (clientX, clientY) => {
        const viewportRect = imageViewport.getBoundingClientRect();
        const x = clientX - viewportRect.left;
        const y = clientY - viewportRect.top;
        if (x < 0 || y < 0 || x > viewportRect.width || y > viewportRect.height) {
            return undefined;
        }
        // Charts are appended after images and therefore paint above them. Do not
        // open an image hidden under a chart when their saved bounds overlap.
        if ([...sheetCharts].reverse().some(chart => containsViewportPoint(chart, x, y))) {
            return undefined;
        }
        return [...sheetImages].reverse().find(image => containsViewportPoint(image, x, y));
    };
    const handleImageDoubleClick = (event) => {
        const image = findImageAtViewportPoint(event.clientX, event.clientY);
        if (!image) {
            return;
        }
        event.preventDefault();
        event.stopPropagation();
        openImageLightbox(image);
    };
    const handleImageLightboxClick = (event) => {
        if (event.target === imageLightbox) {
            closeImageLightbox();
        }
    };
    const handleImageLightboxKeyDown = (event) => {
        if (event.key === 'Escape' && imageLightbox.dataset.open === 'true') {
            event.preventDefault();
            closeImageLightbox();
        }
    };
    tableHostShell.addEventListener('dblclick', handleImageDoubleClick, true);
    imageLightbox.addEventListener('click', handleImageLightboxClick);
    lightboxCloseButton.addEventListener('click', closeImageLightbox);
    documentRef.addEventListener('keydown', handleImageLightboxKeyDown);
    const getSheetLoadingText = () => {
        var _a;
        if (!sheets.length) {
            return t('spreadsheet.state.parsingWorkbook');
        }
        const activeName = (_a = getActiveSheet()) === null || _a === void 0 ? void 0 : _a.name;
        return activeName
            ? t('spreadsheet.state.preparingSheetNamed', { name: activeName })
            : t('spreadsheet.state.preparingSheet');
    };
    const getCachedSummary = () => {
        if (!totalRows) {
            return '';
        }
        const cachedRows = Math.min(loadedWindowCount * WINDOW_SIZE, totalRows);
        return t('spreadsheet.state.cachedRows', {
            cached: cachedRows.toLocaleString(),
            total: totalRows.toLocaleString(),
        });
    };
    const getStatusSummary = () => {
        var _a, _b;
        const rows = totalRows || ((_a = getActiveSheet()) === null || _a === void 0 ? void 0 : _a.rowCount) || 0;
        const cols = totalCols || ((_b = getActiveSheet()) === null || _b === void 0 ? void 0 : _b.colCount) || 0;
        if (!rows) {
            return '';
        }
        if (!cols) {
            return t('spreadsheet.state.rows', { rows: rows.toLocaleString() });
        }
        return t('spreadsheet.state.rowsAndColumns', {
            rows: rows.toLocaleString(),
            cols: cols.toLocaleString(),
        });
    };
    const getZoomState = () => ({
        scale: zoom,
        label: `${Math.round(zoom * 100)}%`,
        canZoomIn: zoom < SPREADSHEET_MAX_ZOOM,
        canZoomOut: zoom > SPREADSHEET_MIN_ZOOM,
        canReset: zoom !== 1,
        minScale: SPREADSHEET_MIN_ZOOM,
        maxScale: SPREADSHEET_MAX_ZOOM,
    });
    const getImageViewportScrollbarGuard = () => {
        const tableContainer = tableHost.querySelector('.e-virt-table-container');
        const vertical = tableContainer
            ? Math.max(tableContainer.offsetWidth - tableContainer.clientWidth, 0)
            : 0;
        const horizontal = tableContainer
            ? Math.max(tableContainer.offsetHeight - tableContainer.clientHeight, 0)
            : 0;
        // e-virt-table may draw overlay scrollbars, so keep a small reserved lane
        // even when native scrollbar metrics report zero.
        return {
            vertical: vertical || EXCEL_IMAGE_SCROLLBAR_GUARD,
            horizontal: horizontal || EXCEL_IMAGE_SCROLLBAR_GUARD,
        };
    };
    const renderImages = () => {
        const margin = 240;
        const guard = getImageViewportScrollbarGuard();
        const width = Math.max(imageViewportState.width - scalePx(INDEX_COLUMN_WIDTH) - guard.vertical, 0);
        const height = Math.max(imageViewportState.height - scalePx(HEADER_HEIGHT) - guard.horizontal, 0);
        const visibleImages = sheetImages.filter((image) => {
            const x = scalePx(image.left) - imageViewportState.scrollX;
            const y = scalePx(image.top) - imageViewportState.scrollY;
            return x + scalePx(image.width) >= -margin &&
                x <= width + margin &&
                y + scalePx(image.height) >= -margin &&
                y <= height + margin;
        });
        const visibleCharts = sheetCharts.filter((chart) => {
            const x = scalePx(chart.left) - imageViewportState.scrollX;
            const y = scalePx(chart.top) - imageViewportState.scrollY;
            return x + scalePx(chart.width) >= -margin &&
                x <= width + margin &&
                y + scalePx(chart.height) >= -margin &&
                y <= height + margin;
        });
        setHidden(imageViewport, visibleImages.length === 0 && visibleCharts.length === 0);
        Object.assign(imageViewport.style, {
            left: `${scalePx(INDEX_COLUMN_WIDTH)}px`,
            top: `${scalePx(HEADER_HEIGHT)}px`,
            right: 'auto',
            bottom: 'auto',
            width: `${width}px`,
            height: `${height}px`,
        });
        imageLayer.style.transform =
            `translate(${-imageViewportState.scrollX}px, ${-imageViewportState.scrollY}px)`;
        const imageElements = visibleImages.map((image, index) => {
            const element = documentRef.createElement('img');
            element.className = 'excel-image';
            element.src = image.src;
            element.alt = image.id;
            element.draggable = false;
            Object.assign(element.style, {
                left: `${scalePx(image.left)}px`,
                top: `${scalePx(image.top)}px`,
                width: `${scalePx(image.width)}px`,
                height: `${scalePx(image.height)}px`,
            });
            element.dataset.imageIndex = `${index}`;
            void imageSourceResolver.resolve(image).then((source) => {
                if (!disposed && element.isConnected && element.src !== source) {
                    element.src = source;
                }
            });
            return element;
        });
        const chartElements = visibleCharts.map((chart) => {
            const element = renderSpreadsheetChart(documentRef, chart);
            Object.assign(element.style, {
                left: `${scalePx(chart.left)}px`,
                top: `${scalePx(chart.top)}px`,
                width: `${scalePx(chart.width)}px`,
                height: `${scalePx(chart.height)}px`,
            });
            return element;
        });
        imageLayer.replaceChildren(...imageElements, ...chartElements);
    };
    const scrollActiveSheetIntoView = () => {
        requestAnimationFrame(() => {
            var _a;
            (_a = sheetTabsBar.querySelector('.sheet-tab.active')) === null || _a === void 0 ? void 0 : _a.scrollIntoView({
                block: 'nearest',
                inline: 'center',
                behavior: 'smooth',
            });
        });
    };
    const renderChrome = () => {
        setHidden(loading, !showBlockingLoading());
        const loadingTitle = loading.querySelector('[data-loading-title]');
        if (loadingTitle) {
            loadingTitle.textContent = getSheetLoadingText();
        }
        error.textContent = errorMessage;
        setHidden(error, !errorMessage);
        setHidden(sheetLoading, !showStreamingLoading());
        const cacheText = sheetLoading.querySelector('.sheet-loading-summary');
        if (cacheText) {
            cacheText.textContent = getCachedSummary();
        }
        summary.textContent = getStatusSummary();
        sheetTabsBar.replaceChildren(...getSheetTabs().map(sheet => {
            const button = documentRef.createElement('button');
            button.type = 'button';
            button.className = `sheet-tab${sheetIndex === sheet.id ? ' active' : ''}`;
            button.title = sheet.name;
            button.textContent = sheet.name;
            button.setAttribute('aria-pressed', sheetIndex === sheet.id ? 'true' : 'false');
            button.addEventListener('click', () => handleSheet(sheet.id));
            return button;
        }));
        renderImages();
    };
    const setLoading = (value) => {
        loadingState = value;
        renderChrome();
    };
    const emitWorker = (type, payload) => {
        setLoading(true);
        controller.emit(type, payload);
    };
    const applyRowHeight = (row, baseHeight) => {
        row.__baseHeight = baseHeight;
        row._height = scaleRowHeight(baseHeight);
    };
    const syncScaledRowHeights = () => {
        virtualState.rowHeightCache.forEach((height, rowIndex) => {
            const row = virtualState.rows[rowIndex];
            if (row) {
                applyRowHeight(row, height);
            }
        });
    };
    const setZoom = (scale) => {
        zoom = clampZoom(scale);
        syncScaledRowHeights();
        syncTableLayout();
        zoomEmitter.emit();
        renderChrome();
        return getZoomState();
    };
    const getSpreadsheetContentSize = () => {
        const contentWidth = virtualState.columns.reduce((sum, column) => {
            const candidate = column;
            if (candidate.hide) {
                return sum;
            }
            const width = Number(candidate.width);
            return sum + (Number.isFinite(width) && width > 0 ? width : sheetDefaults.colWidth);
        }, 0);
        const defaultRowHeight = normalizeRowHeight(sheetDefaults.rowHeight, DEFAULT_SHEET_DEFAULTS.rowHeight);
        let explicitHeight = 0;
        let explicitRows = 0;
        virtualState.rowHeightCache.forEach((height) => {
            explicitHeight += normalizeRowHeight(height, defaultRowHeight);
            explicitRows += 1;
        });
        const contentHeight = HEADER_HEIGHT + explicitHeight +
            Math.max(0, virtualState.totalRows - explicitRows) * defaultRowHeight;
        return { contentWidth, contentHeight };
    };
    const resolveSpreadsheetScale = (request) => {
        var _a, _b;
        const content = getSpreadsheetContentSize();
        const autoMode = request.mode === 'auto';
        const viewportWidth = request.viewportWidth || tableHost.clientWidth || 0;
        const viewportHeight = request.viewportHeight || tableHost.clientHeight || 0;
        if (viewportWidth <= 0 || viewportHeight <= 0) {
            return undefined;
        }
        return resolveFileViewerFitScale({
            mode: autoMode ? 'width' : request.mode,
            viewportWidth,
            viewportHeight,
            contentWidth: content.contentWidth,
            contentHeight: content.contentHeight,
            currentScale: zoom,
            minScale: (_a = request.minScale) !== null && _a !== void 0 ? _a : SPREADSHEET_MIN_ZOOM,
            maxScale: (_b = request.maxScale) !== null && _b !== void 0 ? _b : (autoMode ? 1 : SPREADSHEET_MAX_ZOOM),
        });
    };
    const fitSpreadsheet = (request) => {
        if (!virtualState.active || !virtualState.columns.length) {
            return {
                applied: false,
                mode: request.mode,
                resize: request.resize,
                source: request.source,
                reason: 'pending',
                provider: 'zoom',
            };
        }
        const scale = resolveSpreadsheetScale(request);
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
    const applyDefaultInitialFit = () => {
        /* Riot 定制：宿主没传 fit 时原本会做一次 mode:'auto' 的初始缩放，
           把几十列的宽表整体缩进容器 —— 列挤成竖条完全没法读。表格的
           正确形态是原始尺寸 + 横向滚动（Excel 本来的浏览方式），
           默认初始缩放整个禁用；显式 fit / initialViewState 仍然生效。 */
        hasAppliedDefaultInitialFit = true;
    };
    registerFileViewerZoomProvider(root, {
        zoomIn: () => setZoom(zoom + 0.1),
        zoomOut: () => setZoom(zoom - 0.1),
        resetZoom: () => setZoom(1),
        setZoom,
        fit: fitSpreadsheet,
        getState: getZoomState,
        subscribe: zoomEmitter.subscribe,
    });
    const syncWindowStats = () => {
        loadedWindowCount = virtualState.loadedWindows.size;
        loadingWindowCount = virtualState.loadingWindows.size;
        renderChrome();
    };
    const syncImageViewport = () => {
        imageViewportState = {
            scrollX: (table === null || table === void 0 ? void 0 : table.ctx.scrollX) || 0,
            scrollY: (table === null || table === void 0 ? void 0 : table.ctx.scrollY) || 0,
            width: tableHost.clientWidth || 0,
            height: tableHost.clientHeight || 0,
        };
        renderImages();
    };
    const markCopiedSelection = (params) => {
        var _a, _b, _c, _d;
        const selector = table === null || table === void 0 ? void 0 : table.ctx.selector;
        if (!selector) {
            return;
        }
        selector.xArrCopy = params.xArr.slice();
        selector.yArrCopy = params.yArr.slice();
        (_b = table === null || table === void 0 ? void 0 : (_a = table.ctx).emit) === null || _b === void 0 ? void 0 : _b.call(_a, 'copyChange', {
            xArr: selector.xArrCopy,
            yArr: selector.yArrCopy,
            data: params.data,
        });
        (_d = table === null || table === void 0 ? void 0 : (_c = table.ctx).emit) === null || _d === void 0 ? void 0 : _d.call(_c, 'draw');
    };
    const copySpreadsheetSelection = (params) => {
        const text = serializeSpreadsheetCopyData(params.data);
        void writeSpreadsheetClipboard(documentRef, text).then((copied) => {
            if (copied) {
                markCopiedSelection(params);
                return;
            }
            console.error('Spreadsheet copy failed: clipboard fallback returned false.');
        }).catch((error) => {
            console.error('Spreadsheet copy failed:', error);
        });
    };
    const buildTableView = () => ({
        config: createTableConfig({
            hostHeight: getHostHeight(),
            darkMode,
            resizableColumns,
            resizableRows,
            copySelection: copySpreadsheetSelection,
            sheetDefaults,
            virtualState,
            zoomScale: zoom,
        }),
        columns: getDisplayColumns(virtualState.columns, zoom),
    });
    const resetViewportTracking = () => {
        viewportRange = { start: 0, end: 0 };
        scrollDirection = 1;
        lastScrollY = 0;
    };
    const ensureViewportWindows = (startRow, endRow) => {
        if (!virtualState.active || !virtualState.totalRows) {
            return;
        }
        collectWindowStarts({
            startRow,
            endRow,
            direction: scrollDirection,
            totalRows: virtualState.totalRows,
        }).forEach(windowStart => requestWindow(windowStart, true));
    };
    const scheduleViewportLoad = () => {
        if (!table || disposed) {
            return;
        }
        if (scrollFrame) {
            cancelAnimationFrame(scrollFrame);
        }
        scrollFrame = requestAnimationFrame(() => {
            scrollFrame = 0;
            if (!table || !virtualState.active || !virtualState.totalRows || disposed) {
                return;
            }
            const head = Math.max(table.ctx.body.headIndex || 0, 0);
            const tail = Math.max(table.ctx.body.tailIndex || head, head);
            const scrollY = table.ctx.scrollY || 0;
            scrollDirection = scrollY >= lastScrollY ? 1 : -1;
            lastScrollY = scrollY;
            viewportRange = { start: head, end: tail };
            syncImageViewport();
            ensureViewportWindows(head, tail);
        });
    };
    const ensureTable = () => {
        if (table) {
            return table;
        }
        table = new EVirtTable(tableHost, {
            data: [],
            columns: [],
            config: createTableConfig({
                hostHeight: getHostHeight(),
                darkMode,
                resizableColumns,
                resizableRows,
                copySelection: copySpreadsheetSelection,
                sheetDefaults,
                virtualState,
                zoomScale: zoom,
            }),
        });
        enableEVirtTableShadowEventTargeting(table.ctx);
        table.on('onScrollX', scheduleViewportLoad);
        table.on('onScrollY', scheduleViewportLoad);
        table.on('resize', scheduleViewportLoad);
        table.on('resizeColumnChange', handleColumnResizeChange);
        table.on('resizeRowChange', handleRowResizeChange);
        return table;
    };
    const renderTable = (instance, columns = virtualState.columns, rows = virtualState.rows, resetScroll = false) => {
        const view = {
            config: createTableConfig({
                hostHeight: getHostHeight(),
                darkMode,
                resizableColumns,
                resizableRows,
                copySelection: copySpreadsheetSelection,
                sheetDefaults,
                virtualState,
                zoomScale: zoom,
            }),
            columns: getDisplayColumns(columns, zoom),
        };
        instance.loadConfig(view.config);
        instance.loadColumns(view.columns);
        instance.loadData(rows);
        instance.draw();
        syncImageViewport();
        if (resetScroll) {
            requestAnimationFrame(() => {
                instance.scrollTo(0, 0);
                instance.draw();
                syncImageViewport();
                scheduleViewportLoad();
            });
            return;
        }
        scheduleViewportLoad();
    };
    function syncTableLayout() {
        const instance = ensureTable();
        const { config, columns } = buildTableView();
        instance.loadConfig(config);
        if (virtualState.active && columns.length) {
            instance.loadColumns(columns);
        }
        instance.doLayout();
        instance.draw();
        syncImageViewport();
        scheduleViewportLoad();
    }
    const clearScheduledLayoutRefresh = () => {
        layoutRefreshToken += 1;
        const view = getTargetWindow(target);
        while (layoutRefreshTimers.length) {
            const timer = layoutRefreshTimers.pop();
            if (timer !== undefined) {
                view === null || view === void 0 ? void 0 : view.clearTimeout(timer);
            }
        }
    };
    const refreshTableLayoutWhenVisible = () => {
        if (disposed || !table || !virtualState.active) {
            return;
        }
        const rect = tableHost.getBoundingClientRect();
        if (rect.width <= 0 || rect.height <= 0) {
            return;
        }
        applyDefaultInitialFit();
        syncTableLayout();
    };
    const scheduleStableFirstPaintRefresh = () => {
        var _a;
        clearScheduledLayoutRefresh();
        const token = layoutRefreshToken;
        const view = getTargetWindow(target);
        if (!view) {
            return;
        }
        const delays = [0, 32, 120, 300, 700];
        delays.forEach((delay) => {
            const timer = view.setTimeout(() => {
                const index = layoutRefreshTimers.indexOf(timer);
                if (index >= 0) {
                    layoutRefreshTimers.splice(index, 1);
                }
                view.requestAnimationFrame(() => {
                    if (token !== layoutRefreshToken) {
                        return;
                    }
                    refreshTableLayoutWhenVisible();
                });
            }, delay);
            layoutRefreshTimers.push(timer);
        });
        void ((_a = documentRef.fonts) === null || _a === void 0 ? void 0 : _a.ready.then(() => {
            if (token !== layoutRefreshToken) {
                return;
            }
            refreshTableLayoutWhenVisible();
        }).catch(() => {
            // Font readiness is only a paint stabilizer; rendering already has a fallback.
        }));
    };
    function setColumnWidthByKey(columns, key, width) {
        for (const column of columns) {
            if (`${column.key}` === key) {
                column.width = width;
                return true;
            }
            if (Array.isArray(column.children) && setColumnWidthByKey(column.children, key, width)) {
                return true;
            }
        }
        return false;
    }
    function clearTableResizableHeaderCache() {
        var _a;
        try {
            (_a = table === null || table === void 0 ? void 0 : table.setCustomHeader) === null || _a === void 0 ? void 0 : _a.call(table, { resizableData: {} }, true);
        }
        catch {
            // 某些旧版 e-virt-table 类型没有公开 setCustomHeader，忽略即可，下一次 loadColumns 仍会使用 core 状态。
        }
    }
    function handleColumnResizeChange(event) {
        if (!resizableColumns || disposed) {
            return;
        }
        const key = (event === null || event === void 0 ? void 0 : event.key) === undefined ? '' : `${event.key}`;
        const displayWidth = Number(event === null || event === void 0 ? void 0 : event.width);
        if (!key || key === INDEX_COLUMN_KEY || !Number.isFinite(displayWidth) || displayWidth <= 0) {
            return;
        }
        const baseWidth = Math.max(1, Math.round(displayWidth / Math.max(zoom, 0.01)));
        const changed = setColumnWidthByKey(virtualState.columns, key, Math.max(baseWidth, Math.round(RESIZABLE_COLUMN_MIN_WIDTH / Math.max(zoom, 1))));
        if (!changed) {
            return;
        }
        const activeSheetId = getActiveSheetId();
        if (activeSheetId !== undefined) {
            sheetStateCache.set(activeSheetId, virtualState);
        }
        clearTableResizableHeaderCache();
        syncTableLayout();
    }
    function handleRowResizeChange(event) {
        if (!resizableRows || disposed) {
            return;
        }
        const rowIndex = Number(event === null || event === void 0 ? void 0 : event.rowIndex);
        const displayHeight = Number(event === null || event === void 0 ? void 0 : event.height);
        if (!Number.isInteger(rowIndex) || rowIndex < 0 || !Number.isFinite(displayHeight) || displayHeight <= 0) {
            return;
        }
        const row = virtualState.rows[rowIndex];
        if (!row) {
            return;
        }
        const baseHeight = normalizeRowHeight(Math.max(RESIZABLE_ROW_MIN_HEIGHT, Math.round(displayHeight / Math.max(zoom, 0.01))), virtualState.defaults.rowHeight);
        applyRowHeight(row, baseHeight);
        virtualState.rowHeightCache.set(rowIndex, baseHeight);
        const activeSheetId = getActiveSheetId();
        if (activeSheetId !== undefined) {
            sheetStateCache.set(activeSheetId, virtualState);
        }
        syncTableLayout();
    }
    function requestWindow(startRow = 0, silent = true) {
        const sheetId = getActiveSheetId();
        if (sheetId === undefined) {
            return;
        }
        const windowStart = clampWindowStart(startRow, virtualState.totalRows);
        if (virtualState.loadedWindows.has(windowStart) || virtualState.loadingWindows.has(windowStart)) {
            return;
        }
        virtualState.loadingWindows.add(windowStart);
        syncWindowStats();
        if (virtualState.active) {
            markWindowState(virtualState.rows, virtualState.totalRows, windowStart, RowState.Loading);
            table === null || table === void 0 ? void 0 : table.draw();
        }
        errorMessage = '';
        emitWorker('parseSheet', {
            sheet: sheetId,
            startRow: windowStart,
            pageSize: WINDOW_SIZE,
            sessionId: sheetSessionId,
        });
        if (silent) {
            loadingState = false;
            renderChrome();
        }
    }
    const initializeVirtualSheet = (ws) => {
        const meta = ws.meta;
        if (!meta) {
            return;
        }
        const { columns, dataKeys } = buildColumns(ws);
        virtualState = {
            ...createEmptyVirtualState(),
            active: true,
            totalRows: meta.totalRows,
            totalCols: meta.totalCols,
            indexOffset: detectIndexOffset(ws),
            defaults: ws.defaults,
            dataKeys,
            rows: buildRows(meta.totalRows),
            columns,
        };
        sheetDefaults = ws.defaults;
        totalRows = meta.totalRows;
        totalCols = meta.totalCols;
        syncWindowStats();
    };
    const clearVirtualRow = (row) => {
        virtualState.dataKeys.forEach((key) => {
            delete row[key];
        });
    };
    const applyStructureRowHeights = (rowHeights) => {
        if (!Array.isArray(rowHeights)) {
            return;
        }
        rowHeights.forEach((rawHeight, absoluteRow) => {
            if (rawHeight === undefined) {
                return;
            }
            const row = virtualState.rows[absoluteRow];
            if (!row) {
                return;
            }
            const height = normalizeRowHeight(rawHeight, virtualState.defaults.rowHeight);
            applyRowHeight(row, height);
            virtualState.rowHeightCache.set(absoluteRow, height);
        });
    };
    const applyWindowRows = (ws) => {
        var _a, _b;
        const meta = ws.meta;
        if (!meta) {
            return;
        }
        const rowIndexes = [];
        const endRow = Math.min(meta.endRow, virtualState.totalRows);
        for (let absoluteRow = meta.startRow; absoluteRow < endRow; absoluteRow += 1) {
            const row = virtualState.rows[absoluteRow];
            const relativeRow = absoluteRow - meta.startRow;
            if (!row) {
                continue;
            }
            clearVirtualRow(row);
            const data = ((_a = ws.data) === null || _a === void 0 ? void 0 : _a[relativeRow]) || [];
            data.forEach((value, colIndex) => {
                if (value === '' || value === null || value === undefined) {
                    return;
                }
                row[getDataKey(colIndex)] = value;
            });
            const windowHeight = getRowHeight(ws.rowHeights, relativeRow, virtualState.defaults.rowHeight);
            const height = normalizeRowHeight(getRowHeight((_b = ws.structure) === null || _b === void 0 ? void 0 : _b.rowHeights, absoluteRow, windowHeight), virtualState.defaults.rowHeight);
            applyRowHeight(row, height);
            row[ROW_STATE_FIELD] = RowState.Loaded;
            virtualState.rowHeightCache.set(absoluteRow, height);
            rowIndexes.push(absoluteRow);
        }
        virtualState.windowRows.set(meta.startRow, rowIndexes);
    };
    const applyWindowCells = (ws) => {
        const meta = ws.meta;
        if (!meta) {
            return;
        }
        const keys = [];
        Object.entries(ws.cell || {}).forEach(([key, value]) => {
            const [row, col] = key.split('-').map(Number);
            const absoluteKey = displayCellKey(meta.startRow + row, col + 1);
            const style = normalizeCellStyle(value);
            if (!style) {
                return;
            }
            virtualState.cellCache.set(absoluteKey, style);
            keys.push(absoluteKey);
        });
        virtualState.windowCells.set(meta.startRow, keys);
    };
    const setSheetMerges = (merges) => {
        virtualState.mergeStartMap.clear();
        virtualState.mergeCoveredMap.clear();
        merges.forEach((merge) => {
            const startKey = displayCellKey(merge.row, merge.col + 1);
            virtualState.mergeStartMap.set(startKey, {
                ...merge,
                col: merge.col + 1,
            });
            for (let rowOffset = 0; rowOffset < merge.rowspan; rowOffset += 1) {
                for (let colOffset = 0; colOffset < merge.colspan; colOffset += 1) {
                    if (rowOffset === 0 && colOffset === 0) {
                        continue;
                    }
                    const coveredKey = displayCellKey(merge.row + rowOffset, merge.col + colOffset + 1);
                    virtualState.mergeCoveredMap.set(coveredKey, true);
                }
            }
        });
    };
    const applySheetStructure = (ws) => {
        const structure = ws.structure;
        const mergeList = structure === null || structure === void 0 ? void 0 : structure.merge;
        if (mergeList) {
            setSheetMerges(mergeList);
        }
        else {
            const meta = ws.meta;
            if (meta && !virtualState.mergeStartMap.size) {
                setSheetMerges((ws.merge || []).map((merge) => ({
                    ...merge,
                    row: merge.row + meta.startRow,
                })));
            }
        }
        applyStructureRowHeights(structure === null || structure === void 0 ? void 0 : structure.rowHeights);
        if (structure === null || structure === void 0 ? void 0 : structure.images) {
            sheetImages = structure.images;
            const sheetId = getActiveSheetId();
            if (sheetId !== undefined) {
                sheetImageCache.set(sheetId, structure.images);
            }
        }
        if (structure === null || structure === void 0 ? void 0 : structure.charts) {
            sheetCharts = structure.charts;
            const sheetId = getActiveSheetId();
            if (sheetId !== undefined) {
                sheetChartCache.set(sheetId, structure.charts);
            }
        }
    };
    const applyVirtualWindow = (ws) => {
        var _a;
        const meta = ws.meta;
        if (!meta) {
            return;
        }
        const isFirstWindow = !hasInitialWindow;
        if (!virtualState.active) {
            initializeVirtualSheet(ws);
        }
        applySheetStructure(ws);
        applyWindowRows(ws);
        applyWindowCells(ws);
        applyDefaultInitialFit();
        virtualState.loadedWindows.add(meta.startRow);
        virtualState.loadingWindows.delete(meta.startRow);
        syncWindowStats();
        hasInitialWindow = true;
        const activeSheetId = getActiveSheetId();
        if (activeSheetId !== undefined) {
            sheetStateCache.set(activeSheetId, virtualState);
        }
        if (isFirstWindow) {
            renderTable(ensureTable(), virtualState.columns, virtualState.rows, true);
        }
        else {
            table === null || table === void 0 ? void 0 : table.draw();
        }
        loadingState = false;
        sheetInitializing = false;
        renderChrome();
        if (!hasNotifiedFirstPaint) {
            hasNotifiedFirstPaint = true;
            (_a = context === null || context === void 0 ? void 0 : context.onProgressiveRender) === null || _a === void 0 ? void 0 : _a.call(context);
        }
        if (isFirstWindow) {
            scheduleStableFirstPaintRefresh();
        }
        const start = viewportRange.start || meta.startRow;
        const end = Math.max(viewportRange.end, meta.endRow - 1, meta.startRow);
        ensureViewportWindows(start, end);
    };
    const resetViewState = () => {
        errorMessage = '';
        totalRows = 0;
        totalCols = 0;
        sheetDefaults = { ...DEFAULT_SHEET_DEFAULTS };
        sheetImages = [];
        sheetCharts = [];
        virtualState = createEmptyVirtualState();
        hasInitialWindow = false;
        resetViewportTracking();
        syncWindowStats();
        syncImageViewport();
        if (!table) {
            return;
        }
        table.loadColumns([]);
        table.loadData([]);
        table.scrollTo(0, 0);
        table.draw();
    };
    const cacheCurrentSheetState = () => {
        const sheetId = getActiveSheetId();
        if (sheetId === undefined || !virtualState.active) {
            return;
        }
        sheetStateCache.set(sheetId, virtualState);
    };
    const restoreCachedSheetState = (sheetId) => {
        const cached = sheetStateCache.get(sheetId);
        if (!cached) {
            return false;
        }
        cached.loadingWindows.clear();
        virtualState = cached;
        errorMessage = '';
        totalRows = cached.totalRows;
        totalCols = cached.totalCols;
        sheetDefaults = cached.defaults;
        sheetImages = sheetImageCache.get(sheetId) || [];
        sheetCharts = sheetChartCache.get(sheetId) || [];
        hasInitialWindow = cached.loadedWindows.size > 0;
        sheetInitializing = !hasInitialWindow;
        syncWindowStats();
        queueMicrotask(() => {
            if (disposed) {
                return;
            }
            renderTable(ensureTable(), cached.columns, cached.rows);
            syncImageViewport();
            scheduleStableFirstPaintRefresh();
        });
        return true;
    };
    const startSheetSession = () => {
        const sheetId = getActiveSheetId();
        if (sheetId === undefined) {
            loadingState = false;
            sheetInitializing = false;
            renderChrome();
            return;
        }
        sheetSessionId += 1;
        if (restoreCachedSheetState(sheetId)) {
            loadingState = false;
            renderChrome();
            return;
        }
        sheetInitializing = true;
        resetViewState();
        requestWindow(0, false);
    };
    function handleSheet(index) {
        if (sheetIndex === index) {
            scrollActiveSheetIntoView();
            return;
        }
        cacheCurrentSheetState();
        sheetIndex = index;
        renderChrome();
        startSheetSession();
        scrollActiveSheetIntoView();
    }
    const emitParseWorkbook = () => {
        var _a, _b;
        emitWorker('parseWorkbook', {
            workbook: buffer,
            fileType: type,
            filename: context === null || context === void 0 ? void 0 : context.filename,
            textEncoding: (_b = (_a = context === null || context === void 0 ? void 0 : context.options) === null || _a === void 0 ? void 0 : _a.spreadsheet) === null || _b === void 0 ? void 0 : _b.textEncoding,
        });
    };
    controller.onWorkerEvent('sheets', ({ sheets: list }) => {
        sheets = list;
        const firstSheet = list.find((sheet) => !sheet.hidden) || list[0];
        if (firstSheet) {
            sheetIndex = firstSheet.id;
            renderChrome();
            startSheetSession();
            scrollActiveSheetIntoView();
            return;
        }
        sheetInitializing = false;
        loadingState = false;
        renderChrome();
    });
    controller.onWorkerEvent('parseSheet', ({ sessionId, sheet, sheetData: ws }) => {
        if (sessionId !== sheetSessionId || sheet !== getActiveSheetId()) {
            return;
        }
        applyVirtualWindow(ws);
    });
    controller.onWorkerEvent('parseError', ({ sessionId, startRow, message }) => {
        if (sessionId && sessionId !== sheetSessionId) {
            return;
        }
        sheetInitializing = false;
        loadingState = false;
        if (typeof startRow === 'number') {
            virtualState.loadingWindows.delete(startRow);
            syncWindowStats();
            if (virtualState.active) {
                markWindowState(virtualState.rows, virtualState.totalRows, startRow, RowState.Placeholder);
                table === null || table === void 0 ? void 0 : table.draw();
            }
        }
        else {
            virtualState.loadingWindows.clear();
            syncWindowStats();
        }
        errorMessage = message || t('spreadsheet.error.parseFailed');
        renderChrome();
    });
    controller.onWorkerError((event) => {
        sheetInitializing = false;
        loadingState = false;
        errorMessage = event.message || t('spreadsheet.error.workerFailed');
        renderChrome();
    });
    (_j = context === null || context === void 0 ? void 0 : context.registerExportAdapter) === null || _j === void 0 ? void 0 : _j.call(context, {
        print: false,
        exportHtml: false,
    });
    (_k = context === null || context === void 0 ? void 0 : context.registerThumbnailAdapter) === null || _k === void 0 ? void 0 : _k.call(context, {
        beforeCapture: async ({ signal }) => {
            while (!hasInitialWindow && !errorMessage && !disposed) {
                if (signal === null || signal === void 0 ? void 0 : signal.aborted) {
                    throw signal.reason;
                }
                await new Promise(resolve => {
                    const view = getTargetWindow(target);
                    if (view)
                        view.setTimeout(resolve, 16);
                    else
                        setTimeout(resolve, 16);
                });
            }
            if (errorMessage) {
                throw new Error(errorMessage);
            }
        },
        getTarget: () => tableWrapper,
    });
    ensureTable();
    const ResizeObserverCtor = ((_l = getTargetWindow(target)) === null || _l === void 0 ? void 0 : _l.ResizeObserver) ||
        (typeof ResizeObserver !== 'undefined' ? ResizeObserver : undefined);
    if (ResizeObserverCtor) {
        resizeObserver = new ResizeObserverCtor(() => {
            if (resizeFrame) {
                cancelAnimationFrame(resizeFrame);
            }
            resizeFrame = requestAnimationFrame(() => {
                resizeFrame = 0;
                syncTableLayout();
            });
        });
        resizeObserver.observe(tableHost);
    }
    renderChrome();
    emitParseWorkbook();
    return {
        $el: root,
        unmount() {
            var _a, _b;
            disposed = true;
            imageSourceResolver.dispose();
            closeImageLightbox();
            tableHostShell.removeEventListener('dblclick', handleImageDoubleClick, true);
            imageLightbox.removeEventListener('click', handleImageLightboxClick);
            lightboxCloseButton.removeEventListener('click', closeImageLightbox);
            documentRef.removeEventListener('keydown', handleImageLightboxKeyDown);
            if (resizeFrame) {
                cancelAnimationFrame(resizeFrame);
            }
            if (scrollFrame) {
                cancelAnimationFrame(scrollFrame);
            }
            clearScheduledLayoutRefresh();
            resizeObserver === null || resizeObserver === void 0 ? void 0 : resizeObserver.disconnect();
            resizeObserver = null;
            unregisterFileViewerZoomProvider(root);
            controller.destroy();
            table === null || table === void 0 ? void 0 : table.destroy();
            table = null;
            (_a = context === null || context === void 0 ? void 0 : context.registerExportAdapter) === null || _a === void 0 ? void 0 : _a.call(context, null);
            (_b = context === null || context === void 0 ? void 0 : context.registerThumbnailAdapter) === null || _b === void 0 ? void 0 : _b.call(context, null);
        },
    };
};
export default renderFileViewerSpreadsheet;
