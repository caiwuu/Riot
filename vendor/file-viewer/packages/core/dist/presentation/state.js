import { normalizeFileViewerToolbar } from '../lifecycle/operations.js';
import { normalizeFileViewerTheme, resolveFileViewerUiDensity } from '../config/options.js';
import { getExtension, resolveFileViewerSourceFilename } from '../source/index.js';
export const resolveFileViewerPresentationState = ({ filename, file, url, options, }) => {
    const displayFilename = resolveFileViewerSourceFilename({
        filename,
        file,
        url,
    });
    return {
        displayFilename,
        extension: getExtension(displayFilename),
        toolbar: normalizeFileViewerToolbar(options),
        theme: normalizeFileViewerTheme(options === null || options === void 0 ? void 0 : options.theme),
        density: resolveFileViewerUiDensity(options),
    };
};
