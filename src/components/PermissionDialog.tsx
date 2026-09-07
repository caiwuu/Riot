import { useEffect, useRef, useState } from "react";

import { type PermissionAsk, type PermissionMode, type PermissionResponse, renderUiText } from "../bridge";
import { useImeGuard } from "../hooks/useImeGuard";
import { useT } from "../i18n";
import { Markdown } from "./Markdown";
import { useEscLayer } from "./Modal";
import { MODE_LABEL_KEY } from "./pickers";

interface Props {
  ask: PermissionAsk;
  /** 含这一个在内，还有几个请求排着队。 */
  pendingCount: number;
  onAnswer: (r: PermissionResponse) => void;
}

/**
 * 权限确认。
 *
 * `[约束]` 必须显示**将要执行的原文** —— 完整命令、完整路径、完整 diff。
 * 显示摘要等于让用户盲签：他点"允许"时以为批准的是摘要里那件事。
 *
 * 没有"关闭"按钮，Esc 等于拒绝。给一个语义模糊的退出口，用户会用它来
 * 跳过自己没看懂的东西，而没看懂正是最该拒绝的情况。
 */
export function PermissionDialog({ ask, pendingCount, onAnswer }: Props) {
  const { t, tn } = useT();
  const denyRef = useRef<HTMLButtonElement>(null);
  // 答过一次就锁死所有按钮 —— IPC 慢时连点会重复提交同一个决定。
  const [answered, setAnswered] = useState(false);

  const answer = (r: PermissionResponse) => {
    if (answered) return;
    setAnswered(true);
    onAnswer(r);
  };

  useEffect(() => {
    // 焦点落在"拒绝"上。一个习惯性的回车不应该批准一次删除。
    denyRef.current?.focus();
  }, []);

  // Esc 走公共栈：图片查看器叠在权限卡之上时，Esc 只关最上层的查看器，
  // 不会顺手把底下的权限请求也拒了。
  useEscLayer(() => answer({ decision: "deny" }));

  // 内核给出了"可以记住"的规则建议时才显示"总是允许"。没有建议
  // 却显示这个按钮，等于许诺一个不会兑现的行为。
  const rememberable = ask.suggestions.filter((s) => s.type === "add_rule");
  const rules = rememberable
    .map((s) => `${s.tool}${s.pattern ? `(${s.pattern})` : ""}`)
    .join(t("transcript.listSep"));

  return (
    <div className="modal-backdrop">
      <div
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-label={t("transcript.permission.title")}
      >
        <div className="modal-head">
          <span className="modal-tool">{ask.tool_name}</span>
          <span className="modal-title">
            {ask.agent_label
              ? t("transcript.permission.fromTask", { label: ask.agent_label, summary: renderUiText(ask.summary) })
              : renderUiText(ask.summary)}
          </span>
          {/* 并发工具会一次问好几个。不说还剩几个的话，用户答完一个
              又冒出一个，会以为是自己点错了或者程序在重复询问。 */}
          {pendingCount > 1 ? (
            <span className="modal-queue">
              {tn("transcript.permission.pending", pendingCount - 1)}
            </span>
          ) : null}
        </div>

        <Preview preview={ask.preview} />

        <div className="modal-actions">
          <button
            ref={denyRef}
            className="btn-deny"
            disabled={answered}
            onClick={() => answer({ decision: "deny" })}
          >
            {t("transcript.permission.deny")}
            {/* Esc=拒绝是纯键盘捷径，界面上不写出来没人发现得了 */}
            <span className="kbd-hint">esc</span>
          </button>
          {/* 把危险的"总是允许"（写永久规则）推到左侧、降成弱按钮，和
              右下角的主操作"允许一次"拉开距离 —— 相邻且等重时误点一下
              就是持久放权。 */}
          <span className="modal-actions-spacer" />
          {rememberable.length > 0 ? (
            <button
              className="btn-allow-always"
              title={t("transcript.permission.alwaysTitle", { rules })}
              disabled={answered}
              onClick={() => answer({ decision: "allow", remember: rememberable })}
            >
              {t("transcript.permission.always")}
              <span className="allow-always-sub">
                {t("transcript.permission.alwaysSub", { rules })}
              </span>
            </button>
          ) : null}
          <button
            className="btn-allow"
            disabled={answered}
            onClick={() => answer({ decision: "allow" })}
          >
            {t("transcript.permission.once")}
          </button>
        </div>
      </div>
    </div>
  );
}

/**
 * 与 `ask.rs` 的 `OTHER_PREFIX` 对齐。用户点「其他」自己填写时，这段话
 * 编进 `choice` 数组；工具侧剥掉前缀再送给模型。改一边必须改另一边。
 */
const OTHER_PREFIX = "__other:";

/**
 * 模型主动提问（AskUserQuestion）。长在对话流里，不弹窗。
 *
 * 选择题不是危险操作，糊一层遮罩会把正在读的对话挡掉，也打断输入。
 * 交互对齐 Cursor：现成选项点一下就交；「其他」展开输入框，用户自己写。
 *
 * 不绑 Esc、不抢焦点 —— 那是权限弹窗的规矩。这张卡跟在工具卡后面，
 * 用户可能还想回头看上面的上下文，焦点留在输入框更合适。
 */
export function AskChoiceCard({
  ask,
  onAnswer,
}: {
  ask: PermissionAsk;
  onAnswer: (r: PermissionResponse) => void;
}) {
  const { t } = useT();
  const q = ask.preview.kind === "choice" ? ask.preview : null;
  const [picked, setPicked] = useState<string[]>([]);
  const [otherOn, setOtherOn] = useState(false);
  const [other, setOther] = useState("");
  const [answered, setAnswered] = useState(false);
  const ime = useImeGuard();

  if (!q) return null;

  const submit = (ids: string[], includeOther: boolean) => {
    if (answered) return;
    const choice = [...ids];
    if (includeOther) {
      const custom = other.trim();
      if (custom) choice.push(`${OTHER_PREFIX}${custom}`);
    }
    if (choice.length === 0) return;
    setAnswered(true);
    // 输入内容到提交才算用完 —— 收起「其他」时清掉会把半段话静默丢弃
    setOther("");
    onAnswer({ decision: "allow", choice });
  };

  const deny = () => {
    if (answered) return;
    setAnswered(true);
    onAnswer({ decision: "deny" });
  };

  const toggle = (id: string) =>
    setPicked((prev) => (prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id]));

  const canSubmit = picked.length > 0 || other.trim().length > 0;
  const needConfirm = q.allow_multiple || otherOn;

  return (
    <div className="plan-card ask-card" role="region" aria-label={t("transcript.ask.ariaLabel")}>
      <div className="plan-card-head">
        <span className="plan-card-badge">{t("transcript.ask.badge")}</span>
        <span className="plan-card-title">{q.question}</span>
      </div>

      <div className="choice-list ask-choices">
        {q.options.map((o) => (
          <button
            key={o.id}
            type="button"
            className={q.allow_multiple && picked.includes(o.id) ? "choice-opt on" : "choice-opt"}
            disabled={answered}
            onClick={() => (q.allow_multiple ? toggle(o.id) : submit([o.id], false))}
          >
            {o.label}
          </button>
        ))}
        <button
          type="button"
          className={otherOn ? "choice-opt on choice-opt-other" : "choice-opt choice-opt-other"}
          disabled={answered}
          onClick={() => setOtherOn((v) => !v)}
        >
          {t("transcript.ask.other")}
        </button>
      </div>

      {otherOn ? (
        <textarea
          className="plan-feedback"
          value={other}
          onChange={(e) => setOther(e.target.value)}
          onCompositionStart={ime.onCompositionStart}
          onCompositionEnd={ime.onCompositionEnd}
          onKeyDown={(e) => {
            // 组字中的回车是确认候选词，不是提交。
            if (e.key === "Enter" && !e.shiftKey && !ime.isComposing(e)) {
              e.preventDefault();
              if (canSubmit) submit(q.allow_multiple ? picked : [], true);
            }
          }}
          placeholder={t("transcript.ask.otherPlaceholder")}
          rows={2}
          spellCheck={false}
          autoFocus
        />
      ) : null}

      <div className="plan-card-actions">
        <button className="btn-deny" disabled={answered} onClick={deny}>
          {t("transcript.ask.skip")}
        </button>
        <span className="plan-card-spacer" />
        {needConfirm ? (
          <button
            className="btn-allow"
            disabled={answered || !canSubmit}
            onClick={() => submit(q.allow_multiple ? picked : [], true)}
          >
            {t("common.ok")}
          </button>
        ) : null}
      </div>
    </div>
  );
}

/**
 * 模型建议换工作方式（SwitchMode 工具）。长在对话流里，不弹窗（Cursor 同款）。
 *
 * 两个方向：agent → plan（"这活改动面大，先定方案"）和 plan → agent
 * （用户在聊天里说"开始做吧"，模型请他确认）。卡上放模型给的理由原文，
 * 用户看着理由决定；「切换」把 set_mode 一并交回宿主 —— 同一轮内立即
 * 生效，模型的下一个工具调用已经按新模式判定。
 *
 * 切回 agent 用的是**进规划前那一档**（`execMode`），不是内核建议里的
 * 兜底值：用户开着「编辑放行」进的规划，出来不该变成逐步确认。
 *
 * 不绑 Esc、不抢焦点 —— 那是权限弹窗的规矩。这张卡不是危险操作，用户
 * 可能还想回头看上面的上下文。
 */
export function ModeSwitchCard({
  ask,
  execMode,
  onAnswer,
}: {
  ask: PermissionAsk;
  /** 这个会话进规划前用的权限档，切回 agent 时落成它。 */
  execMode: PermissionMode;
  onAnswer: (r: PermissionResponse) => void;
}) {
  const { t } = useT();
  const [answered, setAnswered] = useState(false);
  const target = ask.suggestions.find((s) => s.type === "set_mode")?.mode ?? "plan";
  const toPlan = target === "plan";
  // 理由是模型原文（raw）；plain 是词典键，只在模型没给理由时出现。
  const why =
    ask.preview.kind === "raw"
      ? ask.preview.text
      : ask.preview.kind === "plain"
        ? renderUiText(ask.preview.text)
        : "";
  const execLabelKey = MODE_LABEL_KEY[execMode];
  const execLabel = execLabelKey ? t(execLabelKey) : execMode;

  const answer = (r: PermissionResponse) => {
    if (answered) return;
    setAnswered(true);
    onAnswer(r);
  };

  return (
    <div className="plan-card mode-card" role="region" aria-label={t("transcript.mode.ariaLabel")}>
      <div className="plan-card-head">
        <span className="plan-card-badge">{t("transcript.mode.badge")}</span>
        <span className="plan-card-title">
          {toPlan ? t("transcript.mode.toPlanTitle") : t("transcript.mode.toAgentTitle")}
        </span>
      </div>

      {why ? (
        <div className="mode-card-why">
          <Markdown text={why} />
        </div>
      ) : null}

      <div className="plan-card-actions">
        <button className="btn-deny" disabled={answered} onClick={() => answer({ decision: "deny" })}>
          {toPlan ? t("transcript.mode.stayAgent") : t("transcript.mode.stayPlan")}
        </button>
        <span className="plan-card-spacer" />
        <button
          className="btn-allow"
          disabled={answered}
          onClick={() =>
            answer({
              decision: "allow",
              remember: [{ type: "set_mode", mode: toPlan ? "plan" : execMode, scope: "session" }],
            })
          }
        >
          {toPlan ? t("transcript.mode.switchPlan") : t("transcript.mode.switchAgent")}
          <span className="allow-always-sub">
            {toPlan
              ? t("transcript.mode.switchPlanSub")
              : t("transcript.mode.switchAgentSub", { mode: execLabel })}
          </span>
        </button>
      </div>
    </div>
  );
}

function Preview({ preview }: { preview: PermissionAsk["preview"] }) {
  const { t, tn } = useT();
  switch (preview.kind) {
    case "command":
      return (
        <div className="preview">
          <div className="preview-label">{t("transcript.preview.runIn", { cwd: preview.cwd })}</div>
          <pre className="preview-cmd">{preview.command}</pre>
        </div>
      );

    case "file_write":
      return (
        <div className="preview">
          <div className="preview-label">
            {t("transcript.preview.writeFile", { lines: preview.lines, bytes: preview.bytes })}
          </div>
          <pre className="preview-cmd">{preview.path}</pre>
          {/* 内容前 N 行 —— 只给路径和字节数等于让用户盲签。 */}
          {preview.preview ? (
            <pre className="preview-diff">
              {preview.preview.split("\n").map((line, i) => (
                <div key={i} className="add">
                  + {line}
                </div>
              ))}
              {preview.truncated ? (
                <div className="preview-more">
                  {tn("transcript.preview.truncated", preview.lines)}
                </div>
              ) : null}
            </pre>
          ) : null}
        </div>
      );

    case "file_edit":
      return (
        <div className="preview">
          <div className="preview-label">
            {t("transcript.preview.editFile", { path: preview.path })}
          </div>
          <pre className="preview-diff">
            {preview.diff.split("\n").map((line, i) => (
              <div key={i} className={line.startsWith("+") ? "add" : line.startsWith("-") ? "del" : ""}>
                {line}
              </div>
            ))}
          </pre>
        </div>
      );

    case "network_fetch":
      return (
        <div className="preview">
          <div className="preview-label">{t("transcript.preview.network")}</div>
          <pre className="preview-cmd">{preview.url}</pre>
        </div>
      );

    // choice 由 AskChoiceCard 渲染，权限弹窗走不到这里。
    case "choice":
      return null;

    case "plain":
      return (
        <div className="preview">
          <pre className="preview-cmd">{renderUiText(preview.text)}</pre>
        </div>
      );

    default:
      return (
        <div className="preview">
          <pre className="preview-cmd">{preview.text}</pre>
        </div>
      );
  }
}
