import type { FileViewerI18nOptions, FileViewerLocale, FileViewerMessageKey, FileViewerMessageParams, FileViewerMessages, FileViewerOptions } from '../contracts/types';
export declare const FILE_VIEWER_SUPPORTED_LOCALES: readonly ["zh-CN", "en-US", "ja-JP", "de-DE"];
export type FileViewerResolvedLocale = typeof FILE_VIEWER_SUPPORTED_LOCALES[number];
export interface ResolveFileViewerI18nInput {
    locale?: FileViewerLocale;
    messages?: FileViewerMessages;
    i18n?: FileViewerI18nOptions;
}
export type FileViewerI18nInput = ResolveFileViewerI18nInput | FileViewerOptions | undefined | null;
export declare const FILE_VIEWER_DEFAULT_LOCALE: FileViewerResolvedLocale;
export declare const FILE_VIEWER_FALLBACK_LOCALE: FileViewerResolvedLocale;
export declare const FILE_VIEWER_BUILTIN_MESSAGES: Readonly<{
    'zh-CN': Record<FileViewerMessageKey, string>;
    'en-US': Record<FileViewerMessageKey, string>;
    'ja-JP': Record<FileViewerMessageKey, string>;
    'de-DE': Record<FileViewerMessageKey, string>;
}>;
export declare const normalizeFileViewerLocale: (locale?: FileViewerLocale | null) => FileViewerResolvedLocale;
export declare const resolveFileViewerLocale: (input?: FileViewerI18nInput) => "zh-CN" | "en-US" | "ja-JP" | "de-DE";
export declare const formatFileViewerMessage: (message: string, params?: FileViewerMessageParams) => string;
export declare const translateFileViewerMessage: (input: FileViewerI18nInput, key: FileViewerMessageKey, params?: FileViewerMessageParams) => string;
export declare const createFileViewerTranslator: (input?: FileViewerI18nInput) => (key: FileViewerMessageKey, params?: FileViewerMessageParams) => string;
