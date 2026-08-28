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
import {
  type SamplingDraft,
  parseSampling,
  sameSampling,
  samplingDraft,
} from "../../lib/sampling";
import { SamplingSliders } from "../FieldSlider";
import { ModelDialog } from "../ModelDialog";
import { Card, CardBlock, Group, Row } from "./layout";
import { type AskConfirm, blurOnEnter } from "./shared";

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
  // 文本字段走本地草稿、失焦提交。每敲一个字符就 IPC+写盘太吵。
  const [name, setName] = useState(p.name);
  const [baseUrl, setBaseUrl] = useState(p.baseUrl);
  const [apiPath, setApiPath] = useState(p.apiPath ?? "");
  const [keyDraft, setKeyDraft] = useState("");
  const [savedFlash, setSavedFlash] = useState(false);
  /** 正在编辑的模型。null = 没开弹窗。 */
  const [editing, setEditing] = useState<ModelConfig | null>(null);
  const [adding, setAdding] = useState(false);
  const [fetched, setFetched] = useState<string[] | null>(null);
  const [fetching, setFetching] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; text: string } | null>(null);
  const [sampDraft, setSampDraft] = useState<SamplingDraft>(() => samplingDraft(p.sampling));

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
      setSavedFlash(true);
      setTimeout(() => setSavedFlash(false), 2000);
    } catch (e) {
      onError(String(e));
    }
  };

  const activate = (model: string) => {
    void onCommit({ ...cfg, activeProvider: p.id, activeModel: model });
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
      title: `移除模型「${m}」？`,
      body: isActive
        ? "当前正在使用，移除后需要重新选一个才能发消息。"
        : "只从列表移除，随时可以加回来。",
      confirmLabel: "移除",
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
    // 测激活模型（如果属于这个 provider 且非空），否则测列表第一个。
    // 都没有就别发请求 —— 空模型名会换来一句各家措辞不一的 400，
    // 用户从那种报错里看不出"其实是没选模型"。
    const model =
      (cfg.activeProvider === p.id && cfg.activeModel) || p.models[0]?.id || "";
    if (!model) {
      setTestResult({ ok: false, text: "先添加一个模型（手动输入或从 API 获取）再测试。" });
      return;
    }
    setTesting(true);
    setTestResult(null);
    try {
      setTestResult({ ok: true, text: await testConnection(p.id, model) });
    } catch (e) {
      setTestResult({ ok: false, text: String(e) });
    } finally {
      setTesting(false);
    }
  };

  const isActive = cfg.activeProvider === p.id;

  return (
    <>
      <Group title="连接">
        <Card>
          <Row title="名称" desc="只在界面上显示，随便起。">
            <input
              value={name}
              onChange={(e) => setName(e.target.value)}
              onBlur={blurCommit}
              onKeyDown={blurOnEnter}
              autoFocus={autoFocusName}
              spellCheck={false}
              aria-label="名称"
            />
          </Row>
          <Row title="协议" desc="决定请求格式和认证头。选错了会被服务方拒绝。">
            <div className="radio-row" role="radiogroup" aria-label="协议">
              {(["openai", "anthropic"] as Protocol[]).map((proto) => (
                <button
                  key={proto}
                  role="radio"
                  aria-checked={p.protocol === proto}
                  className={p.protocol === proto ? "radio-pill active" : "radio-pill"}
                  onClick={() => void onPatch({ protocol: proto })}
                >
                  {proto === "openai" ? "OpenAI 兼容" : "Anthropic"}
                </button>
              ))}
            </div>
          </Row>
          <Row title="API 主机">
            <input
              value={baseUrl}
              onChange={(e) => setBaseUrl(e.target.value)}
              onBlur={blurCommit}
              onKeyDown={blurOnEnter}
              placeholder="https://api.example.com"
              spellCheck={false}
              aria-label="API 主机"
            />
          </Row>
          <Row
            title="API 路径"
            desc={
              <>
                留空按主机猜。接口不在常规位置时（如智谱的{" "}
                <code>/api/paas/v4/chat/completions</code>）在这里填。
              </>
            }
          >
            <input
              value={apiPath}
              onChange={(e) => setApiPath(e.target.value)}
              onBlur={blurCommit}
              onKeyDown={blurOnEnter}
              placeholder={defaultPath(p.protocol)}
              spellCheck={false}
              aria-label="API 路径"
            />
          </Row>
          {/* 把拼出来的完整地址摆出来。路径错一段的表现只是一个 404，
              报错里没有任何线索指向它 —— 而在这里一眼就能看出来。 */}
          <CardBlock className="url-preview-block">
            <span className="set-row-title">实际请求地址</span>
            <p className="url-preview">
              {joinUrl(baseUrl, apiPath.trim() || defaultPath(p.protocol))}
            </p>
          </CardBlock>
        </Card>
      </Group>

      <Group title="API Key">
        <Card>
          <Row
            title="密钥"
            desc={
              savedFlash ? (
                <span className="key-state ok">已保存。</span>
              ) : keySource === "env" ? (
                <span className="key-state ok">
                  正在使用环境变量 <code>{p.apiKeyEnv}</code>。
                </span>
              ) : keySource === "saved" ? (
                <span className="key-state ok">已保存。粘贴新的可以覆盖。</span>
              ) : (
                <span className="key-state warn">还没有配置，现在还不能发消息。</span>
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
                placeholder={`粘贴 ${p.name} 的 API key`}
                autoComplete="off"
                spellCheck={false}
                aria-label="API key"
              />
              <button
                className="primary"
                onClick={() => void saveKey()}
                disabled={!keyDraft.trim()}
              >
                保存
              </button>
            </div>
          </Row>
        </Card>
      </Group>

      <Group
        title="模型"
        action={
          <div className="set-group-actions">
            <button className="btn-compact" onClick={() => setAdding(true)}>
              添加模型…
            </button>
            {/* disabled 按钮吞掉 title，先决条件挂在外层 span 上才看得见 */}
            <span
              className="tip-wrap"
              title={!keySource ? "先在上面保存 API key，才能从接口获取模型列表" : undefined}
            >
              <button
                className="btn-compact"
                onClick={() => void doFetch()}
                disabled={fetching || !keySource}
              >
                {fetching ? "获取中…" : "从 API 获取"}
              </button>
            </span>
          </div>
        }
      >
        <Card>
        {p.models.length === 0 ? (
          <CardBlock>
            <p className="hint" style={{ margin: 0 }}>
              还没有模型。用右上角的「添加模型」手动填，或从 API 获取。
            </p>
          </CardBlock>
        ) : null}
        <div className="model-list" role="radiogroup" aria-label="当前模型">
          {p.models.map((m) => {
            const active = isActive && cfg.activeModel === m.id;
            return (
              <div key={m.id} className={active ? "model-row active" : "model-row"}>
                <button
                  className="model-name"
                  role="radio"
                  aria-checked={active}
                  onClick={() => activate(m.id)}
                  title={active ? "使用中" : "设为当前模型"}
                >
                  <span className="model-radio">{active ? "●" : "○"}</span>
                  <span className="model-label">
                    {m.name?.trim() || m.id}
                    {m.vision ? (
                      <span className="cap-icon" role="img" aria-label="能收图片" title="这个模型能收图片">
                        <EyeIcon />
                      </span>
                    ) : null}
                  </span>
                  {m.name?.trim() ? <code className="model-id">{m.id}</code> : null}
                </button>
                <button className="row-btn" onClick={() => setEditing(m)} title="编辑模型">
                  <PencilIcon />
                </button>
                <button className="row-btn" onClick={() => removeModel(m.id)} title="从列表移除">
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
                      title={added ? "点击移除" : "点击添加"}
                    >
                      {added ? "✓ " : "+ "}
                      {m}
                    </button>
                  );
                })}
              </div>
            ) : (
              <p className="hint" style={{ margin: 0 }}>
                这个服务方没有返回任何模型。
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
        title="采样参数"
        desc="这一家的默认值。写着「模型默认」的字段一个都不发，由模型自己定；模型没单独设的字段用这里的值，单个模型在它的编辑弹窗里改，对话里还能按会话临时覆盖。"
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
            发一个最小请求验证配置。
          </span>
        )}
        <div className="editor-foot-actions">
          {onRemove ? (
            <button className="btn-danger ghost-danger" onClick={onRemove}>
              删除
            </button>
          ) : null}
          <span className="tip-wrap" title={!keySource ? "先在上面保存 API key，才能测试连接" : undefined}>
            <button className="primary" onClick={() => void doTest()} disabled={testing || !keySource}>
              {testing ? "测试中…" : "测试连接"}
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
