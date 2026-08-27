import JSZip from 'jszip';
import { createFileViewerTranslator, } from '@file-viewer/core';
const openDocumentStyle = `
.odf-viewer{min-height:100%;padding:28px;overflow:auto;background:var(--file-viewer-render-surface-background,#dfe5eb);box-sizing:border-box}
.odf-shell{width:min(100%,980px);margin:0 auto}
.odf-page{min-height:360px;margin:0 auto 18px;padding:42px 48px;border-radius:4px;background:#fff;box-shadow:0 16px 38px rgba(15,23,42,.12);color:#1f2937;box-sizing:border-box}
.odf-page p{margin:0 0 12px;font-size:15px;line-height:1.85;white-space:pre-wrap}
.flyfish-rtf-viewer{min-height:100%;padding:28px;overflow:auto;background:var(--file-viewer-render-surface-background,#dfe5eb);color:#1f2937;box-sizing:border-box}
.flyfish-rtf-paper{width:min(100%,900px);min-height:980px;margin:0 auto;padding:54px 62px;background:#fff;box-shadow:0 16px 38px rgba(15,23,42,.12);line-height:1.75;box-sizing:border-box}.flyfish-rtf-paper p{margin:0 0 12px}
[data-viewer-theme='dark'] .odf-viewer,[data-viewer-theme='dark'] .flyfish-rtf-viewer{color-scheme:dark;background:var(--file-viewer-render-surface-background,#0d1117);color:#e6edf3}
[data-viewer-theme='dark'] .odf-page,[data-viewer-theme='dark'] .flyfish-rtf-paper{border:1px solid rgba(139,148,158,.24);background:#161b22;color:#e6edf3;box-shadow:0 18px 44px rgba(0,0,0,.34)}
@media (prefers-color-scheme:dark){[data-viewer-theme='system'] .odf-viewer,[data-viewer-theme='system'] .flyfish-rtf-viewer{color-scheme:dark;background:var(--file-viewer-render-surface-background,#0d1117);color:#e6edf3}[data-viewer-theme='system'] .odf-page,[data-viewer-theme='system'] .flyfish-rtf-paper{border:1px solid rgba(139,148,158,.24);background:#161b22;color:#e6edf3;box-shadow:0 18px 44px rgba(0,0,0,.34)}}
@media (max-width:720px){.odf-viewer,.flyfish-rtf-viewer{padding:14px}.odf-page{padding:28px 24px}.flyfish-rtf-paper{padding:36px 28px}}
`;
const createStyle = () => {
    const style = document.createElement('style');
    style.textContent = openDocumentStyle;
    return style;
};
const appendText = (parent, tag, text, className) => {
    const element = document.createElement(tag);
    if (className) {
        element.className = className;
    }
    element.textContent = text;
    parent.appendChild(element);
    return element;
};
const nodeText = (node) => {
    return (node.textContent || '').replace(/\s+/g, ' ').trim();
};
const parseOdf = async (buffer, type, t) => {
    var _a;
    const zip = await JSZip.loadAsync(buffer);
    const content = await ((_a = zip.file('content.xml')) === null || _a === void 0 ? void 0 : _a.async('text'));
    if (!content) {
        throw new Error(t('word.error.missingOdfContent'));
    }
    const doc = new DOMParser().parseFromString(content, 'application/xml');
    const parseError = doc.querySelector('parsererror');
    if (parseError) {
        throw new Error(t('word.error.odfXmlParseFailed'));
    }
    if (type === 'odp') {
        const slides = Array.from(doc.getElementsByTagName('draw:page'));
        return slides.map(slide => {
            const blocks = Array.from(slide.getElementsByTagName('text:p'))
                .map(nodeText)
                .filter(Boolean);
            return {
                blocks,
            };
        });
    }
    const blocks = [
        ...Array.from(doc.getElementsByTagName('text:h')).map(nodeText),
        ...Array.from(doc.getElementsByTagName('text:p')).map(nodeText),
    ].filter(Boolean);
    return [{ blocks }];
};
const renderOdfPages = (pages) => {
    const root = document.createElement('div');
    root.className = 'odf-viewer';
    const shell = document.createElement('section');
    shell.className = 'odf-shell';
    pages.forEach(page => {
        const article = document.createElement('article');
        article.className = 'odf-page';
        page.blocks.forEach(block => appendText(article, 'p', block));
        shell.appendChild(article);
    });
    root.appendChild(shell);
    return root;
};
const resolveRtfJs = async () => {
    const rtfModule = await import('rtf.js/dist/RTFJS.bundle.js');
    return rtfModule.RTFJS || rtfModule.default || rtfModule;
};
const renderRtf = async (buffer, target, context) => {
    var _a, _b;
    const RTFJS = await resolveRtfJs();
    (_a = RTFJS.loggingEnabled) === null || _a === void 0 ? void 0 : _a.call(RTFJS, false);
    const doc = new RTFJS.Document(buffer, {});
    const elements = await doc.render();
    const stage = document.createElement('div');
    stage.className = 'flyfish-rtf-viewer';
    const paper = document.createElement('article');
    paper.className = 'flyfish-rtf-paper';
    elements.forEach((element) => paper.appendChild(element));
    stage.appendChild(paper);
    target.replaceChildren(createStyle(), stage);
    (_b = context === null || context === void 0 ? void 0 : context.registerThumbnailAdapter) === null || _b === void 0 ? void 0 : _b.call(context, { getTarget: () => paper });
    return {
        $el: target,
        unmount() {
            var _a;
            (_a = context === null || context === void 0 ? void 0 : context.registerThumbnailAdapter) === null || _a === void 0 ? void 0 : _a.call(context, null);
            target.replaceChildren();
        },
    };
};
export default async function renderOpenDocument(buffer, target, type, context) {
    var _a;
    const t = createFileViewerTranslator(context === null || context === void 0 ? void 0 : context.options);
    const normalizedType = (type || 'odt').toLowerCase();
    if (normalizedType === 'rtf') {
        return renderRtf(buffer, target, context);
    }
    const pages = await parseOdf(buffer, normalizedType, t);
    target.replaceChildren(createStyle(), renderOdfPages(pages));
    (_a = context === null || context === void 0 ? void 0 : context.registerThumbnailAdapter) === null || _a === void 0 ? void 0 : _a.call(context, {
        getTarget: () => target.querySelector('.odf-page') || target
    });
    return {
        $el: target,
        unmount() {
            var _a;
            (_a = context === null || context === void 0 ? void 0 : context.registerThumbnailAdapter) === null || _a === void 0 ? void 0 : _a.call(context, null);
            target.replaceChildren();
        },
    };
}
