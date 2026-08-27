import type { BinaryInput, ParsedCFB } from '../types.js';
/**
 * Parses the CFB/OLE container that wraps a legacy `.doc` file.
 * The returned object exposes stream helpers so higher layers can focus on
 * Word-specific structures instead of low-level sector navigation.
 */
export declare function parseCFB(input: BinaryInput, _options?: Record<string, unknown>): ParsedCFB;
