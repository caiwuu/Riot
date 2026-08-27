export type FileViewerTextEncoding = 'auto' | 'utf-8' | 'utf-16le' | 'utf-16be' | 'gbk' | 'gb18030';
export type ResolvedFileViewerTextEncoding = Exclude<FileViewerTextEncoding, 'auto' | 'gbk'>;
export interface DecodedFileViewerText {
    text: string;
    encoding: ResolvedFileViewerTextEncoding;
}
export interface ResolvedFileViewerTextSource {
    encoding: ResolvedFileViewerTextEncoding;
    bomLength: number;
}
export declare const isValidFileViewerUtf8: (bytes: Uint8Array) => boolean;
export declare const resolveFileViewerTextEncoding: (bytes: Uint8Array, encoding?: FileViewerTextEncoding | string) => ResolvedFileViewerTextSource;
export declare const createFileViewerTextDecoder: (encoding: ResolvedFileViewerTextEncoding) => TextDecoder;
export declare const decodeFileViewerTextBuffer: (data: ArrayBuffer, encoding?: FileViewerTextEncoding | string) => DecodedFileViewerText;
