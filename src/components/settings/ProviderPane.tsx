import { useState } from "react";

import {
  type AppConfig,
  type ConfigStatus,
  type ProviderConfig,
  setConfig,
} from "../../bridge";
import { FieldSelect } from "../FieldSelect";
import { HintTip } from "../HintTip";
import { ProviderEditor } from "./ProviderEditor";
import { type AskConfirm, FormError } from "./shared";

export function ProviderPane({
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
