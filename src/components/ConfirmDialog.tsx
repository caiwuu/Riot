import type { ReactNode } from "react";

import { useT } from "../i18n";
import { Modal } from "./Modal";

/** 破坏性操作的确认内容。 */
export interface ConfirmRequest {
  title: string;
  /** 接受节点而不只是字符串 —— 调用方已经在往里塞代码片段。 */
  body: ReactNode;
  confirmLabel: string;
  action: () => void;
  /** 默认 true。非破坏性确认（「知道了」）设 false，用主按钮而不是红色。 */
  danger?: boolean;
  /** 取消 / Esc / 点遮罩。确认走 `action`，不会进这里。 */
  onCancel?: () => void;
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
  portal,
  onClose,
}: {
  c: ConfirmRequest;
  /** 住在开合面板里时必须置真。理由见 [`Modal`] 的同名 prop。 */
  portal?: boolean;
  onClose: () => void;
}) {
  const { t } = useT();
  const cancel = () => {
    const fn = c.onCancel;
    onClose();
    fn?.();
  };
  return (
    <Modal className="confirm" label={c.title} alert onClose={cancel} portal={!!portal}>
      <div className="confirm-body">
        <h3>{c.title}</h3>
        <p>{c.body}</p>
      </div>
      <div className="modal-actions">
        <button autoFocus onClick={cancel}>
          {t("common.cancel")}
        </button>
        <button
          className={c.danger === false ? "primary" : "btn-danger"}
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
