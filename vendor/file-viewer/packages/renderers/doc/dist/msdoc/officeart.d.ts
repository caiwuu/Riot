export declare function detectMimeFromBlipType(blipType: number): string | null;
/**
 * Inline picture characters often carry a miniature OfficeArt container that
 * points at the BLIP used by the picture. We only need a compact subset of the
 * shape metadata so the higher-level parser can annotate extracted assets with
 * source shape ids and BLIP indices.
 */
export declare function parseInlineOfficeArtShape(bytes: Uint8Array, startOffset?: number): {
    shapeId: number;
    blipIndex?: number;
    name?: string;
    description?: string;
} | null;
