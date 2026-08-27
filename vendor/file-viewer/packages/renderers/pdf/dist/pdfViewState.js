export const normalizePdfRotation = (rotation) => {
    const normalized = (((Math.round(rotation / 90) * 90) % 360) + 360) % 360;
    return (normalized === 90 || normalized === 180 || normalized === 270 ? normalized : 0);
};
export const clampPdfScale = (scale, minScale, maxScale) => {
    return Number(Math.min(maxScale, Math.max(minScale, scale)).toFixed(2));
};
export const resolvePdfViewStateUpdate = (state, current, limits) => {
    var _a, _b;
    const requestedRotation = Number(state.rotation);
    const requestedScale = Number((_a = state.scale) !== null && _a !== void 0 ? _a : (_b = state.zoom) === null || _b === void 0 ? void 0 : _b.scale);
    const requestedPage = Number(state.page);
    const rotation = Number.isFinite(requestedRotation)
        ? normalizePdfRotation(requestedRotation)
        : current.rotation;
    const scale = Number.isFinite(requestedScale)
        ? clampPdfScale(requestedScale, limits.minScale, limits.maxScale)
        : current.scale;
    const page = Number.isFinite(requestedPage)
        ? Math.min(current.pageCount, Math.max(1, Math.round(requestedPage)))
        : current.page;
    return {
        rotation: rotation !== current.rotation ? rotation : undefined,
        scale: Math.abs(scale - current.scale) > 0.001 ? scale : undefined,
        page: page !== current.page ? page : undefined
    };
};
