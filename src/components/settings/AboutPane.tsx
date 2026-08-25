import {
  type ConfigStatus,
  type UpdateInfo,
  openInBrowser,
  revealInFinder,
} from "../../bridge";
import { HintTip } from "../HintTip";

function friendlyUpdateError(raw: string): string {
  if (/403|429|rate limit/i.test(raw)) return "GitHub 暂时限流，过一会再试。";
  if (/404/.test(raw)) return "还没有发布过正式版本。";
  return "现在连不上更新服务。";
}

export function AboutPane({
  status,
  version,
  update,
  checking,
  error,
  onCheck,
}: {
  status: ConfigStatus;
  version: string;
  update: UpdateInfo | null;
  checking: boolean;
  error: string | null;
  onCheck: () => void;
}) {
  const configDir = status.configPath.replace(/\/[^/]*$/, "");
  const statusKind = checking
    ? "pending"
    : error
      ? "err"
      : update?.newer
        ? "new"
        : update
          ? "ok"
          : null;
  const statusText =
    statusKind === "pending"
      ? "正在检查…"
      : statusKind === "err"
        ? friendlyUpdateError(error ?? "")
        : statusKind === "new"
          ? `有新版本 ${update?.latest}`
          : statusKind === "ok"
            ? "已是最新版本"
            : null;

  return (
    <>
      <section>
        <h2>关于</h2>
        <div className="about-card">
          <div className="about-brand">
            <span className="about-mark" aria-hidden>
              <AboutMark />
            </span>
            <div className="about-brand-text">
              <div className="about-title-row">
                <span className="about-name">Riot</span>
                {version ? <span className="about-ver">v{version}</span> : null}
              </div>
              <p className="about-tagline">一款轻量、强大的智能体工作台</p>
            </div>
            <div className="about-actions">
              <button disabled={checking} onClick={onCheck}>
                {checking ? "检查中…" : "检查更新"}
              </button>
              {update?.newer ? (
                <button className="primary" onClick={() => void openInBrowser(update.url)}>
                  去下载 {update.latest}
                </button>
              ) : null}
            </div>
          </div>
          {statusText ? (
            <p
              className={`about-status ${statusKind ?? ""}`}
              title={error ?? undefined}
            >
              {statusText}
            </p>
          ) : null}
        </div>
      </section>
      <section>
        <h2>
          配置文件
          <HintTip>
            API key 单独存在同目录的 <code>auth.json</code>。
          </HintTip>
        </h2>
        <div className="about-card">
          <div className="about-path">
            <code title={status.configPath}>{status.configPath}</code>
            <button onClick={() => void revealInFinder(configDir)}>在访达中显示</button>
          </div>
        </div>
      </section>
    </>
  );
}

function AboutMark() {
  return (
    <svg width="22" height="22" viewBox="0 0 24 24" fill="none" aria-hidden>
      <g stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
        <path d="M7.4 4.2v15.6" />
        <path d="M7.4 4.2h5A4.2 4.2 0 0 1 12.4 12.6H7.4" />
        <path d="M11.6 12.6l5 4.9-2.7 2.3" />
      </g>
    </svg>
  );
}
