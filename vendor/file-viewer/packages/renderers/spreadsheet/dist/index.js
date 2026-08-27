import { DEFAULT_RENDERER_DEFINITIONS, } from '@file-viewer/core';
const spreadsheetDefinition = DEFAULT_RENDERER_DEFINITIONS.find(definition => definition.id === 'spreadsheet-openxml');
const dbfDefinition = DEFAULT_RENDERER_DEFINITIONS.find(definition => definition.id === 'spreadsheet-dbf');
if (!spreadsheetDefinition || !dbfDefinition) {
    throw new Error('@file-viewer/renderer-spreadsheet could not locate the shared Spreadsheet format definition.');
}
export const spreadsheetRendererDefinition = spreadsheetDefinition;
export const dbfRendererDefinition = dbfDefinition;
export const renderFileViewerSpreadsheet = (buffer, target, type, context) => import('./spreadsheet.js').then(({ default: renderSpreadsheet }) => renderSpreadsheet(buffer, target, type, context));
export const spreadsheetRenderer = {
    id: 'file-viewer-renderer-spreadsheet',
    label: 'Flyfish File Viewer Spreadsheet renderer',
    definitions: [spreadsheetRendererDefinition, dbfRendererDefinition],
    handlers: [spreadsheetRendererDefinition, dbfRendererDefinition].map(definition => ({
        rendererId: definition.id,
        handler: renderFileViewerSpreadsheet,
    })),
};
export default spreadsheetRenderer;
