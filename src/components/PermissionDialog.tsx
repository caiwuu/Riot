import { useEffect, useRef, useState } from "react";

import type { PermissionAsk, PermissionMode, PermissionResponse } from "../bridge";
import { Markdown } from "./Markdown";
import { useEscLayer } from "./Modal";

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

  return (
    <div className="modal-backdrop">
      <div className="modal" role="dialog" aria-modal="true" aria-label="权限确认">
        <div className="modal-head">
          <span className="modal-tool">{ask.tool_name}</span>
          <span className="modal-title">{ask.summary}</span>
          {/* 并发工具会一次问好几个。不说还剩几个的话，用户答完一个
              又冒出一个，会以为是自己点错了或者程序在重复询问。 */}
          {pendingCount > 1 ? (
            <span className="modal-queue">还有 {pendingCount - 1} 个待确认</span>
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
            拒绝
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
              title={`写一条永久规则（可在 设置 → 权限 撤销）：${rememberable
                .map((s) => `${s.tool}${s.pattern ? `(${s.pattern})` : ""}`)
                .join("、")}`}
              disabled={answered}
              onClick={() => answer({ decision: "allow", remember: rememberable })}
            >
              总是允许（记规则）
              <span className="allow-always-sub">
                以后不再询问 ·{" "}
                {rememberable
                  .map((s) => `${s.tool}${s.pattern ? `(${s.pattern})` : ""}`)
                  .join("、")}
              </span>
            </button>
          ) : null}
          <button
            className="btn-allow"
            disabled={answered}
            onClick={() => answer({ decision: "allow" })}
          >
            允许一次
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
  const q = ask.preview.kind === "choice" ? ask.preview : null;
  const [picked, setPicked] = useState<string[]>([]);
  const [otherOn, setOtherOn] = useState(false);
  const [other, setOther] = useState("");
  const [answered, setAnswered] = useState(false);

  if (!q) return null;

  const submit = (ids: string[], includeOther: boolean) => {
    if (answered) return;
    const choice = [...ids];
    if (includeOther) {
      const t = other.trim();
      if (t) choice.push(`${OTHER_PREFIX}${t}`);
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
    <div className="plan-card ask-card" role="region" aria-label="需要你决定">
      <div className="plan-card-head">
        <span className="plan-card-badge">决定</span>
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
          其他
        </button>
      </div>

      {otherOn ? (
        <textarea
          className="plan-feedback"
          value={other}
          onChange={(e) => setOther(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              if (canSubmit) submit(q.allow_multiple ? picked : [], true);
            }
          }}
          placeholder="自己写…（Enter 提交）"
          rows={2}
          spellCheck={false}
          autoFocus
        />
      ) : null}

      <div className="plan-card-actions">
        <button className="btn-deny" disabled={answered} onClick={deny}>
          跳过
        </button>
        <span className="plan-card-spacer" />
        {needConfirm ? (
          <button
            className="btn-allow"
            disabled={answered || !canSubmit}
            onClick={() => submit(q.allow_multiple ? picked : [], true)}
          >
            确定
          </button>
        ) : null}
      </div>
    </div>
  );
}

/** 批准后切到哪个档，按钮上要写清楚 —— 这是批准动作的一部分，不是细节。 */
const APPROVE_LABEL: Partial<Record<PermissionMode, { label: string; sub: string }>> = {
  acceptEdits: { label: "批准，自动接受编辑", sub: "文件修改直接放行，命令仍询问" },
  default: { label: "批准，逐步确认", sub: "每个写操作都再问一次" },
};

/**
 * 计划还在往 tool_input 里写的时候用。外观跟批准卡同一套，
 * 没有按钮 —— 写完才轮到用户审。
 */
export function PlanDraft({ text }: { text: string }) {
  const bodyRef = useRef<HTMLDivElement>(null);
  // 只有本来就贴着底部才继续跟随 —— 用户上滚回读时，新 token 不能
  // 把他一次次拽回底部。
  const stickRef = useRef(true);
  useEffect(() => {
    const el = bodyRef.current;
    if (el && stickRef.current) el.scrollTop = el.scrollHeight;
  }, [text]);

  return (
    <div className="plan-card plan-draft" role="status" aria-label="正在撰写计划">
      <div className="plan-card-head">
        <span className="plan-card-badge">计划</span>
        <span className="plan-card-title">正在撰写…</span>
      </div>
      <div
        className="plan-body"
        ref={bodyRef}
        onScroll={(e) => {
          const el = e.currentTarget;
          stickRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
      >
        {text ? <Markdown text={text} /> : null}
        <span className="plan-caret" aria-hidden />
      </div>
    </div>
  );
}

/**
 * 计划批准卡（对照 Claude Code 的 "Ready to code?"，但**长在对话流里**）。
 *
 * 内联而不是弹窗：计划在模型侦察几分钟之后才到，弹窗会突然糊在脸上；
 * 而它本来就是对话的一部分 —— 跟在 ExitPlanMode 的工具卡后面，随流
 * 滚动，答完就地消失、工具卡随之落定结果。
 *
 * 三个交互决策：计划按 Markdown 渲染（它是要读的文档）；批准按钮按
 * 执行档分两个（批准之后再逐个确认编辑，等于把刚做的决定再问一遍，
 * 所以"自动接受编辑"是主选项）；打回可以带反馈 —— 不带的话模型只
 * 知道"被拒了"，不知道往哪改。
 */
export function PlanApprovalCard({
  ask,
  onAnswer,
}: {
  ask: PermissionAsk;
  onAnswer: (r: PermissionResponse) => void;
}) {
  const [feedback, setFeedback] = useState("");
  const [answered, setAnswered] = useState(false);
  const modes = ask.suggestions.flatMap((s) => (s.type === "set_mode" ? [s.mode] : []));
  const plan = ask.preview.kind === "plain" ? ask.preview.text : "";

  const answer = (r: PermissionResponse) => {
    if (answered) return;
    setAnswered(true);
    onAnswer(r);
  };

  const approve = (mode: PermissionMode) => {
    const chosen = ask.suggestions.find((s) => s.type === "set_mode" && s.mode === mode);
    answer({ decision: "allow", remember: chosen ? [chosen] : [] });
  };

  return (
    <div className="plan-card" role="region" aria-label="计划批准">
      <div className="plan-card-head">
        <span className="plan-card-badge">计划</span>
        <span className="plan-card-title">审阅后选择怎么执行</span>
      </div>

      <div className="plan-body">
        <Markdown text={plan || "（计划为空）"} />
      </div>

      <textarea
        className="plan-feedback"
        value={feedback}
        onChange={(e) => setFeedback(e.target.value)}
        placeholder="要打回的话，告诉它往哪改（可留空）"
        rows={2}
        spellCheck={false}
      />

      <div className="plan-card-actions">
        <button
          className="btn-deny"
          disabled={answered}
          onClick={() =>
            answer({
              decision: "deny",
              ...(feedback.trim() ? { message: feedback.trim() } : {}),
            })
          }
        >
          打回，继续规划
        </button>
        <span className="plan-card-spacer" />
        {modes.length > 0 ? (
          modes.map((m, i) => {
            const label = APPROVE_LABEL[m] ?? { label: `批准（${m}）`, sub: "" };
            return (
              <button
                key={m}
                className={i === 0 ? "btn-allow" : "btn-allow-always"}
                disabled={answered}
                onClick={() => approve(m)}
              >
                {label.label}
                {label.sub ? <span className="allow-always-sub">{label.sub}</span> : null}
              </button>
            );
          })
        ) : (
          // 内核没给 set_mode 建议时也得有出口 —— 只剩"打回"的计划卡是死胡同
          <button
            className="btn-allow"
            disabled={answered}
            onClick={() => answer({ decision: "allow", remember: [] })}
          >
            批准
          </button>
        )}
      </div>
    </div>
  );
}

function Preview({ preview }: { preview: PermissionAsk["preview"] }) {
  switch (preview.kind) {
    case "command":
      return (
        <div className="preview">
          <div className="preview-label">将在 {preview.cwd} 执行</div>
          <pre className="preview-cmd">{preview.command}</pre>
        </div>
      );

    case "file_write":
      return (
        <div className="preview">
          <div className="preview-label">
            写入文件（{preview.lines} 行 · {preview.bytes} 字节）
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
                <div className="preview-more">… 共 {preview.lines} 行，仅显示前段</div>
              ) : null}
            </pre>
          ) : null}
        </div>
      );

    case "file_edit":
      return (
        <div className="preview">
          <div className="preview-label">修改 {preview.path}</div>
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
          <div className="preview-label">访问网络</div>
          <pre className="preview-cmd">{preview.url}</pre>
        </div>
      );

    // choice 由 AskChoiceCard 渲染，权限弹窗走不到这里。
    case "choice":
      return null;

    default:
      return (
        <div className="preview">
          <pre className="preview-cmd">{preview.text}</pre>
        </div>
      );
  }
}
