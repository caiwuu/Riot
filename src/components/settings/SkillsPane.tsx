import { useEffect, useState } from "react";

import {
  type ConfigStatus,
  type SkillInfo,
  renderUiError,
  revealInFinder,
  skillsList,
} from "../../bridge";
import { type MessageKey, useT } from "../../i18n";
import { Card, CardBlock, Group, Row } from "./layout";

const SOURCE_TIER: Record<SkillInfo["source"], MessageKey> = {
  builtin: "settings.ext.tier.builtin",
  project: "settings.ext.tier.project",
  pack: "settings.ext.tier.pack",
  global: "settings.ext.tier.global",
};

/**
 * 技能清单（只读）。技能就是磁盘上的 SKILL.md，编辑器比表单好用 ——
 * 这页只负责"有哪些、哪个坏了、目录在哪"。
 */
export function SkillsPane({ status, activeRoot }: { status: ConfigStatus; activeRoot: string | null }) {
  const { t, tx } = useT();
  const [skills, setSkills] = useState<SkillInfo[] | null>(null);
  const [loadError, setLoadError] = useState("");
  const configDir = status.configPath.replace(/\/[^/]*$/, "");
  const globalDir = `${configDir}/skills`;

  const refresh = async () => {
    setLoadError("");
    try {
      setSkills(await skillsList(activeRoot));
    } catch (e) {
      // 读失败不能装成"还没有技能"：空状态会引导用户去建目录，而不是去修权限
      setSkills(null);
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
        desc={tx("settings.skills.howTo.desc", {
          skillMd: <code>SKILL.md</code>,
          priority: <strong>{t("settings.skills.howTo.priority")}</strong>,
          slash: <code>/</code>,
        })}
      >
        <Card>
          <Row
            title={t("settings.skills.globalDir")}
            desc={<code>{t("settings.skills.dirPattern", { dir: globalDir })}</code>}
          >
            <button onClick={() => void revealInFinder(globalDir)}>{t("settings.ext.openDir")}</button>
          </Row>
          {activeRoot ? (
            <Row
              title={t("settings.skills.projectDir")}
              desc={<code>{t("settings.skills.dirPattern", { dir: `${activeRoot}/.riot/skills` })}</code>}
            >
              <button onClick={() => void revealInFinder(activeRoot)}>{t("settings.ext.openProject")}</button>
            </Row>
          ) : null}
        </Card>
      </Group>

      <Group
        title={t("settings.skills.found")}
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
        ) : skills === null ? (
          <Card>
            <CardBlock>
              <p className="hint" style={{ margin: 0 }}>
                {t("settings.ext.loading")}
              </p>
            </CardBlock>
          </Card>
        ) : skills.length === 0 ? (
          <div className="empty-state">
            <p className="empty-title">{t("settings.skills.empty.title")}</p>
            <p className="hint">{t("settings.skills.empty.hint")}</p>
            <pre className="skill-example">{t("settings.skills.example")}</pre>
          </div>
        ) : (
          <ul className="skill-list">
            {skills.map((s) => (
              <li
                key={s.path || `builtin-${s.name}`}
                className={s.error ? "skill-item bad" : "skill-item"}
              >
                <div className="skill-item-head">
                  <span className="skill-name">{s.name}</span>
                  <span className="skill-source">
                    {t(SOURCE_TIER[s.source])}
                  </span>
                </div>
                <p className={s.error ? "form-error" : "hint"} style={{ margin: "2px 0 0" }}>
                  {s.error ? renderUiError(s.error) : s.description}
                </p>
              </li>
            ))}
          </ul>
        )}
      </Group>
    </>
  );
}
