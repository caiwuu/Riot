import { read, utils } from 'styled-exceljs';
import SheetJsModel from './SheetJsModel.js';
import { parseSpreadsheetCharts } from './chartParser.js';
import { prepareSpreadsheetReadInput, } from './textEncoding.js';
import { parseSpreadsheetCellImages } from './richDataParser.js';
const readOptions = {
    type: 'array',
    dense: true,
    cellDates: true,
    cellStyles: true,
    browserPixels: true,
    drawings: true,
    validateMerges: true,
};
export const createSpreadsheetParserContext = () => ({
    workbook: null,
    sheets: [],
    charts: {},
    cellImages: {},
});
const toErrorResponse = (error, payload = {}) => ({
    type: 'parseError',
    payload: {
        ...payload,
        message: error instanceof Error ? error.message : String(error),
    },
});
const getDrawingBounds = (worksheet) => {
    var _a;
    const images = ((_a = worksheet === null || worksheet === void 0 ? void 0 : worksheet['!drawings']) === null || _a === void 0 ? void 0 : _a.images) || [];
    return images.reduce((bounds, image) => {
        var _a, _b, _c, _d, _e, _f;
        const anchor = image.anchor;
        const row = Number((_b = (_a = anchor === null || anchor === void 0 ? void 0 : anchor.to) === null || _a === void 0 ? void 0 : _a.row) !== null && _b !== void 0 ? _b : (_c = anchor === null || anchor === void 0 ? void 0 : anchor.from) === null || _c === void 0 ? void 0 : _c.row);
        const col = Number((_e = (_d = anchor === null || anchor === void 0 ? void 0 : anchor.to) === null || _d === void 0 ? void 0 : _d.col) !== null && _e !== void 0 ? _e : (_f = anchor === null || anchor === void 0 ? void 0 : anchor.from) === null || _f === void 0 ? void 0 : _f.col);
        return {
            rowCount: Number.isFinite(row) ? Math.max(bounds.rowCount, row + 1) : bounds.rowCount,
            colCount: Number.isFinite(col) ? Math.max(bounds.colCount, col + 1) : bounds.colCount,
        };
    }, {
        rowCount: 0,
        colCount: 0,
    });
};
const getChartBounds = (charts) => {
    return (charts || []).reduce((bounds, chart) => {
        var _a, _b, _c, _d, _e, _f;
        const estimatedRows = ((_a = chart.ext) === null || _a === void 0 ? void 0 : _a.height)
            ? Math.ceil(chart.ext.height / 9525 / 20)
            : 0;
        const estimatedCols = ((_b = chart.ext) === null || _b === void 0 ? void 0 : _b.width)
            ? Math.ceil(chart.ext.width / 9525 / 64)
            : 0;
        const row = (_d = (_c = chart.to) === null || _c === void 0 ? void 0 : _c.row) !== null && _d !== void 0 ? _d : chart.from.row + estimatedRows;
        const col = (_f = (_e = chart.to) === null || _e === void 0 ? void 0 : _e.col) !== null && _f !== void 0 ? _f : chart.from.col + estimatedCols;
        return {
            rowCount: Math.max(bounds.rowCount, row + 1),
            colCount: Math.max(bounds.colCount, col + 1),
        };
    }, {
        rowCount: 0,
        colCount: 0,
    });
};
const EMPTY_RANGE_ROW_LIMIT = 1000;
const EMPTY_RANGE_COLUMN_LIMIT = 256;
const RANGE_ROW_SLACK = 256;
const RANGE_COLUMN_SLACK = 64;
const RANGE_GROWTH_FACTOR = 4;
const getCellBounds = (worksheet) => {
    const bounds = { rowCount: 0, colCount: 0 };
    if (!worksheet)
        return bounds;
    const denseRows = Array.isArray(worksheet)
        ? worksheet
        : worksheet['!data'];
    if (Array.isArray(denseRows)) {
        for (const rowKey of Object.keys(denseRows)) {
            const rowIndex = Number(rowKey);
            const row = denseRows[rowIndex];
            if (!Number.isInteger(rowIndex) || rowIndex < 0 || !Array.isArray(row))
                continue;
            for (const colKey of Object.keys(row)) {
                const colIndex = Number(colKey);
                if (!Number.isInteger(colIndex) || colIndex < 0 || row[colIndex] == null)
                    continue;
                bounds.rowCount = Math.max(bounds.rowCount, rowIndex + 1);
                bounds.colCount = Math.max(bounds.colCount, colIndex + 1);
            }
        }
        return bounds;
    }
    for (const key of Object.keys(worksheet)) {
        if (key.startsWith('!') || !/^[A-Z]+[1-9][0-9]*$/i.test(key) || worksheet[key] == null)
            continue;
        try {
            const cell = utils.decode_cell(key);
            bounds.rowCount = Math.max(bounds.rowCount, cell.r + 1);
            bounds.colCount = Math.max(bounds.colCount, cell.c + 1);
        }
        catch {
            // Ignore non-cell extension keys exposed by third-party workbook producers.
        }
    }
    return bounds;
};
const getMergeBounds = (worksheet) => {
    return ((worksheet === null || worksheet === void 0 ? void 0 : worksheet['!merges']) || []).reduce((bounds, merge) => {
        var _a, _b, _c, _d, _e, _f;
        const row = Number((_b = (_a = merge.e) === null || _a === void 0 ? void 0 : _a.row) !== null && _b !== void 0 ? _b : (_c = merge.e) === null || _c === void 0 ? void 0 : _c.r);
        const col = Number((_e = (_d = merge.e) === null || _d === void 0 ? void 0 : _d.col) !== null && _e !== void 0 ? _e : (_f = merge.e) === null || _f === void 0 ? void 0 : _f.c);
        return {
            rowCount: Number.isFinite(row) ? Math.max(bounds.rowCount, row + 1) : bounds.rowCount,
            colCount: Number.isFinite(col) ? Math.max(bounds.colCount, col + 1) : bounds.colCount,
        };
    }, { rowCount: 0, colCount: 0 });
};
const reconcileDeclaredRange = (declaredCount, observedCount, slack, emptyLimit) => {
    if (observedCount <= 0) {
        return Math.min(Math.max(declaredCount, 1), emptyLimit);
    }
    const plausibleLimit = Math.max(observedCount + slack, observedCount * RANGE_GROWTH_FACTOR);
    return declaredCount <= plausibleLimit
        ? Math.max(declaredCount, observedCount)
        : observedCount;
};
export const getWorksheetDisplayBounds = (worksheet, charts) => {
    let declaredRowCount = 0;
    let declaredColCount = 0;
    const ref = worksheet === null || worksheet === void 0 ? void 0 : worksheet['!ref'];
    if (ref) {
        try {
            const range = utils.decode_range(ref);
            declaredRowCount = range.e.r + 1;
            declaredColCount = range.e.c + 1;
        }
        catch {
            // Invalid producer dimensions must not block content-based recovery.
        }
    }
    const cellBounds = getCellBounds(worksheet);
    const mergeBounds = getMergeBounds(worksheet);
    const drawingBounds = getDrawingBounds(worksheet);
    const chartBounds = getChartBounds(charts);
    const observedRowCount = Math.max(cellBounds.rowCount, mergeBounds.rowCount, drawingBounds.rowCount, chartBounds.rowCount);
    const observedColCount = Math.max(cellBounds.colCount, mergeBounds.colCount, drawingBounds.colCount, chartBounds.colCount);
    const rowCount = reconcileDeclaredRange(declaredRowCount, observedRowCount, RANGE_ROW_SLACK, EMPTY_RANGE_ROW_LIMIT);
    const colCount = reconcileDeclaredRange(declaredColCount, observedColCount, RANGE_COLUMN_SLACK, EMPTY_RANGE_COLUMN_LIMIT);
    return {
        rowCount,
        colCount,
        declaredRowCount,
        declaredColCount,
        observedRowCount,
        observedColCount,
        trimmed: rowCount < declaredRowCount || colCount < declaredColCount,
    };
};
const parseSheets = (context) => {
    var _a;
    const workbook = context.workbook;
    if (!(workbook === null || workbook === void 0 ? void 0 : workbook.SheetNames)) {
        return [];
    }
    const workbookSheets = ((_a = workbook.Workbook) === null || _a === void 0 ? void 0 : _a.Sheets) || [];
    context.sheets = workbook.SheetNames.reduce((result, name, sourceIndex) => {
        var _a;
        const worksheet = workbook.Sheets[name];
        const bounds = getWorksheetDisplayBounds(worksheet, context.charts[name]);
        if (!(worksheet === null || worksheet === void 0 ? void 0 : worksheet['!ref']) && !bounds.observedRowCount && !bounds.observedColCount) {
            return result;
        }
        if (bounds.trimmed) {
            console.warn(`[file-viewer] Ignored pathological worksheet dimensions for ${name}: `
                + `${bounds.declaredRowCount}x${bounds.declaredColCount} -> ${bounds.rowCount}x${bounds.colCount}.`);
        }
        result.push({
            id: result.length,
            name,
            hidden: !!((_a = workbookSheets[sourceIndex]) === null || _a === void 0 ? void 0 : _a.Hidden),
            rowCount: bounds.rowCount,
            colCount: bounds.colCount,
        });
        return result;
    }, []);
    return [{ type: 'sheets', payload: { sheets: context.sheets } }];
};
export const parseSpreadsheetWorkbook = async (context, data, source = {}) => {
    try {
        const input = prepareSpreadsheetReadInput(data, source);
        context.workbook = input.kind === 'text'
            ? read(input.data, { ...readOptions, type: 'string' })
            : read(input.data, readOptions);
        const signature = data.byteLength >= 2 ? new DataView(data).getUint16(0, false) : 0;
        if (signature === 0x504b) {
            const [charts, cellImages] = await Promise.all([
                parseSpreadsheetCharts(data, context.workbook).catch((error) => {
                    console.warn('[file-viewer] Spreadsheet chart parsing failed; continuing with cell content.', error);
                    return {};
                }),
                parseSpreadsheetCellImages(data).catch((error) => {
                    console.warn('[file-viewer] Spreadsheet cell image parsing failed; continuing with cell content.', error);
                    return {};
                }),
            ]);
            context.charts = charts;
            context.cellImages = cellImages;
        }
        else {
            context.charts = {};
            context.cellImages = {};
        }
        return parseSheets(context);
    }
    catch (error) {
        return [toErrorResponse(error)];
    }
};
export const parseSpreadsheetSheet = (context, payload = {}) => {
    var _a;
    const { sheet, startRow = 0, pageSize = 500, sessionId = 0, } = payload;
    try {
        const workbook = context.workbook;
        const sheetName = (_a = context.sheets.find(item => item.id === sheet)) === null || _a === void 0 ? void 0 : _a.name;
        if (!(workbook === null || workbook === void 0 ? void 0 : workbook.Sheets) || !sheetName) {
            return [];
        }
        const worksheet = workbook.Sheets[sheetName];
        if (!worksheet) {
            return [];
        }
        const sheetMeta = context.sheets.find(item => item.id === sheet);
        const sheetModel = SheetJsModel.create(worksheet, {
            startRow,
            pageSize,
            totalRows: sheetMeta === null || sheetMeta === void 0 ? void 0 : sheetMeta.rowCount,
            totalCols: sheetMeta === null || sheetMeta === void 0 ? void 0 : sheetMeta.colCount,
            charts: context.charts[sheetName],
            cellImages: context.cellImages[sheetName],
        });
        // Keep the first response backward-compatible; later virtual windows only need rows and cells.
        // Avoid recalculating auto-fit column widths for every 500-row request.
        const windowData = sheetModel.toObject({ includeLayout: startRow === 0 });
        const structure = startRow === 0 ? sheetModel.structure : undefined;
        return [{
                type: 'parseSheet',
                payload: {
                    sessionId,
                    sheet,
                    sheetData: structure ? {
                        ...windowData,
                        structure,
                    } : windowData,
                },
            }];
    }
    catch (error) {
        return [toErrorResponse(error, { sessionId, startRow })];
    }
};
export const handleSpreadsheetWorkerRequest = (context, request) => {
    var _a, _b, _c, _d;
    switch (request.type) {
        case 'parseWorkbook':
            return parseSpreadsheetWorkbook(context, (_a = request.payload) === null || _a === void 0 ? void 0 : _a.workbook, {
                fileType: (_b = request.payload) === null || _b === void 0 ? void 0 : _b.fileType,
                filename: (_c = request.payload) === null || _c === void 0 ? void 0 : _c.filename,
                textEncoding: (_d = request.payload) === null || _d === void 0 ? void 0 : _d.textEncoding,
            });
        case 'parseSheet':
            return parseSpreadsheetSheet(context, request.payload);
        default:
            return [];
    }
};
