import type { FileViewerWatermarkOptions } from '../contracts/types';
export type FileViewerWatermarkStyle = Record<string, string> & {
    backgroundImage: string;
};
export interface FileViewerWatermarkPresentationState {
    normalizedWatermark: FileViewerWatermarkOptions | null;
    watermarkStyle: FileViewerWatermarkStyle | undefined;
    watermarkInlineStyle: string;
}
export declare const normalizeFileViewerWatermark: (watermark?: boolean | FileViewerWatermarkOptions) => FileViewerWatermarkOptions | null;
export declare const buildFileViewerWatermarkSvg: (watermark: FileViewerWatermarkOptions) => string;
export declare const buildFileViewerWatermarkBackgroundImage: (watermark?: boolean | FileViewerWatermarkOptions) => string;
export declare const buildFileViewerWatermarkStyle: (watermark?: boolean | FileViewerWatermarkOptions) => FileViewerWatermarkStyle | undefined;
export declare const buildFileViewerWatermarkInlineStyle: (watermark?: boolean | FileViewerWatermarkOptions) => string;
export declare const resolveFileViewerWatermarkPresentationState: (watermark?: boolean | FileViewerWatermarkOptions) => FileViewerWatermarkPresentationState;
