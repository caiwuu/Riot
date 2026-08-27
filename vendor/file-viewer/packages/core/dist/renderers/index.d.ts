import type { FileViewerBuiltinRendererPreset, FileRenderHandler, FileViewerRenderedInstance } from '../contracts/types';
type CoreBrowserRendererHandler = FileRenderHandler<FileViewerRenderedInstance, HTMLDivElement>;
interface CoreBrowserRendererHandlerEntry {
    rendererId: string;
    handler: CoreBrowserRendererHandler;
}
export declare const coreBrowserRendererHandlers: readonly CoreBrowserRendererHandlerEntry[];
export declare const CORE_LITE_RENDERER_IDS: readonly ["image"];
export declare const coreLiteBrowserRendererHandlers: CoreBrowserRendererHandlerEntry[];
export declare const coreLiteRendererDefinitions: ({
    readonly id: "office-word-openxml";
    readonly label: "Word OpenXML";
    readonly category: "office";
    readonly extensions: readonly ["docx", "docm", "dotx", "dotm"];
    readonly async: true;
    readonly supportLevel: "high-fidelity";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-word";
    readonly presets: readonly ["office", "all"];
    readonly containerVersions: readonly ["OOXML Transitional", "OOXML Strict"];
    readonly knownLimits: readonly ["VBA is never executed"];
    readonly capabilities: {
        readonly download: true;
        readonly print: "adapter";
        readonly exportHtml: "adapter";
        readonly zoom: "provider";
        readonly search: true;
    };
} | {
    readonly id: "office-word-binary";
    readonly label: "Word Binary";
    readonly category: "office";
    readonly extensions: readonly ["doc", "dot"];
    readonly async: true;
    readonly supportLevel: "structured";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-word";
    readonly presets: readonly ["office", "all"];
    readonly containerVersions: readonly ["Word 97-2003 Binary"];
    readonly knownLimits: readonly ["Macros are never executed"];
    readonly capabilities: {
        readonly download: true;
        readonly print: "adapter";
        readonly exportHtml: "adapter";
        readonly zoom: "provider";
        readonly search: true;
    };
} | {
    readonly id: "office-presentation-binary";
    readonly label: "PowerPoint 97–2003";
    readonly category: "office";
    readonly extensions: readonly ["ppt", "pot"];
    readonly async: true;
    readonly supportLevel: "structured";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-presentation";
    readonly presets: readonly ["office", "all"];
    readonly containerVersions: readonly ["PowerPoint 97-2003 Binary"];
    readonly knownLimits: readonly ["Embedded engine watermark is retained", "Macros are never executed"];
    readonly capabilities: {
        readonly download: true;
        readonly print: "adapter";
        readonly exportHtml: "adapter";
        readonly zoom: "provider";
        readonly search: false;
    };
} | {
    readonly id: "office-presentation";
    readonly label: "PowerPoint OpenXML";
    readonly category: "office";
    readonly extensions: readonly ["pptx", "pptm", "potx", "potm", "ppsx", "ppsm"];
    readonly async: true;
    readonly supportLevel: "high-fidelity";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-presentation";
    readonly presets: readonly ["office", "all"];
    readonly containerVersions: readonly ["OOXML Transitional", "OOXML Strict"];
    readonly knownLimits: readonly ["Macros and external programs are never executed"];
    readonly capabilities: {
        readonly download: true;
        readonly print: true;
        readonly exportHtml: true;
        readonly zoom: "provider";
        readonly search: true;
    };
} | {
    readonly id: "open-document";
    readonly label: "Open Document";
    readonly category: "office";
    readonly extensions: readonly ["rtf", "odt", "odp"];
    readonly async: true;
    readonly supportLevel: "structured";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-word";
    readonly presets: readonly ["office", "all"];
    readonly containerVersions: readonly ["RTF", "OpenDocument"];
    readonly knownLimits: readonly [];
    readonly capabilities: {
        readonly download: true;
        readonly print: true;
        readonly exportHtml: true;
        readonly zoom: "provider";
        readonly search: true;
    };
} | {
    readonly id: "spreadsheet-openxml";
    readonly label: "Spreadsheet";
    readonly category: "office";
    readonly extensions: readonly ["xlsx", "xltx", "xlsm", "xlsb", "xls", "xlt", "xla", "xlam", "xltm", "csv", "tsv", "ods", "fods"];
    readonly async: true;
    readonly supportLevel: "structured";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-spreadsheet";
    readonly presets: readonly ["office", "all"];
    readonly containerVersions: readonly ["OOXML", "BIFF", "OpenDocument", "Delimited text"];
    readonly knownLimits: readonly ["VBA and add-in code are never executed", "Formula results are read from the saved workbook cache"];
    readonly capabilities: {
        readonly download: true;
        readonly print: false;
        readonly exportHtml: false;
        readonly zoom: "provider";
        readonly search: true;
    };
} | {
    readonly id: "apple-pages";
    readonly label: "Apple Pages";
    readonly category: "office";
    readonly extensions: readonly ["pages"];
    readonly async: true;
    readonly supportLevel: "high-fidelity";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-iwork";
    readonly presets: readonly ["office", "all"];
    readonly containerVersions: readonly ["iWork '09 index.xml(.gz)", "iWork 2013+ IWA"];
    readonly knownLimits: readonly ["Static preview only", "Encrypted iwpv2 files are detected but not decrypted"];
    readonly capabilities: {
        readonly download: true;
        readonly print: "adapter";
        readonly exportHtml: "adapter";
        readonly zoom: "provider";
        readonly search: true;
    };
} | {
    readonly id: "apple-numbers";
    readonly label: "Apple Numbers";
    readonly category: "office";
    readonly extensions: readonly ["numbers"];
    readonly async: true;
    readonly supportLevel: "high-fidelity";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-iwork";
    readonly presets: readonly ["office", "all"];
    readonly containerVersions: readonly ["iWork '09 index.xml", "iWork 2013+ IWA"];
    readonly knownLimits: readonly ["Formula results are read from the saved document cache", "Encrypted iwpv2 files are detected but not decrypted"];
    readonly capabilities: {
        readonly download: true;
        readonly print: "adapter";
        readonly exportHtml: "adapter";
        readonly zoom: "provider";
        readonly search: true;
    };
} | {
    readonly id: "apple-keynote";
    readonly label: "Apple Keynote";
    readonly category: "office";
    readonly extensions: readonly ["key"];
    readonly async: true;
    readonly supportLevel: "high-fidelity";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-iwork";
    readonly presets: readonly ["office", "all"];
    readonly containerVersions: readonly ["iWork '09 index.apxl", "iWork 2013+ IWA"];
    readonly knownLimits: readonly ["Animations, transitions and video playback are not executed", "Encrypted iwpv2 files are detected but not decrypted"];
    readonly capabilities: {
        readonly download: true;
        readonly print: "adapter";
        readonly exportHtml: "adapter";
        readonly zoom: "provider";
        readonly search: true;
    };
} | {
    readonly id: "office-wordperfect";
    readonly label: "WordPerfect";
    readonly category: "office";
    readonly extensions: readonly ["wpd", "wp", "wp5", "wp6"];
    readonly async: true;
    readonly supportLevel: "structured";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-wordperfect";
    readonly presets: readonly ["office", "all"];
    readonly containerVersions: readonly ["WordPerfect 4.2", "WordPerfect 5.x", "WordPerfect 6+"];
    readonly knownLimits: readonly ["Macros are never executed", ".wp5 and .wp6 are routing aliases for genuine WP5/WP6 payloads, not separate container formats", "Pagination and unsupported embedded objects are presented as structured static preview"];
    readonly capabilities: {
        readonly download: true;
        readonly print: "adapter";
        readonly exportHtml: "adapter";
        readonly zoom: "provider";
        readonly search: true;
    };
} | {
    readonly id: "spreadsheet-dbf";
    readonly label: "dBASE Table";
    readonly category: "office";
    readonly extensions: readonly ["dbf"];
    readonly async: true;
    readonly supportLevel: "structured";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-spreadsheet";
    readonly presets: readonly ["office", "all"];
    readonly containerVersions: readonly ["dBASE III/IV", "Visual FoxPro"];
    readonly knownLimits: readonly ["Missing DBT/FPT memo sidecars are reported as incomplete"];
    readonly capabilities: {
        readonly download: true;
        readonly print: false;
        readonly exportHtml: false;
        readonly zoom: "provider";
        readonly search: true;
    };
} | {
    readonly id: "pdf";
    readonly label: "PDF";
    readonly category: "document";
    readonly extensions: readonly ["pdf"];
    readonly async: true;
    readonly supportLevel: "high-fidelity";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-pdf";
    readonly presets: readonly ["office", "all"];
    readonly containerVersions: readonly ["PDF"];
    readonly knownLimits: readonly [];
    readonly capabilities: {
        readonly download: true;
        readonly print: "adapter";
        readonly exportHtml: "adapter";
        readonly zoom: "provider";
        readonly search: "provider";
    };
} | {
    readonly id: "ofd";
    readonly label: "OFD";
    readonly category: "document";
    readonly extensions: readonly ["ofd"];
    readonly async: true;
    readonly supportLevel: "structured";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-ofd";
    readonly presets: readonly ["office", "all"];
    readonly containerVersions: readonly ["OFD"];
    readonly knownLimits: readonly [];
    readonly capabilities: {
        readonly download: true;
        readonly print: true;
        readonly exportHtml: true;
        readonly zoom: "provider";
        readonly search: true;
    };
} | {
    readonly id: "office-hangul";
    readonly label: "Hancom Hangul";
    readonly category: "office";
    readonly extensions: readonly ["hwp", "hwpx"];
    readonly async: true;
    readonly supportLevel: "structured";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-hangul";
    readonly presets: readonly ["office", "all"];
    readonly containerVersions: readonly ["HWP v5 CFB", "HWPX ZIP/XML"];
    readonly knownLimits: readonly ["Encrypted, DRM-protected and distribution documents are detected but not decrypted", "Charts, OLE objects and advanced drawing effects remain limited in the static structured preview", "HWP v5 pagination is approximated when the binary producer omits usable page geometry"];
    readonly capabilities: {
        readonly download: true;
        readonly print: "adapter";
        readonly exportHtml: "adapter";
        readonly zoom: "provider";
        readonly search: true;
    };
} | {
    readonly id: "typst";
    readonly label: "Typst";
    readonly category: "document";
    readonly extensions: readonly ["typ", "typst"];
    readonly async: true;
    readonly supportLevel: "high-fidelity";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-typst";
    readonly presets: readonly ["all"];
    readonly containerVersions: readonly ["Typst source"];
    readonly knownLimits: readonly [];
    readonly capabilities: {
        readonly download: true;
        readonly print: "adapter";
        readonly exportHtml: "adapter";
        readonly zoom: "provider";
        readonly search: true;
    };
} | {
    readonly id: "archive";
    readonly label: "Archive";
    readonly category: "archive";
    readonly extensions: readonly ["zip", "zipx", "7z", "rar", "tar", "gz", "gzip", "tgz", "bz2", "bzip2", "tbz", "tbz2", "xz", "txz", "lzma", "zst", "tzst", "cab", "ar", "cpio", "iso", "xar", "lha", "lzh", "jar", "war", "ear", "apk", "cbz", "cbr"];
    readonly async: true;
    readonly supportLevel: "structured";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-archive";
    readonly presets: readonly ["all"];
    readonly containerVersions: readonly ["libarchive-supported containers"];
    readonly knownLimits: readonly [];
    readonly capabilities: {
        readonly download: true;
        readonly print: false;
        readonly exportHtml: false;
        readonly zoom: false;
        readonly search: true;
    };
} | {
    readonly id: "email";
    readonly label: "Email";
    readonly category: "email";
    readonly extensions: readonly ["eml", "msg", "mbox"];
    readonly async: true;
    readonly supportLevel: "structured";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-email";
    readonly presets: readonly ["all"];
    readonly containerVersions: readonly ["MIME", "Outlook MSG", "mbox"];
    readonly knownLimits: readonly [];
    readonly capabilities: {
        readonly download: true;
        readonly print: false;
        readonly exportHtml: true;
        readonly zoom: false;
        readonly search: true;
    };
} | {
    readonly id: "eda";
    readonly label: "EDA";
    readonly category: "eda";
    readonly extensions: readonly ["olb", "dra", "gds", "oas", "oasis"];
    readonly async: true;
    readonly supportLevel: "structured";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-eda";
    readonly presets: readonly ["engineering", "all"];
    readonly containerVersions: readonly ["OrCAD", "GDSII", "OASIS"];
    readonly knownLimits: readonly [];
    readonly capabilities: {
        readonly download: true;
        readonly print: true;
        readonly exportHtml: true;
        readonly zoom: false;
        readonly search: true;
    };
} | {
    readonly id: "cad";
    readonly label: "CAD";
    readonly category: "cad";
    readonly extensions: readonly ["dxf", "dwg", "dwf", "dwfx", "xps"];
    readonly async: true;
    readonly supportLevel: "high-fidelity";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-cad";
    readonly presets: readonly ["engineering", "all"];
    readonly containerVersions: readonly ["DWG", "DXF", "DWF", "DWFX/XPS"];
    readonly knownLimits: readonly [];
    readonly capabilities: {
        readonly download: true;
        readonly print: true;
        readonly exportHtml: true;
        readonly zoom: "provider";
        readonly search: false;
    };
} | {
    readonly id: "model";
    readonly label: "3D Model";
    readonly category: "model";
    readonly extensions: readonly ["glb", "gltf", "obj", "stl", "ply", "fbx", "dae", "3ds", "3mf", "amf", "usd", "usda", "usdc", "usdz", "kmz", "step", "stp", "iges", "igs", "ifc", "3dm", "brep", "pcd", "wrl", "vrml", "xyz", "vtk", "vtp"];
    readonly async: true;
    readonly supportLevel: "structured";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-3d";
    readonly presets: readonly ["engineering", "all"];
    readonly containerVersions: readonly ["Mesh", "CAD exchange", "scene"];
    readonly knownLimits: readonly [];
    readonly capabilities: {
        readonly download: true;
        readonly print: false;
        readonly exportHtml: false;
        readonly zoom: "provider";
        readonly search: false;
    };
} | {
    readonly id: "geo";
    readonly label: "Geospatial";
    readonly category: "geo";
    readonly extensions: readonly ["geojson", "kml", "gpx", "shp"];
    readonly async: true;
    readonly supportLevel: "structured";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-geo";
    readonly presets: readonly ["engineering", "all"];
    readonly containerVersions: readonly ["GeoJSON", "KML", "GPX", "Shapefile"];
    readonly knownLimits: readonly [];
    readonly capabilities: {
        readonly download: true;
        readonly print: true;
        readonly exportHtml: true;
        readonly zoom: "provider";
        readonly search: true;
    };
} | {
    readonly id: "drawing";
    readonly label: "Drawing";
    readonly category: "drawing";
    readonly extensions: readonly ["excalidraw", "drawio", "dio", "mermaid", "mmd", "plantuml", "puml"];
    readonly async: true;
    readonly supportLevel: "structured";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-drawing";
    readonly presets: readonly ["all"];
    readonly containerVersions: readonly ["Draw.io", "Excalidraw", "Mermaid", "PlantUML"];
    readonly knownLimits: readonly [];
    readonly capabilities: {
        readonly download: true;
        readonly print: true;
        readonly exportHtml: true;
        readonly zoom: "provider";
        readonly search: true;
    };
} | {
    readonly id: "mindmap";
    readonly label: "Mind Map";
    readonly category: "mindmap";
    readonly extensions: readonly ["xmind"];
    readonly async: true;
    readonly supportLevel: "structured";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-mindmap";
    readonly presets: readonly ["all"];
    readonly containerVersions: readonly ["XMind"];
    readonly knownLimits: readonly [];
    readonly capabilities: {
        readonly download: true;
        readonly print: true;
        readonly exportHtml: true;
        readonly zoom: "provider";
        readonly search: true;
    };
} | {
    readonly id: "epub";
    readonly label: "EPUB";
    readonly category: "ebook";
    readonly extensions: readonly ["epub"];
    readonly async: true;
    readonly supportLevel: "high-fidelity";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-epub";
    readonly presets: readonly ["all"];
    readonly containerVersions: readonly ["EPUB 2", "EPUB 3"];
    readonly knownLimits: readonly [];
    readonly capabilities: {
        readonly download: true;
        readonly print: false;
        readonly exportHtml: true;
        readonly zoom: false;
        readonly search: "provider";
    };
} | {
    readonly id: "ebook-fb2";
    readonly label: "FictionBook";
    readonly category: "ebook";
    readonly extensions: readonly ["fb2"];
    readonly async: true;
    readonly supportLevel: "structured";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-epub";
    readonly presets: readonly ["all"];
    readonly containerVersions: readonly ["FictionBook 2 XML"];
    readonly knownLimits: readonly ["External network resources are not loaded"];
    readonly capabilities: {
        readonly download: true;
        readonly print: true;
        readonly exportHtml: true;
        readonly zoom: false;
        readonly search: true;
    };
} | {
    readonly id: "umd";
    readonly label: "UMD";
    readonly category: "ebook";
    readonly extensions: readonly ["umd"];
    readonly async: true;
    readonly supportLevel: "structured";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-epub";
    readonly presets: readonly ["all"];
    readonly containerVersions: readonly ["UMD"];
    readonly knownLimits: readonly [];
    readonly capabilities: {
        readonly download: true;
        readonly print: true;
        readonly exportHtml: true;
        readonly zoom: "provider";
        readonly search: true;
    };
} | {
    readonly id: "image";
    readonly label: "Image";
    readonly category: "image";
    readonly extensions: readonly ["gif", "jpg", "jpeg", "bmp", "tiff", "tif", "png", "svg", "webp", "avif", "ico", "heic", "heif", "jxl"];
    readonly async: true;
    readonly supportLevel: "high-fidelity";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-image";
    readonly presets: readonly ["lite", "all"];
    readonly containerVersions: readonly ["Raster", "SVG"];
    readonly knownLimits: readonly [];
    readonly capabilities: {
        readonly download: true;
        readonly print: true;
        readonly exportHtml: true;
        readonly zoom: "provider";
        readonly search: false;
    };
} | {
    readonly id: "markdown";
    readonly label: "Markdown";
    readonly category: "markdown";
    readonly extensions: readonly ["md", "markdown"];
    readonly async: true;
    readonly supportLevel: "structured";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-text";
    readonly presets: readonly ["lite", "all"];
    readonly containerVersions: readonly ["CommonMark/GFM"];
    readonly knownLimits: readonly [];
    readonly capabilities: {
        readonly download: true;
        readonly print: true;
        readonly exportHtml: true;
        readonly zoom: false;
        readonly search: true;
    };
} | {
    readonly id: "code";
    readonly label: "Code and Text";
    readonly category: "code";
    readonly extensions: readonly ["txt", "json", "js", "mjs", "cjs", "css", "java", "py", "html", "htm", "jsx", "ts", "tsx", "xml", "log", "vue", "yaml", "yml", "ini", "sh", "bash", "sql", "go", "rs", "php", "c", "cpp", "cc", "h", "hpp", "cs", "diff", "patch", "bundle", "bdl", "jsonc", "json5", "ipynb", "toml", "proto", "hcl", "tex", "gv", "http", "react", "rb", "swift", "kt"];
    readonly async: true;
    readonly supportLevel: "structured";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-text";
    readonly presets: readonly ["lite", "all"];
    readonly containerVersions: readonly ["Plain text", "source code", "Git bundle"];
    readonly knownLimits: readonly [];
    readonly capabilities: {
        readonly download: true;
        readonly print: true;
        readonly exportHtml: true;
        readonly zoom: false;
        readonly search: true;
    };
} | {
    readonly id: "video";
    readonly label: "Video";
    readonly category: "media";
    readonly extensions: readonly ["mp4", "webm", "m3u8"];
    readonly async: true;
    readonly supportLevel: "high-fidelity";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-media";
    readonly presets: readonly ["lite", "all"];
    readonly containerVersions: readonly ["Browser media"];
    readonly knownLimits: readonly ["Codec support follows the browser"];
    readonly capabilities: {
        readonly download: true;
        readonly print: false;
        readonly exportHtml: false;
        readonly zoom: false;
        readonly search: false;
    };
} | {
    readonly id: "audio";
    readonly label: "Audio";
    readonly category: "media";
    readonly extensions: readonly ["mp3", "mpeg", "wav", "ogg", "oga", "opus", "m4a", "aac", "flac", "weba", "midi", "mid"];
    readonly async: true;
    readonly supportLevel: "high-fidelity";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-media";
    readonly presets: readonly ["lite", "all"];
    readonly containerVersions: readonly ["Browser media", "MIDI"];
    readonly knownLimits: readonly ["Codec support follows the browser"];
    readonly capabilities: {
        readonly download: true;
        readonly print: false;
        readonly exportHtml: false;
        readonly zoom: false;
        readonly search: false;
    };
} | {
    readonly id: "data-asset";
    readonly label: "Data Asset";
    readonly category: "asset";
    readonly extensions: readonly ["ttf", "otf", "woff", "woff2", "psd", "ai", "eps", "sqlite", "wasm", "parquet", "avro", "webarchive"];
    readonly async: true;
    readonly supportLevel: "structured";
    readonly status: "stable";
    readonly packageName: "@file-viewer/renderer-data";
    readonly presets: readonly ["all"];
    readonly containerVersions: readonly ["Font", "design", "database", "binary data"];
    readonly knownLimits: readonly [];
    readonly capabilities: {
        readonly download: true;
        readonly print: false;
        readonly exportHtml: true;
        readonly zoom: false;
        readonly search: true;
    };
})[];
export interface CreateFileViewerCoreRendererRegistryOptions {
    builtinRenderers?: FileViewerBuiltinRendererPreset;
}
export declare const createFileViewerCoreRendererRegistry: (options?: CreateFileViewerCoreRendererRegistryOptions) => {
    dispatcher: import("..").FileViewerRendererDispatcher<CoreBrowserRendererHandler>;
    registry: import("..").RendererRegistry;
    missingRendererIds: string[];
};
export declare const fileViewerCoreRendererRegistryBridge: {
    dispatcher: import("..").FileViewerRendererDispatcher<CoreBrowserRendererHandler>;
    registry: import("..").RendererRegistry;
    missingRendererIds: string[];
};
export declare const fileViewerCoreRendererRegistry: import("..").RendererRegistry;
export declare const fileViewerCoreRendererDispatcher: import("..").FileViewerRendererDispatcher<CoreBrowserRendererHandler>;
export declare const missingFileViewerCoreRendererHandlers: string[];
export {};
