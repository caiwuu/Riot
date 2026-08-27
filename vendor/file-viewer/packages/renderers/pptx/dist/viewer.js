import { renderPptxPostProcessing } from './chart.js';
import { resolvePptxEngineOptions, RECOMMENDED_ZIP_LIMITS } from './options.js';
import { PptxPresentation } from './presentation.js';
import { sanitizePptxCss, sanitizePptxMarkup } from './sanitize.js';
import { ensurePptxViewerStyles } from './styles.js';
import { createPptxWorker } from './worker.js';
const clamp = (value, min, max) => {
    return Math.min(max, Math.max(min, value));
};
const toPercent = (value) => {
    const numeric = typeof value === 'number' && Number.isFinite(value) ? value : 100;
    return clamp(numeric, 25, 300);
};
const stringifyErrorDetail = (error) => {
    if (error === undefined || error === null) {
        return '';
    }
    if (typeof error === 'string') {
        return error;
    }
    if (error instanceof Error && error.message) {
        return error.message;
    }
    try {
        const serialized = JSON.stringify(error);
        return serialized && serialized !== '{}' ? serialized : String(error);
    }
    catch {
        return String(error);
    }
};
const createPptxDiagnosticError = (code, stage, message, detailOrError, hint) => ({
    name: 'PptxDiagnosticError',
    code,
    stage,
    message,
    detail: stringifyErrorDetail(detailOrError),
    hint,
});
const ensureZipWithinLimits = (buffer, options) => {
    const limits = {
        ...RECOMMENDED_ZIP_LIMITS,
        ...options.zipLimits,
    };
    if (limits.maxFileBytes && buffer.byteLength > limits.maxFileBytes) {
        throw createPptxDiagnosticError('PPTX_FILE_TOO_LARGE', 'preflight', 'PPTX 文件超过浏览器安全预览体积限制。', `${buffer.byteLength} bytes > ${limits.maxFileBytes} bytes`, '请压缩图片、拆分演示文稿，或在业务侧提高 zipLimits.maxFileBytes 后再预览。');
    }
};
const appendHtml = (container, html) => {
    const fragment = sanitizePptxMarkup(container.ownerDocument, html);
    const nodes = Array.from(fragment.children);
    container.append(fragment);
    return nodes[0] || null;
};
const escapeHtml = (value) => String(value !== null && value !== void 0 ? value : '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
const buildSlideErrorHtml = (slideNumber, error) => {
    const diagnostic = error && typeof error === 'object'
        ? error
        : null;
    const message = (diagnostic === null || diagnostic === void 0 ? void 0 : diagnostic.message) || stringifyErrorDetail(error) || 'This slide could not be rendered.';
    return `<section class="slide flyfish-pptx-slide-error" data-slide-index="${slideNumber}"><div class="flyfish-pptx-slide-error-card" role="status"><strong>Slide ${slideNumber}</strong><span>${escapeHtml(message)}</span></div></section>`;
};
const DEFAULT_INITIAL_SLIDES = 3;
const DEFAULT_BATCH_SIZE = 4;
const DEFAULT_OVERSCAN_VIEWPORTS = 1.5;
const DEFAULT_SLIDE_HEIGHT = 540;
const DEFAULT_SLIDE_GAP = 50;
const toPositiveInteger = (value, fallback, min = 1, max = Number.MAX_SAFE_INTEGER) => {
    const numeric = typeof value === 'number' && Number.isFinite(value)
        ? Math.floor(value)
        : fallback;
    return clamp(numeric, min, max);
};
const toPositiveNumber = (value, fallback, min = 0, max = Number.MAX_SAFE_INTEGER) => {
    const numeric = typeof value === 'number' && Number.isFinite(value) ? value : fallback;
    return clamp(numeric, min, max);
};
export class PptxViewer {
    static async open(buffer, target, options = {}) {
        const viewer = new PptxViewer(buffer, target, options);
        await viewer.open();
        return viewer;
    }
    constructor(buffer, target, options) {
        this.worker = null;
        this.resizeObserver = null;
        this.resizeFrame = 0;
        this.fitScale = 1;
        this.userZoomPercent = 100;
        this.currentZoomPercent = 100;
        this.thumbnailElement = null;
        this.previewThumbnailDataUrl = null;
        this.slideSize = null;
        this.slideRecords = [];
        // Total slides received from the worker. The windowed path mirrors this in
        // slideRecords, but the default (non-windowed) path appends slides straight
        // into the content node without creating records, so the deck size has to be
        // counted independently of the virtualization bookkeeping.
        this.totalSlides = 0;
        this.savedScroll = null;
        this.pendingScrollRestore = null;
        this.slideWindowTarget = null;
        this.slideWindowListeners = [];
        this.slideWindowFrame = 0;
        this.charts = null;
        this.chartHandles = new Set();
        this.mediaRecords = new Map();
        this.disposed = false;
        this.completed = false;
        this.presentation = null;
        this.handleSlideWindowChange = () => this.scheduleSlideWindowUpdate();
        this.buffer = buffer;
        this.target = target;
        this.options = options;
        this.userZoomPercent = toPercent(options.zoomPercent);
        this.currentZoomPercent = this.userZoomPercent;
        const documentRef = target.ownerDocument || document;
        this.scaleBox = documentRef.createElement('div');
        this.scaleBox.className = 'flyfish-pptx-scale-box';
        this.content = documentRef.createElement('div');
        this.content.className = 'flyfish-pptx-content';
        this.content.dataset.renderState = 'loading';
        this.scaleBox.append(this.content);
    }
    get zoomPercent() {
        return Math.round(this.currentZoomPercent);
    }
    get thumbnailDataUrl() {
        return this.previewThumbnailDataUrl;
    }
    get slideCount() {
        return this.totalSlides;
    }
    get slideDimensions() {
        var _a, _b;
        const width = Number((_a = this.slideSize) === null || _a === void 0 ? void 0 : _a.width);
        const height = Number((_b = this.slideSize) === null || _b === void 0 ? void 0 : _b.height);
        return Number.isFinite(width) && width > 0 && Number.isFinite(height) && height > 0
            ? { width, height }
            : null;
    }
    get presenting() {
        var _a;
        return Boolean((_a = this.presentation) === null || _a === void 0 ? void 0 : _a.active);
    }
    get presentationSlideNumber() {
        var _a, _b;
        return (_b = (_a = this.presentation) === null || _a === void 0 ? void 0 : _a.slideNumber) !== null && _b !== void 0 ? _b : 1;
    }
    get styleRoot() {
        var _a;
        if (this.options.styleRoot) {
            return this.options.styleRoot;
        }
        const documentRef = this.target.ownerDocument || document;
        const root = this.target.getRootNode();
        const ShadowRootCtor = (_a = documentRef.defaultView) === null || _a === void 0 ? void 0 : _a.ShadowRoot;
        return ShadowRootCtor && root instanceof ShadowRootCtor
            ? root
            : documentRef;
    }
    /**
     * Where the slideshow overlay is mounted. It has to share a root with the injected slide styles,
     * or the engine's scoped CSS stops applying once the slides move into the overlay.
     */
    get presentationRoot() {
        const documentRef = this.target.ownerDocument || document;
        const styleRoot = this.styleRoot;
        if (styleRoot !== documentRef) {
            return styleRoot;
        }
        return documentRef.body || documentRef.documentElement;
    }
    /** Force a slide out of the virtualized window so the slideshow can show it immediately. */
    ensureSlideRendered(slideNumber) {
        const record = this.slideRecords.find(item => item.slideNumber === slideNumber);
        if (!record) {
            return null;
        }
        if (!record.rendered) {
            this.renderSlideRecord(record);
        }
        return record.element;
    }
    refreshLayout() {
        this.scheduleResize();
    }
    /**
     * Remember where the deck was scrolled before the slideshow moves the scale
     * box into the overlay. Moving it collapses the scroller and clamps scrollTop
     * to zero, so the position has to be captured up front and restored on exit.
     */
    saveScrollPosition() {
        const view = this.target.ownerDocument.defaultView || window;
        const HTMLElementCtor = view.HTMLElement;
        const scroller = this.slideWindowTarget || this.findSlideWindowTarget();
        if (scroller instanceof HTMLElementCtor) {
            this.savedScroll = { top: scroller.scrollTop, left: scroller.scrollLeft };
        }
        else {
            this.savedScroll = { top: view.scrollY, left: view.scrollX };
        }
    }
    restoreScrollPosition() {
        if (this.savedScroll) {
            this.pendingScrollRestore = this.savedScroll;
            this.savedScroll = null;
        }
        this.scheduleResize();
    }
    emitPresentationChange(state) {
        var _a, _b;
        (_b = (_a = this.options).onPresentationChange) === null || _b === void 0 ? void 0 : _b.call(_a, state);
    }
    async enterPresentation(slideNumber) {
        if (this.disposed || this.totalSlides === 0) {
            return;
        }
        this.presentation || (this.presentation = new PptxPresentation(this, this.options.presentationLabels, this.options.presentationFullscreen));
        await this.presentation.enter(slideNumber);
    }
    exitPresentation() {
        var _a;
        (_a = this.presentation) === null || _a === void 0 ? void 0 : _a.exit();
    }
    async togglePresentation(slideNumber) {
        if (this.presenting) {
            this.exitPresentation();
            return;
        }
        await this.enterPresentation(slideNumber);
    }
    async open() {
        ensureZipWithinLimits(this.buffer, this.options);
        ensurePptxViewerStyles(this.target.ownerDocument || document, this.styleRoot);
        this.target.replaceChildren(this.scaleBox);
        this.attachResizeObserver();
        this.attachSlideWindowListeners();
        this.startWorker();
    }
    async setZoom(percent) {
        const desiredEffectivePercent = toPercent(percent);
        this.userZoomPercent = clamp(desiredEffectivePercent / Math.max(this.fitScale, 0.01), 25, 600);
        this.scheduleResize();
    }
    destroy() {
        var _a, _b, _c, _d, _e;
        this.disposed = true;
        (_a = this.presentation) === null || _a === void 0 ? void 0 : _a.destroy();
        this.presentation = null;
        this.previewThumbnailDataUrl = null;
        this.releaseCharts();
        this.releaseMedia();
        (_b = this.worker) === null || _b === void 0 ? void 0 : _b.terminate();
        this.worker = null;
        (_c = this.resizeObserver) === null || _c === void 0 ? void 0 : _c.disconnect();
        this.resizeObserver = null;
        if (this.resizeFrame) {
            (_d = this.target.ownerDocument.defaultView) === null || _d === void 0 ? void 0 : _d.cancelAnimationFrame(this.resizeFrame);
        }
        if (this.slideWindowFrame) {
            (_e = this.target.ownerDocument.defaultView) === null || _e === void 0 ? void 0 : _e.cancelAnimationFrame(this.slideWindowFrame);
        }
        this.detachSlideWindowListeners();
        this.target.replaceChildren();
    }
    startWorker() {
        var _a;
        (_a = this.worker) === null || _a === void 0 ? void 0 : _a.terminate();
        this.releaseCharts();
        this.releaseMedia();
        this.completed = false;
        this.charts = null;
        this.previewThumbnailDataUrl = null;
        this.slideSize = null;
        this.slideRecords = [];
        this.totalSlides = 0;
        this.content.replaceChildren();
        this.content.dataset.renderState = 'loading';
        try {
            this.worker = createPptxWorker(this.options);
        }
        catch (error) {
            throw createPptxDiagnosticError('PPTX_WORKER_FAILED', 'start-worker', 'PPTX Worker 启动失败。', error, '请检查 presentation.workerUrl、Worker 文件是否 200 返回、MIME/CSP/跨域策略是否允许加载。');
        }
        this.worker.addEventListener('message', event => this.processMessage(event.data));
        this.worker.addEventListener('error', event => this.fail(createPptxDiagnosticError('PPTX_WORKER_FAILED', 'worker-runtime', 'PPTX Worker 运行失败。', event.error || event.message, '请检查 Worker 脚本是否完整、浏览器控制台是否有 CSP/MIME/跨域或语法错误。')));
        try {
            this.worker.postMessage({
                type: 'processPPTX',
                data: this.buffer,
                IE11: false,
                options: resolvePptxEngineOptions(this.options.engineOptions),
            });
        }
        catch (error) {
            throw createPptxDiagnosticError('PPTX_WORKER_FAILED', 'post-worker-message', 'PPTX 数据发送到 Worker 失败。', error, '请确认浏览器支持 Worker 消息传递，并避免传入已被释放或不可克隆的数据。');
        }
    }
    processMessage(message) {
        var _a, _b, _c, _d, _e, _f, _g, _h, _j, _k, _l, _m, _o, _p, _q, _r;
        if (this.disposed || this.completed) {
            return;
        }
        switch (message.type) {
            case 'slide': {
                this.clearThumbnail();
                this.totalSlides += 1;
                if (this.shouldWindowSlides()) {
                    this.appendWindowedSlide(String(message.data || ''), Number(message.slide_num || 0));
                }
                else {
                    const element = appendHtml(this.content, String(message.data || ''));
                    this.hydrateMedia(element);
                    this.scheduleResize();
                    (_b = (_a = this.options).onSlideRendered) === null || _b === void 0 ? void 0 : _b.call(_a, Number(message.slide_num || 0), element);
                }
                break;
            }
            case 'media':
                this.storeMedia(message.data);
                break;
            case 'slide-error': {
                const slideNumber = Number(message.slide_num || 0);
                const error = message.data;
                const html = buildSlideErrorHtml(slideNumber, error);
                this.clearThumbnail();
                this.totalSlides += 1;
                if (this.shouldWindowSlides()) {
                    this.appendWindowedSlide(html, slideNumber);
                }
                else {
                    const element = appendHtml(this.content, html);
                    this.scheduleResize();
                    (_d = (_c = this.options).onSlideRendered) === null || _d === void 0 ? void 0 : _d.call(_c, slideNumber, element);
                }
                (_f = (_e = this.options).onSlideError) === null || _f === void 0 ? void 0 : _f.call(_e, slideNumber, error);
                (_h = (_g = this.options).onWarning) === null || _h === void 0 ? void 0 : _h.call(_g, error);
                break;
            }
            case 'pptx-thumb':
                this.showThumbnail(String(message.data || ''));
                (_k = (_j = this.options).onThumbnail) === null || _k === void 0 ? void 0 : _k.call(_j, String(message.data || ''));
                break;
            case 'slideSize':
                this.slideSize = (message.data || {});
                this.syncWindowedPlaceholderHeights();
                (_m = (_l = this.options).onSlideSize) === null || _m === void 0 ? void 0 : _m.call(_l, (message.data || {}));
                break;
            case 'globalCSS':
                this.appendGlobalCss(String(message.data || ''));
                break;
            case 'progress-update':
                (_p = (_o = this.options).onProgress) === null || _p === void 0 ? void 0 : _p.call(_o, Number(message.data || 0), message);
                break;
            case 'ExecutionTime':
            case 'Done':
                void this.complete(message.charts).catch(error => this.fail(createPptxDiagnosticError('PPTX_PARSE_FAILED', 'post-process', 'PPTX 渲染后处理失败。', error)));
                break;
            case 'WARN':
                (_r = (_q = this.options).onWarning) === null || _r === void 0 ? void 0 : _r.call(_q, message.data);
                break;
            case 'ERROR':
                this.fail(message.data);
                break;
            default:
                break;
        }
    }
    appendGlobalCss(css) {
        const sanitized = sanitizePptxCss(this.target.ownerDocument, css);
        if (!sanitized) {
            return;
        }
        const style = this.target.ownerDocument.createElement('style');
        style.textContent = sanitized;
        this.content.append(style);
    }
    showThumbnail(base64Jpeg) {
        if (!base64Jpeg) {
            return;
        }
        this.previewThumbnailDataUrl = `data:image/jpeg;base64,${base64Jpeg}`;
        if (this.content.children.length > 0) {
            return;
        }
        const image = this.target.ownerDocument.createElement('img');
        image.className = 'flyfish-pptx-thumbnail';
        image.alt = 'PPTX preview thumbnail';
        image.src = this.previewThumbnailDataUrl;
        this.thumbnailElement = image;
        this.scaleBox.insertBefore(image, this.content);
    }
    clearThumbnail() {
        var _a;
        (_a = this.thumbnailElement) === null || _a === void 0 ? void 0 : _a.remove();
        this.thumbnailElement = null;
    }
    shouldWindowSlides() {
        var _a;
        return Boolean(this.options.lazySlides || ((_a = this.options.listOptions) === null || _a === void 0 ? void 0 : _a.windowed));
    }
    getSlideWindowOptions() {
        const listOptions = this.options.listOptions || {};
        return {
            initialSlides: toPositiveInteger(listOptions.initialSlides, DEFAULT_INITIAL_SLIDES, 1, 20),
            batchSize: toPositiveInteger(listOptions.batchSize, DEFAULT_BATCH_SIZE, 1, 20),
            overscanViewport: toPositiveNumber(listOptions.overscanViewport, DEFAULT_OVERSCAN_VIEWPORTS, 0.25, 6),
        };
    }
    getEstimatedSlideHeight() {
        var _a, _b;
        const sizeHeight = Number((_a = this.slideSize) === null || _a === void 0 ? void 0 : _a.height);
        const measuredHeight = (_b = this.slideRecords.find(record => record.estimatedHeight > 0)) === null || _b === void 0 ? void 0 : _b.estimatedHeight;
        return Math.ceil((Number.isFinite(sizeHeight) && sizeHeight > 0
            ? sizeHeight + DEFAULT_SLIDE_GAP
            : measuredHeight || DEFAULT_SLIDE_HEIGHT + DEFAULT_SLIDE_GAP));
    }
    appendWindowedSlide(html, slideNumber) {
        const record = this.createWindowedSlideRecord(html, slideNumber || this.slideRecords.length + 1);
        this.slideRecords.push(record);
        this.content.append(record.slot);
        if (this.slideRecords.length <= this.getSlideWindowOptions().initialSlides) {
            this.renderSlideRecord(record);
        }
        this.scheduleResize();
        this.scheduleSlideWindowUpdate();
    }
    createWindowedSlideRecord(html, slideNumber) {
        const slot = this.target.ownerDocument.createElement('div');
        const estimatedHeight = this.getEstimatedSlideHeight();
        slot.className = 'flyfish-pptx-slide-slot';
        slot.dataset.slideNumber = String(slideNumber);
        slot.style.minHeight = `${estimatedHeight}px`;
        return {
            slideNumber,
            html,
            slot,
            element: null,
            estimatedHeight,
            rendered: false,
            notified: false,
            postProcessed: false,
            postProcessPromise: null,
            postProcessVersion: 0,
            chartHandle: null,
        };
    }
    getDesignerCanvases(record) {
        return Array.from(record.slot.children).filter(element => (element.classList.contains('fv-print-mask-canvas')));
    }
    renderSlideRecord(record) {
        var _a, _b;
        if (record.rendered || this.disposed) {
            return;
        }
        const designerCanvases = this.getDesignerCanvases(record);
        record.slot.replaceChildren();
        record.element = appendHtml(record.slot, record.html);
        this.hydrateMedia(record.element);
        record.slot.append(...designerCanvases);
        record.rendered = true;
        record.postProcessed = false;
        record.postProcessVersion += 1;
        this.updateMeasuredSlideHeight(record);
        if (!record.notified) {
            record.notified = true;
            (_b = (_a = this.options).onSlideRendered) === null || _b === void 0 ? void 0 : _b.call(_a, record.slideNumber, record.element);
        }
        if (this.completed) {
            void this.postProcessSlideRecord(record).finally(() => this.scheduleResize());
        }
        this.scheduleResize();
    }
    unmountSlideRecord(record) {
        if (!record.rendered || this.getDesignerCanvases(record).length > 0) {
            return;
        }
        this.updateMeasuredSlideHeight(record);
        this.releaseChartHandle(record.chartHandle);
        record.chartHandle = null;
        record.postProcessVersion += 1;
        record.slot.replaceChildren();
        record.element = null;
        record.rendered = false;
        record.postProcessed = false;
        record.postProcessPromise = null;
    }
    updateMeasuredSlideHeight(record) {
        var _a;
        const HTMLElementCtor = ((_a = this.target.ownerDocument.defaultView) === null || _a === void 0 ? void 0 : _a.HTMLElement) || HTMLElement;
        if (!(record.element instanceof HTMLElementCtor)) {
            return;
        }
        const view = this.target.ownerDocument.defaultView || window;
        const computed = view.getComputedStyle(record.element);
        const marginTop = parseFloat(computed.marginTop) || 0;
        const marginBottom = parseFloat(computed.marginBottom) || 0;
        const measuredHeight = Math.ceil(record.element.offsetHeight + marginTop + marginBottom);
        const nextHeight = Math.max(measuredHeight, record.estimatedHeight, 1);
        record.estimatedHeight = nextHeight;
        record.slot.style.minHeight = `${nextHeight}px`;
    }
    syncWindowedPlaceholderHeights() {
        if (!this.shouldWindowSlides() || !this.slideRecords.length) {
            return;
        }
        const estimatedHeight = this.getEstimatedSlideHeight();
        for (const record of this.slideRecords) {
            if (!record.rendered && record.estimatedHeight < estimatedHeight) {
                record.estimatedHeight = estimatedHeight;
                record.slot.style.minHeight = `${estimatedHeight}px`;
            }
        }
        this.scheduleResize();
    }
    scheduleSlideWindowUpdate() {
        if (!this.shouldWindowSlides() || this.disposed) {
            return;
        }
        const view = this.target.ownerDocument.defaultView || window;
        if (this.slideWindowFrame) {
            view.cancelAnimationFrame(this.slideWindowFrame);
        }
        this.slideWindowFrame = view.requestAnimationFrame(() => this.updateSlideWindow());
    }
    updateSlideWindow() {
        this.slideWindowFrame = 0;
        if (!this.shouldWindowSlides() || !this.slideRecords.length || this.disposed) {
            return;
        }
        const windowOptions = this.getSlideWindowOptions();
        const indexesToRender = this.getWindowedSlideIndexes(windowOptions);
        // While presenting, the active slide and its neighbour live inside the fixed
        // overlay, outside the scroll-view geometry the windowing code measures. Keep
        // them mounted so a resize cannot unmount the slide that is on screen.
        const presentation = this.presentation;
        if (presentation === null || presentation === void 0 ? void 0 : presentation.active) {
            const activeIndex = this.slideRecords.findIndex(record => record.slideNumber === presentation.slideNumber);
            if (activeIndex >= 0) {
                indexesToRender.add(activeIndex);
                if (activeIndex + 1 < this.slideRecords.length) {
                    indexesToRender.add(activeIndex + 1);
                }
            }
        }
        let changed = false;
        this.slideRecords.forEach((record, index) => {
            if (indexesToRender.has(index)) {
                const wasRendered = record.rendered;
                this.renderSlideRecord(record);
                changed || (changed = !wasRendered);
            }
            else if (record.rendered) {
                this.unmountSlideRecord(record);
                changed = true;
            }
        });
        if (changed) {
            this.scheduleResize();
        }
    }
    getWindowedSlideIndexes(windowOptions) {
        const viewport = this.getSlideViewport();
        const maxMountedSlides = Math.max(windowOptions.initialSlides, windowOptions.initialSlides + windowOptions.batchSize * 4, 12);
        const selected = new Set();
        for (let index = 0; index < Math.min(windowOptions.initialSlides, this.slideRecords.length); index += 1) {
            selected.add(index);
        }
        const candidates = this.slideRecords
            .map((record, index) => ({
            index,
            distance: this.getDistanceFromViewport(record, viewport, windowOptions.overscanViewport),
        }))
            .filter(candidate => candidate.distance !== Number.POSITIVE_INFINITY)
            .sort((a, b) => a.distance - b.distance || a.index - b.index);
        const closestFallback = candidates[0];
        for (const candidate of candidates) {
            if (candidate.distance > 0 && selected.size >= windowOptions.initialSlides) {
                continue;
            }
            selected.add(candidate.index);
        }
        if (closestFallback) {
            selected.add(closestFallback.index);
        }
        if (selected.size <= maxMountedSlides) {
            return selected;
        }
        const capped = new Set();
        for (let index = 0; index < Math.min(windowOptions.initialSlides, this.slideRecords.length); index += 1) {
            capped.add(index);
        }
        for (const candidate of candidates) {
            if (capped.size >= maxMountedSlides) {
                break;
            }
            capped.add(candidate.index);
        }
        return capped;
    }
    getDistanceFromViewport(record, viewport, overscanViewport) {
        const rect = record.slot.getBoundingClientRect();
        const overscan = viewport.height * overscanViewport;
        const top = viewport.top - overscan;
        const bottom = viewport.bottom + overscan;
        if (rect.bottom >= top && rect.top <= bottom) {
            return 0;
        }
        if (rect.bottom < top) {
            return top - rect.bottom;
        }
        if (rect.top > bottom) {
            return rect.top - bottom;
        }
        return Number.POSITIVE_INFINITY;
    }
    getSlideViewport() {
        const view = this.target.ownerDocument.defaultView || window;
        const HTMLElementCtor = view.HTMLElement;
        if (this.slideWindowTarget instanceof HTMLElementCtor) {
            const rect = this.slideWindowTarget.getBoundingClientRect();
            const height = Math.max(1, rect.height || this.slideWindowTarget.clientHeight || view.innerHeight || 1);
            return {
                top: rect.top,
                bottom: rect.top + height,
                height,
            };
        }
        const height = Math.max(1, view.innerHeight || this.target.ownerDocument.documentElement.clientHeight || DEFAULT_SLIDE_HEIGHT);
        return {
            top: 0,
            bottom: height,
            height,
        };
    }
    attachSlideWindowListeners() {
        if (!this.shouldWindowSlides()) {
            return;
        }
        this.detachSlideWindowListeners();
        const view = this.target.ownerDocument.defaultView || window;
        this.slideWindowTarget = this.findSlideWindowTarget();
        this.addSlideWindowListener(view, 'scroll');
        this.addSlideWindowListener(view, 'resize');
        if (this.slideWindowTarget && this.slideWindowTarget !== view) {
            this.addSlideWindowListener(this.slideWindowTarget, 'scroll');
        }
    }
    addSlideWindowListener(target, type) {
        target.addEventListener(type, this.handleSlideWindowChange, { passive: true });
        this.slideWindowListeners.push({ target, type, listener: this.handleSlideWindowChange });
    }
    detachSlideWindowListeners() {
        for (const { target, type, listener } of this.slideWindowListeners) {
            target.removeEventListener(type, listener);
        }
        this.slideWindowListeners = [];
        this.slideWindowTarget = null;
    }
    findSlideWindowTarget() {
        const view = this.target.ownerDocument.defaultView || window;
        let element = this.target.parentElement;
        while (element && element !== this.target.ownerDocument.body && element !== this.target.ownerDocument.documentElement) {
            const style = view.getComputedStyle(element);
            if (/(auto|scroll|overlay)/.test(`${style.overflowY} ${style.overflow}`)) {
                return element;
            }
            element = element.parentElement;
        }
        return view;
    }
    async postProcessRenderedContent(charts) {
        if (!this.shouldWindowSlides()) {
            const handle = await renderPptxPostProcessing(charts, this.content);
            this.trackChartHandle(handle);
            return;
        }
        await Promise.all(this.slideRecords
            .filter(record => record.rendered)
            .map(record => this.postProcessSlideRecord(record)));
    }
    async postProcessSlideRecord(record) {
        if (!record.rendered || record.postProcessed || this.disposed) {
            return;
        }
        if (!record.postProcessPromise) {
            const version = record.postProcessVersion;
            const postProcessPromise = (async () => {
                const handle = await renderPptxPostProcessing(this.charts, record.slot);
                if (this.disposed || !record.rendered || record.postProcessVersion !== version) {
                    handle === null || handle === void 0 ? void 0 : handle.destroy();
                    return;
                }
                this.releaseChartHandle(record.chartHandle);
                record.chartHandle = handle;
                this.trackChartHandle(handle);
                record.postProcessed = true;
                this.updateMeasuredSlideHeight(record);
            })().finally(() => {
                if (record.postProcessPromise === postProcessPromise) {
                    record.postProcessPromise = null;
                }
            });
            record.postProcessPromise = postProcessPromise;
        }
        await record.postProcessPromise;
    }
    /** Materializes every lazy slide only for the duration of an export snapshot. */
    async cloneForExport() {
        if (!this.shouldWindowSlides()) {
            return this.target.cloneNode(true);
        }
        const previouslyRendered = new Set(this.slideRecords.filter(record => record.rendered));
        try {
            this.slideRecords.forEach(record => this.renderSlideRecord(record));
            await Promise.all(this.slideRecords.map(record => this.postProcessSlideRecord(record)));
            return this.target.cloneNode(true);
        }
        finally {
            this.slideRecords.forEach(record => {
                if (!previouslyRendered.has(record)) {
                    this.unmountSlideRecord(record);
                }
            });
            this.scheduleSlideWindowUpdate();
            this.scheduleResize();
        }
    }
    async complete(charts) {
        var _a, _b, _c;
        if (this.disposed || this.completed) {
            return;
        }
        this.completed = true;
        this.charts = charts;
        this.content.dataset.renderState = 'ready';
        (_a = this.worker) === null || _a === void 0 ? void 0 : _a.terminate();
        this.worker = null;
        this.scheduleSlideWindowUpdate();
        this.scheduleResize();
        await this.postProcessRenderedContent(charts);
        if (this.disposed) {
            return;
        }
        this.scheduleResize();
        (_c = (_b = this.options).onRenderComplete) === null || _c === void 0 ? void 0 : _c.call(_b);
    }
    fail(error) {
        var _a, _b, _c;
        if (this.disposed) {
            return;
        }
        this.completed = true;
        (_a = this.worker) === null || _a === void 0 ? void 0 : _a.terminate();
        this.worker = null;
        this.releaseCharts();
        this.releaseMedia();
        this.content.dataset.renderState = 'error';
        (_c = (_b = this.options).onError) === null || _c === void 0 ? void 0 : _c.call(_b, error);
    }
    trackChartHandle(handle) {
        if (!handle) {
            return;
        }
        if (this.disposed) {
            handle.destroy();
            return;
        }
        this.chartHandles.add(handle);
    }
    releaseChartHandle(handle) {
        if (!handle) {
            return;
        }
        this.chartHandles.delete(handle);
        handle.destroy();
    }
    releaseCharts() {
        for (const handle of this.chartHandles) {
            handle.destroy();
        }
        this.chartHandles.clear();
        for (const record of this.slideRecords) {
            record.chartHandle = null;
        }
    }
    storeMedia(data) {
        if (!data || typeof data !== 'object') {
            return;
        }
        const media = data;
        if (typeof media.id !== 'string' ||
            !media.id ||
            typeof media.mimeType !== 'string' ||
            !(media.buffer instanceof ArrayBuffer)) {
            return;
        }
        this.releaseMedia(media.id);
        this.mediaRecords.set(media.id, {
            buffer: media.buffer,
            mimeType: media.mimeType,
            objectUrl: null,
            revokeObjectUrl: null,
        });
        this.hydrateMedia(this.content);
    }
    hydrateMedia(root) {
        var _a;
        if (!root) {
            return;
        }
        const elements = Array.from(root.querySelectorAll('[data-pptx-media-id]'));
        if (root.matches('[data-pptx-media-id]')) {
            elements.unshift(root);
        }
        for (const element of elements) {
            const mediaId = element.dataset.pptxMediaId;
            const record = mediaId ? this.mediaRecords.get(mediaId) : undefined;
            if (!record) {
                continue;
            }
            if (!record.objectUrl) {
                const view = this.target.ownerDocument.defaultView;
                const BlobCtor = (view === null || view === void 0 ? void 0 : view.Blob) || globalThis.Blob;
                const UrlApi = (view === null || view === void 0 ? void 0 : view.URL) || globalThis.URL;
                if (typeof BlobCtor !== 'function' ||
                    typeof (UrlApi === null || UrlApi === void 0 ? void 0 : UrlApi.createObjectURL) !== 'function') {
                    continue;
                }
                const objectUrl = UrlApi.createObjectURL(new BlobCtor([record.buffer], { type: record.mimeType }));
                record.objectUrl = objectUrl;
                record.revokeObjectUrl = () => UrlApi.revokeObjectURL(objectUrl);
            }
            if (element.src !== record.objectUrl) {
                element.src = record.objectUrl;
                (_a = element.load) === null || _a === void 0 ? void 0 : _a.call(element);
            }
        }
    }
    releaseMedia(mediaId) {
        var _a, _b;
        if (mediaId) {
            const record = this.mediaRecords.get(mediaId);
            (_a = record === null || record === void 0 ? void 0 : record.revokeObjectUrl) === null || _a === void 0 ? void 0 : _a.call(record);
            this.mediaRecords.delete(mediaId);
            return;
        }
        for (const record of this.mediaRecords.values()) {
            (_b = record.revokeObjectUrl) === null || _b === void 0 ? void 0 : _b.call(record);
        }
        this.mediaRecords.clear();
    }
    attachResizeObserver() {
        if (typeof ResizeObserver === 'undefined') {
            return;
        }
        this.resizeObserver = new ResizeObserver(() => {
            this.scheduleResize();
            this.scheduleSlideWindowUpdate();
        });
        this.resizeObserver.observe(this.target);
        if (this.target.parentElement) {
            this.resizeObserver.observe(this.target.parentElement);
        }
    }
    scheduleResize() {
        const view = this.target.ownerDocument.defaultView || window;
        if (this.resizeFrame) {
            view.cancelAnimationFrame(this.resizeFrame);
        }
        this.resizeFrame = view.requestAnimationFrame(() => this.resize());
    }
    resize() {
        var _a, _b, _c;
        // While presenting, the slideshow owns the transform of the same nodes.
        if ((_a = this.presentation) === null || _a === void 0 ? void 0 : _a.active) {
            this.presentation.layout();
            return;
        }
        const slides = this.getMountedSlideElements();
        const sizeWidth = Number((_b = this.slideSize) === null || _b === void 0 ? void 0 : _b.width);
        const fallbackWidth = Number.isFinite(sizeWidth) && sizeWidth > 0 ? sizeWidth : 0;
        const slideWidth = Math.max(...slides.map(slide => slide.offsetWidth), fallbackWidth);
        if (!slideWidth) {
            return;
        }
        const viewportWidth = this.target.clientWidth || ((_c = this.target.parentElement) === null || _c === void 0 ? void 0 : _c.clientWidth) || slideWidth;
        this.fitScale = this.options.fitMode === 'none'
            ? 1
            : Math.min(1, viewportWidth / slideWidth);
        const effectiveScale = clamp(this.fitScale * (this.userZoomPercent / 100), 0.25, 3);
        this.currentZoomPercent = effectiveScale * 100;
        this.content.style.width = `${slideWidth}px`;
        this.content.style.transform = `scale(${effectiveScale})`;
        this.scaleBox.style.width = `${Math.ceil(slideWidth * effectiveScale)}px`;
        this.scaleBox.style.height = `${Math.ceil(this.content.scrollHeight * effectiveScale)}px`;
        this.scaleBox.style.minHeight = '';
        // The scale box height is recomputed above; only now can the saved scroll
        // position be applied without the scroller clamping it back to zero.
        if (this.pendingScrollRestore) {
            const { top, left } = this.pendingScrollRestore;
            this.pendingScrollRestore = null;
            const view = this.target.ownerDocument.defaultView || window;
            const HTMLElementCtor = view.HTMLElement;
            const scroller = this.slideWindowTarget || this.findSlideWindowTarget();
            if (scroller instanceof HTMLElementCtor) {
                scroller.scrollTop = top;
                scroller.scrollLeft = left;
            }
            else {
                view.scrollTo(left, top);
            }
        }
    }
    getMountedSlideElements() {
        var _a;
        const HTMLElementCtor = ((_a = this.target.ownerDocument.defaultView) === null || _a === void 0 ? void 0 : _a.HTMLElement) || HTMLElement;
        const slides = [];
        for (const child of Array.from(this.content.children)) {
            if (!(child instanceof HTMLElementCtor)) {
                continue;
            }
            if (this.isSlideElement(child)) {
                slides.push(child);
                continue;
            }
            if (!child.classList.contains('flyfish-pptx-slide-slot')) {
                continue;
            }
            const firstChild = child.firstElementChild;
            if (firstChild instanceof HTMLElementCtor && this.isSlideElement(firstChild)) {
                slides.push(firstChild);
            }
        }
        return slides;
    }
    isSlideElement(element) {
        return element.classList.contains('slide') || element.tagName === 'SECTION';
    }
}
