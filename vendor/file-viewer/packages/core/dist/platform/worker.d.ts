export type WorkerProvider = () => Worker;
export type FileViewerWorkerFactory = () => Worker | undefined;
export type FileViewerWorkerEventHandler = (payload: any) => void;
export type FileViewerWorkerMessageHook = (event: MessageEvent) => void;
export type FileViewerWorkerErrorHook = (event: ErrorEvent) => void;
export interface FileViewerWorkerContext {
    emit(type: string, payload: any): void;
}
export interface CreateFileViewerWorkerControllerOptions {
    logErrors?: boolean;
}
export interface FileViewerWorkerController {
    readonly instance: Worker | undefined;
    readonly worker: FileViewerWorkerContext;
    emit(type: string, payload: any): void;
    onWorkerMessage(hook: FileViewerWorkerMessageHook): () => void;
    onWorkerError(hook: FileViewerWorkerErrorHook): () => void;
    onWorkerEvent(type: string, hook: FileViewerWorkerEventHandler): () => void;
    mapEvents(mappings: Array<string> | Record<string, string>): Record<string, (payload: any) => void>;
    destroy(): void;
}
export interface WorkerRef {
    name: string;
    worker: Worker | null;
    defaults(provider: WorkerProvider): Worker;
}
export declare class WorkerRefImpl implements WorkerRef {
    readonly name: string;
    worker: Worker | null;
    constructor(nameOrWorker: string | Worker | null, worker?: Worker | null);
    defaults(provider: WorkerProvider): Worker;
}
export declare const refWorker: (name: string, _module?: boolean) => WorkerRef;
export declare const createFileViewerWorkerController: (factory: FileViewerWorkerFactory, options?: CreateFileViewerWorkerControllerOptions) => FileViewerWorkerController;
