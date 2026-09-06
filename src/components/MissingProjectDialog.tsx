import { useT } from "../i18n";
import { Modal } from "./Modal";

/**
 * 项目目录被删掉之后的恢复框。
 *
 * 对照 VS Code / JetBrains：最近项目还在列表里，点开时问「从列表移除」
 * 还是「另选目录」，不把整窗换成启动失败页。取消是默认焦点 —— 回车
 * 不该顺手把项目拿掉。
 */
export function MissingProjectDialog({
  root,
  onClose,
  onRemove,
  onRelocate,
}: {
  root: string;
  onClose: () => void;
  onRemove: () => void;
  onRelocate: () => void;
}) {
  const { t } = useT();
  return (
    <Modal
      className="confirm missing-project"
      label={t("app.missingProject.title")}
      alert
      onClose={onClose}
    >
      <div className="confirm-body">
        <h3>{t("app.missingProject.title")}</h3>
        <p>{t("app.missingProject.body")}</p>
        <div className="missing-project-path" title={root}>
          {root}
        </div>
      </div>
      <div className="modal-actions">
        <button autoFocus onClick={onClose}>
          {t("common.cancel")}
        </button>
        <span className="modal-actions-spacer" />
        <button
          onClick={() => {
            onClose();
            onRelocate();
          }}
        >
          {t("app.missingProject.relocate")}
        </button>
        <button
          className="btn-danger"
          onClick={() => {
            onClose();
            onRemove();
          }}
        >
          {t("app.project.removeFromList")}
        </button>
      </div>
    </Modal>
  );
}
