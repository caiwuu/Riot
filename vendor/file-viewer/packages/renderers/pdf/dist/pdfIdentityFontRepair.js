import { decodePDFRawStream, PDFArray, PDFDict, PDFDocument, PDFHexString, PDFName, PDFNumber, PDFRawStream, PDFString, } from 'pdf-lib';
const MAX_REPAIR_SOURCE_BYTES = 64 * 1024 * 1024;
const MAX_TTF_TABLES = 256;
const MAX_CMAP_GLYPHS = 0xffff;
const MAX_CMAP_CODEPOINT_VISITS = 0x10000;
const MAX_DECODED_FONT_BYTES = 64 * 1024 * 1024;
const CMAP_BFCHAR_CHUNK_SIZE = 100;
const BASE_FONT = PDFName.of('BaseFont');
const CID_TO_GID_MAP = PDFName.of('CIDToGIDMap');
const DESCENDANT_FONTS = PDFName.of('DescendantFonts');
const ENCODING = PDFName.of('Encoding');
const FONT = PDFName.of('Font');
const FONT_DESCRIPTOR = PDFName.of('FontDescriptor');
const FONT_FAMILY = PDFName.of('FontFamily');
const FONT_FILE_2 = PDFName.of('FontFile2');
const LENGTH_1 = PDFName.of('Length1');
const RESOURCES = PDFName.of('Resources');
const SUBTYPE = PDFName.of('Subtype');
const TO_UNICODE = PDFName.of('ToUnicode');
const X_OBJECT = PDFName.of('XObject');
const CJK_FONT_FAMILY_ALIASES = {
    '微软雅黑': 'microsoftyahei',
    '宋体': 'simsun',
    '黑体': 'simhei',
    '楷体': 'kaiti',
    '仿宋': 'fangsong',
};
const CJK_FONT_FAMILY_MARKERS = [
    'microsoftyahei',
    'simsun',
    'nsimsun',
    'simhei',
    'kaiti',
    'fangsong',
    'pingfang',
    'songti',
    'heiti',
    'hiragino',
    'notosanscjk',
    'sourcehansans',
    'sourcehanserif',
];
const readUint16 = (bytes, offset) => {
    if (offset < 0 || offset + 2 > bytes.length) {
        throw new Error('Unexpected end of TrueType font data.');
    }
    return (bytes[offset] << 8) | bytes[offset + 1];
};
const readUint32 = (bytes, offset) => {
    if (offset < 0 || offset + 4 > bytes.length) {
        throw new Error('Unexpected end of TrueType font data.');
    }
    return (bytes[offset] * 0x1000000 +
        (bytes[offset + 1] << 16) +
        (bytes[offset + 2] << 8) +
        bytes[offset + 3]) >>> 0;
};
const readTag = (bytes, offset) => {
    if (offset < 0 || offset + 4 > bytes.length) {
        return '';
    }
    return String.fromCharCode(bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]);
};
const normalizeFontFamily = (value) => {
    const withoutSubset = value.replace(/^[A-Z]{6}\+/i, '');
    const withoutStyle = withoutSubset.replace(/(?:[\s,_-]+)(?:bold|regular|italic|oblique|medium|semibold|demibold|light|black|thin)(?:mt)?(?:[\s,_-].*)?$/i, '');
    const normalized = withoutStyle
        .normalize('NFKC')
        .toLowerCase()
        .replace(/[\s,_-]+/g, '');
    return CJK_FONT_FAMILY_ALIASES[normalized] || normalized;
};
const isCjkFontFamily = (family) => {
    const key = normalizeFontFamily(family);
    return CJK_FONT_FAMILY_MARKERS.some(marker => key.includes(marker)) ||
        /[\u3400-\u9fff]/.test(family);
};
const readFontName = (font) => {
    var _a;
    return ((_a = font.lookupMaybe(BASE_FONT, PDFName)) === null || _a === void 0 ? void 0 : _a.decodeText()) || '';
};
const readFontFamily = (descriptor) => {
    var _a;
    if (!descriptor) {
        return '';
    }
    return ((_a = descriptor.lookupMaybe(FONT_FAMILY, PDFString, PDFHexString)) === null || _a === void 0 ? void 0 : _a.decodeText()) || '';
};
const getDescendantFont = (font) => {
    var _a;
    return (_a = font.lookupMaybe(DESCENDANT_FONTS, PDFArray)) === null || _a === void 0 ? void 0 : _a.lookupMaybe(0, PDFDict);
};
const getEmbeddedTrueTypeFont = (descriptor) => {
    var _a;
    if (!descriptor) {
        return undefined;
    }
    const fontFile = descriptor.get(FONT_FILE_2);
    if (!fontFile) {
        return undefined;
    }
    const resolved = descriptor.context.lookup(fontFile);
    if (!(resolved instanceof PDFRawStream)) {
        return undefined;
    }
    const declaredLength = (_a = resolved.dict.lookupMaybe(LENGTH_1, PDFNumber)) === null || _a === void 0 ? void 0 : _a.asNumber();
    if (typeof declaredLength !== 'number' ||
        !Number.isSafeInteger(declaredLength) ||
        declaredLength < 1 ||
        declaredLength > MAX_DECODED_FONT_BYTES ||
        resolved.contents.byteLength > MAX_DECODED_FONT_BYTES) {
        return undefined;
    }
    return resolved;
};
const createFontRecord = (font) => {
    var _a;
    const descendant = getDescendantFont(font);
    if (!descendant) {
        return null;
    }
    const encoding = (_a = font.lookupMaybe(ENCODING, PDFName)) === null || _a === void 0 ? void 0 : _a.decodeText();
    if (encoding !== 'Identity-H' && encoding !== 'Identity-V') {
        return null;
    }
    const descriptor = descendant.lookupMaybe(FONT_DESCRIPTOR, PDFDict);
    const baseFont = readFontName(font);
    const descriptorFamily = readFontFamily(descriptor);
    const cidToGidMapObject = descendant.get(CID_TO_GID_MAP);
    const cidToGidMap = cidToGidMapObject
        ? descendant.context.lookup(cidToGidMapObject)
        : undefined;
    const familyKeys = new Set([baseFont, descriptorFamily]
        .filter(Boolean)
        .map(normalizeFontFamily)
        .filter(Boolean));
    return {
        font,
        descendant,
        familyKeys,
        baseFont,
        identityCidToGidMap: !cidToGidMap ||
            (cidToGidMap instanceof PDFName && cidToGidMap.decodeText() === 'Identity'),
        embeddedFont: getEmbeddedTrueTypeFont(descriptor),
    };
};
const collectFontRecords = (pdfDocument) => {
    const records = new Map();
    const visitedResources = new Set();
    const visitedXObjects = new Set();
    const visitResources = (resources) => {
        var _a;
        if (!resources || visitedResources.has(resources)) {
            return;
        }
        visitedResources.add(resources);
        const fonts = resources.lookupMaybe(FONT, PDFDict);
        for (const fontObject of (fonts === null || fonts === void 0 ? void 0 : fonts.values()) || []) {
            const font = pdfDocument.context.lookup(fontObject);
            if (!(font instanceof PDFDict) || records.has(font)) {
                continue;
            }
            const record = createFontRecord(font);
            if (record) {
                records.set(font, record);
            }
        }
        const xObjects = resources.lookupMaybe(X_OBJECT, PDFDict);
        for (const xObjectRef of (xObjects === null || xObjects === void 0 ? void 0 : xObjects.values()) || []) {
            const xObject = pdfDocument.context.lookup(xObjectRef);
            if (!(xObject instanceof PDFRawStream) || visitedXObjects.has(xObject)) {
                continue;
            }
            visitedXObjects.add(xObject);
            if (((_a = xObject.dict.lookupMaybe(SUBTYPE, PDFName)) === null || _a === void 0 ? void 0 : _a.decodeText()) !== 'Form') {
                continue;
            }
            visitResources(xObject.dict.lookupMaybe(RESOURCES, PDFDict));
        }
    };
    for (const page of pdfDocument.getPages()) {
        visitResources(page.node.Resources());
    }
    return [...records.values()];
};
const findCmapTable = (fontBytes) => {
    if (fontBytes.length < 12) {
        throw new Error('Invalid TrueType font header.');
    }
    const tableCount = readUint16(fontBytes, 4);
    if (tableCount < 1 || tableCount > MAX_TTF_TABLES) {
        throw new Error(`Invalid TrueType table count: ${tableCount}.`);
    }
    let cmapOffset = -1;
    for (let index = 0; index < tableCount; index += 1) {
        const entryOffset = 12 + index * 16;
        if (entryOffset + 16 > fontBytes.length) {
            throw new Error('Invalid TrueType table directory.');
        }
        if (readTag(fontBytes, entryOffset) === 'cmap') {
            cmapOffset = readUint32(fontBytes, entryOffset + 8);
            break;
        }
    }
    if (cmapOffset < 0 || cmapOffset + 4 > fontBytes.length) {
        throw new Error('TrueType font does not contain a readable cmap table.');
    }
    const cmapCount = readUint16(fontBytes, cmapOffset + 2);
    const tables = [];
    for (let index = 0; index < cmapCount; index += 1) {
        const recordOffset = cmapOffset + 4 + index * 8;
        if (recordOffset + 8 > fontBytes.length) {
            break;
        }
        const offset = cmapOffset + readUint32(fontBytes, recordOffset + 4);
        if (offset + 2 > fontBytes.length) {
            continue;
        }
        const format = readUint16(fontBytes, offset);
        if (format === 4 || format === 12) {
            tables.push({
                offset,
                format,
                platformId: readUint16(fontBytes, recordOffset),
                encodingId: readUint16(fontBytes, recordOffset + 2),
            });
        }
    }
    const score = (table) => (table.format === 12 ? 100 : 0) +
        (table.platformId === 3 ? 20 : 0) +
        (table.platformId === 0 ? 10 : 0) +
        (table.encodingId === 10 ? 4 : 0) +
        (table.encodingId === 1 ? 2 : 0);
    const selected = tables.sort((left, right) => score(right) - score(left))[0];
    if (!selected) {
        throw new Error('TrueType font does not contain a supported Unicode cmap.');
    }
    return selected;
};
const setGlyphMapping = (map, glyphId, codePoint) => {
    if (glyphId > 0 &&
        glyphId <= MAX_CMAP_GLYPHS &&
        codePoint > 0 &&
        codePoint <= 0x10ffff &&
        !map.has(glyphId)) {
        map.set(glyphId, codePoint);
    }
};
const parseFormat12Cmap = (fontBytes, table, glyphToUnicode) => {
    const tableLength = readUint32(fontBytes, table.offset + 4);
    const tableEnd = table.offset + tableLength;
    const groupCount = readUint32(fontBytes, table.offset + 12);
    if (tableLength < 16 ||
        tableEnd > fontBytes.length ||
        table.offset + 16 + groupCount * 12 > tableEnd) {
        throw new Error('Invalid TrueType cmap format 12 groups.');
    }
    let visitedCodePoints = 0;
    for (let index = 0; index < groupCount; index += 1) {
        const offset = table.offset + 16 + index * 12;
        const startCodePoint = readUint32(fontBytes, offset);
        const endCodePoint = readUint32(fontBytes, offset + 4);
        const startGlyphId = readUint32(fontBytes, offset + 8);
        const length = Math.min(endCodePoint - startCodePoint + 1, MAX_CMAP_GLYPHS - startGlyphId + 1);
        if (!Number.isSafeInteger(length) || length < 1) {
            continue;
        }
        for (let innerIndex = 0; innerIndex < length; innerIndex += 1) {
            if (visitedCodePoints >= MAX_CMAP_CODEPOINT_VISITS) {
                return;
            }
            visitedCodePoints += 1;
            setGlyphMapping(glyphToUnicode, startGlyphId + innerIndex, startCodePoint + innerIndex);
        }
    }
};
const parseFormat4Cmap = (fontBytes, table, glyphToUnicode) => {
    const tableLength = readUint16(fontBytes, table.offset + 2);
    const tableEnd = table.offset + tableLength;
    const segmentCount = readUint16(fontBytes, table.offset + 6) / 2;
    const endCodesOffset = table.offset + 14;
    const startCodesOffset = endCodesOffset + segmentCount * 2 + 2;
    const deltasOffset = startCodesOffset + segmentCount * 2;
    const rangeOffsetsOffset = deltasOffset + segmentCount * 2;
    if (!Number.isInteger(segmentCount) ||
        segmentCount < 1 ||
        tableLength < 16 ||
        tableEnd > fontBytes.length ||
        rangeOffsetsOffset + segmentCount * 2 > tableEnd) {
        throw new Error('Invalid TrueType cmap format 4 segments.');
    }
    let visitedCodePoints = 0;
    for (let index = 0; index < segmentCount; index += 1) {
        const endCodePoint = readUint16(fontBytes, endCodesOffset + index * 2);
        const startCodePoint = readUint16(fontBytes, startCodesOffset + index * 2);
        const delta = readUint16(fontBytes, deltasOffset + index * 2);
        const rangeOffset = readUint16(fontBytes, rangeOffsetsOffset + index * 2);
        if (endCodePoint < startCodePoint) {
            continue;
        }
        for (let codePoint = startCodePoint; codePoint <= endCodePoint && codePoint !== 0xffff; codePoint += 1) {
            if (visitedCodePoints >= MAX_CMAP_CODEPOINT_VISITS) {
                return;
            }
            visitedCodePoints += 1;
            let glyphId = 0;
            if (rangeOffset === 0) {
                glyphId = (codePoint + delta) & 0xffff;
            }
            else {
                const glyphOffset = rangeOffsetsOffset + index * 2 + rangeOffset +
                    (codePoint - startCodePoint) * 2;
                if (glyphOffset + 2 <= tableEnd) {
                    glyphId = readUint16(fontBytes, glyphOffset);
                    if (glyphId) {
                        glyphId = (glyphId + delta) & 0xffff;
                    }
                }
            }
            setGlyphMapping(glyphToUnicode, glyphId, codePoint);
        }
    }
};
const parseTrueTypeGlyphToUnicode = (fontBytes) => {
    const selected = findCmapTable(fontBytes);
    const glyphToUnicode = new Map();
    if (selected.format === 12) {
        parseFormat12Cmap(fontBytes, selected, glyphToUnicode);
    }
    else {
        parseFormat4Cmap(fontBytes, selected, glyphToUnicode);
    }
    return glyphToUnicode;
};
const toHex = (value, minLength = 4) => value.toString(16).toUpperCase().padStart(minLength, '0');
const encodeUnicodeCodePoint = (codePoint) => {
    if (codePoint <= 0xffff) {
        return toHex(codePoint);
    }
    const offset = codePoint - 0x10000;
    const highSurrogate = 0xd800 + (offset >> 10);
    const lowSurrogate = 0xdc00 + (offset & 0x3ff);
    return `${toHex(highSurrogate)}${toHex(lowSurrogate)}`;
};
const createToUnicodeCmap = (glyphToUnicode) => {
    const entries = [...glyphToUnicode]
        .filter(([glyphId, codePoint]) => glyphId > 0 && glyphId <= 0xffff &&
        codePoint > 0 && codePoint <= 0x10ffff)
        .sort(([left], [right]) => left - right);
    const sections = [];
    for (let index = 0; index < entries.length; index += CMAP_BFCHAR_CHUNK_SIZE) {
        const chunk = entries.slice(index, index + CMAP_BFCHAR_CHUNK_SIZE);
        sections.push([
            `${chunk.length} beginbfchar`,
            ...chunk.map(([glyphId, codePoint]) => `<${toHex(glyphId)}> <${encodeUnicodeCodePoint(codePoint)}>`),
            'endbfchar',
        ].join('\n'));
    }
    return [
        '/CIDInit /ProcSet findresource begin',
        '12 dict begin',
        'begincmap',
        '/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def',
        '/CMapName /Adobe-Identity-UCS def',
        '/CMapType 2 def',
        '1 begincodespacerange',
        '<0000> <FFFF>',
        'endcodespacerange',
        ...sections,
        'endcmap',
        'CMapName currentdict /CMap defineresource pop',
        'end',
        'end',
    ].join('\n');
};
const familiesOverlap = (left, right) => {
    for (const key of left) {
        if (right.has(key)) {
            return true;
        }
    }
    return false;
};
export const repairMalformedIdentityCjkFonts = async (sourceBytes, candidateFamilies = []) => {
    if (!sourceBytes.byteLength || sourceBytes.byteLength > MAX_REPAIR_SOURCE_BYTES) {
        return { bytes: sourceBytes, repairedFonts: 0, repairedFamilies: [] };
    }
    const candidateKeys = new Set(candidateFamilies.map(normalizeFontFamily).filter(Boolean));
    const pdfDocument = await PDFDocument.load(sourceBytes, {
        updateMetadata: false,
    });
    const records = collectFontRecords(pdfDocument);
    const sourceMaps = new Map();
    const getSourceMap = (record) => {
        const cached = sourceMaps.get(record);
        if (cached) {
            return cached;
        }
        if (!record.embeddedFont) {
            return undefined;
        }
        const decoded = decodePDFRawStream(record.embeddedFont).decode();
        if (decoded.byteLength > MAX_DECODED_FONT_BYTES) {
            throw new Error('Embedded TrueType font exceeds the Identity repair limit.');
        }
        const glyphMap = parseTrueTypeGlyphToUnicode(decoded);
        sourceMaps.set(record, glyphMap);
        return glyphMap;
    };
    let repairedFonts = 0;
    const repairedFamilies = new Set();
    for (const target of records) {
        if (target.font.has(TO_UNICODE) ||
            target.embeddedFont ||
            !target.identityCidToGidMap ||
            ![...target.familyKeys].some(key => isCjkFontFamily(key)) ||
            (candidateKeys.size > 0 && ![...target.familyKeys].some(key => candidateKeys.has(key)))) {
            continue;
        }
        let bestMap;
        for (const source of records) {
            if (!source.embeddedFont || !familiesOverlap(target.familyKeys, source.familyKeys)) {
                continue;
            }
            try {
                const sourceMap = getSourceMap(source);
                if (sourceMap && (!bestMap || sourceMap.size > bestMap.size)) {
                    bestMap = sourceMap;
                }
            }
            catch {
                // A different same-family embedded font may still provide a valid cmap.
            }
        }
        if (!bestMap || bestMap.size < 2) {
            continue;
        }
        const cmap = createToUnicodeCmap(bestMap);
        const cmapRef = pdfDocument.context.register(pdfDocument.context.flateStream(cmap));
        target.font.set(TO_UNICODE, cmapRef);
        repairedFonts += 1;
        repairedFamilies.add(target.baseFont || [...target.familyKeys][0] || 'CJK font');
    }
    if (!repairedFonts) {
        return { bytes: sourceBytes, repairedFonts: 0, repairedFamilies: [] };
    }
    return {
        bytes: await pdfDocument.save({ useObjectStreams: true }),
        repairedFonts,
        repairedFamilies: [...repairedFamilies],
    };
};
