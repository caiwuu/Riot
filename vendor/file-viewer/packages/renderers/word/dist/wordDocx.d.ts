import type { Options, renderAsync } from '@file-viewer/docx';
import { type FileRenderContext, type FileViewerDocxOptions, type FileViewerRenderedInstance as AppWrapper } from '@file-viewer/core';
type DocxRenderAsync = typeof renderAsync;
type DocxExternalLinkPolicy = NonNullable<FileViewerDocxOptions['externalLinkPolicy']>;
type DocxRenderOptions = Partial<Options> & {
    externalLinkPolicy: DocxExternalLinkPolicy;
};
export declare const isMissingDocxHeaderFooterRootError: (error: unknown) => boolean;
/**
 * Some malformed or partially generated DOCX files reference a header/footer
 * part whose parsed root is missing. @file-viewer/docx 0.3.21 skips that part;
 * this retry keeps older installed engines usable while the dependency update
 * rolls through lockfiles and private registries.
 */
export declare const renderDocxWithHeaderFooterFallback: (render: DocxRenderAsync, buffer: ArrayBuffer, target: HTMLDivElement, options: Options) => Promise<boolean>;
/**
 * WPS and Word can store a page background as a document-level VML fill. The
 * DOCX engine intentionally ignores that legacy drawing node, so resolve only
 * its package-local image relationship here and leave all body layout to it.
 */
export declare const resolveDocxPageBackgroundImage: (buffer: ArrayBuffer, createXmlParser?: () => Pick<DOMParser, "parseFromString">) => Promise<string | undefined>;
export declare const applyDocxPageBackgroundImage: (target: HTMLDivElement, imageUrl: string | undefined) => number;
export declare const applyDocxExternalLinkPolicy: (target: Pick<ParentNode, "querySelectorAll">, policy: DocxExternalLinkPolicy) => number;
export declare const createDocxOptions: (target: HTMLDivElement, context: FileRenderContext | undefined, notifyProgressiveRender: () => void) => DocxRenderOptions;
/**
 * 渲染docx文件
 */
export default function (buffer: ArrayBuffer, target: HTMLDivElement, context?: FileRenderContext): Promise<AppWrapper>;
export {};
