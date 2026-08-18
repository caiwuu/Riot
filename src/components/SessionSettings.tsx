import { useCallback, useEffect, useRef, useState } from "react";

import {
  type Sampling,
  type SessionInfo,
  type ThinkingPolicy,
  detectVenvs,
  pickDirectory,
  setSessionPythonVenv,
  setSessionSampling,
  setSessionSystemPrompt,
  setSessionThinking,
} from "../bridge";
import { FieldNumber } from "./FieldNumber";
import { FieldSelect, type FieldOption } from "./FieldSelect";
import { HintTip } from "./HintTip";
import { Modal } from "./Modal";
import { basename } from "../pathDisplay";

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

/** 思考策略的下拉项。固定档位摊平成一级选项 —— 六个选项不值得两级菜单。
 *  说明走 hint（第二行灰字）：塞进主标签会挤成两行大字，见谁选谁难受。 */
const THINKING_OPTIONS: FieldOption[] = [
  { value: "default", label: "默认", hint: "不发思考参数，随端点默认" },
  { value: "adaptive", label: "自适应", hint: "首轮认真想，工具续轮少想" },
  { value: "low", label: "低" },
  { value: "medium", label: "中" },
  { value: "high", label: "高" },
  { value: "disabled", label: "关闭思考", hint: "部分端点不支持" },
];

function thinkingKey(p: ThinkingPolicy): string {
  return p.mode === "fixed" ? p.level : p.mode;
}

function thinkingFromKey(k: string): ThinkingPolicy {
  if (k === "low" || k === "medium" || k === "high") return { mode: "fixed", level: k };
  if (k === "adaptive" || k === "disabled") return { mode: k };
  return { mode: "default" };
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
  const [thinking, setThinking] = useState(() => thinkingKey(session.thinking));
  /** 项目根下探测到的 venv（.venv / venv）。系统选择框藏起点开头的目录，
   *  多数人的 .venv 在选择框里根本看不到 —— 探测到就给一键填入。 */
  const [venvFound, setVenvFound] = useState<string[]>([]);

  useEffect(() => {
    // 探测失败不打扰：这只是个便利入口，手输和选择框都还在。
    detectVenvs(session.id).then(setVenvFound).catch(() => {});
  }, [session.id]);
  const [error, setError] = useState("");
  /** 「全部恢复继承」的点击回执。没有它，点下去唯一的变化是按钮自己变灰。 */
  const [resetDone, setResetDone] = useState(false);
  const resetTimer = useRef(0);

  /**
   * 关窗前把焦点从输入框上拿走，让"失焦提交"先落地。不做的话，
   * 正写了一半的系统提示词会随组件卸载无声蒸发 —— 那可能是用户
   * 斟酌了几分钟的长文本。
   */
  const requestClose = useCallback(() => {
    (document.activeElement as HTMLElement | null)?.blur?.();
    onClose();
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
    setResetDone(true);
    window.clearTimeout(resetTimer.current);
    resetTimer.current = window.setTimeout(() => setResetDone(false), 1500);
  };

  const commitThinking = (key: string) => {
    if (key === thinking) return;
    setThinking(key);
    setError("");
    const policy = thinkingFromKey(key);
    setSessionThinking(session.id, policy)
      .then(() => onPatch({ thinking: policy }))
      .catch((e: unknown) => setError(String(e)));
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
    // 从会话根打开：venv 几乎总在项目里，从家目录翻过去纯属折磨。
    const dir = await pickDirectory(session.root);
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
    <Modal className="session-dialog" label="会话设置" onClose={requestClose}>
        <div className="modal-head">
          <span className="modal-title">会话设置</span>
          <span className="modal-queue">{session.title ?? "新会话"}</span>
          <button className="ghost" onClick={requestClose} aria-label="关闭 (Esc)">
            ✕
          </button>
        </div>

        <div className="session-dialog-body">
          {/* 路径常驻而不是藏在 title 里 —— 开错会话的设置时，它是唯一的线索。 */}
          <p className="session-path">{session.root}</p>
          <h3 className="dialog-section" style={{ marginTop: 0 }}>
            采样参数
            <HintTip>留空继承服务方的设置，占位符是继承来的值。</HintTip>
          </h3>
          <div className="samp-grid">
            {SAMPLING_FIELDS.map((f) => (
              <div className="field-row" key={f.key}>
                <label>{f.label}</label>
                <FieldNumber
                  value={samp[f.key] ?? ""}
                  onChange={(e) => setSamp({ ...samp, [f.key]: e.target.value })}
                  onBlur={() => commitSampling(parseSampling(samp))}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                  }}
                  placeholder={inherited[f.key]?.toString() ?? "默认"}
                />
              </div>
            ))}
          </div>
          {/* 按钮常驻：忽隐忽现的按钮像 bug，disabled 才说明"现在没有可恢复的"。 */}
          <div className="samp-reset">
            <button className="ghost" onClick={resetSampling} disabled={!overrides}>
              全部恢复继承
            </button>
            {resetDone ? (
              <span className="hint" role="status" style={{ margin: 0 }}>
                已恢复
              </span>
            ) : null}
          </div>

          <h3 className="dialog-section">
            思考力度
            <HintTip>
              控制推理模型每次请求想多深（reasoning_effort / thinking）。自适应 =
              新指令用中档、工具续轮用低档，省时省钱。「默认」不发任何参数；
              档位和关闭需要端点支持（DeepSeek、GLM、OpenAI 推理模型），
              不支持的端点会拒绝请求，届时改回「默认」。下一轮生效。
            </HintTip>
          </h3>
          {/* 包一层 field-row 撑满：菜单宽度跟着触发框走，触发框太窄
              说明文字就会折行。 */}
          <div className="field-row">
            <FieldSelect
              value={thinking}
              onChange={commitThinking}
              options={THINKING_OPTIONS}
            />
          </div>

          <h3 className="dialog-section">
            Python 虚拟环境
            <HintTip>
              注入 VIRTUAL_ENV 并把 bin 排到 PATH 最前，python / pip 直接落在这个环境。清空恢复系统默认。
              系统选择框默认隐藏 .venv 这类点开头的目录（⌘⇧. 切换显示），也可以直接手输路径。
            </HintTip>
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
          {/* 探测到的 venv 一键填入。已填的不再展示 —— 按钮的意义是"帮你
              绕开藏起来的 .venv"，不是常驻装饰。 */}
          {venvFound.filter((p) => p !== venv).length > 0 ? (
            <div className="venv-found">
              {venvFound
                .filter((p) => p !== venv)
                .map((p) => (
                  <button
                    key={p}
                    className="ghost"
                    title={p}
                    onClick={() => {
                      setVenv(p);
                      void commitVenv(p);
                    }}
                  >
                    检测到 {basename(p)}，使用
                  </button>
                ))}
            </div>
          ) : null}

          <h3 className="dialog-section">
            系统提示词
            <HintTip>追加在内置提示词之后，不替换它。留空只用内置提示词。</HintTip>
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
    </Modal>
  );
}
