const PDF_JS_SEMVER_PATTERN = '(\\d+\\.\\d+\\.\\d+(?:-[0-9A-Za-z.-]+)?)';
const PDF_JS_WORKER_VERSION_PATTERNS = [
    new RegExp(`\\bpdfjsVersion\\s*=\\s*["']?${PDF_JS_SEMVER_PATTERN}`, 'i'),
    new RegExp(`\\bworkerVersion\\s*=\\s*["']${PDF_JS_SEMVER_PATTERN}["']`, 'i'),
    new RegExp(`\\bPDF\\.js\\s+v${PDF_JS_SEMVER_PATTERN}`, 'i'),
];
/**
 * Read the version marker emitted by official PDF.js worker bundles.
 *
 * The marker is intentionally read from a small source prefix before a Worker
 * is constructed. PDF.js does not reject an API/worker mismatch until the
 * first document request, which is too late for `PDFWorker.promise` to be a
 * useful compatibility probe.
 */
export const readPdfJsWorkerVersion = (sourcePrefix) => {
    for (const pattern of PDF_JS_WORKER_VERSION_PATTERNS) {
        const match = pattern.exec(sourcePrefix);
        if (match === null || match === void 0 ? void 0 : match[1]) {
            return match[1];
        }
    }
    return null;
};
