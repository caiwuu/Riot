import type { Align, Column, VerticalAlign } from 'e-virt-table';
import type { CellMerge } from './worker/type.js';
export declare const WINDOW_SIZE = 500;
export declare const PREFETCH_AHEAD = 3;
export declare const PREFETCH_BEHIND = 1;
export declare const INDEX_COLUMN_KEY = "__index";
export declare const ROW_KEY_FIELD = "__k";
export declare const ROW_STATE_FIELD = "__s";
export interface SheetDefaults {
    rowHeight: number;
    colWidth: number;
}
export declare const DEFAULT_SHEET_DEFAULTS: SheetDefaults;
export declare const RowState: {
    readonly Placeholder: 0;
    readonly Loading: 1;
    readonly Loaded: 2;
};
export type RowStateValue = (typeof RowState)[keyof typeof RowState];
export interface VirtualRow extends Record<string, any> {
    __k: string;
    __s: RowStateValue;
    __baseHeight?: number;
    _height?: number;
}
export interface CellBorderCache {
    width: number;
    style: string;
    color: string;
}
export interface CellStyleCache {
    backgroundColor?: string;
    color?: string;
    font?: string;
    horizontalAlign?: Align;
    verticalAlign?: VerticalAlign;
    wrapText?: boolean;
    shrinkToFit?: boolean;
    borderTop?: CellBorderCache;
    borderRight?: CellBorderCache;
    borderBottom?: CellBorderCache;
    borderLeft?: CellBorderCache;
}
export interface VirtualSheetState {
    active: boolean;
    totalRows: number;
    totalCols: number;
    indexOffset: number;
    defaults: SheetDefaults;
    dataKeys: string[];
    rows: VirtualRow[];
    columns: Column[];
    cellCache: Map<string, CellStyleCache>;
    mergeStartMap: Map<string, CellMerge>;
    mergeCoveredMap: Map<string, true>;
    rowHeightCache: Map<number, number>;
    windowRows: Map<number, number[]>;
    windowCells: Map<number, string[]>;
    loadedWindows: Set<number>;
    loadingWindows: Set<number>;
}
export type ScrollDirection = 1 | -1;
interface CollectWindowStartsOptions {
    direction: ScrollDirection;
    endRow: number;
    far?: boolean;
    startRow: number;
    totalRows: number;
}
export declare const createEmptyVirtualState: () => VirtualSheetState;
export declare const displayCellKey: (row: number, col: number) => string;
export declare const getDataKey: (index: number) => string;
export declare const getRowState: (row?: VirtualRow | null) => RowStateValue;
export declare const buildRows: (count: number) => VirtualRow[];
export declare const getMaxWindowStart: (totalRows: number) => number;
export declare const clampWindowStart: (startRow?: number, totalRows?: number) => number;
export declare const getWindowStart: (row?: number, totalRows?: number) => number;
export declare const collectWindowStarts: ({ startRow, endRow, direction, totalRows, far }: CollectWindowStartsOptions) => number[];
export declare const markWindowState: (rows: VirtualRow[], totalRows: number, windowStart: number, nextState: RowStateValue) => void;
export {};
