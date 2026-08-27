import type { MsDocParseResult, MsDocRenderOptions, MsDocRenderResult } from '../types.js';
type ExternalLinkPolicy = NonNullable<MsDocRenderOptions['externalLinkPolicy']>;
/**
 * Normalizes a document-provided hyperlink before it reaches generated HTML.
 * Internal bookmarks always remain available. External navigation is opt-in
 * and is still restricted to browser-safe schemes and relative paths.
 */
export declare function sanitizeMsDocLinkHref(href: string | undefined | null, policy?: ExternalLinkPolicy): string | null;
export declare function defaultMsDocCss(): string;
/**
 * Converts the parsed AST into HTML and a companion CSS string.
 * Keeping rendering separate from parsing makes it easier for downstream apps
 * to customize styles or consume the AST directly.
 */
export declare function renderMsDoc(parsed: MsDocParseResult, options?: MsDocRenderOptions): MsDocRenderResult;
export {};
