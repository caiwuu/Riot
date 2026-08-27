import { useEffect, useRef, useState } from "react";

import {
  type ConfigStatus,
  type PermissionMode,
  type SandboxMode,
  type SandboxStatus,
  sandboxInstall,
  sandboxStatus,
  setConfig,
} from "../../bridge";
import {
  DEFAULT_COMPACT_THRESHOLD as DEFAULT_COMPACT_AT,
  MAX_COMPACT_THRESHOLD as MAX_COMPACT_AT,
  MIN_COMPACT_THRESHOLD as MIN_COMPACT_AT,
} from "../../lib/contextWindow";
import { FieldNumber } from "../FieldNumber";
import { HintTip } from "../HintTip";
import { type AskConfirm, FormError, blurOnEnter } from "./shared";

/** 和宿主侧 config::normalize 的夹紧区间保持一致。 */
const MIN_TIMEOUT = 5;
const MAX_TIMEOUT = 3600;
const MIN_TURNS = 1;
const MAX_TURNS = 1000;
const DEFAULT_TURNS = 48;

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

/**
 * 「开关说开着」和「这台机器上真的隔离着」之间的差额。
 *
 * 只在两者不一致、或有事要用户做时才出声：一切正常时多一行"生效中"是噪音，
 * 而噪音会让真正要紧的那次提示也被略过。
 */
function SandboxReality({
  sbx,
  error,
  wanted,
  installing,
  onInstall,
}: {
  sbx: SandboxStatus | null;
  error: string;
  wanted: SandboxMode;
  installing: boolean;
  onInstall: () => void;
}) {
  const line = (cls: string, text: string) => (
    <p className={cls} style={{ margin: "8px 0 0" }}>
      {text}
    </p>
  );
  // 探测失败先报，且不受「选了不隔离就不出声」的约束：这说明的是应用自己
  // 有问题，和用户选了哪一档无关。
  if (error) {
    return line("form-error", `查不到隔离是否生效（${error}）—— 下面的选择可能不反映实际情况。`);
  }
  if (!sbx || wanted === "off") return null;

  if (!sbx.implemented) {
    return line("hint", "这个平台还没有系统级隔离，选了也不会生效 —— 实际仍然逐条询问。");
  }
  if (sbx.blocker?.kind === "needsElevatedInstall") {
    return (
      <>
        {line(
          "form-error",
          "还没安装，所以当前并没有隔离：命令照常直接跑，只剩规则判断和逐条询问拦着。",
        )}
        <div className="pack-actions" style={{ marginTop: 6 }}>
          <button disabled={installing} onClick={onInstall}>
            {installing ? "等待权限确认…" : "安装（需要管理员）"}
          </button>
        </div>
      </>
    );
  }
  if (sbx.blocker?.kind === "broken") {
    return line("form-error", `装过但用不了，当前没有隔离：${sbx.blocker.error}`);
  }
  // 这一档在 Windows 上会整档降级成不隔离（断网要靠 WFP，而那一半没装）。
  // 不说的话，用户选了更严的档位反而什么都没得到。
  if (wanted === "workspaceWriteNoNet" && !sbx.networkIsolation) {
    return line(
      "form-error",
      "这个平台还断不了网，所以整档退回不隔离 —— 想要隔离请改选「隔离（推荐）」。",
    );
  }
  return null;
}

export function PermissionPane({
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

  // 开关是**意图**，这个是**现实**。两者会分叉：Windows 上没跑过提权安装
  // 时，每轮激活都静默失败、命令照常裸跑，而界面上看不出区别 —— 用户以为
  // 开着隔离，还得多点一堆确认框却不知道为什么。
  const [sbx, setSbx] = useState<SandboxStatus | null>(null);
  const [sbxError, setSbxError] = useState("");
  const [installing, setInstalling] = useState(false);
  const refreshSbx = () => {
    sandboxStatus()
      .then((s) => {
        setSbx(s);
        setSbxError("");
      })
      // `[约束]` 探测失败要说出来，不能静默不显示。这一整块的存在意义就是
      // 「别让用户以为隔离着、其实没有」—— 而查不到状态时什么都不画，和
      // 「一切正常」在屏幕上长得一模一样，正好复刻了它要解决的那个问题。
      // （实际踩到过：dev server 是在这两个命令加进去之前起的，invoke 被
      // ACL 拒掉，界面上什么都没有，看起来像功能没做。）
      .catch((e: unknown) => {
        setSbx(null);
        setSbxError(String(e));
      });
  };
  useEffect(() => {
    refreshSbx();
    // 只在打开设置时探一次。它不会自己变 —— 唯一会改变它的是下面那个安装
    // 按钮，而那条路径装完自己会刷。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const runInstall = () => {
    askConfirm({
      title: "现在安装命令隔离？",
      // 两次不是笔误，要提前说 —— 不说的话第二个弹窗看起来像出了问题。
      body: "会弹出两次 Windows 权限确认（UAC）：第一次建一个专用的低权限账户，第二次摘掉它自带的联网限制（不摘的话沙箱内会彻底断网）。只需要装一次。",
      confirmLabel: "开始安装",
      action: () => {
        setInstalling(true);
        setError("");
        sandboxInstall()
          .catch((e: unknown) => setError(String(e)))
          .finally(() => {
            setInstalling(false);
            refreshSbx();
          });
      },
    });
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
            边界由内核守着。macOS 开箱可用；Windows 需要装一次（下面会提示）。
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
        <SandboxReality
          sbx={sbx}
          error={sbxError}
          wanted={sandbox}
          installing={installing}
          onInstall={runInstall}
        />
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
          默认压缩阈值
          <HintTip>
            会话历史估算超过这个 token 数时自动摘要压缩。只对<b>没填上下文窗口</b>的模型生效 ——
            填了窗口的按窗口算（在输入框的模型菜单里选，或在「服务方 → 模型」里填）。范围{" "}
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
