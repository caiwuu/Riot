export const RECOMMENDED_ZIP_LIMITS = {
    maxFileBytes: 160 * 1024 * 1024,
};
export const createDefaultPptxOptions = () => ({
    slidesScale: '',
    slideMode: false,
    slideType: 'divs2slidesjs',
    revealjsPath: '',
    keyBoardShortCut: false,
    mediaProcess: true,
    jsZipV2: false,
    themeProcess: true,
    incSlide: {
        width: 0,
        height: 0,
    },
    slideModeConfig: {
        first: 1,
        nav: true,
        navTxtColor: 'black',
        keyBoardShortCut: true,
        showSlideNum: true,
        showTotalSlideNum: true,
        autoSlide: true,
        randomAutoSlide: false,
        loop: false,
        background: false,
        transition: 'default',
        transitionTime: 1,
    },
    revealjsConfig: {},
});
export const resolvePptxEngineOptions = (options) => {
    var _a, _b, _c, _d, _e, _f, _g, _h;
    const defaults = createDefaultPptxOptions();
    return {
        ...defaults,
        ...options,
        incSlide: {
            width: (_d = (_b = (_a = options === null || options === void 0 ? void 0 : options.incSlide) === null || _a === void 0 ? void 0 : _a.width) !== null && _b !== void 0 ? _b : (_c = defaults.incSlide) === null || _c === void 0 ? void 0 : _c.width) !== null && _d !== void 0 ? _d : 0,
            height: (_h = (_f = (_e = options === null || options === void 0 ? void 0 : options.incSlide) === null || _e === void 0 ? void 0 : _e.height) !== null && _f !== void 0 ? _f : (_g = defaults.incSlide) === null || _g === void 0 ? void 0 : _g.height) !== null && _h !== void 0 ? _h : 0,
        },
        slideModeConfig: {
            ...defaults.slideModeConfig,
            ...options === null || options === void 0 ? void 0 : options.slideModeConfig,
        },
        revealjsConfig: {
            ...defaults.revealjsConfig,
            ...options === null || options === void 0 ? void 0 : options.revealjsConfig,
        },
    };
};
