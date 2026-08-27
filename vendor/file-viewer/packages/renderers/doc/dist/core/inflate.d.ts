/**
 * Small RFC1950/zlib + RFC1951/DEFLATE inflater.
 *
 * OfficeArt stores EMF/WMF BLIPs behind OfficeArtMetafileHeader.compression=0x00
 * using the zlib wrapper defined by RFC1950. Keeping this inflater dependency-free
 * preserves the browser-only build while still allowing the synchronous MS-DOC
 * parser to expose browser-displayable SVG assets.
 */
export declare function inflateZlib(bytes: Uint8Array, expectedLength?: number): Uint8Array | null;
