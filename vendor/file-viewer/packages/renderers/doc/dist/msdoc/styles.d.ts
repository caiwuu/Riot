import type { DecodedProperty, FibRgFcLcb, ResolvedStyle, StyleCollection, StyleDefinition } from '../types.js';
declare function mergePropertyArrays(...arrays: Array<DecodedProperty[] | undefined | null>): DecodedProperty[];
declare function splitPropertiesByKind(properties: DecodedProperty[]): {
    para: DecodedProperty[];
    char: DecodedProperty[];
    table: DecodedProperty[];
};
export declare function parseStyles(tableBytes: Uint8Array, fibRgFcLcb: FibRgFcLcb): StyleCollection;
export declare function resolveStyle(styleMap: Map<number, StyleDefinition>, istd: number | null | undefined, seen?: Set<number>): ResolvedStyle;
export { mergePropertyArrays, splitPropertiesByKind };
