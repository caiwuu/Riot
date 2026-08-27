import { DEFAULT_RENDERER_DEFINITIONS, } from '@file-viewer/core';
const wordRendererIds = [
    'office-word-openxml',
    'office-word-binary',
    'open-document',
];
const wordDefinitions = DEFAULT_RENDERER_DEFINITIONS.filter(definition => wordRendererIds.includes(definition.id));
if (wordDefinitions.length !== wordRendererIds.length) {
    throw new Error('@file-viewer/renderer-word could not locate the shared Word renderer definitions.');
}
export const wordRendererDefinitions = wordDefinitions;
export const renderFileViewerWordDocx = (buffer, target, _type, context) => import('./wordDocx.js').then(({ default: renderWordDocx }) => renderWordDocx(buffer, target, context));
/**
 * A surprising number of legacy systems keep the `.doc` suffix after saving
 * an OOXML document. Route those ZIP-based files through the DOCX engine while
 * leaving genuine OLE/CFB `.doc` files on the binary parser.
 */
export const resolveFileViewerWordContainer = (buffer) => {
    if (buffer.byteLength < 2) {
        return 'binary';
    }
    const bytes = new Uint8Array(buffer, 0, 2);
    return bytes[0] === 0x50 && bytes[1] === 0x4b ? 'openxml' : 'binary';
};
export const renderFileViewerWordDoc = (buffer, target, _type, context) => resolveFileViewerWordContainer(buffer) === 'openxml'
    ? import('./wordDocx.js').then(({ default: renderWordDocx }) => renderWordDocx(buffer, target, context))
    : import('./wordDoc.js').then(({ default: renderWordDoc }) => renderWordDoc(buffer, target, context));
export const renderFileViewerOpenDocument = (buffer, target, type, context) => import('./openDocument.js').then(({ default: renderOpenDocument }) => renderOpenDocument(buffer, target, type, context));
export const wordRenderer = {
    id: 'file-viewer-renderer-word',
    label: 'Flyfish File Viewer Word renderer',
    definitions: wordRendererDefinitions,
    handlers: [
        {
            rendererId: 'office-word-openxml',
            handler: renderFileViewerWordDocx,
        },
        {
            rendererId: 'office-word-binary',
            handler: renderFileViewerWordDoc,
        },
        {
            rendererId: 'open-document',
            handler: renderFileViewerOpenDocument,
        },
    ],
};
export default wordRenderer;
