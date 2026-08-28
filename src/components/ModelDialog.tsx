import { useState } from "react";

import {
  type ModelConfig,
  type ProviderConfig,
  type Sampling,
  testConnection,
} from "../bridge";
import {
  MAX_CONTEXT_WINDOW,
  MIN_CONTEXT_WINDOW,
  compactThresholdForWindow,
  fmtTokens,
} from "../lib/contextWindow";
import { parseSampling, samplingDraft } from "../lib/sampling";
import { FieldNumber } from "./FieldNumber";
import { SamplingSliders } from "./FieldSlider";
import { HintTip } from "./HintTip";
import { Modal } from "./Modal";

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
  const [ctxWindow, setCtxWindow] = useState(model?.contextWindow?.toString() ?? "");
  // 数字字段走字符串草稿:输入过程中会经过 "0."、"-" 这种还不是合法数字的
  // 中间态，直接绑成 number 的话那些字符会被吃掉，表现是"打不出小数点"。
  const [samp, setSamp] = useState<Record<string, string>>(() =>
    samplingDraft(model?.sampling ?? {}),
  );
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; text: string } | null>(null);

  const adding = model === null;
  const trimmedId = id.trim();
  // 新增时不能和已有的撞名 —— 撞了的话后面按 id 找配置会拿到第一个，
  // 而用户改的是第二个，表现为"设置没生效"。
  const duplicate = adding && provider.models.some((m) => m.id === trimmedId);

  const sampling = (): Sampling => parseSampling(samp);

  /** 留空 = 不填这个字段，压缩阈值走设置里的全局值。 */
  const contextWindow = (): number | undefined => {
    const n = Number.parseInt(ctxWindow.trim(), 10);
    if (!Number.isFinite(n)) return undefined;
    return Math.min(Math.max(n, MIN_CONTEXT_WINDOW), MAX_CONTEXT_WINDOW);
  };

  const save = () => {
    if (!trimmedId || duplicate) return;
    const win = contextWindow();
    onSave({
      id: trimmedId,
      name: name.trim(),
      vision,
      // 没填就整个不带这个键（`exactOptionalPropertyTypes`），跟宿主侧
      // `skip_serializing_if` 对上 —— 存量配置不会平白多出一堆 null。
      ...(win === undefined ? {} : { contextWindow: win }),
      sampling: sampling(),
    });
    onClose();
  };

  // 把窗口换算成"大概什么时候会压"给用户看。填 200000 本身不说明任何
  // 事情 —— 用户想知道的是这个数会让压缩早一点还是晚一点发生。
  const windowNote = (() => {
    const w = contextWindow();
    if (w === undefined) return "留空 = 跟随设置里的全局压缩阈值。";
    const maxOut = sampling().maxOutputTokens ?? provider.sampling?.maxOutputTokens ?? undefined;
    return `历史约 ${fmtTokens(compactThresholdForWindow(w, maxOut))} 时自动压缩（窗口减去回复和摘要的预留）。`;
  })();

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
    <Modal className="model-dialog" label={adding ? "添加模型" : "编辑模型"} onClose={onClose}>
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

          <h3 className="model-dialog-section">
            能力
            <HintTip>
              关着时，截图和附的图会先交给「视觉兼容模型」转成文字。开错了服务方会拒图。
            </HintTip>
          </h3>
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

          <div className="field-row">
            <label>
              上下文窗口
              <HintTip>
                模型文档上写的窗口大小。填了它，压缩时机就按这个模型算；留空则用设置里的全局阈值。
              </HintTip>
            </label>
            <FieldNumber
              value={ctxWindow}
              onChange={(e) => setCtxWindow(e.target.value)}
              placeholder="如 128000"
            />
          </div>
          <p className="hint model-dialog-note">{windowNote}</p>

          <h3 className="model-dialog-section">
            采样参数
            <HintTip>数字是服务方的值（或常见默认）。改过的字段才写入覆盖。</HintTip>
          </h3>
          <SamplingSliders
            draft={samp}
            inherited={provider.sampling}
            hint
            onChange={(key, value) => setSamp((s) => ({ ...s, [key]: value }))}
          />
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
    </Modal>
  );
}
