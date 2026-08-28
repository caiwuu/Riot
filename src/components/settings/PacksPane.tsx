import { useCallback, useEffect, useState } from "react";

import {
  type PackProgress,
  type PackStatus,
  packsStatus,
  packsUninstall,
} from "../../bridge";
import {
  clearDonePackProgress,
  clearPackProgress,
  reportPackFailure,
  startPackInstall,
  usePackInstalls,
} from "../../hooks/usePackInstalls";
import { Card, CardBlock, Group } from "./layout";
import type { AskConfirm } from "./shared";

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
function progressText(p: PackProgress): string {
  switch (p.kind) {
    case "downloading":
      return p.total > 0
        ? `下载中 ${humanSize(p.received)} / ${humanSize(p.total)}`
        : `下载中 ${humanSize(p.received)}`;
    case "verifying":
      return "校验中…";
    case "extracting":
      return "解压中…";
    case "selfCheck":
      return "自检中…";
    case "done":
      return "完成";
    case "failed":
      return p.error;
  }
}

export function PacksPane({ askConfirm }: { askConfirm: AskConfirm }) {
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
      title: `卸载「${p.name}」？`,
      body: "会删掉整个包，连带摘掉它注册的 MCP 服务器和技能。可以随时重新下载。",
      confirmLabel: "卸载",
      action: () => {
        setUninstalling(p.id);
        void (async () => {
          try {
            await packsUninstall(p.id);
            clearPackProgress(p.id);
            await refresh();
          } catch (e) {
            reportPackFailure(p.id, String(e));
          } finally {
            setUninstalling(null);
          }
        })();
      },
    });
  };

  return (
    <Group
      title="可下载的包"
      desc="装上之后模型自己会用 —— 相关技能和工具自动注册，不用在别处再配一遍。包体较大，建议在网络稳定时装；装的过程中可以关掉设置去干别的，回来还能看到进度。下载中断可以重来，已下好的部分会接着传。"
    >
      {loadError ? (
        <div className="empty-state">
          <p className="form-error" style={{ margin: 0 }}>
            读取失败：{loadError}
          </p>
          <button onClick={() => void refresh()}>重试</button>
        </div>
      ) : packs === null ? (
        <Card>
          <CardBlock>
            <p className="hint" style={{ margin: 0 }}>
              读取中…
            </p>
          </CardBlock>
        </Card>
      ) : packs.length === 0 ? (
        <Card>
          <CardBlock>
            <p className="hint" style={{ margin: 0 }}>
              当前没有可用的能力包。
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
                  <span className="pack-name">{p.name}</span>
                  {p.installedVersion ? (
                    <span className="pack-badge on">已装 {p.installedVersion}</span>
                  ) : null}
                  {upgradable ? (
                    <span className="pack-badge">可升级到 {p.availableVersion}</span>
                  ) : null}
                </div>
                <p className="hint" style={{ margin: "2px 0 0" }}>
                  {p.description}
                </p>

                {!p.supported ? (
                  <p className="hint" style={{ margin: "6px 0 0" }}>
                    这个包没有适配当前系统的版本。
                  </p>
                ) : p.manifestError && !p.installedVersion ? (
                  <p className="form-error" style={{ margin: "6px 0 0" }}>
                    拉不到清单：{p.manifestError}
                  </p>
                ) : !p.availableVersion && !p.installedVersion ? (
                  // 清单拉到了、但里面还没有这个包。不说话的话这一行就只剩名字和
                  // 描述、没有任何按钮，用户分不清是在加载、坏了、还是没发布。
                  <p className="hint" style={{ margin: "6px 0 0" }}>
                    还没有发布可下载的版本。
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
                      {progressText(prog)}
                    </span>
                  </div>
                ) : null}

                <div className="pack-actions">
                  {p.availableVersion && (!p.installedVersion || upgradable) ? (
                    <button
                      disabled={busy || !p.supported}
                      onClick={() => startPackInstall(p.id)}
                    >
                      {p.installedVersion ? "升级" : "下载安装"}
                      {p.downloadSize > 0 ? `（${humanSize(p.downloadSize)}）` : null}
                    </button>
                  ) : null}
                  {p.installedVersion ? (
                    <button className="ghost" disabled={busy} onClick={() => uninstall(p)}>
                      卸载
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
