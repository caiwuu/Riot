export class WorkerRefImpl {
    constructor(nameOrWorker, worker = null) {
        if (typeof nameOrWorker === 'string') {
            this.name = nameOrWorker;
            this.worker = worker;
            return;
        }
        this.name = '';
        this.worker = nameOrWorker;
    }
    defaults(provider) {
        if (!this.worker) {
            this.worker = provider();
        }
        return this.worker;
    }
}
export const refWorker = (name, _module = false) => {
    return new WorkerRefImpl(name, null);
};
export const createFileViewerWorkerController = (factory, options = {}) => {
    const instance = factory();
    const eventHandlers = new Map();
    const messageHooks = new Set();
    const errorHooks = new Set();
    const emit = (type, payload) => {
        instance === null || instance === void 0 ? void 0 : instance.postMessage({
            type,
            payload,
        });
    };
    const handleMessage = (event) => {
        const { type, payload } = event.data || {};
        const handlers = eventHandlers.get(type);
        handlers === null || handlers === void 0 ? void 0 : handlers.forEach(handler => handler(payload));
        messageHooks.forEach(hook => hook(event));
    };
    const handleError = (event) => {
        if (options.logErrors !== false) {
            console.error(event);
        }
        errorHooks.forEach(hook => hook(event));
    };
    instance === null || instance === void 0 ? void 0 : instance.addEventListener('message', handleMessage);
    instance === null || instance === void 0 ? void 0 : instance.addEventListener('error', handleError);
    const worker = {
        emit,
    };
    return {
        instance,
        worker,
        emit,
        onWorkerMessage(hook) {
            messageHooks.add(hook);
            return () => {
                messageHooks.delete(hook);
            };
        },
        onWorkerError(hook) {
            errorHooks.add(hook);
            return () => {
                errorHooks.delete(hook);
            };
        },
        onWorkerEvent(type, hook) {
            let handlers = eventHandlers.get(type);
            if (!handlers) {
                handlers = new Set();
                eventHandlers.set(type, handlers);
            }
            handlers.add(hook);
            return () => {
                handlers === null || handlers === void 0 ? void 0 : handlers.delete(hook);
                if ((handlers === null || handlers === void 0 ? void 0 : handlers.size) === 0) {
                    eventHandlers.delete(type);
                }
            };
        },
        mapEvents(mappings) {
            if (Array.isArray(mappings)) {
                return mappings.reduce((result, key) => {
                    result[key] = (payload) => emit(key, payload);
                    return result;
                }, {});
            }
            return Object.keys(mappings).reduce((result, key) => {
                const name = mappings[key];
                result[name] = (payload) => emit(key, payload);
                return result;
            }, {});
        },
        destroy() {
            instance === null || instance === void 0 ? void 0 : instance.removeEventListener('message', handleMessage);
            instance === null || instance === void 0 ? void 0 : instance.removeEventListener('error', handleError);
            instance === null || instance === void 0 ? void 0 : instance.terminate();
            eventHandlers.clear();
            messageHooks.clear();
            errorHooks.clear();
        },
    };
};
