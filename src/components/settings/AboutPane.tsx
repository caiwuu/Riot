import {
  type ConfigStatus,
  type UpdateInfo,
  openInBrowser,
  revealInFinder,
} from "../../bridge";
import { type MessageKey, useT } from "../../i18n";
import { Card, CardBlock, Group, Row } from "./layout";

function friendlyUpdateError(raw: string): MessageKey {
  if (/403|429|rate limit/i.test(raw)) return "settings.about.err.rateLimit";
  if (/404/.test(raw)) return "settings.about.err.noRelease";
  return "settings.about.err.offline";
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
  const { t, tx } = useT();
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
      ? t("settings.about.checkingStatus")
      : statusKind === "err"
        ? t(friendlyUpdateError(error ?? ""))
        : statusKind === "new"
          ? t("settings.about.newer", { version: update?.latest ?? "" })
          : statusKind === "ok"
            ? t("settings.about.upToDate")
            : null;

  return (
    <>
      <Group title={t("settings.about.version")}>
        <Card>
          <CardBlock>
            <div className="about-brand">
              <span className="about-mark" aria-hidden>
                <AboutMark />
              </span>
              <div className="about-brand-text">
                <div className="about-title-row">
                  <span className="about-name">Riot</span>
                  {version ? <span className="about-ver">v{version}</span> : null}
                </div>
                <p className="about-tagline">{t("settings.about.tagline")}</p>
              </div>
              <div className="about-actions">
                <button disabled={checking} onClick={onCheck}>
                  {checking ? t("settings.about.checking") : t("settings.about.check")}
                </button>
                {update?.newer ? (
                  <button className="primary" onClick={() => void openInBrowser(update.url)}>
                    {t("settings.about.download")}
                  </button>
                ) : null}
              </div>
            </div>
            {statusText ? (
              <p className={`about-status ${statusKind ?? ""}`} title={error ?? undefined}>
                {statusText}
              </p>
            ) : null}
          </CardBlock>
        </Card>
      </Group>

      <Group title={t("settings.about.config")}>
        <Card>
          <Row
            title="config.json"
            desc={
              <>
                <code title={status.configPath}>{status.configPath}</code>
                <br />
                {tx("settings.about.config.desc", { file: <code>auth.json</code> })}
              </>
            }
          >
            <button onClick={() => void revealInFinder(configDir)}>{t("common.revealInFinder")}</button>
          </Row>
        </Card>
      </Group>
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
