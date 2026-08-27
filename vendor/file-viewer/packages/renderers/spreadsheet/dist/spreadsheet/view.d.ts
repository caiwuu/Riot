import type { Column, ConfigType } from 'e-virt-table';
import type { SheetModel } from './worker/type.js';
import { type CellStyleCache, type SheetDefaults, type VirtualSheetState } from './state.js';
export declare const INDEX_COLUMN_WIDTH = 68;
export declare const SPREADSHEET_MIN_ZOOM = 0.25;
export declare const SPREADSHEET_MAX_ZOOM = 2.5;
export declare const HEADER_HEIGHT = 34;
export declare const RESIZABLE_COLUMN_MIN_WIDTH = 40;
export declare const RESIZABLE_ROW_MIN_HEIGHT = 18;
interface TableConfigOptions {
    hostHeight: number;
    darkMode?: boolean;
    resizableColumns?: boolean;
    resizableRows?: boolean;
    copySelection?: (params: SpreadsheetCopyParams) => void;
    sheetDefaults: SheetDefaults;
    virtualState: VirtualSheetState;
    zoomScale?: number;
}
interface SpreadsheetCopyParams {
    focusCell?: unknown;
    data: unknown;
    xArr: number[];
    yArr: number[];
}
export declare const getRowHeight: (heights: number | number[] | undefined, index: number, fallback: number) => number;
export declare const normalizeRowHeight: (height: number | undefined, fallback: number) => number;
export declare const detectIndexOffset: (ws: SheetModel) => 0 | 1;
export declare const buildColumns: (ws: SheetModel) => {
    columns: Column[];
    dataKeys: string[];
};
export declare const getDisplayColumns: (columns: Column[], zoomScale?: number) => Column[];
export declare const normalizeCellStyle: (meta: {
    className?: string;
    style: any;
} | undefined) => CellStyleCache | undefined;
export declare const createTableConfig: ({ hostHeight, darkMode, resizableColumns, resizableRows, copySelection, sheetDefaults, virtualState, zoomScale }: TableConfigOptions) => ConfigType;
export {};
