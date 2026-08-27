import { DEFAULT_RENDERER_DEFINITIONS } from './formats.js';
import { normalizeFileExtension } from '../source/index.js';
const autoRendererBucketKey = '__flyfish_file_viewer_auto_renderer_presets__';
const getAutoRendererBucket = () => {
    const host = globalThis;
    if (!host[autoRendererBucketKey]) {
        host[autoRendererBucketKey] = {
            version: 0,
            presets: new Map(),
        };
    }
    return host[autoRendererBucketKey];
};
const normalizeDefinition = (definition) => ({
    ...definition,
    extensions: definition.extensions.map(normalizeFileExtension),
});
export const createRendererRegistry = (initialDefinitions = DEFAULT_RENDERER_DEFINITIONS) => {
    const byId = new Map();
    const byExtension = new Map();
    const register = (definition) => {
        const normalized = normalizeDefinition(definition);
        const existing = byId.get(normalized.id);
        if (existing) {
            existing.extensions.forEach(extension => {
                var _a;
                if (((_a = byExtension.get(extension)) === null || _a === void 0 ? void 0 : _a.id) === existing.id) {
                    byExtension.delete(extension);
                }
            });
        }
        byId.set(normalized.id, normalized);
        normalized.extensions.forEach(extension => {
            const owner = byExtension.get(extension);
            if (owner && owner.id !== normalized.id) {
                throw new Error(`File extension "${extension}" is already registered by renderer "${owner.id}".`);
            }
            byExtension.set(extension, normalized);
        });
    };
    initialDefinitions.forEach(register);
    return {
        register,
        unregister(id) {
            const existing = byId.get(id);
            if (!existing) {
                return false;
            }
            existing.extensions.forEach(extension => {
                var _a;
                if (((_a = byExtension.get(extension)) === null || _a === void 0 ? void 0 : _a.id) === id) {
                    byExtension.delete(extension);
                }
            });
            byId.delete(id);
            return true;
        },
        getById(id) {
            return byId.get(id);
        },
        getByExtension(extension) {
            return byExtension.get(normalizeFileExtension(extension));
        },
        hasExtension(extension) {
            return byExtension.has(normalizeFileExtension(extension));
        },
        list() {
            return Array.from(byId.values());
        },
        listExtensions() {
            return Array.from(byExtension.keys()).sort();
        },
    };
};
const isRendererPreset = (input) => {
    return !!input && typeof input === 'object' && !Array.isArray(input) &&
        Array.isArray(input.renderers);
};
export const collectFileViewerRendererPlugins = (input) => {
    if (!input) {
        return [];
    }
    if (Array.isArray(input)) {
        return input.flatMap(item => collectFileViewerRendererPlugins(item));
    }
    if (isRendererPreset(input)) {
        return collectFileViewerRendererPlugins(input.renderers);
    }
    return [input];
};
const resolveAutoRendererPresetId = (input, options = {}) => {
    if (options.id) {
        return options.id;
    }
    if (options.packageName) {
        return options.packageName;
    }
    if (isRendererPreset(input)) {
        return input.id;
    }
    if (!Array.isArray(input) && input && typeof input === 'object' && 'id' in input) {
        return String(input.id);
    }
    return 'file-viewer-auto-renderers';
};
export const registerFileViewerAutoRendererPreset = (input, options = {}) => {
    const bucket = getAutoRendererBucket();
    const id = resolveAutoRendererPresetId(input, options);
    const existing = bucket.presets.get(id);
    if ((existing === null || existing === void 0 ? void 0 : existing.input) !== input || existing.packageName !== options.packageName) {
        bucket.presets.set(id, {
            id,
            packageName: options.packageName,
            input: input,
        });
        bucket.version += 1;
    }
    return () => {
        unregisterFileViewerAutoRendererPreset(id);
    };
};
export const unregisterFileViewerAutoRendererPreset = (id) => {
    const bucket = getAutoRendererBucket();
    const removed = bucket.presets.delete(id);
    if (removed) {
        bucket.version += 1;
    }
    return removed;
};
export const clearFileViewerAutoRendererPresets = () => {
    const bucket = getAutoRendererBucket();
    if (!bucket.presets.size) {
        return;
    }
    bucket.presets.clear();
    bucket.version += 1;
};
export const listFileViewerAutoRendererPresets = () => Array.from(getAutoRendererBucket().presets.values()).map(entry => entry.input);
export const listFileViewerAutoRendererPresetEntries = () => Array.from(getAutoRendererBucket().presets.values()).map(entry => ({
    ...entry,
    input: entry.input,
}));
export const findFileViewerAutoRendererPreset = (id) => {
    var _a;
    const bucket = getAutoRendererBucket();
    const direct = bucket.presets.get(id);
    if (direct) {
        return direct.input;
    }
    const packageSuffix = id.startsWith('@file-viewer/preset-')
        ? id
        : `@file-viewer/preset-${id}`;
    return (_a = Array.from(bucket.presets.values()).find(entry => entry.packageName === packageSuffix ||
        entry.id === packageSuffix)) === null || _a === void 0 ? void 0 : _a.input;
};
export const getFileViewerAutoRendererPresetVersion = () => getAutoRendererBucket().version;
export const hasFileViewerRendererPresetName = (input) => {
    if (!input) {
        return false;
    }
    if (typeof input === 'string') {
        return true;
    }
    if (Array.isArray(input)) {
        return input.some(item => hasFileViewerRendererPresetName(item));
    }
    return false;
};
/**
 * Normalizes `options.preset` / `options.presets` into renderer plugin inputs.
 *
 * Passing a preset object is the most portable integration style because it
 * works in any bundler. String selectors intentionally only resolve presets
 * that are already registered by a side-effect import or by build tooling.
 */
export const resolveFileViewerRendererPresetInputs = (input) => {
    if (!input) {
        return [];
    }
    if (typeof input === 'string') {
        const preset = findFileViewerAutoRendererPreset(input);
        return preset ? [preset] : [];
    }
    if (Array.isArray(input)) {
        return input.flatMap(item => resolveFileViewerRendererPresetInputs(item));
    }
    return [input];
};
export const installFileViewerRendererPlugins = async ({ registry, plugins, registerHandler, }) => {
    var _a, _b, _c;
    for (const plugin of plugins) {
        (_a = plugin.definitions) === null || _a === void 0 ? void 0 : _a.forEach(definition => {
            registry.register(definition);
        });
        (_b = plugin.handlers) === null || _b === void 0 ? void 0 : _b.forEach(registration => {
            registerHandler === null || registerHandler === void 0 ? void 0 : registerHandler(registration);
        });
        await ((_c = plugin.install) === null || _c === void 0 ? void 0 : _c.call(plugin, { registry, registerHandler }));
    }
    return registry;
};
