import type { FileViewerFileRef, FileViewerSource } from '../contracts/types';
export type FileViewerPrecheckStatus = 'ready' | 'invalid' | 'unsupported';
export type FileViewerPrecheckReason = 'content-valid' | 'content-not-inspected' | 'content-inconclusive' | 'empty-structured-file' | 'signature-mismatch' | 'invalid-package' | 'missing-package-part' | 'unsupported-extension';
export interface FileViewerPrecheckOptions {
    /** Filename used when a Blob or ArrayBuffer has no name. */
    filename?: string;
    /** Explicit extension, with or without a leading dot. */
    type?: string;
    /** Restricts the result to the renderers installed by the host application. */
    supportedExtensions?: Iterable<string>;
    /** Maximum ZIP central-directory bytes read from a Blob. Defaults to 8 MiB. */
    maxPackageMetadataBytes?: number;
}
export interface FileViewerPrecheckResult {
    filename: string;
    extension: string;
    rendererId: string | null;
    supported: boolean;
    inspected: boolean;
    /** `null` means the extension is supported but no structural validator exists. */
    valid: boolean | null;
    previewable: boolean;
    status: FileViewerPrecheckStatus;
    reason: FileViewerPrecheckReason;
    missingParts: string[];
}
type FileViewerPrecheckInput = FileViewerSource | FileViewerFileRef;
/**
 * Performs a cheap, renderer-free capability and structure check.
 *
 * This intentionally does not promise rendering fidelity. A `ready` result
 * means the extension is available and any known container/signature checks
 * passed; the real renderer remains the final authority for malformed content.
 */
export declare const precheckFileViewerSource: (input: FileViewerPrecheckInput, options?: FileViewerPrecheckOptions) => Promise<FileViewerPrecheckResult>;
export {};
