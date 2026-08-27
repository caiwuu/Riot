export const capturePdfJsWorkerGlobal = (workerGlobal) => ({
    hadOwnNamespace: Object.prototype.hasOwnProperty.call(workerGlobal, 'pdfjsWorker'),
    namespace: workerGlobal.pdfjsWorker,
});
/**
 * Temporarily own the fake-worker handler used by this renderer.
 *
 * Host applications can load another PDF.js build before File Viewer. Reusing
 * that global handler is unsafe because PDF.js requires its API and worker
 * versions to match exactly. The original namespace is restored after our
 * PDFWorker has captured and initialized the bundled handler, so a host PDF.js
 * build can continue using its own global afterwards.
 */
export const scopePdfJsWorkerMessageHandler = (workerGlobal, WorkerMessageHandler, restoreSnapshot = capturePdfJsWorkerGlobal(workerGlobal)) => {
    const currentNamespace = workerGlobal.pdfjsWorker;
    const installResult = (currentNamespace === null || currentNamespace === void 0 ? void 0 : currentNamespace.WorkerMessageHandler) === WorkerMessageHandler
        ? 'unchanged'
        : (currentNamespace === null || currentNamespace === void 0 ? void 0 : currentNamespace.WorkerMessageHandler)
            ? 'replaced'
            : 'installed';
    const installedNamespace = installResult === 'unchanged'
        ? currentNamespace
        : {
            ...currentNamespace,
            WorkerMessageHandler,
        };
    if (!installedNamespace) {
        return {
            installResult: 'unchanged',
            restore: () => false,
        };
    }
    if (installResult !== 'unchanged') {
        workerGlobal.pdfjsWorker = installedNamespace;
    }
    const restorationIsNoop = restoreSnapshot.hadOwnNamespace &&
        restoreSnapshot.namespace === installedNamespace;
    let restoreAttempted = false;
    return {
        installResult,
        restore: () => {
            if (restorationIsNoop || restoreAttempted) {
                return false;
            }
            restoreAttempted = true;
            if (workerGlobal.pdfjsWorker !== installedNamespace) {
                return false;
            }
            if (restoreSnapshot.hadOwnNamespace) {
                workerGlobal.pdfjsWorker = restoreSnapshot.namespace;
            }
            else {
                delete workerGlobal.pdfjsWorker;
            }
            return true;
        },
    };
};
