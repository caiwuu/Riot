import type { MsDocParseResult } from '../types.js';
export declare function isHtmlWordDocumentStream(bytes: Uint8Array): boolean;
export declare function parseHtmlWordDocumentStream(bytes: Uint8Array): MsDocParseResult | null;
