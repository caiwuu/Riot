import { utils } from 'styled-exceljs';
import { getTintColor, indexedColors } from './color.js';
const EXCEL_DEFAULT_COLUMN_WIDTH = 8.43;
const EXCEL_DEFAULT_ROW_HEIGHT_PT = 15;
const EXCEL_DEFAULT_TEXT_COLOR = '#202124';
const EMU_PER_PIXEL = 9525;
const DEFAULT_IMAGE_WIDTH = 480;
const DEFAULT_IMAGE_HEIGHT = 288;
const AUTO_FIT_MIN_WIDTH = 24;
const AUTO_FIT_PADDING = 8;
const AUTO_FIT_MAX_SAMPLE_CELLS = 100000;
const AUTO_FIT_MAX_SAMPLE_WINDOWS = 8;
const AUTO_FIT_MAX_ROWS_PER_WINDOW = 128;
export const createAutoFitSampleRanges = (totalRowCount, totalColCount) => {
    const totalRows = Math.max(totalRowCount, 1);
    const totalCols = Math.max(totalColCount, 1);
    const endColumn = totalCols - 1;
    const totalCells = totalRows * totalCols;
    if (totalCells <= AUTO_FIT_MAX_SAMPLE_CELLS) {
        return [{
                s: { r: 0, c: 0 },
                e: { r: totalRows - 1, c: endColumn }
            }];
    }
    const rowsPerWindow = Math.max(1, Math.min(AUTO_FIT_MAX_ROWS_PER_WINDOW, Math.floor(AUTO_FIT_MAX_SAMPLE_CELLS / totalCols / AUTO_FIT_MAX_SAMPLE_WINDOWS)));
    const maxStartRow = Math.max(totalRows - rowsPerWindow, 0);
    const maxWindowsWithinBudget = Math.max(1, Math.floor(AUTO_FIT_MAX_SAMPLE_CELLS / totalCols / rowsPerWindow));
    const windowCount = Math.min(AUTO_FIT_MAX_SAMPLE_WINDOWS, maxWindowsWithinBudget, Math.ceil(totalRows / rowsPerWindow));
    const starts = new Set();
    for (let index = 0; index < windowCount; index += 1) {
        const startRow = windowCount === 1
            ? 0
            : Math.round(maxStartRow * index / (windowCount - 1));
        starts.add(startRow);
    }
    return Array.from(starts, startRow => ({
        s: { r: startRow, c: 0 },
        e: { r: Math.min(startRow + rowsPerWindow - 1, totalRows - 1), c: endColumn }
    }));
};
const toFiniteNumber = (value) => {
    return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
};
const getDefaultColumnWidth = () => {
    var _a;
    const converter = utils.col_width_to_px;
    const value = typeof converter === 'function' ? converter(EXCEL_DEFAULT_COLUMN_WIDTH) : undefined;
    return Math.ceil((_a = toFiniteNumber(value)) !== null && _a !== void 0 ? _a : 64);
};
const getDefaultRowHeight = () => {
    var _a;
    const converter = utils.row_height_to_px;
    const value = typeof converter === 'function' ? converter(EXCEL_DEFAULT_ROW_HEIGHT_PT) : undefined;
    return Math.ceil((_a = toFiniteNumber(value)) !== null && _a !== void 0 ? _a : 20);
};
export const defaults = {
    rowHeight: getDefaultRowHeight(),
    colWidth: getDefaultColumnWidth()
};
const cellKey = (row, col) => {
    return `${row}-${col}`;
};
const formatCellValue = (cell) => {
    if (!cell) {
        return '';
    }
    if (cell.w !== undefined && cell.w !== null) {
        return `${cell.w}`;
    }
    if (cell.v === undefined || cell.v === null) {
        return '';
    }
    if (cell.t === 'd' && cell.v instanceof Date) {
        return cell.v.toLocaleDateString();
    }
    return `${cell.v}`;
};
const getColumnPixelWidth = (column) => {
    if (!column) {
        return undefined;
    }
    if (column.hidden) {
        return 0;
    }
    // styled-exceljs 在 browserPixels 模式下会优先输出 wpx，这是最接近浏览器渲染的列宽。
    const wpx = toFiniteNumber(column.wpx);
    if (wpx !== undefined && wpx >= 0) {
        return Math.ceil(wpx);
    }
    const width = toFiniteNumber(column.width);
    if (width === 0) {
        return 0;
    }
    if (width !== undefined && width > 0) {
        const converter = utils.col_width_to_px;
        const converted = typeof converter === 'function' ? toFiniteNumber(converter(width)) : undefined;
        if (converted !== undefined) {
            return Math.ceil(converted);
        }
        return Math.ceil(width * (column.MDW || 7));
    }
    const wch = toFiniteNumber(column.wch);
    if (wch === 0) {
        return 0;
    }
    if (wch !== undefined && wch > 0) {
        return Math.ceil(wch * (column.MDW || 7) + 5);
    }
    return undefined;
};
const getVectorSize = (sizes, index, fallback) => {
    var _a;
    if (typeof sizes === 'number') {
        return sizes;
    }
    return (_a = sizes === null || sizes === void 0 ? void 0 : sizes[index]) !== null && _a !== void 0 ? _a : fallback;
};
const emuToPixels = (value) => {
    return (value || 0) / EMU_PER_PIXEL;
};
const hasColumnWidth = (column) => {
    var _a, _b, _c;
    if (!column) {
        return false;
    }
    if (column.hidden) {
        return true;
    }
    return (((_a = toFiniteNumber(column.wpx)) !== null && _a !== void 0 ? _a : -1) >= 0 ||
        ((_b = toFiniteNumber(column.width)) !== null && _b !== void 0 ? _b : -1) >= 0 ||
        ((_c = toFiniteNumber(column.wch)) !== null && _c !== void 0 ? _c : -1) >= 0);
};
const getRowPixelHeight = (rowMeta) => {
    if (!rowMeta) {
        return undefined;
    }
    if (rowMeta.hidden) {
        return 0;
    }
    const hpx = toFiniteNumber(rowMeta.hpx);
    if (hpx !== undefined && hpx >= 0) {
        return Math.ceil(hpx);
    }
    const hpt = toFiniteNumber(rowMeta.hpt);
    if (hpt !== undefined && hpt >= 0) {
        const converter = utils.row_height_to_px;
        const converted = typeof converter === 'function' ? toFiniteNumber(converter(hpt)) : undefined;
        return Math.ceil(converted !== null && converted !== void 0 ? converted : hpt * 96 / 72);
    }
    return undefined;
};
const normalizeHorizontalAlign = (value) => {
    switch (`${value || ''}`) {
        case 'left':
            return 'Left';
        case 'center':
        case 'centerContinuous':
        case 'distributed':
        case 'justify':
            return 'Center';
        case 'right':
            return 'Right';
        default:
            return undefined;
    }
};
const normalizeVerticalAlign = (value) => {
    switch (`${value || ''}`) {
        case 'top':
            return 'Top';
        case 'center':
        case 'middle':
        case 'distributed':
        case 'justify':
            return 'Middle';
        case 'bottom':
            return 'Bottom';
        default:
            return undefined;
    }
};
const alignToClassName = (alignment) => {
    if (!alignment) {
        return '';
    }
    const classNames = [
        normalizeHorizontalAlign(alignment.horizontal),
        normalizeVerticalAlign(alignment.vertical)
    ].filter(Boolean).map(value => `ht${value}`);
    if (alignment.wrapText) {
        classNames.push('htWrap');
    }
    if (alignment.shrinkToFit) {
        classNames.push('htShrink');
    }
    return classNames.join(' ');
};
const normalizeColor = (color) => {
    if (!color) {
        return undefined;
    }
    const tintedRgb = color.raw_rgb && typeof color.tint === 'number'
        ? getTintColor(color.raw_rgb, color.tint)
        : undefined;
    const rgb = color.rgb || tintedRgb || color.raw_rgb;
    if (typeof rgb === 'string' && rgb) {
        const clean = rgb.startsWith('#') ? rgb.slice(1) : rgb;
        const value = clean.length > 6 ? clean.slice(-6) : clean;
        return `#${value}`;
    }
    const indexed = typeof color.indexed === 'number' ? color.indexed : color.index;
    if (typeof indexed === 'number') {
        const value = indexedColors[indexed];
        if (value) {
            return `#${value.slice(-6)}`;
        }
    }
    return undefined;
};
const isAutomaticPaletteColor = (color) => {
    if (!color) {
        return false;
    }
    const indexed = typeof color.indexed === 'number' ? color.indexed : color.index;
    return indexed === 32767;
};
const normalizeFontColor = (color) => {
    // BIFF/XLS 会把“自动字体色”解析成 indexed 32767，部分文件还会附带
    // FFFFFF 的 rgb 值。Excel 实际显示为默认黑色，不能当成显式白色。
    if (isAutomaticPaletteColor(color)) {
        return EXCEL_DEFAULT_TEXT_COLOR;
    }
    return normalizeColor(color);
};
const borderWidthFromStyle = (borderStyle) => {
    switch (borderStyle) {
        case 'hair':
            return '0.5px';
        case 'medium':
        case 'mediumDashed':
        case 'mediumDashDot':
        case 'mediumDashDotDot':
            return '2px';
        case 'thick':
        case 'double':
            return '3px';
        default:
            return '1px';
    }
};
const borderStyleToCss = (borderStyle) => {
    switch (borderStyle) {
        case 'dashed':
        case 'mediumDashed':
        case 'dashDot':
        case 'mediumDashDot':
        case 'dashDotDot':
        case 'mediumDashDotDot':
        case 'slantDashDot':
            return 'dashed';
        case 'dotted':
            return 'dotted';
        case 'double':
            return 'double';
        default:
            return 'solid';
    }
};
const mergeStyle = (...styles) => {
    const result = {};
    styles.forEach((style) => {
        if (!style) {
            return;
        }
        Object.entries(style).forEach(([key, value]) => {
            if (value && typeof value === 'object' && !Array.isArray(value)) {
                result[key] = {
                    ...(result[key] && typeof result[key] === 'object' ? result[key] : {}),
                    ...value
                };
                return;
            }
            result[key] = value;
        });
    });
    return Object.keys(result).length ? result : undefined;
};
const getCellStyle = (cellStyle) => {
    const style = {};
    const fill = (cellStyle === null || cellStyle === void 0 ? void 0 : cellStyle.fill) || {};
    const fillColor = normalizeColor((cellStyle === null || cellStyle === void 0 ? void 0 : cellStyle.fgColor) || fill.fgColor || (cellStyle === null || cellStyle === void 0 ? void 0 : cellStyle.bgColor) || fill.bgColor);
    const patternType = (cellStyle === null || cellStyle === void 0 ? void 0 : cellStyle.patternType) || fill.patternType;
    if (fillColor && patternType !== 'none') {
        style.backgroundColor = fillColor;
    }
    const font = (cellStyle === null || cellStyle === void 0 ? void 0 : cellStyle.font) || {};
    const fontColor = normalizeFontColor(font.color || (cellStyle === null || cellStyle === void 0 ? void 0 : cellStyle.color));
    if (fontColor) {
        style.color = fontColor;
    }
    if (font.italic || (cellStyle === null || cellStyle === void 0 ? void 0 : cellStyle.italic)) {
        style.fontStyle = 'italic';
    }
    if (font.bold || (cellStyle === null || cellStyle === void 0 ? void 0 : cellStyle.bold)) {
        style.fontWeight = 'bold';
    }
    const fontSize = font.sz || (cellStyle === null || cellStyle === void 0 ? void 0 : cellStyle.sz);
    if (fontSize) {
        style.fontSize = `${fontSize}px`;
    }
    const fontName = font.name || (cellStyle === null || cellStyle === void 0 ? void 0 : cellStyle.name);
    if (fontName) {
        style.fontFamily = fontName;
    }
    const border = cellStyle === null || cellStyle === void 0 ? void 0 : cellStyle.border;
    if (border) {
        ;
        ['top', 'right', 'bottom', 'left'].forEach((side) => {
            const borderItem = border[side];
            if (!(borderItem === null || borderItem === void 0 ? void 0 : borderItem.style) || borderItem.style === 'none') {
                return;
            }
            const prefix = `border${side.charAt(0).toUpperCase()}${side.slice(1)}`;
            style[`${prefix}Width`] = borderWidthFromStyle(borderItem.style);
            style[`${prefix}Style`] = borderStyleToCss(borderItem.style);
            style[`${prefix}Color`] = normalizeColor(borderItem.color) || '#000000';
        });
    }
    return Object.keys(style).length ? style : undefined;
};
const fixMatrix = (data, colLen) => {
    for (const row of data) {
        for (let index = 0; index < colLen; index += 1) {
            if (row[index] === undefined || row[index] === null) {
                row[index] = '';
            }
        }
    }
    return data;
};
class SheetJsModel {
    static create(ws, options = {}) {
        return new SheetJsModel(ws, options);
    }
    constructor(ws, options) {
        this._ws = ws;
        this._startRow = Math.max(options.startRow || 0, 0);
        this._pageSize = Math.max(options.pageSize || 500, 1);
        this._totalRows = options.totalRows;
        this._totalCols = options.totalCols;
        this._charts = options.charts || [];
        this._cellImages = options.cellImages || [];
        this._cellImageKeys = new Set(this._cellImages.map((image) => cellKey(image.row, image.col)));
        const { '!ref': refs } = ws;
        this.range = utils.decode_range(refs || 'A1');
    }
    get ws() {
        return this._ws;
    }
    get defaults() {
        return SheetJsModel.defaults;
    }
    get data() {
        var _a;
        return (_a = this._data) !== null && _a !== void 0 ? _a : (this._data = this.getData());
    }
    get cell() {
        var _a;
        return (_a = this._cell) !== null && _a !== void 0 ? _a : (this._cell = this.getCell());
    }
    get merge() {
        var _a;
        return (_a = this._merge) !== null && _a !== void 0 ? _a : (this._merge = this.getMerge());
    }
    get columns() {
        var _a;
        return (_a = this._columns) !== null && _a !== void 0 ? _a : (this._columns = this.getColumns());
    }
    get structure() {
        var _a;
        return (_a = this._structure) !== null && _a !== void 0 ? _a : (this._structure = this.getStructure());
    }
    get rowHeights() {
        var _a;
        return (_a = this._rowHeights) !== null && _a !== void 0 ? _a : (this._rowHeights = this.getRowHeights());
    }
    get colWidths() {
        var _a;
        return (_a = this._colWidths) !== null && _a !== void 0 ? _a : (this._colWidths = this.getColWidths());
    }
    get meta() {
        var _a;
        return (_a = this._meta) !== null && _a !== void 0 ? _a : (this._meta = {
            startRow: this.startRow,
            endRow: this.endRow,
            pageSize: this._pageSize,
            totalRows: this.totalRows,
            totalCols: this.totalCols
        });
    }
    get totalRows() {
        var _a;
        return (_a = this._totalRows) !== null && _a !== void 0 ? _a : this.range.e.r + 1;
    }
    get totalCols() {
        var _a;
        return (_a = this._totalCols) !== null && _a !== void 0 ? _a : this.range.e.c + 1;
    }
    get startRow() {
        return Math.min(this._startRow, Math.max(this.totalRows - 1, 0));
    }
    get endRow() {
        return Math.min(this.startRow + this._pageSize, this.totalRows);
    }
    get denseRows() {
        const worksheet = this.ws;
        if (Array.isArray(worksheet)) {
            return worksheet;
        }
        return Array.isArray(worksheet['!data']) ? worksheet['!data'] : undefined;
    }
    getCellAt(row, col) {
        var _a;
        const rows = this.denseRows;
        if (rows) {
            return (_a = rows[row]) === null || _a === void 0 ? void 0 : _a[col];
        }
        return this.ws[utils.encode_cell({ r: row, c: col })];
    }
    getAxisOffset(index, sizes, fallback) {
        let offset = 0;
        for (let current = 0; current < index; current += 1) {
            offset += getVectorSize(sizes, current, fallback);
        }
        return offset;
    }
    getMarkerLeft(marker) {
        if (!marker) {
            return 0;
        }
        return this.getAxisOffset(marker.col || 0, this.getColWidths(), this.defaults.colWidth) + emuToPixels(marker.colOff);
    }
    getMarkerTop(marker) {
        if (!marker) {
            return 0;
        }
        return this.getAxisOffset(marker.row || 0, this.getAllRowHeights(), this.defaults.rowHeight) + emuToPixels(marker.rowOff);
    }
    getImages() {
        const drawings = this.ws['!drawings'];
        const images = (drawings === null || drawings === void 0 ? void 0 : drawings.images) || [];
        if (!images.length && !this._cellImages.length) {
            return undefined;
        }
        const drawingImages = images.flatMap((image, index) => {
            var _a, _b, _c, _d, _e;
            const anchor = image.anchor;
            if (!image.dataURI || !anchor) {
                return [];
            }
            const from = anchor.from;
            const left = from ? this.getMarkerLeft(from) : emuToPixels((_a = anchor.pos) === null || _a === void 0 ? void 0 : _a.x);
            const top = from ? this.getMarkerTop(from) : emuToPixels((_b = anchor.pos) === null || _b === void 0 ? void 0 : _b.y);
            const right = anchor.to ? this.getMarkerLeft(anchor.to) : left + emuToPixels((_c = anchor.ext) === null || _c === void 0 ? void 0 : _c.cx);
            const bottom = anchor.to ? this.getMarkerTop(anchor.to) : top + emuToPixels((_d = anchor.ext) === null || _d === void 0 ? void 0 : _d.cy);
            return [{
                    id: image.id || ((_e = image.objectId) === null || _e === void 0 ? void 0 : _e.toString()) || image.target || `image-${index + 1}`,
                    src: image.dataURI,
                    contentType: image.contentType,
                    left: Math.max(0, left),
                    top: Math.max(0, top),
                    width: Math.max(1, right > left ? right - left : DEFAULT_IMAGE_WIDTH),
                    height: Math.max(1, bottom > top ? bottom - top : DEFAULT_IMAGE_HEIGHT),
                    row: (from === null || from === void 0 ? void 0 : from.row) || 0,
                    col: (from === null || from === void 0 ? void 0 : from.col) || 0
                }];
        });
        const cellImages = this._cellImages.flatMap((image) => {
            const width = getVectorSize(this.getColWidths(), image.col, this.defaults.colWidth);
            const height = getVectorSize(this.getAllRowHeights(), image.row, this.defaults.rowHeight);
            if (width <= 0 || height <= 0) {
                return [];
            }
            return [{
                    ...image,
                    left: this.getAxisOffset(image.col, this.getColWidths(), this.defaults.colWidth),
                    top: this.getAxisOffset(image.row, this.getAllRowHeights(), this.defaults.rowHeight),
                    width,
                    height
                }];
        });
        const result = [...drawingImages, ...cellImages];
        return result.length ? result : undefined;
    }
    getCharts() {
        const result = this._charts.map((chart) => {
            var _a, _b;
            const left = this.getMarkerLeft(chart.from);
            const top = this.getMarkerTop(chart.from);
            const right = chart.to
                ? this.getMarkerLeft(chart.to)
                : left + emuToPixels((_a = chart.ext) === null || _a === void 0 ? void 0 : _a.width);
            const bottom = chart.to
                ? this.getMarkerTop(chart.to)
                : top + emuToPixels((_b = chart.ext) === null || _b === void 0 ? void 0 : _b.height);
            return {
                id: chart.id,
                type: chart.type,
                title: chart.title,
                categoryAxisTitle: chart.categoryAxisTitle,
                valueAxisTitle: chart.valueAxisTitle,
                barDirection: chart.barDirection,
                grouping: chart.grouping,
                legendPosition: chart.legendPosition,
                series: chart.series,
                left: Math.max(0, left),
                top: Math.max(0, top),
                width: Math.max(1, right > left ? right - left : DEFAULT_IMAGE_WIDTH),
                height: Math.max(1, bottom > top ? bottom - top : DEFAULT_IMAGE_HEIGHT),
                row: chart.from.row,
                col: chart.from.col
            };
        });
        return result.length ? result : undefined;
    }
    getAllMerge() {
        const sheet = this.ws;
        const { '!merges': merges = [] } = sheet;
        return merges.map((merge) => {
            const { r: top, c: left } = merge.s;
            const { r: bottom, c: right } = merge.e;
            return {
                row: top,
                col: left,
                rowspan: bottom - top + 1,
                colspan: right - left + 1
            };
        });
    }
    getData() {
        const result = [];
        const rows = this.denseRows;
        for (let rowIndex = this.startRow; rowIndex < this.endRow; rowIndex += 1) {
            const row = rows === null || rows === void 0 ? void 0 : rows[rowIndex];
            const values = row
                ? row.slice(0, this.totalCols).map((cell, colIndex) => (this._cellImageKeys.has(cellKey(rowIndex, colIndex)) ? '' : formatCellValue(cell)))
                : Array.from({ length: this.totalCols }, (_, colIndex) => (this._cellImageKeys.has(cellKey(rowIndex, colIndex))
                    ? ''
                    : formatCellValue(this.getCellAt(rowIndex, colIndex))));
            result.push(values);
        }
        return fixMatrix(result, this.totalCols);
    }
    getCell() {
        var _a, _b;
        const result = {};
        const { '!cols': cols = [], '!rows': rows = [] } = this.ws;
        for (let rowIndex = this.startRow; rowIndex < this.endRow; rowIndex += 1) {
            for (let colIndex = 0; colIndex < this.totalCols; colIndex += 1) {
                const cell = this.getCellAt(rowIndex, colIndex);
                const rawStyle = mergeStyle((_a = cols[colIndex]) === null || _a === void 0 ? void 0 : _a.s, (_b = rows[rowIndex]) === null || _b === void 0 ? void 0 : _b.s, cell === null || cell === void 0 ? void 0 : cell.s);
                const className = alignToClassName(rawStyle === null || rawStyle === void 0 ? void 0 : rawStyle.alignment);
                const style = getCellStyle(rawStyle);
                if (!className && !style) {
                    continue;
                }
                result[cellKey(rowIndex - this.startRow, colIndex)] = {
                    ...(className ? { className } : {}),
                    style: style || {}
                };
            }
        }
        return result;
    }
    getMerge() {
        return this.getAllMerge().flatMap((merge) => {
            const bottom = merge.row + merge.rowspan - 1;
            if (bottom < this.startRow || merge.row >= this.endRow || merge.row < this.startRow) {
                return [];
            }
            return {
                ...merge,
                row: merge.row - this.startRow
            };
        });
    }
    getRowHeights() {
        const { rowHeight } = this.defaults;
        const { '!rows': rows = [] } = this.ws;
        const heights = [];
        if (rows.length && this.endRow > this.startRow) {
            for (let absoluteRow = this.startRow; absoluteRow < this.endRow; absoluteRow += 1) {
                const height = getRowPixelHeight(rows[absoluteRow]);
                if (height !== undefined) {
                    heights[absoluteRow - this.startRow] = height;
                }
            }
        }
        if (heights.length === 1) {
            return heights[0];
        }
        return heights.length ? heights : rowHeight;
    }
    // 整表行高必须按绝对行号下发，否则拖动滚动条时隐藏行和特殊行高会造成高度跳变。
    getAllRowHeights() {
        const { '!rows': rows = [] } = this.ws;
        const heights = [];
        if (rows.length) {
            for (let absoluteRow = 0; absoluteRow < this.totalRows; absoluteRow += 1) {
                const height = getRowPixelHeight(rows[absoluteRow]);
                if (height !== undefined) {
                    heights[absoluteRow] = height;
                }
            }
        }
        return heights.length ? heights : undefined;
    }
    getAutoFitColumns() {
        var _a, _b;
        const autoFitColumns = utils.auto_fit_columns || utils.autofit_columns;
        if (typeof autoFitColumns !== 'function') {
            return undefined;
        }
        try {
            const sampleRanges = this.getAutoFitSampleRanges();
            const fittedColumns = [];
            for (const range of sampleRanges) {
                // 自动列宽只作为缺失列宽的兜底。合并标题和 Excel 溢出文本不应该反向撑开基础列宽。
                // 大表只抽样固定数量的单元格，避免为了首屏列宽扫描整张工作表。
                const measuredColumns = autoFitColumns(this.ws, {
                    range,
                    set: false,
                    skipHidden: true,
                    includeMerged: false,
                    minPx: AUTO_FIT_MIN_WIDTH,
                    padding: AUTO_FIT_PADDING
                });
                for (let colIndex = 0; colIndex < this.totalCols; colIndex += 1) {
                    const measured = measuredColumns === null || measuredColumns === void 0 ? void 0 : measuredColumns[colIndex];
                    if (!measured) {
                        continue;
                    }
                    const currentWidth = (_a = getColumnPixelWidth(fittedColumns[colIndex])) !== null && _a !== void 0 ? _a : -1;
                    const measuredWidth = (_b = getColumnPixelWidth(measured)) !== null && _b !== void 0 ? _b : -1;
                    if (!fittedColumns[colIndex] || measuredWidth > currentWidth) {
                        fittedColumns[colIndex] = measured;
                    }
                }
            }
            return fittedColumns.length ? fittedColumns : undefined;
        }
        catch (error) {
            console.warn('[file-viewer] Excel 自动列宽计算失败，已回退到原始列宽。', error);
            return undefined;
        }
    }
    getAutoFitSampleRanges() {
        return createAutoFitSampleRanges(this.totalRows, this.totalCols);
    }
    get autoFitColumns() {
        if (this._autoFitColumns === undefined) {
            this._autoFitColumns = this.getAutoFitColumns() || null;
        }
        return this._autoFitColumns || undefined;
    }
    getColumnMeta(sourceCols, colIndex) {
        var _a;
        const sourceColumn = sourceCols[colIndex];
        if (hasColumnWidth(sourceColumn)) {
            return sourceColumn;
        }
        // A drawing anchor is measured against the workbook's stored/default
        // column geometry. Auto-fitting blank columns to 24px changes that
        // coordinate system and makes two-cell images visibly smaller than Excel.
        if (this.hasAnchoredObjects) {
            return sourceColumn;
        }
        // 没有显式列宽时，再使用 styled-exceljs 的内容测量兜底，避免报表类标题污染原始宽度。
        return ((_a = this.autoFitColumns) === null || _a === void 0 ? void 0 : _a[colIndex]) || sourceColumn;
    }
    get hasAnchoredObjects() {
        var _a;
        const drawings = this.ws['!drawings'];
        return !!((_a = drawings === null || drawings === void 0 ? void 0 : drawings.images) === null || _a === void 0 ? void 0 : _a.length) || !!this._charts.length || !!this._cellImages.length;
    }
    getColWidths() {
        const { colWidth } = this.defaults;
        const { '!cols': sourceCols = [] } = this.ws;
        const widths = [];
        for (let colIndex = 0; colIndex < this.totalCols; colIndex += 1) {
            const width = getColumnPixelWidth(this.getColumnMeta(sourceCols, colIndex));
            if (width !== undefined) {
                widths[colIndex] = width;
            }
        }
        return widths.length ? widths : colWidth;
    }
    getColumns() {
        const { '!cols': sourceCols = [] } = this.ws;
        return Array.from({ length: this.totalCols }, (_, index) => {
            var _a;
            const column = this.getColumnMeta(sourceCols, index);
            return {
                key: index + 1,
                title: utils.encode_col(index),
                hidden: !!(column === null || column === void 0 ? void 0 : column.hidden),
                editor: false,
                className: alignToClassName((_a = column === null || column === void 0 ? void 0 : column.s) === null || _a === void 0 ? void 0 : _a.alignment),
                renderer: 'styleRender'
            };
        });
    }
    getStructure() {
        return {
            merge: this.getAllMerge(),
            colWidths: this.getColWidths(),
            rowHeights: this.getAllRowHeights(),
            columns: this.getColumns(),
            images: this.getImages(),
            charts: this.getCharts()
        };
    }
    toObject(options = {}) {
        const { defaults, data, cell, merge, rowHeights, meta } = this;
        return {
            defaults,
            data,
            cell,
            merge,
            rowHeights,
            ...(options.includeLayout === false ? {} : {
                colWidths: this.colWidths,
                columns: this.columns
            }),
            meta
        };
    }
}
SheetJsModel.defaults = defaults;
export default SheetJsModel;
