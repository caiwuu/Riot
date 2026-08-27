const clamp = (value, min = 0, max = 1) => Math.min(max, Math.max(min, value));
const finitePositive = (value) => Number.isFinite(value) && Number(value) > 0;
export const normalizePdfBoundingBoxInput = (input) => {
    const values = input ? (Array.isArray(input) ? input : [input]) : [];
    return values.filter(value => !!value && typeof value === 'object');
};
export const normalizePdfBoundingBox = (input, pageBox, fallbackPage = 1) => {
    if (!Number.isFinite(input.x) ||
        !Number.isFinite(input.y) ||
        !finitePositive(input.width) ||
        !finitePositive(input.height) ||
        !finitePositive(pageBox.width) ||
        !finitePositive(pageBox.height)) {
        return null;
    }
    const unit = input.unit || 'pdf-point';
    let x = input.x;
    let y = input.y;
    let width = input.width;
    let height = input.height;
    if (unit === 'percent') {
        x /= 100;
        y /= 100;
        width /= 100;
        height /= 100;
    }
    else if (unit === 'pixel') {
        if (!finitePositive(input.sourceWidth) || !finitePositive(input.sourceHeight)) {
            return null;
        }
        x /= Number(input.sourceWidth);
        y /= Number(input.sourceHeight);
        width /= Number(input.sourceWidth);
        height /= Number(input.sourceHeight);
    }
    else if (unit === 'pdf-point') {
        x = (x - pageBox.x) / pageBox.width;
        y = (y - pageBox.y) / pageBox.height;
        width /= pageBox.width;
        height /= pageBox.height;
    }
    const origin = input.origin || (unit === 'pdf-point' ? 'bottom-left' : 'top-left');
    if (origin === 'bottom-left') {
        y = 1 - y - height;
    }
    const left = clamp(x);
    const top = clamp(y);
    const right = clamp(x + width);
    const bottom = clamp(y + height);
    if (right <= left || bottom <= top) {
        return null;
    }
    return {
        id: input.id,
        page: Math.max(1, Math.round(Number(input.page) || fallbackPage || 1)),
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
        color: input.color,
        label: input.label,
    };
};
export const rotateNormalizedPdfBoundingBox = (box, rotation) => {
    const normalizedRotation = ((Math.round(rotation / 90) * 90) % 360 + 360) % 360;
    if (normalizedRotation === 90) {
        return {
            ...box,
            x: 1 - box.y - box.height,
            y: box.x,
            width: box.height,
            height: box.width,
        };
    }
    if (normalizedRotation === 180) {
        return {
            ...box,
            x: 1 - box.x - box.width,
            y: 1 - box.y - box.height,
        };
    }
    if (normalizedRotation === 270) {
        return {
            ...box,
            x: box.y,
            y: 1 - box.x - box.width,
            width: box.height,
            height: box.width,
        };
    }
    return { ...box };
};
export const serializePdfBoundingBoxes = (input) => JSON.stringify(normalizePdfBoundingBoxInput(input));
