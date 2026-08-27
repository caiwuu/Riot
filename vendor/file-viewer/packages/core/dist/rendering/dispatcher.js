import { DEFAULT_RENDERER_DEFINITIONS } from '../registry/formats.js';
import { createRendererRegistry } from '../registry/registry.js';
import { normalizeFileExtension } from '../source/index.js';
export const resolveFileViewerRendererRedirectId = (error) => {
    if (!error || typeof error !== 'object' || !('actualRendererId' in error)) {
        return undefined;
    }
    const rendererId = String(error.actualRendererId || '').trim();
    return rendererId || undefined;
};
export const createFileViewerRendererDispatcher = ({ registry = createRendererRegistry(DEFAULT_RENDERER_DEFINITIONS), handlers, fallbackHandler, fallbackKey = 'error', }) => {
    const handlersByRendererId = Array.from(handlers).reduce((result, entry) => {
        result.set(entry.rendererId, entry.handler);
        return result;
    }, new Map());
    const handlersByExtension = new Map();
    const missingRendererIds = [];
    registry.list().forEach(definition => {
        const handler = handlersByRendererId.get(definition.id);
        if (!handler) {
            missingRendererIds.push(definition.id);
            return;
        }
        definition.extensions.forEach(extension => {
            handlersByExtension.set(normalizeFileExtension(extension), handler);
        });
    });
    if (fallbackHandler && fallbackKey) {
        handlersByExtension.set(normalizeFileExtension(fallbackKey), fallbackHandler);
    }
    const get = (extension) => {
        return handlersByExtension.get(normalizeFileExtension(extension));
    };
    return {
        handlersByRendererId,
        handlersByExtension,
        missingRendererIds,
        get,
        resolve(extension) {
            return get(extension) || (fallbackKey ? get(fallbackKey) : undefined);
        },
        has(extension) {
            return handlersByExtension.has(normalizeFileExtension(extension));
        },
        listExtensions() {
            return Array.from(handlersByExtension.keys()).sort();
        },
    };
};
