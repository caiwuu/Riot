import { jsx as _jsx } from "react/jsx-runtime";
import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState } from 'react';
import { createViewerControllerHandle, mountViewer } from './controller.js';
import { fileViewerCoreRendererRegistry } from '@file-viewer/core';
const defaultStyle = {
    width: '100%',
    height: '100%',
    minHeight: 0
};
const viewerCoreOptions = {
    registry: fileViewerCoreRendererRegistry
};
const createInitialViewerState = () => ({
    loading: false,
    ready: false,
    error: null,
    lastEvent: null,
    lifecycle: null,
    availability: null,
    search: null,
    zoom: null,
    location: null,
    viewState: null
});
const destroyController = (controllerRef, container) => {
    var _a;
    (_a = controllerRef.current) === null || _a === void 0 ? void 0 : _a.destroy();
    controllerRef.current = null;
    if (container) {
        container.innerHTML = '';
    }
};
export const FileViewer = forwardRef((props, forwardedRef) => {
    const { url, file, buffer, name, filename, type, size, options, onEvent, onStateChange, style, ...containerProps } = props;
    const containerRef = useRef(null);
    const controllerRef = useRef(null);
    const appliedViewerOptionsRef = useRef(null);
    const viewerOptions = useMemo(() => ({
        url,
        file,
        buffer,
        name,
        filename,
        type,
        size,
        options,
        onEvent,
        onStateChange
    }), [url, file, buffer, name, filename, type, size, options, onEvent, onStateChange]);
    useEffect(() => {
        const container = containerRef.current;
        if (!container || controllerRef.current) {
            return undefined;
        }
        appliedViewerOptionsRef.current = viewerOptions;
        controllerRef.current = mountViewer(container, viewerOptions, viewerCoreOptions);
        return () => {
            appliedViewerOptionsRef.current = null;
            destroyController(controllerRef, container);
        };
    }, []);
    useEffect(() => {
        var _a;
        if (appliedViewerOptionsRef.current === viewerOptions) {
            return;
        }
        appliedViewerOptionsRef.current = viewerOptions;
        void ((_a = controllerRef.current) === null || _a === void 0 ? void 0 : _a.update(viewerOptions));
    }, [viewerOptions]);
    useImperativeHandle(forwardedRef, () => createViewerControllerHandle(() => controllerRef.current, () => destroyController(controllerRef, containerRef.current)), []);
    return (_jsx("div", { ...containerProps, ref: containerRef, style: { ...defaultStyle, ...style } }));
});
FileViewer.displayName = 'FileViewer';
export const useFileViewerState = (onEvent) => {
    const [state, setState] = useState(() => createInitialViewerState());
    const onStateChange = useCallback((nextState, event) => {
        setState(nextState);
        if (event) {
            onEvent === null || onEvent === void 0 ? void 0 : onEvent(event);
        }
    }, [onEvent]);
    const resetState = useCallback(() => {
        setState(createInitialViewerState());
    }, []);
    return {
        state,
        onStateChange,
        resetState
    };
};
export const useFileViewer = (options = {}) => {
    const ref = useRef(null);
    const { onEvent, ...viewerOptions } = options;
    const { state, onStateChange, resetState } = useFileViewerState(onEvent);
    const props = useMemo(() => ({
        ...viewerOptions,
        onStateChange
    }), [viewerOptions, onStateChange]);
    const handle = useMemo(() => createViewerControllerHandle(() => { var _a, _b; return (_b = (_a = ref.current) === null || _a === void 0 ? void 0 : _a.getController()) !== null && _b !== void 0 ? _b : null; }, () => { var _a; return (_a = ref.current) === null || _a === void 0 ? void 0 : _a.destroy(); }), []);
    return {
        ref,
        props,
        state,
        handle,
        resetState
    };
};
export default FileViewer;
