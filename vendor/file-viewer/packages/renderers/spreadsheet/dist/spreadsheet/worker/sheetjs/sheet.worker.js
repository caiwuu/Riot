import { createSpreadsheetParserContext, handleSpreadsheetWorkerRequest } from './parser.js';
const ctx = typeof self === 'undefined'
    ? null
    : self;
if (ctx) {
    const context = createSpreadsheetParserContext();
    ctx.onmessage = async (message) => {
        const responses = await handleSpreadsheetWorkerRequest(context, message.data);
        responses.forEach(response => {
            ctx.postMessage(response);
        });
    };
    ctx.onerror = (err) => {
        console.error(err);
    };
}
