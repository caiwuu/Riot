import { useEffect } from "react";

/** 破坏性操作的确认内容。 */
export interface ConfirmRequest {
  title: string;
  body: string;
  confirmLabel: string;
  action: () => void;
}

/**
 * 破坏性操作的确认。默认焦点在"取消"，Esc / 点遮罩也是取消 ——
 * 一个习惯性的回车不应该完成一次删除。
 */
export function ConfirmDialog({
  c,
  onClose,
}: {
  c: ConfirmRequest;
  onClose: () => void;
}) {
  useEffect(() => {
    const esc = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  }, [onClose]);

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal confirm">
        <div className="confirm-body">
          <h3>{c.title}</h3>
          <p>{c.body}</p>
        </div>
        <div className="modal-actions">
          <button autoFocus onClick={onClose}>
            取消
          </button>
          <button
            className="btn-danger"
            onClick={() => {
              onClose();
              c.action();
            }}
          >
            {c.confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
