import type { SheetCellImage } from '../type.js';
export declare const parseSpreadsheetCellImages: (data: ArrayBuffer) => Promise<Record<string, SheetCellImage[]>>;
