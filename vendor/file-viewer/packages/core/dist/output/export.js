import { applyFileViewerPagePrintMasksToHtml, buildFileViewerPrintMaskOverlayHtml, FILE_VIEWER_PRINT_MASK_STYLE, normalizeFileViewerPrintMaskOptions, } from '../features/printMask.js';
const escapeHtmlAttribute = (value) => value
    .replace(/&/g, '&amp;')
    .replace(/"/g, '&quot;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
const EXPORT_DOCUMENT_STYLE = `
  * { box-sizing: border-box; }
  html, body { margin: 0; min-height: 100%; background: #f2f4f7; color: #172033; font-family: Aptos, "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif; print-color-adjust: exact; -webkit-print-color-adjust: exact; }
  body { padding: 24px; }
  .viewer-export-shell { position: relative; min-height: calc(100vh - 48px); overflow: visible; background: #f2f4f7; }
  .viewer-export-content { position: relative; z-index: 1; contain: none; width: 100%; min-height: 100%; overflow: visible; }
  .viewer-export-watermark { position: absolute; inset: 0; pointer-events: none; z-index: 20; background-repeat: repeat; print-color-adjust: exact; -webkit-print-color-adjust: exact; }
  .viewer-export-content .file-render,
  .viewer-export-content .file-viewer,
  .viewer-export-content .viewer-stage,
  .viewer-export-content .content,
  .viewer-export-content .pdf-shell,
  .viewer-export-content .pdf-content,
  .viewer-export-content .pdf-viewport,
  .viewer-export-content .pdf-wrapper,
  .viewer-export-content .docx-fit-viewer,
  .viewer-export-content .docx-wrapper,
  .viewer-export-content .msdoc-stage,
  .viewer-export-content .code-viewer,
  .viewer-export-content .markdown-viewer,
  .viewer-export-content .email-shell,
  .viewer-export-content .archive-shell,
  .viewer-export-content .eda-shell,
  .viewer-export-content .ebook-shell,
  .viewer-export-content .umd-shell,
  .viewer-export-content .drawing-shell,
  .viewer-export-content .audio-shell,
  .viewer-export-content .cad-shell,
  .viewer-export-content .cad-body,
  .viewer-export-content .cad-canvas-wrap,
  .viewer-export-content .dwg-preview-frame {
    position: relative !important;
    inset: auto !important;
    contain: none !important;
    width: 100% !important;
    height: auto !important;
    min-height: 0 !important;
    max-height: none !important;
    overflow: visible !important;
  }
  .viewer-export-content .docx-wrapper {
    display: block !important;
    padding: 0 !important;
    background: transparent !important;
  }
  .viewer-export-content .docx-print-document {
    display: block !important;
    width: fit-content !important;
    max-width: 100% !important;
    height: auto !important;
    overflow: visible !important;
    margin: 0 auto !important;
  }
  .viewer-export-content .docx-page-frame {
    position: relative !important;
    width: var(--viewer-print-page-width, fit-content) !important;
    height: var(--viewer-print-page-height, auto) !important;
    min-height: var(--viewer-print-page-height, 0) !important;
    max-width: 100% !important;
    margin: 0 auto 18px !important;
    overflow: hidden !important;
    break-inside: avoid;
    page-break-inside: avoid;
    break-after: page;
    page-break-after: always;
  }
  .viewer-export-content .msdoc-page {
    position: relative !important;
    width: var(--viewer-print-page-width, 794px) !important;
    min-height: var(--viewer-print-page-height, 1123px) !important;
    max-width: 100% !important;
    height: auto !important;
    margin: 0 auto 18px !important;
    overflow: visible !important;
    break-after: page;
    page-break-after: always;
  }
  .viewer-export-content .docx-page-frame:last-child,
  .viewer-export-content .msdoc-page:last-child {
    break-after: auto;
    page-break-after: auto;
  }
  .viewer-export-content .docx-page-frame > section.docx {
    position: relative !important;
    top: auto !important;
    left: auto !important;
    width: var(--viewer-print-page-width, auto) !important;
    min-height: var(--viewer-print-page-height, auto) !important;
    max-width: none !important;
    margin: 0 auto !important;
    overflow: visible !important;
    transform: none !important;
    box-shadow: none !important;
    break-inside: avoid;
    page-break-inside: avoid;
  }
  .viewer-export-content .msdoc-stage {
    display: block !important;
    padding: 0 !important;
    background: transparent !important;
  }
  .viewer-export-content .msdoc-page > .msdoc-root {
    margin: 0 auto !important;
    box-shadow: none !important;
    overflow: visible !important;
  }
  .viewer-export-content .pdf-toolbar,
  .viewer-export-content .pdf-nav-pane,
  .viewer-export-content .viewer-actions,
  .viewer-export-content .code-toolbar,
  .viewer-export-content .umd-toolbar,
  .viewer-export-content .drawing-toolbar,
  .viewer-export-content .cad-toolbar {
    display: none !important;
  }
  .viewer-export-content .pdf-content,
  .viewer-export-content .pdf-shell--nav-hidden .pdf-content,
  .viewer-export-content .cad-body.without-layers {
    display: block !important;
    grid-template-columns: none !important;
  }
  .viewer-export-content .pdfViewer { padding: 0 !important; }
  .viewer-export-content .pdfViewer .page {
    margin: 0 auto 16px !important;
    border: 0 !important;
    box-shadow: none !important;
    break-after: page;
    page-break-after: always;
  }
  .viewer-export-content .pdfViewer .page:last-child {
    break-after: auto;
    page-break-after: auto;
  }
  .viewer-export-content .pdf-export-document {
    display: grid;
    justify-items: center;
    gap: 18px;
    padding: 4px 0;
  }
  .viewer-export-content .pdf-export-page {
    width: var(--viewer-print-page-width, auto);
    height: var(--viewer-print-page-height, auto);
    max-width: 100%;
    overflow: hidden;
    background: #ffffff;
    box-shadow: 0 12px 32px rgba(15, 23, 42, 0.12);
    break-inside: avoid;
    page-break-inside: avoid;
    break-after: page;
    page-break-after: always;
  }
  .viewer-export-content .pdf-export-page:last-child {
    break-after: auto;
    page-break-after: auto;
  }
  .viewer-export-content .pdf-export-page img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: contain;
  }
  .viewer-export-content .pptx-wrapper {
    width: 100% !important;
    max-width: 100% !important;
    height: auto !important;
    overflow: visible !important;
    transform: none !important;
  }
  .viewer-export-content .pptx-wrapper .slide {
    margin: 0 auto 18px !important;
    break-inside: avoid;
    page-break-inside: avoid;
    break-after: page;
    page-break-after: always;
    box-shadow: none !important;
  }
  .viewer-export-content .pptx-wrapper .slide:last-child {
    break-after: auto;
    page-break-after: auto;
  }
  .viewer-export-content .ofd-stage {
    padding: 0 !important;
    overflow: visible !important;
  }
  .viewer-export-content .ofd-page,
  .viewer-export-content .drawing-svg,
  .viewer-export-content .cad-canvas-wrap,
  .viewer-export-content .dwg-preview-frame {
    break-inside: avoid;
    page-break-inside: avoid;
    break-after: page;
    page-break-after: always;
    box-shadow: none !important;
  }
  .viewer-export-content .ofd-page:last-child,
  .viewer-export-content .drawing-svg:last-child,
  .viewer-export-content .cad-canvas-wrap:last-child,
  .viewer-export-content .dwg-preview-frame:last-child {
    break-after: auto;
    page-break-after: auto;
  }
  .viewer-export-content .code-area {
    overflow: visible !important;
    white-space: pre-wrap !important;
    word-break: break-word !important;
  }
  .viewer-export-content .umd-body,
  .viewer-export-content .umd-stage-wrap,
  .viewer-export-content .umd-stage {
    display: block !important;
    height: auto !important;
    max-height: none !important;
    overflow: visible !important;
  }
  .viewer-export-content .umd-toc {
    display: none !important;
  }
  img, canvas, svg, video { max-width: 100%; }
  @media print {
    @page { margin: 12mm; }
    html, body { min-height: auto; background: #ffffff; }
    body { padding: 0; }
    .viewer-export-shell,
    .viewer-export-content {
      min-height: 0;
      overflow: visible;
      background: #ffffff;
    }
    .viewer-export-content .pdf-export-document {
      display: block;
      padding: 0;
    }
    .viewer-export-content .pdf-export-page {
      width: var(--viewer-print-page-width, auto) !important;
      height: var(--viewer-print-page-height, auto) !important;
      max-width: none !important;
      margin: 0;
      overflow: hidden;
      box-shadow: none;
    }
    .viewer-export-content .docx-page-frame {
      width: var(--viewer-print-page-width, auto) !important;
      height: var(--viewer-print-page-height, auto) !important;
      min-height: var(--viewer-print-page-height, 0) !important;
      max-width: none !important;
      margin: 0 !important;
      overflow: hidden !important;
    }
    .viewer-export-content .msdoc-page {
      width: var(--viewer-print-page-width, 794px) !important;
      min-height: var(--viewer-print-page-height, 1123px) !important;
      max-width: none !important;
      margin: 0 !important;
      overflow: visible !important;
    }
    .viewer-export-content .docx-page-frame > section.docx,
    .viewer-export-content .msdoc-page > .msdoc-root {
      width: var(--viewer-print-page-width, 100%) !important;
      max-width: none !important;
      border: 0 !important;
    }
    .viewer-export-content .pptx-wrapper .slide,
    .viewer-export-content .ofd-page,
    .viewer-export-content .drawing-svg,
    .viewer-export-content .cad-canvas-wrap,
    .viewer-export-content .dwg-preview-frame {
      box-shadow: none !important;
    }
  }
`;
export const collectDocumentStyles = () => {
    return Array.from(document.querySelectorAll('style, link[rel="stylesheet"]'))
        .map(node => {
        if (node instanceof HTMLStyleElement) {
            return `<style>${node.textContent || ''}</style>`;
        }
        if (node.href) {
            return `<link rel="stylesheet" href="${escapeHtmlAttribute(node.href)}" />`;
        }
        return '';
    })
        .filter(Boolean)
        .join('\n');
};
export const buildExportHtmlDocument = ({ contentHtml, includeDocumentStyles = true, printStyle = '', title, watermarkInlineStyle = '', mask = null, }) => {
    var _a, _b;
    const watermark = watermarkInlineStyle
        ? `<div class="viewer-export-watermark" style="${watermarkInlineStyle}"></div>`
        : '';
    const normalizedMask = normalizeFileViewerPrintMaskOptions(mask);
    const globalMask = normalizedMask
        ? {
            ...normalizedMask,
            regions: (_a = normalizedMask.regions) === null || _a === void 0 ? void 0 : _a.filter(region => region.pageIndex === undefined),
            stamps: (_b = normalizedMask.stamps) === null || _b === void 0 ? void 0 : _b.filter(stamp => stamp.pageIndex === undefined),
        }
        : null;
    const maskHtml = buildFileViewerPrintMaskOverlayHtml(globalMask);
    const maskedContentHtml = applyFileViewerPagePrintMasksToHtml(contentHtml, normalizedMask);
    const styles = includeDocumentStyles ? collectDocumentStyles() : '';
    const printOverrideStyle = printStyle ? `<style data-viewer-print-style>${printStyle}</style>` : '';
    const maskStyle = normalizedMask ? `<style data-viewer-print-mask-style>${FILE_VIEWER_PRINT_MASK_STYLE}</style>` : '';
    const safeTitle = escapeHtmlAttribute(title);
    return `<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width,initial-scale=1" />
  <title>${safeTitle}</title>
  ${styles}
  <style>${EXPORT_DOCUMENT_STYLE}</style>
  ${maskStyle}
</head>
<body>
  <main class="viewer-export-shell">
    <div class="viewer-export-content">${maskedContentHtml}</div>
    ${maskHtml}
    ${watermark}
  </main>
  ${printOverrideStyle}
</body>
</html>`;
};
export const triggerFileViewerBlobDownload = (blob, name) => {
    const objectUrl = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = objectUrl;
    link.download = name;
    link.rel = 'noopener';
    document.body.appendChild(link);
    link.click();
    link.remove();
    window.setTimeout(() => URL.revokeObjectURL(objectUrl), 4000);
};
export const triggerFileViewerUrlDownload = (url, name) => {
    const link = document.createElement('a');
    link.href = url;
    link.download = name;
    link.rel = 'noopener';
    link.target = '_blank';
    document.body.appendChild(link);
    link.click();
    link.remove();
};
export const replaceFileViewerCanvasWithImages = (source, clone) => {
    const sourceCanvases = Array.from(source.querySelectorAll('canvas'));
    const clonedCanvases = Array.from(clone.querySelectorAll('canvas'));
    clonedCanvases.forEach((canvas, index) => {
        const sourceCanvas = sourceCanvases[index];
        if (!sourceCanvas) {
            return;
        }
        try {
            const image = document.createElement('img');
            image.src = sourceCanvas.toDataURL('image/png');
            image.alt = 'rendered canvas';
            image.style.maxWidth = '100%';
            image.style.display = 'block';
            image.style.margin = '0 auto';
            canvas.replaceWith(image);
        }
        catch {
            // 跨域资源污染过的 canvas 无法导出，只保留原 canvas 占位。
        }
    });
};
export const waitForFileViewerNextPaint = (targetWindow) => {
    return new Promise(resolve => {
        const currentWindow = targetWindow || (typeof window !== 'undefined' ? window : undefined);
        if (!currentWindow || typeof currentWindow.requestAnimationFrame !== 'function') {
            const schedule = (currentWindow === null || currentWindow === void 0 ? void 0 : currentWindow.setTimeout)
                ? currentWindow.setTimeout.bind(currentWindow)
                : globalThis.setTimeout.bind(globalThis);
            schedule(() => resolve(), 0);
            return;
        }
        const requestAnimationFrame = currentWindow.requestAnimationFrame.bind(currentWindow);
        requestAnimationFrame(() => {
            requestAnimationFrame(() => resolve());
        });
    });
};
export const waitForFileViewerImages = async (root) => {
    if (!root || typeof root.querySelectorAll !== 'function') {
        return;
    }
    const images = Array.from(root.querySelectorAll('img'));
    await Promise.all(images.map(async (image) => {
        if (image.complete) {
            return;
        }
        if ('decode' in image) {
            try {
                await image.decode();
                return;
            }
            catch {
                // decode 失败时继续走 load/error 事件，避免单张异常图片阻塞打印。
            }
        }
        await new Promise(resolve => {
            image.addEventListener('load', () => resolve(), { once: true });
            image.addEventListener('error', () => resolve(), { once: true });
        });
    }));
};
const bytesToDataUrl = (bytes, mimeType) => {
    const type = mimeType || 'application/octet-stream';
    const nodeBuffer = globalThis.Buffer;
    if (nodeBuffer) {
        return `data:${type};base64,${nodeBuffer.from(bytes).toString('base64')}`;
    }
    let binary = '';
    const view = new Uint8Array(bytes);
    for (let index = 0; index < view.length; index += 1) {
        binary += String.fromCharCode(view[index]);
    }
    return `data:${type};base64,${btoa(binary)}`;
};
const blobToDataUrl = async (blob) => {
    if (typeof FileReader === 'function') {
        try {
            return await new Promise((resolve, reject) => {
                const reader = new FileReader();
                reader.onload = () => resolve(String(reader.result || ''));
                reader.onerror = () => reject(reader.error || new Error('Failed to read blob'));
                reader.readAsDataURL(blob);
            });
        }
        catch {
            // Fall through to ArrayBuffer encoding for Node / incomplete FileReader shims.
        }
    }
    return bytesToDataUrl(await blob.arrayBuffer(), blob.type || 'application/octet-stream');
};
const collectBlobUrls = (html) => {
    const matches = html.match(/blob:[^\s"'<>)\\]+/g) || [];
    return Array.from(new Set(matches));
};
/**
 * Rewrite ephemeral `blob:` URLs into portable `data:` URLs so exported / printed
 * HTML keeps images after leaving the live viewer document (GitHub #90).
 */
export const inlineFileViewerBlobUrlsInHtml = async (html) => {
    if (!html.includes('blob:') || typeof fetch !== 'function') {
        return html;
    }
    const urls = collectBlobUrls(html);
    if (!urls.length) {
        return html;
    }
    const replacements = await Promise.all(urls.map(async (url) => {
        try {
            const response = await fetch(url);
            if (!response.ok) {
                return null;
            }
            const blob = await response.blob();
            const dataUrl = await blobToDataUrl(blob);
            return dataUrl ? [url, dataUrl] : null;
        }
        catch {
            return null;
        }
    }));
    let next = html;
    for (const pair of replacements) {
        if (!pair) {
            continue;
        }
        const [from, to] = pair;
        next = next.split(from).join(to);
    }
    return next;
};
export const waitForFileViewerPrintWindowReady = async (printWindow) => {
    const { document: printDocument } = printWindow;
    if (printDocument.readyState !== 'complete') {
        await new Promise(resolve => {
            printWindow.addEventListener('load', () => resolve(), { once: true });
            printWindow.setTimeout(() => resolve(), 1200);
        });
    }
    await Promise.all(Array.from(printDocument.images).map(async (image) => {
        if (image.complete) {
            return;
        }
        if ('decode' in image) {
            try {
                await image.decode();
                return;
            }
            catch {
                // 图片解码失败不阻塞打印，浏览器仍会尝试按现有资源输出。
            }
        }
        await new Promise(resolve => {
            image.addEventListener('load', () => resolve(), { once: true });
            image.addEventListener('error', () => resolve(), { once: true });
            printWindow.setTimeout(() => resolve(), 1500);
        });
    }));
    await new Promise(resolve => {
        printWindow.requestAnimationFrame(() => {
            printWindow.requestAnimationFrame(() => resolve());
        });
    });
};
export const resolveFileViewerPrintStyle = async (adapter, options) => {
    if (options.mode !== 'print' || !(adapter === null || adapter === void 0 ? void 0 : adapter.printStyle)) {
        return '';
    }
    if (typeof adapter.printStyle === 'function') {
        return await adapter.printStyle(options);
    }
    return adapter.printStyle;
};
export const prepareFileViewerRenderedContentForSnapshot = async (source, adapter) => {
    var _a;
    await ((_a = adapter === null || adapter === void 0 ? void 0 : adapter.beforeSnapshot) === null || _a === void 0 ? void 0 : _a.call(adapter));
    await waitForFileViewerNextPaint();
    await waitForFileViewerImages(source);
};
export const buildFileViewerRenderedHtmlDocument = async ({ source, mode = 'export', title, adapter = null, watermarkInlineStyle = '', mask = null, }) => {
    const exportOptions = { mode, title };
    const toHtml = adapter === null || adapter === void 0 ? void 0 : adapter.toHtml;
    const normalizedMask = normalizeFileViewerPrintMaskOptions(mask);
    if (toHtml) {
        await prepareFileViewerRenderedContentForSnapshot(source, adapter);
        const contentHtml = await inlineFileViewerBlobUrlsInHtml(await toHtml(exportOptions));
        const printStyle = await resolveFileViewerPrintStyle(adapter, exportOptions);
        return buildExportHtmlDocument({
            contentHtml,
            includeDocumentStyles: adapter.includeDocumentStyles !== false,
            printStyle,
            title,
            watermarkInlineStyle,
            mask: normalizedMask,
        });
    }
    await prepareFileViewerRenderedContentForSnapshot(source, adapter);
    const clone = source.cloneNode(true);
    clone.querySelectorAll('.viewer-watermark').forEach(node => node.remove());
    replaceFileViewerCanvasWithImages(source, clone);
    const printStyle = await resolveFileViewerPrintStyle(adapter, exportOptions);
    return buildExportHtmlDocument({
        contentHtml: await inlineFileViewerBlobUrlsInHtml(clone.innerHTML),
        printStyle,
        title,
        watermarkInlineStyle,
        mask: normalizedMask,
    });
};
