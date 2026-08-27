import type { FileViewerArchiveOptions, FileViewerCadOptions, FileViewerDataOptions, FileViewerDocxOptions, FileViewerDrawingOptions, FileViewerIworkOptions, FileViewerHangulOptions, FileViewerModelOptions, FileViewerOptions, FileViewerPdfOptions, FileViewerPresentationOptions, FileViewerSpreadsheetOptions, FileViewerTypstOptions, FileViewerWordPerfectOptions } from '../contracts/types';
export declare const DEFAULT_FILE_VIEWER_ARCHIVE_WORKER_PATH = "vendor/libarchive/worker-bundle.js";
export declare const DEFAULT_FILE_VIEWER_ARCHIVE_WASM_PATH = "vendor/libarchive/libarchive.wasm";
export declare const DEFAULT_FILE_VIEWER_DOCX_WORKER_PATH = "vendor/docx/docx.worker.js";
export declare const DEFAULT_FILE_VIEWER_DOCX_WORKER_JSZIP_PATH = "vendor/docx/jszip.min.js";
export declare const DEFAULT_FILE_VIEWER_DOCX_RUNTIME_VERSION = "0.3.26";
export declare const DEFAULT_FILE_VIEWER_PRESENTATION_WORKER_PATH = "vendor/pptx/pptx.worker.js";
export declare const DEFAULT_FILE_VIEWER_PPT_RUNTIME_PATH = "vendor/ppt";
export declare const DEFAULT_FILE_VIEWER_PPT_RUNTIME_VERSION = "0.3.3";
export declare const DEFAULT_FILE_VIEWER_PPT_MODULE_PATH = "vendor/ppt/index.mjs";
export declare const DEFAULT_FILE_VIEWER_PPT_WORKER_PATH = "vendor/ppt/worker.mjs";
export declare const DEFAULT_FILE_VIEWER_PPT_FRAME_CACHE_PATH = "vendor/ppt/frame-cache.mjs";
export declare const DEFAULT_FILE_VIEWER_PPT_WASM_PATH = "vendor/ppt/ppt-native.wasm";
export declare const DEFAULT_FILE_VIEWER_PPT_FONT_PATH = "vendor/ppt/ppt-font-cjk.otf";
export declare const DEFAULT_FILE_VIEWER_SPREADSHEET_WORKER_PATH = "vendor/xlsx/sheet.worker.js";
export declare const DEFAULT_FILE_VIEWER_IWORK_WORKER_PATH = "vendor/iwork/iwork.worker.js";
export declare const DEFAULT_FILE_VIEWER_IWORK_WORKER_PACKAGE_PATH = "@file-viewer/renderer-iwork/worker/iwork.worker.js";
export declare const DEFAULT_FILE_VIEWER_HANGUL_WORKER_PATH = "vendor/hangul/hangul.worker.js";
export declare const DEFAULT_FILE_VIEWER_HANGUL_WORKER_PACKAGE_PATH = "@file-viewer/renderer-hangul/worker/hangul.worker.js";
export declare const DEFAULT_FILE_VIEWER_WORDPERFECT_WORKER_PATH = "vendor/wordperfect/wordperfect.worker.js";
export declare const DEFAULT_FILE_VIEWER_WORDPERFECT_WORKER_PACKAGE_PATH = "@file-viewer/renderer-wordperfect/worker/wordperfect.worker.js";
export declare const DEFAULT_FILE_VIEWER_WORDPERFECT_WASM_PATH = "vendor/wordperfect/libwpd.wasm";
export declare const DEFAULT_FILE_VIEWER_WORDPERFECT_WASM_PACKAGE_PATH = "@file-viewer/renderer-wordperfect/worker/libwpd.wasm";
export declare const DEFAULT_FILE_VIEWER_WORDPERFECT_MODULE_PATH = "vendor/wordperfect/libwpd.mjs";
export declare const DEFAULT_FILE_VIEWER_WORDPERFECT_MODULE_PACKAGE_PATH = "@file-viewer/renderer-wordperfect/worker/libwpd.mjs";
export declare const DEFAULT_FILE_VIEWER_PDF_WORKER_PATH = "vendor/pdf/pdf.worker.mjs";
export declare const DEFAULT_FILE_VIEWER_PDF_CMAP_PATH = "vendor/pdf/cmaps/";
export declare const DEFAULT_FILE_VIEWER_PDF_WASM_PATH = "vendor/pdf/wasm/";
export declare const DEFAULT_FILE_VIEWER_PDF_STANDARD_FONT_PATH = "vendor/pdf/standard_fonts/";
export declare const DEFAULT_FILE_VIEWER_PDF_CJK_FONT_FALLBACK_PATH = "vendor/pdf/fonts/";
export declare const DEFAULT_FILE_VIEWER_DRAWIO_VIEWER_SCRIPT_PATH = "vendor/drawio/viewer-static.min.js";
export declare const DEFAULT_FILE_VIEWER_DRAWIO_ASSET_PATH = "vendor/drawio/";
export declare const DEFAULT_FILE_VIEWER_CAD_RUNTIME_VERSION = "0.8.0";
export declare const DEFAULT_FILE_VIEWER_CAD_WASM_PATH = "wasm/cad/0.8.0/";
export declare const DEFAULT_FILE_VIEWER_CAD_WORKER_PATH = "wasm/cad/0.8.0/dwg-worker.js";
export declare const DEFAULT_FILE_VIEWER_CAD_DWF_WASM_PATH = "wasm/cad/0.8.0/dwfv-render.wasm";
export declare const DEFAULT_FILE_VIEWER_CAD_LIBREDWG_SCRIPT_PATH = "wasm/cad/0.8.0/libredwg-web.js";
export declare const DEFAULT_FILE_VIEWER_CAD_LIBREDWG_WASM_PATH = "wasm/cad/0.8.0/libredwg-web.wasm";
export declare const DEFAULT_FILE_VIEWER_TYPST_COMPILER_WASM_URL = "wasm/typst/typst_ts_web_compiler_bg.wasm";
export declare const DEFAULT_FILE_VIEWER_TYPST_RENDERER_WASM_URL = "wasm/typst/typst_ts_renderer_bg.wasm";
export declare const DEFAULT_FILE_VIEWER_TYPST_FONT_ASSETS_URL = "wasm/typst/fonts/";
export declare const FALLBACK_FILE_VIEWER_TYPST_COMPILER_WASM_URL = "wasm/typst/typst_ts_web_compiler_bg.wasm";
export declare const FALLBACK_FILE_VIEWER_TYPST_RENDERER_WASM_URL = "wasm/typst/typst_ts_renderer_bg.wasm";
export declare const DEFAULT_FILE_VIEWER_TYPST_COMPILER_WASM_PACKAGE_PATH = "@myriaddreamin/typst-ts-web-compiler/pkg/typst_ts_web_compiler_bg.wasm";
export declare const DEFAULT_FILE_VIEWER_TYPST_RENDERER_WASM_PACKAGE_PATH = "@myriaddreamin/typst-ts-renderer/pkg/typst_ts_renderer_bg.wasm";
export declare const DEFAULT_FILE_VIEWER_DATA_SQL_WASM_URL = "wasm/data/sql-wasm.wasm";
export declare const DEFAULT_FILE_VIEWER_DATA_SQL_WASM_PACKAGE_PATH = "sql.js/dist/sql-wasm.wasm";
export declare const DEFAULT_FILE_VIEWER_MODEL_WORKER_URL = "wasm/model/occt-worker.js";
export declare const DEFAULT_FILE_VIEWER_MODEL_RUNTIME_URL = "wasm/model/occt-import-js.js";
export declare const DEFAULT_FILE_VIEWER_MODEL_WASM_URL = "wasm/model/occt-import-js.wasm";
export declare const DEFAULT_FILE_VIEWER_MODEL_OCCT_LICENSE_URL = "wasm/model/LICENSE.occt.txt";
export declare const DEFAULT_FILE_VIEWER_MODEL_IMPORT_LICENSE_URL = "wasm/model/LICENSE.occt-import-js.txt";
export declare const DEFAULT_FILE_VIEWER_MODEL_RUNTIME_PACKAGE_PATH = "occt-import-js/dist/occt-import-js.js";
export declare const DEFAULT_FILE_VIEWER_MODEL_WASM_PACKAGE_PATH = "occt-import-js/dist/occt-import-js.wasm";
export declare const DEFAULT_FILE_VIEWER_MODEL_OCCT_LICENSE_PACKAGE_PATH = "occt-import-js/dist/license.occt.txt";
export declare const DEFAULT_FILE_VIEWER_MODEL_IMPORT_LICENSE_PACKAGE_PATH = "occt-import-js/dist/license.occt-import-js.txt";
export interface ResolveFileViewerAssetUrlOptions {
    baseUrl?: string;
    documentBaseUrl?: string;
    trimTrailingSlash?: boolean;
}
export declare const normalizeFileViewerAssetBaseUrl: (baseUrl?: string | URL | null) => string | undefined;
export declare const setDefaultFileViewerAssetBaseUrl: (baseUrl?: string | URL | null) => void;
export declare const resetDefaultFileViewerAssetBaseUrl: () => void;
/**
 * Resolves the stable public base for runtime assets without reading
 * bundler-specific environment metadata or webpack public-path variables.
 * Explicit HTML `<base>` configuration stays authoritative; for SPA fallback
 * routes, emitted Vite/Webpack/UMI entry scripts reveal the deployment root
 * more reliably than the route-derived page URL.
 */
export declare const resolveFileViewerRuntimeAssetBaseUrl: (documentRef: Document) => string;
export declare const getDefaultFileViewerAssetBaseUrl: (documentRef?: Document | null) => string | undefined;
export interface ResolvedFileViewerCadAssetUrls {
    wasmPath: string;
    workerUrl: string;
    dwfWasmUrl: string;
}
export interface ResolvedFileViewerPdfAssetUrls {
    workerUrl: string;
    cMapUrl: string;
    wasmUrl: string;
    standardFontDataUrl: string;
    cjkFontFallbackPath: string;
}
export interface ResolvedFileViewerModelAssetUrls {
    workerUrl: string;
    runtimeUrl: string;
    wasmUrl: string;
}
export type FileViewerRendererAssetKind = 'directory' | 'worker' | 'wasm' | 'wasm-directory' | 'script' | 'font' | 'metadata' | 'bundled-wasm' | 'license';
export type FileViewerRendererAssetTarget = 'public' | 'bundled' | 'external';
export type FileViewerRendererAssetOptionPath = 'archive.workerUrl' | 'archive.wasmUrl' | 'cad.wasmPath' | 'cad.workerUrl' | 'cad.dwfWasmUrl' | 'data.sqlWasmUrl' | 'docx.workerJsZipUrl' | 'docx.workerUrl' | 'drawing.viewerScriptUrl' | 'iwork.workerUrl' | 'hangul.workerUrl' | 'wordPerfect.workerUrl' | 'wordPerfect.wasmUrl' | 'model.workerUrl' | 'model.runtimeUrl' | 'model.wasmUrl' | 'pdf.workerUrl' | 'pdf.cMapUrl' | 'pdf.wasmUrl' | 'pdf.standardFontDataUrl' | 'pdf.cjkFontFallbackPath' | 'presentation.pptModuleUrl' | 'presentation.pptWorkerUrl' | 'presentation.pptWasmUrl' | 'presentation.pptFontUrl' | 'presentation.workerUrl' | 'spreadsheet.workerUrl' | 'typst.compilerWasmUrl' | 'typst.fontAssetsUrl' | 'typst.rendererWasmUrl';
export interface FileViewerRendererAssetDefinition {
    id: string;
    rendererId: string;
    kind: FileViewerRendererAssetKind;
    target: FileViewerRendererAssetTarget;
    required: boolean;
    defaultPath?: string;
    defaultUrl?: string;
    packagePath?: string;
    optionPath?: FileViewerRendererAssetOptionPath;
    description: string;
}
export interface FileViewerRendererAssetManifest {
    rendererId: string;
    assets: readonly FileViewerRendererAssetDefinition[];
}
export interface ResolvedFileViewerRendererAsset extends FileViewerRendererAssetDefinition {
    configured: boolean;
    url?: string;
    packagePath?: string;
}
export interface ResolveFileViewerRendererAssetsOptions extends ResolveFileViewerAssetUrlOptions {
    options?: FileViewerOptions | null;
}
export declare const DEFAULT_FILE_VIEWER_RENDERER_ASSET_MANIFESTS: readonly FileViewerRendererAssetManifest[];
export declare const resolveFileViewerAssetUrl: (value: string | URL | undefined, fallback: string, options?: ResolveFileViewerAssetUrlOptions) => string;
export declare const resolveFileViewerArchiveWorkerUrl: (options?: Pick<FileViewerArchiveOptions, "workerUrl"> | null, baseUrl?: string) => string;
export declare const resolveFileViewerArchiveWasmUrl: (options?: Pick<FileViewerArchiveOptions, "wasmUrl"> | null, fallback?: string, documentBaseUrl?: string) => string;
export declare const resolveFileViewerCadAssetUrls: (options?: Pick<FileViewerCadOptions, "wasmPath" | "workerUrl" | "dwfWasmUrl"> | null, documentBaseUrl?: string) => ResolvedFileViewerCadAssetUrls;
export declare const resolveFileViewerPdfAssetUrls: (options?: Pick<FileViewerPdfOptions, "assetBaseUrl" | "workerUrl" | "cMapUrl" | "wasmUrl" | "standardFontDataUrl" | "cjkFontFallbackPath"> | null, documentBaseUrl?: string) => ResolvedFileViewerPdfAssetUrls;
export declare const resolveFileViewerDrawioViewerScriptUrl: (options?: Pick<FileViewerDrawingOptions, "viewerScriptUrl"> | null, documentBaseUrl?: string) => string;
export declare const resolveFileViewerDocxWorkerUrl: (options?: Pick<FileViewerDocxOptions, "workerUrl"> | null, documentBaseUrl?: string) => string;
export declare const resolveFileViewerDocxWorkerJsZipUrl: (options?: Pick<FileViewerDocxOptions, "workerJsZipUrl"> | null, documentBaseUrl?: string) => string;
export declare const resolveFileViewerSpreadsheetWorkerUrl: (options?: Pick<FileViewerSpreadsheetOptions, "workerUrl"> | null, documentBaseUrl?: string) => string;
export declare const resolveFileViewerIworkWorkerUrl: (options?: Pick<FileViewerIworkOptions, "workerUrl"> | null, documentBaseUrl?: string) => string;
export declare const resolveFileViewerHangulWorkerUrl: (options?: Pick<FileViewerHangulOptions, "workerUrl"> | null, documentBaseUrl?: string) => string;
export declare const resolveFileViewerWordPerfectWorkerUrl: (options?: Pick<FileViewerWordPerfectOptions, "workerUrl"> | null, documentBaseUrl?: string) => string;
export declare const resolveFileViewerWordPerfectWasmUrl: (options?: Pick<FileViewerWordPerfectOptions, "wasmUrl"> | null, documentBaseUrl?: string) => string;
export declare const resolveFileViewerPresentationWorkerUrl: (options?: Pick<FileViewerPresentationOptions, "workerUrl"> | null, documentBaseUrl?: string) => string;
export declare const resolveFileViewerTypstCompilerWasmUrl: (options?: Pick<FileViewerTypstOptions, "compilerWasmUrl"> | null, overrides?: Array<string | undefined>, documentBaseUrl?: string) => string;
export declare const resolveFileViewerTypstRendererWasmUrl: (options?: Pick<FileViewerTypstOptions, "rendererWasmUrl"> | null, overrides?: Array<string | undefined>, documentBaseUrl?: string) => string;
export declare const resolveFileViewerTypstFontAssetsUrl: (options?: Pick<FileViewerTypstOptions, "fontAssetsUrl"> | null, overrides?: Array<string | undefined>, documentBaseUrl?: string) => string;
export declare const resolveFileViewerDataSqlWasmUrl: (options?: Pick<FileViewerDataOptions, "sqlWasmUrl"> | null, overrides?: Array<string | undefined>, documentBaseUrl?: string) => string;
export declare const resolveFileViewerModelAssetUrls: (options?: Pick<FileViewerModelOptions, "workerUrl" | "runtimeUrl" | "wasmUrl"> | null, documentBaseUrl?: string) => ResolvedFileViewerModelAssetUrls;
export declare const listFileViewerRendererAssetManifests: () => FileViewerRendererAssetManifest[];
export declare const getFileViewerRendererAssetManifest: (rendererId: string) => FileViewerRendererAssetManifest | null;
export declare const resolveFileViewerRendererAssets: (rendererId: string, resolveOptions?: ResolveFileViewerRendererAssetsOptions) => ResolvedFileViewerRendererAsset[];
