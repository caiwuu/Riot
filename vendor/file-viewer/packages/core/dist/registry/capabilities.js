import { ARCHIVE_EXTENSIONS, IMAGE_EXTENSIONS, MODEL_EXTENSIONS, TEXT_EXTENSIONS } from './formats.js';
import { normalizeFileExtension } from '../source/index.js';
export const DEFAULT_OPERATION_AVAILABILITY = Object.freeze({
    download: false,
    print: false,
    exportHtml: false,
    zoom: false,
    zoomIn: false,
    zoomOut: false,
    zoomReset: false,
});
const resolveBooleanCapability = (value) => {
    return value === true || value === 'adapter' || value === 'provider';
};
export const getRendererAvailability = (renderer, session) => {
    var _a, _b, _c, _d, _e, _f, _g, _h;
    if (!renderer) {
        return { ...DEFAULT_OPERATION_AVAILABILITY };
    }
    const base = {
        download: ((_a = renderer.capabilities) === null || _a === void 0 ? void 0 : _a.download) !== false,
        print: resolveBooleanCapability((_b = renderer.capabilities) === null || _b === void 0 ? void 0 : _b.print),
        exportHtml: resolveBooleanCapability((_c = renderer.capabilities) === null || _c === void 0 ? void 0 : _c.exportHtml),
        zoom: resolveBooleanCapability((_d = renderer.capabilities) === null || _d === void 0 ? void 0 : _d.zoom),
        zoomIn: resolveBooleanCapability((_e = renderer.capabilities) === null || _e === void 0 ? void 0 : _e.zoom),
        zoomOut: resolveBooleanCapability((_f = renderer.capabilities) === null || _f === void 0 ? void 0 : _f.zoom),
        zoomReset: resolveBooleanCapability((_g = renderer.capabilities) === null || _g === void 0 ? void 0 : _g.zoom),
    };
    return {
        ...base,
        ...(_h = session === null || session === void 0 ? void 0 : session.getAvailability) === null || _h === void 0 ? void 0 : _h.call(session),
    };
};
export const applyFileViewerZoomAvailability = (availability, zoomState) => {
    const zoom = availability.zoom && (zoomState.canZoomIn || zoomState.canZoomOut || zoomState.canReset);
    return {
        ...availability,
        zoom,
        zoomIn: zoom && zoomState.canZoomIn,
        zoomOut: zoom && zoomState.canZoomOut,
        zoomReset: zoom && zoomState.canReset,
    };
};
export const createUnsupportedAvailability = (extension) => ({
    ...DEFAULT_OPERATION_AVAILABILITY,
    download: normalizeFileExtension(extension).length > 0,
});
/**
 * 这些格式只有专属适配器准备好后才展示打印。
 *
 * 它们的在线预览常依赖分页引擎、虚拟渲染或 Worker 生命周期，直接克隆
 * DOM 很容易只得到当前页或当前视口。
 */
export const ADAPTER_PRINT_REQUIRED_EXTENSIONS = [
    'docx', 'docm', 'dotx', 'dotm', 'doc', 'dot', 'ppt', 'pdf', 'typ', 'typst',
];
/**
 * 这些格式的预览结果是完整 DOM / SVG / Canvas 截图，解除滚动容器裁切后
 * 可以稳定进入浏览器打印流程。
 */
export const DOM_PRINTABLE_EXTENSIONS = [
    'pptx', 'pptm', 'potx', 'potm', 'ppsx', 'ppsm', 'ofd', 'dxf', 'dwg', 'dwf', 'dwfx', 'xps',
    'excalidraw', 'drawio', 'dio', 'mermaid', 'mmd', 'plantuml', 'puml', 'umd', 'md', 'markdown',
    'olb', 'dra',
    ...TEXT_EXTENSIONS,
    ...IMAGE_EXTENSIONS,
];
/**
 * 这些格式默认不展示打印按钮，避免导出半截内容。
 */
export const NON_PRINTABLE_EXTENSIONS = [
    'xlsx', 'xltx', 'xlsm', 'xlsb', 'xls', 'xlt', 'xltm', 'csv', 'tsv', 'ods', 'fods', 'numbers',
    ...ARCHIVE_EXTENSIONS,
    'eml', 'msg', 'epub', 'mp4',
    'mp3', 'mpeg', 'wav', 'ogg', 'oga', 'opus', 'm4a', 'aac', 'flac', 'weba',
    ...MODEL_EXTENSIONS,
];
const hasExtension = (items, extension) => {
    return items.includes(normalizeFileExtension(extension));
};
export const needsDedicatedPrintAdapter = (extension) => {
    return hasExtension(ADAPTER_PRINT_REQUIRED_EXTENSIONS, extension);
};
export const isDomPrintableExtension = (extension) => {
    return hasExtension(DOM_PRINTABLE_EXTENSIONS, extension);
};
export const isKnownNonPrintableExtension = (extension) => {
    return hasExtension(NON_PRINTABLE_EXTENSIONS, extension);
};
export const resolvePrintAvailability = (extension, adapter, renderedReady) => {
    if (!renderedReady) {
        return false;
    }
    if (adapter) {
        if (adapter.print === false) {
            return false;
        }
        if (adapter.toHtml) {
            return true;
        }
    }
    if (needsDedicatedPrintAdapter(extension) || isKnownNonPrintableExtension(extension)) {
        return false;
    }
    return isDomPrintableExtension(extension);
};
