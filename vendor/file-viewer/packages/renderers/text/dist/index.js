import { DEFAULT_RENDERER_DEFINITIONS, } from '@file-viewer/core';
const textRendererIds = ['code', 'markdown'];
const textDefinitions = DEFAULT_RENDERER_DEFINITIONS.filter(definition => textRendererIds.includes(definition.id));
if (textDefinitions.length !== textRendererIds.length) {
    throw new Error('@file-viewer/renderer-text could not locate the shared code/markdown format definitions.');
}
export const textRendererDefinitions = textDefinitions;
export const renderFileViewerCode = (buffer, target, type, context) => import('./code.js').then(({ default: renderCode }) => renderCode(buffer, target, type, context));
export const renderFileViewerMarkdown = (buffer, target, type, context) => import('./largeText.js').then(async ({ default: renderLargeText, shouldVirtualizeMarkdownBuffer }) => {
    if (shouldVirtualizeMarkdownBuffer(buffer, context)) {
        return renderLargeText(buffer, target, type || 'md', context);
    }
    const { default: renderMarkdown } = await import('./markdown.js');
    return renderMarkdown(buffer, target, context);
});
export const textRenderer = {
    id: 'file-viewer-renderer-text',
    label: 'Flyfish File Viewer text renderer',
    definitions: textRendererDefinitions,
    handlers: [
        {
            rendererId: 'code',
            handler: renderFileViewerCode,
        },
        {
            rendererId: 'markdown',
            handler: renderFileViewerMarkdown,
        },
    ],
};
export default textRenderer;
