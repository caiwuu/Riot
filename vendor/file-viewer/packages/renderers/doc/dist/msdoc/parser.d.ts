import type { MsDocParseOptions, MsDocParseResult, ParagraphModel, TableRowBlock } from '../types.js';
export declare function finalizeTableGrid(rows: TableRowBlock[]): void;
export declare function normalizeRowEndOnlyTables(paragraphs: ParagraphModel[]): void;
/**
 * Main MS-DOC entry point.
 * It parses the OLE container, restores text through the piece table, resolves
 * paragraph/character/table properties, and finally produces a normalized AST
 * that the HTML renderer can consume.
 */
export declare function parseMsDoc(input: ArrayBuffer | Uint8Array | ArrayBufferView, options?: MsDocParseOptions): MsDocParseResult;
