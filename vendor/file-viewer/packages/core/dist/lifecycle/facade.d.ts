import { type FileViewerActiveUnloadState, type FileViewerLifecycleActions, type FileViewerLifecycleComponentEmit } from './operations';
import { type FileViewerLoadStartState, type FileViewerRenderCompleteState } from '../source/loading';
import type { FileViewerErrorMessageFormatter } from '../viewer/state';
import type { FileViewerFileRef, FileViewerLifecycleContext, FileViewerOperationContext, FileViewerOperationType, FileViewerOptions } from '../contracts/types';
export interface BuildFileViewerLifecycleFacadeLoadStartStateInput {
    version: number;
    source: FileViewerLifecycleContext['source'];
    file?: File | null;
    sourceUrl?: string | null;
}
export interface BuildFileViewerLifecycleFacadeRenderCompleteStateInput {
    version: number;
    source: FileViewerLifecycleContext['source'];
    file?: File | null;
    sourceUrl?: string | null;
}
export interface CreateFileViewerLifecycleFacadeInput {
    getOptions: () => FileViewerOptions | undefined;
    getFilename: () => string;
    getBufferSize: () => number | undefined;
    getCurrentFile: () => File | null;
    getCurrentVersion: () => number;
    getFallbackFile: () => FileViewerFileRef | null | undefined;
    getFallbackUrl: () => string | null | undefined;
    emitLifecycle: FileViewerLifecycleComponentEmit;
    emitOperationBefore: (context: FileViewerOperationContext) => void;
    emitOperationCancel: (context: FileViewerOperationContext) => void;
    formatErrorMessage: FileViewerErrorMessageFormatter;
    handleLifecycleError: (error: unknown, context: FileViewerLifecycleContext) => void;
    handleOperationError?: (error: unknown, context: FileViewerOperationContext) => void;
    onOperationErrorMessage?: (message: string, context: FileViewerOperationContext) => void;
}
export interface FileViewerLifecycleFacade {
    markLoadStarted(version: number, timestamp?: number): void;
    clearLoadStarted(version: number): void;
    notifyLifecycle: FileViewerLifecycleActions['notifyLifecycle'];
    notifyActiveUnloadStart(reason?: FileViewerLifecycleContext['reason']): FileViewerLifecycleContext | null;
    notifyActiveUnloadComplete(context: FileViewerLifecycleContext | null, reason?: FileViewerLifecycleContext['reason']): FileViewerActiveUnloadState;
    setActiveDocumentContext(context: FileViewerLifecycleContext): void;
    clearActiveDocumentContext(): void;
    buildOperationContext(operation: FileViewerOperationType): FileViewerOperationContext;
    buildLoadStartState(input: BuildFileViewerLifecycleFacadeLoadStartStateInput): FileViewerLoadStartState;
    buildRenderCompleteState(input: BuildFileViewerLifecycleFacadeRenderCompleteStateInput): FileViewerRenderCompleteState;
    runBeforeOperation(operation: FileViewerOperationType): Promise<boolean>;
}
export declare const createFileViewerLifecycleFacade: ({ getOptions, getFilename, getBufferSize, getCurrentFile, getCurrentVersion, getFallbackFile, getFallbackUrl, emitLifecycle, emitOperationBefore, emitOperationCancel, formatErrorMessage, handleLifecycleError, handleOperationError, onOperationErrorMessage, }: CreateFileViewerLifecycleFacadeInput) => FileViewerLifecycleFacade;
