import { type FileViewerI18nInput } from '../i18n/messages';
import type { FileViewerStateTheme } from '../contracts/types';
export type FileViewerLoadingTheme = FileViewerStateTheme;
export interface FileViewerLoadingState {
    loading: boolean;
    error: string;
    message: string;
    theme: FileViewerLoadingTheme;
    styleVars: Record<'--viewer-accent' | '--viewer-soft', string>;
}
export type MutableFileViewerLoadingState = FileViewerLoadingState;
export interface FileViewerLoadingController {
    readonly state: FileViewerLoadingState;
    setExtension(nextExtend?: string): FileViewerLoadingState;
    setI18n(nextI18n?: FileViewerI18nInput): FileViewerLoadingState;
    startLoading(nextMessage: string): FileViewerLoadingState;
    setLoadingMessage(nextMessage: string): FileViewerLoadingState;
    stopLoading(): FileViewerLoadingState;
    showError(nextMessage: string): FileViewerLoadingState;
    clearError(): FileViewerLoadingState;
    resetLoading(): FileViewerLoadingState;
    getState(): FileViewerLoadingState;
}
export interface FileViewerLoadingControllerActionHandlers {
    setExtension(nextExtend?: string): FileViewerLoadingState;
    setI18n(nextI18n?: FileViewerI18nInput): FileViewerLoadingState;
    startLoading(nextMessage: string): FileViewerLoadingState;
    setLoadingMessage(nextMessage: string): FileViewerLoadingState;
    stopLoading(): FileViewerLoadingState;
    showError(nextMessage: string): FileViewerLoadingState;
    clearError(): FileViewerLoadingState;
    resetLoading(): FileViewerLoadingState;
    syncLoadingState(): FileViewerLoadingState;
}
export interface RunFileViewerLoadingExtensionSyncInput<Target extends MutableFileViewerLoadingState = MutableFileViewerLoadingState> {
    target: Target;
    controller: Pick<FileViewerLoadingController, 'setExtension'>;
    extension?: string;
}
export declare const FALLBACK_FILE_VIEWER_LOADING_THEME: FileViewerLoadingTheme;
export declare const FILE_VIEWER_LOADING_THEME_MAP: Record<string, FileViewerLoadingTheme>;
export declare const resolveFileViewerLoadingTheme: (extend?: string, i18n?: FileViewerI18nInput) => FileViewerLoadingTheme;
export declare const createFileViewerLoadingStyleVars: (theme: FileViewerLoadingTheme) => {
    '--viewer-accent': string;
    '--viewer-soft': string;
};
export declare const createFileViewerLoadingState: (extend?: string, i18n?: FileViewerI18nInput) => FileViewerLoadingState;
export declare const cloneFileViewerLoadingState: (state: FileViewerLoadingState) => FileViewerLoadingState;
export declare const applyFileViewerLoadingState: <Target extends MutableFileViewerLoadingState>(target: Target, source: FileViewerLoadingState) => Target;
export declare const syncFileViewerLoadingControllerState: <Target extends MutableFileViewerLoadingState>(target: Target, controller: Pick<FileViewerLoadingController, "getState">, source?: FileViewerLoadingState) => Target;
export declare const runFileViewerLoadingControllerAction: <Target extends MutableFileViewerLoadingState>(target: Target, action: () => FileViewerLoadingState) => Target;
export declare const runFileViewerLoadingExtensionSync: <Target extends MutableFileViewerLoadingState>({ target, controller, extension, }: RunFileViewerLoadingExtensionSyncInput<Target>) => Target;
export declare const createFileViewerLoadingControllerActionHandlers: <Target extends MutableFileViewerLoadingState>(target: Target, controller: FileViewerLoadingController) => FileViewerLoadingControllerActionHandlers;
/**
 * 统一管理加载、错误、文案和主题色。
 * wrapper 只负责把这个加载状态映射到各自框架的响应式系统。
 */
export declare const createFileViewerLoadingController: (extend?: string, initialI18n?: FileViewerI18nInput) => FileViewerLoadingController;
