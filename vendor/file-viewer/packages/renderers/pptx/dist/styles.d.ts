/**
 * The legacy engine emits class-only selector lists. Prefix each top-level
 * class rule so light-DOM consumers do not inherit generic `.slide` or
 * generated `._css_*` rules from a renderer-local style element.
 */
export declare const scopePptxContentStyleText: (cssText: string) => string;
export declare const pptxViewerCss: string;
export declare const ensurePptxViewerStyles: (documentRef: Document, root?: Document | ShadowRoot) => void;
