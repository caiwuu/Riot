import { type ResolvedFileViewerTextEncoding } from '@file-viewer/core';
export type SpreadsheetTextEncoding = 'auto' | 'utf-8' | 'gbk' | 'gb18030';
export interface SpreadsheetTextSource {
    fileType?: string;
    filename?: string;
    textEncoding?: SpreadsheetTextEncoding;
}
export interface DecodedSpreadsheetText {
    text: string;
    encoding: ResolvedFileViewerTextEncoding;
}
export type PreparedSpreadsheetReadInput = {
    kind: 'binary';
    data: ArrayBuffer;
} | {
    kind: 'text';
    data: string;
    encoding: DecodedSpreadsheetText['encoding'];
};
export declare const isTextSpreadsheetSource: ({ fileType, filename, }: Pick<SpreadsheetTextSource, "fileType" | "filename">) => boolean;
export declare const isValidUtf8: (bytes: Uint8Array) => boolean;
export declare const decodeSpreadsheetText: (data: ArrayBuffer, encoding?: SpreadsheetTextEncoding) => DecodedSpreadsheetText;
export declare const prepareSpreadsheetReadInput: (data: ArrayBuffer, source?: SpreadsheetTextSource) => PreparedSpreadsheetReadInput;
