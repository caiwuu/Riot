import { FieldSelect } from "../FieldSelect";
import { LOCALE_NAMES, LOCALES, isLocale, setLocaleChoice, useLocaleChoice, useT } from "../../i18n";
import { isTheme, setThemeChoice, useThemeChoice } from "../../theme";
import { Card, Group, Row } from "./layout";

/** 下拉里"跟随系统"那一项的值。语言代码和主题名里都不会出现，不会撞。 */
const SYSTEM = "system";

export function GeneralPane() {
  const { t } = useT();
  const choice = useLocaleChoice();
  const theme = useThemeChoice();

  const options = [
    { value: SYSTEM, label: t("settings.general.language.system") },
    ...LOCALES.map((l) => ({ value: l, label: LOCALE_NAMES[l] })),
  ];

  const themeOptions = [
    { value: SYSTEM, label: t("settings.general.theme.system") },
    { value: "light", label: t("settings.general.theme.light") },
    { value: "dark", label: t("settings.general.theme.dark") },
  ];

  return (
    <Group title={t("settings.tab.general.title")}>
      <Card>
        <Row title={t("settings.general.language")} desc={t("settings.general.language.desc")}>
          <FieldSelect
            value={choice ?? SYSTEM}
            options={options}
            onChange={(v) => setLocaleChoice(isLocale(v) ? v : null)}
          />
        </Row>
        <Row title={t("settings.general.theme")} desc={t("settings.general.theme.desc")}>
          <FieldSelect
            value={theme ?? SYSTEM}
            options={themeOptions}
            onChange={(v) => setThemeChoice(isTheme(v) ? v : null)}
          />
        </Row>
      </Card>
    </Group>
  );
}
