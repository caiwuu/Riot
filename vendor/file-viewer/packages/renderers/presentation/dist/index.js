import { DEFAULT_RENDERER_DEFINITIONS, } from '@file-viewer/core';
const binaryPresentationDefinition = DEFAULT_RENDERER_DEFINITIONS.find(definition => definition.id === 'office-presentation-binary');
const presentationDefinition = DEFAULT_RENDERER_DEFINITIONS.find(definition => definition.id === 'office-presentation');
if (!binaryPresentationDefinition || !presentationDefinition) {
    throw new Error('@file-viewer/renderer-presentation could not locate the core presentation renderer definitions.');
}
export const binaryPresentationRendererDefinition = binaryPresentationDefinition;
export const presentationRendererDefinition = presentationDefinition;
export const renderFileViewerBinaryPresentation = (buffer, target, type, context) => import('./ppt.js').then(({ default: renderPpt }) => renderPpt(buffer, target, type, context));
export const renderFileViewerPresentation = (buffer, target, type, context) => import('./pptx.js').then(({ default: renderPptx }) => renderPptx(buffer, target, type, context));
export const presentationRenderer = {
    id: 'file-viewer-renderer-presentation',
    label: 'Flyfish File Viewer presentation renderer',
    definitions: [binaryPresentationRendererDefinition, presentationRendererDefinition],
    handlers: [
        {
            rendererId: binaryPresentationRendererDefinition.id,
            handler: renderFileViewerBinaryPresentation,
        },
        {
            rendererId: presentationRendererDefinition.id,
            handler: renderFileViewerPresentation,
        },
    ],
};
export default presentationRenderer;
