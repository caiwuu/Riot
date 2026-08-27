/**
 * Lazily opens the print-mask designer.
 * Kept as a dynamic import so the designer stays out of the main core graph,
 * while remaining reachable through the primary `@file-viewer/core` entry
 * (no consumer alias / subpath setup required).
 */
export const openFileViewerPrintMaskDesignerAsync = async (options) => {
    const { openFileViewerPrintMaskDesigner } = await import('../print-mask.js');
    return openFileViewerPrintMaskDesigner(options);
};
