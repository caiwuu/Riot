/**
 * Read the version marker emitted by official PDF.js worker bundles.
 *
 * The marker is intentionally read from a small source prefix before a Worker
 * is constructed. PDF.js does not reject an API/worker mismatch until the
 * first document request, which is too late for `PDFWorker.promise` to be a
 * useful compatibility probe.
 */
export declare const readPdfJsWorkerVersion: (sourcePrefix: string) => string | null;
