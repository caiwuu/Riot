export { DEFAULT_REGISTERED_EXTENSIONS, DEFAULT_RENDERER_DEFINITIONS, DEFAULT_STABLE_SUPPORTED_EXTENSIONS, DEFAULT_SUPPORTED_EXTENSIONS, } from './formats.generated.js';
import { DEFAULT_RENDERER_DEFINITIONS } from './formats.generated.js';
const extensionsFor = (rendererId) => {
    var _a, _b;
    return Object.freeze([...((_b = (_a = DEFAULT_RENDERER_DEFINITIONS.find(definition => definition.id === rendererId)) === null || _a === void 0 ? void 0 : _a.extensions) !== null && _b !== void 0 ? _b : [])]);
};
export const ARCHIVE_EXTENSIONS = extensionsFor('archive');
export const MODEL_EXTENSIONS = extensionsFor('model');
export const TEXT_EXTENSIONS = extensionsFor('code');
export const IMAGE_EXTENSIONS = extensionsFor('image');
