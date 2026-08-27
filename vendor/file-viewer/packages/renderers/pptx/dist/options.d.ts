import type { NativePptxEngineOptions, PptxZipLimits } from './types';
export declare const RECOMMENDED_ZIP_LIMITS: Required<PptxZipLimits>;
export declare const createDefaultPptxOptions: () => NativePptxEngineOptions;
export declare const resolvePptxEngineOptions: (options?: Partial<NativePptxEngineOptions>) => NativePptxEngineOptions;
