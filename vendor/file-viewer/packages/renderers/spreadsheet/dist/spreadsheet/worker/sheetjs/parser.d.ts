import type { WorkBook } from 'styled-exceljs';
import { type SpreadsheetTextSource } from './textEncoding.js';
import type { SheetCellImage, SheetChartDefinition, SheetDefinition } from '../type.js';
interface DrawingMarkerLike {
    row?: number;
    col?: number;
}
interface DrawingImageLike {
    anchor?: {
        from?: DrawingMarkerLike;
        to?: DrawingMarkerLike;
    };
}
interface WorksheetWithDrawings {
    '!drawings'?: {
        images?: DrawingImageLike[];
    };
}
export interface SpreadsheetParserContext {
    workbook: WorkBook | null;
    sheets: SheetDefinition[];
    charts: Record<string, SheetChartDefinition[]>;
    cellImages: Record<string, SheetCellImage[]>;
}
export interface SpreadsheetWorkerRequest {
    type: string;
    payload?: Record<string, any>;
}
export interface SpreadsheetWorkerResponse {
    type: string;
    payload?: Record<string, any>;
}
export declare const createSpreadsheetParserContext: () => SpreadsheetParserContext;
interface WorksheetRangeLike extends WorksheetWithDrawings {
    '!ref'?: string;
    '!data'?: Array<Array<unknown> | undefined>;
    '!merges'?: Array<{
        e?: DrawingMarkerLike;
    }>;
    [key: string]: unknown;
}
export interface WorksheetDisplayBounds {
    rowCount: number;
    colCount: number;
    declaredRowCount: number;
    declaredColCount: number;
    observedRowCount: number;
    observedColCount: number;
    trimmed: boolean;
}
export declare const getWorksheetDisplayBounds: (worksheet: WorksheetRangeLike | undefined, charts: SheetChartDefinition[] | undefined) => WorksheetDisplayBounds;
export declare const parseSpreadsheetWorkbook: (context: SpreadsheetParserContext, data: ArrayBuffer, source?: SpreadsheetTextSource) => Promise<SpreadsheetWorkerResponse[]>;
export declare const parseSpreadsheetSheet: (context: SpreadsheetParserContext, payload?: Record<string, any>) => SpreadsheetWorkerResponse[];
export declare const handleSpreadsheetWorkerRequest: (context: SpreadsheetParserContext, request: SpreadsheetWorkerRequest) => SpreadsheetWorkerResponse[] | Promise<SpreadsheetWorkerResponse[]>;
export {};
