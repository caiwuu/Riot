import { useEffect, useState } from "react";

import {
  type ConfigStatus,
  type SlashCommand,
  revealInFinder,
  slashCommands,
} from "../../bridge";
import { type MessageKey, useT } from "../../i18n";
import { Card, CardBlock, Group, Row } from "./layout";

/** 技能在命令页的层级前缀。和 Skills 页用同一套词。 */
const SKILL_TIER: Record<string, MessageKey> = {
  builtin: "settings.ext.tier.builtin",
  pack: "settings.ext.tier.pack",
  global: "settings.ext.tier.global",
  project: "settings.ext.tier.project",
};

/** 「内置技能」「项目技能」…；层级不认识就只写「技能」。 */
function skillSourceLabel(source: string | undefined, t: ReturnType<typeof useT>["t"]): string {
  const tier = SKILL_TIER[source ?? ""];
  return tier ? t("settings.commands.skillTier", { tier: t(tier) }) : t("settings.commands.skill");
}

export function CommandsPane({ status, activeRoot }: { status: ConfigStatus; activeRoot: string | null }) {
  const { t, tx } = useT();
  const [commands, setCommands] = useState<SlashCommand[] | null>(null);
  const [loadError, setLoadError] = useState("");
  const configDir = status.configPath.replace(/\/[^/]*$/, "");

  const refresh = async () => {
    setLoadError("");
    try {
      setCommands(await slashCommands(activeRoot));
    } catch (e) {
      setCommands(null);
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
        desc={tx("settings.commands.howTo.desc", {
          slash: <code>/</code>,
          args: <code>$ARGUMENTS</code>,
          positional: <code>$1 $2</code>,
          example: (
            <>
              <code>git/pr.md</code> → <code>/git:pr</code>
            </>
          ),
          priority: <strong>{t("settings.commands.howTo.priority")}</strong>,
        })}
      >
        <Card>
          <Row
            title={t("settings.commands.globalDir")}
            desc={<code>{t("settings.commands.dirPattern", { dir: `${configDir}/commands` })}</code>}
          >
            <button onClick={() => void revealInFinder(configDir)}>{t("settings.ext.openDir")}</button>
          </Row>
          {activeRoot ? (
            <Row
              title={t("settings.commands.projectDir")}
              desc={<code>{t("settings.commands.dirPattern", { dir: `${activeRoot}/.riot/commands` })}</code>}
            >
              <button onClick={() => void revealInFinder(activeRoot)}>{t("settings.ext.openProject")}</button>
            </Row>
          ) : null}
        </Card>
      </Group>

      <Group
        title={t("settings.commands.available")}
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
        ) : commands === null ? (
          <Card>
            <CardBlock>
              <p className="hint" style={{ margin: 0 }}>
                {t("settings.ext.loading")}
              </p>
            </CardBlock>
          </Card>
        ) : commands.length === 0 ? (
          <div className="empty-state">
            <p className="empty-title">{t("settings.commands.empty.title")}</p>
            <p className="hint">{tx("settings.commands.empty.hint", { ext: <code>.md</code> })}</p>
            <pre className="skill-example">{t("settings.commands.example")}</pre>
          </div>
        ) : (
          <ul className="skill-list">
            {commands.map((c) => (
              <li key={c.name} className="skill-item">
                <div className="skill-item-head">
                  <span className="skill-name">/{c.name}</span>
                  {c.argumentHint ? <code className="hook-matcher">{c.argumentHint}</code> : null}
                  {/* 技能带上自己的层级 —— 只写「技能」的话，同一个
                      extend-riot 在 Skills 页是「内置」、这里是「技能」，
                      同一个东西两套说法。 */}
                  <span className="skill-source">
                    {c.source === "builtin"
                      ? t("settings.ext.tier.builtin")
                      : c.source === "skill"
                        ? skillSourceLabel(c.skillSource, t)
                        : c.source === "project"
                          ? t("settings.ext.tier.project")
                          : t("settings.ext.tier.global")}
                  </span>
                </div>
                <p className="hint" style={{ margin: "2px 0 0" }}>
                  {c.description}
                </p>
              </li>
            ))}
          </ul>
        )}
      </Group>
    </>
  );
}
