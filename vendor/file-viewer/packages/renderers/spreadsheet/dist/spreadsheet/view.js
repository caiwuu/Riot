import tinycolor from 'tinycolor2';
import { displayCellKey, getDataKey, getRowState, INDEX_COLUMN_KEY, ROW_KEY_FIELD, RowState } from './state.js';
export const INDEX_COLUMN_WIDTH = 68;
export const SPREADSHEET_MIN_ZOOM = 0.25;
export const SPREADSHEET_MAX_ZOOM = 2.5;
const TABLE_FONT_FAMILY = 'Aptos, Calibri, "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif';
const TABLE_FONT_SIZE = 11;
const MIN_RENDERABLE_ROW_HEIGHT = 8;
const HIDDEN_ROW_HEIGHT = 0.1;
const CELL_PADDING = 2;
const CELL_LINE_HEIGHT = 1.2;
export const HEADER_HEIGHT = 34;
export const RESIZABLE_COLUMN_MIN_WIDTH = 40;
export const RESIZABLE_ROW_MIN_HEIGHT = 18;
const EXCEL_HEADER_BG = '#f3f3f3';
const EXCEL_HEADER_TEXT = '#5f6368';
const EXCEL_GRID = '#d7dbe0';
const EXCEL_DARK_HEADER_BG = '#1f2937';
const EXCEL_DARK_HEADER_TEXT = '#cbd5e1';
const EXCEL_DARK_GRID = '#374151';
const EXCEL_DARK_BODY_BG = '#111827';
const EXCEL_DARK_BODY_TEXT = '#e5e7eb';
const EXCEL_GREEN = '#21a366';
const EXCEL_GREEN_SOFT = 'rgba(33, 163, 102, 0.1)';
const DATA_AREA_MIN_COL_INDEX = 1;
const _BORDER_SIDE_KEYS = ['Top', 'Right', 'Bottom', 'Left'];
const DEFAULT_BORDER_COLOR = '#000000';
const BORDER_STYLE_PRIORITY = {
    double: 8,
    solid: 7,
    dashed: 5,
    dotted: 4
};
const HEADER_FONT = `bold 12px ${TABLE_FONT_FAMILY}`;
const BODY_FONT = `${TABLE_FONT_SIZE}px ${TABLE_FONT_FAMILY}`;
const MAX_OVERFLOW_SCAN_COLS = 32;
let measureCanvas;
const textWidthCache = new Map();
const normalizeZoomScale = (scale = 1) => {
    return Number.isFinite(scale)
        ? Math.min(SPREADSHEET_MAX_ZOOM, Math.max(SPREADSHEET_MIN_ZOOM, scale))
        : 1;
};
const scaleNumber = (value, zoomScale) => {
    return Math.max(1, Math.round(value * normalizeZoomScale(zoomScale)));
};
const scaleFont = (font, zoomScale) => {
    if (!font) {
        return font;
    }
    const normalizedScale = normalizeZoomScale(zoomScale);
    return font.replace(/(\d+(?:\.\d+)?)px/g, (_, size) => {
        return `${Number(size) * normalizedScale}px`;
    });
};
const scaleBorder = (border, zoomScale) => {
    if (!border) {
        return undefined;
    }
    return {
        ...border,
        width: Math.max(0.5, border.width * normalizeZoomScale(zoomScale))
    };
};
const scaleCellStyle = (style, zoomScale) => {
    if (!style) {
        return undefined;
    }
    return {
        ...style,
        font: scaleFont(style.font, zoomScale),
        borderTop: scaleBorder(style.borderTop, zoomScale),
        borderRight: scaleBorder(style.borderRight, zoomScale),
        borderBottom: scaleBorder(style.borderBottom, zoomScale),
        borderLeft: scaleBorder(style.borderLeft, zoomScale)
    };
};
const getHeaderFont = (zoomScale) => scaleFont(HEADER_FONT, zoomScale) || HEADER_FONT;
const getBodyFont = (zoomScale) => scaleFont(BODY_FONT, zoomScale) || BODY_FONT;
const getIndexCellStyle = (zoomScale, darkMode = false) => ({
    backgroundColor: darkMode ? EXCEL_DARK_HEADER_BG : EXCEL_HEADER_BG,
    color: darkMode ? EXCEL_DARK_HEADER_TEXT : EXCEL_HEADER_TEXT,
    font: getHeaderFont(zoomScale)
});
const getLoadingCellStyle = (zoomScale, darkMode = false) => ({
    backgroundColor: darkMode ? '#18212f' : '#f4f7f9',
    color: darkMode ? '#94a3b8' : '#73808d',
    font: scaleFont(`italic ${TABLE_FONT_SIZE}px ${TABLE_FONT_FAMILY}`, zoomScale)
});
const cloneColumns = (columns) => {
    return columns.map(column => {
        var _a;
        return ({
            ...column,
            ...(((_a = column.children) === null || _a === void 0 ? void 0 : _a.length) ? { children: cloneColumns(column.children) } : {})
        });
    });
};
const buildFont = (style) => {
    if (typeof style.font === 'string' && style.font.trim()) {
        return style.font;
    }
    const hasFont = style.fontFamily !== undefined ||
        style.fontSize !== undefined ||
        style.fontWeight !== undefined ||
        style.fontStyle !== undefined;
    if (!hasFont) {
        return undefined;
    }
    const normalizedWeight = (() => {
        const { fontWeight } = style;
        if (typeof fontWeight === 'number') {
            return fontWeight >= 600 ? 'bold' : 'normal';
        }
        if (typeof fontWeight === 'string') {
            const trimmed = fontWeight.trim();
            if (/^\d+$/.test(trimmed)) {
                return Number(trimmed) >= 600 ? 'bold' : 'normal';
            }
            return trimmed;
        }
        return 'normal';
    })();
    return [
        style.fontStyle || 'normal',
        normalizedWeight,
        `${Number.parseInt(style.fontSize, 10) || TABLE_FONT_SIZE}px`,
        style.fontFamily || TABLE_FONT_FAMILY
    ].join(' ');
};
const alignFromClassName = (className) => {
    if (!className) {
        return undefined;
    }
    if (className.includes('htLeft')) {
        return 'left';
    }
    if (className.includes('htRight')) {
        return 'right';
    }
    if (className.includes('htCenter')) {
        return 'center';
    }
    return undefined;
};
const verticalAlignFromClassName = (className) => {
    if (!className) {
        return undefined;
    }
    if (className.includes('htTop')) {
        return 'top';
    }
    if (className.includes('htBottom')) {
        return 'bottom';
    }
    if (className.includes('htMiddle')) {
        return 'middle';
    }
    return undefined;
};
const hasClassName = (className, value) => {
    return !!(className === null || className === void 0 ? void 0 : className.split(/\s+/).includes(value));
};
const getColumnWidth = (widths, index, fallback) => {
    var _a;
    if (typeof widths === 'number') {
        return widths;
    }
    return (_a = widths === null || widths === void 0 ? void 0 : widths[index]) !== null && _a !== void 0 ? _a : fallback;
};
const getRenderColumnWidth = (column) => {
    const width = Number(column === null || column === void 0 ? void 0 : column.width);
    return Number.isFinite(width) && width > 0 ? width : 0;
};
const isVisibleDataColumn = (column) => {
    return !!column && !column.hide && getRenderColumnWidth(column) > 0;
};
const getSheetColumnsMeta = (ws) => {
    var _a, _b;
    return ((_b = (_a = ws.structure) === null || _a === void 0 ? void 0 : _a.columns) === null || _b === void 0 ? void 0 : _b.length) ? ws.structure.columns : ws.columns;
};
const getSheetColWidths = (ws) => {
    var _a, _b;
    return (_b = (_a = ws.structure) === null || _a === void 0 ? void 0 : _a.colWidths) !== null && _b !== void 0 ? _b : ws.colWidths;
};
export const getRowHeight = (heights, index, fallback) => {
    var _a;
    if (typeof heights === 'number') {
        return heights;
    }
    return (_a = heights === null || heights === void 0 ? void 0 : heights[index]) !== null && _a !== void 0 ? _a : fallback;
};
// Excel 已经给出了行高时，预览层不再二次猜测。
// 只有完全缺失时才回退到默认行高，同时给极小值留一个最小可渲染下限。
export const normalizeRowHeight = (height, fallback) => {
    if (typeof height === 'number' && Number.isFinite(height)) {
        if (height <= 0) {
            // e-virt-table 内部用 item._height || CELL_HEIGHT，0 会被误判为未设置。
            return HIDDEN_ROW_HEIGHT;
        }
        return Math.max(Math.ceil(height), MIN_RENDERABLE_ROW_HEIGHT);
    }
    return Math.max(Math.ceil(fallback), MIN_RENDERABLE_ROW_HEIGHT);
};
const isPlainTextCell = (value) => {
    if (value === null || value === undefined) {
        return false;
    }
    const text = `${value}`.trim();
    if (!text) {
        return false;
    }
    return !/^\d+([./:-]\d+)*$/.test(text);
};
// 普通明细表通常会把第一行当成字段表头，而报表类工作表往往需要保留原始 Excel 行号。
// 这里只在“首行像简单字段头且没有合并单元格”时，才给左侧序号做 1 行偏移。
export const detectIndexOffset = (ws) => {
    const meta = ws.meta;
    const rows = ws.data || [];
    if (!meta || rows.length < 2 || (ws.merge || []).length) {
        return 0;
    }
    const firstRow = rows[0] || [];
    const secondRow = rows[1] || [];
    const firstRowValues = firstRow.filter(value => `${value !== null && value !== void 0 ? value : ''}`.trim() !== '');
    const secondRowValues = secondRow.filter(value => `${value !== null && value !== void 0 ? value : ''}`.trim() !== '');
    if (firstRowValues.length < Math.min(Math.max(Math.floor(meta.totalCols / 2), 3), 6)) {
        return 0;
    }
    if (!firstRowValues.every(isPlainTextCell) || secondRowValues.length < 2) {
        return 0;
    }
    return 1;
};
export const buildColumns = (ws) => {
    var _a;
    const columnsMeta = getSheetColumnsMeta(ws);
    const colWidths = getSheetColWidths(ws);
    const columns = [{
            key: INDEX_COLUMN_KEY,
            title: '',
            type: 'index',
            fixed: 'left',
            width: INDEX_COLUMN_WIDTH,
            minWidth: INDEX_COLUMN_WIDTH,
            maxWidth: INDEX_COLUMN_WIDTH,
            widthFillDisable: true,
            headerAlign: 'center',
            align: 'center',
            headerVerticalAlign: 'middle',
            verticalAlign: 'middle',
            overflowTooltipShow: false,
            overflowTooltipHeaderShow: false
        }];
    const dataKeys = [];
    for (let index = 0; index < (((_a = ws.meta) === null || _a === void 0 ? void 0 : _a.totalCols) || 0); index += 1) {
        const key = getDataKey(index);
        const column = columnsMeta === null || columnsMeta === void 0 ? void 0 : columnsMeta[index];
        const width = getColumnWidth(colWidths, index, ws.defaults.colWidth);
        const hidden = !!(column === null || column === void 0 ? void 0 : column.hidden) || width <= 0;
        const columnVerticalAlign = verticalAlignFromClassName(column === null || column === void 0 ? void 0 : column.className) || 'middle';
        dataKeys.push(key);
        columns.push({
            key,
            title: (column === null || column === void 0 ? void 0 : column.title) || `${index + 1}`,
            width: hidden ? 0 : Math.max(Math.ceil(width), 1),
            hide: hidden,
            widthFillDisable: true,
            renderType: 'both',
            headerAlign: 'center',
            headerVerticalAlign: 'middle',
            align: alignFromClassName(column === null || column === void 0 ? void 0 : column.className) || 'left',
            verticalAlign: columnVerticalAlign,
            overflowTooltipShow: true,
            overflowTooltipHeaderShow: true
        });
    }
    return { columns, dataKeys };
};
const scaleColumn = (column, zoomScale) => {
    var _a;
    const nextColumn = {
        ...column,
        ...(((_a = column.children) === null || _a === void 0 ? void 0 : _a.length) ? { children: column.children.map(child => scaleColumn(child, zoomScale)) } : {})
    };
    const width = Number(column.width);
    if (Number.isFinite(width) && width > 0) {
        ;
        nextColumn.width = scaleNumber(width, zoomScale);
    }
    const minWidth = Number(column.minWidth);
    if (Number.isFinite(minWidth) && minWidth > 0) {
        ;
        nextColumn.minWidth = scaleNumber(minWidth, zoomScale);
    }
    const maxWidth = Number(column.maxWidth);
    if (Number.isFinite(maxWidth) && maxWidth > 0) {
        ;
        nextColumn.maxWidth = scaleNumber(maxWidth, zoomScale);
    }
    return nextColumn;
};
// Excel 预览优先忠实还原原始列宽，不再为了“铺满容器”二次拉伸列宽。
export const getDisplayColumns = (columns, zoomScale = 1) => {
    const normalizedScale = normalizeZoomScale(zoomScale);
    if (normalizedScale === 1) {
        return cloneColumns(columns);
    }
    return columns.map(column => scaleColumn(column, normalizedScale));
};
const toBorderWidth = (value) => {
    if (typeof value === 'number' && Number.isFinite(value)) {
        return Math.max(value, 0);
    }
    if (typeof value === 'string') {
        const parsed = Number.parseFloat(value);
        return Number.isFinite(parsed) ? Math.max(parsed, 0) : 0;
    }
    return 0;
};
const toBorderStyle = (value) => {
    if (typeof value !== 'string') {
        return 'solid';
    }
    const style = value.trim();
    switch (style) {
        case 'double':
        case 'dashed':
        case 'dotted':
        case 'solid':
            return style;
        default:
            return 'solid';
    }
};
const getBorderSide = (style, side) => {
    const width = toBorderWidth(style[`border${side}Width`]);
    if (!width) {
        return undefined;
    }
    return {
        width,
        style: toBorderStyle(style[`border${side}Style`]),
        color: style[`border${side}Color`] || DEFAULT_BORDER_COLOR
    };
};
const hasBorder = (style) => {
    if (!style) {
        return false;
    }
    return !!(style.borderTop || style.borderRight || style.borderBottom || style.borderLeft);
};
const hasTextLayout = (style) => {
    if (!style) {
        return false;
    }
    return !!(style.horizontalAlign || style.verticalAlign || style.wrapText || style.shrinkToFit);
};
const shouldRenderTextInOverlay = (style, column) => {
    if (!hasTextLayout(style)) {
        return false;
    }
    // e-virt-table 的列级 align 可以交给 canvas；只有单元格级布局差异才用 DOM 覆盖层补齐。
    return !!((style === null || style === void 0 ? void 0 : style.wrapText) ||
        (style === null || style === void 0 ? void 0 : style.shrinkToFit) ||
        ((style === null || style === void 0 ? void 0 : style.horizontalAlign) && style.horizontalAlign !== ((column === null || column === void 0 ? void 0 : column.align) || 'left')) ||
        ((style === null || style === void 0 ? void 0 : style.verticalAlign) && style.verticalAlign !== ((column === null || column === void 0 ? void 0 : column.verticalAlign) || 'middle')));
};
const toFlexJustify = (align) => {
    switch (align) {
        case 'center':
            return 'center';
        case 'right':
            return 'flex-end';
        default:
            return 'flex-start';
    }
};
const toFlexAlign = (verticalAlign) => {
    switch (verticalAlign) {
        case 'top':
            return 'flex-start';
        case 'bottom':
            return 'flex-end';
        default:
            return 'center';
    }
};
const toTextLayerLeft = (align, ownWidth, overflowWidth, leftRoom) => {
    switch (align) {
        case 'right':
            return ownWidth - overflowWidth;
        case 'center':
            return -Math.min(leftRoom, Math.max((overflowWidth - ownWidth) / 2, 0));
        default:
            return 0;
    }
};
const isHiddenVirtualRow = (row) => {
    return typeof row._height === 'number' && row._height <= HIDDEN_ROW_HEIGHT;
};
const getDataColumnIndex = (column, fallbackColIndex) => {
    const key = column === null || column === void 0 ? void 0 : column.key;
    if (typeof key === 'string' && key.startsWith('c')) {
        const index = Number(key.slice(1));
        if (Number.isInteger(index) && index >= 0) {
            return index;
        }
    }
    return Math.max(fallbackColIndex - 1, 0);
};
const getCellCacheKey = (rowIndex, colIndex, column) => {
    return displayCellKey(rowIndex, getDataColumnIndex(column, colIndex) + 1);
};
const getCellKeyByDataColumn = (rowIndex, dataColIndex) => {
    return displayCellKey(rowIndex, dataColIndex + 1);
};
const getTextValue = (value) => {
    if (value === null || value === undefined) {
        return '';
    }
    return `${value}`;
};
const hasRenderableValue = (value) => {
    return getTextValue(value).trim() !== '';
};
const isNumericLikeValue = (text) => {
    return /^[+-]?(?:\d+|\d{1,3}(?:,\d{3})+)(?:\.\d+)?%?$/.test(text.trim());
};
const measureTextWidth = (text, font, padding, documentRef) => {
    const cacheKey = `${font}\n${padding}\n${text}`;
    const cached = textWidthCache.get(cacheKey);
    if (cached !== undefined) {
        return cached;
    }
    if (!measureCanvas && documentRef) {
        measureCanvas = documentRef.createElement('canvas');
    }
    const context = measureCanvas === null || measureCanvas === void 0 ? void 0 : measureCanvas.getContext('2d');
    if (!context) {
        return text.length * TABLE_FONT_SIZE;
    }
    context.font = font;
    const width = context.measureText(text).width + padding * 2;
    textWidthCache.set(cacheKey, width);
    return width;
};
const isBlankOverflowTarget = (virtualState, row, rowIndex, dataColIndex) => {
    const column = virtualState.columns[dataColIndex + 1];
    if (!isVisibleDataColumn(column)) {
        return false;
    }
    const cellKey = getCellKeyByDataColumn(rowIndex, dataColIndex);
    if (virtualState.mergeStartMap.has(cellKey) || virtualState.mergeCoveredMap.has(cellKey)) {
        return false;
    }
    return !hasRenderableValue(row[getDataKey(dataColIndex)]);
};
const collectOverflowRoom = (virtualState, row, rowIndex, dataColIndex, direction) => {
    let room = 0;
    for (let step = 1; step <= MAX_OVERFLOW_SCAN_COLS; step += 1) {
        const nextColIndex = dataColIndex + step * direction;
        if (nextColIndex < 0 || nextColIndex >= virtualState.totalCols) {
            break;
        }
        if (!isBlankOverflowTarget(virtualState, row, rowIndex, nextColIndex)) {
            break;
        }
        room += getRenderColumnWidth(virtualState.columns[nextColIndex + 1]);
    }
    return room;
};
const getExcelOverflowLayout = (virtualState, row, rowIndex, dataColIndex, column, style, value, zoomScale, documentRef) => {
    const text = getTextValue(value);
    if (!text || (style === null || style === void 0 ? void 0 : style.wrapText) || (style === null || style === void 0 ? void 0 : style.shrinkToFit) || isNumericLikeValue(text)) {
        return undefined;
    }
    const cellKey = getCellKeyByDataColumn(rowIndex, dataColIndex);
    if (virtualState.mergeStartMap.has(cellKey) || virtualState.mergeCoveredMap.has(cellKey)) {
        return undefined;
    }
    const ownWidth = getRenderColumnWidth(column);
    if (!ownWidth) {
        return undefined;
    }
    const font = (style === null || style === void 0 ? void 0 : style.font) || BODY_FONT;
    const measuredWidth = Math.ceil(measureTextWidth(text, font, scaleNumber(CELL_PADDING, zoomScale), documentRef));
    if (measuredWidth <= ownWidth) {
        return undefined;
    }
    const align = (style === null || style === void 0 ? void 0 : style.horizontalAlign) || (column === null || column === void 0 ? void 0 : column.align) || 'left';
    const leftRoom = align === 'right' || align === 'center'
        ? collectOverflowRoom(virtualState, row, rowIndex, dataColIndex, -1)
        : 0;
    const rightRoom = align !== 'right'
        ? collectOverflowRoom(virtualState, row, rowIndex, dataColIndex, 1)
        : 0;
    const maxWidth = ownWidth + leftRoom + rightRoom;
    if (maxWidth <= ownWidth) {
        return undefined;
    }
    const width = Math.min(measuredWidth, maxWidth);
    return {
        left: toTextLayerLeft(align, ownWidth, width, leftRoom),
        width
    };
};
const getDataColumnSpanWidth = (virtualState, startDataColIndex, endDataColIndex) => {
    let width = 0;
    for (let index = startDataColIndex; index < endDataColIndex; index += 1) {
        width += getRenderColumnWidth(virtualState.columns[index + 1]);
    }
    return width;
};
const getLeftOverflowMirror = (virtualState, row, rowIndex, targetDataColIndex, zoomScale, documentRef) => {
    if (!isBlankOverflowTarget(virtualState, row, rowIndex, targetDataColIndex)) {
        return undefined;
    }
    const targetColumn = virtualState.columns[targetDataColIndex + 1];
    const targetWidth = getRenderColumnWidth(targetColumn);
    if (!targetWidth) {
        return undefined;
    }
    for (let step = 1; step <= MAX_OVERFLOW_SCAN_COLS; step += 1) {
        const sourceDataColIndex = targetDataColIndex - step;
        if (sourceDataColIndex < 0) {
            break;
        }
        const sourceColumn = virtualState.columns[sourceDataColIndex + 1];
        if (!isVisibleDataColumn(sourceColumn)) {
            break;
        }
        const sourceValue = row[getDataKey(sourceDataColIndex)];
        if (!hasRenderableValue(sourceValue)) {
            if (!isBlankOverflowTarget(virtualState, row, rowIndex, sourceDataColIndex)) {
                break;
            }
            continue;
        }
        const sourceCellKey = getCellKeyByDataColumn(rowIndex, sourceDataColIndex);
        const rawSourceStyle = virtualState.cellCache.get(sourceCellKey);
        const sourceStyle = {
            ...(scaleCellStyle(rawSourceStyle, zoomScale) || {}),
            horizontalAlign: (rawSourceStyle === null || rawSourceStyle === void 0 ? void 0 : rawSourceStyle.horizontalAlign) || (sourceColumn === null || sourceColumn === void 0 ? void 0 : sourceColumn.align),
            verticalAlign: (rawSourceStyle === null || rawSourceStyle === void 0 ? void 0 : rawSourceStyle.verticalAlign) || (sourceColumn === null || sourceColumn === void 0 ? void 0 : sourceColumn.verticalAlign)
        };
        const sourceLayout = getExcelOverflowLayout(virtualState, row, rowIndex, sourceDataColIndex, sourceColumn, sourceStyle, sourceValue, zoomScale, documentRef);
        if (!sourceLayout) {
            return undefined;
        }
        const targetOffset = getDataColumnSpanWidth(virtualState, sourceDataColIndex, targetDataColIndex);
        const sourceTextLeft = sourceLayout.left;
        const sourceTextRight = sourceLayout.left + sourceLayout.width;
        const targetLeft = targetOffset;
        const targetRight = targetOffset + targetWidth;
        if (sourceTextLeft < targetRight && sourceTextRight > targetLeft) {
            return {
                value: sourceValue,
                style: sourceStyle,
                overflowLayout: {
                    left: sourceLayout.left - targetOffset,
                    width: sourceLayout.width
                }
            };
        }
        return undefined;
    }
    return undefined;
};
const getBorderScore = (border) => {
    if (!border) {
        return 0;
    }
    return border.width * 100 + (BORDER_STYLE_PRIORITY[border.style] || 1);
};
const getCollapsedBorderStyle = (virtualState, rowIndex, dataColIndex, style, zoomScale) => {
    const previousRow = scaleCellStyle(virtualState.cellCache.get(displayCellKey(rowIndex - 1, dataColIndex + 1)), zoomScale);
    const nextRow = scaleCellStyle(virtualState.cellCache.get(displayCellKey(rowIndex + 1, dataColIndex + 1)), zoomScale);
    const previousCol = scaleCellStyle(virtualState.cellCache.get(displayCellKey(rowIndex, dataColIndex)), zoomScale);
    const nextCol = scaleCellStyle(virtualState.cellCache.get(displayCellKey(rowIndex, dataColIndex + 2)), zoomScale);
    const borderTop = getBorderScore(style.borderTop) >= getBorderScore(previousRow === null || previousRow === void 0 ? void 0 : previousRow.borderBottom)
        ? style.borderTop
        : undefined;
    const borderBottom = getBorderScore(style.borderBottom) > getBorderScore(nextRow === null || nextRow === void 0 ? void 0 : nextRow.borderTop)
        ? style.borderBottom
        : undefined;
    const borderLeft = getBorderScore(style.borderLeft) >= getBorderScore(previousCol === null || previousCol === void 0 ? void 0 : previousCol.borderRight)
        ? style.borderLeft
        : undefined;
    const borderRight = getBorderScore(style.borderRight) > getBorderScore(nextCol === null || nextCol === void 0 ? void 0 : nextCol.borderLeft)
        ? style.borderRight
        : undefined;
    return {
        ...style,
        borderTop,
        borderRight,
        borderBottom,
        borderLeft
    };
};
const createBorderLine = (documentRef, side, border) => {
    const line = documentRef.createElement('span');
    const half = border.width / 2;
    Object.assign(line.style, {
        position: 'absolute',
        pointerEvents: 'none',
        boxSizing: 'border-box'
    });
    if (side === 'top' || side === 'bottom') {
        Object.assign(line.style, {
            left: `${-half}px`,
            width: `calc(100% + ${border.width}px)`,
            height: '0'
        });
        line.style[side] = `${-half}px`;
        line.style.borderTop = `${border.width}px ${border.style} ${border.color}`;
    }
    else {
        Object.assign(line.style, {
            top: `${-half}px`,
            height: `calc(100% + ${border.width}px)`,
            width: '0'
        });
        line.style[side] = `${-half}px`;
        line.style.borderLeft = `${border.width}px ${border.style} ${border.color}`;
    }
    return line;
};
const createTextLayer = (documentRef, value, style, padding) => {
    const text = documentRef.createElement('span');
    text.textContent = getTextValue(value);
    Object.assign(text.style, {
        position: 'relative',
        zIndex: '1',
        width: '100%',
        height: '100%',
        boxSizing: 'border-box',
        display: 'flex',
        alignItems: toFlexAlign(style.verticalAlign),
        justifyContent: toFlexJustify(style.horizontalAlign),
        padding: `0 ${padding}px`,
        overflow: 'hidden',
        textOverflow: style.wrapText ? 'clip' : 'ellipsis',
        whiteSpace: style.wrapText ? 'pre-wrap' : 'nowrap',
        wordBreak: style.wrapText ? 'break-word' : 'normal',
        lineHeight: `${CELL_LINE_HEIGHT}`,
        textAlign: style.horizontalAlign || 'left',
        color: style.color || 'inherit',
        font: style.font || BODY_FONT,
        transform: style.shrinkToFit ? 'scale(0.92)' : 'none',
        transformOrigin: `${style.horizontalAlign || 'left'} center`
    });
    return text;
};
const applyOverflowLayout = (text, overflowLayout) => {
    if (!overflowLayout) {
        return;
    }
    Object.assign(text.style, {
        position: 'absolute',
        left: `${overflowLayout.left}px`,
        top: '0',
        width: `${overflowLayout.width}px`,
        overflow: 'hidden',
        textOverflow: 'clip'
    });
};
// e-virt-table 的普通样式接口不支持 Excel 式四边边框和单元格级换行/垂直对齐，
// 这里借助官方的 BODY_CELL_RENDER_METHOD，只在可视区补轻量 DOM 覆盖层。
const renderCellOverlay = (cellEl, borderStyle, textStyle, textValue, renderText, overflowLayout, zoomScale) => {
    const documentRef = cellEl.ownerDocument;
    cellEl.replaceChildren();
    Object.assign(cellEl.style, {
        pointerEvents: 'none',
        overflow: overflowLayout ? 'hidden' : 'visible',
        background: 'transparent'
    });
    const fragment = documentRef.createDocumentFragment();
    if (renderText) {
        const text = createTextLayer(documentRef, textValue, textStyle, scaleNumber(CELL_PADDING, zoomScale));
        applyOverflowLayout(text, overflowLayout);
        fragment.appendChild(text);
    }
    if (borderStyle.borderTop) {
        fragment.appendChild(createBorderLine(documentRef, 'top', borderStyle.borderTop));
    }
    if (borderStyle.borderRight) {
        fragment.appendChild(createBorderLine(documentRef, 'right', borderStyle.borderRight));
    }
    if (borderStyle.borderBottom) {
        fragment.appendChild(createBorderLine(documentRef, 'bottom', borderStyle.borderBottom));
    }
    if (borderStyle.borderLeft) {
        fragment.appendChild(createBorderLine(documentRef, 'left', borderStyle.borderLeft));
    }
    cellEl.appendChild(fragment);
};
export const normalizeCellStyle = (meta) => {
    if (!meta) {
        return undefined;
    }
    const style = meta.style || {};
    const nextStyle = {};
    if (style.backgroundColor) {
        nextStyle.backgroundColor = style.backgroundColor;
    }
    if (style.color) {
        nextStyle.color = style.color;
    }
    else if (style.backgroundColor && tinycolor(style.backgroundColor).isDark()) {
        nextStyle.color = '#ffffff';
    }
    const font = buildFont(style);
    if (font) {
        nextStyle.font = font;
    }
    else if (style.backgroundColor && tinycolor(style.backgroundColor).isDark()) {
        nextStyle.font = `bold ${TABLE_FONT_SIZE}px ${TABLE_FONT_FAMILY}`;
    }
    const borderTop = getBorderSide(style, 'Top');
    if (borderTop) {
        nextStyle.borderTop = borderTop;
    }
    const borderRight = getBorderSide(style, 'Right');
    if (borderRight) {
        nextStyle.borderRight = borderRight;
    }
    const borderBottom = getBorderSide(style, 'Bottom');
    if (borderBottom) {
        nextStyle.borderBottom = borderBottom;
    }
    const borderLeft = getBorderSide(style, 'Left');
    if (borderLeft) {
        nextStyle.borderLeft = borderLeft;
    }
    const horizontalAlign = alignFromClassName(meta.className);
    if (horizontalAlign) {
        nextStyle.horizontalAlign = horizontalAlign;
    }
    const verticalAlign = verticalAlignFromClassName(meta.className);
    if (verticalAlign) {
        nextStyle.verticalAlign = verticalAlign;
    }
    if (hasClassName(meta.className, 'htWrap')) {
        nextStyle.wrapText = true;
    }
    if (hasClassName(meta.className, 'htShrink')) {
        nextStyle.shrinkToFit = true;
    }
    if (!nextStyle.backgroundColor && !nextStyle.color && !nextStyle.font && !hasBorder(nextStyle) && !hasTextLayout(nextStyle)) {
        return undefined;
    }
    return nextStyle;
};
export const createTableConfig = ({ hostHeight, darkMode = false, resizableColumns = false, resizableRows = false, copySelection, sheetDefaults, virtualState, zoomScale = 1 }) => {
    const normalizedScale = normalizeZoomScale(zoomScale);
    const scaledPadding = scaleNumber(CELL_PADDING, normalizedScale);
    const scaledHeaderHeight = scaleNumber(HEADER_HEIGHT, normalizedScale);
    const scaledCellHeight = scaleNumber(normalizeRowHeight(sheetDefaults.rowHeight, sheetDefaults.rowHeight), normalizedScale);
    const scaledCellWidth = scaleNumber(sheetDefaults.colWidth, normalizedScale);
    const headerStyle = getIndexCellStyle(normalizedScale, darkMode);
    const loadingStyle = getLoadingCellStyle(normalizedScale, darkMode);
    const spanMethod = ({ rowIndex, colIndex, column }) => {
        if (colIndex === 0) {
            return;
        }
        const key = getCellCacheKey(rowIndex, colIndex, column);
        if (virtualState.mergeCoveredMap.has(key)) {
            return { rowspan: 0, colspan: 0 };
        }
        const merge = virtualState.mergeStartMap.get(key);
        if (!merge) {
            return;
        }
        return {
            rowspan: merge.rowspan,
            colspan: merge.colspan
        };
    };
    const headerStyleMethod = () => headerStyle;
    const styleMethod = ({ row, rowIndex, colIndex, column }) => {
        if (colIndex === 0) {
            return headerStyle;
        }
        const currentRow = row;
        if (isHiddenVirtualRow(currentRow)) {
            return undefined;
        }
        if (getRowState(currentRow) !== RowState.Loaded) {
            return loadingStyle;
        }
        return scaleCellStyle(virtualState.cellCache.get(getCellCacheKey(rowIndex, colIndex, column)), normalizedScale);
    };
    const renderMethod = ({ row, rowIndex, colIndex, column, value }) => {
        const currentRow = row;
        if (colIndex === 0 || isHiddenVirtualRow(currentRow) || getRowState(currentRow) !== RowState.Loaded) {
            return undefined;
        }
        const cellKey = getCellCacheKey(rowIndex, colIndex, column);
        if (virtualState.mergeCoveredMap.has(cellKey)) {
            return undefined;
        }
        const style = scaleCellStyle(virtualState.cellCache.get(cellKey), normalizedScale);
        const dataColIndex = getDataColumnIndex(column, colIndex);
        const measureDocument = typeof document === 'undefined' ? undefined : document;
        const overflowLayout = getExcelOverflowLayout(virtualState, currentRow, rowIndex, dataColIndex, column, style, value, normalizedScale, measureDocument);
        const mirror = !hasRenderableValue(value)
            ? getLeftOverflowMirror(virtualState, currentRow, rowIndex, dataColIndex, normalizedScale, measureDocument)
            : undefined;
        const renderText = shouldRenderTextInOverlay(style, column) || !!overflowLayout || !!mirror;
        if (!hasBorder(style) && !renderText) {
            return undefined;
        }
        return ((cellEl) => {
            const baseStyle = {
                ...(style || {}),
                horizontalAlign: (style === null || style === void 0 ? void 0 : style.horizontalAlign) || (column === null || column === void 0 ? void 0 : column.align),
                verticalAlign: (style === null || style === void 0 ? void 0 : style.verticalAlign) || (column === null || column === void 0 ? void 0 : column.verticalAlign)
            };
            const collapsedStyle = hasBorder(style)
                ? getCollapsedBorderStyle(virtualState, rowIndex, dataColIndex, baseStyle, normalizedScale)
                : baseStyle;
            renderCellOverlay(cellEl, collapsedStyle, (mirror === null || mirror === void 0 ? void 0 : mirror.style) || collapsedStyle, mirror ? mirror.value : value, renderText, (mirror === null || mirror === void 0 ? void 0 : mirror.overflowLayout) || overflowLayout, normalizedScale);
        });
    };
    return {
        ROW_KEY: ROW_KEY_FIELD,
        DISABLED: true,
        HEIGHT: Math.max(hostHeight, 240),
        MAX_HEIGHT: Math.max(hostHeight, 240),
        HEADER_HEIGHT: scaledHeaderHeight,
        CELL_HEIGHT: scaledCellHeight,
        CELL_WIDTH: scaledCellWidth,
        CELL_PADDING: scaledPadding,
        CELL_LINE_HEIGHT,
        COLUMNS_VERTICAL_ALIGN: 'middle',
        HEADER_FONT: getHeaderFont(normalizedScale),
        BODY_FONT: getBodyFont(normalizedScale),
        BORDER_RADIUS: 0,
        BORDER_COLOR: darkMode ? EXCEL_DARK_GRID : EXCEL_GRID,
        HEADER_BG_COLOR: darkMode ? EXCEL_DARK_HEADER_BG : EXCEL_HEADER_BG,
        BODY_BG_COLOR: darkMode ? EXCEL_DARK_BODY_BG : '#ffffff',
        HEADER_TEXT_COLOR: darkMode ? EXCEL_DARK_HEADER_TEXT : EXCEL_HEADER_TEXT,
        BODY_TEXT_COLOR: darkMode ? EXCEL_DARK_BODY_TEXT : '#202124',
        READONLY_COLOR: darkMode ? EXCEL_DARK_BODY_BG : '#ffffff',
        READONLY_TEXT_COLOR: darkMode ? EXCEL_DARK_BODY_TEXT : '#202124',
        EDIT_BG_COLOR: darkMode ? '#18212f' : '#ffffff',
        PLACEHOLDER_COLOR: darkMode ? '#94a3b8' : '#8a94a3',
        SCROLLER_COLOR: darkMode ? '#475569' : '#c1c7d0',
        SCROLLER_FOCUS_COLOR: darkMode ? '#64748b' : '#9aa0a6',
        SELECT_ROW_COL_BG_COLOR: EXCEL_GREEN_SOFT,
        SELECT_AREA_COLOR: 'rgba(33, 163, 102, 0.14)',
        SELECT_BORDER_COLOR: EXCEL_GREEN,
        AUTOFILL_POINT_BORDER_COLOR: EXCEL_GREEN,
        ENABLE_SELECTOR: true,
        ENABLE_SELECTOR_SINGLE: false,
        ENABLE_SELECTOR_SPAN_COL: true,
        ENABLE_SELECTOR_SPAN_ROW: true,
        ENABLE_SELECTOR_ALL_ROWS: true,
        ENABLE_SELECTOR_ALL_COLS: true,
        SELECTOR_AREA_MIN_X: DATA_AREA_MIN_COL_INDEX,
        ENABLE_CONTEXT_MENU: false,
        ENABLE_HEADER_CONTEXT_MENU: false,
        ENABLE_AUTOFILL: true,
        ENABLE_AUTOFILL_SPAN_COL: true,
        ENABLE_AUTOFILL_SPAN_ROW: true,
        ENABLE_PASTER: false,
        ENABLE_HISTORY: false,
        ENABLE_RESIZE_COLUMN: resizableColumns,
        ENABLE_RESIZE_COLUMN_TEXT: resizableColumns,
        RESIZE_COLUMN_LINE_COLOR: EXCEL_GREEN,
        RESIZE_COLUMN_TEXT_BG_COLOR: EXCEL_GREEN,
        RESIZE_COLUMN_MIN_WIDTH: RESIZABLE_COLUMN_MIN_WIDTH,
        ENABLE_RESIZE_ROW: resizableRows,
        RESIZE_ROW_LINE_COLOR: EXCEL_GREEN,
        RESIZE_ROW_MIN_HEIGHT: scaleNumber(RESIZABLE_ROW_MIN_HEIGHT, normalizedScale),
        ENABLE_KEYBOARD: true,
        ENABLE_COPY: true,
        BEFORE_COPY_METHOD: copySelection
            ? (params) => {
                copySelection(params);
                return undefined;
            }
            : undefined,
        // 预览态只开放“拖动扩选”的交互，不允许真正写回单元格内容。
        BEFORE_AUTOFILL_DATA_METHOD: () => [],
        SPAN_METHOD: spanMethod,
        HEADER_CELL_STYLE_METHOD: headerStyleMethod,
        BODY_CELL_STYLE_METHOD: styleMethod,
        BODY_CELL_RENDER_METHOD: renderMethod,
        BODY_CELL_FORMATTER_METHOD: ({ row, rowIndex, colIndex, column, value }) => {
            if (isHiddenVirtualRow(row)) {
                return '';
            }
            if (colIndex === 0) {
                const displayIndex = rowIndex + 1 - virtualState.indexOffset;
                return displayIndex > 0 ? `${displayIndex}` : '';
            }
            if (getRowState(row) !== RowState.Loaded) {
                return '';
            }
            const currentRow = row;
            const style = scaleCellStyle(virtualState.cellCache.get(getCellCacheKey(rowIndex, colIndex, column)), normalizedScale);
            const dataColIndex = getDataColumnIndex(column, colIndex);
            const measureDocument = typeof document === 'undefined' ? undefined : document;
            const overflowLayout = getExcelOverflowLayout(virtualState, currentRow, rowIndex, dataColIndex, column, style, value, normalizedScale, measureDocument);
            const mirror = !hasRenderableValue(value)
                ? getLeftOverflowMirror(virtualState, currentRow, rowIndex, dataColIndex, normalizedScale, measureDocument)
                : undefined;
            if (shouldRenderTextInOverlay(style, column) || overflowLayout || mirror) {
                return '';
            }
            if (value === null || value === undefined) {
                return '';
            }
            return `${value}`;
        }
    };
};
