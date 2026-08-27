// Browser renderer strategy registry.
//
// Each entry lazy-loads a concrete renderer only after its format is selected.
// Keep renderer-specific dependencies inside the leaf renderer files so the
// core shell and framework wrappers stay fast on first load.
import { DEFAULT_RENDERER_DEFINITIONS } from '../registry/formats.js';
import { createFileRenderHandlerRegistry } from '../rendering/handler.js';
import { createFileViewerRendererDispatcher } from '../rendering/dispatcher.js';
import { createFileViewerUnsupportedState } from '../viewer/state.js';
const createWrapper = (el) => ({
    $el: el,
    unmount() {
        // DOM renderers clean themselves up through their own returned instance.
    },
});
export const coreBrowserRendererHandlers = [
    {
        rendererId: 'image',
        handler: async (buffer, target, type, context) => {
            const { default: renderImage } = await import('./image.js');
            return renderImage(buffer, target, type, context);
        },
    },
];
export const CORE_LITE_RENDERER_IDS = [
    'image',
];
export const coreLiteBrowserRendererHandlers = coreBrowserRendererHandlers.filter(handler => CORE_LITE_RENDERER_IDS.includes(handler.rendererId));
export const coreLiteRendererDefinitions = DEFAULT_RENDERER_DEFINITIONS.filter(definition => CORE_LITE_RENDERER_IDS.includes(definition.id));
const resolveCoreRendererDefinitions = (preset) => {
    if (preset === 'none') {
        return [];
    }
    if (preset === 'lite') {
        return coreLiteRendererDefinitions;
    }
    return DEFAULT_RENDERER_DEFINITIONS;
};
const resolveCoreRendererHandlers = (preset) => {
    if (preset === 'none') {
        return [];
    }
    if (preset === 'lite') {
        return coreLiteBrowserRendererHandlers;
    }
    return coreBrowserRendererHandlers;
};
const renderUnsupported = async (_buffer, target, type, context) => {
    const state = createFileViewerUnsupportedState(type, undefined, context === null || context === void 0 ? void 0 : context.options);
    const wrapper = document.createElement('div');
    wrapper.style.textAlign = 'center';
    wrapper.style.marginTop = '80px';
    const message = document.createElement('div');
    message.textContent = state.message;
    wrapper.appendChild(message);
    if (state.description) {
        const description = document.createElement('div');
        description.textContent = state.description;
        wrapper.appendChild(description);
    }
    target.replaceChildren(wrapper);
    return createWrapper(target);
};
export const createFileViewerCoreRendererRegistry = (options = {}) => {
    const preset = options.builtinRenderers || 'all';
    const bridge = createFileRenderHandlerRegistry({
        definitions: resolveCoreRendererDefinitions(preset),
        handlers: resolveCoreRendererHandlers(preset),
    });
    return {
        ...bridge,
        dispatcher: createFileViewerRendererDispatcher({
            registry: bridge.registry,
            handlers: coreBrowserRendererHandlers,
            fallbackHandler: renderUnsupported,
        }),
    };
};
export const fileViewerCoreRendererRegistryBridge = createFileViewerCoreRendererRegistry();
export const fileViewerCoreRendererRegistry = fileViewerCoreRendererRegistryBridge.registry;
export const fileViewerCoreRendererDispatcher = fileViewerCoreRendererRegistryBridge.dispatcher;
export const missingFileViewerCoreRendererHandlers = fileViewerCoreRendererRegistryBridge.missingRendererIds;
