import { type Document as XmlDocument, type Element as XmlElement, type Node as XmlNode } from '@xmldom/xmldom';
import JSZip from 'jszip';
import type { WorkBook } from 'styled-exceljs';
import type { SheetChartDefinition } from '../type.js';
export type Relationship = {
    id: string;
    target: string;
    type: string;
};
export declare const localName: (node: XmlNode) => string;
export declare const childElements: (node: XmlNode | null | undefined) => XmlElement[];
export declare const elementsByLocal: (node: XmlNode | null | undefined, name: string) => XmlElement[];
export declare const relationshipId: (element: XmlElement | undefined) => string;
export declare const loadXml: (zip: JSZip, path: string) => Promise<XmlDocument | null>;
export declare const loadRelationships: (zip: JSZip, sourcePart: string) => Promise<Relationship[]>;
export declare const relationById: (relationships: Relationship[], id: string) => Relationship | undefined;
export declare const parseSpreadsheetCharts: (data: ArrayBuffer, workbook?: WorkBook | null) => Promise<Record<string, SheetChartDefinition[]>>;
