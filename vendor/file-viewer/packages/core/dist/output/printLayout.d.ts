export interface PrintPageSize {
    width: number;
    height: number;
}
export interface ApplyPrintPageSizeOptions {
    heightMode?: 'fixed' | 'min';
}
export interface BuildPrintPageStyleOptions extends PrintPageSize {
    selector: string;
    heightMode?: 'fixed' | 'min';
}
export declare const formatCssPixels: (value: number) => string;
/**
 * Browser print helper. DOM measurement belongs in the core browser layer
 * because it depends on CSS layout and HTMLElement.
 */
export declare const getElementPrintPageSize: (element: HTMLElement, fallback?: Partial<PrintPageSize>) => PrintPageSize;
export declare const applyPrintPageSize: (element: HTMLElement, size: PrintPageSize, options?: ApplyPrintPageSizeOptions) => void;
export declare const buildPrintPageStyle: ({ selector, width, height, heightMode, }: BuildPrintPageStyleOptions) => string;
