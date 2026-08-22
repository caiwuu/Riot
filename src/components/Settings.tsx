import { type KeyboardEvent, useCallback, useEffect, useRef, useState } from "react";

import {
  type AppConfig,
  type ModelConfig,
  type ConfigStatus,
  type HookInfo,
  type McpServerConfig,
  type McpServerStatus,
  type PackProgress,
  type PackStatus,
  type PermissionMode,
  type Protocol,
  type ProviderConfig,
  type Sampling,
  type SandboxMode,
  type SkillInfo,
  type SlashCommand,
  type WebConfig,
  hooksList,
  listModels,
  mcpExportJson,
  mcpImportJson,
  mcpRestart,
  mcpStatus,
  packsStatus,
  packsUninstall,
  revealInFinder,
  setApiKey,
  setConfig,
  skillsList,
  slashCommands,
  testConnection,
  testSearchBackend,
} from "../bridge";
import {
  clearDonePackProgress,
  clearPackProgress,
  reportPackFailure,
  startPackInstall,
  usePackInstalls,
} from "../hooks/usePackInstalls";
import { FieldNumber } from "./FieldNumber";
import { FieldSelect } from "./FieldSelect";
import { HintTip } from "./HintTip";
import { ModelDialog } from "./ModelDialog";
import { ConfirmDialog, type ConfirmRequest } from "./ConfirmDialog";
import { Modal } from "./Modal";

interface Props {
  status: ConfigStatus;
  onStatus: (s: ConfigStatus) => void;
  onClose: () => void;
  /** 当前会话的项目根。Skills 页用它列项目级技能；没有活跃会话时为 null。 */
  activeRoot?: string | null;
}

type AskConfirm = (req: ConfirmRequest) => void;

/**
 * 离开当前分区前的拦截：返回 null 放行，返回确认内容则先问一句。
 * 目前只有 MCP 的 JSON 视图用 —— 那里可能躺着用户刚粘贴、还没保存的
 * 一整段配置，Esc/点遮罩/切标签任何一条路都不该无声地丢掉它。
 */
type LeaveGuard = () => Omit<ConfirmRequest, "action"> | null;

type Tab =
  | "provider"
  | "web"
  | "permission"
  | "mcp"
  | "packs"
  | "skills"
  | "commands"
  | "hooks"
  | "about";

const TABS: { id: Tab; label: string }[] = [
  { id: "provider", label: "Provider" },
  { id: "web", label: "联网" },
  { id: "permission", label: "权限" },
  { id: "mcp", label: "MCP" },
  { id: "packs", label: "能力包" },
  { id: "skills", label: "Skills" },
  { id: "commands", label: "命令" },
  { id: "hooks", label: "Hooks" },
  { id: "about", label: "关于" },
];

/** "失焦提交"的单行输入统一支持回车：Enter → blur，提交仍走 onBlur 一条路。 */
function blurOnEnter(e: KeyboardEvent<HTMLInputElement>) {
  if (e.key === "Enter") e.currentTarget.blur();
}

/** 底部错误行。出现时滚进视野 —— 长页面里它可能在两屏之外，等于没报。 */
function FormError({ text }: { text: string }) {
  const ref = useRef<HTMLParagraphElement>(null);
  useEffect(() => {
    ref.current?.scrollIntoView({ block: "nearest" });
  }, [text]);
  return (
    <p ref={ref} className="form-error">
      {text}
    </p>
  );
}

/**
 * 设置弹层，左侧分区导航。
 *
 * 所有修改都提交整个 [`AppConfig`] —— 宿主在保存前 resolve 一次，
 * 把"active 指向不存在的 provider"这类坏状态挡在写盘之前。
 */
export function Settings({ status, onStatus, onClose, activeRoot }: Props) {
  const [tab, setTab] = useState<Tab>("provider");
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);

  /** 「已保存 ✓」瞬时提示。计数器当 key：连续保存也能重启淡出动画。 */
  const [savedTick, setSavedTick] = useState(0);
  const flashSaved = useCallback(() => setSavedTick((t) => t + 1), []);

  /** 当前分区注册的离开拦截。ref 而不是 state：它只在离开的瞬间被读一次。 */
  const leaveGuard = useRef<LeaveGuard | null>(null);
  const registerLeaveGuard = useCallback((g: LeaveGuard | null) => {
    leaveGuard.current = g;
  }, []);

  /** 关闭 / 切分区都从这儿走：有未保存的内容就先问，没有就直接做。 */
  const guarded = useCallback(
    (proceed: () => void) => {
      const ask = leaveGuard.current?.();
      if (ask) {
        setConfirm({
          ...ask,
          action: () => {
            leaveGuard.current = null;
            proceed();
          },
        });
      } else {
        proceed();
      }
    },
    [],
  );
  const requestClose = useCallback(() => {
    // 关窗前把焦点从输入框上拿走，让"失焦提交"的字段先落地 ——
    // 不做的话，正在编辑的 baseUrl/系统提示词随组件卸载无声蒸发。
    (document.activeElement as HTMLElement | null)?.blur?.();
    guarded(onClose);
  }, [guarded, onClose]);

  return (
    <>
      <Modal className="settings" label="设置" onClose={requestClose}>
          <div className="settings-head">
            <span className="settings-head-title">设置</span>
            <button className="settings-close" onClick={requestClose} title="关闭 (Esc)" aria-label="关闭">
              <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
                <path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
              </svg>
            </button>
          </div>

          <div className="settings-main">
            <div className="settings-nav" role="tablist" aria-label="设置分区">
              {TABS.map((t) => (
                <button
                  key={t.id}
                  role="tab"
                  aria-selected={tab === t.id}
                  className={tab === t.id ? "settings-tab active" : "settings-tab"}
                  onClick={() => guarded(() => setTab(t.id))}
                >
                  {t.label}
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
              <ProviderPane
                status={status}
                onStatus={onStatus}
                askConfirm={setConfirm}
                onSaved={flashSaved}
              />
            ) : null}
            {tab === "web" ? (
              <WebPane status={status} onStatus={onStatus} onSaved={flashSaved} />
            ) : null}
            {tab === "permission" ? (
              <PermissionPane
                status={status}
                onStatus={onStatus}
                askConfirm={setConfirm}
                onSaved={flashSaved}
              />
            ) : null}
            {tab === "mcp" ? (
              <McpPane
                status={status}
                onStatus={onStatus}
                askConfirm={setConfirm}
                registerLeaveGuard={registerLeaveGuard}
                onSaved={flashSaved}
              />
            ) : null}
            {tab === "packs" ? <PacksPane askConfirm={setConfirm} /> : null}
            {tab === "skills" ? (
              <SkillsPane status={status} activeRoot={activeRoot ?? null} />
            ) : null}
            {tab === "commands" ? (
              <CommandsPane status={status} activeRoot={activeRoot ?? null} />
            ) : null}
            {tab === "hooks" ? (
              <HooksPane status={status} activeRoot={activeRoot ?? null} />
            ) : null}
            {tab === "about" ? <AboutPane status={status} /> : null}
            </div>
          </div>
          {/* 低调的保存回执：各 Pane 的失焦提交原本全程静默，成功与否只能猜。 */}
          {savedTick > 0 ? (
            <span key={savedTick} className="save-flash" role="status">
              已保存 ✓
            </span>
          ) : null}
      </Modal>
      {confirm ? <ConfirmDialog c={confirm} onClose={() => setConfirm(null)} /> : null}
    </>
  );
}

/* ---------- Provider 分区 ---------- */

function ProviderPane({
  status,
  onStatus,
  askConfirm,
  onSaved,
}: {
  status: ConfigStatus;
  onStatus: (s: ConfigStatus) => void;
  askConfirm: AskConfirm;
  onSaved: () => void;
}) {
  const cfg = status.config;
  const [selId, setSelId] = useState(cfg.activeProvider);
  /** 刚新建的服务方：编辑器聚焦到名称，省得用户自己找第一个待填字段。 */
  const [justAdded, setJustAdded] = useState("");
  const [error, setError] = useState("");

  const sel = cfg.providers.find((p) => p.id === selId) ?? cfg.providers[0];

  const commit = async (next: AppConfig) => {
    setError("");
    try {
      onStatus(await setConfig(next));
      onSaved();
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    }
  };

  // 把成功与否交回给调用方 —— 失焦提交失败时，编辑器要用它决定是否回滚草稿。
  const patchSel = (patch: Partial<ProviderConfig>) => {
    if (!sel) return Promise.resolve(false);
    return commit({
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
    // 第一家服务方直接设为当前。active 留空的话（validate 放行），主界面
    // 显示会拿 providers[0] 兜底，key 状态却按空 id 查 —— 两边说的不是
    // 同一家，表现为「key 已保存，横幅还说没配」。
    void commit({
      ...cfg,
      providers: [...cfg.providers, p],
      ...(cfg.activeProvider ? {} : { activeProvider: id }),
    }).then((ok) => {
      if (ok) {
        setSelId(id);
        setJustAdded(id);
      }
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
          next.activeModel = first?.models[0]?.id ?? "";
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
      {/* 全局设置放在最上面，而且不在任何一个服务方的编辑器里面 ——
          放在下面的话它看着就像"当前这家的设置"，而它管的是所有模型。 */}
      <VisionFallback cfg={cfg} onCommit={commit} />
      <SubagentModel cfg={cfg} onCommit={commit} />

      <section>
        <h2>服务方</h2>
        <div className="prov-tabs">
          {cfg.providers.map((p) => (
            <button
              key={p.id}
              className={p.id === sel.id ? "prov-tab active" : "prov-tab"}
              onClick={() => {
                setSelId(p.id);
                setJustAdded("");
              }}
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
        autoFocusName={sel.id === justAdded}
        onPatch={patchSel}
        onCommit={commit}
        onStatus={onStatus}
        onRemove={removeProvider}
        askConfirm={askConfirm}
        onError={setError}
      />

      {error ? <FormError text={error} /> : null}
    </>
  );
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
 * 视觉兼容:给收不了图片的模型配一个"眼睛"。
 *
 * 放在服务方这一页而不是联网页:它和「支持图片」那个开关是一对，两者分开
 * 摆的话，用户勾了开关却不知道还有个兜底可以配。
 *
 * `[约束]` 常驻显示，不跟当前激活模型联动。早先当前模型能看图时把选择器
 * 藏起来，结果"全局配置随着切换模型时隐时现"更像 bug —— 生效与否由内核
 * 判（`vision_target`），界面只负责说明"只对没勾「看图」的模型生效"。
 */
function VisionFallback({
  cfg,
  onCommit,
}: {
  cfg: AppConfig;
  onCommit: (next: AppConfig) => Promise<boolean>;
}) {
  // 候选只列**勾了「看图」的模型** —— 让用户从纯文本模型里挑一个当"眼睛"，
  // 选完之后每次截图都是一个 400，而报错会在另一个地方冒出来。
  const options = cfg.providers.flatMap((p) =>
    p.models
      .filter((m) => m.vision)
      .map((m) => ({
        value: `${p.id}/${m.id}`,
        label: `${p.name} · ${m.name?.trim() || m.id}`,
      })),
  );
  const known = options.some((o) => o.value === cfg.visionModel);

  return (
    <section>
      <h2>
        视觉兼容（全局）
        <HintTip>
          只对没勾「看图」的模型生效：先让这里配的模型看图、转成文字，再交给主模型。
          主模型自己能收图时不走这条路，直接发原图。转述有损，精确像素判断不要依赖它。
        </HintTip>
      </h2>
      <div className="field-row">
        <label>兼容模型</label>
        <FieldSelect
          value={known ? cfg.visionModel : ""}
          onChange={(v) => void onCommit({ ...cfg, visionModel: v })}
          disabled={options.length === 0}
          options={[
            { value: "", label: "不转（截图工具会说用不了）" },
            ...options,
          ]}
        />
      </div>
      {options.length === 0 ? (
        <p className="hint">
          还没有标记为能看图的模型。先在上面的模型列表里给一个视觉模型点上「看图」。
        </p>
      ) : null}
      {cfg.visionModel && !known ? (
        <p className="key-state warn">
          <code>{cfg.visionModel}</code> 已不可用（模型被删了，或者它的「看图」被
          取消了），当前不会转述。
        </p>
      ) : null}
    </section>
  );
}

/**
 * 子 agent 的便宜档。
 *
 * 和视觉兼容摆在一起:两者都是"主模型之外再指一个模型"，形状一样
 * （`providerId/model`），失效方式也一样（指向的模型被删掉）。
 */
function SubagentModel({
  cfg,
  onCommit,
}: {
  cfg: AppConfig;
  onCommit: (next: AppConfig) => Promise<boolean>;
}) {
  // 这里不筛模型：任何模型都能读文件、写报告。视觉兼容要筛是因为
  // 挑错了每次截图都是个 400，而这里挑错了只是慢一点或笨一点。
  const options = cfg.providers.flatMap((p) =>
    p.models.map((m) => ({
      value: `${p.id}/${m.id}`,
      label: `${p.name} · ${m.name?.trim() || m.id}`,
    })),
  );
  const activeValue = `${cfg.activeProvider}/${cfg.activeModel}`;
  const known = options.some((o) => o.value === cfg.subagentModel);

  return (
    <section>
      <h2>
        子 agent 便宜档（全局）
        <HintTip>
          只读侦察的子 agent 走这一档。翻代码、写报告不改东西，但搜索结果全进上下文，往往更吃
          token。会改代码的子 agent 始终用主模型。
        </HintTip>
      </h2>
      <div className="field-row">
        <label>侦察模型</label>
        <FieldSelect
          value={known ? cfg.subagentModel : ""}
          onChange={(v) => void onCommit({ ...cfg, subagentModel: v })}
          disabled={options.length === 0}
          options={[
            { value: "", label: "跟主模型" },
            ...options.filter((o) => o.value !== activeValue),
          ]}
        />
      </div>
      {cfg.subagentModel && !known ? (
        <p className="key-state warn">
          <code>{cfg.subagentModel}</code> 已不可用（模型被删了），侦察当前走主模型。
        </p>
      ) : null}
    </section>
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

/** 采样值转回输入框草稿。初始化和"提交失败回滚"共用同一条真值来源。 */
function samplingDraft(s: Sampling): Record<string, string> {
  return {
    temperature: s.temperature?.toString() ?? "",
    topP: s.topP?.toString() ?? "",
    topK: s.topK?.toString() ?? "",
    maxOutputTokens: s.maxOutputTokens?.toString() ?? "",
  };
}

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
  const [sampDraft, setSampDraft] = useState<Record<string, string>>(() =>
    samplingDraft(p.sampling),
  );

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

  const commitSampling = () => {
    const next = parseSampling(sampDraft);
    if (JSON.stringify(next) === JSON.stringify(p.sampling)) return;
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
      <section>
        <div className="field-row">
          <label>名称</label>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            onBlur={blurCommit}
            onKeyDown={blurOnEnter}
            autoFocus={autoFocusName}
            spellCheck={false}
          />
        </div>
        <div className="field-row">
          <label>协议</label>
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
        </div>
        <div className="field-row">
          <label>API 主机</label>
          <input
            value={baseUrl}
            onChange={(e) => setBaseUrl(e.target.value)}
            onBlur={blurCommit}
            onKeyDown={blurOnEnter}
            placeholder="https://api.example.com"
            spellCheck={false}
          />
        </div>
        <div className="field-row">
          <label>
            API 路径
            <HintTip>
              OpenAI 兼容：DeepSeek、Kimi、vLLM、Ollama 及各家中转。路径留空按主机猜；接口不在常规位置时（如智谱的{" "}
              <code>/api/paas/v4/chat/completions</code>）在这里填。
            </HintTip>
          </label>
          <input
            value={apiPath}
            onChange={(e) => setApiPath(e.target.value)}
            onBlur={blurCommit}
            onKeyDown={blurOnEnter}
            placeholder={defaultPath(p.protocol)}
            spellCheck={false}
          />
        </div>
        {/* 把拼出来的完整地址摆出来。路径错一段的表现只是一个 404，
            报错里没有任何线索指向它 —— 而在这里一眼就能看出来。 */}
        <p className="url-preview">
          {joinUrl(baseUrl, apiPath.trim() || defaultPath(p.protocol))}
        </p>
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

        <div className="key-row">
          <button onClick={() => setAdding(true)}>添加模型…</button>
          {/* disabled 按钮吞掉 title，先决条件挂在外层 span 上才看得见 */}
          <span className="tip-wrap" title={!keySource ? "先在上面保存 API key，才能从接口获取模型列表" : undefined}>
            <button onClick={() => void doFetch()} disabled={fetching || !keySource}>
              {fetching ? "获取中…" : "从 API 获取"}
            </button>
          </span>
        </div>

        {fetched ? (
          fetched.length ? (
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
            <p className="hint">这个服务方没有返回任何模型。</p>
          )
        ) : null}
      </section>

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

      <section>
        <h2>
          采样参数（这一家的默认值）
          <HintTip>
            模型没单独设的字段用这里的值。单个模型在它的编辑弹窗里改；对话里还能按会话临时覆盖。
          </HintTip>
        </h2>
        {SAMPLING_FIELDS.map((f) => (
          <div className="field-row" key={f.key}>
            <label>
              {f.label}
              <HintTip>{f.hint}</HintTip>
            </label>
            <FieldNumber
              value={sampDraft[f.key] ?? ""}
              onChange={(e) => setSampDraft({ ...sampDraft, [f.key]: e.target.value })}
              onBlur={commitSampling}
              onKeyDown={blurOnEnter}
              placeholder="默认"
            />
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

/* ---------- 联网分区 ---------- */

/**
 * 抓取、搜索、蒸馏三块。
 *
 * 排布顺序对应用户配置的顺序：先决定让不让上网，再配搜索后端，
 * 最后是可选的辅助模型。把辅助模型放前面会让人以为它是必填项。
 */
function WebPane({
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
            SearXNG
            <HintTip>
              需自建实例，公共实例会限流。要求 <code>server.limiter: false</code>，且{" "}
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
              placeholder="http://127.0.0.1:8080"
              spellCheck={false}
              disabled={!web.searchEnabled}
            />
            <span
              className="tip-wrap"
              title={
                !web.searchEnabled
                  ? "先打开上面的搜索开关"
                  : !url.trim()
                    ? "先填写 SearXNG 地址"
                    : "会真发一次查询"
              }
            >
              <button
                className="btn-compact"
                onClick={() => void doTest()}
                disabled={testing || !web.searchEnabled || !url.trim()}
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
const MIN_TURNS = 1;
const MAX_TURNS = 1000;
const DEFAULT_TURNS = 48;
/** 和宿主的 MIN/MAX_COMPACT_THRESHOLD 一致。 */
const MIN_COMPACT_AT = 8_000;
const MAX_COMPACT_AT = 1_000_000;
const DEFAULT_COMPACT_AT = 100_000;

const SANDBOX_MODES: { id: SandboxMode; name: string; desc: string; danger?: boolean }[] = [
  {
    id: "workspaceWrite",
    name: "隔离（推荐）",
    desc: "只能改工作区和构建缓存，读和联网不受限。",
  },
  {
    id: "workspaceWriteNoNet",
    name: "隔离并断网",
    desc: "另外掐掉命令的网络。npm、cargo 拉依赖会失败。",
  },
  {
    id: "off",
    name: "不隔离",
    desc: "命令能改任何文件，只剩规则判断拦着。",
    danger: true,
  },
];

const MODES: { id: PermissionMode; name: string; desc: string; danger?: boolean }[] = [
  { id: "default", name: "每次询问", desc: "写文件、执行命令前询问。" },
  { id: "acceptEdits", name: "自动接受编辑", desc: "文件修改放行，命令仍询问。" },
  {
    id: "plan",
    name: "规划模式",
    desc: "只读侦察并产出计划，批准后才动手。",
  },
  {
    id: "auto",
    name: "自动判危",
    // 不写"自动放行安全操作"就完了 —— 用户会以为它替他做了全部判断。
    // 要点出两件事：靠的是小模型（所以要配便宜档），以及它压不过安全检查。
    desc: "小模型先判一遍，明确安全的不再问；安全检查与你写的规则仍然拦。需要配「子 agent 便宜档」。",
  },
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
  onSaved,
}: {
  status: ConfigStatus;
  onStatus: (s: ConfigStatus) => void;
  askConfirm: (c: ConfirmRequest) => void;
  onSaved: () => void;
}) {
  const [error, setError] = useState("");
  const current = status.config.defaultMode ?? "default";
  // 编辑期间存字符串：绑成 number 的话，用户删到空输入框会立刻变成 0，
  // 而 0 在这里的含义是"每个弹窗瞬间超时"。等失焦再解析并夹紧。
  const [timeout, setTimeout_] = useState(String(status.config.askTimeoutSecs));
  // 轮数默认 48，老配置里可能没有这个字段（后端有默认，但前端要兜一下）。
  const [turns, setTurns] = useState(String(status.config.maxTurns ?? DEFAULT_TURNS));
  const [compactAt, setCompactAt] = useState(
    String(status.config.compactThresholdTokens ?? DEFAULT_COMPACT_AT),
  );

  // 夹紧发生时在字段旁说一声 —— 不说的话，99999 无声变 3600 像是输入被吞了。
  const [clamp, setClamp] = useState<{ key: string; text: string } | null>(null);
  const clampTimer = useRef(0);
  const noteClamp = (key: string, raw: number, v: number) => {
    if (raw === v) return;
    setClamp({ key, text: raw > v ? `已调整为最大值 ${v}` : `已调整为最小值 ${v}` });
    window.clearTimeout(clampTimer.current);
    clampTimer.current = window.setTimeout(() => setClamp(null), 2500);
  };

  const saved = (s: ConfigStatus) => {
    onStatus(s);
    onSaved();
  };

  const commitTimeout = () => {
    const n = Number.parseInt(timeout, 10);
    const v = Number.isFinite(n) ? Math.min(Math.max(n, MIN_TIMEOUT), MAX_TIMEOUT) : status.config.askTimeoutSecs;
    if (Number.isFinite(n)) noteClamp("timeout", n, v);
    setTimeout_(String(v));
    if (v === status.config.askTimeoutSecs) return;
    setError("");
    setConfig({ ...status.config, askTimeoutSecs: v })
      .then(saved)
      .catch((e: unknown) => setError(String(e)));
  };

  const commitTurns = () => {
    const cur = status.config.maxTurns ?? DEFAULT_TURNS;
    const n = Number.parseInt(turns, 10);
    const v = Number.isFinite(n) ? Math.min(Math.max(n, MIN_TURNS), MAX_TURNS) : cur;
    if (Number.isFinite(n)) noteClamp("turns", n, v);
    setTurns(String(v));
    if (v === cur) return;
    setError("");
    setConfig({ ...status.config, maxTurns: v })
      .then(saved)
      .catch((e: unknown) => setError(String(e)));
  };

  const commitCompactAt = () => {
    const cur = status.config.compactThresholdTokens ?? DEFAULT_COMPACT_AT;
    const n = Number.parseInt(compactAt, 10);
    const v = Number.isFinite(n) ? Math.min(Math.max(n, MIN_COMPACT_AT), MAX_COMPACT_AT) : cur;
    if (Number.isFinite(n)) noteClamp("compactAt", n, v);
    setCompactAt(String(v));
    if (v === cur) return;
    setError("");
    setConfig({ ...status.config, compactThresholdTokens: v })
      .then(saved)
      .catch((e: unknown) => setError(String(e)));
  };

  const apply = async (mode: PermissionMode) => {
    setError("");
    try {
      saved(await setConfig({ ...status.config, defaultMode: mode }));
    } catch (e) {
      setError(String(e));
    }
  };

  const sandbox = status.config.sandbox ?? "workspaceWrite";
  const pickSandbox = (mode: SandboxMode) => {
    // 关沙箱要确认一次。它和"无人值守"是同一类决定：关掉之后唯一挡在
    // 危险命令前面的就只剩规则判断了，而判断是会错的。
    const commit = () => {
      setError("");
      setConfig({ ...status.config, sandbox: mode })
        .then(saved)
        .catch((e: unknown) => setError(String(e)));
    };
    if (mode === "off" && sandbox !== "off") {
      askConfirm({
        title: "关掉命令隔离？",
        body: "之后命令能改工作区以外的任何文件，只剩规则判断拦着。规则读不懂「python -c \"...\"」里的代码。",
        confirmLabel: "确认关闭",
        action: commit,
      });
      return;
    }
    commit();
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
        <h2>
          新会话的默认模式
          <HintTip>只影响之后创建的会话。当前会话在输入框左下角切换。</HintTip>
        </h2>
        <div className="mode-cards" role="radiogroup" aria-label="新会话的默认模式">
          {MODES.map((m) => (
            <button
              key={m.id}
              role="radio"
              aria-checked={current === m.id}
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
        <h2>
          命令隔离
          <HintTip>
            由操作系统限制命令能改什么。开着时，没有规则命中、也不是只读的命令可以直接放行 ——
            边界由内核守着。目前只有 macOS 能真正生效。
          </HintTip>
        </h2>
        <div className="mode-cards" role="radiogroup" aria-label="命令隔离">
          {SANDBOX_MODES.map((m) => (
            <button
              key={m.id}
              role="radio"
              aria-checked={sandbox === m.id}
              className={sandbox === m.id ? "mode-card active" : "mode-card"}
              onClick={() => pickSandbox(m.id)}
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
        <h2>
          等待授权的时间
          <HintTip>
            弹窗多久没人回应就放弃，超时按拒绝处理。范围 {MIN_TIMEOUT}–{MAX_TIMEOUT} 秒。
          </HintTip>
        </h2>
        <label className="field-inline">
          <FieldNumber
            value={timeout}
            onChange={(e) => setTimeout_(e.target.value)}
            onBlur={commitTimeout}
            onKeyDown={blurOnEnter}
          />
          <span className="field-unit">秒</span>
          {clamp?.key === "timeout" ? (
            <span className="clamp-note" role="status">
              {clamp.text}
            </span>
          ) : null}
        </label>
      </section>
      <section>
        <h2>
          单轮最大步数
          <HintTip>
            一句话之内模型最多自主往返多少步。到顶就停下等你再说，不是报错。浏览器自动化、渗透这类多步任务容易吃满，可以调高。范围{" "}
            {MIN_TURNS}–{MAX_TURNS} 步。
          </HintTip>
        </h2>
        <label className="field-inline">
          <FieldNumber
            value={turns}
            onChange={(e) => setTurns(e.target.value)}
            onBlur={commitTurns}
            onKeyDown={blurOnEnter}
          />
          <span className="field-unit">步</span>
          {clamp?.key === "turns" ? (
            <span className="clamp-note" role="status">
              {clamp.text}
            </span>
          ) : null}
        </label>
      </section>
      <section>
        <h2>
          上下文压缩阈值
          <HintTip>
            会话历史估算超过这个 token 数时自动摘要压缩。默认适配 128k 窗口，更小的模型请调低。范围{" "}
            {MIN_COMPACT_AT.toLocaleString()}–{MAX_COMPACT_AT.toLocaleString()}。
          </HintTip>
        </h2>
        <label className="field-inline">
          <FieldNumber
            value={compactAt}
            onChange={(e) => setCompactAt(e.target.value)}
            onBlur={commitCompactAt}
            onKeyDown={blurOnEnter}
          />
          <span className="field-unit">token</span>
          {clamp?.key === "compactAt" ? (
            <span className="clamp-note" role="status">
              {clamp.text}
            </span>
          ) : null}
        </label>
      </section>
      <section>
        <h2>
          会话内规则
          <HintTip>
            点「总是允许」记住的规则（如 <code>Bash(npm run *)</code>）只在当前会话有效。
          </HintTip>
        </h2>
      </section>
      {error ? <FormError text={error} /> : null}
    </>
  );
}

/* ---------- MCP 分区 ---------- */

/**
 * MCP 服务器管理。
 *
 * 配置改动走 setConfig（宿主保存后自动 reconcile 连接）；连接状态另有
 * 一条只读通道（mcpStatus），打开本页时轮询 —— 配置是"想要什么"，
 * 状态是"现在是什么"，两者永远可能不一致（正在连、连失败了）。
 */
function McpPane({
  status,
  onStatus,
  askConfirm,
  registerLeaveGuard,
  onSaved,
}: {
  status: ConfigStatus;
  onStatus: (s: ConfigStatus) => void;
  askConfirm: AskConfirm;
  registerLeaveGuard: (g: LeaveGuard | null) => void;
  onSaved: () => void;
}) {
  const cfg = status.config;
  const servers = cfg.mcpServers;
  const [selId, setSelId] = useState(servers[0]?.id ?? "");
  /** 刚新建的服务器：编辑器聚焦到名称，省得用户自己找第一个待填字段。 */
  const [justAdded, setJustAdded] = useState("");
  const [error, setError] = useState("");
  const [live, setLive] = useState<McpServerStatus[]>([]);
  /** null = 表单视图；字符串 = JSON 视图的编辑内容。 */
  const [jsonDraft, setJsonDraft] = useState<string | null>(null);
  /** 打开 JSON 视图那一刻的导出值。和它相同 = 没改过，关掉不用问。 */
  const [jsonBase, setJsonBase] = useState("");
  const [jsonBusy, setJsonBusy] = useState(false);

  const sel = servers.find((s) => s.id === selId) ?? servers[0];

  // 用户粘了一整段 JSON、还没保存 —— 关设置或切分区前得先问一句。
  // 这可能是他花了几分钟从 README 里拼出来的东西。
  const jsonDirty = jsonDraft !== null && jsonDraft !== jsonBase;
  useEffect(() => {
    registerLeaveGuard(
      jsonDirty
        ? () => ({
            title: "放弃未保存的 JSON 配置？",
            body: "JSON 视图里的改动还没保存，离开会丢掉它们。",
            confirmLabel: "放弃",
          })
        : null,
    );
    return () => registerLeaveGuard(null);
  }, [jsonDirty, registerLeaveGuard]);

  const openJson = async () => {
    setError("");
    try {
      const s = await mcpExportJson();
      setJsonBase(s);
      setJsonDraft(s);
    } catch (e) {
      setError(String(e));
    }
  };

  const applyJson = async () => {
    if (jsonDraft === null) return;
    setJsonBusy(true);
    setError("");
    try {
      const s = await mcpImportJson(jsonDraft);
      onStatus(s);
      onSaved();
      setJsonDraft(null);
      setSelId(s.config.mcpServers[0]?.id ?? "");
    } catch (e) {
      // 留在 JSON 视图里报错 —— 关掉的话用户就丢了刚粘的内容
      setError(String(e));
    } finally {
      setJsonBusy(false);
    }
  };

  // 打开本页时轮询连接状态。2.5s 是"点了重连能很快看到变化"和
  // "别对着宿主刷屏"之间的折中。
  useEffect(() => {
    let stopped = false;
    const pull = async () => {
      try {
        const s = await mcpStatus();
        if (!stopped) setLive(s);
      } catch {
        // 拿不到状态就保持上一份 —— 状态点短暂过时无伤大雅
      }
    };
    void pull();
    const timer = setInterval(() => void pull(), 2500);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, []);

  const commit = async (next: AppConfig) => {
    setError("");
    try {
      onStatus(await setConfig(next));
      onSaved();
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    }
  };

  const patchSel = (patch: Partial<McpServerConfig>) => {
    if (!sel) return;
    void commit({
      ...cfg,
      mcpServers: servers.map((s) => (s.id === sel.id ? { ...s, ...patch } : s)),
    });
  };

  const addServer = () => {
    // id 是工具名和权限规则的一部分，给一个能用但明显该改的默认值。
    let n = servers.length + 1;
    while (servers.some((s) => s.id === `server-${n}`)) n += 1;
    const id = `server-${n}`;
    const s: McpServerConfig = { id, command: "", args: [], env: {}, enabled: true };
    void commit({ ...cfg, mcpServers: [...servers, s] }).then((ok) => {
      if (ok) {
        setSelId(id);
        setJustAdded(id);
      }
    });
  };

  const removeServer = () => {
    if (!sel) return;
    const target = sel;
    askConfirm({
      title: `删除 MCP 服务器「${target.name || target.id}」？`,
      body: "它的进程会被停掉，工具在下一轮对话消失。",
      confirmLabel: "删除",
      action: () => {
        const rest = servers.filter((s) => s.id !== target.id);
        void commit({ ...cfg, mcpServers: rest }).then((ok) => {
          if (ok) setSelId(rest[0]?.id ?? "");
        });
      },
    });
  };

  // JSON 视图：显示并编辑标准格式（{"mcpServers": {...}}），整体替换。
  if (jsonDraft !== null) {
    return (
      <section>
        <h2>
          MCP 服务器 · JSON
          <HintTip>
            标准格式，和 Claude Desktop / Cursor / Cline 通用。README 里的{" "}
            <code>mcpServers</code> 片段可以整段粘贴。保存会整体替换当前列表。
          </HintTip>
        </h2>
        <textarea
          className="mcp-json-input"
          value={jsonDraft}
          onChange={(e) => setJsonDraft(e.target.value)}
          rows={16}
          spellCheck={false}
        />
        <div className="editor-foot">
          {error ? <span className="test-result err">{error}</span> : <span />}
          <div className="editor-foot-actions">
            <button
              onClick={() => {
                const back = () => {
                  setJsonDraft(null);
                  setError("");
                };
                if (jsonDirty) {
                  askConfirm({
                    title: "放弃未保存的 JSON 配置？",
                    body: "改动还没保存，返回表单视图会丢掉它们。",
                    confirmLabel: "放弃",
                    action: back,
                  });
                } else {
                  back();
                }
              }}
              disabled={jsonBusy}
            >
              取消
            </button>
            <button className="primary" onClick={() => void applyJson()} disabled={jsonBusy}>
              {jsonBusy ? "保存中…" : "保存"}
            </button>
          </div>
        </div>
      </section>
    );
  }

  if (!sel) {
    return (
      <section>
        <h2>MCP 服务器</h2>
        <div className="empty-state">
          <p className="empty-title">还没有 MCP 服务器</p>
          <p className="hint">
            通过 MCP 接入外部工具（文件、数据库、API……）。服务器跑在本机的
            独立进程里，工具和内置工具走同一套权限询问。
          </p>
          <div className="empty-actions">
            <button className="primary" onClick={addServer}>
              添加服务器
            </button>
            <button onClick={() => void openJson()}>粘贴 JSON 配置</button>
          </div>
          {error ? <p className="form-error">{error}</p> : null}
        </div>
      </section>
    );
  }

  return (
    <>
      <section>
        <div className="skills-head">
          <h2>
            MCP 服务器
            <HintTip>
              工具名是 <code>mcp__服务器id__…</code>，权限规则按它匹配。每个工具首次调用会像内置工具一样询问。
            </HintTip>
          </h2>
          <div className="mcp-head-actions">
            <button className="ghost" onClick={addServer}>
              添加
            </button>
            <button className="ghost" onClick={() => void openJson()} title="以标准 JSON 格式查看和编辑">
              JSON
            </button>
          </div>
        </div>
        {/* 竖排列表而不是 pill 铺排：几十个服务器时 pill 会糊成一片，
            长名字还会把整行撑爆。行内名字省略号截断，超高滚动。 */}
        <ul className="mcp-list">
          {servers.map((s) => {
            const st = live.find((l) => l.id === s.id);
            const state = s.enabled === false ? "off" : (st?.state ?? "off");
            const meta =
              s.enabled === false
                ? "已停用"
                : state === "connected"
                  ? `${st?.tools.length ?? 0} 个工具`
                  : state === "connecting"
                    ? "连接中…"
                    : state === "failed"
                      ? "连接失败"
                      : "";
            return (
              <li key={s.id}>
                <button
                  className={s.id === sel.id ? "mcp-row active" : "mcp-row"}
                  onClick={() => {
                    setSelId(s.id);
                    setJustAdded("");
                  }}
                  title={s.name || s.id}
                >
                  <span className={`mcp-dot ${state}`} />
                  <span className="mcp-row-name">{s.name || s.id}</span>
                  {meta ? <span className="mcp-row-meta">{meta}</span> : null}
                </button>
              </li>
            );
          })}
        </ul>
      </section>

      <McpServerEditor
        key={sel.id}
        server={sel}
        live={live.find((l) => l.id === sel.id) ?? null}
        autoFocusName={sel.id === justAdded}
        onPatch={patchSel}
        onRemove={removeServer}
        onError={setError}
      />

      {error ? <FormError text={error} /> : null}
    </>
  );
}

/** 已连接服务器的工具名默认只铺这么多个，其余收起 —— 几十个全量平铺会把配置区挤到两屏外。 */
const MCP_TOOLS_SHOWN = 12;

/** `mcp__better-icons__search_icons` → `search_icons`。前缀每条都一样，铺出来只添噪音。 */
function mcpToolShortName(full: string): string {
  const parts = full.split("__");
  if (parts[0] === "mcp" && parts.length >= 3) return parts.slice(2).join("__");
  return full;
}

function McpServerEditor({
  server,
  live,
  autoFocusName,
  onPatch,
  onRemove,
  onError,
}: {
  server: McpServerConfig;
  live: McpServerStatus | null;
  /** 刚新建时聚焦名称输入框。 */
  autoFocusName?: boolean;
  onPatch: (p: Partial<McpServerConfig>) => void;
  onRemove: () => void;
  onError: (e: string) => void;
}) {
  const [name, setName] = useState(server.name ?? "");
  const [command, setCommand] = useState(server.command);
  const [args, setArgs] = useState((server.args ?? []).join("\n"));
  const [env, setEnv] = useState(
    Object.entries(server.env ?? {})
      .map(([k, v]) => `${k}=${v}`)
      .join("\n"),
  );
  const [restarting, setRestarting] = useState(false);
  const [toolsOpen, setToolsOpen] = useState(false);

  const commitArgs = () => {
    const list = args
      .split("\n")
      .map((a) => a.trim())
      .filter(Boolean);
    if (JSON.stringify(list) !== JSON.stringify(server.args ?? [])) onPatch({ args: list });
  };

  const commitEnv = () => {
    const map: Record<string, string> = {};
    for (const line of env.split("\n")) {
      const t = line.trim();
      if (!t) continue;
      const eq = t.indexOf("=");
      if (eq <= 0) {
        onError(`环境变量要写成 KEY=VALUE：「${t}」`);
        return;
      }
      map[t.slice(0, eq).trim()] = t.slice(eq + 1).trim();
    }
    if (JSON.stringify(map) !== JSON.stringify(server.env ?? {})) onPatch({ env: map });
  };

  const doRestart = async () => {
    setRestarting(true);
    try {
      await mcpRestart(server.id);
    } catch (e) {
      onError(String(e));
    } finally {
      setRestarting(false);
    }
  };

  const state = server.enabled === false ? "off" : (live?.state ?? "off");
  const stateText: Record<string, string> = {
    connected: `已连接${live?.detail ? ` · ${live.detail}` : ""} · ${live?.tools.length ?? 0} 个工具`,
    connecting: "连接中…",
    failed: `连接失败：${live?.detail ?? "未知原因"}`,
    off:
      server.enabled === false
        ? "已停用"
        : server.command.trim()
          ? "未启动（保存配置后自动连接）"
          : "填好启动命令后自动连接",
  };

  return (
    <section>
      <div className={`mcp-status ${state}`}>
        <span className={`mcp-dot ${state}`} />
        <span className="mcp-status-text">{stateText[state]}</span>
        {server.enabled !== false ? (
          <button className="ghost" onClick={() => void doRestart()} disabled={restarting}>
            {restarting ? "重连中…" : "重连"}
          </button>
        ) : null}
      </div>
      {state === "connected" && live && live.tools.length > 0 ? (
        <ul className="mcp-tools">
          {(toolsOpen ? live.tools : live.tools.slice(0, MCP_TOOLS_SHOWN)).map((t) => (
            <li key={t}>
              <span className="mcp-tool-chip" title={t}>
                {mcpToolShortName(t)}
              </span>
            </li>
          ))}
          {live.tools.length > MCP_TOOLS_SHOWN ? (
            <li>
              <button type="button" className="mcp-tools-more" onClick={() => setToolsOpen(!toolsOpen)}>
                {toolsOpen ? "收起" : `还有 ${live.tools.length - MCP_TOOLS_SHOWN} 个`}
              </button>
            </li>
          ) : null}
        </ul>
      ) : null}

      <Toggle
        on={server.enabled !== false}
        onChange={(v) => onPatch({ enabled: v })}
        label="启用这个服务器"
      />

      <div className="field-row">
        <label>名称</label>
        <input
          value={name}
          onChange={(e) => setName(e.target.value)}
          onBlur={() => name.trim() !== (server.name ?? "") && onPatch({ name: name.trim() })}
          onKeyDown={blurOnEnter}
          autoFocus={autoFocusName}
          placeholder={server.id}
          spellCheck={false}
        />
      </div>
      <div className="field-row">
        <label>命令</label>
        <input
          value={command}
          onChange={(e) => setCommand(e.target.value)}
          onBlur={() => command.trim() !== server.command && onPatch({ command: command.trim() })}
          onKeyDown={blurOnEnter}
          placeholder="npx / uvx / 可执行文件路径"
          spellCheck={false}
        />
      </div>
      <div className="field-row">
        <label>参数</label>
        <textarea
          value={args}
          onChange={(e) => setArgs(e.target.value)}
          onBlur={commitArgs}
          placeholder={"一行一个，如：\n-y\n@modelcontextprotocol/server-filesystem\n/tmp"}
          rows={4}
          spellCheck={false}
        />
      </div>
      <div className="field-row">
        <label>环境变量</label>
        <textarea
          value={env}
          onChange={(e) => setEnv(e.target.value)}
          onBlur={commitEnv}
          placeholder={"一行一个 KEY=VALUE，如：\nGITHUB_TOKEN=ghp_..."}
          rows={2}
          spellCheck={false}
        />
      </div>
      <div className="editor-foot">
        <span />
        <div className="editor-foot-actions">
          <button className="btn-danger ghost-danger" onClick={onRemove}>
            删除服务器
          </button>
        </div>
      </div>
    </section>
  );
}

/* ---------- 能力包分区 ---------- */

/** 字节数写成人话。包是几百 MB 量级，一位小数够用。 */
function humanSize(bytes: number): string {
  if (bytes >= 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GB`;
  if (bytes >= 1024 * 1024) return `${Math.round(bytes / 1024 / 1024)} MB`;
  return `${Math.round(bytes / 1024)} KB`;
}

/**
 * 安装进度的一句话描述。下载有百分比，后面三步没有 —— 它们相对下载
 * 短得多，硬凑一个总进度只会让进度条在末尾诡异地卡住。
 */
function progressText(p: PackProgress): string {
  switch (p.kind) {
    case "downloading":
      return p.total > 0
        ? `下载中 ${humanSize(p.received)} / ${humanSize(p.total)}`
        : `下载中 ${humanSize(p.received)}`;
    case "verifying":
      return "校验中…";
    case "extracting":
      return "解压中…";
    case "selfCheck":
      return "自检中…";
    case "done":
      return "完成";
    case "failed":
      return p.error;
  }
}

function PacksPane({ askConfirm }: { askConfirm: AskConfirm }) {
  const [packs, setPacks] = useState<PackStatus[] | null>(null);
  const [loadError, setLoadError] = useState("");
  /** 安装的进度和"正在装"标记在模块级 —— 关掉设置面板不该把它们连同组件一起丢掉。 */
  const installs = usePackInstalls();
  /** 卸载只是删本地目录，秒回，不值得也挪出去。 */
  const [uninstalling, setUninstalling] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setPacks(await packsStatus());
      setLoadError("");
    } catch (e) {
      setPacks(null);
      setLoadError(String(e));
    }
  }, []);

  // completed 变了 = 有安装刚跑完，清单得重拉。它可能是在面板关着的时候
  // 完成的，所以这里既管首次挂载，也管"装完了但没人看着"。
  useEffect(() => {
    void refresh().then(clearDonePackProgress);
  }, [refresh, installs.completed]);

  const uninstall = (p: PackStatus) => {
    askConfirm({
      title: `卸载「${p.name}」？`,
      body: "会删掉整个包，连带摘掉它注册的 MCP 服务器和技能。可以随时重新下载。",
      confirmLabel: "卸载",
      action: () => {
        setUninstalling(p.id);
        void (async () => {
          try {
            await packsUninstall(p.id);
            clearPackProgress(p.id);
            await refresh();
          } catch (e) {
            reportPackFailure(p.id, String(e));
          } finally {
            setUninstalling(null);
          }
        })();
      },
    });
  };

  return (
    <section>
      <h2>
        能力包
        <HintTip>
          可选下载的运行时。装上之后模型自己会用 —— 相关技能和工具自动注册，
          不需要你在别处再配一遍。包体较大，建议在网络稳定时装；装的过程中可以关掉
          设置去干别的，回来还能看到进度。下载中断可以重来，已下好的部分会接着传。
        </HintTip>
      </h2>

      {loadError ? (
        <div className="empty-state">
          <p className="form-error" style={{ margin: 0 }}>
            读取失败：{loadError}
          </p>
          <button onClick={() => void refresh()}>重试</button>
        </div>
      ) : packs === null ? (
        <p className="hint">读取中…</p>
      ) : packs.length === 0 ? (
        <p className="hint">当前没有可用的能力包。</p>
      ) : (
        <ul className="pack-list">
          {packs.map((p) => {
            const prog = installs.progress[p.id];
            const installing = Boolean(installs.running[p.id]);
            const busy = installing || uninstalling === p.id;
            const upgradable =
              p.installedVersion !== null &&
              p.availableVersion !== null &&
              p.installedVersion !== p.availableVersion;
            return (
              <li key={p.id} className="pack-item">
                <div className="pack-head">
                  <span className="pack-name">{p.name}</span>
                  {p.installedVersion ? (
                    <span className="pack-badge on">已装 {p.installedVersion}</span>
                  ) : null}
                  {upgradable ? (
                    <span className="pack-badge">可升级到 {p.availableVersion}</span>
                  ) : null}
                </div>
                <p className="hint" style={{ margin: "2px 0 0" }}>
                  {p.description}
                </p>

                {!p.supported ? (
                  <p className="hint" style={{ margin: "6px 0 0" }}>
                    这个包没有适配当前系统的版本。
                  </p>
                ) : p.manifestError && !p.installedVersion ? (
                  <p className="form-error" style={{ margin: "6px 0 0" }}>
                    拉不到清单：{p.manifestError}
                  </p>
                ) : !p.availableVersion && !p.installedVersion ? (
                  // 清单拉到了、但里面还没有这个包。不说话的话这一行就只剩名字和
                  // 描述、没有任何按钮，用户分不清是在加载、坏了、还是没发布。
                  <p className="hint" style={{ margin: "6px 0 0" }}>
                    还没有发布可下载的版本。
                  </p>
                ) : null}

                {prog ? (
                  <div className="pack-progress">
                    {prog.kind === "downloading" && prog.total > 0 ? (
                      <div className="pack-bar">
                        <div
                          className="pack-bar-fill"
                          style={{ width: `${Math.round((prog.received / prog.total) * 100)}%` }}
                        />
                      </div>
                    ) : null}
                    <span className={prog.kind === "failed" ? "form-error" : "hint"}>
                      {progressText(prog)}
                    </span>
                  </div>
                ) : null}

                <div className="pack-actions">
                  {p.availableVersion && (!p.installedVersion || upgradable) ? (
                    <button
                      disabled={busy || !p.supported}
                      onClick={() => startPackInstall(p.id)}
                    >
                      {p.installedVersion ? "升级" : "下载安装"}
                      {p.downloadSize > 0 ? `（${humanSize(p.downloadSize)}）` : null}
                    </button>
                  ) : null}
                  {p.installedVersion ? (
                    <button className="ghost" disabled={busy} onClick={() => uninstall(p)}>
                      卸载
                    </button>
                  ) : null}
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

/* ---------- Skills 分区 ---------- */

/**
 * 技能清单（只读）。技能就是磁盘上的 SKILL.md，编辑器比表单好用 ——
 * 这页只负责"有哪些、哪个坏了、目录在哪"。
 */
function SkillsPane({ status, activeRoot }: { status: ConfigStatus; activeRoot: string | null }) {
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
      <section>
        <h2>
          Skills
          <HintTip>
            写成 <code>SKILL.md</code>，模型按需加载（清单进上下文，正文用到才读）。优先级{" "}
            <strong>项目 &gt; 全局 &gt; 内置</strong>，同名即可盖掉内置。
            名字和描述每轮都在上下文里，所以描述写「什么时候用」。只想自己敲{" "}
            <code>/</code>、不想占上下文的，去「命令」页。
            {activeRoot ? (
              <>
                {" "}
                项目级放 <code>{activeRoot}/.riot/skills/</code>。
              </>
            ) : null}
          </HintTip>
        </h2>
        <div className="about-row">
          <code>{globalDir}/&lt;名字&gt;/SKILL.md</code>
          <button className="ghost" onClick={() => void revealInFinder(globalDir)}>
            打开目录
          </button>
        </div>
      </section>

      <section>
        <div className="skills-head">
          <h2>已发现的技能</h2>
          <button className="ghost" onClick={() => void refresh()}>
            刷新
          </button>
        </div>
        {loadError ? (
          <div className="empty-state">
            <p className="form-error" style={{ margin: 0 }}>
              读取失败：{loadError}
            </p>
            <button onClick={() => void refresh()}>重试</button>
          </div>
        ) : skills === null ? (
          <p className="hint">读取中…</p>
        ) : skills.length === 0 ? (
          <div className="empty-state">
            <p className="empty-title">还没有技能</p>
            <p className="hint">
              示例格式：
            </p>
            <pre className="skill-example">{`---
name: 发布流程
description: 发布新版本时用。跑测试、打 tag、更新 changelog。
---
1. 跑 cargo test --workspace
2. ……`}</pre>
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
                    {s.source === "builtin"
                      ? "内置"
                      : s.source === "project"
                        ? "项目"
                        : s.source === "pack"
                          ? "能力包"
                          : "全局"}
                  </span>
                </div>
                <p className={s.error ? "form-error" : "hint"} style={{ margin: "2px 0 0" }}>
                  {s.error ?? s.description}
                </p>
              </li>
            ))}
          </ul>
        )}
      </section>
    </>
  );
}

/* ---------- 命令 ---------- */

/** 技能在命令页的层级前缀。和 Skills 页用同一套词。 */
const SKILL_TIER: Record<string, string> = {
  builtin: "内置",
  pack: "能力包",
  global: "全局",
  project: "项目",
};

function CommandsPane({ status, activeRoot }: { status: ConfigStatus; activeRoot: string | null }) {
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
    <section>
      <div className="skills-head">
        <h2>
          斜杠命令
          <HintTip>
            输入框敲 <code>/</code> 调用。<code>$ARGUMENTS</code> 换整段参数，
            <code>$1 $2</code> 换第 N 个。子目录是命名空间（<code>git/pr.md</code> →{" "}
            <code>/git:pr</code>）。
            <strong>模型看不到命令</strong>，只有你敲了才展开，不占上下文；技能的描述每轮都在，模型能自己决定用。优先级{" "}
            <strong>内置命令 &gt; 命令文件 &gt; 技能</strong>。
            {activeRoot ? (
              <>
                {" "}
                项目级放 <code>{activeRoot}/.riot/commands/</code>。
              </>
            ) : null}
          </HintTip>
        </h2>
        <button className="ghost" onClick={() => void refresh()}>
          刷新
        </button>
      </div>
      <div className="about-row">
        <code>{configDir}/commands/&lt;名字&gt;.md</code>
        <button className="ghost" onClick={() => void revealInFinder(configDir)}>
          打开目录
        </button>
      </div>
      {loadError ? (
        <div className="empty-state">
          <p className="form-error" style={{ margin: 0 }}>
            读取失败：{loadError}
          </p>
          <button onClick={() => void refresh()}>重试</button>
        </div>
      ) : commands === null ? (
        <p className="hint">读取中…</p>
      ) : commands.length === 0 ? (
        <div className="empty-state">
          <p className="empty-title">还没有命令</p>
          <p className="hint">
            在上面的目录里放一个 <code>.md</code> 文件就有了：文件名是命令名，正文是敲
            <code>/</code> 后发出去的提示词。
          </p>
          <pre className="skill-example">{`# review.md
帮我审查当前改动，重点看：$ARGUMENTS`}</pre>
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
                    ? "内置"
                    : c.source === "skill"
                      ? `${SKILL_TIER[c.skillSource ?? ""] ?? ""}技能`
                      : c.source === "project"
                        ? "项目"
                        : "全局"}
                </span>
              </div>
              <p className="hint" style={{ margin: "2px 0 0" }}>
                {c.description}
              </p>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/* ---------- Hooks ---------- */

const HOOK_EVENT_HINT: Record<string, string> = {
  PreToolUse: "工具执行前。exit 2 = 拦下这次调用",
  PostToolUse: "工具执行后。反馈给模型（格式检查、lint）",
  Stop: "模型想收尾时。exit 2 = 不许停，带理由再跑一轮",
  UserPromptSubmit: "消息发出前。exit 2 = 拦下这条消息",
};

function HooksPane({ status, activeRoot }: { status: ConfigStatus; activeRoot: string | null }) {
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
    <section>
      <div className="skills-head">
        <h2>
          Hooks
          <HintTip>
            固定检查点跑脚本：stdin 收一行事件 JSON，<b>exit 2 拦下</b>（stderr
            给模型看），exit 0 的 stdout 作补充上下文。事件：PreToolUse / PostToolUse / Stop /
            UserPromptSubmit。matcher 支持工具名、<code>A|B</code>、正则。
            {activeRoot ? (
              <>
                {" "}
                项目级 <code>{activeRoot}/.riot/hooks.json</code> 和全局叠加。
              </>
            ) : null}
          </HintTip>
        </h2>
        <button className="ghost" onClick={() => void refresh()}>
          刷新
        </button>
      </div>
      <div className="about-row">
        <code>{configDir}/hooks.json</code>
        <button className="ghost" onClick={() => void revealInFinder(configDir)}>
          打开目录
        </button>
      </div>
      {loadError ? (
        <div className="empty-state">
          <p className="form-error" style={{ margin: 0 }}>
            读取失败：{loadError}
          </p>
          <button onClick={() => void refresh()}>重试</button>
        </div>
      ) : hooks === null ? (
        <p className="hint">读取中…</p>
      ) : hooks.length === 0 ? (
        <div className="empty-state">
          <p className="empty-title">还没有 hooks</p>
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
          {hooks.map((h, i) => (
            <li key={`${h.event}-${h.command}-${i}`} className={h.error ? "skill-item bad" : "skill-item"}>
              <div className="skill-item-head">
                <span className="skill-name">{h.error ? "配置有问题" : h.event}</span>
                {h.matcher ? <code className="hook-matcher">{h.matcher}</code> : null}
                <span className="skill-source">{h.source === "project" ? "项目" : "全局"}</span>
              </div>
              <p className={h.error ? "form-error" : "hint"} style={{ margin: "2px 0 0" }}>
                {h.error ? `${h.command}：${h.error}` : h.command}
              </p>
              {!h.error ? (
                <p className="hint" style={{ margin: "2px 0 0" }}>
                  {HOOK_EVENT_HINT[h.event] ?? ""}（超时 {h.timeoutSecs}s）
                </p>
              ) : null}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/* ---------- 关于 ---------- */

function AboutPane({ status }: { status: ConfigStatus }) {
  const configDir = status.configPath.replace(/\/[^/]*$/, "");
  return (
    <>
      <section>
        <h2>
          Riot
          <HintTip>侧栏、浏览器抽屉、终端的边缘可以拖，调整大小；双击恢复默认。</HintTip>
        </h2>
        <p className="hint">本地 coding agent。Tauri + Rust。</p>
      </section>
      <section>
        <h2>
          配置文件
          <HintTip>
            API key 单独存在同目录的 <code>auth.json</code>。
          </HintTip>
        </h2>
        <div className="about-row">
          <code>{status.configPath}</code>
          <button className="ghost" onClick={() => void revealInFinder(configDir)}>
            在访达中显示
          </button>
        </div>
      </section>
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
