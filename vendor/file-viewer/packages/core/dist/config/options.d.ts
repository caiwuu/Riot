import type { FileViewerArchiveOptions, FileViewerCadOptions, FileViewerOptions, FileViewerResolvedThemeMode, FileViewerThemeMode, FileViewerToolbarOptions, FileViewerUiDensity } from '../contracts/types';
export type FileViewerSerializableToolbarOptions = Omit<FileViewerToolbarOptions, 'beforeOperation' | 'beforeDownload' | 'beforePrint' | 'beforeExportHtml'>;
export type FileViewerSerializableCadOptions = Omit<FileViewerCadOptions, 'workerUrl'> & {
    workerUrl?: string;
};
export interface FileViewerSerializableOptions extends Omit<FileViewerOptions, 'toolbar' | 'cad' | 'hooks' | 'beforeOperation' | 'preset' | 'presets' | 'renderers' | 'rendererMode'> {
    toolbar?: boolean | FileViewerSerializableToolbarOptions;
    archive?: FileViewerArchiveOptions;
    cad?: FileViewerSerializableCadOptions;
}
export declare const normalizeFileViewerTheme: (theme: FileViewerThemeMode | undefined) => FileViewerThemeMode;
export declare const resolveFileViewerColorScheme: (theme: FileViewerThemeMode | undefined, systemDark?: boolean) => FileViewerResolvedThemeMode;
export declare const toggleFileViewerColorScheme: (theme: FileViewerThemeMode | undefined, systemDark?: boolean) => FileViewerResolvedThemeMode;
export declare const normalizeFileViewerUiDensity: (density: FileViewerUiDensity | undefined) => FileViewerUiDensity;
export declare const resolveFileViewerUiDensity: (options?: Pick<FileViewerOptions, "ui"> | null) => FileViewerUiDensity;
export declare const sanitizeFileViewerOptions: (options?: FileViewerOptions | FileViewerSerializableOptions | null) => FileViewerSerializableOptions | undefined;
export declare const serializeFileViewerOptions: (options?: FileViewerOptions | FileViewerSerializableOptions | null) => string | undefined;
export declare const parseFileViewerOptions: (value: unknown) => FileViewerSerializableOptions | undefined;
export declare const setFileViewerOptionsSearchParam: (searchParams: URLSearchParams, options?: FileViewerOptions | FileViewerSerializableOptions | null, key?: string) => void;
export declare const getFileViewerOptionsSearchParam: (searchParams: URLSearchParams, key?: string) => FileViewerSerializableOptions | undefined;
