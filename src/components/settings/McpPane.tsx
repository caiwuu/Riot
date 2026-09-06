import { useEffect, useState } from "react";

import {
  type AppConfig,
  type ConfigStatus,
  type McpServerConfig,
  type McpServerStatus,
  mcpExportJson,
  mcpImportJson,
  mcpRestart,
  mcpStatus,
  renderUiError,
  setConfig,
} from "../../bridge";
import { useT } from "../../i18n";
import { ResizableTextarea } from "../ResizableTextarea";
import { Card, CardBlock, Group, Row } from "./layout";
import { type AskConfirm, type LeaveGuard, FormError, Switch, blurOnEnter } from "./shared";

/**
 * MCP 服务器管理。
 *
 * 配置改动走 setConfig（宿主保存后自动 reconcile 连接）；连接状态另有
 * 一条只读通道（mcpStatus），打开本页时轮询 —— 配置是"想要什么"，
 * 状态是"现在是什么"，两者永远可能不一致（正在连、连失败了）。
 */
export function McpPane({
  status,
  onStatus,
  askConfirm,
  registerLeaveGuard,
  onSaved,
}: {
  status: ConfigStatus;
  onStatus: (s: ConfigStatus) => void;
  askConfirm: AskConfirm;
  registerLeaveGuard: (g: LeaveGuard | null) => void;
  onSaved: () => void;
}) {
  const { t, tn, tx } = useT();
  const cfg = status.config;
  const servers = cfg.mcpServers;
  const [selId, setSelId] = useState(servers[0]?.id ?? "");
  /** 刚新建的服务器：编辑器聚焦到名称，省得用户自己找第一个待填字段。 */
  const [justAdded, setJustAdded] = useState("");
  const [error, setError] = useState("");
  const [live, setLive] = useState<McpServerStatus[]>([]);
  /** null = 表单视图；字符串 = JSON 视图的编辑内容。 */
  const [jsonDraft, setJsonDraft] = useState<string | null>(null);
  /** 打开 JSON 视图那一刻的导出值。和它相同 = 没改过，关掉不用问。 */
  const [jsonBase, setJsonBase] = useState("");
  const [jsonBusy, setJsonBusy] = useState(false);

  const sel = servers.find((s) => s.id === selId) ?? servers[0];

  // 用户粘了一整段 JSON、还没保存 —— 关设置或切分区前得先问一句。
  // 这可能是他花了几分钟从 README 里拼出来的东西。
  const jsonDirty = jsonDraft !== null && jsonDraft !== jsonBase;
  useEffect(() => {
    registerLeaveGuard(
      jsonDirty
        ? () => ({
            title: t("settings.mcp.leave.title"),
            body: t("settings.mcp.leave.body"),
            confirmLabel: t("settings.mcp.leave.discard"),
          })
        : null,
    );
    return () => registerLeaveGuard(null);
  }, [jsonDirty, registerLeaveGuard, t]);

  const openJson = async () => {
    setError("");
    try {
      const s = await mcpExportJson();
      setJsonBase(s);
      setJsonDraft(s);
    } catch (e) {
      setError(String(e));
    }
  };

  const applyJson = async () => {
    if (jsonDraft === null) return;
    setJsonBusy(true);
    setError("");
    try {
      const s = await mcpImportJson(jsonDraft);
      onStatus(s);
      onSaved();
      setJsonDraft(null);
      setSelId(s.config.mcpServers[0]?.id ?? "");
    } catch (e) {
      // 留在 JSON 视图里报错 —— 关掉的话用户就丢了刚粘的内容
      setError(String(e));
    } finally {
      setJsonBusy(false);
    }
  };

  // 打开本页时轮询连接状态。2.5s 是"点了重连能很快看到变化"和
  // "别对着宿主刷屏"之间的折中。
  useEffect(() => {
    let stopped = false;
    const pull = async () => {
      try {
        const s = await mcpStatus();
        if (!stopped) setLive(s);
      } catch {
        // 拿不到状态就保持上一份 —— 状态点短暂过时无伤大雅
      }
    };
    void pull();
    const timer = setInterval(() => void pull(), 2500);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, []);

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

  const patchSel = (patch: Partial<McpServerConfig>) => {
    if (!sel) return;
    void commit({
      ...cfg,
      mcpServers: servers.map((s) => (s.id === sel.id ? { ...s, ...patch } : s)),
    });
  };

  const addServer = () => {
    // id 是工具名和权限规则的一部分，给一个能用但明显该改的默认值。
    let n = servers.length + 1;
    while (servers.some((s) => s.id === `server-${n}`)) n += 1;
    const id = `server-${n}`;
    const s: McpServerConfig = { id, command: "", args: [], env: {}, enabled: true };
    void commit({ ...cfg, mcpServers: [...servers, s] }).then((ok) => {
      if (ok) {
        setSelId(id);
        setJustAdded(id);
      }
    });
  };

  const removeServer = () => {
    if (!sel) return;
    const target = sel;
    askConfirm({
      title: t("settings.mcp.remove.title", { name: target.name || target.id }),
      body: t("settings.mcp.remove.body"),
      confirmLabel: t("common.delete"),
      action: () => {
        const rest = servers.filter((s) => s.id !== target.id);
        void commit({ ...cfg, mcpServers: rest }).then((ok) => {
          if (ok) setSelId(rest[0]?.id ?? "");
        });
      },
    });
  };

  // JSON 视图：显示并编辑标准格式（{"mcpServers": {...}}），整体替换。
  if (jsonDraft !== null) {
    return (
      <Group
        title={t("settings.mcp.json.title")}
        desc={tx("settings.mcp.json.desc", { code: <code>mcpServers</code> })}
      >
        <ResizableTextarea
          className="mcp-json-input"
          value={jsonDraft}
          onChange={(e) => setJsonDraft(e.target.value)}
          rows={16}
          spellCheck={false}
        />
        <div className="editor-foot">
          {error ? <span className="test-result err">{error}</span> : <span />}
          <div className="editor-foot-actions">
            <button
              onClick={() => {
                const back = () => {
                  setJsonDraft(null);
                  setError("");
                };
                if (jsonDirty) {
                  askConfirm({
                    title: t("settings.mcp.leave.title"),
                    body: t("settings.mcp.leave.backBody"),
                    confirmLabel: t("settings.mcp.leave.discard"),
                    action: back,
                  });
                } else {
                  back();
                }
              }}
              disabled={jsonBusy}
            >
              {t("common.cancel")}
            </button>
            <button className="primary" onClick={() => void applyJson()} disabled={jsonBusy}>
              {jsonBusy ? t("common.saving") : t("common.save")}
            </button>
          </div>
        </div>
      </Group>
    );
  }

  if (!sel) {
    return (
      <Group title={t("settings.mcp.servers")}>
        <div className="empty-state">
          <p className="empty-title">{t("settings.mcp.empty.title")}</p>
          <p className="hint">{t("settings.mcp.empty.hint")}</p>
          <div className="empty-actions">
            <button className="primary" onClick={addServer}>
              {t("settings.mcp.addServer")}
            </button>
            <button onClick={() => void openJson()}>{t("settings.mcp.pasteJson")}</button>
          </div>
          {error ? <p className="form-error">{error}</p> : null}
        </div>
      </Group>
    );
  }

  return (
    <>
      <Group
        title={t("settings.mcp.servers")}
        desc={tx("settings.mcp.servers.desc", {
          pattern: <code>{t("settings.mcp.toolNamePattern")}</code>,
        })}
        action={
          <div className="set-group-actions">
            <button className="btn-compact" onClick={addServer}>
              {t("common.add")}
            </button>
            <button
              className="btn-compact"
              onClick={() => void openJson()}
              title={t("settings.mcp.jsonView.title")}
            >
              JSON
            </button>
          </div>
        }
      >
        {/* 竖排列表而不是 pill 铺排：几十个服务器时 pill 会糊成一片，
            长名字还会把整行撑爆。行内名字省略号截断，超高滚动。 */}
        <ul className="mcp-list">
          {servers.map((s) => {
            const st = live.find((l) => l.id === s.id);
            const state = s.enabled === false ? "off" : (st?.state ?? "off");
            const meta =
              s.enabled === false
                ? t("common.disabled")
                : state === "connected"
                  ? tn("settings.mcp.toolCount", st?.tools.length ?? 0)
                  : state === "connecting"
                    ? t("settings.mcp.status.connecting")
                    : state === "failed"
                      ? t("settings.mcp.status.failedShort")
                      : "";
            return (
              <li key={s.id}>
                <button
                  className={s.id === sel.id ? "mcp-row active" : "mcp-row"}
                  onClick={() => {
                    setSelId(s.id);
                    setJustAdded("");
                  }}
                  title={s.name || s.id}
                >
                  <span className={`mcp-dot ${state}`} />
                  <span className="mcp-row-name">{s.name || s.id}</span>
                  {meta ? <span className="mcp-row-meta">{meta}</span> : null}
                </button>
              </li>
            );
          })}
        </ul>
      </Group>

      <McpServerEditor
        key={sel.id}
        server={sel}
        live={live.find((l) => l.id === sel.id) ?? null}
        autoFocusName={sel.id === justAdded}
        onPatch={patchSel}
        onRemove={removeServer}
        onError={setError}
      />

      {error ? <FormError text={error} /> : null}
    </>
  );
}

/** 已连接服务器的工具名默认只铺这么多个，其余收起 —— 几十个全量平铺会把配置区挤到两屏外。 */
const MCP_TOOLS_SHOWN = 12;

/** `mcp__better-icons__search_icons` → `search_icons`。前缀每条都一样，铺出来只添噪音。 */
function mcpToolShortName(full: string): string {
  const parts = full.split("__");
  if (parts[0] === "mcp" && parts.length >= 3) return parts.slice(2).join("__");
  return full;
}

function McpServerEditor({
  server,
  live,
  autoFocusName,
  onPatch,
  onRemove,
  onError,
}: {
  server: McpServerConfig;
  live: McpServerStatus | null;
  /** 刚新建时聚焦名称输入框。 */
  autoFocusName?: boolean;
  onPatch: (p: Partial<McpServerConfig>) => void;
  onRemove: () => void;
  onError: (e: string) => void;
}) {
  const { t, tn } = useT();
  const [name, setName] = useState(server.name ?? "");
  const [command, setCommand] = useState(server.command);
  const [args, setArgs] = useState((server.args ?? []).join("\n"));
  const [env, setEnv] = useState(
    Object.entries(server.env ?? {})
      .map(([k, v]) => `${k}=${v}`)
      .join("\n"),
  );
  const [restarting, setRestarting] = useState(false);
  const [toolsOpen, setToolsOpen] = useState(false);

  const commitArgs = () => {
    const list = args
      .split("\n")
      .map((a) => a.trim())
      .filter(Boolean);
    if (JSON.stringify(list) !== JSON.stringify(server.args ?? [])) onPatch({ args: list });
  };

  const commitEnv = () => {
    const map: Record<string, string> = {};
    for (const line of env.split("\n")) {
      const item = line.trim();
      if (!item) continue;
      const eq = item.indexOf("=");
      if (eq <= 0) {
        onError(t("settings.mcp.env.format", { line: item }));
        return;
      }
      map[item.slice(0, eq).trim()] = item.slice(eq + 1).trim();
    }
    if (JSON.stringify(map) !== JSON.stringify(server.env ?? {})) onPatch({ env: map });
  };

  const doRestart = async () => {
    setRestarting(true);
    try {
      await mcpRestart(server.id);
    } catch (e) {
      onError(String(e));
    } finally {
      setRestarting(false);
    }
  };

  const state = server.enabled === false ? "off" : (live?.state ?? "off");
  const stateText: Record<string, string> = {
    connected: tn("settings.mcp.status.connected", live?.tools.length ?? 0, {
      detail: live?.detail ? ` · ${live.detail}` : "",
    }),
    connecting: t("settings.mcp.status.connecting"),
    failed: t("settings.mcp.status.failed", {
      reason: live?.error
        ? renderUiError(live.error)
        : live?.detail || t("settings.mcp.status.unknownReason"),
    }),
    off:
      server.enabled === false
        ? t("common.disabled")
        : server.command.trim()
          ? t("settings.mcp.status.notStarted")
          : t("settings.mcp.status.needCommand"),
  };

  return (
    <Group
      title={server.name || server.id}
      action={
        <div className="set-group-actions">
          <button className="btn-compact ghost-danger" onClick={onRemove}>
            {t("settings.mcp.removeServer")}
          </button>
        </div>
      }
    >
      <Card>
        <Row title={t("common.enable")} desc={t("settings.mcp.enable.desc")}>
          <Switch
            on={server.enabled !== false}
            onChange={(v) => onPatch({ enabled: v })}
            label={t("settings.mcp.enable.label")}
          />
        </Row>
        <CardBlock>
          <div className={`mcp-status ${state}`}>
            <span className={`mcp-dot ${state}`} />
            <span className="mcp-status-text">{stateText[state]}</span>
            {server.enabled !== false ? (
              <button className="ghost" onClick={() => void doRestart()} disabled={restarting}>
                {restarting ? t("settings.mcp.reconnecting") : t("settings.mcp.reconnect")}
              </button>
            ) : null}
          </div>
          {state === "connected" && live && live.tools.length > 0 ? (
            <ul className="mcp-tools">
              {(toolsOpen ? live.tools : live.tools.slice(0, MCP_TOOLS_SHOWN)).map((tool) => (
                <li key={tool}>
                  <span className="mcp-tool-chip" title={tool}>
                    {mcpToolShortName(tool)}
                  </span>
                </li>
              ))}
              {live.tools.length > MCP_TOOLS_SHOWN ? (
                <li>
                  <button
                    type="button"
                    className="mcp-tools-more"
                    onClick={() => setToolsOpen(!toolsOpen)}
                  >
                    {toolsOpen
                      ? t("common.less")
                      : tn("settings.mcp.moreTools", live.tools.length - MCP_TOOLS_SHOWN)}
                  </button>
                </li>
              ) : null}
            </ul>
          ) : null}
        </CardBlock>
        <Row title={t("settings.mcp.name")} desc={t("settings.mcp.name.desc")}>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            onBlur={() => name.trim() !== (server.name ?? "") && onPatch({ name: name.trim() })}
            onKeyDown={blurOnEnter}
            autoFocus={autoFocusName}
            placeholder={server.id}
            spellCheck={false}
            aria-label={t("settings.mcp.name")}
          />
        </Row>
        <Row title={t("settings.mcp.command")}>
          <input
            value={command}
            onChange={(e) => setCommand(e.target.value)}
            onBlur={() => command.trim() !== server.command && onPatch({ command: command.trim() })}
            onKeyDown={blurOnEnter}
            placeholder={t("settings.mcp.command.placeholder")}
            spellCheck={false}
            aria-label={t("settings.mcp.command")}
          />
        </Row>
        <Row title={t("settings.mcp.args")} desc={t("settings.mcp.args.desc")} stack>
          <ResizableTextarea
            className="paths-input"
            value={args}
            onChange={(e) => setArgs(e.target.value)}
            onBlur={commitArgs}
            placeholder={t("settings.mcp.args.placeholder")}
            rows={4}
            spellCheck={false}
            aria-label={t("settings.mcp.args")}
          />
        </Row>
        <Row title={t("settings.mcp.env")} desc={t("settings.mcp.env.desc")} stack>
          <ResizableTextarea
            className="paths-input"
            value={env}
            onChange={(e) => setEnv(e.target.value)}
            onBlur={commitEnv}
            placeholder={t("settings.mcp.env.placeholder")}
            rows={2}
            spellCheck={false}
            aria-label={t("settings.mcp.env")}
          />
        </Row>
      </Card>
    </Group>
  );
}
