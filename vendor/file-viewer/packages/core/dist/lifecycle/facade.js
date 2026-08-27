import { buildFileViewerOperationContextFromLifecycleState, createFileViewerLifecycleActions, createFileViewerLifecycleStateController, emitFileViewerComponentLifecycleEvent, resolveFileViewerBeforeOperationErrorMessage, } from './operations.js';
import { createFileViewerLoadStartState, createFileViewerRenderCompleteState, } from '../source/loading.js';
export const createFileViewerLifecycleFacade = ({ getOptions, getFilename, getBufferSize, getCurrentFile, getCurrentVersion, getFallbackFile, getFallbackUrl, emitLifecycle, emitOperationBefore, emitOperationCancel, formatErrorMessage, handleLifecycleError, handleOperationError, onOperationErrorMessage, }) => {
    const lifecycleState = createFileViewerLifecycleStateController();
    const lifecycleActions = createFileViewerLifecycleActions({
        lifecycleState,
        getOptions,
        onLifecycleChange: (_event, context) => {
            emitFileViewerComponentLifecycleEvent(emitLifecycle, context);
        },
        onLifecycleError: handleLifecycleError,
        onOperationBefore: emitOperationBefore,
        onOperationCancel: emitOperationCancel,
        onOperationError: (error, context) => {
            handleOperationError === null || handleOperationError === void 0 ? void 0 : handleOperationError(error, context);
            onOperationErrorMessage === null || onOperationErrorMessage === void 0 ? void 0 : onOperationErrorMessage(resolveFileViewerBeforeOperationErrorMessage({
                error,
                formatErrorMessage,
                i18n: getOptions(),
            }), context);
        },
    });
    const buildOperationContext = (operation) => {
        return buildFileViewerOperationContextFromLifecycleState({
            operation,
            lifecycleState,
            version: getCurrentVersion(),
            filename: getFilename(),
            bufferSize: getBufferSize(),
            currentFile: getCurrentFile(),
            fallbackFile: getFallbackFile(),
            fallbackUrl: getFallbackUrl(),
            i18n: getOptions(),
        });
    };
    return {
        markLoadStarted: lifecycleState.markLoadStarted,
        clearLoadStarted: lifecycleState.clearLoadStarted,
        notifyLifecycle: lifecycleActions.notifyLifecycle,
        notifyActiveUnloadStart: lifecycleActions.notifyActiveUnloadStart,
        notifyActiveUnloadComplete: lifecycleActions.notifyActiveUnloadComplete,
        setActiveDocumentContext: lifecycleState.setActiveDocumentContext,
        clearActiveDocumentContext: lifecycleState.clearActiveDocumentContext,
        buildOperationContext,
        buildLoadStartState({ version, source, file, sourceUrl, }) {
            return createFileViewerLoadStartState({
                version,
                source,
                file,
                sourceUrl,
                filename: getFilename(),
                bufferSize: getBufferSize(),
                i18n: getOptions(),
            });
        },
        buildRenderCompleteState({ version, source, file, sourceUrl, }) {
            return createFileViewerRenderCompleteState({
                version,
                source,
                file,
                sourceUrl,
                filename: getFilename(),
                bufferSize: getBufferSize(),
                lifecycleState,
            });
        },
        runBeforeOperation(operation) {
            return lifecycleActions.runBeforeOperation(buildOperationContext(operation));
        },
    };
};
