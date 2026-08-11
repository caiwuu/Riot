import { useEffect, useState } from "react";

import {
  type AppConfig,
  type ConfigStatus,
  type PermissionMode,
  type Protocol,
  type ProviderConfig,
  type Sampling,
  type WebConfig,
  listModels,
  revealInFinder,
  setApiKey,
  setConfig,
  testConnection,
  testSearchBackend,
} from "../bridge";
import { ConfirmDialog, type ConfirmRequest } from "./ConfirmDialog";

interface Props {
  status: ConfigStatus;
  onStatus: (s: ConfigStatus) => void;
  onClose: () => void;
}

type AskConfirm = (req: ConfirmRequest) => void;

type Tab = "provider" | "web" | "permission" | "mcp" | "skills" | "about";

const TABS: { id: Tab; label: string }[] = [
  { id: "provider", label: "Provider" },
  { id: "web", label: "联网" },
  { id: "permission", label: "权限" },
  { id: "mcp", label: "MCP" },
  { id: "skills", label: "Skills" },
  { id: "about", label: "关于" },
];

/**
 * 设置弹层，左侧分区导航。
 *
 * 所有修改都提交整个 [`AppConfig`] —— 宿主在保存前 resolve 一次，
 * 把"active 指向不存在的 provider"这类坏状态挡在写盘之前。
 */
export function Settings({ status, onStatus, onClose }: Props) {
  const [tab, setTab] = useState<Tab>("provider");
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // 确认框打开时 Esc 只关确认，不连带关掉设置
      if (e.key === "Escape" && !confirm) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose, confirm]);

  return (
    <>
      <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
        <div className="modal settings">
          <button className="settings-close" onClick={onClose} title="关闭 (Esc)" aria-label="关闭">
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
              <path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
            </svg>
          </button>

          <div className="settings-nav">
            <div className="settings-nav-title">设置</div>
            {TABS.map((t) => (
              <button
                key={t.id}
                className={tab === t.id ? "settings-tab active" : "settings-tab"}
                onClick={() => setTab(t.id)}
              >
                {t.label}
                {(t.id === "mcp" || t.id === "skills") && <span className="tab-soon">规划中</span>}
              </button>
            ))}
          </div>

          <div className="settings-body">
            {/* 配置读不懂被回落成默认值时，用户看到的是"我配的东西全没了"。
                不在这儿说一句，他不会知道旁边躺着一份完好的备份。放在
                标签页外面：无论他点开哪一页都得看见。 */}
            {status.configBackup ? (
              <div className="recovered-note">
                <p className="empty-title">配置文件损坏</p>
                <p className="hint">已用默认设置启动，原文件备份在：</p>
                <code className="path">{status.configBackup}</code>
                <button onClick={() => void revealInFinder(status.configBackup ?? "")}>
                  在访达中显示
                </button>
              </div>
            ) : null}
            {tab === "provider" ? (
              <ProviderPane status={status} onStatus={onStatus} askConfirm={setConfirm} />
            ) : null}
            {tab === "web" ? <WebPane status={status} onStatus={onStatus} /> : null}
            {tab === "permission" ? (
              <PermissionPane status={status} onStatus={onStatus} askConfirm={setConfirm} />
            ) : null}
            {tab === "mcp" ? (
              <PlaceholderPane
                title="MCP 服务器"
                body="通过 MCP 接入外部工具。开发中。"
              />
            ) : null}
            {tab === "skills" ? (
              <PlaceholderPane
                title="Skills"
                body="把常用流程写成文档，模型按需加载。开发中。"
              />
            ) : null}
            {tab === "about" ? <AboutPane status={status} /> : null}
          </div>
        </div>
      </div>
      {confirm ? <ConfirmDialog c={confirm} onClose={() => setConfirm(null)} /> : null}
    </>
  );
}

/* ---------- Provider 分区 ---------- */

function ProviderPane({
  status,
  onStatus,
  askConfirm,
}: {
  status: ConfigStatus;
  onStatus: (s: ConfigStatus) => void;
  askConfirm: AskConfirm;
}) {
  const cfg = status.config;
  const [selId, setSelId] = useState(cfg.activeProvider);
  const [error, setError] = useState("");

  const sel = cfg.providers.find((p) => p.id === selId) ?? cfg.providers[0];

  const commit = async (next: AppConfig) => {
    setError("");
    try {
      onStatus(await setConfig(next));
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    }
  };

  const patchSel = (patch: Partial<ProviderConfig>) => {
    if (!sel) return;
    void commit({
      ...cfg,
      providers: cfg.providers.map((p) => (p.id === sel.id ? { ...p, ...patch } : p)),
    });
  };

  const addProvider = () => {
    const n = cfg.providers.length + 1;
    const id = `custom-${Date.now().toString(36)}`;
    const p: ProviderConfig = {
      id,
      name: `服务方 ${n}`,
      protocol: "openai",
      baseUrl: "",
      apiKeyEnv: `RIOT_KEY_${n}`,
      models: [],
      fallbackModel: null,
      sampling: {},
    };
    void commit({ ...cfg, providers: [...cfg.providers, p] }).then((ok) => {
      if (ok) setSelId(id);
    });
  };

  const removeProvider = () => {
    if (!sel) return;
    const target = sel;
    const last = cfg.providers.length === 1;
    askConfirm({
      title: `删除服务方「${target.name}」？`,
      body: last
        ? "删掉后要重新添加才能发消息。API key 不会被删除。"
        : target.id === cfg.activeProvider
          ? "会自动切换到下一个。API key 不会被删除。"
          : "API key 不会被删除。",
      confirmLabel: "删除",
      action: () => {
        const rest = cfg.providers.filter((p) => p.id !== target.id);
        const next: AppConfig = { ...cfg, providers: rest };
        // 删的是激活中的：切到剩余第一个。一个都不剩就置空 ——
        // 空 active 是宿主认可的合法状态（见 AppConfig::validate），
        // 留着指向已删对象的 id 才会被拒绝保存。
        if (cfg.activeProvider === target.id) {
          const first = rest[0];
          next.activeProvider = first?.id ?? "";
          next.activeModel = first?.models[0] ?? "";
        }
        void commit(next).then((ok) => {
          if (ok) setSelId(next.activeProvider);
        });
      },
    });
  };

  // 一个服务方都没有。不预置出厂数据的代价就是这个空状态必须做好 ——
  // 少了它这里是一片空白，用户看不出是"还没配"还是"设置页坏了"。
  if (!sel) {
    return (
      <section>
        <h2>服务方</h2>
        <div className="empty-state">
          <p className="empty-title">还没有服务方</p>
          <button className="primary" onClick={addProvider}>
            添加服务方
          </button>
        </div>
      </section>
    );
  }

  return (
    <>
      <section>
        <h2>服务方</h2>
        <div className="prov-tabs">
          {cfg.providers.map((p) => (
            <button
              key={p.id}
              className={p.id === sel.id ? "prov-tab active" : "prov-tab"}
              onClick={() => setSelId(p.id)}
            >
              {p.name}
              {p.id === cfg.activeProvider ? <span className="prov-dot" title="使用中" /> : null}
            </button>
          ))}
          <button className="prov-tab add" onClick={addProvider} title="添加服务方">
            +
          </button>
        </div>
      </section>

      <ProviderEditor
        key={sel.id}
        provider={sel}
        cfg={cfg}
        keySource={status.keyStatus[sel.id] ?? null}
        onPatch={patchSel}
        onCommit={commit}
        onStatus={onStatus}
        onRemove={removeProvider}
        askConfirm={askConfirm}
        onError={setError}
      />

      {error ? <p className="form-error">{error}</p> : null}
    </>
  );
}

const SAMPLING_FIELDS: {
  key: keyof Sampling;
  label: string;
  hint: string;
  step: string;
  integer?: boolean;
}[] = [
  { key: "temperature", label: "temperature", hint: "0–2。越高越发散。", step: "0.1" },
  { key: "topP", label: "top_p", hint: "0–1。一般不与 temperature 同调。", step: "0.05" },
  { key: "topK", label: "top_k", hint: "仅 Anthropic 协议发送。", step: "1", integer: true },
  { key: "maxOutputTokens", label: "max tokens", hint: "单次回复的输出上限。", step: "256", integer: true },
];

/** 把输入框草稿解析成采样值：空/非法 = null（不设置）。 */
function parseSampling(draft: Record<string, string>): Sampling {
  const num = (s: string | undefined, integer?: boolean) => {
    const t = (s ?? "").trim();
    if (!t) return null;
    const v = Number(t);
    if (!Number.isFinite(v)) return null;
    return integer ? Math.round(v) : v;
  };
  return {
    temperature: num(draft.temperature),
    topP: num(draft.topP),
    topK: num(draft.topK, true),
    maxOutputTokens: num(draft.maxOutputTokens, true),
  };
}

/**
 * 单个 provider 的编辑表单。
 *
 * `key={provider.id}` 让切换服务方时整个表单重挂载 —— 文本框的本地
 * 草稿不会串到另一个 provider 头上。
 */
function ProviderEditor({
  provider: p,
  cfg,
  keySource,
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
  onPatch: (patch: Partial<ProviderConfig>) => void;
  onCommit: (next: AppConfig) => Promise<boolean>;
  onStatus: (s: ConfigStatus) => void;
  onRemove: (() => void) | null;
  askConfirm: AskConfirm;
  onError: (e: string) => void;
}) {
  // 文本字段走本地草稿、失焦提交。每敲一个字符就 IPC+写盘太吵。
  const [name, setName] = useState(p.name);
  const [baseUrl, setBaseUrl] = useState(p.baseUrl);
  const [keyDraft, setKeyDraft] = useState("");
  const [savedFlash, setSavedFlash] = useState(false);
  const [modelDraft, setModelDraft] = useState("");
  const [fetched, setFetched] = useState<string[] | null>(null);
  const [fetching, setFetching] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; text: string } | null>(null);
  const [sampDraft, setSampDraft] = useState<Record<string, string>>({
    temperature: p.sampling.temperature?.toString() ?? "",
    topP: p.sampling.topP?.toString() ?? "",
    topK: p.sampling.topK?.toString() ?? "",
    maxOutputTokens: p.sampling.maxOutputTokens?.toString() ?? "",
  });

  const blurCommit = () => {
    const patch: Partial<ProviderConfig> = {};
    if (name.trim() && name.trim() !== p.name) patch.name = name.trim();
    const url = baseUrl.trim().replace(/\/+$/, "");
    if (url && url !== p.baseUrl) patch.baseUrl = url;
    if (Object.keys(patch).length) onPatch(patch);
  };

  const commitSampling = () => {
    const next = parseSampling(sampDraft);
    if (JSON.stringify(next) !== JSON.stringify(p.sampling)) onPatch({ sampling: next });
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
    const model = m.trim();
    if (!model || p.models.includes(model)) return;
    onPatch({ models: [...p.models, model] });
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
        const models = p.models.filter((x) => x !== m);
        // 删的是激活模型：清空 active，避免留下指向幽灵名字的配置
        if (isActive) {
          void onCommit({
            ...cfg,
            activeModel: "",
            providers: cfg.providers.map((x) => (x.id === p.id ? { ...x, models } : x)),
          });
        } else {
          onPatch({ models });
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
      (cfg.activeProvider === p.id && cfg.activeModel) || p.models[0] || "";
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
      <section>
        <div className="field-row">
          <label>名称</label>
          <input value={name} onChange={(e) => setName(e.target.value)} onBlur={blurCommit} spellCheck={false} />
        </div>
        <div className="field-row">
          <label>协议</label>
          <div className="radio-row">
            {(["openai", "anthropic"] as Protocol[]).map((proto) => (
              <button
                key={proto}
                className={p.protocol === proto ? "radio-pill active" : "radio-pill"}
                onClick={() => onPatch({ protocol: proto })}
              >
                {proto === "openai" ? "OpenAI 兼容" : "Anthropic"}
              </button>
            ))}
          </div>
        </div>
        <div className="field-row">
          <label>Base URL</label>
          <input
            value={baseUrl}
            onChange={(e) => setBaseUrl(e.target.value)}
            onBlur={blurCommit}
            placeholder="https://api.example.com"
            spellCheck={false}
          />
        </div>
        <p className="hint">OpenAI 兼容：DeepSeek、Kimi、vLLM、Ollama 及各家中转。</p>
      </section>

      <section>
        <h2>API Key</h2>
        {savedFlash ? (
          <p className="key-state ok">已保存。</p>
        ) : keySource === "env" ? (
          <p className="key-state ok">
            正在使用环境变量 <code>{p.apiKeyEnv}</code>。
          </p>
        ) : keySource === "saved" ? (
          <p className="key-state ok">已保存。粘贴新的可以覆盖。</p>
        ) : (
          <p className="key-state warn">还没有配置。</p>
        )}
        <div className="key-row">
          <input
            type="password"
            value={keyDraft}
            onChange={(e) => setKeyDraft(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && void saveKey()}
            placeholder={`粘贴 ${p.name} 的 API key`}
            autoComplete="off"
            spellCheck={false}
          />
          <button className="primary" onClick={() => void saveKey()} disabled={!keyDraft.trim()}>
            保存
          </button>
        </div>
      </section>

      <section>
        <h2>模型</h2>
        {p.models.length === 0 ? <p className="hint">还没有模型。手动输入，或从 API 获取。</p> : null}
        <div className="model-list">
          {p.models.map((m) => {
            const active = isActive && cfg.activeModel === m;
            return (
              <div key={m} className={active ? "model-row active" : "model-row"}>
                <button className="model-name" onClick={() => activate(m)} title={active ? "使用中" : "设为当前模型"}>
                  <span className="model-radio">{active ? "●" : "○"}</span>
                  {m}
                </button>
                <button className="row-btn" onClick={() => removeModel(m)} title="从列表移除">
                  ✕
                </button>
              </div>
            );
          })}
        </div>

        <div className="key-row">
          <input
            value={modelDraft}
            onChange={(e) => setModelDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.nativeEvent.isComposing) {
                addModel(modelDraft);
                setModelDraft("");
              }
            }}
            placeholder="手动输入模型名，回车添加"
            spellCheck={false}
          />
          <button
            onClick={() => {
              addModel(modelDraft);
              setModelDraft("");
            }}
            disabled={!modelDraft.trim()}
          >
            添加
          </button>
          <button onClick={() => void doFetch()} disabled={fetching || !keySource}>
            {fetching ? "获取中…" : "从 API 获取"}
          </button>
        </div>

        {fetched ? (
          fetched.length ? (
            <div className="fetched-list">
              {fetched.map((m) => {
                const added = p.models.includes(m);
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
            <p className="hint">这个服务方没有返回任何模型。</p>
          )
        ) : null}
      </section>

      <section>
        <h2>采样参数</h2>
        <p className="hint">留空用服务端默认。对话里可按会话覆盖。</p>
        {SAMPLING_FIELDS.map((f) => (
          <div className="field-row" key={f.key}>
            <label>{f.label}</label>
            <input
              type="number"
              step={f.step}
              value={sampDraft[f.key] ?? ""}
              onChange={(e) => setSampDraft({ ...sampDraft, [f.key]: e.target.value })}
              onBlur={commitSampling}
              placeholder="默认"
              spellCheck={false}
            />
            <span className="field-hint">{f.hint}</span>
          </div>
        ))}
      </section>

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
          <button className="primary" onClick={() => void doTest()} disabled={testing || !keySource}>
            {testing ? "测试中…" : "测试连接"}
          </button>
        </div>
      </div>
    </>
  );
}

/* ---------- 联网分区 ---------- */

/**
 * 抓取、搜索、蒸馏三块。
 *
 * 排布顺序对应用户配置的顺序：先决定让不让上网，再配搜索后端，
 * 最后是可选的辅助模型。把辅助模型放前面会让人以为它是必填项。
 */
function WebPane({ status, onStatus }: { status: ConfigStatus; onStatus: (s: ConfigStatus) => void }) {
  const web = status.config.web;
  const [url, setUrl] = useState(web.searxngUrl);
  const [error, setError] = useState("");
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; text: string } | null>(null);

  const patch = async (p: Partial<WebConfig>) => {
    setError("");
    try {
      onStatus(await setConfig({ ...status.config, web: { ...web, ...p } }));
    } catch (e) {
      setError(String(e));
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
        <h2>网页抓取</h2>
        <Toggle
          on={web.fetchEnabled}
          onChange={(v) => void patch({ fetchEnabled: v })}
          label="允许模型抓取网页（WebFetch）"
        />
        <p className="hint">首次访问每个域名会询问。内网地址一律拒绝。</p>
      </section>

      <section>
        <h2>搜索</h2>
        <Toggle
          on={web.searchEnabled}
          onChange={(v) => void patch({ searchEnabled: v })}
          label="允许模型联网搜索（WebSearch）"
        />
        <div className="field-row">
          <label>SearXNG</label>
          <input
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            onBlur={() => url.trim() !== web.searxngUrl && void patch({ searxngUrl: url.trim() })}
            placeholder="http://127.0.0.1:8080"
            spellCheck={false}
            disabled={!web.searchEnabled}
          />
        </div>
        <p className="hint">
          需自建实例，公共实例会限流。要求 <code>server.limiter: false</code>，
          且 <code>search.formats</code> 含 <code>json</code>。
        </p>
        <div className="editor-foot">
          {testResult ? (
            <span className={testResult.ok ? "test-result ok" : "test-result err"}>{testResult.text}</span>
          ) : (
            <span className="hint" style={{ margin: 0 }}>
              会真发一次查询。
            </span>
          )}
          <div className="editor-foot-actions">
            <button className="primary" onClick={() => void doTest()} disabled={testing || !url.trim()}>
              {testing ? "测试中…" : "测试"}
            </button>
          </div>
        </div>
      </section>

      <section>
        <h2>正文蒸馏</h2>
        <div className="field-row">
          <label>辅助模型</label>
          <select
            value={allModels.some((m) => m.value === web.distillModel) ? web.distillModel : ""}
            onChange={(e) => void patch({ distillModel: e.target.value })}
          >
            <option value="">不蒸馏（返回截断的正文）</option>
            {allModels.map((m) => (
              <option key={m.value} value={m.value}>
                {m.label}
              </option>
            ))}
          </select>
        </div>
        <p className="hint">用便宜的模型把网页压成摘要，省上下文。留空则直接截断。</p>
        {web.distillModel && !allModels.some((m) => m.value === web.distillModel) ? (
          <p className="key-state warn">
            <code>{web.distillModel}</code> 已不存在，当前不会蒸馏。
          </p>
        ) : null}
      </section>

      {error ? <p className="form-error">{error}</p> : null}
    </>
  );
}

function Toggle({ on, onChange, label }: { on: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <button className="toggle-row" onClick={() => onChange(!on)} role="switch" aria-checked={on}>
      <span className={on ? "toggle-track on" : "toggle-track"}>
        <span className="toggle-knob" />
      </span>
      <span className="toggle-label">{label}</span>
    </button>
  );
}

/* ---------- 权限分区 ---------- */

/** 和宿主侧 config::normalize 的夹紧区间保持一致。 */
const MIN_TIMEOUT = 5;
const MAX_TIMEOUT = 3600;

const MODES: { id: PermissionMode; name: string; desc: string; danger?: boolean }[] = [
  { id: "default", name: "每次询问", desc: "写文件、执行命令前询问。" },
  { id: "acceptEdits", name: "自动接受编辑", desc: "文件修改放行，命令仍询问。" },
  {
    id: "bypassPermissions",
    name: "全部放行",
    // 必须点出"仍会拦"。写成"所有操作不再询问"是假承诺：用户照着这句话
    // 挂机走人，回来发现任务停在一个弹窗上。
    desc: "常规操作不再询问，危险操作仍会拦。",
    danger: true,
  },
  {
    id: "unattended",
    name: "无人值守",
    desc: "全部放行，包括危险操作。仅限一次性环境。",
    danger: true,
  },
];

function PermissionPane({
  status,
  onStatus,
  askConfirm,
}: {
  status: ConfigStatus;
  onStatus: (s: ConfigStatus) => void;
  askConfirm: (c: ConfirmRequest) => void;
}) {
  const [error, setError] = useState("");
  const current = status.config.defaultMode ?? "default";
  // 编辑期间存字符串：绑成 number 的话，用户删到空输入框会立刻变成 0，
  // 而 0 在这里的含义是"每个弹窗瞬间超时"。等失焦再解析并夹紧。
  const [timeout, setTimeout_] = useState(String(status.config.askTimeoutSecs));

  const commitTimeout = () => {
    const n = Number.parseInt(timeout, 10);
    const v = Number.isFinite(n) ? Math.min(Math.max(n, MIN_TIMEOUT), MAX_TIMEOUT) : status.config.askTimeoutSecs;
    setTimeout_(String(v));
    if (v === status.config.askTimeoutSecs) return;
    setError("");
    setConfig({ ...status.config, askTimeoutSecs: v })
      .then(onStatus)
      .catch((e: unknown) => setError(String(e)));
  };

  const apply = async (mode: PermissionMode) => {
    setError("");
    try {
      onStatus(await setConfig({ ...status.config, defaultMode: mode }));
    } catch (e) {
      setError(String(e));
    }
  };

  const pick = (mode: PermissionMode) => {
    // 无人值守要额外确认一次。它是唯一一个连安全检查都关掉的模式，
    // 而且这里设的是**新会话的默认值** —— 手滑点中的话，之后每个新
    // 会话都不设防，且没有任何弹窗会再提醒。
    if (mode === "unattended" && current !== "unattended") {
      askConfirm({
        title: "把默认模式设为无人值守？",
        body: "之后新建的会话都会跳过全部权限检查，包括危险操作。",
        confirmLabel: "确认",
        action: () => void apply(mode),
      });
      return;
    }
    void apply(mode);
  };

  return (
    <>
      <section>
        <h2>新会话的默认模式</h2>
        <p className="hint">只影响之后创建的会话。当前会话在输入框左下角切换。</p>
        <div className="mode-cards">
          {MODES.map((m) => (
            <button
              key={m.id}
              className={current === m.id ? "mode-card active" : "mode-card"}
              onClick={() => pick(m.id)}
            >
              <span className="mode-card-name">
                {m.name}
                {m.danger ? <span className="mode-card-flag">高风险</span> : null}
              </span>
              <span className="mode-card-desc">{m.desc}</span>
            </button>
          ))}
        </div>
      </section>
      <section>
        <h2>等待授权的时间</h2>
        <p className="hint">弹窗多久没人回应就放弃。超时按拒绝处理。</p>
        <label className="field-inline">
          <input
            type="number"
            min={MIN_TIMEOUT}
            max={MAX_TIMEOUT}
            value={timeout}
            onChange={(e) => setTimeout_(e.target.value)}
            onBlur={commitTimeout}
          />
          <span className="field-unit">秒</span>
        </label>
      </section>
      <section>
        <h2>会话内规则</h2>
        <p className="hint">
          点「总是允许」记住的规则（如 <code>Bash(npm run *)</code>）只在当前会话有效。
        </p>
      </section>
      {error ? <p className="form-error">{error}</p> : null}
    </>
  );
}

/* ---------- 占位与关于 ---------- */

function PlaceholderPane({ title, body }: { title: string; body: string }) {
  return (
    <section>
      <h2>{title}</h2>
      <p className="hint">{body}</p>
    </section>
  );
}

function AboutPane({ status }: { status: ConfigStatus }) {
  const configDir = status.configPath.replace(/\/[^/]*$/, "");
  return (
    <>
      <section>
        <h2>Riot</h2>
        <p className="hint">本地 coding agent。Tauri + Rust。</p>
      </section>
      <section>
        <h2>配置文件</h2>
        <div className="about-row">
          <code>{status.configPath}</code>
          <button className="ghost" onClick={() => void revealInFinder(configDir)}>
            在访达中显示
          </button>
        </div>
        <p className="hint">
          API key 单独存在同目录的 <code>auth.json</code>。
        </p>
      </section>
    </>
  );
}
