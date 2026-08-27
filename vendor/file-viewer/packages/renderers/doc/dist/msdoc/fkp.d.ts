import type { DecodedProperty, ParsedClx, ParsedFib } from '../types.js';
export interface ChpxRun {
    cpStart: number;
    cpEnd: number;
    fcStart: number;
    fcEnd: number;
    properties: DecodedProperty[];
}
export interface PapxRun extends ChpxRun {
    styleId: number;
    rawOffset: number;
}
export declare function readChpxRuns(wordBytes: Uint8Array, tableBytes: Uint8Array, fib: ParsedFib, clx: ParsedClx): ChpxRun[];
export declare function readPapxRuns(wordBytes: Uint8Array, tableBytes: Uint8Array, fib: ParsedFib, clx: ParsedClx, dataBytes?: Uint8Array): PapxRun[];
export declare function readChpxProperties(wordBytes: Uint8Array, offset: number | null | undefined): DecodedProperty[];
export declare function readPapxProperties(wordBytes: Uint8Array, offset: number | null | undefined, dataBytes?: Uint8Array): {
    styleId: number;
    properties: DecodedProperty[];
};
