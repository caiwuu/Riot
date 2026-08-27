import type { Range, WorkSheet } from 'styled-exceljs';
import type { CellMerge, SheetChartDefinition, SheetCellImage, SheetColumn, SheetModel, SheetStructure, SheetWindow } from '../type.js';
export declare const createAutoFitSampleRanges: (totalRowCount: number, totalColCount: number) => Range[];
export declare const defaults: {
    rowHeight: number;
    colWidth: number;
};
interface SheetSliceOptions {
    startRow?: number;
    pageSize?: number;
    totalRows?: number;
    totalCols?: number;
    charts?: SheetChartDefinition[];
    cellImages?: SheetCellImage[];
}
interface SheetObjectOptions {
    includeLayout?: boolean;
}
type SheetCellMeta = {
    className?: string;
    style: Record<string, string>;
};
export default class SheetJsModel implements SheetModel {
    private readonly _ws;
    private readonly _startRow;
    private readonly _pageSize;
    private readonly _totalRows?;
    private readonly _totalCols?;
    private readonly _charts;
    private readonly _cellImages;
    private readonly _cellImageKeys;
    private static readonly defaults;
    private _data;
    private _cell;
    private _merge;
    private _rowHeights;
    private _colWidths;
    private _autoFitColumns;
    private _columns;
    private _structure;
    private _meta;
    private readonly range;
    static create(ws: WorkSheet, options?: SheetSliceOptions): SheetJsModel;
    private constructor();
    private get ws();
    get defaults(): {
        rowHeight: number;
        colWidth: number;
    };
    get data(): string[][];
    get cell(): Record<string, SheetCellMeta>;
    get merge(): CellMerge[];
    get columns(): SheetColumn[];
    get structure(): SheetStructure;
    get rowHeights(): number | number[];
    get colWidths(): number | number[];
    get meta(): SheetWindow;
    private get totalRows();
    private get totalCols();
    private get startRow();
    private get endRow();
    private get denseRows();
    private getCellAt;
    private getAxisOffset;
    private getMarkerLeft;
    private getMarkerTop;
    private getImages;
    private getCharts;
    private getAllMerge;
    private getData;
    private getCell;
    private getMerge;
    private getRowHeights;
    private getAllRowHeights;
    private getAutoFitColumns;
    private getAutoFitSampleRanges;
    private get autoFitColumns();
    private getColumnMeta;
    private get hasAnchoredObjects();
    private getColWidths;
    private getColumns;
    private getStructure;
    toObject(options?: SheetObjectOptions): object;
}
export {};
