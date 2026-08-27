import type { FileViewerErrorMessageFormatter } from './state';
import { type FileViewerI18nInput } from '../i18n/messages';
import type { FileRenderExportAdapter, FileViewerDownloadOptions, FileViewerExportHtmlOptions, FileViewerOperationType, FileViewerPrintOptions, NormalizedFileViewerSource } from '../contracts/types';
export interface FileViewerOriginalSourceState {
    buffer?: ArrayBuffer | null;
    file?: File | Blob | null;
    url?: string | null;
    filename?: string | null;
    mimeType?: string | null;
}
export type CreateFileViewerOriginalSourceStateInput = FileViewerOriginalSourceState;
export declare const DEFAULT_FILE_VIEWER_PREVIEW_TITLE = "file-viewer-preview";
export declare const DEFAULT_FILE_VIEWER_EXPORT_FILENAME = "preview";
export declare const DEFAULT_FILE_VIEWER_DOWNLOAD_FILENAME = "preview.bin";
export interface ResolveFileViewerOperationFilenameInput {
    filename?: string | null;
    source?: FileViewerOriginalSourceState | null;
    fallback?: string;
}
export interface FileViewerOperationExecutorBase {
    beforeOperation?: (operation: FileViewerOperationType) => boolean | Promise<boolean>;
    i18n?: FileViewerI18nInput;
}
export interface ExecuteFileViewerDownloadOperationInput extends FileViewerOperationExecutorBase, FileViewerDownloadOptions {
    source: FileViewerOriginalSourceState;
    throwOnMissingSource?: boolean;
}
export interface ExecuteFileViewerExportHtmlOperationInput extends FileViewerOperationExecutorBase, FileViewerExportHtmlOptions {
    source: HTMLElement | null | undefined;
    adapter?: FileRenderExportAdapter | null;
}
export interface ExecuteFileViewerPrintOperationInput extends FileViewerOperationExecutorBase, FileViewerPrintOptions {
    source: HTMLElement | null | undefined;
    adapter?: FileRenderExportAdapter | null;
    printAvailable?: boolean;
}
export type FileViewerFileOperationType = Extract<FileViewerOperationType, 'download' | 'export-html' | 'print'>;
export interface FileViewerOperationActionErrorContext {
    operation: FileViewerFileOperationType;
    error: unknown;
}
export type FileViewerOperationActionErrorFormatter = FileViewerErrorMessageFormatter;
export type FileViewerOperationActionErrorPrefixes = Partial<Record<FileViewerFileOperationType, string>>;
export declare const FILE_VIEWER_OPERATION_ACTION_ERROR_PREFIXES: {
    readonly download: "下载失败";
    readonly print: "打印失败";
    readonly 'export-html': "导出 HTML 失败";
};
export interface ResolveFileViewerOperationActionErrorMessageInput {
    context: FileViewerOperationActionErrorContext;
    formatErrorMessage: FileViewerOperationActionErrorFormatter;
    prefixes?: FileViewerOperationActionErrorPrefixes;
    i18n?: FileViewerI18nInput;
}
export interface CreateFileViewerOperationActionHandlersInput extends FileViewerOperationExecutorBase {
    getBuffer?: () => ArrayBuffer | null | undefined;
    getFile?: () => File | Blob | null | undefined;
    getUrl?: () => string | null | undefined;
    getI18n?: () => FileViewerI18nInput;
    getFilename: () => string | null | undefined;
    getMimeType?: () => string | null | undefined;
    getRenderedSource: () => HTMLElement | null | undefined;
    getAdapter?: () => FileRenderExportAdapter | null | undefined;
    getWatermarkInlineStyle?: () => string | null | undefined;
    getPrintAvailable?: () => boolean | undefined;
    onError?: (context: FileViewerOperationActionErrorContext) => void;
    formatErrorMessage?: FileViewerOperationActionErrorFormatter;
    errorPrefixes?: FileViewerOperationActionErrorPrefixes;
    onErrorMessage?: (message: string, context: FileViewerOperationActionErrorContext) => void;
}
export interface FileViewerOperationActionHandlers {
    downloadOriginalFile(): Promise<boolean | undefined>;
    exportRenderedHtml(): Promise<string | undefined>;
    printRenderedHtml(options?: FileViewerPrintOptions): Promise<boolean | undefined>;
    printWithMask(options?: FileViewerPrintOptions): Promise<boolean | undefined>;
}
export interface FileViewerPublicOperationActionHandlers {
    downloadOriginalFile(): Promise<void>;
    exportRenderedHtml(): Promise<void>;
    printRenderedHtml(options?: FileViewerPrintOptions): Promise<void>;
    printWithMask(options?: FileViewerPrintOptions): Promise<void>;
}
export declare const createFileViewerOriginalSourceState: ({ buffer, file, url, filename, mimeType, }?: CreateFileViewerOriginalSourceStateInput) => FileViewerOriginalSourceState;
export declare const resolveFileViewerDisplayFilename: (source?: Pick<NormalizedFileViewerSource, "filename"> | null, fallback?: string) => string;
export declare const createFileViewerOriginalSourceStateFromNormalizedSource: (source?: NormalizedFileViewerSource | null, fallbackFilename?: string) => FileViewerOriginalSourceState;
export declare const resolveFileViewerOriginalFilename: (source: FileViewerOriginalSourceState, fallback?: string) => string;
export declare const resolveFileViewerOperationFilename: ({ filename, source, fallback, }: ResolveFileViewerOperationFilenameInput) => string;
export declare const resolveFileViewerOperationActionErrorMessage: ({ context, formatErrorMessage, prefixes, i18n, }: ResolveFileViewerOperationActionErrorMessageInput) => string;
export declare const hasFileViewerOriginalSource: (source: FileViewerOriginalSourceState) => boolean;
export declare const executeFileViewerDownloadOperation: ({ source, filename, beforeOperation, i18n, throwOnMissingSource, }: ExecuteFileViewerDownloadOperationInput) => Promise<boolean>;
export declare const executeFileViewerExportHtmlOperation: ({ download, filename, beforeOperation, i18n, ...input }: ExecuteFileViewerExportHtmlOperationInput) => Promise<string>;
export declare const executeFileViewerPrintOperation: ({ autoPrint, beforeOperation, i18n, openWindow, printAvailable, printWindow, ...input }: ExecuteFileViewerPrintOperationInput) => Promise<boolean>;
export declare const createFileViewerOperationActionHandlers: ({ getBuffer, getFile, getUrl, getI18n, getFilename, getMimeType, getRenderedSource, getAdapter, getWatermarkInlineStyle, getPrintAvailable, beforeOperation, i18n, errorPrefixes, formatErrorMessage, onError, onErrorMessage, }: CreateFileViewerOperationActionHandlersInput) => FileViewerOperationActionHandlers;
export declare const createFileViewerPublicOperationActionHandlers: (input: CreateFileViewerOperationActionHandlersInput) => FileViewerPublicOperationActionHandlers;
