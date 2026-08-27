export type PdfJsWorkerNamespace = {
    WorkerMessageHandler?: unknown;
    [key: string]: unknown;
};
export type PdfJsWorkerGlobal = {
    pdfjsWorker?: PdfJsWorkerNamespace;
};
export type PdfJsWorkerHandlerInstallResult = 'installed' | 'replaced' | 'unchanged';
export type PdfJsWorkerHandlerScope = {
    installResult: PdfJsWorkerHandlerInstallResult;
    restore: () => boolean;
};
export type PdfJsWorkerGlobalSnapshot = {
    hadOwnNamespace: boolean;
    namespace: PdfJsWorkerNamespace | undefined;
};
export declare const capturePdfJsWorkerGlobal: (workerGlobal: PdfJsWorkerGlobal) => PdfJsWorkerGlobalSnapshot;
/**
 * Temporarily own the fake-worker handler used by this renderer.
 *
 * Host applications can load another PDF.js build before File Viewer. Reusing
 * that global handler is unsafe because PDF.js requires its API and worker
 * versions to match exactly. The original namespace is restored after our
 * PDFWorker has captured and initialized the bundled handler, so a host PDF.js
 * build can continue using its own global afterwards.
 */
export declare const scopePdfJsWorkerMessageHandler: (workerGlobal: PdfJsWorkerGlobal, WorkerMessageHandler: unknown, restoreSnapshot?: PdfJsWorkerGlobalSnapshot) => PdfJsWorkerHandlerScope;
