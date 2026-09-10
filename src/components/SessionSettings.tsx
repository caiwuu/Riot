import { useCallback, useEffect, useRef, useState } from "react";

import {
  type PromptPreset,
  type Sampling,
  type SessionInfo,
  type ThinkingPolicy,
  detectVenvs,
  setSessionPythonVenv,
  setSessionSampling,
  setSessionSystemPrompt,
  setSessionThinking,
} from "../bridge";
import { type MessageKey, useT } from "../i18n";
import { findPreset, presetLabel, presetSummary } from "../lib/prompts";
import { useDirectoryPicker } from "./DirPicker";
import {
  type SamplingDraft,
  SAMPLING_FIELDS,
  parseSampling,
  sameSampling,
  samplingDraft,
} from "../lib/sampling";
import { SamplingSliders } from "./FieldSlider";
import { FieldSelect, type FieldOption } from "./FieldSelect";
import { HintTip } from "./HintTip";
import { Modal } from "./Modal";
import { ResizableTextarea } from "./ResizableTextarea";
import { basename } from "../pathDisplay";

/**
 * 会话设置弹窗：只管**这个会话**的东西 —— 采样覆盖、Python 虚拟环境、
 * 追加的系统提示词。全局的新会话默认权限 / 联网在侧栏的「设置」里；
 * 当前会话的权限档在顶栏标题旁的下拉里切，模型在输入框上切。
 *
 * `[约束]` 所有字段都是"改完下一轮生效"，真值在宿主的 Session 上。
 * 提交成功后通过 `onPatch` 回写 App 的会话列表 —— 不回写的话，关掉
 * 弹窗再打开，显示的还是启动时 listSessions 拉到的旧值。
 */

/** 思考策略的下拉项。固定档位摊平成一级选项 —— 六个选项不值得两级菜单。
 *  说明走 hint（第二行灰字）：塞进主标签会挤成两行大字，见谁选谁难受。
 *  存的是词典键，渲染时再 `t()`。 */
const THINKING_OPTIONS: { value: string; labelKey: MessageKey; hintKey?: MessageKey }[] = [
  { value: "default", labelKey: "common.default", hintKey: "composer.thinking.default.hint" },
  { value: "adaptive", labelKey: "composer.thinking.adaptive", hintKey: "composer.thinking.adaptive.hint" },
  { value: "low", labelKey: "composer.thinking.low" },
  { value: "medium", labelKey: "composer.thinking.medium" },
  { value: "high", labelKey: "composer.thinking.high" },
  { value: "disabled", labelKey: "composer.thinking.disabled", hintKey: "composer.thinking.disabled.hint" },
];

/** 下拉里表示「正文是手写的，不对应库里任何一条」。不会是真的 id。 */
const CUSTOM_PROMPT = "\u0000custom";

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
  presets,
  onSavePreset,
  onPatch,
  onClose,
}: {
  session: SessionInfo;
  /** 继承来的采样默认值（当前激活 provider 的），没覆盖时数字格就显示它。 */
  inherited: Sampling;
  /** 设置里收藏的提示词。空 = 只能自己写。 */
  presets: PromptPreset[];
  /** 把当前正文存进提示词库。 */
  onSavePreset: (body: string) => Promise<void>;
  /** 提交成功后回写 App 里的会话信息。 */
  onPatch: (patch: Partial<SessionInfo>) => void;
  onClose: () => void;
}) {
  const { t } = useT();
  // 数字字段走字符串草稿：绑成 number 的话 "0."、"-" 这种中间态会被吃掉。
  const [samp, setSamp] = useState<SamplingDraft>(() => samplingDraft(session.sampling));
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
  /** 选预设时被顶掉的旧正文。null = 没有可撤的东西。 */
  const [replaced, setReplaced] = useState<string | null>(null);
  /** 「存为提示词」的点击回执。存完按钮自己会消失，但那个变化太安静。 */
  const [presetSaved, setPresetSaved] = useState(false);
  const presetTimer = useRef(0);

  /**
   * 关窗前把焦点从输入框上拿走，让"失焦提交"先落地。不做的话，
   * 正写了一半的系统提示词会随组件卸载无声蒸发 —— 那可能是用户
   * 斟酌了几分钟的长文本。
   */
  const requestClose = useCallback(() => {
    (document.activeElement as HTMLElement | null)?.blur?.();
    onClose();
  }, [onClose]);

  // 选了「模型默认」（null）也是一次覆盖 —— 它同样要能被"全部恢复继承"收回去。
  const overrides = SAMPLING_FIELDS.filter((f) => {
    const v = samp[f.key];
    return v === null || (v ?? "").trim() !== "";
  }).length;

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

  const dirPicker = useDirectoryPicker();
  const pickVenv = async () => {
    // 从会话根打开：venv 几乎总在项目里，从家目录翻过去纯属折磨。
    const dir = await dirPicker.pick(session.root);
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

  /** 当前正文对应库里的哪一条。靠内容反查而不是存 id —— 存 id 的话，
   *  用户手改两个字之后下拉还理直气壮地显示着预设名。 */
  const matched = findPreset(presets, prompt);
  const promptChoice = matched ? matched.id : prompt.trim() ? CUSTOM_PROMPT : "";

  const promptOptions: FieldOption[] = [
    {
      value: "",
      label: t("composer.sessionSettings.prompt.none"),
      hint: t("composer.sessionSettings.prompt.none.hint"),
    },
    ...presets.map((p) => ({ value: p.id, label: presetLabel(p), hint: presetSummary(p) })),
  ];
  // 「自定义」是当前状态的名字，不是一个能选的目标 —— 手写的内容没有
  // 第二份可以切回来。它只在正文确实脱离库时出现，让触发框有话可说。
  if (promptChoice === CUSTOM_PROMPT) {
    promptOptions.push({
      value: CUSTOM_PROMPT,
      label: t("composer.sessionSettings.prompt.custom"),
      hint: t("composer.sessionSettings.prompt.custom.hint"),
    });
  }

  const thinkingOptions: FieldOption[] = THINKING_OPTIONS.map((o) => ({
    value: o.value,
    label: t(o.labelKey),
    ...(o.hintKey ? { hint: t(o.hintKey) } : {}),
  }));

  const choosePreset = (id: string) => {
    if (id === promptChoice || id === CUSTOM_PROMPT) return;
    const body = presets.find((p) => p.id === id)?.body.trim() ?? "";
    // 顶掉手写的内容才留后路：那可能是刚斟酌了几分钟的长文本，库里没有
    // 第二份。顶掉的是库里另一条时不吭声 —— 再挑一次就回去了，不算丢。
    // 事前弹确认更差：每次换预设都要点一下，而多数时候框里本来就空着。
    setReplaced(prompt.trim() && !matched ? prompt : null);
    setPrompt(body);
    commitPrompt(body);
  };

  const undoReplace = () => {
    const back = replaced ?? "";
    setReplaced(null);
    setPrompt(back);
    commitPrompt(back);
  };

  const savePreset = async () => {
    const body = prompt.trim();
    if (!body || matched) return;
    setError("");
    try {
      await onSavePreset(body);
      setPresetSaved(true);
      window.clearTimeout(presetTimer.current);
      presetTimer.current = window.setTimeout(() => setPresetSaved(false), 1800);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <Modal className="session-dialog" label={t("composer.sessionSettings.title")} onClose={requestClose}>
        <div className="modal-head">
          <span className="modal-title">{t("composer.sessionSettings.title")}</span>
          <span className="modal-queue">
            {session.title ?? t("composer.sessionSettings.newSession")}
          </span>
          <button className="ghost" onClick={requestClose} aria-label={t("composer.closeEsc")}>
            ✕
          </button>
        </div>

        <div className="session-dialog-body">
          {/* 路径常驻而不是藏在 title 里 —— 开错会话的设置时，它是唯一的线索。 */}
          <p className="session-path">{session.root}</p>
          <h3 className="dialog-section" style={{ marginTop: 0 }}>
            {t("composer.sampling.title")}
            <HintTip>{t("composer.sessionSettings.sampling.hint")}</HintTip>
            {/* 按钮常驻：忽隐忽现的按钮像 bug，disabled 才说明"现在没有可恢复的"。 */}
            <button
              className="ghost samp-reset"
              onClick={resetSampling}
              disabled={!overrides}
            >
              {t("composer.sessionSettings.sampling.reset")}
            </button>
            {resetDone ? (
              <span className="hint samp-reset-done" role="status">
                {t("composer.sessionSettings.sampling.resetDone")}
              </span>
            ) : null}
          </h3>
          <SamplingSliders
            draft={samp}
            inherited={inherited}
            onChange={(key, value) => setSamp((s) => ({ ...s, [key]: value }))}
            onCommit={(next) => commitSampling(parseSampling(next))}
          />

          <h3 className="dialog-section">
            {t("composer.thinking.title")}
            <HintTip>{t("composer.thinking.hint")}</HintTip>
          </h3>
          {/* 包一层 field-row 撑满：菜单宽度跟着触发框走，触发框太窄
              说明文字就会折行。 */}
          <div className="field-row">
            <FieldSelect
              value={thinking}
              onChange={commitThinking}
              options={thinkingOptions}
            />
          </div>

          <h3 className="dialog-section">
            {t("composer.sessionSettings.venv")}
            <HintTip>{t("composer.sessionSettings.venv.hint")}</HintTip>
          </h3>
          <div className="key-row">
            <input
              value={venv}
              onChange={(e) => setVenv(e.target.value)}
              onBlur={(e) => void commitVenv(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") (e.target as HTMLInputElement).blur();
              }}
              placeholder={t("composer.sessionSettings.venv.placeholder")}
              spellCheck={false}
            />
            <button onClick={() => void pickVenv()}>{t("composer.sessionSettings.venv.pick")}</button>
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
                    {t("composer.sessionSettings.venv.found", { name: basename(p) })}
                  </button>
                ))}
            </div>
          ) : null}

          <h3 className="dialog-section">
            {t("composer.sessionSettings.prompt")}
            <HintTip>{t("composer.sessionSettings.prompt.hint")}</HintTip>
            {prompt.trim() && !matched ? (
              <button className="ghost prompt-save" onClick={() => void savePreset()}>
                {t("composer.sessionSettings.prompt.save")}
              </button>
            ) : null}
            {presetSaved ? (
              <span className="hint prompt-saved" role="status">
                {t("composer.sessionSettings.prompt.saved")}
              </span>
            ) : null}
          </h3>
          {/* 库是空的时候不摆下拉：只有「不使用」一项的菜单点开是一场空。
              这时「存为提示词」就是攒第一条的入口。 */}
          {presets.length > 0 ? (
            <div className="field-row">
              <FieldSelect
                value={promptChoice}
                onChange={choosePreset}
                options={promptOptions}
                title={t("composer.sessionSettings.prompt.pickTitle")}
              />
            </div>
          ) : null}
          <ResizableTextarea
            className="preset-body-input"
            value={prompt}
            onChange={(ev) => {
              setPrompt(ev.target.value);
              // 一旦动手改，"撤销回替换前"就不再是用户想要的那个状态了。
              setReplaced(null);
            }}
            onBlur={(ev) => commitPrompt(ev.target.value)}
            placeholder={t("composer.sessionSettings.prompt.placeholder")}
            rows={6}
            spellCheck={false}
          />
          {replaced !== null ? (
            <div className="prompt-undo" role="status">
              <span className="hint">{t("composer.sessionSettings.prompt.replaced")}</span>
              <button className="ghost" onClick={undoReplace}>
                {t("composer.undo")}
              </button>
            </div>
          ) : null}

          {error ? <p className="form-error">{error}</p> : null}
        </div>
        {dirPicker.element}
    </Modal>
  );
}
