import { useEffect, useState } from "react";

import {
  type ModelConfig,
  type ProviderConfig,
  type Sampling,
  testConnection,
} from "../bridge";

/**
 * 一个模型的设置。
 *
 * `[取舍]` 做成弹窗，而不是挂在模型行上的一排小控件。
 *
 * 行上放得下的只有开关和图标，而模型有 ID、显示名、能力、四个采样参数 ——
 * 挤在一行里每一样都得靠猜。弹窗的代价是多一次点击，换来的是每个字段都能带
 * 上标签和"留空是什么意思"的说明。
 *
 * 采样参数也在这里，不在别处:它们和能力一样属于这个模型，分两个地方设的话，
 * 用户改完参数会去找"那我的 temperature 到底存哪了"。
 */
const FIELDS: {
  key: keyof Sampling;
  label: string;
  step: string;
  integer?: boolean;
  hint: string;
}[] = [
  { key: "temperature", label: "temperature", step: "0.1", hint: "越高越随机" },
  { key: "topP", label: "top_p", step: "0.05", hint: "核采样" },
  {
    key: "topK",
    label: "top_k",
    step: "1",
    integer: true,
    hint: "仅 Anthropic 协议发送",
  },
  {
    key: "maxOutputTokens",
    label: "最大输出 token",
    step: "256",
    integer: true,
    hint: "单次回复的上限",
  },
];

export function ModelDialog({
  provider,
  model,
  onSave,
  onClose,
}: {
  provider: ProviderConfig;
  /** `null` = 新增一个模型。 */
  model: ModelConfig | null;
  onSave: (m: ModelConfig) => void;
  onClose: () => void;
}) {
  const [id, setId] = useState(model?.id ?? "");
  const [name, setName] = useState(model?.name ?? "");
  const [vision, setVision] = useState(model?.vision ?? false);
  // 数字字段走字符串草稿:输入过程中会经过 "0."、"-" 这种还不是合法数字的
  // 中间态，直接绑成 number 的话那些字符会被吃掉，表现是"打不出小数点"。
  const [samp, setSamp] = useState<Record<string, string>>(() =>
    Object.fromEntries(
      FIELDS.map((f) => [f.key, model?.sampling?.[f.key]?.toString() ?? ""]),
    ),
  );
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; text: string } | null>(null);

  useEffect(() => {
    const esc = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  }, [onClose]);

  const adding = model === null;
  const trimmedId = id.trim();
  // 新增时不能和已有的撞名 —— 撞了的话后面按 id 找配置会拿到第一个，
  // 而用户改的是第二个，表现为"设置没生效"。
  const duplicate = adding && provider.models.some((m) => m.id === trimmedId);

  const sampling = (): Sampling => {
    const out: Sampling = {};
    for (const f of FIELDS) {
      const raw = samp[f.key]?.trim() ?? "";
      if (!raw) continue;
      const n = Number(raw);
      if (!Number.isFinite(n)) continue;
      out[f.key] = f.integer ? Math.round(n) : n;
    }
    return out;
  };

  const save = () => {
    if (!trimmedId || duplicate) return;
    onSave({
      id: trimmedId,
      name: name.trim(),
      vision,
      sampling: sampling(),
    });
    onClose();
  };

  const doTest = async () => {
    setTesting(true);
    setTestResult(null);
    try {
      setTestResult({ ok: true, text: await testConnection(provider.id, trimmedId) });
    } catch (e) {
      setTestResult({ ok: false, text: String(e) });
    } finally {
      setTesting(false);
    }
  };

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => e.target === e.currentTarget && onClose()}
    >
      <div className="modal model-dialog">
        <div className="modal-head">
          <span className="modal-title">{adding ? "添加模型" : "编辑模型"}</span>
          <span className="bar-spacer" />
          <button className="ghost" onClick={onClose} aria-label="关闭">
            ✕
          </button>
        </div>

        <div className="model-dialog-body">
          <div className="field-row">
            <label>模型 ID</label>
            <input
              value={id}
              onChange={(e) => setId(e.target.value)}
              placeholder="发给服务方的模型名，如 glm-4.6v"
              spellCheck={false}
              // `[约束]` 编辑时不让改 ID。改它等于换了一个模型 —— 而
              // activeModel、fallbackModel、视觉兼容那条 `providerId/model`
              // 都按这个字符串引用它，改完会留下一堆指向幽灵名字的配置。
              // 要换名字就删掉重加。
              disabled={!adding}
              autoFocus={adding}
            />
          </div>
          {duplicate ? (
            <p className="form-error">这个服务方下已经有同名模型了。</p>
          ) : null}

          <div className="field-row">
            <label>显示名称</label>
            <input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={trimmedId || "留空就直接显示模型 ID"}
              spellCheck={false}
            />
          </div>

          <h3 className="model-dialog-section">能力</h3>
          <button
            className="toggle-row"
            onClick={() => setVision(!vision)}
            role="switch"
            aria-checked={vision}
          >
            <span className={vision ? "toggle-track on" : "toggle-track"}>
              <span className="toggle-knob" />
            </span>
            <span>视觉（能收图片）</span>
          </button>
          <p className="hint">
            关着时，截图和你附的图会先交给「视觉兼容模型」转成文字。
            开错了的表现很直接：图片发过去被服务方拒。
          </p>

          <h3 className="model-dialog-section">采样参数</h3>
          <p className="hint">留空继承服务方的设置，占位符就是继承来的值。</p>
          {FIELDS.map((f) => (
            <div className="field-row" key={f.key}>
              <label>{f.label}</label>
              <input
                type="number"
                step={f.step}
                value={samp[f.key] ?? ""}
                onChange={(e) => setSamp({ ...samp, [f.key]: e.target.value })}
                placeholder={provider.sampling?.[f.key]?.toString() ?? "服务端默认"}
                spellCheck={false}
              />
              <span className="field-hint">{f.hint}</span>
            </div>
          ))}
        </div>

        <div className="editor-foot">
          {testResult ? (
            <span className={testResult.ok ? "test-result ok" : "test-result err"}>
              {testResult.text}
            </span>
          ) : (
            <span className="hint" style={{ margin: 0 }}>
              「测试」会用这个模型真发一个最小请求。
            </span>
          )}
          <div className="editor-foot-actions">
            <button onClick={onClose}>取消</button>
            <button onClick={() => void doTest()} disabled={testing || !trimmedId}>
              {testing ? "测试中…" : "测试模型"}
            </button>
            <button className="primary" onClick={save} disabled={!trimmedId || duplicate}>
              保存
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
