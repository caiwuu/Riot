const asChartQueue = (charts) => {
    return Array.isArray(charts === null || charts === void 0 ? void 0 : charts.MsgQueue) ? charts.MsgQueue : [];
};
export const findPptxChartTarget = (root, chartID) => {
    const rootElement = root;
    if (rootElement.id === chartID) {
        return rootElement;
    }
    return Array.from(root.querySelectorAll('[id]'))
        .find(element => element.id === chartID) || null;
};
const getNumericBulletText = (type, index) => {
    switch (type) {
        case 'arabicPeriod':
            return `${index}. `;
        case 'arabicParenR':
            return `${index}) `;
        case 'alphaLcParenR':
            return `${String.fromCharCode(index + 96)}) `;
        case 'alphaLcPeriod':
            return `${String.fromCharCode(index + 96)}. `;
        case 'alphaUcParenR':
            return `${String.fromCharCode(index + 64)}) `;
        case 'alphaUcPeriod':
            return `${String.fromCharCode(index + 64)}. `;
        default:
            return String(index);
    }
};
const restoreNumericBullets = (root) => {
    const scopes = [
        ...Array.from(root.querySelectorAll('.block')),
        ...Array.from(root.querySelectorAll('table td')),
    ];
    for (const scope of scopes) {
        const bullets = Array.from(scope.querySelectorAll('.numeric-bullet-style'));
        const counters = new Map();
        for (const bullet of bullets) {
            const type = String(bullet.dataset.bulltname || 'arabicPeriod');
            const level = String(bullet.dataset.bulltlvl || '0');
            const key = `${level}:${type}`;
            const nextIndex = (counters.get(key) || 0) + 1;
            counters.set(key, nextIndex);
            bullet.textContent = getNumericBulletText(type, nextIndex);
        }
    }
};
const TEXT_FIT_MIN_SCALE = 0.7;
const TEXT_FIT_MAX_PASSES = 8;
const TEXT_FIT_TOLERANCE = 1;
const getFitDataKey = (property) => `pptxFit${property.charAt(0).toUpperCase()}${property.slice(1)}`;
const readOriginalPx = (element, property, computed = getComputedStyle(element)) => {
    const key = getFitDataKey(property);
    const existing = Number(element.dataset[key]);
    if (Number.isFinite(existing) && existing > 0) {
        return existing;
    }
    const value = parseFloat(computed[property]);
    if (!Number.isFinite(value) || value <= 0) {
        return undefined;
    }
    element.dataset[key] = String(value);
    return value;
};
const setScaledPx = (element, property, scale, computed = getComputedStyle(element)) => {
    const original = readOriginalPx(element, property, computed);
    if (original === undefined) {
        return;
    }
    element.style[property] = `${original * scale}px`;
};
const collectTextFitElements = (block) => {
    const elements = new Set();
    block.querySelectorAll('.text-block, .numeric-bullet-style').forEach(element => {
        elements.add(element);
    });
    block.querySelectorAll('.slide-prgrph').forEach(paragraph => {
        Array.from(paragraph.children).forEach(child => {
            if (!(child instanceof HTMLElement) || child.querySelector('.text-block')) {
                return;
            }
            const computed = getComputedStyle(child);
            const fontSize = parseFloat(computed.fontSize);
            if (Number.isFinite(fontSize) && fontSize > 0) {
                elements.add(child);
            }
        });
    });
    return elements;
};
const applyTextFitScale = (block, scale) => {
    block.dataset.pptxTextFitScale = String(scale);
    for (const element of collectTextFitElements(block)) {
        const computed = getComputedStyle(element);
        setScaledPx(element, 'fontSize', scale, computed);
        setScaledPx(element, 'lineHeight', scale, computed);
        setScaledPx(element, 'paddingLeft', scale, computed);
        setScaledPx(element, 'paddingRight', scale, computed);
    }
    block.querySelectorAll('.slide-prgrph').forEach(paragraph => {
        const computed = getComputedStyle(paragraph);
        setScaledPx(paragraph, 'lineHeight', scale, computed);
        setScaledPx(paragraph, 'marginTop', scale, computed);
        setScaledPx(paragraph, 'marginBottom', scale, computed);
        setScaledPx(paragraph, 'paddingTop', scale, computed);
        setScaledPx(paragraph, 'paddingBottom', scale, computed);
    });
    block.querySelectorAll('.slide-prgrph > *').forEach(child => {
        const computed = getComputedStyle(child);
        setScaledPx(child, 'marginLeft', scale, computed);
        setScaledPx(child, 'marginRight', scale, computed);
        setScaledPx(child, 'paddingLeft', scale, computed);
        setScaledPx(child, 'paddingRight', scale, computed);
    });
};
const hasTextOverflow = (block) => block.scrollHeight > block.clientHeight + TEXT_FIT_TOLERANCE ||
    block.scrollWidth > block.clientWidth + TEXT_FIT_TOLERANCE;
const fitOverflowingTextBlock = (block) => {
    if (!block.querySelector('.text-block') || block.clientWidth <= 0 || block.clientHeight <= 0) {
        return;
    }
    let scale = Number(block.dataset.pptxTextFitScale) || 1;
    if (!hasTextOverflow(block)) {
        return;
    }
    for (let pass = 0; pass < TEXT_FIT_MAX_PASSES && hasTextOverflow(block); pass += 1) {
        const heightRatio = block.scrollHeight > 0
            ? Math.min(1, block.clientHeight / block.scrollHeight)
            : 1;
        const widthRatio = block.scrollWidth > 0
            ? Math.min(1, block.clientWidth / block.scrollWidth)
            : 1;
        const ratio = Math.min(heightRatio, widthRatio, 0.98);
        const nextScale = Math.max(TEXT_FIT_MIN_SCALE, scale * Math.max(ratio, 0.95));
        if (nextScale >= scale - 0.002) {
            scale = Math.max(TEXT_FIT_MIN_SCALE, scale * 0.97);
        }
        else {
            scale = nextScale;
        }
        applyTextFitScale(block, scale);
        if (scale <= TEXT_FIT_MIN_SCALE && hasTextOverflow(block)) {
            break;
        }
    }
};
const fitOverflowingTextBlocks = (root) => {
    root
        .querySelectorAll('.slide div.content, .slide div.content-rtl')
        .forEach(fitOverflowingTextBlock);
};
const renderChart = async (message, root) => {
    var _a;
    const payload = message.data;
    if (!(payload === null || payload === void 0 ? void 0 : payload.chartID) || !payload.chartType || !payload.chartData) {
        return;
    }
    const chartTarget = findPptxChartTarget(root, payload.chartID);
    if (!chartTarget) {
        return;
    }
    const billboard = await import('billboard.js');
    const d3Format = await import('d3-format');
    const bb = billboard.default || billboard;
    const { area, bar, line, pie, scatter } = billboard;
    const chart = {
        // A selector makes Billboard query the main document. That misses chart
        // placeholders inside the viewer Shadow DOM and makes it fall back to body.
        bindto: chartTarget,
    };
    const chartData = payload.chartData;
    const axis = {
        x: {
            tick: {
                format(index) {
                    var _a, _b;
                    return ((_b = (_a = chartData[0]) === null || _a === void 0 ? void 0 : _a.xlabels) === null || _b === void 0 ? void 0 : _b[index]) || index;
                },
            },
        },
    };
    switch (payload.chartType) {
        case 'lineChart':
            Object.assign(chart, {
                data: {
                    columns: chartData.map((item) => [item.key, ...item.values.map(({ y }) => y)]),
                    type: line(),
                },
                axis,
                interaction: { enabled: true },
            });
            break;
        case 'barChart':
            Object.assign(chart, {
                data: {
                    columns: chartData.map((item) => [item.key, ...item.values.map(({ y }) => y)]),
                    type: bar(),
                },
                axis: {
                    x: {
                        tick: {
                            multiline: true,
                            format(index) {
                                var _a, _b;
                                return ((_b = (_a = chartData[0]) === null || _a === void 0 ? void 0 : _a.xlabels) === null || _b === void 0 ? void 0 : _b[index]) || index;
                            },
                        },
                    },
                },
            });
            break;
        case 'pieChart':
        case 'pie3DChart':
            Object.assign(chart, {
                data: {
                    columns: Object.values(((_a = chartData[0]) === null || _a === void 0 ? void 0 : _a.xlabels) || {}).map((value, index) => {
                        var _a, _b, _c;
                        return [
                            value,
                            (_c = (_b = (_a = chartData[0]) === null || _a === void 0 ? void 0 : _a.values) === null || _b === void 0 ? void 0 : _b[index]) === null || _c === void 0 ? void 0 : _c.y,
                        ];
                    }),
                    type: pie(),
                },
            });
            break;
        case 'areaChart':
            Object.assign(chart, {
                data: {
                    columns: chartData.map((item) => [item.key, ...item.values.map(({ y }) => y)]),
                    type: area(),
                },
                axis,
                interaction: { enabled: true },
            });
            break;
        case 'scatterChart':
            Object.assign(chart, {
                data: {
                    xs: { y: 'x' },
                    columns: chartData.map((item, index) => [index ? 'y' : 'x', ...item]),
                    type: scatter(),
                },
                axis: {
                    x: {
                        label: 'X',
                        showDist: true,
                        tick: {
                            format: d3Format.format('.02f'),
                        },
                    },
                    y: {
                        label: 'Y',
                        showDist: true,
                        tick: {
                            format: d3Format.format('.02f'),
                        },
                    },
                },
            });
            break;
        default:
            return;
    }
    if (chart.data) {
        return bb.generate(chart);
    }
};
export const renderPptxPostProcessing = async (charts, root) => {
    restoreNumericBullets(root);
    fitOverflowingTextBlocks(root);
    const queue = asChartQueue(charts);
    const chartInstances = [];
    if (queue.length) {
        const results = await Promise.allSettled(queue.map(message => renderChart(message, root)));
        for (const result of results) {
            if (result.status === 'fulfilled') {
                if (result.value) {
                    chartInstances.push(result.value);
                }
            }
            else {
                console.warn('PPTX chart rendering skipped:', result.reason);
            }
        }
    }
    let destroyed = false;
    return {
        destroy() {
            var _a;
            if (destroyed) {
                return;
            }
            destroyed = true;
            for (const chart of chartInstances) {
                try {
                    (_a = chart.destroy) === null || _a === void 0 ? void 0 : _a.call(chart);
                }
                catch {
                    // The DOM may already be detached during framework unmount.
                }
            }
            chartInstances.length = 0;
        },
    };
};
