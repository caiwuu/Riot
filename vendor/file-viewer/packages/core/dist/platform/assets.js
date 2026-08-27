export const DEFAULT_FILE_VIEWER_ARCHIVE_WORKER_PATH = 'vendor/libarchive/worker-bundle.js';
export const DEFAULT_FILE_VIEWER_ARCHIVE_WASM_PATH = 'vendor/libarchive/libarchive.wasm';
export const DEFAULT_FILE_VIEWER_DOCX_WORKER_PATH = 'vendor/docx/docx.worker.js';
export const DEFAULT_FILE_VIEWER_DOCX_WORKER_JSZIP_PATH = 'vendor/docx/jszip.min.js';
export const DEFAULT_FILE_VIEWER_DOCX_RUNTIME_VERSION = '0.3.26';
export const DEFAULT_FILE_VIEWER_PRESENTATION_WORKER_PATH = 'vendor/pptx/pptx.worker.js';
export const DEFAULT_FILE_VIEWER_PPT_RUNTIME_PATH = 'vendor/ppt';
export const DEFAULT_FILE_VIEWER_PPT_RUNTIME_VERSION = '0.3.3';
export const DEFAULT_FILE_VIEWER_PPT_MODULE_PATH = `${DEFAULT_FILE_VIEWER_PPT_RUNTIME_PATH}/index.mjs`;
export const DEFAULT_FILE_VIEWER_PPT_WORKER_PATH = `${DEFAULT_FILE_VIEWER_PPT_RUNTIME_PATH}/worker.mjs`;
export const DEFAULT_FILE_VIEWER_PPT_FRAME_CACHE_PATH = `${DEFAULT_FILE_VIEWER_PPT_RUNTIME_PATH}/frame-cache.mjs`;
export const DEFAULT_FILE_VIEWER_PPT_WASM_PATH = `${DEFAULT_FILE_VIEWER_PPT_RUNTIME_PATH}/ppt-native.wasm`;
export const DEFAULT_FILE_VIEWER_PPT_FONT_PATH = `${DEFAULT_FILE_VIEWER_PPT_RUNTIME_PATH}/ppt-font-cjk.otf`;
export const DEFAULT_FILE_VIEWER_SPREADSHEET_WORKER_PATH = 'vendor/xlsx/sheet.worker.js';
export const DEFAULT_FILE_VIEWER_IWORK_WORKER_PATH = 'vendor/iwork/iwork.worker.js';
export const DEFAULT_FILE_VIEWER_IWORK_WORKER_PACKAGE_PATH = '@file-viewer/renderer-iwork/worker/iwork.worker.js';
export const DEFAULT_FILE_VIEWER_HANGUL_WORKER_PATH = 'vendor/hangul/hangul.worker.js';
export const DEFAULT_FILE_VIEWER_HANGUL_WORKER_PACKAGE_PATH = '@file-viewer/renderer-hangul/worker/hangul.worker.js';
export const DEFAULT_FILE_VIEWER_WORDPERFECT_WORKER_PATH = 'vendor/wordperfect/wordperfect.worker.js';
export const DEFAULT_FILE_VIEWER_WORDPERFECT_WORKER_PACKAGE_PATH = '@file-viewer/renderer-wordperfect/worker/wordperfect.worker.js';
export const DEFAULT_FILE_VIEWER_WORDPERFECT_WASM_PATH = 'vendor/wordperfect/libwpd.wasm';
export const DEFAULT_FILE_VIEWER_WORDPERFECT_WASM_PACKAGE_PATH = '@file-viewer/renderer-wordperfect/worker/libwpd.wasm';
export const DEFAULT_FILE_VIEWER_WORDPERFECT_MODULE_PATH = 'vendor/wordperfect/libwpd.mjs';
export const DEFAULT_FILE_VIEWER_WORDPERFECT_MODULE_PACKAGE_PATH = '@file-viewer/renderer-wordperfect/worker/libwpd.mjs';
export const DEFAULT_FILE_VIEWER_PDF_WORKER_PATH = 'vendor/pdf/pdf.worker.mjs';
export const DEFAULT_FILE_VIEWER_PDF_CMAP_PATH = 'vendor/pdf/cmaps/';
export const DEFAULT_FILE_VIEWER_PDF_WASM_PATH = 'vendor/pdf/wasm/';
export const DEFAULT_FILE_VIEWER_PDF_STANDARD_FONT_PATH = 'vendor/pdf/standard_fonts/';
export const DEFAULT_FILE_VIEWER_PDF_CJK_FONT_FALLBACK_PATH = 'vendor/pdf/fonts/';
export const DEFAULT_FILE_VIEWER_DRAWIO_VIEWER_SCRIPT_PATH = 'vendor/drawio/viewer-static.min.js';
export const DEFAULT_FILE_VIEWER_DRAWIO_ASSET_PATH = 'vendor/drawio/';
export const DEFAULT_FILE_VIEWER_CAD_RUNTIME_VERSION = '0.8.0';
export const DEFAULT_FILE_VIEWER_CAD_WASM_PATH = `wasm/cad/${DEFAULT_FILE_VIEWER_CAD_RUNTIME_VERSION}/`;
export const DEFAULT_FILE_VIEWER_CAD_WORKER_PATH = `${DEFAULT_FILE_VIEWER_CAD_WASM_PATH}dwg-worker.js`;
export const DEFAULT_FILE_VIEWER_CAD_DWF_WASM_PATH = `${DEFAULT_FILE_VIEWER_CAD_WASM_PATH}dwfv-render.wasm`;
export const DEFAULT_FILE_VIEWER_CAD_LIBREDWG_SCRIPT_PATH = `${DEFAULT_FILE_VIEWER_CAD_WASM_PATH}libredwg-web.js`;
export const DEFAULT_FILE_VIEWER_CAD_LIBREDWG_WASM_PATH = `${DEFAULT_FILE_VIEWER_CAD_WASM_PATH}libredwg-web.wasm`;
export const DEFAULT_FILE_VIEWER_TYPST_COMPILER_WASM_URL = 'wasm/typst/typst_ts_web_compiler_bg.wasm';
export const DEFAULT_FILE_VIEWER_TYPST_RENDERER_WASM_URL = 'wasm/typst/typst_ts_renderer_bg.wasm';
export const DEFAULT_FILE_VIEWER_TYPST_FONT_ASSETS_URL = 'wasm/typst/fonts/';
// Compatibility aliases kept for older imports. They intentionally resolve to
// local viewer assets; the preview runtime must not fall back to a public CDN.
export const FALLBACK_FILE_VIEWER_TYPST_COMPILER_WASM_URL = DEFAULT_FILE_VIEWER_TYPST_COMPILER_WASM_URL;
export const FALLBACK_FILE_VIEWER_TYPST_RENDERER_WASM_URL = DEFAULT_FILE_VIEWER_TYPST_RENDERER_WASM_URL;
export const DEFAULT_FILE_VIEWER_TYPST_COMPILER_WASM_PACKAGE_PATH = '@myriaddreamin/typst-ts-web-compiler/pkg/typst_ts_web_compiler_bg.wasm';
export const DEFAULT_FILE_VIEWER_TYPST_RENDERER_WASM_PACKAGE_PATH = '@myriaddreamin/typst-ts-renderer/pkg/typst_ts_renderer_bg.wasm';
export const DEFAULT_FILE_VIEWER_DATA_SQL_WASM_URL = 'wasm/data/sql-wasm.wasm';
export const DEFAULT_FILE_VIEWER_DATA_SQL_WASM_PACKAGE_PATH = 'sql.js/dist/sql-wasm.wasm';
export const DEFAULT_FILE_VIEWER_MODEL_WORKER_URL = 'wasm/model/occt-worker.js';
export const DEFAULT_FILE_VIEWER_MODEL_RUNTIME_URL = 'wasm/model/occt-import-js.js';
export const DEFAULT_FILE_VIEWER_MODEL_WASM_URL = 'wasm/model/occt-import-js.wasm';
export const DEFAULT_FILE_VIEWER_MODEL_OCCT_LICENSE_URL = 'wasm/model/LICENSE.occt.txt';
export const DEFAULT_FILE_VIEWER_MODEL_IMPORT_LICENSE_URL = 'wasm/model/LICENSE.occt-import-js.txt';
export const DEFAULT_FILE_VIEWER_MODEL_RUNTIME_PACKAGE_PATH = 'occt-import-js/dist/occt-import-js.js';
export const DEFAULT_FILE_VIEWER_MODEL_WASM_PACKAGE_PATH = 'occt-import-js/dist/occt-import-js.wasm';
export const DEFAULT_FILE_VIEWER_MODEL_OCCT_LICENSE_PACKAGE_PATH = 'occt-import-js/dist/license.occt.txt';
export const DEFAULT_FILE_VIEWER_MODEL_IMPORT_LICENSE_PACKAGE_PATH = 'occt-import-js/dist/license.occt-import-js.txt';
const automaticFileViewerAssetBaseUrl = Symbol('automatic-file-viewer-asset-base');
let configuredFileViewerAssetBaseUrl = automaticFileViewerAssetBaseUrl;
export const normalizeFileViewerAssetBaseUrl = (baseUrl) => {
    if (!baseUrl) {
        return undefined;
    }
    const value = String(baseUrl).trim();
    if (!value) {
        return undefined;
    }
    return value.endsWith('/') ? value : `${value}/`;
};
export const setDefaultFileViewerAssetBaseUrl = (baseUrl) => {
    configuredFileViewerAssetBaseUrl = normalizeFileViewerAssetBaseUrl(baseUrl);
};
export const resetDefaultFileViewerAssetBaseUrl = () => {
    configuredFileViewerAssetBaseUrl = automaticFileViewerAssetBaseUrl;
};
const pathDepth = (pathname) => pathname.split('/').filter(Boolean).length;
const longestCommonDirectoryPath = (left, right) => {
    const leftSegments = left.split('/').filter(Boolean);
    const rightSegments = right.split('/').filter(Boolean);
    const common = [];
    const length = Math.min(leftSegments.length, Math.max(0, rightSegments.length - 1));
    for (let index = 0; index < length && leftSegments[index] === rightSegments[index]; index += 1) {
        common.push(leftSegments[index]);
    }
    return common.length ? `/${common.join('/')}/` : '/';
};
const resolveFileViewerScriptAssetBaseCandidate = (script, documentBaseUrl, index) => {
    try {
        const rawScriptUrl = script.src || script.getAttribute('src') || '';
        if (!rawScriptUrl) {
            return [];
        }
        const scriptUrl = new URL(rawScriptUrl, documentBaseUrl);
        const viteClient = scriptUrl.pathname.match(/^(.*\/)@vite\/client$/i);
        const isSourceModule = /\/(?:src|node_modules|@fs|@id|@vite)(?:\/|$)/i.test(scriptUrl.pathname);
        const documentUrl = new URL(documentBaseUrl);
        const documentPathname = documentUrl.pathname.endsWith('/')
            ? documentUrl.pathname
            : `${documentUrl.pathname}/`;
        const scriptName = scriptUrl.pathname.slice(scriptUrl.pathname.lastIndexOf('/') + 1);
        const isEntryScript = /^(?:app|index|main|runtime|umi)(?:[.-]|$)/i.test(scriptName);
        const candidates = new Map();
        const addCandidate = (basePath, quality) => {
            const url = new URL(basePath, scriptUrl).href;
            const candidateUrl = new URL(url);
            let score = quality + index / 10000;
            if (candidateUrl.origin === documentUrl.origin) {
                score += 8;
            }
            if (documentPathname.startsWith(candidateUrl.pathname)) {
                // Prefer the deepest candidate that is still an ancestor of the SPA
                // route. This avoids stripping an earlier deployment segment named
                // `assets` when the emitted suffix is actually `static/js`.
                score += 16 + pathDepth(candidateUrl.pathname) / 4;
            }
            if (script.type === 'module') {
                score += 2;
            }
            if (isEntryScript) {
                score += 2;
            }
            const previous = candidates.get(url);
            if (!previous || previous.score < score) {
                candidates.set(url, { url, score });
            }
        };
        if (viteClient) {
            addCandidate(viteClient[1], 12);
        }
        if (isSourceModule) {
            return [...candidates.values()];
        }
        const pathSegments = scriptUrl.pathname.split('/').filter(Boolean);
        const directories = pathSegments.slice(0, -1);
        const emittedDirectoryWeights = [
            { segments: ['static', 'js'], weight: 9 },
            { segments: ['assets', 'js'], weight: 9 },
            { segments: ['assets', 'chunks'], weight: 8 },
            { segments: ['assets'], weight: 6 },
            { segments: ['static'], weight: 6 },
            { segments: ['js'], weight: 4 },
        ];
        emittedDirectoryWeights.forEach(({ segments, weight }) => {
            for (let segmentIndex = 0; segmentIndex <= directories.length - segments.length; segmentIndex += 1) {
                const matches = segments.every((segment, offset) => { var _a; return ((_a = directories[segmentIndex + offset]) === null || _a === void 0 ? void 0 : _a.toLowerCase()) === segment; });
                if (!matches) {
                    continue;
                }
                const basePath = directories.slice(0, segmentIndex).length
                    ? `/${directories.slice(0, segmentIndex).join('/')}/`
                    : '/';
                addCandidate(basePath, weight + segments.length);
            }
        });
        if (isEntryScript) {
            const scriptDirectory = scriptUrl.pathname.slice(0, scriptUrl.pathname.lastIndexOf('/') + 1);
            addCandidate(scriptDirectory, 3);
        }
        if (scriptUrl.origin === documentUrl.origin) {
            addCandidate(longestCommonDirectoryPath(documentUrl.pathname, scriptUrl.pathname), 8);
        }
        return [...candidates.values()];
    }
    catch {
        return [];
    }
};
/**
 * Resolves the stable public base for runtime assets without reading
 * bundler-specific environment metadata or webpack public-path variables.
 * Explicit HTML `<base>` configuration stays authoritative; for SPA fallback
 * routes, emitted Vite/Webpack/UMI entry scripts reveal the deployment root
 * more reliably than the route-derived page URL.
 */
export const resolveFileViewerRuntimeAssetBaseUrl = (documentRef) => {
    var _a;
    const documentBaseUrl = documentRef.baseURI || documentRef.URL || 'file:///';
    if (configuredFileViewerAssetBaseUrl !== automaticFileViewerAssetBaseUrl) {
        if (!configuredFileViewerAssetBaseUrl) {
            return documentBaseUrl;
        }
        try {
            return new URL(configuredFileViewerAssetBaseUrl, documentBaseUrl).href;
        }
        catch {
            return configuredFileViewerAssetBaseUrl;
        }
    }
    if (documentRef.querySelector('base[href]')) {
        return documentBaseUrl;
    }
    const candidates = Array.from(documentRef.querySelectorAll('script[src]'))
        .flatMap((script, index) => resolveFileViewerScriptAssetBaseCandidate(script, documentBaseUrl, index))
        .sort((left, right) => right.score - left.score);
    return ((_a = candidates[0]) === null || _a === void 0 ? void 0 : _a.url) || documentBaseUrl;
};
export const getDefaultFileViewerAssetBaseUrl = (documentRef) => {
    if (configuredFileViewerAssetBaseUrl !== automaticFileViewerAssetBaseUrl) {
        return configuredFileViewerAssetBaseUrl;
    }
    // Core remains environment-neutral: callers that want automatic browser
    // inference pass their owning Document explicitly.
    return documentRef ? resolveFileViewerRuntimeAssetBaseUrl(documentRef) : undefined;
};
export const DEFAULT_FILE_VIEWER_RENDERER_ASSET_MANIFESTS = [
    {
        rendererId: 'model',
        assets: [
            {
                id: 'model-occt-worker',
                rendererId: 'model',
                kind: 'worker',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_MODEL_WORKER_URL,
                optionPath: 'model.workerUrl',
                description: 'File Viewer worker that runs STEP, IGES, and BREP tessellation off the UI thread.',
            },
            {
                id: 'model-occt-runtime',
                rendererId: 'model',
                kind: 'script',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_MODEL_RUNTIME_URL,
                packagePath: DEFAULT_FILE_VIEWER_MODEL_RUNTIME_PACKAGE_PATH,
                optionPath: 'model.runtimeUrl',
                description: 'Self-hosted occt-import-js runtime for browser-native STEP, IGES, and BREP preview.',
            },
            {
                id: 'model-occt-wasm',
                rendererId: 'model',
                kind: 'wasm',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_MODEL_WASM_URL,
                packagePath: DEFAULT_FILE_VIEWER_MODEL_WASM_PACKAGE_PATH,
                optionPath: 'model.wasmUrl',
                description: 'OpenCascade WebAssembly geometry kernel used by the model renderer.',
            },
            {
                id: 'model-occt-license',
                rendererId: 'model',
                kind: 'license',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_MODEL_OCCT_LICENSE_URL,
                packagePath: DEFAULT_FILE_VIEWER_MODEL_OCCT_LICENSE_PACKAGE_PATH,
                description: 'OpenCascade license notice distributed with the browser geometry kernel.',
            },
            {
                id: 'model-occt-import-license',
                rendererId: 'model',
                kind: 'license',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_MODEL_IMPORT_LICENSE_URL,
                packagePath: DEFAULT_FILE_VIEWER_MODEL_IMPORT_LICENSE_PACKAGE_PATH,
                description: 'occt-import-js license notice distributed with the browser geometry kernel.',
            },
        ],
    },
    {
        rendererId: 'drawing',
        assets: [
            {
                id: 'drawio-viewer-script',
                rendererId: 'drawing',
                kind: 'script',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_DRAWIO_VIEWER_SCRIPT_PATH,
                optionPath: 'drawing.viewerScriptUrl',
                description: 'Official diagrams.net viewer-static.min.js self-hosted for offline Draw.io rendering.',
            },
            {
                id: 'drawio-offline-assets',
                rendererId: 'drawing',
                kind: 'directory',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_DRAWIO_ASSET_PATH,
                description: 'Official diagrams.net styles, shapes, stencils, images, mxGraph and math assets for offline rendering.',
            },
        ],
    },
    {
        rendererId: 'pdf',
        assets: [
            {
                id: 'pdf-worker',
                rendererId: 'pdf',
                kind: 'worker',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_PDF_WORKER_PATH,
                optionPath: 'pdf.workerUrl',
                description: 'PDF.js module worker copied into viewer assets for offline PDF rendering.',
            },
            {
                id: 'pdf-cmaps',
                rendererId: 'pdf',
                kind: 'directory',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_PDF_CMAP_PATH,
                optionPath: 'pdf.cMapUrl',
                description: 'PDF.js packed CMaps used for CJK and embedded text decoding.',
            },
            {
                id: 'pdf-wasm',
                rendererId: 'pdf',
                kind: 'wasm-directory',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_PDF_WASM_PATH,
                optionPath: 'pdf.wasmUrl',
                description: 'PDF.js WebAssembly helpers for image decoding and fully self-hosted PDF rendering.',
            },
            {
                id: 'pdf-standard-fonts',
                rendererId: 'pdf',
                kind: 'directory',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_PDF_STANDARD_FONT_PATH,
                optionPath: 'pdf.standardFontDataUrl',
                description: 'PDF.js standard font data used when PDF files reference base fonts.',
            },
            {
                id: 'pdf-cjk-font-fallback',
                rendererId: 'pdf',
                kind: 'directory',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_PDF_CJK_FONT_FALLBACK_PATH,
                optionPath: 'pdf.cjkFontFallbackPath',
                description: 'Self-hosted Noto Sans SC variable font shards used for missing unembedded CJK fonts.',
            },
        ],
    },
    {
        rendererId: 'archive',
        assets: [
            {
                id: 'libarchive-worker',
                rendererId: 'archive',
                kind: 'worker',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_ARCHIVE_WORKER_PATH,
                optionPath: 'archive.workerUrl',
                description: 'libarchive.js module worker used for broad archive format parsing.',
            },
            {
                id: 'libarchive-wasm',
                rendererId: 'archive',
                kind: 'wasm',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_ARCHIVE_WASM_PATH,
                optionPath: 'archive.wasmUrl',
                description: 'libarchive.js WebAssembly module expected next to the public worker.',
            },
        ],
    },
    {
        rendererId: 'cad',
        assets: [
            {
                id: 'cad-wasm-directory',
                rendererId: 'cad',
                kind: 'wasm-directory',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_CAD_WASM_PATH,
                optionPath: 'cad.wasmPath',
                description: '@flyfish-dev/cad-viewer WebAssembly directory for DWG/DXF helpers.',
            },
            {
                id: 'cad-dwg-worker',
                rendererId: 'cad',
                kind: 'worker',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_CAD_WORKER_PATH,
                optionPath: 'cad.workerUrl',
                description: 'DWG worker entry used by @flyfish-dev/cad-viewer.',
            },
            {
                id: 'cad-dwf-wasm',
                rendererId: 'cad',
                kind: 'wasm',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_CAD_DWF_WASM_PATH,
                optionPath: 'cad.dwfWasmUrl',
                description: 'DWF/DWFx/XPS raster WebAssembly module used by @flyfish-dev/cad-viewer.',
            },
            {
                id: 'cad-libredwg-script',
                rendererId: 'cad',
                kind: 'script',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_CAD_LIBREDWG_SCRIPT_PATH,
                description: 'LibreDWG JavaScript runtime loaded by the CAD DWG worker for offline parsing.',
            },
            {
                id: 'cad-libredwg-wasm',
                rendererId: 'cad',
                kind: 'wasm',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_CAD_LIBREDWG_WASM_PATH,
                description: 'LibreDWG WebAssembly runtime loaded by the CAD DWG worker for offline parsing.',
            },
            {
                id: 'cad-legacy-dwg-worker',
                rendererId: 'cad',
                kind: 'worker',
                target: 'public',
                required: false,
                defaultPath: 'wasm/cad/dwg-worker.js',
                description: 'Unversioned compatibility alias for deployments that explicitly configured the former DWG worker path.',
            },
            {
                id: 'cad-legacy-dwf-wasm',
                rendererId: 'cad',
                kind: 'wasm',
                target: 'public',
                required: false,
                defaultPath: 'wasm/cad/dwfv-render.wasm',
                description: 'Unversioned compatibility alias for deployments that explicitly configured the former DWF runtime path.',
            },
            {
                id: 'cad-legacy-libredwg-script',
                rendererId: 'cad',
                kind: 'script',
                target: 'public',
                required: false,
                defaultPath: 'wasm/cad/libredwg-web.js',
                description: 'Unversioned compatibility alias for the former LibreDWG JavaScript runtime path.',
            },
            {
                id: 'cad-legacy-libredwg-wasm',
                rendererId: 'cad',
                kind: 'wasm',
                target: 'public',
                required: false,
                defaultPath: 'wasm/cad/libredwg-web.wasm',
                description: 'Unversioned compatibility alias for the former LibreDWG WebAssembly runtime path.',
            },
        ],
    },
    {
        rendererId: 'office-word-openxml',
        assets: [
            {
                id: 'docx-worker',
                rendererId: 'office-word-openxml',
                kind: 'worker',
                target: 'public',
                required: false,
                defaultPath: DEFAULT_FILE_VIEWER_DOCX_WORKER_PATH,
                optionPath: 'docx.workerUrl',
                description: 'Optional @file-viewer/docx worker for off-main-thread DOCX ZIP/XML parsing.',
            },
            {
                id: 'docx-worker-jszip',
                rendererId: 'office-word-openxml',
                kind: 'script',
                target: 'public',
                required: false,
                defaultPath: DEFAULT_FILE_VIEWER_DOCX_WORKER_JSZIP_PATH,
                optionPath: 'docx.workerJsZipUrl',
                description: 'JSZip runtime loaded by the @file-viewer/docx worker for fully offline DOCX parsing.',
            },
        ],
    },
    {
        rendererId: 'office-presentation-binary',
        assets: [
            {
                id: 'ppt-module',
                rendererId: 'office-presentation-binary',
                kind: 'script',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_PPT_MODULE_PATH,
                optionPath: 'presentation.pptModuleUrl',
                description: 'Official @file-viewer/ppt 0.3 browser entry for PowerPoint 97–2003 preview.',
            },
            {
                id: 'ppt-worker',
                rendererId: 'office-presentation-binary',
                kind: 'worker',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_PPT_WORKER_PATH,
                optionPath: 'presentation.pptWorkerUrl',
                description: 'Official @file-viewer/ppt 0.3 module Worker.',
            },
            {
                id: 'ppt-frame-cache',
                rendererId: 'office-presentation-binary',
                kind: 'script',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_PPT_FRAME_CACHE_PATH,
                description: 'Bounded IndexedDB frame cache imported by the binary-PPT Worker.',
            },
            {
                id: 'ppt-wasm',
                rendererId: 'office-presentation-binary',
                kind: 'wasm',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_PPT_WASM_PATH,
                optionPath: 'presentation.pptWasmUrl',
                description: 'Official @file-viewer/ppt 0.3 native WebAssembly parser and renderer.',
            },
            {
                id: 'ppt-cjk-font',
                rendererId: 'office-presentation-binary',
                kind: 'font',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_PPT_FONT_PATH,
                optionPath: 'presentation.pptFontUrl',
                description: 'CJK font pack verified and loaded by the binary-PPT runtime.',
            },
            {
                id: 'ppt-runtime-manifest',
                rendererId: 'office-presentation-binary',
                kind: 'metadata',
                target: 'public',
                required: true,
                defaultPath: `${DEFAULT_FILE_VIEWER_PPT_RUNTIME_PATH}/manifest.json`,
                description: 'Integrity and edition metadata for the official binary-PPT runtime.',
            },
            {
                id: 'ppt-runtime-package',
                rendererId: 'office-presentation-binary',
                kind: 'metadata',
                target: 'public',
                required: true,
                defaultPath: `${DEFAULT_FILE_VIEWER_PPT_RUNTIME_PATH}/package.json`,
                description: 'Package identity metadata for the official binary-PPT runtime.',
            },
            {
                id: 'ppt-runtime-license',
                rendererId: 'office-presentation-binary',
                kind: 'license',
                target: 'public',
                required: true,
                defaultPath: `${DEFAULT_FILE_VIEWER_PPT_RUNTIME_PATH}/LICENSE`,
                description: 'License shipped by the independently versioned @file-viewer/ppt package.',
            },
            {
                id: 'ppt-runtime-notice',
                rendererId: 'office-presentation-binary',
                kind: 'license',
                target: 'public',
                required: true,
                defaultPath: `${DEFAULT_FILE_VIEWER_PPT_RUNTIME_PATH}/NOTICE`,
                description: 'Notices shipped by the independently versioned @file-viewer/ppt package.',
            },
        ],
    },
    {
        rendererId: 'office-presentation',
        assets: [
            {
                id: 'pptx-worker',
                rendererId: 'office-presentation',
                kind: 'worker',
                target: 'public',
                required: false,
                defaultPath: DEFAULT_FILE_VIEWER_PRESENTATION_WORKER_PATH,
                optionPath: 'presentation.workerUrl',
                description: 'Optional @file-viewer/pptx worker for stable PPTX parsing in IIFE, CDN, and offline deployments.',
            },
        ],
    },
    {
        rendererId: 'spreadsheet-openxml',
        assets: [
            {
                id: 'spreadsheet-worker',
                rendererId: 'spreadsheet-openxml',
                kind: 'worker',
                target: 'public',
                required: false,
                defaultPath: DEFAULT_FILE_VIEWER_SPREADSHEET_WORKER_PATH,
                optionPath: 'spreadsheet.workerUrl',
                description: 'Optional Spreadsheet worker for off-main-thread styled-exceljs workbook parsing.',
            },
        ],
    },
    ...['apple-pages', 'apple-numbers', 'apple-keynote'].map(rendererId => ({
        rendererId,
        assets: [
            {
                id: `${rendererId}-iwork-worker`,
                rendererId,
                kind: 'worker',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_IWORK_WORKER_PATH,
                packagePath: DEFAULT_FILE_VIEWER_IWORK_WORKER_PACKAGE_PATH,
                optionPath: 'iwork.workerUrl',
                description: 'Module Worker for bounded ZIP, Snappy, IWA/Protobuf and iWork 09 XML/APXL parsing.',
            },
        ],
    })),
    {
        rendererId: 'office-hangul',
        assets: [
            {
                id: 'hangul-worker',
                rendererId: 'office-hangul',
                kind: 'worker',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_HANGUL_WORKER_PATH,
                packagePath: DEFAULT_FILE_VIEWER_HANGUL_WORKER_PACKAGE_PATH,
                optionPath: 'hangul.workerUrl',
                description: 'Module Worker for bounded HWP v5 CFB and HWPX ZIP/XML parsing.',
            },
        ],
    },
    {
        rendererId: 'office-wordperfect',
        assets: [
            {
                id: 'wordperfect-worker',
                rendererId: 'office-wordperfect',
                kind: 'worker',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_WORDPERFECT_WORKER_PATH,
                packagePath: DEFAULT_FILE_VIEWER_WORDPERFECT_WORKER_PACKAGE_PATH,
                optionPath: 'wordPerfect.workerUrl',
                description: 'Module Worker for bounded WordPerfect signature detection and parsing.',
            },
            {
                id: 'wordperfect-libwpd-wasm',
                rendererId: 'office-wordperfect',
                kind: 'wasm',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_WORDPERFECT_WASM_PATH,
                packagePath: DEFAULT_FILE_VIEWER_WORDPERFECT_WASM_PACKAGE_PATH,
                optionPath: 'wordPerfect.wasmUrl',
                description: 'MPL-2.0 libwpd/librevenge WebAssembly parser lazy-loaded by the WordPerfect Worker.',
            },
            {
                id: 'wordperfect-libwpd-module',
                rendererId: 'office-wordperfect',
                kind: 'script',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_WORDPERFECT_MODULE_PATH,
                packagePath: DEFAULT_FILE_VIEWER_WORDPERFECT_MODULE_PACKAGE_PATH,
                description: 'Emscripten ES module loader for the MPL-2.0 libwpd/librevenge WebAssembly runtime.',
            },
        ],
    },
    {
        rendererId: 'typst',
        assets: [
            {
                id: 'typst-compiler-wasm',
                rendererId: 'typst',
                kind: 'wasm',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_TYPST_COMPILER_WASM_URL,
                packagePath: DEFAULT_FILE_VIEWER_TYPST_COMPILER_WASM_PACKAGE_PATH,
                optionPath: 'typst.compilerWasmUrl',
                description: 'Typst compiler WebAssembly module copied to the public assets directory.',
            },
            {
                id: 'typst-renderer-wasm',
                rendererId: 'typst',
                kind: 'wasm',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_TYPST_RENDERER_WASM_URL,
                packagePath: DEFAULT_FILE_VIEWER_TYPST_RENDERER_WASM_PACKAGE_PATH,
                optionPath: 'typst.rendererWasmUrl',
                description: 'Typst SVG renderer WebAssembly module copied to the public assets directory.',
            },
            {
                id: 'typst-font-assets',
                rendererId: 'typst',
                kind: 'directory',
                target: 'public',
                required: true,
                defaultPath: DEFAULT_FILE_VIEWER_TYPST_FONT_ASSETS_URL,
                optionPath: 'typst.fontAssetsUrl',
                description: 'Self-hosted default Typst text fonts used by typst.ts without public CDN requests.',
            },
        ],
    },
    {
        rendererId: 'data-asset',
        assets: [
            {
                id: 'data-sql-wasm',
                rendererId: 'data-asset',
                kind: 'wasm',
                target: 'public',
                required: false,
                defaultPath: DEFAULT_FILE_VIEWER_DATA_SQL_WASM_URL,
                packagePath: DEFAULT_FILE_VIEWER_DATA_SQL_WASM_PACKAGE_PATH,
                optionPath: 'data.sqlWasmUrl',
                description: 'sql.js WebAssembly module copied to the public assets directory for SQLite previews.',
            },
        ],
    },
];
const DEFAULT_FILE_VIEWER_DOCUMENT_BASE_URL = 'file:///';
export const resolveFileViewerAssetUrl = (value, fallback, options = {}) => {
    const raw = value ? String(value) : fallback;
    const documentBaseUrl = options.documentBaseUrl || DEFAULT_FILE_VIEWER_DOCUMENT_BASE_URL;
    const configuredBaseUrl = !value && configuredFileViewerAssetBaseUrl !== automaticFileViewerAssetBaseUrl
        ? configuredFileViewerAssetBaseUrl
        : undefined;
    const preferredBaseUrl = options.baseUrl || configuredBaseUrl;
    const baseUrl = preferredBaseUrl
        ? preferredBaseUrl.endsWith('/') ? preferredBaseUrl : `${preferredBaseUrl}/`
        : documentBaseUrl;
    const resolvedBase = preferredBaseUrl
        ? new URL(baseUrl, documentBaseUrl).href
        : baseUrl;
    const resolved = new URL(raw, resolvedBase).href;
    return options.trimTrailingSlash ? resolved.replace(/\/+$/, '') : resolved;
};
export const resolveFileViewerArchiveWorkerUrl = (options, baseUrl) => {
    return resolveFileViewerAssetUrl(options === null || options === void 0 ? void 0 : options.workerUrl, DEFAULT_FILE_VIEWER_ARCHIVE_WORKER_PATH, { baseUrl });
};
export const resolveFileViewerArchiveWasmUrl = (options, fallback = '', documentBaseUrl) => {
    if (!(options === null || options === void 0 ? void 0 : options.wasmUrl)) {
        return fallback;
    }
    return resolveFileViewerAssetUrl(options.wasmUrl, fallback || options.wasmUrl, {
        documentBaseUrl,
    });
};
export const resolveFileViewerCadAssetUrls = (options, documentBaseUrl) => {
    const workerUrl = resolveFileViewerAssetUrl(options === null || options === void 0 ? void 0 : options.workerUrl, DEFAULT_FILE_VIEWER_CAD_WORKER_PATH, { documentBaseUrl });
    const dwfWasmUrl = resolveFileViewerAssetUrl(options === null || options === void 0 ? void 0 : options.dwfWasmUrl, DEFAULT_FILE_VIEWER_CAD_DWF_WASM_PATH, { documentBaseUrl });
    const versionDefaultRuntimeAsset = (url, overridden) => {
        if (overridden) {
            return url;
        }
        const separator = url.includes('?') ? '&' : '?';
        return `${url}${separator}file-viewer-cad=${encodeURIComponent(DEFAULT_FILE_VIEWER_CAD_RUNTIME_VERSION)}`;
    };
    return {
        wasmPath: resolveFileViewerAssetUrl(options === null || options === void 0 ? void 0 : options.wasmPath, DEFAULT_FILE_VIEWER_CAD_WASM_PATH, {
            documentBaseUrl,
            trimTrailingSlash: true,
        }),
        workerUrl: versionDefaultRuntimeAsset(workerUrl, Boolean(options === null || options === void 0 ? void 0 : options.workerUrl)),
        dwfWasmUrl: versionDefaultRuntimeAsset(dwfWasmUrl, Boolean(options === null || options === void 0 ? void 0 : options.dwfWasmUrl)),
    };
};
export const resolveFileViewerPdfAssetUrls = (options, documentBaseUrl) => {
    const rawAssetBaseUrl = (options === null || options === void 0 ? void 0 : options.assetBaseUrl) ? String(options.assetBaseUrl) : '';
    const assetBaseUrl = rawAssetBaseUrl
        ? new URL(rawAssetBaseUrl.endsWith('/') ? rawAssetBaseUrl : `${rawAssetBaseUrl}/`, documentBaseUrl || DEFAULT_FILE_VIEWER_DOCUMENT_BASE_URL).href
        : documentBaseUrl;
    return {
        workerUrl: resolveFileViewerAssetUrl(options === null || options === void 0 ? void 0 : options.workerUrl, DEFAULT_FILE_VIEWER_PDF_WORKER_PATH, {
            documentBaseUrl: assetBaseUrl,
        }),
        cMapUrl: resolveFileViewerAssetUrl(options === null || options === void 0 ? void 0 : options.cMapUrl, DEFAULT_FILE_VIEWER_PDF_CMAP_PATH, {
            documentBaseUrl: assetBaseUrl,
        }),
        wasmUrl: resolveFileViewerAssetUrl(options === null || options === void 0 ? void 0 : options.wasmUrl, DEFAULT_FILE_VIEWER_PDF_WASM_PATH, {
            documentBaseUrl: assetBaseUrl,
        }),
        standardFontDataUrl: resolveFileViewerAssetUrl(options === null || options === void 0 ? void 0 : options.standardFontDataUrl, DEFAULT_FILE_VIEWER_PDF_STANDARD_FONT_PATH, { documentBaseUrl: assetBaseUrl }),
        cjkFontFallbackPath: resolveFileViewerAssetUrl(options === null || options === void 0 ? void 0 : options.cjkFontFallbackPath, DEFAULT_FILE_VIEWER_PDF_CJK_FONT_FALLBACK_PATH, { documentBaseUrl: assetBaseUrl }),
    };
};
export const resolveFileViewerDrawioViewerScriptUrl = (options, documentBaseUrl) => {
    return resolveFileViewerAssetUrl(options === null || options === void 0 ? void 0 : options.viewerScriptUrl, DEFAULT_FILE_VIEWER_DRAWIO_VIEWER_SCRIPT_PATH, { documentBaseUrl });
};
export const resolveFileViewerDocxWorkerUrl = (options, documentBaseUrl) => {
    const resolved = resolveFileViewerAssetUrl(options === null || options === void 0 ? void 0 : options.workerUrl, DEFAULT_FILE_VIEWER_DOCX_WORKER_PATH, {
        documentBaseUrl,
    });
    if (options === null || options === void 0 ? void 0 : options.workerUrl) {
        return resolved;
    }
    const separator = resolved.includes('?') ? '&' : '?';
    return `${resolved}${separator}file-viewer-docx=${encodeURIComponent(DEFAULT_FILE_VIEWER_DOCX_RUNTIME_VERSION)}`;
};
export const resolveFileViewerDocxWorkerJsZipUrl = (options, documentBaseUrl) => {
    const resolved = resolveFileViewerAssetUrl(options === null || options === void 0 ? void 0 : options.workerJsZipUrl, DEFAULT_FILE_VIEWER_DOCX_WORKER_JSZIP_PATH, { documentBaseUrl });
    if (options === null || options === void 0 ? void 0 : options.workerJsZipUrl) {
        return resolved;
    }
    const separator = resolved.includes('?') ? '&' : '?';
    return `${resolved}${separator}file-viewer-docx=${encodeURIComponent(DEFAULT_FILE_VIEWER_DOCX_RUNTIME_VERSION)}`;
};
export const resolveFileViewerSpreadsheetWorkerUrl = (options, documentBaseUrl) => {
    return resolveFileViewerAssetUrl(options === null || options === void 0 ? void 0 : options.workerUrl, DEFAULT_FILE_VIEWER_SPREADSHEET_WORKER_PATH, {
        documentBaseUrl,
    });
};
export const resolveFileViewerIworkWorkerUrl = (options, documentBaseUrl) => resolveFileViewerAssetUrl(options === null || options === void 0 ? void 0 : options.workerUrl, DEFAULT_FILE_VIEWER_IWORK_WORKER_PATH, { documentBaseUrl });
export const resolveFileViewerHangulWorkerUrl = (options, documentBaseUrl) => resolveFileViewerAssetUrl(options === null || options === void 0 ? void 0 : options.workerUrl, DEFAULT_FILE_VIEWER_HANGUL_WORKER_PATH, { documentBaseUrl });
export const resolveFileViewerWordPerfectWorkerUrl = (options, documentBaseUrl) => resolveFileViewerAssetUrl(options === null || options === void 0 ? void 0 : options.workerUrl, DEFAULT_FILE_VIEWER_WORDPERFECT_WORKER_PATH, { documentBaseUrl });
export const resolveFileViewerWordPerfectWasmUrl = (options, documentBaseUrl) => resolveFileViewerAssetUrl(options === null || options === void 0 ? void 0 : options.wasmUrl, DEFAULT_FILE_VIEWER_WORDPERFECT_WASM_PATH, { documentBaseUrl });
export const resolveFileViewerPresentationWorkerUrl = (options, documentBaseUrl) => {
    return resolveFileViewerAssetUrl(options === null || options === void 0 ? void 0 : options.workerUrl, DEFAULT_FILE_VIEWER_PRESENTATION_WORKER_PATH, {
        documentBaseUrl,
    });
};
export const resolveFileViewerTypstCompilerWasmUrl = (options, overrides = [], documentBaseUrl) => {
    return resolveFileViewerAssetUrl((options === null || options === void 0 ? void 0 : options.compilerWasmUrl) || overrides.find(Boolean), DEFAULT_FILE_VIEWER_TYPST_COMPILER_WASM_URL, { documentBaseUrl });
};
export const resolveFileViewerTypstRendererWasmUrl = (options, overrides = [], documentBaseUrl) => {
    return resolveFileViewerAssetUrl((options === null || options === void 0 ? void 0 : options.rendererWasmUrl) || overrides.find(Boolean), DEFAULT_FILE_VIEWER_TYPST_RENDERER_WASM_URL, { documentBaseUrl });
};
export const resolveFileViewerTypstFontAssetsUrl = (options, overrides = [], documentBaseUrl) => {
    return resolveFileViewerAssetUrl((options === null || options === void 0 ? void 0 : options.fontAssetsUrl) || overrides.find(Boolean), DEFAULT_FILE_VIEWER_TYPST_FONT_ASSETS_URL, { documentBaseUrl, trimTrailingSlash: true });
};
export const resolveFileViewerDataSqlWasmUrl = (options, overrides = [], documentBaseUrl) => {
    return resolveFileViewerAssetUrl((options === null || options === void 0 ? void 0 : options.sqlWasmUrl) || overrides.find(Boolean), DEFAULT_FILE_VIEWER_DATA_SQL_WASM_URL, { documentBaseUrl });
};
export const resolveFileViewerModelAssetUrls = (options, documentBaseUrl) => ({
    workerUrl: resolveFileViewerAssetUrl(options === null || options === void 0 ? void 0 : options.workerUrl, DEFAULT_FILE_VIEWER_MODEL_WORKER_URL, { documentBaseUrl }),
    runtimeUrl: resolveFileViewerAssetUrl(options === null || options === void 0 ? void 0 : options.runtimeUrl, DEFAULT_FILE_VIEWER_MODEL_RUNTIME_URL, { documentBaseUrl }),
    wasmUrl: resolveFileViewerAssetUrl(options === null || options === void 0 ? void 0 : options.wasmUrl, DEFAULT_FILE_VIEWER_MODEL_WASM_URL, { documentBaseUrl }),
});
export const listFileViewerRendererAssetManifests = () => {
    return [...DEFAULT_FILE_VIEWER_RENDERER_ASSET_MANIFESTS];
};
export const getFileViewerRendererAssetManifest = (rendererId) => {
    return DEFAULT_FILE_VIEWER_RENDERER_ASSET_MANIFESTS.find(manifest => manifest.rendererId === rendererId) || null;
};
const getRendererAssetOptionValue = (options, optionPath) => {
    var _a, _b, _c, _d, _e, _f, _g, _h, _j, _k, _l, _m, _o, _p, _q, _r, _s, _t, _u, _v, _w, _x, _y, _z, _0, _1, _2, _3, _4, _5;
    switch (optionPath) {
        case 'archive.workerUrl':
            return (_a = options === null || options === void 0 ? void 0 : options.archive) === null || _a === void 0 ? void 0 : _a.workerUrl;
        case 'archive.wasmUrl':
            return (_b = options === null || options === void 0 ? void 0 : options.archive) === null || _b === void 0 ? void 0 : _b.wasmUrl;
        case 'cad.wasmPath':
            return (_c = options === null || options === void 0 ? void 0 : options.cad) === null || _c === void 0 ? void 0 : _c.wasmPath;
        case 'cad.workerUrl':
            return (_d = options === null || options === void 0 ? void 0 : options.cad) === null || _d === void 0 ? void 0 : _d.workerUrl;
        case 'cad.dwfWasmUrl':
            return (_e = options === null || options === void 0 ? void 0 : options.cad) === null || _e === void 0 ? void 0 : _e.dwfWasmUrl;
        case 'data.sqlWasmUrl':
            return (_f = options === null || options === void 0 ? void 0 : options.data) === null || _f === void 0 ? void 0 : _f.sqlWasmUrl;
        case 'docx.workerJsZipUrl':
            return (_g = options === null || options === void 0 ? void 0 : options.docx) === null || _g === void 0 ? void 0 : _g.workerJsZipUrl;
        case 'docx.workerUrl':
            return (_h = options === null || options === void 0 ? void 0 : options.docx) === null || _h === void 0 ? void 0 : _h.workerUrl;
        case 'drawing.viewerScriptUrl':
            return (_j = options === null || options === void 0 ? void 0 : options.drawing) === null || _j === void 0 ? void 0 : _j.viewerScriptUrl;
        case 'iwork.workerUrl':
            return (_k = options === null || options === void 0 ? void 0 : options.iwork) === null || _k === void 0 ? void 0 : _k.workerUrl;
        case 'hangul.workerUrl':
            return (_l = options === null || options === void 0 ? void 0 : options.hangul) === null || _l === void 0 ? void 0 : _l.workerUrl;
        case 'wordPerfect.workerUrl':
            return (_m = options === null || options === void 0 ? void 0 : options.wordPerfect) === null || _m === void 0 ? void 0 : _m.workerUrl;
        case 'wordPerfect.wasmUrl':
            return (_o = options === null || options === void 0 ? void 0 : options.wordPerfect) === null || _o === void 0 ? void 0 : _o.wasmUrl;
        case 'model.workerUrl':
            return (_p = options === null || options === void 0 ? void 0 : options.model) === null || _p === void 0 ? void 0 : _p.workerUrl;
        case 'model.runtimeUrl':
            return (_q = options === null || options === void 0 ? void 0 : options.model) === null || _q === void 0 ? void 0 : _q.runtimeUrl;
        case 'model.wasmUrl':
            return (_r = options === null || options === void 0 ? void 0 : options.model) === null || _r === void 0 ? void 0 : _r.wasmUrl;
        case 'pdf.workerUrl':
            return (_s = options === null || options === void 0 ? void 0 : options.pdf) === null || _s === void 0 ? void 0 : _s.workerUrl;
        case 'pdf.cMapUrl':
            return (_t = options === null || options === void 0 ? void 0 : options.pdf) === null || _t === void 0 ? void 0 : _t.cMapUrl;
        case 'pdf.wasmUrl':
            return (_u = options === null || options === void 0 ? void 0 : options.pdf) === null || _u === void 0 ? void 0 : _u.wasmUrl;
        case 'pdf.standardFontDataUrl':
            return (_v = options === null || options === void 0 ? void 0 : options.pdf) === null || _v === void 0 ? void 0 : _v.standardFontDataUrl;
        case 'pdf.cjkFontFallbackPath':
            return (_w = options === null || options === void 0 ? void 0 : options.pdf) === null || _w === void 0 ? void 0 : _w.cjkFontFallbackPath;
        case 'presentation.pptModuleUrl':
            return (_x = options === null || options === void 0 ? void 0 : options.presentation) === null || _x === void 0 ? void 0 : _x.pptModuleUrl;
        case 'presentation.pptWorkerUrl':
            return (_y = options === null || options === void 0 ? void 0 : options.presentation) === null || _y === void 0 ? void 0 : _y.pptWorkerUrl;
        case 'presentation.pptWasmUrl':
            return (_z = options === null || options === void 0 ? void 0 : options.presentation) === null || _z === void 0 ? void 0 : _z.pptWasmUrl;
        case 'presentation.pptFontUrl':
            return (_0 = options === null || options === void 0 ? void 0 : options.presentation) === null || _0 === void 0 ? void 0 : _0.pptFontUrl;
        case 'presentation.workerUrl':
            return (_1 = options === null || options === void 0 ? void 0 : options.presentation) === null || _1 === void 0 ? void 0 : _1.workerUrl;
        case 'spreadsheet.workerUrl':
            return (_2 = options === null || options === void 0 ? void 0 : options.spreadsheet) === null || _2 === void 0 ? void 0 : _2.workerUrl;
        case 'typst.compilerWasmUrl':
            return (_3 = options === null || options === void 0 ? void 0 : options.typst) === null || _3 === void 0 ? void 0 : _3.compilerWasmUrl;
        case 'typst.fontAssetsUrl':
            return (_4 = options === null || options === void 0 ? void 0 : options.typst) === null || _4 === void 0 ? void 0 : _4.fontAssetsUrl;
        case 'typst.rendererWasmUrl':
            return (_5 = options === null || options === void 0 ? void 0 : options.typst) === null || _5 === void 0 ? void 0 : _5.rendererWasmUrl;
        default:
            return undefined;
    }
};
export const resolveFileViewerRendererAssets = (rendererId, resolveOptions = {}) => {
    const manifest = getFileViewerRendererAssetManifest(rendererId);
    if (!manifest) {
        return [];
    }
    return manifest.assets.map(asset => {
        const optionValue = getRendererAssetOptionValue(resolveOptions.options, asset.optionPath);
        const configuredValue = optionValue ? String(optionValue) : undefined;
        const fallbackValue = asset.defaultUrl || asset.defaultPath;
        const resolved = {
            ...asset,
            configured: Boolean(optionValue),
        };
        if (configuredValue || fallbackValue) {
            resolved.url = resolveFileViewerAssetUrl(configuredValue, fallbackValue || configuredValue || '', {
                baseUrl: resolveOptions.baseUrl,
                documentBaseUrl: resolveOptions.documentBaseUrl,
                trimTrailingSlash: asset.kind === 'directory' ||
                    asset.kind === 'wasm-directory' ||
                    resolveOptions.trimTrailingSlash,
            });
        }
        return resolved;
    });
};
