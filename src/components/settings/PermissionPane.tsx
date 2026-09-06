import { useEffect, useRef, useState } from "react";

import {
  type ConfigStatus,
  type PermissionMode,
  type SandboxMode,
  type SandboxStatus,
  sandboxInstall,
  sandboxStatus,
  sandboxUninstall,
  setConfig,
} from "../../bridge";
import {
  DEFAULT_COMPACT_THRESHOLD as DEFAULT_COMPACT_AT,
  MAX_COMPACT_THRESHOLD as MAX_COMPACT_AT,
  MIN_COMPACT_THRESHOLD as MIN_COMPACT_AT,
} from "../../lib/contextWindow";
import { type MessageKey, numberFormat, useT } from "../../i18n";
import { FieldNumber } from "../FieldNumber";
import { ResizableTextarea } from "../ResizableTextarea";
import { Card, CardBlock, Group, Row } from "./layout";
import { type AskConfirm, FormError, Switch, blurOnEnter } from "./shared";

/** 和宿主侧 config::normalize 的夹紧区间保持一致。 */
const MIN_TIMEOUT = 5;
const MAX_TIMEOUT = 3600;
const MIN_TURNS = 1;
const MAX_TURNS = 1000;
/** 见 Rust 侧 `default_max_turns`。 */
const DEFAULT_TURNS = 120;
/** 见 Rust 侧 `default_permission_mode`。 */
const DEFAULT_MODE: PermissionMode = "bypassPermissions";

const SANDBOX_MODES: { id: SandboxMode; labelKey: MessageKey; descKey: MessageKey; danger?: boolean }[] = [
  {
    id: "workspaceWrite",
    labelKey: "settings.permission.sandbox.workspaceWrite",
    descKey: "settings.permission.sandbox.workspaceWrite.desc",
  },
  {
    id: "workspaceWriteNoNet",
    labelKey: "settings.permission.sandbox.workspaceWriteNoNet",
    descKey: "settings.permission.sandbox.workspaceWriteNoNet.desc",
  },
  {
    id: "off",
    labelKey: "settings.permission.sandbox.off",
    descKey: "settings.permission.sandbox.off.desc",
    danger: true,
  },
];

const MODES: { id: PermissionMode; labelKey: MessageKey; descKey: MessageKey; danger?: boolean }[] = [
  {
    id: "default",
    labelKey: "settings.permission.mode.default",
    descKey: "settings.permission.mode.default.desc",
  },
  {
    id: "acceptEdits",
    labelKey: "settings.permission.mode.acceptEdits",
    descKey: "settings.permission.mode.acceptEdits.desc",
  },
  {
    id: "plan",
    labelKey: "settings.permission.mode.plan",
    descKey: "settings.permission.mode.plan.desc",
  },
  {
    id: "auto",
    labelKey: "settings.permission.mode.auto",
    // 不写"自动放行安全操作"就完了 —— 用户会以为它替他做了全部判断。
    // 要点出两件事：靠的是小模型（所以要配便宜档），以及它压不过安全检查。
    descKey: "settings.permission.mode.auto.desc",
  },
  {
    id: "bypassPermissions",
    labelKey: "settings.permission.mode.bypassPermissions",
    // 必须点出"仍会拦"。写成"所有操作不再询问"是假承诺：用户照着这句话
    // 挂机走人，回来发现任务停在一个弹窗上。
    descKey: "settings.permission.mode.bypassPermissions.desc",
    danger: true,
  },
  {
    id: "unattended",
    labelKey: "settings.permission.mode.unattended",
    descKey: "settings.permission.mode.unattended.desc",
    danger: true,
  },
];

// 安装/卸载入口只在 Windows 出现：macOS 的隔离用系统自带的 sandbox-exec，
// 没有装卸的生命周期。UA 判断和 main.tsx 的材质门控是同一招。
const IS_WINDOWS = navigator.userAgent.includes("Windows");

/** 出厂档按平台分，见 Rust 侧 `SandboxMode::default`。 */
const DEFAULT_SANDBOX: SandboxMode = IS_WINDOWS ? "off" : "workspaceWrite";

/**
 * 「开关说开着」和「这台机器上真的隔离着」之间的差额。
 *
 * 差额分支只在两者不一致、或有事要用户做时才出声：一切正常时多一行
 * "生效中"是噪音，而噪音会让真正要紧的那次提示也被略过。
 *
 * 例外是已装好时的卸载入口：它在**任何档位**下都显示（包括「不隔离」）。
 * 开关关的是会话策略，账户和凭证还留在系统里 —— 界面上分不出"关掉"和
 * "卸载"的话，真有人以为选了不隔离就是卸载了（发生过）。
 */
function SandboxReality({
  sbx,
  error,
  wanted,
  installing,
  onInstall,
  uninstalling,
  onUninstall,
}: {
  sbx: SandboxStatus | null;
  error: string;
  wanted: SandboxMode;
  installing: boolean;
  onInstall: () => void;
  uninstalling: boolean;
  onUninstall: () => void;
}) {
  const { t } = useT();
  const line = (cls: string, text: string) => (
    <p className={cls} style={{ margin: "8px 0 0" }}>
      {text}
    </p>
  );
  // 探测失败先报，且不受「选了不隔离就不出声」的约束：这说明的是应用自己
  // 有问题，和用户选了哪一档无关。
  if (error) {
    return line("form-error", t("settings.permission.sandbox.probeFailed", { error }));
  }
  if (!sbx) return null;

  const uninstallEntry = IS_WINDOWS && sbx.implemented && sbx.ready && (
    <>
      {line(
        "hint",
        wanted === "off"
          ? t("settings.permission.sandbox.installedOff")
          : t("settings.permission.sandbox.installed"),
      )}
      <div className="pack-actions" style={{ marginTop: 6 }}>
        <button disabled={uninstalling} onClick={onUninstall}>
          {uninstalling ? t("settings.permission.sandbox.waitingUac") : t("settings.permission.sandbox.uninstall")}
        </button>
      </div>
    </>
  );

  if (wanted === "off") return uninstallEntry || null;

  if (!sbx.implemented) {
    return line("hint", t("settings.permission.sandbox.unsupported"));
  }
  if (sbx.blocker?.kind === "needsElevatedInstall") {
    return (
      <>
        {line("form-error", t("settings.permission.sandbox.notInstalled"))}
        <div className="pack-actions" style={{ marginTop: 6 }}>
          <button disabled={installing} onClick={onInstall}>
            {installing ? t("settings.permission.sandbox.waitingUac") : t("settings.permission.sandbox.install")}
          </button>
        </div>
      </>
    );
  }
  if (sbx.blocker?.kind === "broken") {
    return line("form-error", t("settings.permission.sandbox.broken", { error: sbx.blocker.error }));
  }
  // 这一档在 Windows 上会整档降级成不隔离（断网要靠 WFP，而那一半没装）。
  // 不说的话，用户选了更严的档位反而什么都没得到。
  if (wanted === "workspaceWriteNoNet" && !sbx.networkIsolation) {
    return (
      <>
        {line("form-error", t("settings.permission.sandbox.noNetIsolation"))}
        {uninstallEntry}
      </>
    );
  }
  return uninstallEntry || null;
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
  const { t, tx } = useT();
  const [error, setError] = useState("");
  const current = status.config.defaultMode ?? DEFAULT_MODE;
  // 编辑期间存字符串：绑成 number 的话，用户删到空输入框会立刻变成 0，
  // 而 0 在这里的含义是"每个弹窗瞬间超时"。等失焦再解析并夹紧。
  const [timeout, setTimeout_] = useState(String(status.config.askTimeoutSecs));
  // 老配置里可能没有这个字段（后端有默认，但前端要兜一下）。
  const [turns, setTurns] = useState(String(status.config.maxTurns ?? DEFAULT_TURNS));
  const [compactAt, setCompactAt] = useState(
    String(status.config.compactThresholdTokens ?? DEFAULT_COMPACT_AT),
  );
  // 沙箱 allowRead：编辑期间存整段文本，失焦再拆行提交（一行一条，空行
  // 忽略），和 MCP 面板的参数输入同一套。老配置可能没有这个字段。
  const [allowRead, setAllowRead] = useState(
    (status.config.sandboxAllowRead ?? []).join("\n"),
  );

  // 夹紧发生时在字段旁说一声 —— 不说的话，99999 无声变 3600 像是输入被吞了。
  const [clamp, setClamp] = useState<{ key: string; bound: "max" | "min"; value: number } | null>(null);
  const clampTimer = useRef(0);
  const noteClamp = (key: string, raw: number, v: number) => {
    if (raw === v) return;
    setClamp({ key, bound: raw > v ? "max" : "min", value: v });
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
  const [uninstalling, setUninstalling] = useState(false);
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
  }, []);

  const runInstall = () => {
    askConfirm({
      title: t("settings.permission.sandbox.installConfirm.title"),
      // 两次不是笔误，要提前说 —— 不说的话第二个弹窗看起来像出了问题。
      body: t("settings.permission.sandbox.installConfirm.body"),
      confirmLabel: t("settings.permission.sandbox.installConfirm.confirm"),
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

  const runUninstall = () => {
    askConfirm({
      title: t("settings.permission.sandbox.uninstallConfirm.title"),
      body: t("settings.permission.sandbox.uninstallConfirm.body"),
      confirmLabel: t("settings.permission.sandbox.uninstallConfirm.confirm"),
      action: () => {
        setUninstalling(true);
        setError("");
        sandboxUninstall()
          .catch((e: unknown) => setError(String(e)))
          .finally(() => {
            setUninstalling(false);
            // 卸载成功后状态会翻回「还没安装」，安装入口重新出现。
            refreshSbx();
          });
      },
    });
  };

  const commitAllowRead = () => {
    const list = allowRead
      .split("\n")
      .map((s) => s.trim())
      .filter(Boolean);
    setAllowRead(list.join("\n"));
    const prev = status.config.sandboxAllowRead ?? [];
    if (list.length === prev.length && list.every((p, i) => p === prev[i])) return;
    setError("");
    setConfig({ ...status.config, sandboxAllowRead: list })
      .then(saved)
      .catch((e: unknown) => setError(String(e)));
  };

  const sandbox = status.config.sandbox ?? DEFAULT_SANDBOX;
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
        title: t("settings.permission.sandbox.offConfirm.title"),
        body: t("settings.permission.sandbox.offConfirm.body"),
        confirmLabel: t("settings.permission.sandbox.offConfirm.confirm"),
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
        title: t("settings.permission.unattendedConfirm.title"),
        body: t("settings.permission.unattendedConfirm.body"),
        confirmLabel: t("common.confirm"),
        action: () => void apply(mode),
      });
      return;
    }
    void apply(mode);
  };

  const clampText = clamp
    ? t(clamp.bound === "max" ? "settings.permission.clamp.max" : "settings.permission.clamp.min", {
        value: clamp.value,
      })
    : null;

  return (
    <>
      <Group title={t("settings.permission.defaultMode")} desc={t("settings.permission.defaultMode.desc")}>
        <div className="mode-cards" role="radiogroup" aria-label={t("settings.permission.defaultMode.aria")}>
          {MODES.map((m) => (
            <button
              key={m.id}
              role="radio"
              aria-checked={current === m.id}
              className={current === m.id ? "mode-card active" : "mode-card"}
              onClick={() => pick(m.id)}
            >
              <span className="mode-card-name">
                {t(m.labelKey)}
                {m.danger ? <span className="mode-card-flag">{t("settings.permission.highRisk")}</span> : null}
              </span>
              <span className="mode-card-desc">{t(m.descKey)}</span>
            </button>
          ))}
        </div>
      </Group>

      <Group title={t("settings.permission.sandbox")} desc={t("settings.permission.sandbox.desc")}>
        <div className="mode-cards" role="radiogroup" aria-label={t("settings.permission.sandbox")}>
          {SANDBOX_MODES.map((m) => (
            <button
              key={m.id}
              role="radio"
              aria-checked={sandbox === m.id}
              className={sandbox === m.id ? "mode-card active" : "mode-card"}
              onClick={() => pickSandbox(m.id)}
            >
              <span className="mode-card-name">
                {t(m.labelKey)}
                {m.danger ? <span className="mode-card-flag">{t("settings.permission.highRisk")}</span> : null}
              </span>
              <span className="mode-card-desc">{t(m.descKey)}</span>
            </button>
          ))}
        </div>
        <SandboxReality
          sbx={sbx}
          error={sbxError}
          wanted={sandbox}
          installing={installing}
          onInstall={runInstall}
          uninstalling={uninstalling}
          onUninstall={runUninstall}
        />
        {IS_WINDOWS && sandbox !== "off" ? (
          <Card>
            <Row title={t("settings.permission.allowRead")} desc={t("settings.permission.allowRead.desc")} stack>
              <ResizableTextarea
                className="paths-input"
                value={allowRead}
                onChange={(e) => setAllowRead(e.target.value)}
                onBlur={commitAllowRead}
                placeholder={t("settings.permission.allowRead.placeholder")}
                rows={3}
                spellCheck={false}
              />
            </Row>
          </Card>
        ) : null}
      </Group>

      <Group title={t("settings.permission.limits")}>
        <Card>
          <Row
            title={t("settings.permission.timeout")}
            desc={t("settings.permission.timeout.desc", { min: MIN_TIMEOUT, max: MAX_TIMEOUT })}
          >
            <span className="field-inline">
              <FieldNumber
                value={timeout}
                onChange={(e) => setTimeout_(e.target.value)}
                onBlur={commitTimeout}
                onKeyDown={blurOnEnter}
                aria-label={t("settings.permission.timeout.aria")}
              />
              <span className="field-unit">{t("settings.permission.unit.seconds")}</span>
            </span>
            {clamp?.key === "timeout" ? (
              <span className="clamp-note" role="status">
                {clampText}
              </span>
            ) : null}
          </Row>
          <Row
            title={t("settings.permission.turns")}
            desc={t("settings.permission.turns.desc", { min: MIN_TURNS, max: MAX_TURNS })}
          >
            <span className="field-inline">
              <FieldNumber
                value={turns}
                onChange={(e) => setTurns(e.target.value)}
                onBlur={commitTurns}
                onKeyDown={blurOnEnter}
                aria-label={t("settings.permission.turns")}
              />
              <span className="field-unit">{t("settings.permission.unit.steps")}</span>
            </span>
            {clamp?.key === "turns" ? (
              <span className="clamp-note" role="status">
                {clampText}
              </span>
            ) : null}
          </Row>
          <Row
            title={t("settings.permission.compactAt")}
            desc={tx("settings.permission.compactAt.desc", {
              noWindow: <b>{t("settings.permission.compactAt.noWindow")}</b>,
              min: numberFormat().format(MIN_COMPACT_AT),
              max: numberFormat().format(MAX_COMPACT_AT),
            })}
          >
            <span className="field-inline">
              <FieldNumber
                value={compactAt}
                onChange={(e) => setCompactAt(e.target.value)}
                onBlur={commitCompactAt}
                onKeyDown={blurOnEnter}
                aria-label={t("settings.permission.compactAt.aria")}
              />
              <span className="field-unit">token</span>
            </span>
            {clamp?.key === "compactAt" ? (
              <span className="clamp-note" role="status">
                {clampText}
              </span>
            ) : null}
          </Row>
        </Card>
      </Group>

      <Group title={t("settings.permission.memory")}>
        <Card>
          <Row title={t("settings.permission.recall")} desc={t("settings.permission.recall.desc")}>
            <Switch
              on={status.config.sessionRecall ?? true}
              label={t("settings.permission.recall")}
              onChange={(v) => {
                setError("");
                setConfig({ ...status.config, sessionRecall: v })
                  .then(saved)
                  .catch((e: unknown) => setError(String(e)));
              }}
            />
          </Row>
        </Card>
      </Group>

      <Group title={t("settings.permission.rules")}>
        <Card>
          <CardBlock>
            <p className="hint" style={{ margin: 0 }}>
              {tx("settings.permission.rules.hint", { example: <code>Bash(npm run *)</code> })}
            </p>
          </CardBlock>
        </Card>
      </Group>

      {error ? <FormError text={error} /> : null}
    </>
  );
}
