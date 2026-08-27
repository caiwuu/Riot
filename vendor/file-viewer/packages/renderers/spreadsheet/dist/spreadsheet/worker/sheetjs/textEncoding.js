import { decodeFileViewerTextBuffer, isValidFileViewerUtf8, } from '@file-viewer/core';
const TEXT_SPREADSHEET_EXTENSIONS = new Set(['csv', 'tsv']);
const TEXT_SPREADSHEET_MIME_TYPES = new Set([
    'text/csv',
    'text/tab-separated-values',
]);
const normalizeFileType = (value) => {
    return String(value || '')
        .trim()
        .toLowerCase()
        .replace(/^\./, '')
        .split(/[?#;]/, 1)[0];
};
const getFilenameExtension = (filename) => {
    const clean = String(filename || '').trim().toLowerCase().split(/[?#]/, 1)[0];
    const slash = Math.max(clean.lastIndexOf('/'), clean.lastIndexOf('\\'));
    const dot = clean.lastIndexOf('.');
    return dot > slash ? clean.slice(dot + 1) : '';
};
export const isTextSpreadsheetSource = ({ fileType, filename, }) => {
    const normalizedType = normalizeFileType(fileType);
    if (normalizedType) {
        return TEXT_SPREADSHEET_EXTENSIONS.has(normalizedType) ||
            TEXT_SPREADSHEET_MIME_TYPES.has(normalizedType);
    }
    return TEXT_SPREADSHEET_EXTENSIONS.has(getFilenameExtension(filename));
};
export const isValidUtf8 = isValidFileViewerUtf8;
export const decodeSpreadsheetText = (data, encoding = 'auto') => decodeFileViewerTextBuffer(data, encoding);
export const prepareSpreadsheetReadInput = (data, source = {}) => {
    if (!isTextSpreadsheetSource(source)) {
        return { kind: 'binary', data };
    }
    const decoded = decodeSpreadsheetText(data, source.textEncoding);
    return {
        kind: 'text',
        data: decoded.text,
        encoding: decoded.encoding,
    };
};
