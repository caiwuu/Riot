export type PdfIdentityFontRepairResult = {
    bytes: Uint8Array;
    repairedFonts: number;
    repairedFamilies: string[];
};
export declare const repairMalformedIdentityCjkFonts: (sourceBytes: Uint8Array, candidateFamilies?: readonly string[]) => Promise<PdfIdentityFontRepairResult>;
