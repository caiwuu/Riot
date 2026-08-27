export const PDF_FIT_MIN_VIEWPORT_SIZE = 96;
/* Riot 定制：贴宽计算原本写死预留 28px 呼吸边 + 18px 页边框宽，
   预览抽屉里页面两侧因此永远空 23px。宿主已把页 border 和容器
   padding 全部压 0，这里的预留量同步归零，让 canvas 真正贴满。 */
export const PDF_FIT_HORIZONTAL_PADDING = 0;
export const PDF_PAGE_BORDER_WIDTH = 0;
const normalizePadding = (padding) => (Number.isFinite(padding) && Number(padding) > 0 ? Number(padding) : 0);
/**
 * Resolves the PDF page viewport after the navigation pane has been laid out.
 * Core request dimensions already exclude fit.padding; live container and
 * window fallback dimensions do not, so only those branches subtract it.
 */
export const resolvePdfFitViewportSize = ({ containerWidth, containerHeight, fallbackWidth, fallbackHeight, request, }) => {
    const padding = normalizePadding(request.padding);
    const requestWidth = Number(request.viewportWidth) || 0;
    const requestHeight = Number(request.viewportHeight) || 0;
    const hasContainerWidth = containerWidth > 0;
    const hasContainerHeight = containerHeight > 0;
    const width = hasContainerWidth
        ? containerWidth - padding * 2
        : requestWidth || fallbackWidth - padding * 2;
    const height = hasContainerHeight
        ? containerHeight - padding * 2
        : requestHeight || fallbackHeight - padding * 2;
    return {
        width: Math.max(PDF_FIT_MIN_VIEWPORT_SIZE, width - PDF_FIT_HORIZONTAL_PADDING - PDF_PAGE_BORDER_WIDTH),
        height: Math.max(PDF_FIT_MIN_VIEWPORT_SIZE, height - PDF_PAGE_BORDER_WIDTH),
    };
};
