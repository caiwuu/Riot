import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";

import { type DirBrowse, browseDirs, describeError, host, pickDirectory, renderUiError } from "../bridge";
import { useT } from "../i18n";
import { Modal } from "./Modal";

/**
 * 选一个宿主机上的目录。
 *
 * 桌面走系统对话框（`pickDirectory`）。浏览器里没有宿主机的对话框 ——
 * 手机上的文件选择器选的是手机的文件，而项目目录在跑 Riot 的那台机器上；
 * 所以网页版用一个应用内的目录浏览器（底层 `browseDirs`）代替。
 *
 * 用法：`const dir = useDirectoryPicker(); … await dir.pick(defaultPath)`，
 * 并把 `dir.element` 渲染进树里（桌面上永远是 null）。
 */
export function useDirectoryPicker(): {
  pick: (defaultPath?: string) => Promise<string | null>;
  element: ReactNode;
} {
  type Req = { start: string | undefined; resolve: (v: string | null) => void };
  const [req, setReq] = useState<Req | null>(null);
  // 和 state 同一份，给 pick / 卸载这两个不在渲染里的地方用：卸载后的
  // setState 更新函数不一定会跑，靠它了结 Promise 不可靠。
  const reqRef = useRef<Req | null>(null);

  const settle = useCallback((next: Req | null) => {
    // 上一个还没选完就被顶掉 / 组件卸载了：按"取消"了结，别让它的
    // Promise 挂着永远不回 —— 调用方多半在 await 它，那一处就卡死了。
    reqRef.current?.resolve(null);
    reqRef.current = next;
    setReq(next);
  }, []);

  const pick = useCallback(
    (defaultPath?: string) => {
      if (host.nativePaths) return pickDirectory(defaultPath);
      return new Promise<string | null>((resolve) => {
        settle({ start: defaultPath, resolve });
      });
    },
    [settle],
  );

  useEffect(() => () => settle(null), [settle]);

  const element = req ? (
    <DirPickerDialog
      start={req.start}
      onDone={(v) => {
        req.resolve(v);
        reqRef.current = null;
        setReq(null);
      }}
    />
  ) : null;

  return { pick, element };
}

function DirPickerDialog({
  start,
  onDone,
}: {
  start: string | undefined;
  onDone: (path: string | null) => void;
}) {
  const { t, tx } = useT();
  const [view, setView] = useState<DirBrowse | null>(null);
  const [typed, setTyped] = useState("");
  const [loading, setLoading] = useState(false);
  const seq = useRef(0);

  const go = useCallback((path: string | null | undefined) => {
    const my = ++seq.current;
    setLoading(true);
    browseDirs(path ?? null)
      .then((v) => {
        if (my !== seq.current) return;
        setView(v);
        setTyped(v.path);
      })
      .catch((e: unknown) => {
        if (my !== seq.current) return;
        setView((prev) => ({
          path: prev?.path ?? "",
          parent: prev?.parent ?? null,
          entries: [],
          error: { key: "host.legacy", detail: describeError(e) },
          missing: null,
        }));
      })
      .finally(() => {
        if (my === seq.current) setLoading(false);
      });
  }, []);

  useEffect(() => go(start), [go, start]);

  const submitTyped = () => {
    const p = typed.trim();
    if (p && p !== view?.path) go(p);
  };

  return (
    <Modal className="dir-picker" label={t("app.dirPicker.label")} portal onClose={() => onDone(null)}>
      <div className="modal-head">
        <h3>{t("app.dirPicker.title")}</h3>
      </div>
      <div className="dir-picker-path">
        <button
          type="button"
          className="btn-compact"
          disabled={!view?.parent || loading}
          onClick={() => go(view?.parent)}
          title={t("app.dirPicker.up")}
          aria-label={t("app.dirPicker.up")}
        >
          ↑
        </button>
        <input
          value={typed}
          onChange={(e) => setTyped(e.target.value)}
          onBlur={submitTyped}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              submitTyped();
            }
          }}
          spellCheck={false}
          autoComplete="off"
          placeholder={t("app.dirPicker.placeholder")}
        />
      </div>
      <div className="dir-picker-list" role="listbox" aria-busy={loading}>
        {view?.missing ? (
          <p className="test-result err">
            {tx("app.dirPicker.missing", {
              missing: <code className="path">{view.missing}</code>,
              path: view.path,
            })}
          </p>
        ) : null}
        {view?.error ? <p className="test-result err">{renderUiError(view.error)}</p> : null}
        {view && !view.error && view.entries.length === 0 ? (
          <p className="hint">{t("app.dirPicker.empty")}</p>
        ) : null}
        {view?.entries.map((e) => (
          <button
            key={e.path}
            type="button"
            className="dir-picker-entry"
            role="option"
            aria-selected={false}
            onClick={() => go(e.path)}
            onDoubleClick={() => onDone(e.path)}
            title={e.path}
          >
            <span className="dir-picker-icon" aria-hidden>
              ▸
            </span>
            {e.name}
          </button>
        ))}
      </div>
      <div className="modal-actions">
        <button type="button" onClick={() => onDone(null)}>
          {t("common.cancel")}
        </button>
        <button
          type="button"
          className="primary"
          disabled={!view || !!view.error || loading}
          onClick={() => view && onDone(view.path)}
        >
          {t("app.dirPicker.choose")}
        </button>
      </div>
    </Modal>
  );
}
