import type { FibRgFcLcb, ParsedClx, PieceDescriptor, PieceTable } from '../types.js';
export declare function parseClx(tableBytes: Uint8Array, fibRgFcLcb: FibRgFcLcb): ParsedClx;
export declare function parsePlcPcd(bytes: Uint8Array): PieceTable;
export declare function extractDocumentText(wordBytes: Uint8Array, clx: ParsedClx): string;
export declare function extractPieceText(wordBytes: Uint8Array, piece: PieceDescriptor): string;
export declare function buildPieceTextCache(wordBytes: Uint8Array, clx: ParsedClx): string[];
export declare function cpToFileOffset(clx: ParsedClx, cp: number): {
    piece: PieceDescriptor;
    offset: number;
    compressed: boolean;
} | null;
export declare function fcToCp(clx: ParsedClx, fcRawOrActual: number, explicitCompressed?: boolean | null): number | null;
export declare function getTextByCp(_wordBytes: Uint8Array, clx: ParsedClx, pieceTexts: string[], cpStart: number, cpEnd: number): string;
export declare function splitParagraphRanges(documentText: string): Array<{
    cpStart: number;
    cpEnd: number;
    terminator: string;
}>;
