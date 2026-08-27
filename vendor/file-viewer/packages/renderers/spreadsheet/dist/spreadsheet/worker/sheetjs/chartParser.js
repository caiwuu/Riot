import { DOMParser } from '@xmldom/xmldom';
import JSZip from 'jszip';
const CHART_RELATIONSHIP_SUFFIX = '/chart';
const DRAWING_RELATIONSHIP_SUFFIX = '/drawing';
const WORKSHEET_RELATIONSHIP_SUFFIX = '/worksheet';
const CHART_TYPE_MAP = {
    areaChart: 'area',
    area3DChart: 'area',
    barChart: 'bar',
    bar3DChart: 'bar',
    doughnutChart: 'doughnut',
    lineChart: 'line',
    line3DChart: 'line',
    pieChart: 'pie',
    pie3DChart: 'pie',
    radarChart: 'radar',
    scatterChart: 'scatter'
};
const LEGEND_POSITION_MAP = {
    b: 'bottom',
    l: 'left',
    r: 'right',
    t: 'top',
    tr: 'top'
};
const SCHEME_COLORS = {
    accent1: '#4472c4',
    accent2: '#ed7d31',
    accent3: '#a5a5a5',
    accent4: '#ffc000',
    accent5: '#5b9bd5',
    accent6: '#70ad47',
    dk1: '#000000',
    dk2: '#44546a',
    lt1: '#ffffff',
    lt2: '#e7e6e6',
    tx1: '#000000',
    tx2: '#44546a'
};
export const localName = (node) => {
    const name = node.localName || node.nodeName;
    return name.split(':').pop() || name;
};
export const childElements = (node) => {
    if (!node) {
        return [];
    }
    return Array.from(node.childNodes).filter((child) => child.nodeType === 1);
};
const childrenByLocal = (node, name) => {
    return childElements(node).filter((child) => localName(child) === name);
};
const firstChildByLocal = (node, name) => {
    return childrenByLocal(node, name)[0];
};
export const elementsByLocal = (node, name) => {
    const result = [];
    const visit = (current) => {
        childElements(current).forEach((child) => {
            if (localName(child) === name) {
                result.push(child);
            }
            visit(child);
        });
    };
    if (node) {
        visit(node);
    }
    return result;
};
const firstByLocal = (node, name) => {
    return elementsByLocal(node, name)[0];
};
const numericAttribute = (element, name = 'val') => {
    const value = Number(element === null || element === void 0 ? void 0 : element.getAttribute(name));
    return Number.isFinite(value) ? value : 0;
};
const textContent = (element) => {
    var _a;
    return ((_a = element === null || element === void 0 ? void 0 : element.textContent) === null || _a === void 0 ? void 0 : _a.trim()) || '';
};
export const relationshipId = (element) => {
    if (!element) {
        return '';
    }
    return (element.getAttribute('r:id') ||
        element.getAttributeNS('http://schemas.openxmlformats.org/officeDocument/2006/relationships', 'id') ||
        '');
};
const resolvePartPath = (sourcePart, target) => {
    const sourceDirectory = sourcePart.includes('/')
        ? sourcePart.slice(0, sourcePart.lastIndexOf('/'))
        : '';
    const parts = (target.startsWith('/') ? target.slice(1) : `${sourceDirectory}/${target}`).split('/');
    const normalized = [];
    for (const part of parts) {
        if (!part || part === '.') {
            continue;
        }
        if (part === '..') {
            normalized.pop();
            continue;
        }
        normalized.push(part);
    }
    return normalized.join('/');
};
const relationshipPartPath = (sourcePart) => {
    const slash = sourcePart.lastIndexOf('/');
    const directory = slash >= 0 ? sourcePart.slice(0, slash) : '';
    const filename = slash >= 0 ? sourcePart.slice(slash + 1) : sourcePart;
    return `${directory ? `${directory}/` : ''}_rels/${filename}.rels`;
};
const parseXml = (xml) => {
    // Some valid Office producers prefix relationship parts with a UTF-8 BOM.
    // XML declarations must otherwise be the first character, and xmldom reports
    // the BOM as content outside the root element before aborting chart parsing.
    return new DOMParser().parseFromString(xml.replace(/^[\uFEFF\s]+/, ''), 'application/xml');
};
export const loadXml = async (zip, path) => {
    const file = zip.file(path);
    if (!file) {
        return null;
    }
    return parseXml(await file.async('text'));
};
export const loadRelationships = async (zip, sourcePart) => {
    const document = await loadXml(zip, relationshipPartPath(sourcePart));
    if (!document) {
        return [];
    }
    return elementsByLocal(document.documentElement, 'Relationship').flatMap((element) => {
        const id = element.getAttribute('Id') || '';
        const target = element.getAttribute('Target') || '';
        const type = element.getAttribute('Type') || '';
        if (!id || !target || element.getAttribute('TargetMode') === 'External') {
            return [];
        }
        return [{ id, target: resolvePartPath(sourcePart, target), type }];
    });
};
export const relationById = (relationships, id) => {
    return relationships.find((relationship) => relationship.id === id);
};
const parseMarker = (element) => {
    if (!element) {
        return undefined;
    }
    return {
        row: Number(textContent(firstChildByLocal(element, 'row'))) || 0,
        col: Number(textContent(firstChildByLocal(element, 'col'))) || 0,
        rowOff: Number(textContent(firstChildByLocal(element, 'rowOff'))) || 0,
        colOff: Number(textContent(firstChildByLocal(element, 'colOff'))) || 0
    };
};
const columnIndex = (letters) => {
    let result = 0;
    for (const letter of letters.toUpperCase()) {
        result = result * 26 + letter.charCodeAt(0) - 64;
    }
    return result - 1;
};
const parseCellAddress = (address) => {
    const match = /^\$?([A-Z]{1,3})\$?(\d+)$/i.exec(address.trim());
    if (!match) {
        return null;
    }
    return {
        col: columnIndex(match[1]),
        row: Number(match[2]) - 1
    };
};
const encodeCellAddress = (row, col) => {
    let value = col + 1;
    let letters = '';
    while (value > 0) {
        const remainder = (value - 1) % 26;
        letters = String.fromCharCode(65 + remainder) + letters;
        value = Math.floor((value - 1) / 26);
    }
    return `${letters}${row + 1}`;
};
const getWorksheetCell = (worksheet, row, col) => {
    var _a, _b;
    return ((_b = (_a = worksheet['!data']) === null || _a === void 0 ? void 0 : _a[row]) === null || _b === void 0 ? void 0 : _b[col])
        || worksheet[encodeCellAddress(row, col)];
};
const parseFormulaRange = (formula) => {
    const normalized = formula.trim().replace(/^=/, '');
    const separator = normalized.lastIndexOf('!');
    if (separator <= 0) {
        return null;
    }
    const sheetToken = normalized.slice(0, separator).trim();
    const rangeToken = normalized.slice(separator + 1).trim();
    if (!sheetToken || sheetToken.includes('[')) {
        return null;
    }
    const sheetName = sheetToken.startsWith("'") && sheetToken.endsWith("'")
        ? sheetToken.slice(1, -1).replace(/''/g, "'")
        : sheetToken;
    const [startToken, endToken = startToken] = rangeToken.split(':');
    const start = parseCellAddress(startToken);
    const end = parseCellAddress(endToken);
    if (!sheetName || !start || !end) {
        return null;
    }
    return {
        sheetName,
        start: {
            row: Math.min(start.row, end.row),
            col: Math.min(start.col, end.col)
        },
        end: {
            row: Math.max(start.row, end.row),
            col: Math.max(start.col, end.col)
        }
    };
};
const resolveFormulaValues = (formula, workbook, formatted) => {
    var _a;
    const range = parseFormulaRange(formula);
    const worksheet = range && ((_a = workbook === null || workbook === void 0 ? void 0 : workbook.Sheets) === null || _a === void 0 ? void 0 : _a[range.sheetName]);
    if (!range || !worksheet) {
        return [];
    }
    const values = [];
    for (let row = range.start.row; row <= range.end.row; row += 1) {
        for (let col = range.start.col; col <= range.end.col; col += 1) {
            const cell = getWorksheetCell(worksheet, row, col);
            const value = formatted && (cell === null || cell === void 0 ? void 0 : cell.w) !== undefined ? cell.w : cell === null || cell === void 0 ? void 0 : cell.v;
            values.push(value === undefined || value === null ? '' : `${value}`);
        }
    }
    return values;
};
const parsePointValues = (element, workbook, formatted = true) => {
    if (!element) {
        return [];
    }
    const cachedValues = elementsByLocal(element, 'pt')
        .map((point) => ({
        index: Number(point.getAttribute('idx')) || 0,
        value: textContent(firstChildByLocal(point, 'v')) || textContent(firstByLocal(point, 'v'))
    }))
        .sort((left, right) => left.index - right.index)
        .map((point) => point.value);
    if (cachedValues.length) {
        return cachedValues;
    }
    const formula = textContent(firstByLocal(element, 'f'));
    return formula ? resolveFormulaValues(formula, workbook, formatted) : [];
};
const chartText = (element, workbook) => {
    if (!element) {
        return '';
    }
    const points = parsePointValues(element, workbook);
    if (points.length) {
        return points.join(' ').trim();
    }
    const richText = elementsByLocal(element, 't').map(textContent).filter(Boolean).join(' ').trim();
    if (richText) {
        return richText;
    }
    return textContent(firstByLocal(element, 'v'));
};
const parseSeriesColor = (series) => {
    var _a, _b;
    const shape = firstChildByLocal(series, 'spPr');
    const solidFill = firstByLocal(shape, 'solidFill');
    const rgb = (_a = firstByLocal(solidFill, 'srgbClr')) === null || _a === void 0 ? void 0 : _a.getAttribute('val');
    if (rgb && /^[0-9a-f]{6}$/i.test(rgb)) {
        return `#${rgb}`;
    }
    const scheme = ((_b = firstByLocal(solidFill, 'schemeClr')) === null || _b === void 0 ? void 0 : _b.getAttribute('val')) || '';
    return SCHEME_COLORS[scheme];
};
const parseSeries = (chartNode, workbook) => {
    return childrenByLocal(chartNode, 'ser').map((series, index) => {
        const tx = firstChildByLocal(series, 'tx');
        const category = firstChildByLocal(series, 'cat') || firstChildByLocal(series, 'xVal');
        const value = firstChildByLocal(series, 'val') || firstChildByLocal(series, 'yVal');
        const categories = parsePointValues(category, workbook);
        const values = parsePointValues(value, workbook, false).map(Number).filter(Number.isFinite);
        return {
            name: chartText(tx, workbook) || `Series ${index + 1}`,
            categories: categories.length
                ? categories
                : values.map((_, valueIndex) => `${valueIndex + 1}`),
            values,
            color: parseSeriesColor(series)
        };
    });
};
const parseChart = (document, workbook) => {
    var _a, _b, _c;
    const root = document.documentElement;
    const chart = firstByLocal(root, 'chart');
    const plotArea = firstChildByLocal(chart, 'plotArea') || firstByLocal(chart, 'plotArea');
    const chartEntry = childElements(plotArea)
        .map((element) => ({ element, type: CHART_TYPE_MAP[localName(element)] }))
        .find((entry) => entry.type);
    if (!chartEntry) {
        return null;
    }
    const legend = firstChildByLocal(chart, 'legend');
    const legendPositionValue = ((_a = firstChildByLocal(legend, 'legendPos')) === null || _a === void 0 ? void 0 : _a.getAttribute('val')) || '';
    const categoryAxis = firstChildByLocal(plotArea, 'catAx');
    const valueAxis = firstChildByLocal(plotArea, 'valAx');
    const barDirection = ((_b = firstChildByLocal(chartEntry.element, 'barDir')) === null || _b === void 0 ? void 0 : _b.getAttribute('val')) === 'bar'
        ? 'bar'
        : 'column';
    return {
        type: chartEntry.type,
        title: chartText(firstChildByLocal(chart, 'title'), workbook) || undefined,
        categoryAxisTitle: chartText(firstChildByLocal(categoryAxis, 'title'), workbook) || undefined,
        valueAxisTitle: chartText(firstChildByLocal(valueAxis, 'title'), workbook) || undefined,
        barDirection,
        grouping: ((_c = firstChildByLocal(chartEntry.element, 'grouping')) === null || _c === void 0 ? void 0 : _c.getAttribute('val')) || undefined,
        legendPosition: legend ? LEGEND_POSITION_MAP[legendPositionValue] || 'bottom' : undefined,
        series: parseSeries(chartEntry.element, workbook)
    };
};
const parseDrawingCharts = async (zip, drawingPart, workbook) => {
    const [document, relationships] = await Promise.all([
        loadXml(zip, drawingPart),
        loadRelationships(zip, drawingPart)
    ]);
    if (!document) {
        return [];
    }
    const anchors = childElements(document.documentElement).filter((element) => localName(element).endsWith('Anchor'));
    const charts = await Promise.all(anchors.map(async (anchor, index) => {
        var _a;
        const chartReference = firstByLocal(anchor, 'chart');
        const chartRelationship = relationById(relationships, relationshipId(chartReference));
        if (!(chartRelationship === null || chartRelationship === void 0 ? void 0 : chartRelationship.type.endsWith(CHART_RELATIONSHIP_SUFFIX))) {
            return null;
        }
        const chartDocument = await loadXml(zip, chartRelationship.target);
        const chart = chartDocument ? parseChart(chartDocument, workbook) : null;
        if (!chart || !chart.series.length || chart.series.every((series) => !series.values.length)) {
            return null;
        }
        const from = parseMarker(firstChildByLocal(anchor, 'from')) || {
            row: 0,
            col: 0,
            rowOff: numericAttribute(firstChildByLocal(anchor, 'pos'), 'y'),
            colOff: numericAttribute(firstChildByLocal(anchor, 'pos'), 'x')
        };
        const to = parseMarker(firstChildByLocal(anchor, 'to'));
        const extElement = firstChildByLocal(anchor, 'ext');
        const name = (_a = firstByLocal(anchor, 'cNvPr')) === null || _a === void 0 ? void 0 : _a.getAttribute('name');
        return {
            ...chart,
            id: name || chartRelationship.target || `chart-${index + 1}`,
            from,
            to,
            ext: extElement
                ? {
                    width: numericAttribute(extElement, 'cx'),
                    height: numericAttribute(extElement, 'cy')
                }
                : undefined
        };
    }));
    return charts.filter((chart) => chart !== null);
};
export const parseSpreadsheetCharts = async (data, workbook) => {
    const zip = await JSZip.loadAsync(data);
    const workbookPart = 'xl/workbook.xml';
    const [workbookDocument, workbookRelationships] = await Promise.all([
        loadXml(zip, workbookPart),
        loadRelationships(zip, workbookPart)
    ]);
    const result = {};
    if (!workbookDocument) {
        return result;
    }
    for (const sheet of elementsByLocal(workbookDocument.documentElement, 'sheet')) {
        const name = sheet.getAttribute('name') || '';
        const worksheetRelationship = relationById(workbookRelationships, relationshipId(sheet));
        if (!name || !(worksheetRelationship === null || worksheetRelationship === void 0 ? void 0 : worksheetRelationship.type.endsWith(WORKSHEET_RELATIONSHIP_SUFFIX))) {
            continue;
        }
        // A worksheet can expand to hundreds of megabytes even when the drawing
        // relationship part is only a few hundred bytes. Loading sheetN.xml as a
        // string here duplicates the cell parser's work and can exceed V8's string
        // limit before chart parsing starts. Drawing relationships already carry
        // the typed targets needed by the chart parser, so discover them directly.
        const worksheetRelationships = await loadRelationships(zip, worksheetRelationship.target);
        const drawingParts = Array.from(new Set(worksheetRelationships
            .filter((relationship) => relationship.type.endsWith(DRAWING_RELATIONSHIP_SUFFIX))
            .map((relationship) => relationship.target)));
        const charts = (await Promise.all(drawingParts.map((part) => parseDrawingCharts(zip, part, workbook)))).flat();
        if (charts.length) {
            result[name] = charts;
        }
    }
    return result;
};
