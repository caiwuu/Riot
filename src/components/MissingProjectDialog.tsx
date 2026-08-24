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
  return (
    <Modal className="confirm missing-project" label="找不到项目目录" alert onClose={onClose}>
      <div className="confirm-body">
        <h3>找不到项目目录</h3>
        <p>这个目录已经不在磁盘上了。会话必须绑一个还在的工作区。</p>
        <div className="missing-project-path" title={root}>
          {root}
        </div>
      </div>
      <div className="modal-actions">
        <button autoFocus onClick={onClose}>
          取消
        </button>
        <span className="modal-actions-spacer" />
        <button
          onClick={() => {
            onClose();
            onRelocate();
          }}
        >
          另选目录
        </button>
        <button
          className="btn-danger"
          onClick={() => {
            onClose();
            onRemove();
          }}
        >
          从列表移除
        </button>
      </div>
    </Modal>
  );
}
