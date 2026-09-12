import { useState } from "react";

import {
  type AppConfig,
  type ConfigStatus,
  type ModelConfig,
  type Protocol,
  type ProviderConfig,
  listModels,
  setApiKey,
  testConnection,
} from "../../bridge";
import { useTimedFlag } from "../../hooks/useTimedFlag";
import { useT } from "../../i18n";
import {
  type SamplingDraft,
  parseSampling,
  sameSampling,
  samplingDraft,
} from "../../lib/sampling";
import { SamplingSliders } from "../FieldSlider";
import { ModelDialog } from "../ModelDialog";
import { ResizableTextarea } from "../ResizableTextarea";
import { Card, CardBlock, Group, Row } from "./layout";
import { type AskConfirm, blurOnEnter } from "./shared";

function headersToText(h?: Record<string, string> | null): string {
  return Object.entries(h ?? {})
    .map(([k, v]) => `${k}=${v}`)
    .join("\n");
}

function parseHeaders(
  text: string,
): { ok: true; value: Record<string, string> } | { ok: false; line: string } {
  const map: Record<string, string> = {};
  for (const line of text.split("\n")) {
    const item = line.trim();
    if (!item) continue;
    const eq = item.indexOf("=");
    if (eq <= 0) return { ok: false, line: item };
    map[item.slice(0, eq).trim()] = item.slice(eq + 1).trim();
  }
  return { ok: true, value: map };
}

function sameHeaders(a?: Record<string, string> | null, b?: Record<string, string> | null): boolean {
  return JSON.stringify(Object.entries(a ?? {}).sort()) === JSON.stringify(Object.entries(b ?? {}).sort());
}

/** 路径留空时实际会用的默认值。两个协议各不同。 */
function defaultPath(protocol: Protocol): string {
  return protocol === "anthropic" ? "/v1/messages" : "/v1/chat/completions";
}

/**
 * 主机 + 路径拼成完整地址，和宿主那边的拼法保持一致。
 *
 * `[约束]` 这里只是给用户看的预览，真正发请求的拼接在宿主
 * （`riot_providers::endpoint`）。两边规则不一样的话，预览会变成一句谎话 ——
 * 那比不显示更糟。改其中一边时另一边要跟上。
 */
function joinUrl(base: string, path: string): string {
  const b = base.trim().replace(/\/+$/, "");
  const p = path.trim().replace(/^\/+/, "");
  if (!b) return "";
  return `${b}/${p}`;
}


/**
 * 单个 provider 的编辑表单。
 *
 * `key={provider.id}` 让切换服务方时整个表单重挂载 —— 文本框的本地
 * 草稿不会串到另一个 provider 头上。
 */
export function ProviderEditor({
  provider: p,
  cfg,
  keySource,
  autoFocusName,
  onPatch,
  onCommit,
  onStatus,
  onRemove,
  askConfirm,
  onError,
}: {
  provider: ProviderConfig;
  cfg: AppConfig;
  keySource: string | null;
  /** 刚新建时聚焦名称输入框。 */
  autoFocusName?: boolean;
  onPatch: (patch: Partial<ProviderConfig>) => Promise<boolean>;
  onCommit: (next: AppConfig) => Promise<boolean>;
  onStatus: (s: ConfigStatus) => void;
  onRemove: (() => void) | null;
  askConfirm: AskConfirm;
  onError: (e: string) => void;
}) {
  const { t, tx } = useT();
  // 文本字段走本地草稿、失焦提交。每敲一个字符就 IPC+写盘太吵。
  const [name, setName] = useState(p.name);
  const [baseUrl, setBaseUrl] = useState(p.baseUrl);
  const [apiPath, setApiPath] = useState(p.apiPath ?? "");
  const [keyDraft, setKeyDraft] = useState("");
  const [savedFlash, flashSaved] = useTimedFlag(false, 2000);
  /** 正在编辑的模型。null = 没开弹窗。 */
  const [editing, setEditing] = useState<ModelConfig | null>(null);
  const [adding, setAdding] = useState(false);
  const [fetched, setFetched] = useState<string[] | null>(null);
  const [fetching, setFetching] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; text: string } | null>(null);
  const [sampDraft, setSampDraft] = useState<SamplingDraft>(() => samplingDraft(p.sampling));
  const [headersDraft, setHeadersDraft] = useState(() => headersToText(p.extraHeaders));
  /**
   * 「测试连接」拿哪个模型发请求。只是这个编辑器里的一次挑选：不落配置、
   * 不碰对话在用的模型 —— 换对话用的模型在输入框上换。早先这里的圆点
   * 直接改全局 activeModel，结果在设置里点一下，输入框上的模型跟着跳，
   * 用户以为只是在挑要测的那个。
   *
   * 初值取对话正在用的那个（如果属于这家），否则列表第一个 —— 那多半
   * 就是用户最想验证的。之后只跟用户在这里的点击走。
   */
  const [testPick, setTestPick] = useState(
    () => (cfg.activeProvider === p.id && cfg.activeModel) || p.models[0]?.id || "",
  );
  // 挑中的那个被删了就退回列表第一个，别让圆点指着一个不存在的名字。
  const testModel = p.models.some((m) => m.id === testPick) ? testPick : (p.models[0]?.id ?? "");

  const blurCommit = () => {
    const patch: Partial<ProviderConfig> = {};
    if (name.trim() && name.trim() !== p.name) patch.name = name.trim();
    const url = baseUrl.trim().replace(/\/+$/, "");
    if (url && url !== p.baseUrl) patch.baseUrl = url;
    // 路径允许清空 —— 空的意思是"按主机猜"，那是默认行为，不是"没填完"。
    // 所以这里不能像 name / baseUrl 那样跳过空值。
    const path = apiPath.trim();
    if (path !== (p.apiPath ?? "")) patch.apiPath = path;
    if (!Object.keys(patch).length) return;
    void onPatch(patch).then((ok) => {
      // 保存被拒时草稿退回真值 —— 留着用户输入的话，框里显示的和
      // 实际生效的从此分叉，之后每一次调试都建立在假象上。
      if (!ok) {
        setName(p.name);
        setBaseUrl(p.baseUrl);
        setApiPath(p.apiPath ?? "");
      }
    });
  };

  const commitHeaders = () => {
    const parsed = parseHeaders(headersDraft);
    if (!parsed.ok) {
      onError(t("settings.provider.editor.headers.format", { line: parsed.line }));
      setHeadersDraft(headersToText(p.extraHeaders));
      return;
    }
    if (sameHeaders(parsed.value, p.extraHeaders)) return;
    void onPatch({ extraHeaders: parsed.value }).then((ok) => {
      if (!ok) setHeadersDraft(headersToText(p.extraHeaders));
    });
  };

  const commitSampling = (draft: SamplingDraft) => {
    const next = parseSampling(draft);
    if (sameSampling(next, p.sampling)) return;
    void onPatch({ sampling: next }).then((ok) => {
      if (!ok) setSampDraft(samplingDraft(p.sampling));
    });
  };

  const saveKey = async () => {
    const k = keyDraft.trim();
    if (!k) return;
    try {
      onStatus(await setApiKey(p.id, k));
      setKeyDraft("");
      flashSaved(true);
    } catch (e) {
      onError(String(e));
    }
  };

  const addModel = (m: string) => {
    const id = m.trim();
    if (!id || p.models.some((x) => x.id === id)) return;
    void onPatch({ models: [...p.models, { id }] });
  };

  /** 弹窗保存:已有的替换掉，新的追加。 */
  const saveModel = (m: ModelConfig) => {
    const exists = p.models.some((x) => x.id === m.id);
    void onPatch({
      models: exists ? p.models.map((x) => (x.id === m.id ? m : x)) : [...p.models, m],
    });
  };

  const removeModel = (m: string) => {
    const isActive = cfg.activeProvider === p.id && cfg.activeModel === m;
    askConfirm({
      title: t("settings.provider.editor.model.remove.title", { model: m }),
      body: isActive
        ? t("settings.provider.editor.model.remove.active")
        : t("settings.provider.editor.model.remove.other"),
      confirmLabel: t("common.remove"),
      action: () => {
        const models = p.models.filter((x) => x.id !== m);
        // 删的是激活模型：清空 active，避免留下指向幽灵名字的配置
        if (isActive) {
          void onCommit({
            ...cfg,
            activeModel: "",
            providers: cfg.providers.map((x) => (x.id === p.id ? { ...x, models } : x)),
          });
        } else {
          void onPatch({ models });
        }
      },
    });
  };

  const doFetch = async () => {
    setFetching(true);
    setFetched(null);
    try {
      setFetched(await listModels(p.id));
    } catch (e) {
      onError(String(e));
    } finally {
      setFetching(false);
    }
  };

  const doTest = async () => {
    // 一个模型都没有就别发请求 —— 空模型名会换来一句各家措辞不一的 400，
    // 用户从那种报错里看不出"其实是没选模型"。
    if (!testModel) {
      setTestResult({ ok: false, text: t("settings.provider.editor.test.noModel") });
      return;
    }
    setTesting(true);
    setTestResult(null);
    try {
      const detail = await testConnection(p.id, testModel);
      setTestResult({ ok: true, text: t("settings.provider.editor.test.ok", { detail }) });
    } catch (e) {
      setTestResult({ ok: false, text: String(e) });
    } finally {
      setTesting(false);
    }
  };

  const testModelCfg = p.models.find((m) => m.id === testModel);

  return (
    <>
      <Group title={t("settings.provider.editor.connection")}>
        <Card>
          <Row title={t("settings.common.name")} desc={t("settings.provider.editor.name.desc")}>
            <input
              value={name}
              onChange={(e) => setName(e.target.value)}
              onBlur={blurCommit}
              onKeyDown={blurOnEnter}
              autoFocus={autoFocusName}
              spellCheck={false}
              aria-label={t("settings.common.name")}
            />
          </Row>
          <Row title={t("settings.provider.editor.protocol")} desc={t("settings.provider.editor.protocol.desc")}>
            <div className="radio-row" role="radiogroup" aria-label={t("settings.provider.editor.protocol")}>
              {(["openai", "anthropic"] as Protocol[]).map((proto) => (
                <button
                  key={proto}
                  role="radio"
                  aria-checked={p.protocol === proto}
                  className={p.protocol === proto ? "radio-pill active" : "radio-pill"}
                  onClick={() => void onPatch({ protocol: proto })}
                >
                  {proto === "openai" ? t("settings.provider.editor.protocol.openai") : "Anthropic"}
                </button>
              ))}
            </div>
          </Row>
          <Row title={t("settings.provider.editor.baseUrl")}>
            <input
              value={baseUrl}
              onChange={(e) => setBaseUrl(e.target.value)}
              onBlur={blurCommit}
              onKeyDown={blurOnEnter}
              placeholder="https://api.example.com"
              spellCheck={false}
              aria-label={t("settings.provider.editor.baseUrl")}
            />
          </Row>
          <Row
            title={t("settings.provider.editor.apiPath")}
            desc={tx("settings.provider.editor.apiPath.desc", {
              example: <code>/api/paas/v4/chat/completions</code>,
            })}
          >
            <input
              value={apiPath}
              onChange={(e) => setApiPath(e.target.value)}
              onBlur={blurCommit}
              onKeyDown={blurOnEnter}
              placeholder={defaultPath(p.protocol)}
              spellCheck={false}
              aria-label={t("settings.provider.editor.apiPath")}
            />
          </Row>
          {/* 把拼出来的完整地址摆出来。路径错一段的表现只是一个 404，
              报错里没有任何线索指向它 —— 而在这里一眼就能看出来。 */}
          <CardBlock className="url-preview-block">
            <span className="set-row-title">{t("settings.provider.editor.urlPreview")}</span>
            <p className="url-preview">
              {joinUrl(baseUrl, apiPath.trim() || defaultPath(p.protocol))}
            </p>
          </CardBlock>
          <Row
            title={t("settings.provider.editor.headers")}
            desc={tx("settings.provider.editor.headers.desc", {
              session: <code>{"${session_id}"}</code>,
            })}
            stack
          >
            <ResizableTextarea
              className="paths-input"
              value={headersDraft}
              onChange={(e) => setHeadersDraft(e.target.value)}
              onBlur={commitHeaders}
              placeholder={t("settings.provider.editor.headers.placeholder")}
              rows={2}
              spellCheck={false}
              aria-label={t("settings.provider.editor.headers")}
            />
          </Row>
        </Card>
      </Group>

      <Group title="API Key">
        <Card>
          <Row
            title={t("settings.provider.editor.key")}
            desc={
              savedFlash ? (
                <span className="key-state ok">{t("settings.provider.editor.key.saved")}</span>
              ) : keySource === "env" ? (
                <span className="key-state ok">
                  {tx("settings.provider.editor.key.env", { env: <code>{p.apiKeyEnv}</code> })}
                </span>
              ) : keySource === "saved" ? (
                <span className="key-state ok">{t("settings.provider.editor.key.savedOverride")}</span>
              ) : (
                <span className="key-state warn">{t("settings.provider.editor.key.missing")}</span>
              )
            }
            stack
          >
            <div className="key-row">
              <input
                type="password"
                value={keyDraft}
                onChange={(e) => setKeyDraft(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && void saveKey()}
                placeholder={t("settings.provider.editor.key.placeholder", { name: p.name })}
                autoComplete="off"
                spellCheck={false}
                aria-label="API key"
              />
              <button
                className="primary"
                onClick={() => void saveKey()}
                disabled={!keyDraft.trim()}
              >
                {t("common.save")}
              </button>
            </div>
          </Row>
        </Card>
      </Group>

      <Group
        title={t("settings.provider.editor.models")}
        action={
          <div className="set-group-actions">
            <button className="btn-compact" onClick={() => setAdding(true)}>
              {t("settings.provider.editor.addModel")}
            </button>
            {/* disabled 按钮吞掉 title，先决条件挂在外层 span 上才看得见 */}
            <span
              className="tip-wrap"
              title={!keySource ? t("settings.provider.editor.fetchNeedsKey") : undefined}
            >
              <button
                className="btn-compact"
                onClick={() => void doFetch()}
                disabled={fetching || !keySource}
              >
                {fetching ? t("settings.provider.editor.fetching") : t("settings.provider.editor.fetch")}
              </button>
            </span>
          </div>
        }
      >
        <Card>
        {p.models.length === 0 ? (
          <CardBlock>
            <p className="hint" style={{ margin: 0 }}>
              {t("settings.provider.editor.models.empty")}
            </p>
          </CardBlock>
        ) : null}
        {/* 圆点选的是「测试连接」要测的模型，仅此而已。对话用哪个模型在
            输入框上选，这里不替它做决定。 */}
        <div className="model-list" role="radiogroup" aria-label={t("settings.provider.editor.testModel")}>
          {p.models.map((m) => {
            const picked = m.id === testModel;
            return (
              <div key={m.id} className={picked ? "model-row active" : "model-row"}>
                <button
                  className="model-name"
                  role="radio"
                  aria-checked={picked}
                  onClick={() => setTestPick(m.id)}
                  title={picked ? t("settings.provider.editor.testModel") : t("settings.provider.editor.testWith")}
                >
                  <span className="model-radio">{picked ? "●" : "○"}</span>
                  <span className="model-label">
                    {m.name?.trim() || m.id}
                    {m.vision ? (
                      <span
                        className="cap-icon"
                        role="img"
                        aria-label={t("settings.provider.editor.vision.aria")}
                        title={t("settings.provider.editor.vision.title")}
                      >
                        <EyeIcon />
                      </span>
                    ) : null}
                  </span>
                  {m.name?.trim() ? <code className="model-id">{m.id}</code> : null}
                </button>
                <button
                  className="row-btn"
                  onClick={() => setEditing(m)}
                  title={t("settings.provider.editor.editModel")}
                >
                  <PencilIcon />
                </button>
                <button
                  className="row-btn"
                  onClick={() => removeModel(m.id)}
                  title={t("settings.provider.editor.removeFromList")}
                >
                  <CloseIcon />
                </button>
              </div>
            );
          })}
        </div>

        {fetched ? (
          <CardBlock>
            {fetched.length ? (
              <div className="fetched-list">
                {fetched.map((m) => {
                  const added = p.models.some((x) => x.id === m);
                  return (
                    <button
                      key={m}
                      className={added ? "fetched-item added" : "fetched-item"}
                      onClick={() => (added ? removeModel(m) : addModel(m))}
                      title={
                        added ? t("settings.provider.editor.clickRemove") : t("settings.provider.editor.clickAdd")
                      }
                    >
                      {added ? "✓ " : "+ "}
                      {m}
                    </button>
                  );
                })}
              </div>
            ) : (
              <p className="hint" style={{ margin: 0 }}>
                {t("settings.provider.editor.fetched.empty")}
              </p>
            )}
          </CardBlock>
        ) : null}
        </Card>
      </Group>

      {editing || adding ? (
        <ModelDialog
          provider={p}
          model={editing}
          onSave={saveModel}
          onClose={() => {
            setEditing(null);
            setAdding(false);
          }}
        />
      ) : null}

      <Group
        title={t("settings.provider.editor.sampling")}
        desc={t("settings.provider.editor.sampling.desc")}
      >
        <Card>
          <CardBlock>
            <SamplingSliders
              draft={sampDraft}
              hint
              onChange={(key, value) => setSampDraft((s) => ({ ...s, [key]: value }))}
              onCommit={commitSampling}
            />
          </CardBlock>
        </Card>
      </Group>

      <div className="editor-foot">
        {testResult ? (
          <span className={testResult.ok ? "test-result ok" : "test-result err"}>{testResult.text}</span>
        ) : (
          <span className="hint" style={{ margin: 0 }}>
            {/* 把要测的模型名写进提示里：列表里那个圆点和这个按钮隔着好几
                屏，不点名的话看不出两者是一回事。 */}
            {testModelCfg
              ? tx("settings.provider.editor.test.hintModel", {
                  model: <code>{testModelCfg.name?.trim() || testModelCfg.id}</code>,
                })
              : t("settings.provider.editor.test.hint")}
          </span>
        )}
        <div className="editor-foot-actions">
          {onRemove ? (
            <button className="btn-danger ghost-danger" onClick={onRemove}>
              {t("common.delete")}
            </button>
          ) : null}
          <span className="tip-wrap" title={!keySource ? t("settings.provider.editor.testNeedsKey") : undefined}>
            <button className="primary" onClick={() => void doTest()} disabled={testing || !keySource}>
              {testing ? t("settings.common.testing") : t("settings.provider.editor.test")}
            </button>
          </span>
        </div>
      </div>
    </>
  );
}

function EyeIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M1.8 8s2.4-4.5 6.2-4.5S14.2 8 14.2 8s-2.4 4.5-6.2 4.5S1.8 8 1.8 8z"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinejoin="round"
      />
      <circle cx="8" cy="8" r="1.9" stroke="currentColor" strokeWidth="1.3" />
    </svg>
  );
}

function PencilIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M11.2 2.8l2 2-7.6 7.6H3.6v-2L11.2 2.8z"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function CloseIcon() {
  return (
    <svg width="11" height="11" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M4 4l8 8M12 4l-8 8"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
      />
    </svg>
  );
}
