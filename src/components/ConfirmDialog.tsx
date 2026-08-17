import type { ReactNode } from "react";

import { Modal } from "./Modal";

/** 破坏性操作的确认内容。 */
export interface ConfirmRequest {
  title: string;
  /** 接受节点而不只是字符串 —— 调用方已经在往里塞代码片段。 */
  body: ReactNode;
  confirmLabel: string;
  action: () => void;
}

/**
 * 破坏性操作的确认。默认焦点在"取消"，Esc / 点遮罩也是取消 ——
 * 一个习惯性的回车不应该完成一次删除。
 *
 * dialog 语义、焦点陷阱、Esc 分层都在 [`Modal`] 里 —— 确认框是
 * 破坏性操作的最后防线，读屏用户尤其不能听不到它。
 */
export function ConfirmDialog({
  c,
  onClose,
}: {
  c: ConfirmRequest;
  onClose: () => void;
}) {
  return (
    <Modal className="confirm" label={c.title} alert onClose={onClose}>
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
    </Modal>
  );
}
