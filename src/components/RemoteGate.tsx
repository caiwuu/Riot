import { type ReactNode, useEffect, useState } from "react";

import {
  type LinkStatus,
  host,
  remoteDeniedReason,
  remoteSetToken,
  subscribeHostLink,
} from "../bridge";

/**
 * 网页版的门：没连上宿主之前不渲染应用，连着的时候在顶上挂一条连接状态。
 *
 * 桌面窗口里它是透明的（IPC 永远是 open）。浏览器里三种状态各有一屏：
 * - `auth-required`：要令牌。首次访问用带 `#token=` 的链接（扫二维码）
 *   不会看到这一屏；换过令牌、或手动打地址进来的才会。
 * - `connecting`：第一次连接中，给品牌骨架（和桌面冷启动同一个样子）。
 * - `reconnecting`：应用照常渲染，顶上压一条"正在重连"—— 不能把应用
 *   卸掉，那会丢掉输入框里正在打的字。
 */
export function RemoteGate({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<LinkStatus>("connecting");
  useEffect(() => subscribeHostLink(setStatus), []);

  if (host.kind === "tauri") return <>{children}</>;

  if (status === "auth-required") return <TokenForm />;
  if (status === "connecting") {
    return (
      <div className="booting">
        <div className="booting-logo">Riot</div>
        <div className="booting-spinner" aria-label="正在连接" />
        <p className="hint">正在连接宿主…</p>
      </div>
    );
  }
  return (
    <>
      {status === "reconnecting" ? (
        <div className="link-banner" role="status">
          <span className="booting-spinner" aria-hidden />
          与宿主的连接断了，正在重连…
        </div>
      ) : null}
      {children}
    </>
  );
}

function TokenForm() {
  const [token, setToken] = useState("");
  // 每次回到这一屏都是一次新的"被拒"或"没令牌"，读一次当前原因就够；
  // 提交后表单会先切到 connecting 屏，再被拒会重新挂载、重新读。
  const denied = remoteDeniedReason();
  const submit = () => {
    const t = token.trim();
    if (!t) return;
    remoteSetToken(t);
  };
  return (
    <div className="boot-fail remote-login">
      <div className="booting-logo">Riot</div>
      <h1>连接到 Riot</h1>
      <p className="hint">
        输入桌面端「设置 → 远程访问」里显示的访问令牌。扫那里的二维码可以跳过这一步。
      </p>
      {denied ? (
        <p className="form-error" role="alert">
          宿主拒绝了上一次连接：{denied}
        </p>
      ) : null}
      <form
        className="input-with-btn"
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        <input
          autoFocus
          value={token}
          onChange={(e) => setToken(e.target.value)}
          placeholder="访问令牌"
          spellCheck={false}
          autoComplete="off"
          autoCapitalize="off"
          inputMode="text"
        />
        <button type="submit" className="primary" disabled={!token.trim()}>
          连接
        </button>
      </form>
    </div>
  );
}
