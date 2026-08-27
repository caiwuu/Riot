import type { SheetImage } from './worker/type.js';
export interface SpreadsheetImageSourceResolver {
    resolve(image: Pick<SheetImage, 'src' | 'contentType'>): Promise<string>;
    dispose(): void;
}
export declare const createSpreadsheetImageSourceResolver: (documentRef: Document) => SpreadsheetImageSourceResolver;
