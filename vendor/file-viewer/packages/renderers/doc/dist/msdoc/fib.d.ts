import { BinaryReader } from '../core/binary.js';
import type { FibBase, ParsedFib } from '../types.js';
export declare function parseFib(wordBytes: Uint8Array): ParsedFib;
export declare function parseFibBase(reader: BinaryReader): FibBase;
