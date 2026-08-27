import type { RendererRegistry } from '../contracts/types';
export interface FileViewerRendererHandlerEntry<Handler> {
    rendererId: string;
    handler: Handler;
}
/**
 * Content-signature aware renderers can throw this shape when the filename
 * extension points at the wrong container. The orchestration layer follows the
 * redirect once, using the handler registered for the detected renderer id.
 */
export interface FileViewerRendererRedirectError extends Error {
    actualRendererId: string;
}
export declare const resolveFileViewerRendererRedirectId: (error: unknown) => string | undefined;
export interface CreateFileViewerRendererDispatcherOptions<Handler> {
    registry?: RendererRegistry;
    handlers: Iterable<FileViewerRendererHandlerEntry<Handler>>;
    fallbackHandler?: Handler;
    fallbackKey?: string;
}
export interface FileViewerRendererDispatcher<Handler> {
    handlersByRendererId: Map<string, Handler>;
    handlersByExtension: Map<string, Handler>;
    missingRendererIds: string[];
    get(extension: string): Handler | undefined;
    resolve(extension: string): Handler | undefined;
    has(extension: string): boolean;
    listExtensions(): string[];
}
export declare const createFileViewerRendererDispatcher: <Handler>({ registry, handlers, fallbackHandler, fallbackKey, }: CreateFileViewerRendererDispatcherOptions<Handler>) => FileViewerRendererDispatcher<Handler>;
