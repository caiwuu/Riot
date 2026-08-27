export type PptxPostProcessingHandle = {
    destroy: () => void;
};
export declare const findPptxChartTarget: (root: ParentNode, chartID: string) => HTMLElement | null;
export declare const renderPptxPostProcessing: (charts: unknown, root: ParentNode) => Promise<PptxPostProcessingHandle>;
