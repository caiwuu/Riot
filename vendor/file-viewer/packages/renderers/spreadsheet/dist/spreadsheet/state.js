export const WINDOW_SIZE = 500;
export const PREFETCH_AHEAD = 3;
export const PREFETCH_BEHIND = 1;
export const INDEX_COLUMN_KEY = '__index';
export const ROW_KEY_FIELD = '__k';
export const ROW_STATE_FIELD = '__s';
export const DEFAULT_SHEET_DEFAULTS = {
    rowHeight: 20,
    colWidth: 64
};
export const RowState = {
    Placeholder: 0,
    Loading: 1,
    Loaded: 2
};
export const createEmptyVirtualState = () => ({
    active: false,
    totalRows: 0,
    totalCols: 0,
    indexOffset: 0,
    defaults: { ...DEFAULT_SHEET_DEFAULTS },
    dataKeys: [],
    rows: [],
    columns: [],
    cellCache: new Map(),
    mergeStartMap: new Map(),
    mergeCoveredMap: new Map(),
    rowHeightCache: new Map(),
    windowRows: new Map(),
    windowCells: new Map(),
    loadedWindows: new Set(),
    loadingWindows: new Set()
});
export const displayCellKey = (row, col) => {
    return `${row}-${col}`;
};
export const getDataKey = (index) => {
    return `c${index}`;
};
export const getRowState = (row) => {
    var _a;
    return (_a = row === null || row === void 0 ? void 0 : row[ROW_STATE_FIELD]) !== null && _a !== void 0 ? _a : RowState.Placeholder;
};
export const buildRows = (count) => {
    const rows = new Array(count);
    for (let index = 0; index < count; index += 1) {
        rows[index] = {
            [ROW_KEY_FIELD]: `${index}`,
            [ROW_STATE_FIELD]: RowState.Placeholder
        };
    }
    return rows;
};
export const getMaxWindowStart = (totalRows) => {
    if (!totalRows) {
        return 0;
    }
    return Math.floor(Math.max(totalRows - 1, 0) / WINDOW_SIZE) * WINDOW_SIZE;
};
export const clampWindowStart = (startRow = 0, totalRows = 0) => {
    return Math.min(Math.max(startRow, 0), getMaxWindowStart(totalRows));
};
export const getWindowStart = (row = 0, totalRows = 0) => {
    const start = Math.floor(Math.max(row, 0) / WINDOW_SIZE) * WINDOW_SIZE;
    return clampWindowStart(start, totalRows);
};
// 视口周边始终保留一小圈热窗口，滚轮滚动和拖动滚动条都可以直接命中缓存，
// 避免每次跳转都同步等待 worker 回包。
export const collectWindowStarts = ({ startRow, endRow, direction, totalRows, far = false }) => {
    const starts = new Set();
    const firstWindow = getWindowStart(startRow, totalRows);
    const lastWindow = getWindowStart(endRow, totalRows);
    for (let current = firstWindow; current <= lastWindow; current += WINDOW_SIZE) {
        starts.add(current);
    }
    const ahead = far ? PREFETCH_AHEAD + 1 : PREFETCH_AHEAD;
    const behind = far ? PREFETCH_BEHIND + 1 : PREFETCH_BEHIND;
    const anchor = direction > 0 ? lastWindow : firstWindow;
    for (let step = 1; step <= ahead; step += 1) {
        starts.add(clampWindowStart(anchor + direction * step * WINDOW_SIZE, totalRows));
    }
    for (let step = 1; step <= behind; step += 1) {
        starts.add(clampWindowStart(anchor - direction * step * WINDOW_SIZE, totalRows));
    }
    return Array.from(starts);
};
export const markWindowState = (rows, totalRows, windowStart, nextState) => {
    const endRow = Math.min(windowStart + WINDOW_SIZE, totalRows);
    for (let rowIndex = windowStart; rowIndex < endRow; rowIndex += 1) {
        const row = rows[rowIndex];
        if (!row) {
            continue;
        }
        row[ROW_STATE_FIELD] = nextState;
    }
};
