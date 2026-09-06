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
  setConfig,
} from "../../bridge";
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
      setError("复制失败，请手动选中复制。");
    }
  };

  const rotate = () => {
    askConfirm({
      title: "换一枚新令牌？",
      body: "旧的链接和二维码立刻失效，已经登录的设备下次重连时要重新输入。正在连着的不会掉线。",
      confirmLabel: "换新令牌",
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
      setError("端口要在 1–65535 之间。");
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
              你正通过网页访问这台机器上的 Riot。这里的改动作用在宿主机上；关掉开关会让
              当前页面断开。
            </p>
          </CardBlock>
        </Card>
      ) : null}

      <Group title="服务">
        <Card>
          <Row
            title="允许远程访问"
            desc="宿主开一个 HTTP 服务，浏览器里加载同一套界面。拿到令牌的人能做你在这台机器前能做的一切 —— 只在自己的网络里开。"
          >
            <Switch
              on={remote.enabled}
              onChange={(v) => void patch({ enabled: v })}
              label="允许远程访问"
            />
          </Row>
          <Row
            title="监听范围"
            desc="「仅本机」配 SSH 隧道、Tailscale Serve 或反向代理；「局域网」让同一 Wi-Fi 下的手机直连。"
          >
            <FieldSelect
              value={remote.bind}
              onChange={(v) => void patch({ bind: v as RemoteConfig["bind"] })}
              options={[
                { value: "loopback", label: "仅本机（127.0.0.1）" },
                { value: "lan", label: "局域网（所有网卡）" },
              ]}
            />
          </Row>
          <Row title="端口" desc="默认 7823。被占用时改一个再开。" htmlFor="remote-port">
            <span className="field-inline">
              <FieldNumber
                id="remote-port"
                value={port}
                onChange={(e) => setPort(e.target.value)}
                onBlur={commitPort}
                onKeyDown={blurOnEnter}
                aria-label="端口"
              />
            </span>
          </Row>
          {live ? (
            <CardBlock>
              {live.error ? (
                <p className="key-state warn" style={{ margin: 0 }}>
                  没起来：{live.error}
                </p>
              ) : live.running ? (
                <p className="key-state ok" style={{ margin: 0 }}>
                  正在监听 <code>{live.listenAddr}</code>
                  {live.connections > 0 ? ` · ${live.connections} 个连接` : " · 暂无连接"}
                </p>
              ) : (
                <p className="hint" style={{ margin: 0 }}>
                  未开启。
                </p>
              )}
            </CardBlock>
          ) : null}
        </Card>
      </Group>

      {live?.running ? (
        <Group title="连接" desc="手机扫二维码即登录；或把链接发到另一台设备打开（链接里带着令牌，别公开分享）。">
          <Card>
            <div className="remote-connect">
              {qrSrc ? (
                <img className="remote-qr" src={qrSrc} alt="登录二维码" width={180} height={180} />
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
                      {copied === "url" ? "已复制" : "复制登录链接"}
                    </button>
                  </div>
                ) : null}
                {remote.bind === "loopback" ? (
                  <p className="hint" style={{ margin: 0 }}>
                    当前只听本机。别的设备要连，得先把 127.0.0.1:{live.port} 通过隧道或代理转出去，
                    或者把监听范围改成「局域网」。
                  </p>
                ) : null}
              </div>
            </div>
          </Card>
        </Group>
      ) : null}

      <Group title="令牌">
        <Card>
          <Row
            title="访问令牌"
            desc="登录用的密码。存在 auth.json（只有你能读），不进 config.json。泄露了就换一枚。"
            stack
          >
            <div className="input-with-btn">
              <input
                readOnly
                type={showToken ? "text" : "password"}
                value={live?.token ?? ""}
                placeholder={live?.token ? "" : "开启后自动生成"}
                onFocus={(e) => e.currentTarget.select()}
                spellCheck={false}
              />
              <button className="btn-compact" onClick={() => setShowToken((v) => !v)} disabled={!live?.token}>
                {showToken ? "隐藏" : "显示"}
              </button>
              <button
                className="btn-compact"
                onClick={() => void copy(live?.token ?? "", "token")}
                disabled={!live?.token}
              >
                {copied === "token" ? "已复制" : "复制"}
              </button>
              <button className="btn-compact" onClick={rotate} disabled={!live?.token}>
                换新令牌
              </button>
            </div>
          </Row>
        </Card>
      </Group>

      <Group title="安全提示">
        <Card>
          <CardBlock>
            <ul className="remote-notes">
              <li>服务本身不带 TLS。局域网直连在你自己的 Wi-Fi 里；要出公网，用 Tailscale Serve、Cloudflare Tunnel 或 nginx 之类挂证书，别直接把端口暴露到路由器外。</li>
              <li>浏览器里的第三方网页连不上这个端口（来源校验），令牌猜错五次同一来源锁一分钟。</li>
              <li>网页版拿不到宿主机的文件对话框和拖放路径：选项目目录用应用内的目录选择器，附图片直接选或粘贴，引用文件在输入框里打 @。</li>
            </ul>
          </CardBlock>
        </Card>
      </Group>

      {error ? <FormError text={error} /> : null}
    </>
  );
}
