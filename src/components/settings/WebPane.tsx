import { useEffect, useState } from "react";

import {
  type ConfigStatus,
  type WebConfig,
  setConfig,
  testSearchBackend,
} from "../../bridge";
import { FieldSelect } from "../FieldSelect";
import { HintTip } from "../HintTip";
import { FormError, Toggle, blurOnEnter } from "./shared";

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
      setTestResult({ ok: true, text: await testSearchBackend(url) });
    } catch (e) {
      setTestResult({ ok: false, text: String(e) });
    } finally {
      setTesting(false);
    }
  };

  // 辅助模型的候选是所有 provider 下已添加的模型。跨 provider 是有意的：
  // 主对话用贵模型、蒸馏用本地小模型，正是这个功能存在的理由。
  const allModels = status.config.providers.flatMap((p) =>
    p.models.map((m) => ({ value: `${p.id}/${m}`, label: `${p.name} · ${m}` })),
  );

  return (
    <>
      <section>
        <h2>
          网页抓取
          <HintTip>首次访问每个域名会询问。内网地址一律拒绝。</HintTip>
        </h2>
        <Toggle
          on={web.fetchEnabled}
          onChange={(v) => void patch({ fetchEnabled: v })}
          label="允许模型抓取网页（WebFetch）"
        />
      </section>

      <section>
        <h2>搜索</h2>
        <Toggle
          on={web.searchEnabled}
          onChange={(v) => void patch({ searchEnabled: v })}
          label="允许模型联网搜索（WebSearch）"
        />
        <div className="field-row">
          <label>
            自定义 SearXNG
            <HintTip>
              留空使用内置搜索。自建实例要求 <code>server.limiter: false</code>，且{" "}
              <code>search.formats</code> 含 <code>json</code>。
            </HintTip>
          </label>
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
              placeholder="留空则使用内置搜索"
              spellCheck={false}
              disabled={!web.searchEnabled}
            />
            <span
              className="tip-wrap"
              title={
                !web.searchEnabled
                  ? "先打开上面的搜索开关"
                  : url.trim()
                    ? "会真发一次查询"
                    : "会测内置搜索"
              }
            >
              <button
                className="btn-compact"
                onClick={() => void doTest()}
                disabled={testing || !web.searchEnabled}
              >
                {testing ? "测试中…" : "测试"}
              </button>
            </span>
          </div>
        </div>
        {testResult ? (
          <p className={testResult.ok ? "test-result ok" : "test-result err"}>{testResult.text}</p>
        ) : null}
      </section>

      <section>
        <h2>
          正文蒸馏
          <HintTip>用便宜的模型把网页压成摘要，省上下文。留空则直接截断。</HintTip>
        </h2>
        <div className="field-row">
          <label>辅助模型</label>
          <FieldSelect
            value={allModels.some((m) => m.value === web.distillModel) ? web.distillModel : ""}
            onChange={(v) => void patch({ distillModel: v })}
            options={[
              { value: "", label: "不蒸馏（返回截断的正文）" },
              ...allModels,
            ]}
          />
        </div>
        {web.distillModel && !allModels.some((m) => m.value === web.distillModel) ? (
          <p className="key-state warn">
            <code>{web.distillModel}</code> 已不存在，当前不会蒸馏。
          </p>
        ) : null}
      </section>

      {error ? <FormError text={error} /> : null}
    </>
  );
}
