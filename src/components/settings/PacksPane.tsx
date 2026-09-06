import { useCallback, useEffect, useState } from "react";

import {
  type PackProgress,
  type PackStatus,
  packsStatus,
  packsUninstall,
  renderUiError,
  renderUiText,
} from "../../bridge";
import {
  clearDonePackProgress,
  clearPackProgress,
  reportPackFailure,
  startPackInstall,
  usePackInstalls,
} from "../../hooks/usePackInstalls";
import { useT } from "../../i18n";
import { Card, CardBlock, Group } from "./layout";
import type { AskConfirm } from "./shared";

type Translate = ReturnType<typeof useT>["t"];

/** 字节数写成人话。包是几百 MB 量级，一位小数够用。 */
function humanSize(bytes: number): string {
  if (bytes >= 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GB`;
  if (bytes >= 1024 * 1024) return `${Math.round(bytes / 1024 / 1024)} MB`;
  return `${Math.round(bytes / 1024)} KB`;
}

/**
 * 安装进度的一句话描述。下载有百分比，后面三步没有 —— 它们相对下载
 * 短得多，硬凑一个总进度只会让进度条在末尾诡异地卡住。
 */
function progressText(p: PackProgress, t: Translate): string {
  switch (p.kind) {
    case "downloading":
      return p.total > 0
        ? t("settings.packs.progress.downloading", {
            received: humanSize(p.received),
            total: humanSize(p.total),
          })
        : t("settings.packs.progress.downloadingNoTotal", { received: humanSize(p.received) });
    case "verifying":
      return t("settings.packs.progress.verifying");
    case "extracting":
      return t("settings.packs.progress.extracting");
    case "selfCheck":
      return t("settings.packs.progress.selfCheck");
    case "done":
      return t("common.done");
    case "failed":
      return renderUiError(p.error);
  }
}

export function PacksPane({ askConfirm }: { askConfirm: AskConfirm }) {
  const { t } = useT();
  const [packs, setPacks] = useState<PackStatus[] | null>(null);
  const [loadError, setLoadError] = useState("");
  /** 安装的进度和"正在装"标记在模块级 —— 关掉设置面板不该把它们连同组件一起丢掉。 */
  const installs = usePackInstalls();
  /** 卸载只是删本地目录，秒回，不值得也挪出去。 */
  const [uninstalling, setUninstalling] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setPacks(await packsStatus());
      setLoadError("");
    } catch (e) {
      setPacks(null);
      setLoadError(String(e));
    }
  }, []);

  // completed 变了 = 有安装刚跑完，清单得重拉。它可能是在面板关着的时候
  // 完成的，所以这里既管首次挂载，也管"装完了但没人看着"。
  useEffect(() => {
    void refresh().then(clearDonePackProgress);
  }, [refresh, installs.completed]);

  const uninstall = (p: PackStatus) => {
    askConfirm({
      title: t("settings.packs.uninstall.title", { name: renderUiText(p.name) }),
      body: t("settings.packs.uninstall.body"),
      confirmLabel: t("settings.packs.uninstall"),
      action: () => {
        setUninstalling(p.id);
        void (async () => {
          try {
            await packsUninstall(p.id);
            clearPackProgress(p.id);
            await refresh();
          } catch (e) {
            reportPackFailure(p.id, e);
          } finally {
            setUninstalling(null);
          }
        })();
      },
    });
  };

  return (
    <Group title={t("settings.packs.available")} desc={t("settings.packs.available.desc")}>
      {loadError ? (
        <div className="empty-state">
          <p className="form-error" style={{ margin: 0 }}>
            {t("settings.ext.loadFailed", { error: loadError })}
          </p>
          <button onClick={() => void refresh()}>{t("common.retry")}</button>
        </div>
      ) : packs === null ? (
        <Card>
          <CardBlock>
            <p className="hint" style={{ margin: 0 }}>
              {t("settings.ext.loading")}
            </p>
          </CardBlock>
        </Card>
      ) : packs.length === 0 ? (
        <Card>
          <CardBlock>
            <p className="hint" style={{ margin: 0 }}>
              {t("settings.packs.empty")}
            </p>
          </CardBlock>
        </Card>
      ) : (
        <ul className="pack-list">
          {packs.map((p) => {
            const prog = installs.progress[p.id];
            const installing = Boolean(installs.running[p.id]);
            const busy = installing || uninstalling === p.id;
            const upgradable =
              p.installedVersion !== null &&
              p.availableVersion !== null &&
              p.installedVersion !== p.availableVersion;
            return (
              <li key={p.id} className="pack-item">
                <div className="pack-head">
                  <span className="pack-name">{renderUiText(p.name)}</span>
                  {p.installedVersion ? (
                    <span className="pack-badge on">
                      {t("settings.packs.installed", { version: p.installedVersion })}
                    </span>
                  ) : null}
                  {upgradable ? (
                    <span className="pack-badge">
                      {t("settings.packs.upgradable", { version: p.availableVersion ?? "" })}
                    </span>
                  ) : null}
                </div>
                <p className="hint" style={{ margin: "2px 0 0" }}>
                  {renderUiText(p.description)}
                </p>

                {!p.supported ? (
                  <p className="hint" style={{ margin: "6px 0 0" }}>
                    {t("settings.packs.unsupported")}
                  </p>
                ) : p.manifestError && !p.installedVersion ? (
                  <p className="form-error" style={{ margin: "6px 0 0" }}>
                    {t("settings.packs.manifestError", { error: renderUiError(p.manifestError) })}
                  </p>
                ) : !p.availableVersion && !p.installedVersion ? (
                  // 清单拉到了、但里面还没有这个包。不说话的话这一行就只剩名字和
                  // 描述、没有任何按钮，用户分不清是在加载、坏了、还是没发布。
                  <p className="hint" style={{ margin: "6px 0 0" }}>
                    {t("settings.packs.notReleased")}
                  </p>
                ) : null}

                {prog ? (
                  <div className="pack-progress">
                    {prog.kind === "downloading" && prog.total > 0 ? (
                      <div className="pack-bar">
                        <div
                          className="pack-bar-fill"
                          style={{ width: `${Math.round((prog.received / prog.total) * 100)}%` }}
                        />
                      </div>
                    ) : null}
                    <span className={prog.kind === "failed" ? "form-error" : "hint"}>
                      {progressText(prog, t)}
                    </span>
                  </div>
                ) : null}

                <div className="pack-actions">
                  {p.availableVersion && (!p.installedVersion || upgradable) ? (
                    <button
                      disabled={busy || !p.supported}
                      onClick={() => startPackInstall(p.id)}
                    >
                      {p.installedVersion ? t("settings.packs.upgrade") : t("settings.packs.install")}
                      {p.downloadSize > 0
                        ? t("settings.packs.size", { size: humanSize(p.downloadSize) })
                        : null}
                    </button>
                  ) : null}
                  {p.installedVersion ? (
                    <button className="ghost" disabled={busy} onClick={() => uninstall(p)}>
                      {t("settings.packs.uninstall")}
                    </button>
                  ) : null}
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </Group>
  );
}
