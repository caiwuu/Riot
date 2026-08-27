import type { CharState, DecodedProperty, ParaState, TableCellMeta, TableState } from '../types.js';
export declare function propertyArrayToMaps(properties: DecodedProperty[]): {
    char: Record<string, unknown>;
    para: Record<string, unknown>;
    table: Record<string, unknown>;
};
export declare function charPropsToState(properties: DecodedProperty[]): CharState;
export declare function paraPropsToState(properties: DecodedProperty[]): ParaState;
export declare function tablePropsToState(properties: DecodedProperty[]): TableState;
export declare function getTableDepth(paraState: ParaState): number;
export declare function cssTextAlign(value: number | undefined): string;
export declare function cssUnderline(value: number | undefined): string;
export declare function cssVerticalAlign(value: number | undefined): string;
export declare function rangeApply<T>(list: T[], range: {
    first: number;
    lim: number;
} | undefined, callback: (item: T, index: number) => void): void;
export declare function applyTableStateToCells(tableState: TableState): TableCellMeta[];
