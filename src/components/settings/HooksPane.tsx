import { useEffect, useState } from "react";

import {
  type ConfigStatus,
  type HookInfo,
  hooksList,
  renderUiError,
  revealInFinder,
} from "../../bridge";
import { type MessageKey, useT } from "../../i18n";
import { Card, CardBlock, Group, Row } from "./layout";

const HOOK_EVENT_HINT: Record<string, MessageKey> = {
  PreToolUse: "settings.hooks.event.preToolUse",
  PostToolUse: "settings.hooks.event.postToolUse",
  Stop: "settings.hooks.event.stop",
  UserPromptSubmit: "settings.hooks.event.userPromptSubmit",
};

export function HooksPane({ status, activeRoot }: { status: ConfigStatus; activeRoot: string | null }) {
  const { t, tx } = useT();
  const [hooks, setHooks] = useState<HookInfo[] | null>(null);
  const [loadError, setLoadError] = useState("");
  const configDir = status.configPath.replace(/\/[^/]*$/, "");

  const refresh = async () => {
    setLoadError("");
    try {
      setHooks(await hooksList(activeRoot));
    } catch (e) {
      setHooks(null);
      setLoadError(String(e));
    }
  };
  useEffect(() => {
    void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeRoot]);

  return (
    <>
      <Group
        title={t("settings.ext.howTo")}
        desc={tx("settings.hooks.howTo.desc", {
          block: <b>{t("settings.hooks.howTo.block")}</b>,
          alt: <code>A|B</code>,
        })}
      >
        <Card>
          <Row title={t("settings.hooks.global")} desc={<code>{configDir}/hooks.json</code>}>
            <button onClick={() => void revealInFinder(configDir)}>{t("settings.ext.openDir")}</button>
          </Row>
          {activeRoot ? (
            <Row
              title={t("settings.hooks.project")}
              desc={tx("settings.hooks.project.desc", { file: <code>{activeRoot}/.riot/hooks.json</code> })}
            >
              <button onClick={() => void revealInFinder(activeRoot)}>{t("settings.ext.openProject")}</button>
            </Row>
          ) : null}
        </Card>
      </Group>

      <Group
        title={t("settings.hooks.registered")}
        action={
          <button className="btn-compact" onClick={() => void refresh()}>
            {t("common.refresh")}
          </button>
        }
      >
        {loadError ? (
          <div className="empty-state">
            <p className="form-error" style={{ margin: 0 }}>
              {t("settings.ext.loadFailed", { error: loadError })}
            </p>
            <button onClick={() => void refresh()}>{t("common.retry")}</button>
          </div>
        ) : hooks === null ? (
          <Card>
            <CardBlock>
              <p className="hint" style={{ margin: 0 }}>
                {t("settings.ext.loading")}
              </p>
            </CardBlock>
          </Card>
        ) : hooks.length === 0 ? (
          <div className="empty-state">
            <p className="empty-title">{t("settings.hooks.empty.title")}</p>
            <p className="hint">{t("settings.hooks.empty.hint")}</p>
            <pre className="skill-example">{`{
  "PreToolUse": [
    { "matcher": "Bash",
      "hooks": [{ "type": "command", "command": "./scripts/check-cmd.sh" }] }
  ],
  "Stop": [
    { "hooks": [{ "type": "command", "command": "cargo test -q" }] }
  ]
}`}</pre>
          </div>
        ) : (
          <ul className="skill-list">
            {hooks.map((h, i) => {
              const hintKey = HOOK_EVENT_HINT[h.event];
              return (
                <li
                  key={`${h.event}-${h.command}-${i}`}
                  className={h.error ? "skill-item bad" : "skill-item"}
                >
                  <div className="skill-item-head">
                    <span className="skill-name">{h.error ? t("settings.hooks.broken") : h.event}</span>
                    {h.matcher ? <code className="hook-matcher">{h.matcher}</code> : null}
                    <span className="skill-source">
                      {h.source === "project" ? t("settings.ext.tier.project") : t("settings.ext.tier.global")}
                    </span>
                  </div>
                  <p className={h.error ? "form-error" : "hint"} style={{ margin: "2px 0 0" }}>
                    {h.error
                      ? t("settings.hooks.commandError", { command: h.command, error: renderUiError(h.error) })
                      : h.command}
                  </p>
                  {!h.error ? (
                    <p className="hint" style={{ margin: "2px 0 0" }}>
                      {t("settings.hooks.eventHint", {
                        hint: hintKey ? t(hintKey) : "",
                        timeout: h.timeoutSecs,
                      })}
                    </p>
                  ) : null}
                </li>
              );
            })}
          </ul>
        )}
      </Group>
    </>
  );
}
