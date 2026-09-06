import { useEffect, useState } from "react";

import {
  type ConfigStatus,
  type WebConfig,
  setConfig,
  testSearchBackend,
} from "../../bridge";
import { useT } from "../../i18n";
import { FieldSelect } from "../FieldSelect";
import { Card, CardBlock, Group, Row } from "./layout";
import { FormError, Switch, blurOnEnter } from "./shared";

/**
 * 抓取、搜索、蒸馏三块。
 *
 * 排布顺序对应用户配置的顺序：先决定让不让上网，再决定是否覆盖内置搜索，
 * 最后是可选的辅助模型。把辅助模型放前面会让人以为它是必填项。
 */
export function WebPane({
  status,
  onStatus,
  onSaved,
}: {
  status: ConfigStatus;
  onStatus: (s: ConfigStatus) => void;
  onSaved: () => void;
}) {
  const { t, tx } = useT();
  const web = status.config.web;
  const [url, setUrl] = useState(web.searxngUrl);
  const [error, setError] = useState("");
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; text: string } | null>(null);

  // 宿主会把内置域名收成空。输入框必须跟着真值走，否则会把域名留在框里。
  useEffect(() => {
    setUrl(web.searxngUrl);
  }, [web.searxngUrl]);

  const patch = async (p: Partial<WebConfig>) => {
    setError("");
    try {
      onStatus(await setConfig({ ...status.config, web: { ...web, ...p } }));
      onSaved();
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    }
  };

  const doTest = async () => {
    setTesting(true);
    setTestResult(null);
    try {
      const detail = await testSearchBackend(url);
      setTestResult({ ok: true, text: t("settings.web.test.ok", { detail }) });
    } catch (e) {
      setTestResult({ ok: false, text: String(e) });
    } finally {
      setTesting(false);
    }
  };

  // 辅助模型的候选是所有 provider 下已添加的模型。跨 provider 是有意的：
  // 主对话用贵模型、蒸馏用本地小模型，正是这个功能存在的理由。
  const allModels = status.config.providers.flatMap((p) =>
    p.models.map((m) => ({
      value: `${p.id}/${m.id}`,
      label: `${p.name} · ${m.name?.trim() || m.id}`,
    })),
  );

  return (
    <>
      <Group title={t("settings.web.access")}>
        <Card>
          <Row title={t("settings.web.fetch")} desc={t("settings.web.fetch.desc")}>
            <Switch
              on={web.fetchEnabled}
              onChange={(v) => void patch({ fetchEnabled: v })}
              label={t("settings.web.fetch")}
            />
          </Row>
          <Row title={t("settings.web.search")} desc={t("settings.web.search.desc")}>
            <Switch
              on={web.searchEnabled}
              onChange={(v) => void patch({ searchEnabled: v })}
              label={t("settings.web.search")}
            />
          </Row>
          <Row
            title={t("settings.web.searxng")}
            desc={tx("settings.web.searxng.desc", {
              limiter: <code>server.limiter: false</code>,
              formats: <code>search.formats</code>,
              json: <code>json</code>,
            })}
            stack
          >
            <div className="input-with-btn">
              <input
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                onBlur={() => {
                  const v = url.trim();
                  if (v === web.searxngUrl) return;
                  void patch({ searxngUrl: v }).then((ok) => {
                    // 保存被拒时退回真值，别让输入框展示一个没生效的地址
                    if (!ok) setUrl(web.searxngUrl);
                  });
                }}
                onKeyDown={blurOnEnter}
                placeholder={t("settings.web.searxng.placeholder")}
                spellCheck={false}
                disabled={!web.searchEnabled}
              />
              <span
                className="tip-wrap"
                title={
                  !web.searchEnabled
                    ? t("settings.web.test.needsSearch")
                    : url.trim()
                      ? t("settings.web.test.custom")
                      : t("settings.web.test.builtin")
                }
              >
                <button
                  className="btn-compact"
                  onClick={() => void doTest()}
                  disabled={testing || !web.searchEnabled}
                >
                  {testing ? t("settings.common.testing") : t("settings.web.test")}
                </button>
              </span>
            </div>
            {testResult ? (
              <p className={testResult.ok ? "test-result ok" : "test-result err"}>
                {testResult.text}
              </p>
            ) : null}
          </Row>
        </Card>
      </Group>

      <Group title={t("settings.web.distill")}>
        <Card>
          <Row title={t("settings.web.distill.model")} desc={t("settings.web.distill.model.desc")}>
            <FieldSelect
              value={allModels.some((m) => m.value === web.distillModel) ? web.distillModel : ""}
              onChange={(v) => void patch({ distillModel: v })}
              options={[{ value: "", label: t("settings.web.distill.none") }, ...allModels]}
            />
          </Row>
          {web.distillModel && !allModels.some((m) => m.value === web.distillModel) ? (
            <CardBlock>
              <p className="key-state warn" style={{ margin: 0 }}>
                {tx("settings.web.distill.gone", { model: <code>{web.distillModel}</code> })}
              </p>
            </CardBlock>
          ) : null}
        </Card>
      </Group>

      {error ? <FormError text={error} /> : null}
    </>
  );
}
