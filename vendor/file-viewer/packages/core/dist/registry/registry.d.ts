import type { FileViewerRendererHandlerRegistration, FileViewerRendererPluginInput, FileViewerRendererPlugin, FileViewerRendererPresetInput, FileViewerRendererPresetName, RendererDefinition, RendererRegistry } from '../contracts/types';
export interface RegisterFileViewerAutoRendererPresetOptions {
    /**
     * Stable key used to replace an existing auto preset registration.
     */
    id?: string;
    /**
     * Package name is useful for diagnostics and gives generated integrations a
     * deterministic id even when the preset input is an array.
     */
    packageName?: string;
}
export interface FileViewerAutoRendererPresetEntry<Handler = unknown> {
    id: string;
    packageName?: string;
    input: FileViewerRendererPluginInput<Handler>;
}
export declare const createRendererRegistry: (initialDefinitions?: readonly RendererDefinition[]) => RendererRegistry;
export interface InstallFileViewerRendererPluginsOptions<Handler = unknown> {
    registry: RendererRegistry;
    plugins: Iterable<FileViewerRendererPlugin<Handler>>;
    registerHandler?: (registration: FileViewerRendererHandlerRegistration<Handler>) => void;
}
export declare const collectFileViewerRendererPlugins: <Handler = unknown>(input?: FileViewerRendererPluginInput<Handler> | null) => FileViewerRendererPlugin<Handler>[];
export declare const registerFileViewerAutoRendererPreset: <Handler = unknown>(input: FileViewerRendererPluginInput<Handler>, options?: RegisterFileViewerAutoRendererPresetOptions) => () => void;
export declare const unregisterFileViewerAutoRendererPreset: (id: string) => boolean;
export declare const clearFileViewerAutoRendererPresets: () => void;
export declare const listFileViewerAutoRendererPresets: <Handler = unknown>() => FileViewerRendererPluginInput<Handler>[];
export declare const listFileViewerAutoRendererPresetEntries: <Handler = unknown>() => {
    input: FileViewerRendererPluginInput<Handler>;
    id: string;
    packageName?: string;
}[];
export declare const findFileViewerAutoRendererPreset: <Handler = unknown>(id: FileViewerRendererPresetName | string) => FileViewerRendererPluginInput<Handler> | undefined;
export declare const getFileViewerAutoRendererPresetVersion: () => number;
export declare const hasFileViewerRendererPresetName: (input?: FileViewerRendererPresetInput | null) => boolean;
/**
 * Normalizes `options.preset` / `options.presets` into renderer plugin inputs.
 *
 * Passing a preset object is the most portable integration style because it
 * works in any bundler. String selectors intentionally only resolve presets
 * that are already registered by a side-effect import or by build tooling.
 */
export declare const resolveFileViewerRendererPresetInputs: <Handler = unknown>(input?: FileViewerRendererPresetInput<Handler> | null) => FileViewerRendererPluginInput<Handler>[];
export declare const installFileViewerRendererPlugins: <Handler = unknown>({ registry, plugins, registerHandler, }: InstallFileViewerRendererPluginsOptions<Handler>) => Promise<RendererRegistry>;
