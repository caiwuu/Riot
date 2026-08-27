import type { Plugin } from 'vite';
export type FileViewerVitePreset = 'all' | 'lite' | 'office' | 'engineering';
export type FileViewerVitePresetMode = FileViewerVitePreset | 'auto';
export type FileViewerMissingRendererMode = 'error' | 'warn' | 'ignore';
export type FileViewerChunkStrategy = 'renderer' | 'none';
export interface FileViewerRendererScanOptions {
    /**
     * Disable a shared scan object without branching user config.
     */
    enabled?: boolean;
    /**
     * Source roots, relative to Vite root, that should be inspected for format hints.
     * Defaults to common application source folders.
     */
    roots?: readonly string[];
    /**
     * Text-like source extensions to inspect. Values may include or omit the dot.
     */
    extensions?: readonly string[];
    /**
     * Large generated files are ignored by default to keep config/startup fast.
     */
    maxFileSize?: number;
}
export interface FileViewerCopyAssetsOptions {
    /**
     * Directory used by Vite dev. Defaults to config.publicDir.
     */
    publicDir?: string;
    /**
     * Directory used after production build. Defaults to build.outDir.
     */
    outDir?: string;
    /**
     * Directory below publicDir/outDir where assets are published. When omitted,
     * projects with an installed @file-viewer/*-full package use `file-viewer`;
     * other projects keep the existing output-root layout. Use an empty string
     * to explicitly publish at the output root. The injected runtime module
     * keeps every renderer's default asset base aligned with this directory.
     */
    baseDir?: string;
    /**
     * Copy during dev server startup, build closeBundle, or both.
     */
    mode?: 'dev' | 'build' | 'both';
}
export interface FileViewerRenderersPluginOptions {
    /**
     * File extensions or renderer ids. Examples: pdf, .dwg, typst, zip, xmind.
     */
    formats?: readonly string[];
    /**
     * Explicit renderer ids. Useful when several extensions share one renderer.
     */
    renderers?: readonly string[];
    /**
     * Presets import their dedicated @file-viewer/preset-* package. Use `auto`
     * to discover installed preset packages, and add `formats` / `renderers`
     * when you need to extend a preset with extra lines.
     */
    preset?: FileViewerVitePresetMode;
    /**
     * Auto-discovers installed `@file-viewer/preset-*` packages and registers
     * them globally for framework components. Defaults to true only when no
     * explicit preset/formats/renderers are configured, or when `preset: 'auto'`.
     */
    autoPresets?: boolean | readonly FileViewerVitePreset[];
    /**
     * Injects the virtual renderer module into Vite HTML entrypoints so
     * framework components can consume auto-registered renderers without
     * application code importing `virtual:file-viewer-renderers`.
     */
    inject?: boolean;
    /**
     * Virtual module id consumed by application code.
     */
    moduleId?: string;
    /**
     * Controls how planned-but-not-yet-extracted renderer lines are reported.
     */
    missingRenderer?: FileViewerMissingRendererMode;
    /**
     * Adds renderer-oriented manualChunks when the user did not define one.
     */
    chunkStrategy?: FileViewerChunkStrategy;
    /**
     * Wraps existing manualChunks functions with known circular-dependency-safe
     * vendor groups. This keeps CodeMirror/Lezer/Sandpack in one chunk when host
     * apps split node_modules by package name, avoiding production TDZ errors.
     */
    stabilizeInteropChunks?: boolean;
    /**
     * Copies known worker/WASM/vendor assets for selected renderer lines.
     */
    copyAssets?: boolean | FileViewerCopyAssetsOptions;
    /**
     * Opt-in source scan. The plugin reads lightweight hints such as
     * `fileViewerFormats = ['pdf', 'docx']`, `data-file-viewer-formats="pdf,docx"`,
     * and upload `accept=".pdf,.docx"` declarations, then merges them with
     * `formats` / `renderers` before generating the virtual module.
     */
    scan?: boolean | FileViewerRendererScanOptions;
}
interface MissingRendererNotice {
    format: string;
    targetPackage?: string;
    note: string;
}
export declare function extractFileViewerRendererHintTokens(source: string): string[];
export declare function collectFileViewerRendererScanTokens(projectRoot: string, scan: FileViewerRenderersPluginOptions['scan']): string[];
export interface FileViewerCopyAssetsTargetConfig {
    /** Vite project root. Defaults to process.cwd(). */
    projectRoot?: string;
    /** Resolved Vite publicDir. Defaults to `public`. */
    publicDir?: string;
    /** Resolved Vite build.outDir. Defaults to `dist`. */
    outDir?: string;
}
export interface FileViewerCopyAssetsTarget {
    mode: 'dev' | 'build';
    outputRoot: string;
    targetRoot: string;
    baseDir: string;
    installedFullPackages: string[];
}
/**
 * Resolves the exact directory used by copyAssets without running a Vite build.
 * This is exported so integrations can validate their deployment contract.
 */
export declare function resolveFileViewerCopyAssetsTarget(mode: 'dev' | 'build', value?: true | FileViewerCopyAssetsOptions, config?: FileViewerCopyAssetsTargetConfig): FileViewerCopyAssetsTarget;
export declare function fileViewerRenderers(options?: FileViewerRenderersPluginOptions): Plugin;
export declare function createFileViewerManualChunks(options?: FileViewerRenderersPluginOptions): (id: string) => string | undefined;
export declare function resolveFileViewerRendererSelection(options?: FileViewerRenderersPluginOptions, projectRoot?: string): {
    preset: FileViewerVitePreset | null;
    presetPackage: string | null;
    autoPresets: FileViewerVitePreset[];
    autoPresetPackages: string[];
    formats: string[];
    packages: string[];
    rendererIds: string[];
    renderers: {
        id: string;
        packageName: string;
        formats: string[];
        rendererIds: string[];
        chunkName: string;
    }[];
    missing: MissingRendererNotice[];
};
export default fileViewerRenderers;
