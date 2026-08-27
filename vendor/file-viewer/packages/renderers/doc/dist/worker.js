import { parseMsDoc } from './msdoc/parser.js';
import { renderMsDoc } from './render/html.js';
const workerScope = typeof self === 'undefined'
    ? null
    : self;
if (workerScope) {
    workerScope.addEventListener('message', (event) => {
        const message = event.data ?? {};
        const id = typeof message.id === 'number' ? message.id : -1;
        const type = message.type;
        try {
            let result;
            if (type === 'parse') {
                const buffer = message.buffer;
                const options = message.options;
                result = parseMsDoc(buffer, options ?? {});
            }
            else if (type === 'render') {
                const parsed = message.parsed;
                const options = message.options;
                result = renderMsDoc(parsed, options ?? {});
            }
            else if (type === 'parseToHtml') {
                const buffer = message.buffer;
                const options = message.options;
                const parseOptions = options?.parseOptions ?? {};
                const renderOptions = options?.renderOptions ?? {};
                result = renderMsDoc(parseMsDoc(buffer, parseOptions), renderOptions);
            }
            else {
                throw new Error(`Unsupported worker request type: ${String(type)}`);
            }
            workerScope.postMessage({ id, ok: true, result });
        }
        catch (error) {
            const errorMessage = error instanceof Error ? error.message : String(error);
            workerScope.postMessage({ id, ok: false, error: errorMessage });
        }
    });
}
