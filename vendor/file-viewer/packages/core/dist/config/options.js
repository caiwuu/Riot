const isRecord = (value) => {
    return !!value && typeof value === 'object' && !Array.isArray(value);
};
const isUrlLike = (value) => {
    return typeof URL !== 'undefined' && value instanceof URL;
};
const sanitizeJsonValue = (value) => {
    if (value === undefined || typeof value === 'function' || typeof value === 'symbol') {
        return undefined;
    }
    if (value === null || typeof value === 'string' || typeof value === 'boolean') {
        return value;
    }
    if (typeof value === 'number') {
        return Number.isFinite(value) ? value : undefined;
    }
    if (typeof value === 'bigint') {
        return value.toString();
    }
    if (value instanceof Date) {
        return value.toISOString();
    }
    if (isUrlLike(value)) {
        return value.toString();
    }
    if (Array.isArray(value)) {
        return value
            .map(item => sanitizeJsonValue(item))
            .filter((item) => item !== undefined);
    }
    if (!isRecord(value)) {
        return undefined;
    }
    const output = {};
    Object.entries(value).forEach(([key, nextValue]) => {
        const sanitized = sanitizeJsonValue(nextValue);
        if (sanitized !== undefined) {
            output[key] = sanitized;
        }
    });
    return Object.keys(output).length ? output : undefined;
};
const stripExecutionOnlyOptions = (value) => {
    const { beforeOperation: _beforeOperation, hooks: _hooks, preset: _preset, presets: _presets, renderers: _renderers, rendererMode: _rendererMode, ...rest } = value;
    if (isRecord(rest.toolbar)) {
        const { beforeOperation: _toolbarBeforeOperation, beforeDownload: _beforeDownload, beforePrint: _beforePrint, beforeExportHtml: _beforeExportHtml, ...toolbar } = rest.toolbar;
        rest.toolbar = toolbar;
    }
    return rest;
};
export const normalizeFileViewerTheme = (theme) => {
    return theme === 'light' || theme === 'dark' || theme === 'system' ? theme : 'system';
};
const readSystemDarkMode = () => {
    return typeof globalThis.matchMedia === 'function' &&
        globalThis.matchMedia('(prefers-color-scheme: dark)').matches;
};
export const resolveFileViewerColorScheme = (theme, systemDark = readSystemDarkMode()) => {
    const normalizedTheme = normalizeFileViewerTheme(theme);
    return normalizedTheme === 'system'
        ? (systemDark ? 'dark' : 'light')
        : normalizedTheme;
};
export const toggleFileViewerColorScheme = (theme, systemDark = readSystemDarkMode()) => {
    return resolveFileViewerColorScheme(theme, systemDark) === 'dark' ? 'light' : 'dark';
};
export const normalizeFileViewerUiDensity = (density) => {
    return density === 'compact' ? 'compact' : 'comfortable';
};
export const resolveFileViewerUiDensity = (options) => { var _a; return normalizeFileViewerUiDensity((_a = options === null || options === void 0 ? void 0 : options.ui) === null || _a === void 0 ? void 0 : _a.density); };
export const sanitizeFileViewerOptions = (options) => {
    if (!isRecord(options)) {
        return undefined;
    }
    const sanitized = sanitizeJsonValue(stripExecutionOnlyOptions(options));
    if (!isRecord(sanitized)) {
        return undefined;
    }
    return sanitized;
};
export const serializeFileViewerOptions = (options) => {
    const sanitized = sanitizeFileViewerOptions(options);
    return sanitized ? JSON.stringify(sanitized) : undefined;
};
export const parseFileViewerOptions = (value) => {
    if (!value) {
        return undefined;
    }
    if (typeof value === 'string') {
        try {
            return sanitizeFileViewerOptions(JSON.parse(value));
        }
        catch {
            return undefined;
        }
    }
    return sanitizeFileViewerOptions(value);
};
export const setFileViewerOptionsSearchParam = (searchParams, options, key = 'options') => {
    const serialized = serializeFileViewerOptions(options);
    if (!serialized) {
        searchParams.delete(key);
        return;
    }
    searchParams.set(key, serialized);
};
export const getFileViewerOptionsSearchParam = (searchParams, key = 'options') => {
    return parseFileViewerOptions(searchParams.get(key));
};
