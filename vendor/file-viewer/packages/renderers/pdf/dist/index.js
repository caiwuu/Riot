import { DEFAULT_RENDERER_DEFINITIONS, } from '@file-viewer/core';
const pdfDefinition = DEFAULT_RENDERER_DEFINITIONS.find(definition => definition.id === 'pdf');
if (!pdfDefinition) {
    throw new Error('@file-viewer/renderer-pdf could not locate the core PDF renderer definition.');
}
export const pdfRendererDefinition = pdfDefinition;
export const renderFileViewerPdf = (buffer, target, _type, context) => import('./pdf.js').then(({ default: renderPdf }) => renderPdf(buffer, target, context));
export const pdfRenderer = {
    id: 'file-viewer-renderer-pdf',
    label: 'Flyfish File Viewer PDF renderer',
    definitions: [pdfRendererDefinition],
    handlers: [{
            rendererId: pdfRendererDefinition.id,
            handler: renderFileViewerPdf,
        }],
};
export default pdfRenderer;
