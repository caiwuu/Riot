import { type FileViewerI18nInput } from '../i18n/messages';
import type { FileViewerRendererCategory, FileViewerStateDescriptor, FileViewerStateTheme } from '../contracts/types';
export type FileViewerErrorMessageFormatter = (prefix: string, error: unknown, i18n?: FileViewerI18nInput) => string;
export declare const FILE_VIEWER_PREVIEW_MESSAGES: Readonly<{
    downloading: "正在下载文件资源...";
    streamingPdf: "正在建立 PDF 流式预览...";
    reading: "正在解析文件内容...";
}>;
export declare const resolveFileViewerPreviewMessages: (i18n?: FileViewerI18nInput) => Readonly<{
    downloading: string;
    streamingPdf: string;
    reading: string;
}>;
export declare const DEFAULT_FILE_VIEWER_STATE_THEME: FileViewerStateTheme;
export declare const DEFAULT_FILE_VIEWER_UNSUPPORTED_DESCRIPTION = "\u652F\u6301 Office\u3001PDF\u3001OFD\u3001Typst\u3001\u538B\u7F29\u5305\u3001\u90AE\u4EF6\u3001OLB/DRA\u3001CAD\u3001\u5730\u7406\u6570\u636E\u30013D \u6A21\u578B\u3001Excalidraw\u3001draw.io\u3001EPUB\u3001UMD\u3001Markdown\u3001\u4EE3\u7801/\u6587\u672C\u3001\u56FE\u7247\u3001\u97F3\u89C6\u9891\u3001\u5B57\u4F53\u548C\u6570\u636E\u8D44\u4EA7\u7684\u5728\u7EBF\u9884\u89C8";
export interface FileViewerRendererInstallHint {
    extension: string;
    rendererId: string;
    rendererLabel: string;
    rendererCategory: FileViewerRendererCategory;
    rendererPackage?: string;
    presetPackage: string;
    vitePreset: string;
    presetLabel: string;
}
export declare const resolveFileViewerRendererInstallHint: (extension?: string) => FileViewerRendererInstallHint | null;
export declare const createFileViewerPreviewLoadingState: (extension?: string, message?: string, theme?: FileViewerStateTheme, i18n?: FileViewerI18nInput) => FileViewerStateDescriptor;
export declare const createFileViewerReadyState: (extension?: string, theme?: FileViewerStateTheme, i18n?: FileViewerI18nInput) => FileViewerStateDescriptor;
export declare const createFileViewerEmptyState: (extension?: string, theme?: FileViewerStateTheme, i18n?: FileViewerI18nInput) => FileViewerStateDescriptor;
export declare const createFileViewerUnsupportedState: (extension?: string, theme?: FileViewerStateTheme, i18n?: FileViewerI18nInput) => FileViewerStateDescriptor;
export declare const normalizeFileViewerErrorMessage: (error: unknown, i18n?: FileViewerI18nInput) => string;
export declare const formatFileViewerErrorMessage: FileViewerErrorMessageFormatter;
export declare const createFileViewerErrorState: (extension?: string, error?: unknown, theme?: FileViewerStateTheme, i18n?: FileViewerI18nInput) => FileViewerStateDescriptor;
