const clampPercent = (value) => Math.max(0, Math.min(100, value));
export const normalizeFileViewerPrintMaskRegion = (region) => {
    if (!region) {
        return null;
    }
    const pageIndex = region.pageIndex;
    if (pageIndex !== undefined &&
        (!Number.isInteger(pageIndex) || pageIndex < 0)) {
        return null;
    }
    const left = clampPercent(Number(region.left) || 0);
    const top = clampPercent(Number(region.top) || 0);
    const width = clampPercent(Number(region.width) || 0);
    const height = clampPercent(Number(region.height) || 0);
    if (width <= 0 || height <= 0) {
        return null;
    }
    return {
        left,
        top,
        width: Math.min(width, 100 - left),
        height: Math.min(height, 100 - top),
        ...(pageIndex === undefined ? {} : { pageIndex }),
    };
};
const normalizeStampSource = (value) => {
    if (typeof value !== 'string') {
        return '';
    }
    const source = value.trim();
    if (/^data:image\/(?:png|jpe?g|webp|gif|svg\+xml);/i.test(source) ||
        /^(?:blob:|https?:\/\/|\/|\.\.?\/)/i.test(source)) {
        return source;
    }
    return '';
};
const clampOpacity = (value) => {
    const opacity = Number(value);
    return Number.isFinite(opacity) ? Math.max(0, Math.min(1, opacity)) : 1;
};
const normalizeRotation = (value) => {
    const rotation = Number(value);
    if (!Number.isFinite(rotation)) {
        return 0;
    }
    return ((rotation % 360) + 360) % 360;
};
export const normalizeFileViewerPrintStamp = (stamp) => {
    const region = normalizeFileViewerPrintMaskRegion(stamp);
    const src = normalizeStampSource(stamp === null || stamp === void 0 ? void 0 : stamp.src);
    if (!region || !src) {
        return null;
    }
    return {
        ...region,
        src,
        opacity: clampOpacity(stamp === null || stamp === void 0 ? void 0 : stamp.opacity),
        rotate: normalizeRotation(stamp === null || stamp === void 0 ? void 0 : stamp.rotate),
        ...(typeof (stamp === null || stamp === void 0 ? void 0 : stamp.alt) === 'string' && stamp.alt.trim()
            ? { alt: stamp.alt.trim() }
            : {}),
    };
};
export const normalizeFileViewerPrintMaskOptions = (mask) => {
    if (!mask) {
        return null;
    }
    const regions = (mask.regions || [])
        .map(region => normalizeFileViewerPrintMaskRegion(region))
        .filter((region) => !!region);
    const stamps = (mask.stamps || [])
        .map(stamp => normalizeFileViewerPrintStamp(stamp))
        .filter((stamp) => !!stamp);
    if (!regions.length && !stamps.length) {
        return null;
    }
    return {
        ...(regions.length ? { regions } : {}),
        ...(stamps.length ? { stamps } : {}),
        color: mask.color || '#000000',
    };
};
const escapeHtmlAttribute = (value) => value
    .replace(/&/g, '&amp;')
    .replace(/"/g, '&quot;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
export const buildFileViewerPrintMaskOverlayHtml = (mask, pageScoped = false) => {
    const normalized = normalizeFileViewerPrintMaskOptions(mask);
    if (!normalized) {
        return '';
    }
    const color = normalized.color || '#000000';
    const blocks = (normalized.regions || []).map(region => (`<div class="viewer-export-print-mask-block" style="left:${region.left}%;top:${region.top}%;width:${region.width}%;height:${region.height}%;background:${color};"></div>`)).join('');
    const stamps = (normalized.stamps || []).map(stamp => {
        var _a, _b;
        return (`<img class="viewer-export-print-stamp" src="${escapeHtmlAttribute(stamp.src)}" alt="${escapeHtmlAttribute(stamp.alt || '')}" draggable="false" style="left:${stamp.left}%;top:${stamp.top}%;width:${stamp.width}%;height:${stamp.height}%;opacity:${(_a = stamp.opacity) !== null && _a !== void 0 ? _a : 1};transform:rotate(${(_b = stamp.rotate) !== null && _b !== void 0 ? _b : 0}deg);" />`);
    }).join('');
    const className = pageScoped
        ? 'viewer-export-print-mask viewer-export-print-mask--page'
        : 'viewer-export-print-mask';
    return `<div class="${className}" aria-hidden="true">${blocks}${stamps}</div>`;
};
export const FILE_VIEWER_PRINT_MASK_STYLE = `
  .viewer-export-print-mask{position:absolute;inset:0;z-index:15;pointer-events:none;print-color-adjust:exact;-webkit-print-color-adjust:exact;}
  .viewer-export-print-mask--page{z-index:2147483647;}
  .viewer-export-print-mask-block{position:absolute;box-sizing:border-box;print-color-adjust:exact;-webkit-print-color-adjust:exact;}
  .viewer-export-print-stamp{position:absolute;display:block;object-fit:contain;transform-origin:center center;print-color-adjust:exact;-webkit-print-color-adjust:exact;}
  [data-viewer-print-page-index]{position:relative!important;isolation:isolate;}
`;
export const applyFileViewerPagePrintMasksToHtml = (contentHtml, mask) => {
    var _a, _b;
    const normalized = normalizeFileViewerPrintMaskOptions(mask);
    if (!normalized) {
        return contentHtml;
    }
    const regionsByPage = new Map();
    const stampsByPage = new Map();
    (_a = normalized.regions) === null || _a === void 0 ? void 0 : _a.forEach(region => {
        if (region.pageIndex === undefined) {
            return;
        }
        const pageRegions = regionsByPage.get(region.pageIndex) || [];
        pageRegions.push(region);
        regionsByPage.set(region.pageIndex, pageRegions);
    });
    (_b = normalized.stamps) === null || _b === void 0 ? void 0 : _b.forEach(stamp => {
        if (stamp.pageIndex === undefined) {
            return;
        }
        const pageStamps = stampsByPage.get(stamp.pageIndex) || [];
        pageStamps.push(stamp);
        stampsByPage.set(stamp.pageIndex, pageStamps);
    });
    if (!regionsByPage.size && !stampsByPage.size) {
        return contentHtml;
    }
    return contentHtml.replace(/(<[A-Za-z][^>]*\sdata-viewer-print-page-index=(['"])(\d+)\2[^>]*>)/g, (match, openingTag, _quote, rawPageIndex) => {
        const pageRegions = regionsByPage.get(Number(rawPageIndex));
        const pageStamps = stampsByPage.get(Number(rawPageIndex));
        if (!(pageRegions === null || pageRegions === void 0 ? void 0 : pageRegions.length) && !(pageStamps === null || pageStamps === void 0 ? void 0 : pageStamps.length)) {
            return match;
        }
        return `${openingTag}${buildFileViewerPrintMaskOverlayHtml({
            regions: pageRegions || [],
            stamps: pageStamps || [],
            color: normalized.color,
        }, true)}`;
    });
};
