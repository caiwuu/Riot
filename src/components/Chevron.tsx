/**
 * 展开/收起箭头。用方盒包 SVG，旋转中心就是几何中心 ——
 * 文字 ▸ 的字框上下不对称，转 90° 会看起来在绕圈。
 *
 * `down`：下拉菜单用，默认朝下，打开再翻朝上。
 */
export function Chevron({ open, down }: { open?: boolean; down?: boolean }) {
  const cls = ["chevron", down ? "down" : "", open ? "open" : ""].filter(Boolean).join(" ");
  return (
    <span className={cls} aria-hidden>
      <svg width="12" height="12" viewBox="0 0 16 16" fill="none">
        <path
          d="M6 3.5L10.5 8L6 12.5"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    </span>
  );
}
