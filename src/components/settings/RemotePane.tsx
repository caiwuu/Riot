import { useCallback, useEffect, useState } from "react";

import {
  type ConfigStatus,
  DEFAULT_REMOTE,
  type RemoteConfig,
  type RemoteStatus,
  host,
  remoteRotateToken,
  remoteSetToken,
  remoteStatus,
  renderUiError,
  setConfig,
} from "../../bridge";
import { useT } from "../../i18n";
import { FieldNumber } from "../FieldNumber";
import { FieldSelect } from "../FieldSelect";
import { Card, CardBlock, Group, Row } from "./layout";
import { type AskConfirm, FormError, Switch, blurOnEnter } from "./shared";

/**
 * 远程访问：在手机 / 别的电脑的浏览器里用同一个 Riot。
 *
 * 排布按用户的操作顺序：先开、再决定听哪、然后拿到链接 / 二维码去扫。
 * 令牌放最后，因为多数人不需要看到它 —— 扫码就够；只有换令牌和手动输入
 * 的时候才用得上。
 *
 * 开关一动就保存（和联网页同一套失焦提交），保存后立刻重查状态：宿主是
 * 在 `set_config` 里同步起停服务的，所以回来的状态就是最新的。
 */
export function RemotePane({
  status,
  onStatus,
  onSaved,
  askConfirm,
}: {
  status: ConfigStatus;
  onStatus: (s: ConfigStatus) => void;
  onSaved: () => void;
  askConfirm: AskConfirm;
}) {
  const { t, tn, tx } = useT();
  const remote = status.config.remote ?? DEFAULT_REMOTE;
  const [live, setLive] = useState<RemoteStatus | null>(null);
  const [port, setPort] = useState(String(remote.port));
  const [error, setError] = useState("");
  const [showToken, setShowToken] = useState(false);
  const [copied, setCopied] = useState<"url" | "token" | null>(null);

  useEffect(() => setPort(String(remote.port)), [remote.port]);

  const refresh = useCallback(() => {
    remoteStatus()
      .then(setLive)
      .catch((e: unknown) => setError(String(e)));
  }, []);

  // 连接数会变，开着的时候轮询一下；关着就没必要。
  useEffect(() => {
    refresh();
    if (!remote.enabled) return;
    const t = window.setInterval(refresh, 3000);
    return () => window.clearInterval(t);
  }, [refresh, remote.enabled]);

  const patch = async (p: Partial<RemoteConfig>) => {
    setError("");
    try {
      onStatus(await setConfig({ ...status.config, remote: { ...remote, ...p } }));
      onSaved();
      refresh();
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    }
  };

  const copy = async (text: string, what: "url" | "token") => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(what);
      window.setTimeout(() => setCopied(null), 1500);
    } catch {
      setError(t("settings.remote.copyFailed"));
    }
  };

  const rotate = () => {
    askConfirm({
      title: t("settings.remote.token.rotate.title"),
      body: t("settings.remote.token.rotate.body"),
      confirmLabel: t("settings.remote.token.rotate"),
      action: () => {
        remoteRotateToken()
          .then((token) => {
            // 从网页端换的令牌：这个页面存的还是旧的，下次重连就会被拒 ——
            // 等于把自己锁在门外。直接换成新的（会断开重连一次，几百毫秒）。
            if (host.kind === "web") remoteSetToken(token);
            onSaved();
            refresh();
          })
          .catch((e: unknown) => setError(String(e)));
      },
    });
  };

  const commitPort = () => {
    const n = Number(port.trim());
    if (!Number.isInteger(n) || n < 1 || n > 65535) {
      setError(t("settings.remote.port.range"));
      setPort(String(remote.port));
      return;
    }
    if (n === remote.port) return;
    void patch({ port: n }).then((ok) => {
      if (!ok) setPort(String(remote.port));
    });
  };

  const qrSrc = live?.qrSvg ? `data:image/svg+xml;utf8,${encodeURIComponent(live.qrSvg)}` : null;

  return (
    <>
      {host.kind === "web" ? (
        <Card>
          <CardBlock>
            <p className="hint" style={{ margin: 0 }}>
              {t("settings.remote.webNotice")}
            </p>
          </CardBlock>
        </Card>
      ) : null}

      <Group title={t("settings.remote.service")}>
        <Card>
          <Row title={t("settings.remote.enable")} desc={t("settings.remote.enable.desc")}>
            <Switch
              on={remote.enabled}
              onChange={(v) => void patch({ enabled: v })}
              label={t("settings.remote.enable")}
            />
          </Row>
          <Row title={t("settings.remote.bind")} desc={t("settings.remote.bind.desc")}>
            <FieldSelect
              value={remote.bind}
              onChange={(v) => void patch({ bind: v as RemoteConfig["bind"] })}
              options={[
                { value: "loopback", label: t("settings.remote.bind.loopback") },
                { value: "lan", label: t("settings.remote.bind.lan") },
              ]}
            />
          </Row>
          <Row title={t("settings.remote.port")} desc={t("settings.remote.port.desc")} htmlFor="remote-port">
            <span className="field-inline">
              <FieldNumber
                id="remote-port"
                value={port}
                onChange={(e) => setPort(e.target.value)}
                onBlur={commitPort}
                onKeyDown={blurOnEnter}
                aria-label={t("settings.remote.port")}
              />
            </span>
          </Row>
          {live ? (
            <CardBlock>
              {live.error ? (
                <p className="key-state warn" style={{ margin: 0 }}>
                  {t("settings.remote.status.error", { error: renderUiError(live.error) })}
                </p>
              ) : live.running ? (
                <p className="key-state ok" style={{ margin: 0 }}>
                  {tx("settings.remote.status.listening", { addr: <code>{live.listenAddr}</code> })}
                  {" · "}
                  {live.connections > 0
                    ? tn("settings.remote.status.connections", live.connections)
                    : t("settings.remote.status.noConnections")}
                </p>
              ) : (
                <p className="hint" style={{ margin: 0 }}>
                  {t("settings.remote.status.off")}
                </p>
              )}
            </CardBlock>
          ) : null}
        </Card>
      </Group>

      {live?.running ? (
        <Group title={t("settings.remote.connect")} desc={t("settings.remote.connect.desc")}>
          <Card>
            <div className="remote-connect">
              {qrSrc ? (
                <img
                  className="remote-qr"
                  src={qrSrc}
                  alt={t("settings.remote.qrAlt")}
                  width={180}
                  height={180}
                />
              ) : null}
              <div className="remote-links">
                {live.urls.map((u) => (
                  <code key={u} className="path">
                    {u}
                  </code>
                ))}
                {live.loginUrl ? (
                  <div className="input-with-btn">
                    <input readOnly value={live.loginUrl} onFocus={(e) => e.currentTarget.select()} />
                    <button
                      className="btn-compact"
                      onClick={() => void copy(live.loginUrl ?? "", "url")}
                    >
                      {copied === "url" ? t("common.copied") : t("settings.remote.copyLink")}
                    </button>
                  </div>
                ) : null}
                {remote.bind === "loopback" ? (
                  <p className="hint" style={{ margin: 0 }}>
                    {t("settings.remote.loopbackHint", { port: live.port })}
                  </p>
                ) : null}
              </div>
            </div>
          </Card>
        </Group>
      ) : null}

      <Group title={t("settings.remote.token")}>
        <Card>
          <Row title={t("settings.remote.token.title")} desc={t("settings.remote.token.desc")} stack>
            <div className="input-with-btn">
              <input
                readOnly
                type={showToken ? "text" : "password"}
                value={live?.token ?? ""}
                placeholder={live?.token ? "" : t("settings.remote.token.placeholder")}
                onFocus={(e) => e.currentTarget.select()}
                spellCheck={false}
              />
              <button className="btn-compact" onClick={() => setShowToken((v) => !v)} disabled={!live?.token}>
                {showToken ? t("settings.remote.token.hide") : t("settings.remote.token.show")}
              </button>
              <button
                className="btn-compact"
                onClick={() => void copy(live?.token ?? "", "token")}
                disabled={!live?.token}
              >
                {copied === "token" ? t("common.copied") : t("common.copy")}
              </button>
              <button className="btn-compact" onClick={rotate} disabled={!live?.token}>
                {t("settings.remote.token.rotate")}
              </button>
            </div>
          </Row>
        </Card>
      </Group>

      <Group title={t("settings.remote.notes")}>
        <Card>
          <CardBlock>
            <ul className="remote-notes">
              <li>{t("settings.remote.notes.tls")}</li>
              <li>{t("settings.remote.notes.origin")}</li>
              <li>{t("settings.remote.notes.web")}</li>
            </ul>
          </CardBlock>
        </Card>
      </Group>

      {error ? <FormError text={error} /> : null}
    </>
  );
}
