import type { ImageAsset, MsDocParseOptions, ObjectPoolInfo, ParsedCFB } from '../types.js';
export interface OfficeArtRecordHeader {
    recVer: number;
    recInstance: number;
    recType: number;
    recLen: number;
    size: number;
}
export declare function detectImageSegment(bytes: Uint8Array): {
    mime: string;
    start: number;
    end: number;
} | null;
/**
 * OfficeArt records all start with the common 8-byte OfficeArtRecordHeader.
 * Parsing this explicitly lets us walk inline shape containers instead of guessing
 * image payloads by magic bytes, which was the root cause of the broken image output.
 */
export declare function parseOfficeArtRecordHeader(bytes: Uint8Array, offset: number): OfficeArtRecordHeader | null;
/**
 * Resolves a picture character (U+0001 + sprmCPicLocation) to an HTML-friendly asset.
 * The happy path is: PICF -> optional linked picture name -> OfficeArtInlineSpContainer
 * -> OfficeArtBlip*. When no structured BLIP can be found we still fall back to a
 * signature scan so slightly malformed files remain usable.
 */
export declare function extractPictureAsset(dataStreamBytes: Uint8Array, pictureOffset: number | null | undefined, options?: MsDocParseOptions): ImageAsset | null;
export declare function extractFbseImageAsset(drawingBytes: Uint8Array, offset: number, header?: OfficeArtRecordHeader | null): ImageAsset | null;
export declare function parseOle10Native(streamBytes: Uint8Array): {
    label: string;
    originalPath: string;
    tempPath: string;
    dataSize: number;
    bytes: Uint8Array;
} | null;
export declare function extractObjectPool(cfb: ParsedCFB): Map<string, ObjectPoolInfo>;
