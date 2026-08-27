function toArrayBuffer(input) {
    if (input instanceof ArrayBuffer)
        return input.slice(0);
    if (ArrayBuffer.isView(input)) {
        const bytes = new Uint8Array(input.byteLength);
        bytes.set(new Uint8Array(input.buffer, input.byteOffset, input.byteLength));
        return bytes.buffer;
    }
    throw new TypeError('Expected ArrayBuffer or TypedArray');
}
/**
 * Thin typed wrapper around a module worker so applications can offload
 * expensive binary parsing without having to manage message correlation.
 */
export class MsDocWorkerClient {
    worker;
    pending = new Map();
    seq = 0;
    constructor(worker) {
        this.worker = worker;
        this.handleMessage = this.handleMessage.bind(this);
        this.handleError = this.handleError.bind(this);
        worker.addEventListener('message', this.handleMessage);
        worker.addEventListener('error', this.handleError);
    }
    static create(url = new URL('./worker.js', import.meta.url), workerOptions = {}) {
        return new MsDocWorkerClient(new Worker(url, { type: 'module', ...workerOptions }));
    }
    handleMessage(event) {
        const data = (event.data ?? {});
        const id = typeof data.id === 'number' ? data.id : -1;
        if (!this.pending.has(id))
            return;
        const current = this.pending.get(id);
        this.pending.delete(id);
        if (data.ok)
            current.resolve(data.result);
        else
            current.reject(new Error(data.error || 'Worker request failed'));
    }
    handleError(event) {
        for (const { reject } of this.pending.values()) {
            reject(event.error || new Error(event.message || 'Worker crashed'));
        }
        this.pending.clear();
    }
    request(type, payload, transfer = []) {
        const id = ++this.seq;
        return new Promise((resolve, reject) => {
            this.pending.set(id, { resolve: resolve, reject });
            this.worker.postMessage({ id, type, ...payload }, transfer);
        });
    }
    parse(input, options = {}) {
        const buffer = toArrayBuffer(input);
        return this.request('parse', { buffer, options }, [buffer]);
    }
    parseToHtml(input, options = {}) {
        const buffer = toArrayBuffer(input);
        return this.request('parseToHtml', { buffer, options }, [buffer]);
    }
    render(parsed, options = {}) {
        return this.request('render', { parsed, options });
    }
    destroy() {
        this.worker.removeEventListener('message', this.handleMessage);
        this.worker.removeEventListener('error', this.handleError);
        this.worker.terminate();
        this.pending.clear();
    }
}
