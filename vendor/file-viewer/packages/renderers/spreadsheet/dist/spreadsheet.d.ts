import { type FileRenderContext, type FileViewerRenderedInstance as AppWrapper } from '@file-viewer/core';
type EVirtTableInstance = {
    ctx: {
        body: {
            headIndex?: number;
            tailIndex?: number;
        };
        scrollX?: number;
        scrollY?: number;
        containerElement: HTMLElement;
        isTarget(event: Event): boolean;
        selector?: {
            xArr: number[];
            yArr: number[];
            xArrCopy: number[];
            yArrCopy: number[];
        };
        emit?(type: string, ...args: unknown[]): void;
    };
    on(type: string, handler: (...args: any[]) => void): void;
    loadConfig(config: unknown): void;
    loadColumns(columns: unknown[]): void;
    loadData(rows: unknown[]): void;
    setCustomHeader?(customHeader: unknown, ignoreEmit?: boolean): void;
    draw(): void;
    doLayout(): void;
    scrollTo(x: number, y: number): void;
    destroy(): void;
};
type EVirtTableConstructor = new (container: HTMLElement, options: {
    data: unknown[];
    columns: unknown[];
    config: unknown;
}) => EVirtTableInstance;
export declare const enableEVirtTableShadowEventTargeting: (context: Pick<EVirtTableInstance["ctx"], "containerElement" | "isTarget">) => void;
export declare const resolveEVirtTableConstructor: (module: unknown) => EVirtTableConstructor;
export declare const resolveEVirtTableStyleText: (documentRef: Document) => string;
export declare const scopeEVirtTableStyleText: (cssText: string, shadow: boolean) => string;
declare const renderFileViewerSpreadsheet: (buffer: ArrayBuffer, target: HTMLDivElement, type?: string, context?: FileRenderContext) => Promise<AppWrapper>;
export default renderFileViewerSpreadsheet;
