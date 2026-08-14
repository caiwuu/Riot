import { useEffect, useState } from "react";

import {
  type Sampling,
  type SessionInfo,
  pickDirectory,
  setSessionPythonVenv,
  setSessionSampling,
  setSessionSystemPrompt,
} from "../bridge";

/**
 * 会话设置弹窗：只管**这个会话**的东西 —— 采样覆盖、Python 虚拟环境、
 * 追加的系统提示词。全局的 provider / 权限 / 联网在侧栏的「设置」里。
 *
 * `[约束]` 所有字段都是"改完下一轮生效"，真值在宿主的 Session 上。
 * 提交成功后通过 `onPatch` 回写 App 的会话列表 —— 不回写的话，关掉
 * 弹窗再打开，显示的还是启动时 listSessions 拉到的旧值。
 */
const SAMPLING_FIELDS: {
  key: keyof Sampling;
  label: string;
  step: string;
  integer?: boolean;
}[] = [
  { key: "temperature", label: "temperature", step: "0.1" },
  { key: "topP", label: "top_p", step: "0.05" },
  { key: "topK", label: "top_k", step: "1", integer: true },
  { key: "maxOutputTokens", label: "max tokens", step: "256", integer: true },
];

/** 把输入框草稿解析成采样值：空/非法 = null（继承）。 */
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

/** 逐字段比较。宿主回来的空覆盖是 `{}`，解析结果是全 null —— 语义相同。 */
function sameSampling(a: Sampling, b: Sampling): boolean {
  return SAMPLING_FIELDS.every((f) => (a[f.key] ?? null) === (b[f.key] ?? null));
}

export function SessionSettings({
  session,
  inherited,
  onPatch,
  onClose,
}: {
  session: SessionInfo;
  /** 继承来的采样默认值（当前激活 provider 的），显示成占位符。 */
  inherited: Sampling;
  /** 提交成功后回写 App 里的会话信息。 */
  onPatch: (patch: Partial<SessionInfo>) => void;
  onClose: () => void;
}) {
  // 数字字段走字符串草稿：绑成 number 的话 "0."、"-" 这种中间态会被吃掉。
  const [samp, setSamp] = useState<Record<string, string>>(() =>
    Object.fromEntries(
      SAMPLING_FIELDS.map((f) => [f.key, session.sampling[f.key]?.toString() ?? ""]),
    ),
  );
  const [venv, setVenv] = useState(session.pythonVenv ?? "");
  const [prompt, setPrompt] = useState(session.systemPrompt ?? "");
  const [error, setError] = useState("");

  useEffect(() => {
    const esc = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  }, [onClose]);

  const overrides = SAMPLING_FIELDS.filter(
    (f) => (samp[f.key] ?? "").trim() !== "",
  ).length;

  const commitSampling = (next: Sampling) => {
    if (sameSampling(next, session.sampling)) return;
    setError("");
    setSessionSampling(session.id, next)
      .then(() => onPatch({ sampling: next }))
      .catch((e: unknown) => setError(String(e)));
  };

  const resetSampling = () => {
    setSamp(Object.fromEntries(SAMPLING_FIELDS.map((f) => [f.key, ""])));
    commitSampling({});
  };

  const commitVenv = async (value: string) => {
    const v = value.trim();
    if (v === (session.pythonVenv ?? "")) return;
    setError("");
    try {
      await setSessionPythonVenv(session.id, v);
      onPatch({ pythonVenv: v || null });
    } catch (e) {
      // 宿主拒了（目录里没有 bin/python）。草稿留着让用户改，
      // 但真值没变 —— 报错必须说清，不然他以为已经生效了。
      setError(String(e));
    }
  };

  const pickVenv = async () => {
    const dir = await pickDirectory();
    if (!dir) return;
    setVenv(dir);
    await commitVenv(dir);
  };

  const commitPrompt = (value: string) => {
    const p = value.trim();
    if (p === (session.systemPrompt ?? "")) return;
    setError("");
    setSessionSystemPrompt(session.id, p)
      .then(() => onPatch({ systemPrompt: p || null }))
      .catch((e: unknown) => setError(String(e)));
  };

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => e.target === e.currentTarget && onClose()}
    >
      <div className="modal session-dialog">
        <div className="modal-head">
          <span className="modal-title">会话设置</span>
          <span className="modal-queue" title={session.root}>
            {session.title ?? "新会话"}
          </span>
          <button className="ghost" onClick={onClose} aria-label="关闭 (Esc)">
            ✕
          </button>
        </div>

        <div className="session-dialog-body">
          <h3 className="dialog-section" style={{ marginTop: 0 }} title="留空继承服务方的设置，占位符是继承来的值">
            采样参数
          </h3>
          <div className="samp-grid">
            {SAMPLING_FIELDS.map((f) => (
              <div className="field-row" key={f.key}>
                <label>{f.label}</label>
                <input
                  type="number"
                  step={f.step}
                  value={samp[f.key] ?? ""}
                  onChange={(e) => setSamp({ ...samp, [f.key]: e.target.value })}
                  onBlur={() => commitSampling(parseSampling(samp))}
                  placeholder={inherited[f.key]?.toString() ?? "默认"}
                  spellCheck={false}
                />
              </div>
            ))}
          </div>
          {overrides ? (
            <button className="ghost" onClick={resetSampling}>
              全部恢复继承
            </button>
          ) : null}

          <h3 className="dialog-section" title="注入 VIRTUAL_ENV 并把 bin 排到 PATH 最前，python / pip 直接落在这个环境。清空恢复系统默认">
            Python 虚拟环境
          </h3>
          <div className="key-row">
            <input
              value={venv}
              onChange={(e) => setVenv(e.target.value)}
              onBlur={(e) => void commitVenv(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") (e.target as HTMLInputElement).blur();
              }}
              placeholder="venv 根目录"
              spellCheck={false}
            />
            <button onClick={() => void pickVenv()}>选择…</button>
          </div>

          <h3 className="dialog-section" title="追加在内置提示词之后，不替换它。留空只用内置提示词">
            系统提示词
          </h3>
          <textarea
            className="prompt-input"
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            onBlur={(e) => commitPrompt(e.target.value)}
            placeholder="给这个会话补充的指令"
            rows={5}
            spellCheck={false}
          />

          {error ? <p className="form-error">{error}</p> : null}
        </div>
      </div>
    </div>
  );
}
