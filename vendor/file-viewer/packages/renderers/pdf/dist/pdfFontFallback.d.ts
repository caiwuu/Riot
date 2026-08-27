type PdfTextContentItem = {
    str?: string;
    fontName?: string;
};
type PdfTextContentStyle = {
    fontSubstitution?: string;
};
export type PdfTextContent = {
    items?: PdfTextContentItem[];
    styles?: Record<string, PdfTextContentStyle>;
};
export type PdfTextContentPage = {
    getTextContent: () => Promise<PdfTextContent>;
};
export interface PdfCjkFontFallbackManager {
    prepare: () => Promise<boolean>;
    ensureTextContent: (textContent: PdfTextContent) => Promise<boolean>;
    ensurePage: (page: PdfTextContentPage) => Promise<boolean>;
}
export interface CreatePdfCjkFontFallbackManagerOptions {
    documentRef: Document;
    fontAssetPath: string;
    onWarning?: (message: string, error?: unknown) => void;
}
/**
 * Some PDF generators write TrueType glyph IDs through Identity-H but omit
 * ToUnicode. PDF.js then exposes those glyph IDs as control characters, so a
 * replacement font alone cannot recover the intended text.
 */
export declare const detectMalformedIdentityCjkFontFamilies: (textContent: PdfTextContent, resolveFontFamily?: (fontName: string) => string) => string[];
export declare const collectMalformedIdentityFontNames: (textContent: PdfTextContent) => string[];
export declare const createPdfCjkFontFallbackManager: ({ documentRef, fontAssetPath, onWarning, }: CreatePdfCjkFontFallbackManagerOptions) => PdfCjkFontFallbackManager;
export {};
