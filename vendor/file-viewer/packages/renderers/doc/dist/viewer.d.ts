import type { MsDocParseToHtmlOptions, MsDocRenderResult, MsDocViewer, MsDocViewerConfig, ViewerInput } from './types.js';
export declare function sanitizeMsDocHtml(html: string, targetWindow: Window): DocumentFragment;
export declare function mountMsDoc(container: HTMLElement, rendered: MsDocRenderResult): HTMLElement;
export declare function parseMsDocToHtml(input: ViewerInput, options?: MsDocParseToHtmlOptions): Promise<MsDocRenderResult>;
/**
 * Small DOM-oriented helper that keeps browser integration trivial.
 * Apps can either use it directly or consume the lower-level parse/render APIs.
 */
export declare function createMsDocViewer(container: HTMLElement, config?: MsDocViewerConfig): MsDocViewer;
