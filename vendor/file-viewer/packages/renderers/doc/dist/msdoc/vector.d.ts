export interface ConvertedVectorImage {
    mime: 'image/svg+xml';
    bytes: Uint8Array;
    dataUrl: string;
    width: number;
    height: number;
    sourceMime: 'image/emf' | 'image/wmf';
    recordCount: number;
}
export declare function convertMetafileToSvg(mime: string, bytes: Uint8Array): ConvertedVectorImage | null;
