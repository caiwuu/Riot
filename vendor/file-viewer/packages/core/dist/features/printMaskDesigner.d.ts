import type { FileViewerPrintMaskOptions, FileViewerPrintMaskRegion, FileViewerPrintStamp } from '../contracts/types';
import { type FileViewerI18nInput } from '../i18n/messages';
export interface OpenFileViewerPrintMaskDesignerOptions {
    root: HTMLElement;
    pages?: readonly HTMLElement[];
    i18n?: FileViewerI18nInput;
    color?: string;
    initialRegions?: FileViewerPrintMaskRegion[];
    initialStamps?: FileViewerPrintStamp[];
}
export interface FileViewerPrintMaskDesignerResult {
    mask: FileViewerPrintMaskOptions;
}
/**
 * Opens a page-aware print-mask designer. Browsing remains the default mode;
 * drawing is armed for the currently visible page only and disarms after one block.
 */
export declare const openFileViewerPrintMaskDesigner: (options: OpenFileViewerPrintMaskDesignerOptions) => Promise<FileViewerPrintMaskDesignerResult | null>;
